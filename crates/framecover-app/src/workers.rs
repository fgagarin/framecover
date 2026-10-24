//! Off-UI-thread jobs (port of `ui/worker.py`).
//!
//! Each `run_*` function is the body of one Python `QRunnable`; they are
//! plain functions over the service traits so they can be tested with
//! fakes. [`spawn_guarded`] runs a body on a background thread, turning a
//! panic into an error string (so it surfaces in the error dialog instead
//! of killing the app) and hands the outcome back to the UI thread.

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::Instant;

use framecover_core::{ApplyResult, Error, UpdateInfo, VideoFile};
use image::RgbImage;

use crate::error_report::error_details;
use crate::services::{Services, UpdateChecker, VideoConverter};

thread_local! {
    static GUARDED: Cell<bool> = const { Cell::new(false) };
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// True while the current thread runs inside [`guarded`]; the global panic
/// hook then records the panic instead of opening the unhandled-error dialog.
pub fn in_guarded_section() -> bool {
    GUARDED.with(Cell::get)
}

/// Called by the panic hook to hand the formatted panic to [`guarded`].
pub fn record_panic(details: String) {
    LAST_PANIC.with(|slot| *slot.borrow_mut() = Some(details));
}

/// Run `f`, converting a panic into `Err(details)`.
pub fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    let previous = GUARDED.with(|g| g.replace(true));
    let result = catch_unwind(AssertUnwindSafe(f));
    GUARDED.with(|g| g.set(previous));
    result.map_err(|payload| {
        LAST_PANIC
            .with(|slot| slot.borrow_mut().take())
            .unwrap_or_else(|| panic_message(&*payload))
    })
}

/// Text of a panic payload.
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic with a non-string payload".to_owned()
    }
}

/// Run `job` on a new named thread; `done` receives its outcome (or the
/// panic details) on that thread and is expected to post it to the UI.
pub fn spawn_guarded<T: 'static>(
    name: &str,
    job: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(Result<T, String>) + Send + 'static,
) {
    let spawned = std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            done(guarded(job));
        });
    if let Err(err) = spawned {
        log::error!("could not start worker thread {name}: {err}");
    }
}

fn details(err: &Error) -> String {
    log::error!("Video operation failed: {err}");
    error_details(err)
}

/// Outcome of loading a video.
#[derive(Debug)]
pub enum LoadOutcome {
    Loaded(Box<VideoFile>),
    /// The container is not supported for editing; offer conversion.
    Unsupported(PathBuf),
    Failed(String),
}

pub fn run_load(services: &Services, path: &Path) -> LoadOutcome {
    match services.loader.load(path) {
        Ok(video) => LoadOutcome::Loaded(Box::new(video)),
        Err(Error::UnsupportedFormat(_)) => LoadOutcome::Unsupported(path.to_owned()),
        Err(err) => LoadOutcome::Failed(details(&err)),
    }
}

pub fn run_extract(
    services: &Services,
    video: &VideoFile,
    position_ms: u64,
) -> Result<RgbImage, String> {
    services
        .extractor
        .extract(video, position_ms)
        .map_err(|e| details(&e))
}

pub fn run_convert(
    converter: &dyn VideoConverter,
    source: &Path,
    destination: &Path,
    on_progress: &mut dyn FnMut(&str),
) -> Result<PathBuf, String> {
    converter
        .convert(source, destination, true, on_progress)
        .map_err(|e| details(&e))
}

/// Message shown when the update check fails for an unexpected reason.
pub const UNEXPECTED_UPDATE_FAILURE: &str = "Something went wrong while looking for updates. \
FrameCover keeps working normally; click the update indicator to try again.";

pub fn run_update_check(
    checker: &dyn UpdateChecker,
    current_version: &str,
) -> Result<UpdateInfo, String> {
    match checker.check(current_version) {
        Ok(info) => Ok(info),
        Err(Error::UpdateCheck(message)) => Err(message),
        Err(err) => {
            log::error!("Unexpected update check failure: {err}");
            Err(UNEXPECTED_UPDATE_FAILURE.to_owned())
        }
    }
}

/// A save request: in place when `destination` is `None`, else a copy.
#[derive(Debug, Clone)]
pub struct SaveJob {
    pub video: std::sync::Arc<VideoFile>,
    pub thumbnail: std::sync::Arc<RgbImage>,
    pub position_ms: Option<u64>,
    pub destination: Option<PathBuf>,
    pub overwrite: bool,
}

pub const CACHE_STAGE: &str = "Refreshing thumbnail cache";

/// Embed the thumbnail (in place or as a copy), then refresh the
/// file-manager cache on success.
pub fn run_apply(
    services: &Services,
    job: &SaveJob,
    on_progress: &mut dyn FnMut(&str, Option<f64>),
) -> ApplyResult {
    let result = match &job.destination {
        None => services.writer.write(
            &job.video,
            &job.thumbnail,
            job.position_ms,
            Some(&mut *on_progress),
        ),
        Some(destination) => services.copier.save_copy(
            services.writer.as_ref(),
            &job.video,
            &job.thumbnail,
            destination,
            job.overwrite,
            job.position_ms,
            Some(&mut *on_progress),
        ),
    };
    if result.success {
        let started = Instant::now();
        on_progress(CACHE_STAGE, None);
        let path = result.output_path.as_deref().unwrap_or(&job.video.path);
        services.invalidator.invalidate(path, Some(&job.thumbnail));
        on_progress(CACHE_STAGE, Some(started.elapsed().as_secs_f64()));
    }
    result
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::filmstrip::tests::video;
    use crate::services::fakes::*;
    use framecover_core::{ApplyError, UpdateStatus};
    use std::sync::Arc;

    pub(crate) struct Fakes {
        pub writer: Arc<FakeWriter>,
        pub copier: Arc<FakeCopier>,
        pub invalidator: Arc<FakeInvalidator>,
    }

    struct NullDesktop;
    impl crate::services::Desktop for NullDesktop {
        fn reveal_in_file_manager(&self, _path: &Path) {}
        fn open_url(&self, _url: &str) -> bool {
            true
        }
    }

    pub(crate) fn services() -> (Services, Fakes) {
        let writer = Arc::new(FakeWriter::default());
        let copier = Arc::new(FakeCopier::default());
        let invalidator = Arc::new(FakeInvalidator::default());
        let services = Services {
            loader: Arc::new(FakeLoader(Box::new(|p| {
                if p.extension().is_some_and(|e| e == "wmv") {
                    Err(Error::UnsupportedFormat(p.to_owned()))
                } else if p.exists() || p.starts_with("/fake") {
                    Ok(VideoFile {
                        path: p.to_owned(),
                        ..video(5000)
                    })
                } else {
                    Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "missing",
                    )))
                }
            }))),
            extractor: Arc::new(FakeExtractor(Box::new(|_, ms| {
                if ms == 666 {
                    Err(Error::Extraction {
                        path: "/fake/video.mp4".into(),
                        offset_ms: ms,
                        cause: "decode fail".into(),
                    })
                } else {
                    Ok(RgbImage::new(32, 32))
                }
            }))),
            writer: writer.clone(),
            copier: copier.clone(),
            converter: Arc::new(FakeConverter(Box::new(|_, d| Ok(d.to_owned())))),
            update_checker: None,
            invalidator: invalidator.clone(),
            desktop: Arc::new(NullDesktop),
        };
        (
            services,
            Fakes {
                writer,
                copier,
                invalidator,
            },
        )
    }

    fn job(destination: Option<&str>) -> SaveJob {
        SaveJob {
            video: Arc::new(video(5000)),
            thumbnail: Arc::new(RgbImage::new(8, 8)),
            position_ms: Some(1234),
            destination: destination.map(PathBuf::from),
            overwrite: false,
        }
    }

    #[test]
    fn unsupported_format_requests_conversion() {
        let (s, _) = services();
        assert!(
            matches!(run_load(&s, Path::new("/fake/a.wmv")), LoadOutcome::Unsupported(p) if p == Path::new("/fake/a.wmv"))
        );
    }

    #[test]
    fn load_success_and_failure() {
        let (s, _) = services();
        assert!(matches!(
            run_load(&s, Path::new("/fake/a.mp4")),
            LoadOutcome::Loaded(_)
        ));
        match run_load(&s, Path::new("Z:/definitely/missing.mp4")) {
            LoadOutcome::Failed(details) => assert!(details.contains("missing")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn extract_error_has_details() {
        let (s, _) = services();
        assert!(run_extract(&s, &video(5000), 10).is_ok());
        let err = run_extract(&s, &video(5000), 666).unwrap_err();
        assert!(err.contains("decode fail"));
    }

    #[test]
    fn conversion_reports_output_and_progress() {
        let converter = FakeConverter(Box::new(|_, d| Ok(d.to_owned())));
        let mut progress = Vec::new();
        let out = run_convert(
            &converter,
            Path::new("a.wmv"),
            Path::new("a.mp4"),
            &mut |m| progress.push(m.to_owned()),
        );
        assert_eq!(out.unwrap(), PathBuf::from("a.mp4"));
        assert_eq!(progress, ["Remuxing to MP4"]);
        let failing = FakeConverter(Box::new(|_, _| Err(Error::Conversion("boom".into()))));
        assert!(
            run_convert(&failing, Path::new("a"), Path::new("b"), &mut |_| {})
                .unwrap_err()
                .contains("boom")
        );
    }

    #[test]
    fn apply_emits_save_and_cache_progress_and_invalidates() {
        let (s, f) = services();
        let mut stages = Vec::new();
        let result = run_apply(&s, &job(None), &mut |name, t| {
            stages.push((name.to_owned(), t.is_some()))
        });
        assert!(result.success);
        assert_eq!(
            stages,
            [
                ("Writing thumbnail".to_owned(), false),
                ("Writing thumbnail".to_owned(), true),
                (CACHE_STAGE.to_owned(), false),
                (CACHE_STAGE.to_owned(), true),
            ]
        );
        assert_eq!(
            f.writer.writes.lock().unwrap()[0],
            (PathBuf::from("/fake/video.mp4"), Some(1234))
        );
        assert_eq!(
            f.invalidator.calls.lock().unwrap()[0],
            (PathBuf::from("/fake/video.mp4"), true)
        );
    }

    #[test]
    fn apply_failure_skips_invalidator() {
        let (s, f) = services();
        *f.writer.result.lock().unwrap() = Some(ApplyResult::failure(ApplyError::FfmpegError, "x"));
        assert!(!run_apply(&s, &job(None), &mut |_, _| {}).success);
        assert!(f.invalidator.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn destination_saves_copy_and_invalidates_the_copy() {
        let (s, f) = services();
        let result = run_apply(&s, &job(Some("/fake/video-thumbnail.mp4")), &mut |_, _| {});
        assert!(result.success);
        assert!(f.writer.writes.lock().unwrap().is_empty());
        assert_eq!(
            f.copier.calls.lock().unwrap()[0],
            (
                PathBuf::from("/fake/video-thumbnail.mp4"),
                false,
                Some(1234)
            )
        );
        assert_eq!(
            f.invalidator.calls.lock().unwrap()[0].0,
            PathBuf::from("/fake/video-thumbnail.mp4")
        );
    }

    #[test]
    fn destination_collision_skips_cache() {
        let (s, f) = services();
        *f.copier.result.lock().unwrap() = Some(ApplyResult::failure(
            ApplyError::DestinationExists,
            "exists",
        ));
        let result = run_apply(&s, &job(Some("/fake/c.mp4")), &mut |_, _| {});
        assert_eq!(result.error_code, Some(ApplyError::DestinationExists));
        assert!(f.invalidator.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn update_check_messages() {
        let ok = FakeUpdateChecker(Box::new(|v| {
            Ok(UpdateInfo {
                status: UpdateStatus::UpToDate,
                current_version: v.to_owned(),
                latest_version: Some(v.to_owned()),
                release_url: String::new(),
            })
        }));
        assert_eq!(run_update_check(&ok, "1.0").unwrap().current_version, "1.0");
        let user = FakeUpdateChecker(Box::new(|_| Err(Error::UpdateCheck("offline".into()))));
        assert_eq!(run_update_check(&user, "1.0").unwrap_err(), "offline");
        let other = FakeUpdateChecker(Box::new(|_| Err(Error::Other("bug".into()))));
        assert_eq!(
            run_update_check(&other, "1.0").unwrap_err(),
            UNEXPECTED_UPDATE_FAILURE
        );
    }

    #[test]
    fn guarded_turns_panics_into_errors() {
        assert_eq!(guarded(|| 5).unwrap(), 5);
        let err = guarded(|| -> u32 { panic!("not yet implemented: loader") }).unwrap_err();
        assert!(err.contains("not yet implemented: loader"));
        assert!(!in_guarded_section());
    }
}
