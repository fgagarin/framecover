//! In-process decoding sessions (`libav` feature).
//!
//! Same contract as the ffmpeg-child [`super::Session`]: decoding starts at
//! an accurate seek and frames come out with their absolute timestamps. The
//! decoder reports those timestamps directly, so there is no output shift to
//! undo, and dropping a session just frees the decoder.

use std::path::Path;

use image::RgbImage;

use super::Clock;
use crate::color;

pub(super) struct NativeSession {
    decoder: framecover_libav::Decoder,
    clock: Clock,
    /// Set by the stream once the clock is known. ffmpeg sessions add it to
    /// their shifted timestamps; these timestamps are already absolute.
    pub(super) origin: Option<i64>,
    /// Every frame from this tick up to the last one read has been read.
    pub(super) covered_from: i64,
    pub(super) last_tick: Option<i64>,
    pub(super) frames_read: u64,
    pub(super) eof: bool,
}

impl NativeSession {
    pub(super) fn spawn(path: &Path, seek_us: u64) -> Result<Self, String> {
        let mut decoder = framecover_libav::Decoder::open(path, color::display_filter_for(path))?;
        let (num, den) = decoder.time_base();
        let clock = Clock {
            num,
            den,
            start_us: decoder.start_us(),
        };
        if seek_us > 0 {
            decoder.seek(seek_us, Some(clock.threshold(seek_us)));
        }
        Ok(Self {
            decoder,
            clock,
            origin: None,
            covered_from: i64::MIN,
            last_tick: None,
            frames_read: 0,
            eof: false,
        })
    }

    /// The session's clock, known from the stream as soon as it is open (an
    /// ffmpeg session reports it with the first decoded frame, even one the
    /// seek drops, so a seek past the last frame still has a clock).
    pub(super) fn clock(&mut self) -> Result<Option<Clock>, String> {
        Ok(Some(self.clock))
    }

    /// The next frame and its absolute timestamp; `None` at the end.
    pub(super) fn next_frame(&mut self) -> Result<Option<(i64, RgbImage)>, String> {
        if self.eof {
            return Ok(None);
        }
        let Some((tick, frame)) = self.decode()? else {
            self.eof = true;
            return Ok(None);
        };
        self.frames_read += 1;
        self.last_tick = Some(tick);
        Ok(Some((tick, frame)))
    }

    fn decode(&mut self) -> Result<Option<(i64, RgbImage)>, String> {
        let Some(frame) = self.decoder.next_frame()? else {
            return Ok(None);
        };
        let pts = frame.pts.ok_or("frame without a timestamp")?;
        let image = RgbImage::from_raw(frame.width, frame.height, frame.data)
            .ok_or("frame size does not match its data")?;
        Ok(Some((pts, image)))
    }
}
