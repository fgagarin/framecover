//! Video file loading (port of `core/video_loader.py`).
//!
//! STUB — implemented by the read-side work package.

use std::path::Path;

use crate::error::Result;
use crate::models::VideoFile;

/// Loads a video file and returns a populated [`VideoFile`].
pub trait VideoLoader: Send + Sync {
    /// Errors: `Io(NotFound)`, `UnsupportedFormat`, `NoVideoStream`.
    fn load(&self, path: &Path) -> Result<VideoFile>;
}

/// ffprobe-backed loader.
#[derive(Debug, Default, Clone)]
pub struct FfprobeVideoLoader;

impl VideoLoader for FfprobeVideoLoader {
    fn load(&self, _path: &Path) -> Result<VideoFile> {
        todo!("loader")
    }
}
