//! Frame extraction (port of `core/frame_extractor.py`).
//!
//! STUB — implemented by the read-side work package.

use image::RgbImage;

use crate::error::Result;
use crate::models::VideoFile;

/// Extracts a single RGB frame nearest to a timeline position.
pub trait FrameExtractor: Send + Sync {
    /// Errors: `InvalidInput` if `offset_ms > video.duration_ms`, `Extraction` otherwise.
    fn extract(&self, video: &VideoFile, offset_ms: u64) -> Result<RgbImage>;
}

/// ffmpeg-process-backed extractor.
#[derive(Debug, Default, Clone)]
pub struct FfmpegFrameExtractor;

impl FrameExtractor for FfmpegFrameExtractor {
    fn extract(&self, _video: &VideoFile, _offset_ms: u64) -> Result<RgbImage> {
        todo!("frames")
    }
}
