//! The preview's frame decoding thread.
//!
//! One [`FrameSource`] (a persistent ffmpeg session in production) serves
//! every preview frame of the loaded video. It lives on a dedicated thread
//! fed by a channel: requests are answered in order, the source is replaced
//! when a request carries a new video generation, and the thread exits when
//! the [`FrameWorker`] is dropped. A source unused for [`IDLE_RELEASE`] is
//! closed. A panic while decoding is reported as an error (see
//! [`workers::guarded`]) and discards the source.
//!
//! [`Activity`] lets filmstrip jobs, which start their own ffmpeg processes,
//! step aside while the user is scrubbing.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use framecover_core::VideoFile;
use framecover_core::frame_stream::FrameSource;
use image::RgbImage;

use crate::services::FrameSourceFactory;
use crate::workers;

/// An unused source is closed after this long, freeing its decoder process,
/// its frame cache and its handle on the video file.
const IDLE_RELEASE: Duration = Duration::from_secs(30);

/// Receives a request's outcome on the worker thread.
pub type FrameReply = Box<dyn FnOnce(Result<RgbImage, String>) + Send>;

enum Message {
    Frame {
        video: Arc<VideoFile>,
        position_ms: u64,
        generation: u64,
        reply: FrameReply,
    },
    /// Drop the current source, then acknowledge.
    Release(Sender<()>),
}

/// Owner of the frame decoding thread.
pub struct FrameWorker {
    sender: Option<Sender<Message>>,
    thread: Option<JoinHandle<()>>,
    activity: Arc<Activity>,
}

impl FrameWorker {
    /// Start the thread; `factory` creates a source for each loaded video.
    pub fn spawn(factory: FrameSourceFactory) -> Self {
        let (sender, receiver) = mpsc::channel();
        let activity = Arc::new(Activity::default());
        let thread_activity = Arc::clone(&activity);
        let thread = std::thread::Builder::new()
            .name("frame-source".into())
            .spawn(move || serve(&receiver, &factory, &thread_activity))
            .map_err(|err| log::error!("could not start the frame decoding thread: {err}"))
            .ok();
        Self {
            sender: thread.is_some().then_some(sender),
            thread,
            activity,
        }
    }

    /// Queue a frame request; `reply` runs on the worker thread.
    pub fn request(
        &self,
        video: Arc<VideoFile>,
        position_ms: u64,
        generation: u64,
        reply: FrameReply,
    ) {
        let message = Message::Frame {
            video,
            position_ms,
            generation,
            reply,
        };
        self.activity.begin();
        let unsent = match &self.sender {
            Some(sender) => sender.send(message).err().map(|e| e.0),
            None => Some(message),
        };
        if let Some(Message::Frame { reply, .. }) = unsent {
            self.activity.end();
            reply(Err("The frame decoding thread is not running.".into()));
        }
    }

    /// A handle that closes the current source from any thread.
    pub fn releaser(&self) -> Releaser {
        Releaser(self.sender.clone())
    }

    /// Shared record of when frames are being decoded.
    pub fn activity(&self) -> Arc<Activity> {
        Arc::clone(&self.activity)
    }
}

impl Drop for FrameWorker {
    fn drop(&mut self) {
        // Closing the channel ends the thread, which stops its decoder.
        self.sender = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Closes the frame source, e.g. before the video file is replaced on disk
/// (a running decoder keeps it open, which blocks the replacement on Windows).
#[derive(Clone)]
pub struct Releaser(Option<Sender<Message>>);

impl Releaser {
    /// Drop the current source and wait until that has happened. The next
    /// request opens a new one.
    pub fn release(&self) {
        let Some(sender) = &self.0 else { return };
        let (ack, done) = mpsc::channel();
        if sender.send(Message::Release(ack)).is_ok() {
            let _ = done.recv();
        }
    }
}

/// The worker thread's loop.
fn serve(receiver: &Receiver<Message>, factory: &FrameSourceFactory, activity: &Activity) {
    let mut current: Option<(u64, Box<dyn FrameSource>)> = None;
    loop {
        let received = if current.is_some() {
            receiver.recv_timeout(IDLE_RELEASE)
        } else {
            receiver.recv().map_err(RecvTimeoutError::from)
        };
        let message = match received {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => {
                current = None;
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match message {
            Message::Release(ack) => {
                current = None;
                let _ = ack.send(());
            }
            Message::Frame {
                video,
                position_ms,
                generation,
                reply,
            } => {
                let outcome = workers::guarded(|| {
                    let source = source_for(&mut current, factory, &video, generation);
                    workers::run_frame_at(source.as_mut(), position_ms)
                });
                let outcome = outcome.unwrap_or_else(|details| {
                    // The source may be half-way through a frame; start over.
                    current = None;
                    Err(details)
                });
                activity.end();
                reply(outcome);
            }
        }
    }
}

/// The source for `generation`, replacing one for an older video.
fn source_for<'a>(
    current: &'a mut Option<(u64, Box<dyn FrameSource>)>,
    factory: &FrameSourceFactory,
    video: &VideoFile,
    generation: u64,
) -> &'a mut Box<dyn FrameSource> {
    let entry = match current.take() {
        Some(entry) if entry.0 == generation => current.insert(entry),
        old => {
            // Stop the previous video's decoder before starting another.
            drop(old);
            current.insert((generation, factory(video)))
        }
    };
    &mut entry.1
}

/// Tracks queued and running frame requests so that background work can
/// yield to them.
#[derive(Default)]
pub struct Activity {
    state: Mutex<ActivityState>,
    changed: Condvar,
}

#[derive(Default)]
struct ActivityState {
    pending: usize,
    last_finished: Option<Instant>,
}

impl Activity {
    fn begin(&self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pending += 1;
    }

    fn end(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.pending = state.pending.saturating_sub(1);
        state.last_finished = Some(Instant::now());
        self.changed.notify_all();
    }

    /// Block until no frame request is pending and none finished within
    /// `quiet` (scrubbing sends them back to back), or `max_wait` passed.
    pub fn wait_until_quiet(&self, quiet: Duration, max_wait: Duration) {
        let deadline = Instant::now() + max_wait;
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            let now = Instant::now();
            let quiet_left = state
                .last_finished
                .map_or(Duration::ZERO, |t| quiet.saturating_sub(now - t));
            if (state.pending == 0 && quiet_left.is_zero()) || now >= deadline {
                return;
            }
            let mut timeout = deadline - now;
            if state.pending == 0 {
                timeout = timeout.min(quiet_left);
            }
            state = self
                .changed
                .wait_timeout(state, timeout)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filmstrip::tests::video;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A source returning a 1x1 frame whose red channel is `position_ms`,
    /// panicking at 13.
    struct CountingSource;

    impl FrameSource for CountingSource {
        fn frame_at(&mut self, offset_ms: u64) -> framecover_core::Result<RgbImage> {
            assert_ne!(offset_ms, 13, "unlucky frame");
            Ok(RgbImage::from_pixel(
                1,
                1,
                image::Rgb([offset_ms as u8, 0, 0]),
            ))
        }
    }

    fn worker() -> (FrameWorker, Arc<AtomicUsize>) {
        let created = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&created);
        let factory: FrameSourceFactory = Arc::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Box::new(CountingSource)
        });
        (FrameWorker::spawn(factory), created)
    }

    fn request(worker: &FrameWorker, position_ms: u64, generation: u64) -> Result<u8, String> {
        let (sender, receiver) = mpsc::channel();
        worker.request(
            Arc::new(video(5000)),
            position_ms,
            generation,
            Box::new(move |outcome| {
                let _ = sender.send(outcome);
            }),
        );
        receiver
            .recv()
            .unwrap()
            .map(|image| image.get_pixel(0, 0).0[0])
    }

    #[test]
    fn one_source_per_generation() {
        let (worker, created) = worker();
        assert_eq!(request(&worker, 5, 1), Ok(5));
        assert_eq!(request(&worker, 6, 1), Ok(6));
        assert_eq!(created.load(Ordering::SeqCst), 1);
        assert_eq!(request(&worker, 7, 2), Ok(7));
        assert_eq!(created.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn release_closes_the_source() {
        let (worker, created) = worker();
        assert_eq!(request(&worker, 5, 1), Ok(5));
        worker.releaser().release();
        assert_eq!(request(&worker, 5, 1), Ok(5));
        assert_eq!(created.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn panics_become_errors_and_reset_the_source() {
        let (worker, created) = worker();
        let err = request(&worker, 13, 1).unwrap_err();
        assert!(err.contains("unlucky frame"), "{err}");
        assert_eq!(request(&worker, 14, 1), Ok(14));
        assert_eq!(created.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn requests_after_shutdown_fail() {
        let (mut worker, _) = worker();
        worker.sender = None;
        assert!(request(&worker, 1, 1).is_err());
        worker.releaser().release();
    }

    #[test]
    fn quiet_wait_yields_to_requests() {
        let activity = Arc::new(Activity::default());
        let started = Instant::now();
        activity.wait_until_quiet(Duration::from_secs(5), Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(1));

        activity.begin();
        let ending = Arc::clone(&activity);
        let ender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            ending.end();
        });
        let started = Instant::now();
        activity.wait_until_quiet(Duration::from_millis(30), Duration::from_secs(5));
        let waited = started.elapsed();
        ender.join().unwrap();
        assert!(waited >= Duration::from_millis(80), "{waited:?}");
        assert!(waited < Duration::from_secs(2), "{waited:?}");

        // Never longer than `max_wait`.
        activity.begin();
        let started = Instant::now();
        activity.wait_until_quiet(Duration::ZERO, Duration::from_millis(30));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
