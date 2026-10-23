"""Unit tests for PyAVFrameExtractor."""

from __future__ import annotations

from pathlib import Path

import av
import pytest
from PIL import Image, ImageChops

from framecover.core.frame_extractor import PyAVFrameExtractor
from framecover.core.video_loader import PyAVVideoLoader
from framecover.models import TimelinePosition


@pytest.fixture()
def loader() -> PyAVVideoLoader:
    return PyAVVideoLoader()


@pytest.fixture()
def extractor() -> PyAVFrameExtractor:
    return PyAVFrameExtractor()


class TestFrameExtractor:
    @pytest.mark.parametrize("offset_ms", [0, 40, 2500, 5000])
    def test_threaded_seek_matches_sequential_decode(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        sample_video: Path,
        offset_ms: int,
    ) -> None:
        video = loader.load(str(sample_video))
        with av.open(str(sample_video)) as container:
            stream = container.streams.video[0]
            stream.codec_context.thread_count = 1
            frames = list(container.decode(stream))
            nearest = min(
                frames, key=lambda frame: abs(frame.time - offset_ms / 1000)
            )
            expected = nearest.to_image().convert("RGB")

        actual = extractor.extract(video, TimelinePosition(offset_ms=offset_ms))

        assert actual.mode == expected.mode
        assert actual.size == expected.size
        assert ImageChops.difference(actual, expected).getbbox() is None

    def test_extract_at_adjacent_positions_returns_adjacent_frames(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        first = extractor.extract(vf, TimelinePosition(offset_ms=0))
        second = extractor.extract(
            vf, TimelinePosition(offset_ms=vf.frame_step_ms)
        )

        assert ImageChops.difference(first, second).getbbox() is not None

    def test_extract_at_zero_returns_rgb_image(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        img = extractor.extract(vf, TimelinePosition(offset_ms=0))
        assert isinstance(img, Image.Image)
        assert img.mode == "RGB"

    def test_extract_at_mid_duration(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        mid = vf.duration_ms // 2
        img = extractor.extract(vf, TimelinePosition(offset_ms=mid))
        assert img is not None
        assert img.mode == "RGB"

    def test_extract_at_boundary(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        img = extractor.extract(vf, TimelinePosition(offset_ms=vf.duration_ms))
        assert img is not None

    def test_extract_negative_offset_raises(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        with pytest.raises(ValueError):
            extractor.extract(vf, TimelinePosition(offset_ms=-1))

    def test_extract_beyond_duration_raises(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        with pytest.raises(ValueError):
            extractor.extract(vf, TimelinePosition(offset_ms=vf.duration_ms + 1))

    def test_returned_image_dimensions_match_stream(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        img = extractor.extract(vf, TimelinePosition(offset_ms=0))
        assert img.width == vf.width
        assert img.height == vf.height
