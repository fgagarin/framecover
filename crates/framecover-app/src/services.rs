//! Service seams between the GUI and `framecover-core`.
//!
//! The controller and workers only see the traits in this module (plus the
//! core's own `VideoLoader`, `FrameExtractor` and `ThumbnailWriter`, which
//! already exist as stable contracts). Concrete core implementations are
//! connected in exactly one place, [`Services::production`]. Anything the
//! core does not provide yet is bridged by a small shim marked `// WIRE:`;
//! replace each shim with the real core call once it lands.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use framecover_core::frames::{FfmpegFrameExtractor, FrameExtractor};
use framecover_core::loader::{FfprobeVideoLoader, VideoLoader};
use framecover_core::thumbnail_writer::{FormatDispatchThumbnailWriter, ThumbnailWriter};
use framecover_core::{ApplyError, ApplyResult, Error, ProgressFn, UpdateInfo, VideoFile};
use image::RgbImage;

/// Copy-first saving: write a copy of `video` with `thumbnail` embedded to
/// `destination` (core `save_copy::save_copy`).
pub trait CopySaver: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    fn save_copy(
        &self,
        writer: &dyn ThumbnailWriter,
        video: &VideoFile,
        thumbnail: &RgbImage,
        destination: &Path,
        overwrite: bool,
        position_ms: Option<u64>,
        on_progress: Option<ProgressFn<'_>>,
    ) -> ApplyResult;
}

/// Converts an unsupported container into a separate MP4 (core `converter`).
pub trait VideoConverter: Send + Sync {
    /// `on_progress` receives human-readable status lines for the progress dialog.
    fn convert(
        &self,
        source: &Path,
        destination: &Path,
        overwrite: bool,
        on_progress: &mut dyn FnMut(&str),
    ) -> Result<PathBuf, Error>;
}

/// Looks up the latest published release (core `update_checker`).
pub trait UpdateChecker: Send + Sync {
    /// `Err(Error::UpdateCheck(msg))` carries a user-facing explanation.
    fn check(&self, current_version: &str) -> Result<UpdateInfo, Error>;
}

/// Refreshes the OS file-manager thumbnail cache after a save (core `platform`).
pub trait CacheInvalidator: Send + Sync {
    fn invalidate(&self, path: &Path, thumbnail: Option<&RgbImage>);
}

/// Desktop integration (core `platform`).
pub trait Desktop: Send + Sync {
    /// Show `path` selected in Explorer / Finder, or open its folder.
    fn reveal_in_file_manager(&self, path: &Path);
    /// Open `url` in the default browser; `false` if that failed.
    fn open_url(&self, url: &str) -> bool;
}

/// Every service the GUI uses, as shareable trait objects.
#[derive(Clone)]
pub struct Services {
    pub loader: Arc<dyn VideoLoader>,
    pub extractor: Arc<dyn FrameExtractor>,
    pub writer: Arc<dyn ThumbnailWriter>,
    pub copier: Arc<dyn CopySaver>,
    pub converter: Arc<dyn VideoConverter>,
    /// `None` hides the update indicator (as in the Python app).
    pub update_checker: Option<Arc<dyn UpdateChecker>>,
    pub invalidator: Arc<dyn CacheInvalidator>,
    pub desktop: Arc<dyn Desktop>,
}

impl Services {
    /// The real implementations. This is the single wiring point.
    pub fn production() -> Self {
        Self {
            loader: Arc::new(FfprobeVideoLoader),
            extractor: Arc::new(FfmpegFrameExtractor),
            writer: Arc::new(FormatDispatchThumbnailWriter),
            copier: Arc::new(CoreCopySaver),
            converter: Arc::new(CoreConverter),
            update_checker: Some(Arc::new(CoreUpdateChecker)),
            invalidator: Arc::new(CoreCacheInvalidator),
            desktop: Arc::new(CoreDesktop),
        }
    }
}

/// Windows taskbar grouping / icon identity (`FrameCover.Application`).
pub fn set_app_user_model_id() {
    // WIRE: replace with `framecover_core::platform::set_app_user_model_id()`.
    // Until then the taskbar groups the app under the executable path, which
    // is harmless.
}

// ---------------------------------------------------------------------------
// Adapters over core functions. Bodies marked WIRE are temporary shims.
// ---------------------------------------------------------------------------

struct CoreCopySaver;

impl CopySaver for CoreCopySaver {
    fn save_copy(
        &self,
        _writer: &dyn ThumbnailWriter,
        _video: &VideoFile,
        _thumbnail: &RgbImage,
        _destination: &Path,
        _overwrite: bool,
        _position_ms: Option<u64>,
        _on_progress: Option<ProgressFn<'_>>,
    ) -> ApplyResult {
        // WIRE: framecover_core::save_copy::save_copy(
        //     writer, video, thumbnail, destination, overwrite, position_ms, on_progress)
        ApplyResult::failure(
            ApplyError::Unexpected,
            "Saving a copy is not available in this build yet (core save_copy is not wired).",
        )
    }
}

struct CoreConverter;

impl VideoConverter for CoreConverter {
    fn convert(
        &self,
        _source: &Path,
        _destination: &Path,
        _overwrite: bool,
        _on_progress: &mut dyn FnMut(&str),
    ) -> Result<PathBuf, Error> {
        // WIRE: framecover_core::converter::FfmpegVideoConverter::default()
        //     .convert(source, destination, overwrite, on_progress)
        // (adapt the progress callback type if the core one differs).
        Err(Error::Conversion(
            "MP4 conversion is not available in this build yet (core converter is not wired)."
                .to_owned(),
        ))
    }
}

struct CoreUpdateChecker;

impl UpdateChecker for CoreUpdateChecker {
    fn check(&self, _current_version: &str) -> Result<UpdateInfo, Error> {
        // WIRE: framecover_core::update_checker::GitHubUpdateChecker::default()
        //     .check(current_version)
        Err(Error::UpdateCheck(
            "Update checking is not available in this build yet. FrameCover keeps \
working normally."
                .to_owned(),
        ))
    }
}

struct CoreCacheInvalidator;

impl CacheInvalidator for CoreCacheInvalidator {
    fn invalidate(&self, path: &Path, _thumbnail: Option<&RgbImage>) {
        // WIRE: framecover_core::platform::get_cache_invalidator().invalidate(path, thumbnail)
        // (create the invalidator once, e.g. store it in this struct).
        log::debug!(
            "thumbnail cache invalidation not wired yet for {}",
            path.display()
        );
    }
}

struct CoreDesktop;

impl Desktop for CoreDesktop {
    fn reveal_in_file_manager(&self, path: &Path) {
        // WIRE: framecover_core::platform::reveal_in_file_manager(path)
        shim::reveal_in_file_manager(path);
    }

    fn open_url(&self, url: &str) -> bool {
        // WIRE: framecover_core::platform::open_url(url)
        shim::open_url(url)
    }
}

/// Temporary process-spawning fallbacks used until the core platform module
/// lands. Delete together with the WIRE shims above.
mod shim {
    use std::path::Path;
    use std::process::Command;

    pub fn reveal_in_file_manager(path: &Path) {
        let result = if cfg!(windows) {
            let mut arg = std::ffi::OsString::from("/select,");
            arg.push(path.as_os_str());
            Command::new("explorer.exe").arg(arg).spawn().map(drop)
        } else if cfg!(target_os = "macos") {
            Command::new("open").arg("-R").arg(path).spawn().map(drop)
        } else {
            let folder = path.parent().unwrap_or(path);
            Command::new("xdg-open").arg(folder).spawn().map(drop)
        };
        if let Err(err) = result {
            log::warn!("could not reveal {}: {err}", path.display());
        }
    }

    pub fn open_url(url: &str) -> bool {
        let result = if cfg!(windows) {
            Command::new("rundll32.exe")
                .arg("url.dll,FileProtocolHandler")
                .arg(url)
                .spawn()
        } else if cfg!(target_os = "macos") {
            Command::new("open").arg(url).spawn()
        } else {
            Command::new("xdg-open").arg(url).spawn()
        };
        result
            .map_err(|err| log::warn!("could not open {url}: {err}"))
            .is_ok()
    }
}

#[cfg(test)]
pub(crate) mod fakes {
    //! Recording fakes of every service, for controller and worker tests.

    use super::*;
    use std::sync::Mutex;

    type LoadFn = dyn Fn(&Path) -> framecover_core::Result<VideoFile> + Send + Sync;
    type ExtractFn = dyn Fn(&VideoFile, u64) -> framecover_core::Result<RgbImage> + Send + Sync;
    type ConvertFn = dyn Fn(&Path, &Path) -> Result<PathBuf, Error> + Send + Sync;
    type CheckFn = dyn Fn(&str) -> Result<UpdateInfo, Error> + Send + Sync;

    #[derive(Default)]
    pub struct FakeWriter {
        pub reencode_reason: Mutex<Option<String>>,
        pub result: Mutex<Option<ApplyResult>>,
        pub writes: Mutex<Vec<(PathBuf, Option<u64>)>>,
    }

    impl ThumbnailWriter for FakeWriter {
        fn write(
            &self,
            video: &VideoFile,
            _thumbnail: &RgbImage,
            position_ms: Option<u64>,
            on_progress: Option<ProgressFn<'_>>,
        ) -> ApplyResult {
            if let Some(cb) = on_progress {
                cb("Writing thumbnail", None);
                cb("Writing thumbnail", Some(0.5));
            }
            self.writes
                .lock()
                .unwrap()
                .push((video.path.clone(), position_ms));
            self.result
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(ApplyResult::ok)
        }

        fn reencode_reason(&self, _video: &VideoFile) -> Option<String> {
            self.reencode_reason.lock().unwrap().clone()
        }
    }

    #[derive(Default)]
    pub struct FakeCopier {
        pub result: Mutex<Option<ApplyResult>>,
        pub calls: Mutex<Vec<(PathBuf, bool, Option<u64>)>>,
    }

    impl CopySaver for FakeCopier {
        fn save_copy(
            &self,
            _writer: &dyn ThumbnailWriter,
            _video: &VideoFile,
            _thumbnail: &RgbImage,
            destination: &Path,
            overwrite: bool,
            position_ms: Option<u64>,
            on_progress: Option<ProgressFn<'_>>,
        ) -> ApplyResult {
            if let Some(cb) = on_progress {
                cb("Copying video", None);
                cb("Copying video", Some(0.1));
            }
            self.calls
                .lock()
                .unwrap()
                .push((destination.to_owned(), overwrite, position_ms));
            self.result
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| ApplyResult {
                    output_path: Some(destination.to_owned()),
                    ..ApplyResult::ok()
                })
        }
    }

    #[derive(Default)]
    pub struct FakeInvalidator {
        pub calls: Mutex<Vec<(PathBuf, bool)>>,
    }

    impl CacheInvalidator for FakeInvalidator {
        fn invalidate(&self, path: &Path, thumbnail: Option<&RgbImage>) {
            self.calls
                .lock()
                .unwrap()
                .push((path.to_owned(), thumbnail.is_some()));
        }
    }

    pub struct FakeLoader(pub Box<LoadFn>);

    impl VideoLoader for FakeLoader {
        fn load(&self, path: &Path) -> framecover_core::Result<VideoFile> {
            (self.0)(path)
        }
    }

    pub struct FakeExtractor(pub Box<ExtractFn>);

    impl FrameExtractor for FakeExtractor {
        fn extract(&self, video: &VideoFile, offset_ms: u64) -> framecover_core::Result<RgbImage> {
            (self.0)(video, offset_ms)
        }
    }

    pub struct FakeConverter(pub Box<ConvertFn>);

    impl VideoConverter for FakeConverter {
        fn convert(
            &self,
            source: &Path,
            destination: &Path,
            _overwrite: bool,
            on_progress: &mut dyn FnMut(&str),
        ) -> Result<PathBuf, Error> {
            on_progress("Remuxing to MP4");
            (self.0)(source, destination)
        }
    }

    pub struct FakeUpdateChecker(pub Box<CheckFn>);

    impl UpdateChecker for FakeUpdateChecker {
        fn check(&self, current_version: &str) -> Result<UpdateInfo, Error> {
            (self.0)(current_version)
        }
    }
}
