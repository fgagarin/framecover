//! MP4 metadata: FrameCover's frame-position freeform tags and `covr`
//! (port of `core/thumbnail_metadata.py` + covr writing).
//!
//! STUB — implemented by the write-side work package.

/// Stable on-disk identifiers from before the FrameCover rename.
pub const TAG_MEAN: &str = "com.apple.iTunes";
pub const FRAME_NUMBER_NAME: &str = "video-thumbnailer-frame-number";
pub const POSITION_MS_NAME: &str = "video-thumbnailer-position-ms";
