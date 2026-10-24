//! Error types. Messages match the Python version's user-facing wording.

use std::path::PathBuf;

/// Errors raised by FrameCover core services.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "Unsupported video format: '{}'. Supported formats are MP4, MOV, MKV, AVI, WebM, and FLV.",
        .0.display()
    )]
    UnsupportedFormat(PathBuf),

    #[error(
        "No video stream found in '{}'. The file may be audio-only or may have a corrupt video track.",
        .0.display()
    )]
    NoVideoStream(PathBuf),

    #[error("Failed to extract frame at {offset_ms} ms from '{}': {cause}", path.display())]
    Extraction { path: PathBuf, offset_ms: u64, cause: String },

    #[error("{0}")]
    Conversion(String),

    /// The message is written for end users and explains what to do next.
    #[error("{0}")]
    UpdateCheck(String),

    /// An ffmpeg/ffprobe process exited unsuccessfully.
    #[error("ffmpeg failed (exit {code}). stderr: {stderr}")]
    Ffmpeg { code: i32, stderr: String },

    /// ffmpeg/ffprobe executable could not be located or started.
    #[error("FFmpeg is unavailable: {0}")]
    FfmpegMissing(String),

    #[error("{0}")]
    InvalidInput(String),

    #[error("Destination already exists: {}", .0.display())]
    DestinationExists(PathBuf),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
