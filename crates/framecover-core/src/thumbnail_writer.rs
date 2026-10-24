//! Thumbnail embedding (port of `core/thumbnail_writer.py`).
//!
//! STUB — implemented by the write-side work package.

use image::RgbImage;

use crate::models::{ApplyResult, ProgressFn, VideoFile};

/// Embeds a thumbnail in place. Never fails: errors are in the [`ApplyResult`].
pub trait ThumbnailWriter: Send + Sync {
    fn write(
        &self,
        video: &VideoFile,
        thumbnail: &RgbImage,
        position_ms: Option<u64>,
        on_progress: Option<ProgressFn<'_>>,
    ) -> ApplyResult;

    /// Known reason saving this MP4/MOV will re-encode, or `None` for the fast path.
    fn reencode_reason(&self, video: &VideoFile) -> Option<String>;
}

/// Format-dispatching writer (MP4/MOV concat fast path or re-encode, MKV attach,
/// AVI remux, WebM/FLV no-op).
#[derive(Debug, Default, Clone)]
pub struct FormatDispatchThumbnailWriter;

impl ThumbnailWriter for FormatDispatchThumbnailWriter {
    fn write(
        &self,
        _video: &VideoFile,
        _thumbnail: &RgbImage,
        _position_ms: Option<u64>,
        _on_progress: Option<ProgressFn<'_>>,
    ) -> ApplyResult {
        todo!("thumbnail writer")
    }

    fn reencode_reason(&self, _video: &VideoFile) -> Option<String> {
        todo!("thumbnail writer")
    }
}
