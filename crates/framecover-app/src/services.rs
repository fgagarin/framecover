//! Service seams between the GUI and `framecover-core`.
//!
//! The controller and workers only see the traits in this module (plus the
//! core's own `VideoLoader`, `FrameExtractor`, `FrameSource` and
//! `ThumbnailWriter`, which already exist as stable contracts). Concrete core implementations are
//! connected in exactly one place, [`Services::production`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use framecover_core::converter::FfmpegVideoConverter;
use framecover_core::frame_stream::{FfmpegFrameStream, FrameSource};
use framecover_core::frames::{FfmpegFrameExtractor, FrameExtractor};
use framecover_core::loader::{FfprobeVideoLoader, VideoLoader};
#[cfg(not(target_os = "android"))]
use framecover_core::platform;
use framecover_core::thumbnail_writer::{FormatDispatchThumbnailWriter, ThumbnailWriter};
#[cfg(not(target_os = "android"))]
use framecover_core::update_checker::{GitHubUpdateChecker, UpdateChecker as CoreChecker};
use framecover_core::{ApplyResult, Error, ProgressFn, UpdateInfo, VideoFile};
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

/// Creates the scrubbing [`FrameSource`] for a newly loaded video.
pub type FrameSourceFactory = Arc<dyn Fn(&VideoFile) -> Box<dyn FrameSource> + Send + Sync>;

/// Every service the GUI uses, as shareable trait objects.
#[derive(Clone)]
pub struct Services {
    pub loader: Arc<dyn VideoLoader>,
    /// One-off extractions (filmstrip tiles).
    pub extractor: Arc<dyn FrameExtractor>,
    /// The preview's frames while scrubbing (one source per loaded video).
    pub frame_sources: FrameSourceFactory,
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
            frame_sources: Arc::new(|video| Box::new(FfmpegFrameStream::new(video.clone()))),
            writer: Arc::new(FormatDispatchThumbnailWriter),
            copier: Arc::new(CoreCopySaver),
            converter: Arc::new(CoreConverter),
            // Releases are desktop downloads; there is nothing to update to
            // on Android yet.
            #[cfg(not(target_os = "android"))]
            update_checker: Some(Arc::new(CoreUpdateChecker(GitHubUpdateChecker::new()))),
            #[cfg(target_os = "android")]
            update_checker: None,
            #[cfg(not(target_os = "android"))]
            invalidator: Arc::new(CoreCacheInvalidator(platform::get_cache_invalidator())),
            #[cfg(target_os = "android")]
            invalidator: Arc::new(GalleryPublisher),
            #[cfg(not(target_os = "android"))]
            desktop: Arc::new(CoreDesktop),
            #[cfg(target_os = "android")]
            desktop: Arc::new(AndroidDesktop),
        }
    }
}

/// Windows taskbar grouping / icon identity (`FrameCover.Application`).
#[cfg(not(target_os = "android"))]
pub fn set_app_user_model_id() {
    framecover_core::platform::set_app_user_model_id();
}

// ---------------------------------------------------------------------------
// Adapters over core functions.
// ---------------------------------------------------------------------------

struct CoreCopySaver;

impl CopySaver for CoreCopySaver {
    fn save_copy(
        &self,
        writer: &dyn ThumbnailWriter,
        video: &VideoFile,
        thumbnail: &RgbImage,
        destination: &Path,
        overwrite: bool,
        position_ms: Option<u64>,
        on_progress: Option<ProgressFn<'_>>,
    ) -> ApplyResult {
        framecover_core::save_copy::save_copy(
            writer,
            video,
            thumbnail,
            destination,
            overwrite,
            position_ms,
            on_progress,
        )
    }
}

struct CoreConverter;

impl VideoConverter for CoreConverter {
    fn convert(
        &self,
        source: &Path,
        destination: &Path,
        overwrite: bool,
        on_progress: &mut dyn FnMut(&str),
    ) -> Result<PathBuf, Error> {
        FfmpegVideoConverter.convert(source, destination, overwrite, Some(on_progress))
    }
}

#[cfg(not(target_os = "android"))]
struct CoreUpdateChecker(GitHubUpdateChecker);

#[cfg(not(target_os = "android"))]
impl UpdateChecker for CoreUpdateChecker {
    fn check(&self, current_version: &str) -> Result<UpdateInfo, Error> {
        CoreChecker::check(&self.0, current_version)
    }
}

#[cfg(not(target_os = "android"))]
struct CoreCacheInvalidator(Box<dyn platform::CacheInvalidator>);

#[cfg(not(target_os = "android"))]
impl CacheInvalidator for CoreCacheInvalidator {
    fn invalidate(&self, path: &Path, thumbnail: Option<&RgbImage>) {
        self.0.invalidate(path, thumbnail);
    }
}

#[cfg(not(target_os = "android"))]
struct CoreDesktop;

#[cfg(not(target_os = "android"))]
impl Desktop for CoreDesktop {
    fn reveal_in_file_manager(&self, path: &Path) {
        if let Err(err) = platform::reveal_in_file_manager(path) {
            log::warn!("could not reveal {}: {err}", path.display());
        }
    }

    fn open_url(&self, url: &str) -> bool {
        platform::open_url(url)
            .map_err(|err| log::warn!("could not open {url}: {err}"))
            .is_ok()
    }
}

/// Android: saved copies live in the app's cache; publishing them to the
/// shared `Movies/FrameCover` collection is what makes them appear in the
/// gallery (the counterpart of refreshing a desktop thumbnail cache).
#[cfg(target_os = "android")]
struct GalleryPublisher;

#[cfg(target_os = "android")]
impl CacheInvalidator for GalleryPublisher {
    fn invalidate(&self, path: &Path, _thumbnail: Option<&RgbImage>) {
        if let Err(err) = crate::android::publish_video(path) {
            log::error!("could not publish {} to the gallery: {err}", path.display());
        }
    }
}

/// Android: "reveal" opens the published copy; links open in the browser.
#[cfg(target_os = "android")]
struct AndroidDesktop;

#[cfg(target_os = "android")]
impl Desktop for AndroidDesktop {
    fn reveal_in_file_manager(&self, path: &Path) {
        crate::android::view_published(path);
    }

    fn open_url(&self, url: &str) -> bool {
        crate::android::open_url(url)
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

    /// Frame sources that run `extractor` for every request.
    pub fn extractor_sources(extractor: Arc<dyn FrameExtractor>) -> FrameSourceFactory {
        Arc::new(move |video| {
            Box::new(framecover_core::frame_stream::ExtractorFrameSource::new(
                Arc::clone(&extractor),
                video.clone(),
            ))
        })
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
