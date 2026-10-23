"""Qt worker runnables for off-thread video operations."""

from __future__ import annotations

import logging
import time
import traceback
from dataclasses import replace
from typing import TYPE_CHECKING

from PIL import Image
from PySide6.QtCore import QObject, QRunnable, Signal, Slot

from video_thumbnailer.core.save_copy import save_copy
from video_thumbnailer.exceptions import UnsupportedFormatError
from video_thumbnailer.models import TimelinePosition, VideoFile

if TYPE_CHECKING:
    from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor
    from video_thumbnailer.core.thumbnail_writer import FormatDispatchThumbnailWriter
    from video_thumbnailer.core.video_converter import FFmpegVideoConverter
    from video_thumbnailer.core.video_loader import PyAVVideoLoader
    from video_thumbnailer.platform import CacheInvalidator

__all__ = [
    "VideoLoadWorker", "VideoConvertWorker", "FrameExtractWorker", "ApplyWorker",
]

_log = logging.getLogger(__name__)


def _error_details(exc: Exception) -> str:
    _log.error("Video operation failed", exc_info=exc)
    return "".join(traceback.format_exception(exc)).rstrip()


# ---------------------------------------------------------------------------
# VideoLoadWorker
# ---------------------------------------------------------------------------

class _VideoLoadSignals(QObject):
    finished: Signal = Signal(object)  # VideoFile
    error: Signal = Signal(str)
    unsupported: Signal = Signal(str)


class VideoLoadWorker(QRunnable):
    """Load a video file in a worker thread.

    Signals:
        signals.finished(VideoFile): Emitted on success.
        signals.error(str): Emitted on failure with full diagnostic details.
    """

    def __init__(self, loader: PyAVVideoLoader, path: str) -> None:
        super().__init__()
        self._loader = loader
        self._path = path
        self.signals = _VideoLoadSignals()

    @Slot()
    def run(self) -> None:
        try:
            video = self._loader.load(self._path)
            self.signals.finished.emit(video)
        except UnsupportedFormatError:
            self.signals.unsupported.emit(self._path)
        except Exception as exc:  # noqa: BLE001
            self.signals.error.emit(_error_details(exc))


class _VideoConvertSignals(QObject):
    finished: Signal = Signal(str)
    error: Signal = Signal(str)
    progress: Signal = Signal(str)


class VideoConvertWorker(QRunnable):
    """Convert a video off the GUI thread and emit its output path or error."""

    def __init__(
        self,
        converter: FFmpegVideoConverter,
        source: str,
        destination: str,
    ) -> None:
        super().__init__()
        self._converter = converter
        self._source = source
        self._destination = destination
        self.signals = _VideoConvertSignals()

    @Slot()
    def run(self) -> None:
        try:
            path = self._converter.convert(
                self._source, self._destination, overwrite=True,
                on_progress=self.signals.progress.emit,
            )
            self.signals.finished.emit(path)
        except Exception as exc:  # noqa: BLE001
            self.signals.error.emit(_error_details(exc))


# ---------------------------------------------------------------------------
# FrameExtractWorker
# ---------------------------------------------------------------------------

class _FrameExtractSignals(QObject):
    finished: Signal = Signal(object)  # PIL.Image.Image
    error: Signal = Signal(str)


class FrameExtractWorker(QRunnable):
    """Extract a single frame from a video in a worker thread.

    Signals:
        signals.finished(PIL.Image): Emitted on success.
        signals.error(str): Emitted on failure with full diagnostic details.
    """

    def __init__(
        self,
        extractor: PyAVFrameExtractor,
        video: VideoFile,
        position: TimelinePosition,
    ) -> None:
        super().__init__()
        self._extractor = extractor
        self._video = video
        self._position = position
        self.signals = _FrameExtractSignals()

    @Slot()
    def run(self) -> None:
        try:
            image = self._extractor.extract(self._video, self._position)
            self.signals.finished.emit(image)
        except Exception as exc:  # noqa: BLE001
            self.signals.error.emit(_error_details(exc))


# ---------------------------------------------------------------------------
# ApplyWorker
# ---------------------------------------------------------------------------

class _ApplySignals(QObject):
    finished: Signal = Signal(object)  # ApplyResult
    error: Signal = Signal(str)
    progress: Signal = Signal(str, object)  # stage name, seconds or None


class ApplyWorker(QRunnable):
    """Embed a thumbnail and refresh the file-manager cache in a worker thread.

    Signals:
        signals.finished(ApplyResult): Emitted when the operation completes
            (success or failure — check ``result.success``).
        signals.error(str): Emitted only on unhandled exceptions.
    """

    def __init__(
        self,
        writer: FormatDispatchThumbnailWriter,
        invalidator: CacheInvalidator,
        video: VideoFile,
        thumbnail: Image.Image,
        position_ms: int | None = None,
        *,
        destination: str | None = None,
        overwrite: bool = False,
    ) -> None:
        super().__init__()
        self._writer = writer
        self._invalidator = invalidator
        self._video = video
        self._thumbnail = thumbnail
        self._position_ms = position_ms
        self._destination = destination
        self._overwrite = overwrite
        self.signals = _ApplySignals()

    @Slot()
    def run(self) -> None:
        try:
            if self._destination is None:
                result = self._writer.write(
                    self._video,
                    self._thumbnail,
                    position_ms=self._position_ms,
                    on_progress=self.signals.progress.emit,
                )
            else:
                result = save_copy(
                    self._writer, self._video, self._thumbnail, self._destination,
                    overwrite=self._overwrite,
                    position_ms=self._position_ms,
                    on_progress=self.signals.progress.emit,
                )
            if result.success:
                started = time.monotonic()
                self.signals.progress.emit("Refreshing thumbnail cache", None)
                self._invalidator.invalidate(
                    replace(
                        self._video,
                        path=result.output_path or self._video.path,
                        existing_thumbnail=self._thumbnail,
                    )
                )
                self.signals.progress.emit(
                    "Refreshing thumbnail cache", time.monotonic() - started
                )
            self.signals.finished.emit(result)
        except Exception as exc:  # noqa: BLE001
            self.signals.error.emit(_error_details(exc))
