"""Tests for safe MP4 conversion and FFmpeg encoder fallback."""

from __future__ import annotations

import subprocess
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest
from av.error import InvalidDataError

from video_thumbnailer.core.video_converter import FFmpegVideoConverter
from video_thumbnailer.exceptions import ConversionError, NoVideoStreamError

MODULE = "video_thumbnailer.core.video_converter"


@pytest.fixture()
def conversion(tmp_path):
    source = tmp_path / "source.m2ts"
    source.write_bytes(b"original video")
    target = tmp_path / "output.mp4"
    container = MagicMock()
    stream = MagicMock()
    stream.disposition.__and__.return_value = 0
    container.streams.video = [stream]
    with (
        patch(f"{MODULE}.av.open") as open_video,
        patch(f"{MODULE}.imageio_ffmpeg.get_ffmpeg_exe", return_value="ffmpeg"),
        patch(f"{MODULE}._run_ffmpeg") as run,
        patch(
            f"{MODULE}.FormatDispatchThumbnailWriter.concat_ineligibility_reason",
            return_value=None,
        ),
    ):
        open_video.return_value.__enter__.return_value = container
        yield FFmpegVideoConverter(), source, target, run, container


def _ffmpeg_error():
    return subprocess.CalledProcessError(1, ["ffmpeg"], stderr=b"unsupported codec")


def test_remux_first_and_preserve_source(conversion):
    converter, source, target, run, _ = conversion

    def write_output(command):
        Path(command[-1]).write_bytes(b"converted")

    run.side_effect = write_output
    progress = []
    assert converter.convert(str(source), str(target), on_progress=progress.append) == str(target)
    command = run.call_args.args[0]
    assert command[command.index("-c") + 1] == "copy"
    assert command[command.index("-map") + 1] == "0:V:0"
    assert "0:a?" in command
    assert "-nostdin" in command
    assert target.read_bytes() == b"converted"
    assert source.read_bytes() == b"original video"
    assert progress == ["Remuxing to MP4 without re-encoding"]
    assert list(target.parent.glob(".output-*.mp4")) == []


def test_falls_back_to_available_encoder(conversion):
    converter, source, target, run, _ = conversion
    run.side_effect = [
        _ffmpeg_error(),
        subprocess.CompletedProcess([], 0, stdout=b" V....D libx264\n A..... aac\n"),
        None,
    ]
    progress = []
    converter.convert(str(source), str(target), on_progress=progress.append)
    commands = [call.args[0] for call in run.call_args_list]
    assert "-encoders" in commands[1]
    assert commands[2][commands[2].index("-c:v") + 1] == "libx264"
    assert commands[2][commands[2].index("-c:a") + 1] == "aac"
    assert "yuv420p" in commands[2]
    assert commands[2][commands[2].index("-profile:v") + 1] == "high"
    assert progress[-1] == "Encoding MP4 with libx264 / AAC"


def test_remuxable_but_concat_incompatible_video_is_normalized(conversion):
    converter, source, target, run, _ = conversion
    run.side_effect = [
        None,
        subprocess.CompletedProcess([], 0, stdout=b" V....D libx264\n A..... aac\n"),
        None,
    ]
    with patch(
        f"{MODULE}.FormatDispatchThumbnailWriter.concat_ineligibility_reason",
        side_effect=["MPEG-2 cannot be concatenated", None],
    ):
        converter.convert(str(source), str(target))
    assert "libx264" in run.call_args.args[0]


def test_incompatible_encoded_output_is_not_published(conversion):
    converter, source, target, run, _ = conversion
    target.write_bytes(b"existing")
    run.side_effect = [
        _ffmpeg_error(),
        subprocess.CompletedProcess([], 0, stdout=b" V....D libx264\n A..... aac\n"),
        None,
    ]
    with patch(
        f"{MODULE}.FormatDispatchThumbnailWriter.concat_ineligibility_reason",
        return_value="invalid frame rate",
    ):
        with pytest.raises(ConversionError, match="incompatible output"):
            converter.convert(str(source), str(target), overwrite=True)
    assert target.read_bytes() == b"existing"
    assert list(target.parent.glob(".output-*.mp4")) == []


def test_incompatible_encoder_output_tries_next_encoder(conversion):
    converter, source, target, run, _ = conversion
    run.side_effect = [
        _ffmpeg_error(),
        subprocess.CompletedProcess(
            [], 0, stdout=b" V....D libx264\n V....D h264_nvenc\n A..... aac\n"
        ),
        None,
        None,
    ]
    with patch(
        f"{MODULE}.FormatDispatchThumbnailWriter.concat_ineligibility_reason",
        side_effect=["unsupported profile", None],
    ):
        converter.convert(str(source), str(target))
    assert "h264_nvenc" in run.call_args.args[0]


def test_unusable_encoder_falls_back_to_next_available(conversion):
    converter, source, target, run, _ = conversion
    run.side_effect = [
        _ffmpeg_error(),
        subprocess.CompletedProcess(
            [], 0, stdout=b" V....D h264_nvenc\n V....D h264_qsv\n A..... aac\n"
        ),
        _ffmpeg_error(),
        None,
    ]
    converter.convert(str(source), str(target))
    commands = [call.args[0] for call in run.call_args_list]
    assert "h264_nvenc" in commands[2]
    assert "h264_qsv" in commands[3]


@pytest.mark.parametrize(
    "encoders", [b" V....D libx264\n", b" A..... aac\n", b""]
)
def test_missing_codecs_reports_error_without_output(conversion, encoders):
    converter, source, target, run, _ = conversion
    run.side_effect = [
        _ffmpeg_error(),
        subprocess.CompletedProcess([], 0, stdout=encoders),
    ]
    with pytest.raises(ConversionError, match="H.264/AAC"):
        converter.convert(str(source), str(target))
    assert not target.exists()
    assert source.read_bytes() == b"original video"
    assert list(target.parent.glob(".output-*.mp4")) == []


def test_failed_encoding_preserves_existing_destination(conversion):
    converter, source, target, run, _ = conversion
    target.write_bytes(b"existing")
    run.side_effect = [
        _ffmpeg_error(),
        subprocess.CompletedProcess([], 0, stdout=b" V....D libx264\n A..... aac\n"),
        _ffmpeg_error(),
    ]
    with pytest.raises(ConversionError, match="All available H.264"):
        converter.convert(str(source), str(target), overwrite=True)
    assert target.read_bytes() == b"existing"
    assert list(target.parent.glob(".output-*.mp4")) == []


def test_encoder_crash_without_stderr_reports_exit_code(conversion):
    converter, source, target, run, _ = conversion
    run.side_effect = [
        _ffmpeg_error(),
        subprocess.CompletedProcess([], 0, stdout=b" V....D libx264\n A..... aac\n"),
        subprocess.CalledProcessError(-11, ["ffmpeg"], stderr=b""),
    ]
    with pytest.raises(ConversionError, match="libx264: exit code -11"):
        converter.convert(str(source), str(target))
    assert not target.exists()
    assert source.read_bytes() == b"original video"
    assert list(target.parent.glob(".output-*.mp4")) == []


def test_existing_destination_requires_approval(conversion):
    converter, source, target, run, _ = conversion
    target.write_bytes(b"existing")
    with pytest.raises(FileExistsError):
        converter.convert(str(source), str(target))
    run.assert_not_called()
    assert target.read_bytes() == b"existing"


def test_approved_overwrite_publishes_after_success(conversion):
    converter, source, target, run, _ = conversion
    target.write_bytes(b"existing")

    def write_output(command):
        assert target.read_bytes() == b"existing"
        Path(command[-1]).write_bytes(b"converted")

    run.side_effect = write_output
    converter.convert(str(source), str(target), overwrite=True)
    assert target.read_bytes() == b"converted"


@pytest.mark.parametrize("alias", [False, True])
def test_cannot_replace_source_even_with_approval(conversion, alias):
    converter, source, target, run, _ = conversion
    if alias:
        target.hardlink_to(source)
    else:
        target = source
    with pytest.raises(ValueError, match="source file"):
        converter.convert(str(source), str(target), overwrite=True)
    run.assert_not_called()
    assert source.read_bytes() == b"original video"


def test_rejects_non_mp4_output(conversion):
    converter, source, target, run, _ = conversion
    with pytest.raises(ValueError, match=".mp4 extension"):
        converter.convert(str(source), str(target.with_suffix(".avi")))
    run.assert_not_called()


def test_rejects_audio_only_input(conversion):
    converter, source, target, run, container = conversion
    container.streams.video = []
    with pytest.raises(NoVideoStreamError):
        converter.convert(str(source), str(target))
    run.assert_not_called()
    assert not target.exists()


def test_ffmpeg_missing_reports_actionable_error(conversion):
    converter, source, target, run, _ = conversion
    with patch(f"{MODULE}.imageio_ffmpeg.get_ffmpeg_exe", side_effect=RuntimeError("not found")):
        with pytest.raises(ConversionError, match="FFmpeg is unavailable"):
            converter.convert(str(source), str(target))
    run.assert_not_called()


def test_invalid_converted_output_is_not_published(conversion):
    converter, source, target, _, container = conversion
    with patch(f"{MODULE}.av.open") as open_video:
        invalid = MagicMock()
        invalid.streams.video = []
        open_video.return_value.__enter__.side_effect = [container, invalid]
        with pytest.raises(ConversionError, match="no playable video"):
            converter.convert(str(source), str(target))
    assert not target.exists()
    assert list(target.parent.glob(".output-*.mp4")) == []


def test_unreadable_converted_output_reports_domain_error(conversion):
    converter, source, target, _, container = conversion
    with patch(f"{MODULE}.av.open") as open_video:
        open_video.return_value.__enter__.side_effect = [
            container, InvalidDataError(1, "invalid output"),
        ]
        with pytest.raises(ConversionError, match="Invalid converted MP4"):
            converter.convert(str(source), str(target))
    assert not target.exists()
    assert list(target.parent.glob(".output-*.mp4")) == []


def test_concurrently_created_destination_is_not_overwritten(conversion):
    converter, source, target, run, _ = conversion

    def create_destination(command):
        target.write_bytes(b"other process")

    run.side_effect = create_destination
    with pytest.raises(FileExistsError):
        converter.convert(str(source), str(target))
    assert target.read_bytes() == b"other process"
