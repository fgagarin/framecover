"""Capture golden reference results from the Python FrameCover core.

Generates synthetic fixtures (same recipes as ``tests/conftest.py``) in a temp
directory, exercises the Python core APIs directly (loader, frame extractor,
thumbnail writer via ``save_copy``, converter) and writes encoder-agnostic
summaries to ``crates/framecover-core/tests/golden/<case>.json`` plus the exact
ffmpeg command lines run by the Python implementation to
``<case>.commands.json``.

Usage::

    uv run python scripts/golden/generate_golden.py [--ffmpeg PATH] [--out DIR]
        [--only CASE ...]

FFmpeg lookup: ``--ffmpeg``, then ``FRAMECOVER_FFMPEG`` / ``IMAGEIO_FFMPEG_EXE``,
then ``vendor/ffmpeg/ffmpeg[.exe]`` in this checkout or any parent directory
(so it works from a git worktree nested inside the main checkout). ffprobe must
sit next to ffmpeg. The chosen binary is exported as ``IMAGEIO_FFMPEG_EXE``
before ``framecover`` is imported, so the Python writer uses the same build the
Rust port uses.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_OUT = REPO_ROOT / "crates" / "framecover-core" / "tests" / "golden"
EXE = ".exe" if os.name == "nt" else ""
POSITION_MS = 2000
SECOND_POSITION_MS = 3000


def _find_ffmpeg(explicit: str | None) -> Path:
    for candidate in (
        explicit,
        os.environ.get("FRAMECOVER_FFMPEG"),
        os.environ.get("IMAGEIO_FFMPEG_EXE"),
    ):
        if candidate:
            return Path(candidate).resolve()
    for parent in [REPO_ROOT, *REPO_ROOT.parents]:
        path = parent / "vendor" / "ffmpeg" / f"ffmpeg{EXE}"
        if path.is_file():
            return path.resolve()
    sys.exit("ffmpeg not found: pass --ffmpeg or set FRAMECOVER_FFMPEG")


# ---------------------------------------------------------------------------
# Command logging
# ---------------------------------------------------------------------------


class Normaliser:
    """Replace volatile paths/names with stable placeholders."""

    def __init__(self, ffmpeg: Path, ffprobe: Path) -> None:
        self.ffmpeg = ffmpeg
        self.ffprobe = ffprobe
        self.work: Path | None = None

    def __call__(self, text: str) -> str:
        replacements: list[tuple[str, str]] = []
        for exe, token in ((self.ffmpeg, "<FFMPEG>"), (self.ffprobe, "<FFPROBE>")):
            replacements += [(str(exe), token), (exe.as_posix(), token)]
        if self.work is not None:
            for form in {str(self.work), self.work.as_posix(),
                         str(self.work.resolve()), self.work.resolve().as_posix()}:
                replacements.append((form, "<WORK>"))
        for old, new in sorted(replacements, key=lambda r: -len(r[0])):
            text = text.replace(old, new)
        text = text.replace("<WORK>\\", "<WORK>/")
        # tempfile random parts: mkstemp/NamedTemporaryFile/mkdtemp use 8 chars
        # of [a-z0-9_]; ".vt_cover_abcd1234.jpg" -> ".vt_cover_XXXXXXXX.jpg".
        text = re.sub(
            r"((?:\.vt_[a-z]+_)|(?:\.save-)|(?:\.receipt-)|(?:\.backup-)|"
            r"(?:\.vt_)|(?:-(?=[a-z0-9_]{8}\.mp4)))[a-z0-9_]{8}(?=[.\\/'\"]|$)",
            r"\1XXXXXXXX",
            text,
        )
        return text


class CommandLog:
    def __init__(self, norm: Normaliser) -> None:
        self.norm = norm
        self.entries: list[dict[str, Any]] = []

    def record(self, argv: list[str], returncode: int | None) -> None:
        argv = [str(a) for a in argv]
        # Windows reports negative exit codes as unsigned 32-bit (e.g. -22 as
        # 4294967274); store the signed value so goldens are platform-neutral.
        if returncode is not None and returncode >= 2**31:
            returncode -= 2**32
        entry: dict[str, Any] = {
            "argv": [self.norm(a) for a in argv],
            "returncode": returncode,
        }
        if "concat" in argv and "-i" in argv:
            manifest = Path(argv[argv.index("-i", argv.index("concat")) + 1])
            try:
                entry["concat_manifest"] = self.norm(
                    manifest.read_text(encoding="utf-8")
                ).splitlines()
            except OSError:
                pass
        self.entries.append(entry)


@contextmanager
def logging_subprocess(log: CommandLog) -> Iterator[None]:
    original = subprocess.run

    def wrapper(*args: Any, **kwargs: Any) -> Any:
        argv = list(args[0] if args else kwargs["args"])
        try:
            result = original(*args, **kwargs)
        except subprocess.CalledProcessError as exc:
            log.record(argv, exc.returncode)
            raise
        log.record(argv, result.returncode)
        return result

    subprocess.run = wrapper  # type: ignore[assignment]
    try:
        yield
    finally:
        subprocess.run = original  # type: ignore[assignment]


# ---------------------------------------------------------------------------
# Fixture generation (mirrors tests/conftest.py)
# ---------------------------------------------------------------------------

H264_AAC = ["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p"]
VP9_OPUS = ["-c:v", "libvpx-vp9", "-c:a", "libopus", "-b:v", "200k"]


class Fixtures:
    def __init__(self, ffmpeg: Path) -> None:
        self.ffmpeg = str(ffmpeg)

    def _run(self, args: list[str]) -> None:
        subprocess.run([self.ffmpeg, "-y", *args], check=True, capture_output=True)

    def generate(self, output: Path, extra: list[str], *, width: int = 320,
                 height: int = 240, duration: int = 5) -> Path:
        self._run([
            "-f", "lavfi", "-i",
            f"testsrc=size={width}x{height}:rate=25:duration={duration}",
            "-f", "lavfi", "-i", f"sine=frequency=440:duration={duration}",
            *extra, str(output),
        ])
        return output

    def for_format(self, directory: Path, fmt: str) -> Path:
        output = directory / f"sample.{fmt}"
        return self.generate(output, VP9_OPUS if fmt == "webm" else H264_AAC)

    def with_thumb(self, directory: Path) -> Path:
        base = self.generate(directory / "base.mp4", H264_AAC)
        cover = directory / "cover.jpg"
        self._run([
            "-f", "lavfi", "-i", "color=c=blue:size=320x240:rate=1:duration=1",
            "-frames:v", "1", "-q:v", "2", str(cover),
        ])
        output = directory / "with_thumb.mp4"
        self._run([
            "-i", str(base), "-i", str(cover), "-map", "0", "-map", "1",
            "-c", "copy", "-disposition:v:1", "attached_pic", str(output),
        ])
        base.unlink()
        cover.unlink()
        return output


# ---------------------------------------------------------------------------
# Summaries
# ---------------------------------------------------------------------------


def _ms(value: Any) -> int | None:
    try:
        return round(float(value) * 1000)
    except (TypeError, ValueError):
        return None


class Prober:
    def __init__(self, ffprobe: Path, norm: Normaliser) -> None:
        self.ffprobe = str(ffprobe)
        self.norm = norm

    def summary(self, path: Path) -> dict[str, Any]:
        raw = json.loads(subprocess.run(
            [self.ffprobe, "-v", "error", "-show_streams", "-show_format",
             "-of", "json", str(path)],
            check=True, capture_output=True,
        ).stdout)
        fmt = raw.get("format", {})
        ftags = fmt.get("tags", {})
        streams = []
        for s in raw.get("streams", []):
            tags = s.get("tags", {})
            streams.append({
                "index": s.get("index"),
                "codec_type": s.get("codec_type"),
                "codec_name": s.get("codec_name"),
                "profile": s.get("profile"),
                "pix_fmt": s.get("pix_fmt"),
                "width": s.get("width"),
                "height": s.get("height"),
                "time_base": s.get("time_base"),
                "avg_frame_rate": s.get("avg_frame_rate"),
                "sample_rate": s.get("sample_rate"),
                "channels": s.get("channels"),
                "nb_frames": int(s["nb_frames"]) if "nb_frames" in s else None,
                "start_time_ms": _ms(s.get("start_time")),
                "duration_ms": _ms(s.get("duration")),
                "attached_pic": s.get("disposition", {}).get("attached_pic"),
                "tags_filename": tags.get("filename"),
                "tags_mimetype": tags.get("mimetype"),
            })
        return {
            "format_name": fmt.get("format_name"),
            "duration_ms": _ms(fmt.get("duration")),
            "nb_streams": fmt.get("nb_streams"),
            "major_brand": ftags.get("major_brand"),
            "compatible_brands": ftags.get("compatible_brands"),
            "streams": streams,
        }


def mp4_tags(path: Path) -> dict[str, Any]:
    from io import BytesIO

    from mutagen.mp4 import MP4
    from PIL import Image

    from framecover.core.thumbnail_metadata import FRAME_NUMBER_TAG, POSITION_MS_TAG

    try:
        tags = MP4(str(path)).tags or {}
    except Exception as exc:  # noqa: BLE001
        return {"error": f"{type(exc).__name__}"}

    def freeform(key: str) -> str | None:
        values = tags.get(key)
        return bytes(values[0]).decode("ascii") if values else None

    covr = tags.get("covr")
    covr_info = None
    if covr:
        image = Image.open(BytesIO(bytes(covr[0])))
        covr_info = {
            "count": len(covr),
            "imageformat": int(covr[0].imageformat),
            "image_format": image.format,
            "width": image.width,
            "height": image.height,
        }
    return {
        "frame_number": freeform(FRAME_NUMBER_TAG),
        "position_ms": freeform(POSITION_MS_TAG),
        "covr": covr_info,
        "keys": sorted(tags.keys()),
    }


def video_summary(video: Any, norm: Normaliser) -> dict[str, Any]:
    thumb = video.existing_thumbnail
    return {
        "path": norm(video.path),
        "format": video.format.value,
        "duration_ms": video.duration_ms,
        "width": video.width,
        "height": video.height,
        "frame_step_ms": video.frame_step_ms,
        "is_writable": video.is_writable,
        "has_existing_thumbnail": thumb is not None,
        "existing_thumbnail_size": [thumb.width, thumb.height] if thumb else None,
        "thumbnail_frame_number": video.thumbnail_frame_number,
        "thumbnail_position_ms": video.thumbnail_position_ms,
    }


def result_summary(result: Any, norm: Normaliser) -> dict[str, Any]:
    return {
        "success": result.success,
        "error_code": result.error_code.value if result.error_code else None,
        "error_message": norm(result.error_message) if result.error_message else None,
        "duration_shift_ms": result.duration_shift_ms,
        "output_path": norm(result.output_path) if result.output_path else None,
    }


def receipt_summary(output: Path) -> dict[str, Any] | None:
    receipt = output.with_name(f".{output.name}.video-thumbnailer.json")
    if not receipt.exists():
        return None
    data = json.loads(receipt.read_text(encoding="utf-8"))
    return {
        "file_name": receipt.name,
        "keys": sorted(data.keys()),
        "version": data.get("version"),
    }


# ---------------------------------------------------------------------------
# Case runner
# ---------------------------------------------------------------------------


class Golden:
    def __init__(self, ffmpeg: Path, ffprobe: Path, out: Path) -> None:
        self.norm = Normaliser(ffmpeg, ffprobe)
        self.prober = Prober(ffprobe, self.norm)
        self.fixtures = Fixtures(ffmpeg)
        self.out = out

        from framecover.core.frame_extractor import PyAVFrameExtractor
        from framecover.core.save_copy import save_copy
        from framecover.core.thumbnail_writer import FormatDispatchThumbnailWriter
        from framecover.core.video_converter import FFmpegVideoConverter
        from framecover.core.video_loader import PyAVVideoLoader
        from framecover.models import TimelinePosition

        self.loader = PyAVVideoLoader()
        self.extractor = PyAVFrameExtractor()
        self.writer = FormatDispatchThumbnailWriter()
        self.converter = FFmpegVideoConverter()
        self.save_copy = save_copy
        self.Position = TimelinePosition

    # -- building blocks ---------------------------------------------------

    def describe_input(self, path: Path) -> dict[str, Any]:
        video = self.loader.load(str(path))
        info: dict[str, Any] = {
            "file_name": path.name,
            "probe": self.prober.summary(path),
            "load": video_summary(video, self.norm),
            "reencode_reason": self.writer.reencode_reason(video),
        }
        if path.suffix.lower() in (".mp4", ".mov", ".m4v"):
            info["mp4_tags"] = mp4_tags(path)
        return info

    def save(self, path: Path, position_ms: int, log: CommandLog) -> dict[str, Any]:
        video = self.loader.load(str(path))
        frame = self.extractor.extract(video, self.Position(position_ms))
        destination = path.with_name(f"{path.stem}-thumbnail{path.suffix}")
        before = set(os.listdir(path.parent))
        with logging_subprocess(log):
            result = self.save_copy(
                self.writer, video, frame, str(destination), position_ms=position_ms
            )
        after = set(os.listdir(path.parent))
        step: dict[str, Any] = {
            "input": path.name,
            "position_ms": position_ms,
            "extracted_frame_size": [frame.width, frame.height],
            "destination": destination.name,
            "result": result_summary(result, self.norm),
            "new_files_in_directory": sorted(self.norm(n) for n in after - before),
        }
        if result.success and destination.exists():
            step["output"] = self.describe_input(destination)
            step["receipt"] = receipt_summary(destination)
            step["source_unchanged"] = True  # save_copy never touches the source
        return step

    def convert(self, path: Path, log: CommandLog) -> dict[str, Any]:
        destination = path.with_name(f"{path.stem}-converted.mp4")
        stages: list[str] = []
        with logging_subprocess(log):
            try:
                produced = self.converter.convert(
                    str(path), str(destination), on_progress=stages.append
                )
                error = None
            except Exception as exc:  # noqa: BLE001
                produced, error = None, f"{type(exc).__name__}: {self.norm(str(exc))}"
        step: dict[str, Any] = {
            "input": path.name,
            "destination": destination.name,
            "returned_path": self.norm(produced) if produced else None,
            "error": error,
            "progress_stages": stages,
            "new_files_in_directory": sorted(
                self.norm(n) for n in os.listdir(path.parent)
                if n not in (path.name,)
            ),
        }
        if produced:
            step["output"] = self.describe_input(destination)
            step["concat_ineligibility_reason"] = (
                self.writer.concat_ineligibility_reason(str(destination))
            )
        return step

    # -- cases ---------------------------------------------------------------

    def cases(self) -> dict[str, tuple[str, Callable[[Path, CommandLog], dict]]]:
        def save_fmt(fmt: str) -> Callable[[Path, CommandLog], dict]:
            def run(d: Path, log: CommandLog) -> dict:
                src = self.fixtures.for_format(d, fmt)
                return {"fixture": self.describe_input(src),
                        "save": self.save(src, POSITION_MS, log)}
            return run

        def mp4_resave(d: Path, log: CommandLog) -> dict:
            src = self.fixtures.for_format(d, "mp4")
            first = self.save(src, POSITION_MS, log)
            copy = d / first["destination"]
            log.entries.append({"marker": "second save on the produced copy"})
            second = self.save(copy, SECOND_POSITION_MS, log)
            return {"fixture": self.describe_input(src), "first_save": first,
                    "second_save": second}

        def mkv_resave(d: Path, log: CommandLog) -> dict:
            src = self.fixtures.for_format(d, "mkv")
            first = self.save(src, POSITION_MS, log)
            log.entries.append({"marker": "second save on the produced copy"})
            second = self.save(d / first["destination"], SECOND_POSITION_MS, log)
            return {"fixture": self.describe_input(src), "first_save": first,
                    "second_save": second}

        def with_thumb(d: Path, log: CommandLog) -> dict:
            src = self.fixtures.with_thumb(d)
            return {"fixture": self.describe_input(src),
                    "save": self.save(src, POSITION_MS, log)}

        def custom(name: str, extra: list[str], **kw: int) -> Callable:
            def run(d: Path, log: CommandLog) -> dict:
                src = self.fixtures.generate(d / name, extra, **kw)
                return {"fixture": self.describe_input(src),
                        "save": self.save(src, POSITION_MS, log)}
            return run

        def mkv_two_audio(d: Path, log: CommandLog) -> dict:
            base = self.fixtures.generate(d / "base.mkv", H264_AAC)
            src = d / "two_audio.mkv"
            self.fixtures._run(["-i", str(base), "-map", "0:v", "-map", "0:a",
                                "-map", "0:a", "-c", "copy", str(src)])
            base.unlink()
            return {"fixture": self.describe_input(src),
                    "save": self.save(src, POSITION_MS, log)}

        def mov_resave(d: Path, log: CommandLog) -> dict:
            src = self.fixtures.for_format(d, "mov")
            first = self.save(src, POSITION_MS, log)
            log.entries.append({"marker": "second save on the produced copy"})
            second = self.save(d / first["destination"], SECOND_POSITION_MS, log)
            return {"fixture": self.describe_input(src), "first_save": first,
                    "second_save": second}

        def conv(name: str, extra: list[str]) -> Callable:
            def run(d: Path, log: CommandLog) -> dict:
                src = self.fixtures.generate(d / name, extra)
                return {"fixture": {"file_name": src.name,
                                    "probe": self.prober.summary(src)},
                        "convert": self.convert(src, log)}
            return run

        av1 = ["-c:v", "libaom-av1", "-cpu-used", "8", "-row-mt", "1",
               "-crf", "35", "-b:v", "0", "-pix_fmt", "yuv420p", "-c:a", "aac"]
        return {
            "save_mp4": ("H.264/AAC MP4: concat fast path, covr + freeform tags.",
                         save_fmt("mp4")),
            "save_mov": ("H.264/AAC MOV: non-.mp4 extension forces full re-encode.",
                         save_fmt("mov")),
            "save_mkv": ("H.264/AAC MKV: -attach cover.jpg attachment.",
                         save_fmt("mkv")),
            "save_avi": ("H.264/AAC AVI: plain stream-copy remux, no cover.",
                         save_fmt("avi")),
            "save_flv": ("H.264/AAC FLV: no-op, copy published unchanged.",
                         save_fmt("flv")),
            "save_webm": ("VP9/Opus WebM: no-op, copy published unchanged.",
                          save_fmt("webm")),
            "save_mp4_with_thumb": (
                "MP4 with a JPEG attached_pic but no FrameCover tags: full "
                "re-encode replacing the old cover.", with_thumb),
            "resave_mp4": ("Save MP4, then save again on the produced copy "
                           "(replace-existing-preview concat path).", mp4_resave),
            "resave_mov": ("Save MOV, then save again on the produced copy "
                           "(full re-encode both times).", mov_resave),
            "resave_mkv": ("Save MKV, then save again on the produced copy "
                           "(ffmpeg default stream selection drops the previous "
                           "cover; one new cover.jpg is attached).", mkv_resave),
            "save_mkv_two_audio": (
                "MKV with two audio tracks: the -attach remux uses default "
                "stream selection, so only one audio track survives.",
                mkv_two_audio),
            "save_mp4_yuv444p": (
                "H.264 High 4:4:4 MP4: concat ineligible, full re-encode.",
                custom("yuv444p.mp4", ["-c:v", "libx264", "-c:a", "aac",
                                       "-pix_fmt", "yuv444p"])),
            "save_mp4_hevc": (
                "HEVC MP4: concat ineligible (codec), full re-encode.",
                custom("hevc.mp4", ["-c:v", "libx265", "-c:a", "aac",
                                    "-pix_fmt", "yuv420p", "-tag:v", "hvc1"])),
            "save_mp4_av1": ("AV1 (libaom) 160x120 MP4: concat fast path with "
                             "libaom preview frame.",
                             custom("av1.mp4", av1, width=160, height=120)),
            "convert_ts": ("MPEG-TS H.264/AAC -> MP4: stream-copy remux accepted.",
                           conv("sample.ts", H264_AAC)),
            "convert_mpg": ("MPEG-PS MPEG-2/MP2 -> MP4: remux succeeds but is "
                            "concat-ineligible, so transcoded with libx264.",
                            conv("sample.mpg", ["-c:v", "mpeg2video", "-c:a",
                                                "mp2"])),
            "convert_wmv": ("ASF WMV2/WMAv2 -> MP4: remux fails, transcoded "
                            "with libx264.",
                            conv("sample.wmv", ["-c:v", "wmv2", "-c:a",
                                                "wmav2"])),
        }

    def run(self, only: list[str] | None) -> list[str]:
        self.out.mkdir(parents=True, exist_ok=True)
        written = []
        for name, (description, fn) in self.cases().items():
            if only and name not in only:
                continue
            print(f"[golden] {name} ...", flush=True)
            work = Path(tempfile.mkdtemp(prefix="fc-golden-"))
            self.norm.work = work
            log = CommandLog(self.norm)
            try:
                data = fn(work, log)
            finally:
                shutil.rmtree(work, ignore_errors=True)
            payload = {"case": name, "description": description, **data}
            commands = {"case": name, "commands": log.entries}
            for suffix, content in ((".json", payload), (".commands.json", commands)):
                (self.out / f"{name}{suffix}").write_text(
                    json.dumps(content, indent=2, sort_keys=False) + "\n",
                    encoding="utf-8",
                )
            written.append(name)
        return written


def environment(ffmpeg: Path) -> dict[str, Any]:
    import av
    import mutagen
    import PIL

    version = subprocess.run(
        [str(ffmpeg), "-hide_banner", "-version"], capture_output=True, check=True
    ).stdout.decode().splitlines()[0]
    return {
        "ffmpeg": version.split(" Copyright")[0],
        "python": sys.version.split()[0],
        "pyav": av.__version__,
        "pyav_libavformat": ".".join(map(str, av.library_versions["libavformat"])),
        "mutagen": mutagen.version_string,
        "pillow": PIL.__version__,
        "position_ms": POSITION_MS,
        "second_position_ms": SECOND_POSITION_MS,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--ffmpeg")
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--only", nargs="*")
    args = parser.parse_args()

    ffmpeg = _find_ffmpeg(args.ffmpeg)
    ffprobe = ffmpeg.with_name(f"ffprobe{EXE}")
    if not ffprobe.is_file():
        sys.exit(f"ffprobe not found next to {ffmpeg}")
    os.environ["IMAGEIO_FFMPEG_EXE"] = str(ffmpeg)

    golden = Golden(ffmpeg, ffprobe, args.out)
    written = golden.run(args.only)
    if not args.only:
        (args.out / "_environment.json").write_text(
            json.dumps(environment(ffmpeg), indent=2) + "\n", encoding="utf-8"
        )
    print(f"[golden] wrote {len(written)} case(s) to {args.out}")


if __name__ == "__main__":
    main()
