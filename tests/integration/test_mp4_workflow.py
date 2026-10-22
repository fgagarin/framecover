"""Integration tests for end-to-end MP4/MOV workflow."""

from __future__ import annotations

import shutil
import time
from hashlib import sha256
from pathlib import Path

import av
import pytest

from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor
from video_thumbnailer.core.save_copy import save_copy
from video_thumbnailer.core.thumbnail_writer import FormatDispatchThumbnailWriter
from video_thumbnailer.core.video_loader import PyAVVideoLoader
from video_thumbnailer.models import TimelinePosition


@pytest.fixture()
def loader() -> PyAVVideoLoader:
    return PyAVVideoLoader()


@pytest.fixture()
def extractor() -> PyAVFrameExtractor:
    return PyAVFrameExtractor()


@pytest.fixture()
def writer() -> FormatDispatchThumbnailWriter:
    return FormatDispatchThumbnailWriter()


def _has_attached_pic_stream(video_path: Path) -> bool:
    container = av.open(str(video_path))
    try:
        for stream in container.streams:
            if stream.type == "video" and bool(stream.disposition & stream.disposition.attached_pic):
                return True
        return False
    finally:
        container.close()


def _generate_mov(tmp_path: Path) -> Path:
    """Generate a small MOV file for testing."""
    import subprocess

    import imageio_ffmpeg

    out = tmp_path / "sample.mov"
    subprocess.run(
        [
            imageio_ffmpeg.get_ffmpeg_exe(), "-y",
            "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=5",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=5",
            "-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p", "-ar", "44100",
            str(out),
        ],
        check=True, capture_output=True,
    )
    return out


class TestMp4Workflow:
    def test_copy_save_preserves_original_and_reuses_without_duration_growth(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
        tmp_path: Path,
    ) -> None:
        original_digest = sha256(sample_video.read_bytes()).hexdigest()
        video = loader.load(str(sample_video))
        destination = tmp_path / "thumbnail-copy.mp4"
        frame = extractor.extract(video, TimelinePosition(offset_ms=1000))

        first = save_copy(
            writer, video, frame, str(destination), position_ms=1000,
        )

        assert first.success, first.error_message
        assert first.output_path == str(destination)
        reopened = loader.load(str(destination))
        assert reopened.existing_thumbnail is not None
        assert reopened.thumbnail_position_ms == 1000 + first.duration_shift_ms
        assert _has_attached_pic_stream(destination)

        second_frame = extractor.extract(video, TimelinePosition(offset_ms=2000))
        second = save_copy(
            FormatDispatchThumbnailWriter(), loader.load(str(sample_video)),
            second_frame, str(destination), position_ms=2000,
        )

        assert second.success, second.error_message
        assert second.output_path == str(destination)
        repeated = loader.load(str(destination))
        assert repeated.existing_thumbnail is not None
        assert repeated.thumbnail_position_ms == 2000 + second.duration_shift_ms
        assert repeated.duration_ms == reopened.duration_ms
        assert sha256(sample_video.read_bytes()).hexdigest() == original_digest

    def test_mp4_full_pipeline_success(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
        tmp_path: Path,
    ) -> None:
        # Copy to tmp_path so modifications don't affect the fixture
        target = tmp_path / "test.mp4"
        shutil.copy2(str(sample_video), str(target))

        start = time.monotonic()
        vf = loader.load(str(target))
        frame = extractor.extract(vf, type("Pos", (), {"offset_ms": 2500})())
        result = writer.write(vf, frame)
        elapsed = (time.monotonic() - start) * 1000

        assert result.success is True
        assert elapsed < 10_000, f"Pipeline took {elapsed:.0f}ms (limit: 10000ms)"
        assert _has_attached_pic_stream(target), "MP4 has no attached_pic after write"

    def test_mov_full_pipeline_success(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        writer: FormatDispatchThumbnailWriter,
        tmp_path: Path,
    ) -> None:
        mov_path = _generate_mov(tmp_path)

        start = time.monotonic()
        vf = loader.load(str(mov_path))
        frame = extractor.extract(vf, type("Pos", (), {"offset_ms": 2500})())
        result = writer.write(vf, frame, position_ms=2500)
        elapsed = (time.monotonic() - start) * 1000

        assert result.success is True
        assert elapsed < 10_000, f"Pipeline took {elapsed:.0f}ms (limit: 10000ms)"
        assert _has_attached_pic_stream(mov_path), "MOV has no attached_pic after write"
        reopened = loader.load(str(mov_path))
        assert reopened.thumbnail_position_ms == 2500

    def test_pipeline_under_3_seconds(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
        tmp_path: Path,
    ) -> None:
        target = tmp_path / "test.mp4"
        shutil.copy2(str(sample_video), str(target))

        start = time.monotonic()
        vf = loader.load(str(target))
        frame = extractor.extract(vf, type("Pos", (), {"offset_ms": 0})())
        writer.write(vf, frame)
        elapsed = (time.monotonic() - start) * 1000

        assert elapsed < 3000, f"Full pipeline took {elapsed:.0f}ms (limit: 3000ms)"
