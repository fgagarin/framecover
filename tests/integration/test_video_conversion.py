"""Real FFmpeg conversion followed by the thumbnail-editing workflow."""

from __future__ import annotations

import subprocess

import av
import imageio_ffmpeg
import pytest

from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor
from video_thumbnailer.core.thumbnail_writer import FormatDispatchThumbnailWriter
from video_thumbnailer.core.video_converter import FFmpegVideoConverter
from video_thumbnailer.core.video_loader import PyAVVideoLoader
from video_thumbnailer.exceptions import UnsupportedFormatError
from video_thumbnailer.models import TimelinePosition, VideoFormat


@pytest.mark.parametrize(
    "codec,profile,transcode",
    [
        ("libx264", "high", False),
        ("libx264", "baseline", False),
        ("libx264", "main", False),
        ("mpeg2video", None, True),
        ("ffv1", None, True),
    ],
    ids=["h264-high", "h264-baseline", "h264-main", "mpeg2", "ffv1"],
)
def test_conversion_then_thumbnail_workflow(tmp_path, codec, profile, transcode):
    source = tmp_path / ("source.mkv" if codec == "ffv1" else "source.m2ts")
    target = tmp_path / "converted.mp4"
    subprocess.run(
        [
            imageio_ffmpeg.get_ffmpeg_exe(), "-y",
            "-f", "lavfi", "-i", "testsrc=size=96x64:rate=25:duration=1",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=1",
            "-c:v", codec,
            *(["-profile:v", profile] if profile else []),
            "-c:a", "pcm_s16le" if codec == "ffv1" else "aac",
            "-pix_fmt", "yuv420p",
            "-f", "matroska" if codec == "ffv1" else "mpegts", str(source),
        ],
        check=True, capture_output=True,
    )
    original = source.read_bytes()
    loader = PyAVVideoLoader()
    if codec != "ffv1":
        with pytest.raises(UnsupportedFormatError):
            loader.load(str(source))
    stages = []
    FFmpegVideoConverter().convert(str(source), str(target), on_progress=stages.append)
    assert source.read_bytes() == original
    assert len(stages) == (2 if transcode else 1)
    with av.open(str(target)) as container:
        assert container.streams.video[0].codec_context.name == "h264"
        assert container.streams.audio[0].codec_context.name == "aac"
        assert next(container.decode(video=0)).width == 96
    video = loader.load(str(target))
    assert video.format is VideoFormat.MP4
    assert abs(video.duration_ms - 1000) <= 100
    assert (video.width, video.height) == (96, 64)
    frame = PyAVFrameExtractor().extract(video, TimelinePosition(offset_ms=400))
    writer = FormatDispatchThumbnailWriter()
    assert writer.reencode_reason(video) is None
    with av.open(str(target)) as container:
        original_packets = [
            bytes(packet) for packet in container.demux(video=0) if packet.size
        ]
    with av.open(str(target)) as container:
        original_frames = [
            frame.to_image().tobytes() for frame in container.decode(video=0)
        ]
    for position in (400, 600):
        save_stages = []
        result = writer.write(
            video, frame, position_ms=position,
            on_progress=lambda name, elapsed: (
                save_stages.append(name) if elapsed is None else None
            ),
        )
        assert result.success, result.error_message
        assert "Encoding preview frame" in save_stages
        assert "Copying video streams" in save_stages
        assert "Encoding video" not in save_stages
        with av.open(str(target)) as container:
            packets = [
                bytes(packet) for packet in container.demux(video=0) if packet.size
            ]
        # The concat demuxer can insert parameter sets into the first source IDR.
        assert packets[2:] == original_packets[1:]
        with av.open(str(target)) as container:
            frames = list(container.decode(video=0))
        assert len(frames) == 26
        assert [frame.to_image().tobytes() for frame in frames[1:]] == original_frames
        video = loader.load(str(target))
        assert video.existing_thumbnail is not None
    assert source.read_bytes() == original


@pytest.mark.parametrize("audio_tracks", [0, 2])
def test_conversion_preserves_optional_audio_tracks(tmp_path, audio_tracks):
    source = tmp_path / "source.m2ts"
    target = tmp_path / "converted.mp4"
    inputs = []
    maps = ["-map", "0:v:0"]
    for index in range(audio_tracks):
        inputs.extend([
            "-f", "lavfi", "-i", f"sine=frequency={440 + index * 100}:duration=1",
        ])
        maps.extend(["-map", f"{index + 1}:a:0"])
    subprocess.run(
        [
            imageio_ffmpeg.get_ffmpeg_exe(), "-y",
            "-f", "lavfi", "-i", "testsrc=size=96x64:rate=25:duration=1",
            *inputs, *maps, "-c:v", "libx264", "-c:a", "aac",
            "-pix_fmt", "yuv420p", "-f", "mpegts", str(source),
        ],
        check=True, capture_output=True,
    )
    FFmpegVideoConverter().convert(str(source), str(target))
    with av.open(str(target)) as container:
        assert len(container.streams.video) == 1
        assert len(container.streams.audio) == audio_tracks
