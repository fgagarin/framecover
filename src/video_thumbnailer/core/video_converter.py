"""Safe conversion of FFmpeg-readable video files to MP4."""

from __future__ import annotations

import logging
import os
import subprocess
import tempfile
from collections.abc import Callable
from pathlib import Path

import av
import imageio_ffmpeg  # type: ignore[import-untyped]
from av.error import FFmpegError

from video_thumbnailer.exceptions import ConversionError, NoVideoStreamError

__all__ = ["FFmpegVideoConverter"]

logger = logging.getLogger(__name__)

_CREATION_FLAGS = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
_H264_ENCODERS = (
    "libx264", "libopenh264", "h264_mf", "h264_videotoolbox",
    "h264_nvenc", "h264_qsv", "h264_amf",
)


def _run_ffmpeg(command: list[str]) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        command, check=True, capture_output=True, creationflags=_CREATION_FLAGS
    )


def _failure_details(error: subprocess.CalledProcessError) -> str:
    return (error.stderr or b"").decode(errors="replace").strip()[-4000:]


class FFmpegVideoConverter:
    """Convert video to MP4 without modifying the source file."""

    def convert(
        self,
        source: str,
        destination: str,
        *,
        overwrite: bool = False,
        on_progress: Callable[[str], None] | None = None,
    ) -> str:
        """Remux first, then try available H.264 encoders with AAC if needed.

        Only the first playable video and all audio tracks are retained.
        The destination is published only after FFmpeg completes successfully.

        Args:
            source: Path to an FFmpeg-readable video.
            destination: MP4 output path, distinct from the source.
            overwrite: Whether replacing an existing destination was approved.
            on_progress: Optional callback for human-readable stage updates.

        Returns:
            Absolute path to the converted MP4.

        Raises:
            ConversionError: If FFmpeg or the required encoders fail.
            NoVideoStreamError: If the input has no playable video.
            OSError: If input/output files cannot be accessed.
            ValueError: If the destination is invalid or aliases the source.
        """
        source_path = Path(source).resolve(strict=True)
        destination_path = Path(destination).resolve()
        if source_path == destination_path or (
            destination_path.exists() and source_path.samefile(destination_path)
        ):
            raise ValueError("The MP4 destination must not be the source file.")
        if destination_path.suffix.lower() != ".mp4":
            raise ValueError("The destination must have an .mp4 extension.")
        if destination_path.exists() and not overwrite:
            raise FileExistsError(f"Destination already exists: {destination_path}")

        with av.open(str(source_path)) as container:
            if not any(
                not bool(stream.disposition & stream.disposition.attached_pic)
                for stream in container.streams.video
            ):
                raise NoVideoStreamError(str(source_path))

        try:
            executable = imageio_ffmpeg.get_ffmpeg_exe()
        except RuntimeError as exc:
            raise ConversionError(f"FFmpeg is unavailable: {exc}") from exc

        fd, temporary = tempfile.mkstemp(
            prefix=f".{destination_path.stem}-",
            suffix=".mp4",
            dir=destination_path.parent,
        )
        os.close(fd)
        temporary_path = Path(temporary)
        try:
            common = [
                executable, "-hide_banner", "-loglevel", "error", "-nostdin", "-y",
                "-i", str(source_path), "-map", "0:V:0", "-map", "0:a?",
                "-map_chapters", "0",
            ]
            output = ["-movflags", "+faststart", str(temporary_path)]
            if on_progress is not None:
                on_progress("Remuxing to MP4 without re-encoding")
            try:
                _run_ffmpeg([*common, "-c", "copy", *output])
            except subprocess.CalledProcessError as exc:
                logger.warning("MP4 remux failed: %s", _failure_details(exc))
                self._transcode(common, output, on_progress, exc)

            with av.open(str(temporary_path)) as converted:
                if not converted.streams.video:
                    raise ConversionError("FFmpeg produced no playable video.")
            if overwrite:
                os.replace(temporary_path, destination_path)
            else:
                # Publish atomically without clobbering a concurrently created file.
                os.link(temporary_path, destination_path)
            return str(destination_path)
        except subprocess.CalledProcessError as exc:
            raise ConversionError(
                f"FFmpeg failed (exit {exc.returncode}): {_failure_details(exc)}"
            ) from exc
        except FFmpegError as exc:
            raise ConversionError(f"Invalid converted MP4: {exc}") from exc
        finally:
            temporary_path.unlink(missing_ok=True)

    @staticmethod
    def _transcode(
        common: list[str],
        output: list[str],
        on_progress: Callable[[str], None] | None,
        remux_error: subprocess.CalledProcessError,
    ) -> None:
        encoders = _run_ffmpeg(
            [common[0], "-hide_banner", "-encoders"]
        ).stdout.decode(errors="replace")
        available = {
            fields[1]
            for line in encoders.splitlines()
            if len(fields := line.split()) >= 2
            and len(fields[0]) == 6
        }
        candidates = [name for name in _H264_ENCODERS if name in available]
        if not candidates or "aac" not in available:
            raise ConversionError(
                "Lossless MP4 remux failed and this FFmpeg has no usable "
                "H.264/AAC encoders. Install an FFmpeg build with libx264 and AAC "
                "and select it using IMAGEIO_FFMPEG_EXE.\n"
                f"Remux error: {_failure_details(remux_error)}"
            )
        failures: list[str] = []
        for encoder in candidates:
            if on_progress is not None:
                on_progress(f"Encoding MP4 with {encoder} / AAC")
            quality = (
                ["-preset", "medium", "-crf", "20"]
                if encoder == "libx264" else ["-b:v", "5M"]
            )
            try:
                _run_ffmpeg([
                    *common, "-c:v", encoder, *quality,
                    "-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2", "-pix_fmt", "yuv420p",
                    "-c:a", "aac", "-b:a", "192k", *output,
                ])
                return
            except subprocess.CalledProcessError as exc:
                detail = f"{encoder}: {_failure_details(exc)}"
                logger.warning("MP4 encoding failed: %s", detail)
                failures.append(detail)
        raise ConversionError(
            "All available H.264 encoders failed:\n" + "\n".join(failures)
        )
