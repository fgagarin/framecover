"""Unit tests for Qt worker runnables (T039 coverage boost)."""

from __future__ import annotations

from unittest.mock import MagicMock

from PIL import Image

from framecover.core.frame_extractor import PyAVFrameExtractor
from framecover.core.thumbnail_writer import FormatDispatchThumbnailWriter
from framecover.core.video_loader import PyAVVideoLoader
from framecover.models import (
    ApplyResult,
    TimelinePosition,
    VideoFile,
    VideoFormat,
)
from framecover.ui.worker import (
    ApplyWorker,
    FrameExtractWorker,
    VideoConvertWorker,
    VideoLoadWorker,
)


def _make_vf() -> VideoFile:
    return VideoFile(
        path="/fake/video.mp4",
        format=VideoFormat.MP4,
        duration_ms=5000,
        width=320,
        height=240,
        existing_thumbnail=None,
        is_writable=True,
    )


# ---------------------------------------------------------------------------
# Helpers: run a QRunnable synchronously and capture signals
# ---------------------------------------------------------------------------

def _run_worker(worker):
    """Execute worker.run() synchronously and return captured signal values."""
    finished_values = []
    error_values = []
    worker.signals.finished.connect(lambda v: finished_values.append(v))
    worker.signals.error.connect(lambda v: error_values.append(v))
    worker.run()
    return finished_values, error_values


# ---------------------------------------------------------------------------
# VideoLoadWorker
# ---------------------------------------------------------------------------

class TestVideoLoadWorker:
    def test_unsupported_format_emits_conversion_path(self):
        from framecover.exceptions import UnsupportedFormatError

        loader = MagicMock(spec=PyAVVideoLoader)
        loader.load.side_effect = UnsupportedFormatError("clip.m2ts")
        worker = VideoLoadWorker(loader, "clip.m2ts")
        paths = []
        worker.signals.unsupported.connect(paths.append)
        finished, errors = _run_worker(worker)
        assert paths == ["clip.m2ts"]
        assert finished == errors == []

    def test_run_success_emits_finished(self) -> None:
        loader = MagicMock(spec=PyAVVideoLoader)
        vf = _make_vf()
        loader.load.return_value = vf

        worker = VideoLoadWorker(loader, "/fake/video.mp4")
        finished, errors = _run_worker(worker)

        assert len(finished) == 1
        assert finished[0] is vf
        assert errors == []

    def test_run_error_emits_error_string(self) -> None:
        loader = MagicMock(spec=PyAVVideoLoader)
        loader.load.side_effect = FileNotFoundError("file not found")

        worker = VideoLoadWorker(loader, "/no/such/file.mp4")
        finished, errors = _run_worker(worker)

        assert finished == []
        assert len(errors) == 1
        assert "file not found" in errors[0]


# ---------------------------------------------------------------------------
# FrameExtractWorker
# ---------------------------------------------------------------------------

class TestVideoConvertWorker:
    def test_conversion_emits_output_and_progress(self):
        converter = MagicMock()
        converter.convert.return_value = "out.mp4"
        worker = VideoConvertWorker(converter, "in.m2ts", "out.mp4")
        finished, errors = _run_worker(worker)
        assert finished == ["out.mp4"]
        assert errors == []
        converter.convert.assert_called_once_with(
            "in.m2ts", "out.mp4", overwrite=True,
            on_progress=worker.signals.progress.emit,
        )

    def test_conversion_error_is_reported(self):
        converter = MagicMock()
        converter.convert.side_effect = RuntimeError("encoder failed")
        finished, errors = _run_worker(VideoConvertWorker(converter, "in.m2ts", "out.mp4"))
        assert finished == []
        assert len(errors) == 1
        assert "RuntimeError: encoder failed" in errors[0]
        assert "Traceback (most recent call last):" in errors[0]


class TestFrameExtractWorker:
    def test_run_success_emits_finished(self) -> None:
        extractor = MagicMock(spec=PyAVFrameExtractor)
        img = Image.new("RGB", (10, 10))
        extractor.extract.return_value = img
        vf = _make_vf()
        pos = TimelinePosition(offset_ms=2000)

        worker = FrameExtractWorker(extractor, vf, pos)
        finished, errors = _run_worker(worker)

        assert len(finished) == 1
        assert finished[0] is img
        assert errors == []

    def test_run_error_emits_error_string(self) -> None:
        from framecover.exceptions import ExtractionError

        extractor = MagicMock(spec=PyAVFrameExtractor)
        extractor.extract.side_effect = ExtractionError("/fake.mp4", 2000, "codec fail")
        vf = _make_vf()
        pos = TimelinePosition(offset_ms=2000)

        worker = FrameExtractWorker(extractor, vf, pos)
        finished, errors = _run_worker(worker)

        assert finished == []
        assert len(errors) == 1
        assert "codec fail" in errors[0]
        assert "Traceback (most recent call last):" in errors[0]

    def test_chained_decode_error_preserves_cause(self) -> None:
        from framecover.exceptions import ExtractionError

        def fail(video, position):
            try:
                raise ValueError("Invalid data found when processing input")
            except ValueError as exc:
                raise ExtractionError(
                    video.path, position.offset_ms, "Cannot decode frame"
                ) from exc

        extractor = MagicMock(spec=PyAVFrameExtractor)
        extractor.extract.side_effect = fail
        worker = FrameExtractWorker(
            extractor, _make_vf(), TimelinePosition(offset_ms=2000)
        )
        finished, errors = _run_worker(worker)
        assert finished == []
        assert len(errors) == 1
        assert "ValueError: Invalid data" in errors[0]
        assert "direct cause" in errors[0]
        assert "ExtractionError" in errors[0]


# ---------------------------------------------------------------------------
# ApplyWorker
# ---------------------------------------------------------------------------

class TestApplyWorker:
    def test_run_emits_save_and_cache_progress(self) -> None:
        writer = MagicMock(spec=FormatDispatchThumbnailWriter)
        writer.write.return_value = ApplyResult(success=True)
        invalidator = MagicMock()
        worker = ApplyWorker(writer, invalidator, _make_vf(), Image.new("RGB", (10, 10)))
        events = []
        worker.signals.progress.connect(lambda name, elapsed: events.append((name, elapsed)))

        _run_worker(worker)

        assert events[0] == ("Refreshing thumbnail cache", None)
        assert events[1][0] == "Refreshing thumbnail cache"
        assert events[1][1] >= 0

    def test_run_success_calls_invalidator(self) -> None:
        writer = MagicMock(spec=FormatDispatchThumbnailWriter)
        writer.write.return_value = ApplyResult(success=True)
        invalidator = MagicMock()
        vf = _make_vf()
        thumbnail = Image.new("RGB", (10, 10))

        worker = ApplyWorker(writer, invalidator, vf, thumbnail)
        finished, errors = _run_worker(worker)

        invalidator.invalidate.assert_called_once()
        cached_video = invalidator.invalidate.call_args.args[0]
        assert cached_video.path == vf.path
        assert cached_video.existing_thumbnail is thumbnail
        assert vf.existing_thumbnail is None
        assert len(finished) == 1
        assert finished[0].success is True
        assert errors == []

    def test_run_passes_selected_position_to_writer(self) -> None:
        writer = MagicMock(spec=FormatDispatchThumbnailWriter)
        writer.write.return_value = ApplyResult(success=True)
        invalidator = MagicMock()
        vf = _make_vf()
        thumbnail = Image.new("RGB", (10, 10))

        worker = ApplyWorker(writer, invalidator, vf, thumbnail, position_ms=1680)
        _run_worker(worker)

        writer.write.assert_called_once_with(
            vf, thumbnail, position_ms=1680,
            on_progress=worker.signals.progress.emit,
        )

    def test_run_failure_result_skips_invalidator(self) -> None:
        writer = MagicMock(spec=FormatDispatchThumbnailWriter)
        from framecover.models import ApplyError
        writer.write.return_value = ApplyResult(
            success=False,
            error_code=ApplyError.FFMPEG_ERROR,
            error_message="ffmpeg failed",
        )
        invalidator = MagicMock()
        vf = _make_vf()
        thumbnail = Image.new("RGB", (10, 10))

        worker = ApplyWorker(writer, invalidator, vf, thumbnail)
        finished, errors = _run_worker(worker)

        invalidator.invalidate.assert_not_called()
        assert len(finished) == 1
        assert finished[0].success is False

    def test_run_exception_emits_error_string(self) -> None:
        writer = MagicMock(spec=FormatDispatchThumbnailWriter)
        writer.write.side_effect = RuntimeError("unexpected error")
        invalidator = MagicMock()
        vf = _make_vf()
        thumbnail = Image.new("RGB", (10, 10))

        worker = ApplyWorker(writer, invalidator, vf, thumbnail)
        finished, errors = _run_worker(worker)

        assert finished == []
        assert len(errors) == 1
        assert "unexpected error" in errors[0]

    def test_destination_saves_copy_and_invalidates_final_thumbnail(self, tmp_path):
        source = tmp_path / "source.mp4"
        source.write_bytes(b"original")
        destination = tmp_path / "output.mp4"
        vf = _make_vf()
        vf.path = str(source)
        thumbnail = Image.new("RGB", (10, 10))
        writer = MagicMock(spec=FormatDispatchThumbnailWriter)
        writer.write.return_value = ApplyResult(success=True)
        invalidator = MagicMock()
        worker = ApplyWorker(
            writer, invalidator, vf, thumbnail, position_ms=120,
            destination=str(destination),
        )

        finished, errors = _run_worker(worker)

        assert errors == []
        assert finished[0].success
        assert finished[0].output_path == str(destination)
        assert destination.read_bytes() == source.read_bytes() == b"original"
        staged = writer.write.call_args.args[0]
        assert staged.path != vf.path
        assert writer.write.call_args.kwargs["position_ms"] == 120
        assert writer.write.call_args.kwargs["on_progress"] == worker.signals.progress.emit
        cached = invalidator.invalidate.call_args.args[0]
        assert cached.path == str(destination)
        assert cached.existing_thumbnail is thumbnail
        assert vf.path == str(source)
        assert vf.existing_thumbnail is None

    def test_destination_collision_skips_cache_unless_overwrite(self, tmp_path):
        source = tmp_path / "source.mp4"
        source.write_bytes(b"original")
        destination = tmp_path / "output.mp4"
        destination.write_bytes(b"other")
        vf = _make_vf()
        vf.path = str(source)
        writer = MagicMock(spec=FormatDispatchThumbnailWriter)
        writer.write.return_value = ApplyResult(success=True)
        invalidator = MagicMock()
        thumbnail = Image.new("RGB", (10, 10))
        finished, errors = _run_worker(
            ApplyWorker(writer, invalidator, vf, thumbnail, destination=str(destination))
        )
        from framecover.models import ApplyError

        assert errors == []
        assert finished[0].error_code is ApplyError.DESTINATION_EXISTS
        writer.write.assert_not_called()
        invalidator.invalidate.assert_not_called()
        finished, errors = _run_worker(
            ApplyWorker(
                writer, invalidator, vf, thumbnail,
                destination=str(destination), overwrite=True,
            )
        )
        assert errors == []
        assert finished[0].success
        assert destination.read_bytes() == b"original"
