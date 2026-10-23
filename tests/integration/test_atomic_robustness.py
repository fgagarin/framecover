"""Integration tests for atomic write robustness in the write pipeline."""

from __future__ import annotations

import glob
import shutil
from pathlib import Path
from unittest.mock import patch

import pytest

from framecover.core.frame_extractor import PyAVFrameExtractor
from framecover.core.thumbnail_writer import FormatDispatchThumbnailWriter
from framecover.core.video_loader import PyAVVideoLoader


@pytest.fixture()
def loader() -> PyAVVideoLoader:
    return PyAVVideoLoader()


@pytest.fixture()
def extractor() -> PyAVFrameExtractor:
    return PyAVFrameExtractor()


@pytest.fixture()
def writer() -> FormatDispatchThumbnailWriter:
    return FormatDispatchThumbnailWriter()


class TestAtomicRobustness:
    def test_failed_os_replace_returns_failure_result(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
        tmp_path: Path,
    ) -> None:
        target = tmp_path / "test.mp4"
        shutil.copy2(str(sample_video), str(target))

        vf = loader.load(str(target))
        frame = extractor.extract(vf, type("Pos", (), {"offset_ms": 0})())

        with patch(
            "framecover.core.atomic_write.os.replace",
            side_effect=OSError("simulated replace failure"),
        ):
            result = writer.write(vf, frame)

        assert result.success is False

    def test_failed_write_leaves_original_unchanged(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
        tmp_path: Path,
    ) -> None:
        target = tmp_path / "test.mp4"
        shutil.copy2(str(sample_video), str(target))
        original_bytes = target.read_bytes()

        vf = loader.load(str(target))
        frame = extractor.extract(vf, type("Pos", (), {"offset_ms": 0})())

        with patch(
            "framecover.core.atomic_write.os.replace",
            side_effect=OSError("simulated replace failure"),
        ):
            writer.write(vf, frame)

        assert target.read_bytes() == original_bytes

    def test_no_temp_files_left_after_failure(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
        tmp_path: Path,
    ) -> None:
        target = tmp_path / "test.mp4"
        shutil.copy2(str(sample_video), str(target))

        vf = loader.load(str(target))
        frame = extractor.extract(vf, type("Pos", (), {"offset_ms": 0})())

        with patch(
            "framecover.core.atomic_write.os.replace",
            side_effect=OSError("simulated replace failure"),
        ):
            writer.write(vf, frame)

        leftover_tmp = glob.glob(str(tmp_path / ".vt_*.tmp"))
        assert leftover_tmp == [], f"Temp files not cleaned up: {leftover_tmp}"
