"""Custom exceptions for FrameCover."""

from __future__ import annotations

__all__ = [
    "FrameCoverError",
    "UnsupportedFormatError",
    "NoVideoStreamError",
    "ExtractionError",
    "ConversionError",
    "UpdateCheckError",
]


class FrameCoverError(RuntimeError):
    """Base exception for all FrameCover errors."""


class ConversionError(FrameCoverError):
    """Raised when FFmpeg cannot convert a video to MP4."""


class UpdateCheckError(FrameCoverError):
    """Raised when the latest release information cannot be retrieved.

    The message is written for end users and explains what they can do next.
    """


class UnsupportedFormatError(FrameCoverError):
    """Raised when a file's container format is not recognised or supported.

    Attributes:
        path: Path to the offending file.
    """

    def __init__(self, path: str) -> None:
        self.path = path
        super().__init__(
            f"Unsupported video format: '{path}'. "
            "Supported formats are MP4, MOV, MKV, AVI, WebM, and FLV."
        )


class NoVideoStreamError(FrameCoverError):
    """Raised when a file contains no video stream (e.g. audio-only file).

    Attributes:
        path: Path to the offending file.
    """

    def __init__(self, path: str) -> None:
        self.path = path
        super().__init__(
            f"No video stream found in '{path}'. "
            "The file may be audio-only or may have a corrupt video track."
        )


class ExtractionError(FrameCoverError):
    """Raised when a frame cannot be decoded at the requested position.

    Attributes:
        path: Path to the video file.
        offset_ms: Requested timestamp in milliseconds.
        cause: Human-readable description of the underlying error.
    """

    def __init__(self, path: str, offset_ms: int, cause: str) -> None:
        self.path = path
        self.offset_ms = offset_ms
        self.cause = cause
        super().__init__(
            f"Failed to extract frame at {offset_ms} ms from '{path}': {cause}"
        )
