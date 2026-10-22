"""Copy-first thumbnail saving with content-verified output provenance."""

from __future__ import annotations

import errno
import hashlib
import json
import logging
import os
import shutil
import tempfile
import time
from dataclasses import replace
from pathlib import Path
from typing import TYPE_CHECKING, Callable

from video_thumbnailer.models import ApplyError, ApplyResult, VideoFile

if TYPE_CHECKING:
    from PIL import Image

    from video_thumbnailer.core.thumbnail_writer import FormatDispatchThumbnailWriter

__all__ = ["save_copy"]

logger = logging.getLogger(__name__)


def _digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def _receipt_path(destination: Path) -> Path:
    return destination.with_name(f".{destination.name}.video-thumbnailer.json")


def _verified(destination: Path, source_digest: str) -> bool:
    try:
        if destination.is_symlink() or not destination.is_file():
            return False
        receipt = json.loads(_receipt_path(destination).read_text(encoding="utf-8"))
        return (
            isinstance(receipt, dict)
            and receipt.get("version") == 1
            and receipt.get("source_sha256") == source_digest
            and receipt.get("output_sha256") == _digest(destination)
        )
    except (OSError, ValueError, TypeError):
        return False


def _write_receipt(destination: Path, source_digest: str, output_digest: str) -> None:
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w", encoding="utf-8", dir=destination.parent,
            prefix=f".{destination.name}.receipt-", suffix=".json", delete=False,
        ) as stream:
            temporary = Path(stream.name)
            json.dump(
                {
                    "version": 1,
                    "source_sha256": source_digest,
                    "output_sha256": output_digest,
                },
                stream,
            )
        os.replace(temporary, _receipt_path(destination))
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def _refuse_source_alias(source: Path, destination: Path) -> None:
    if (
        destination.is_symlink()
        or source.resolve() == destination.resolve()
        or (destination.exists() and source.samefile(destination))
    ):
        raise FileExistsError("Destination must not alias the source or be a symlink.")


def save_copy(
    writer: FormatDispatchThumbnailWriter,
    video: VideoFile,
    thumbnail: Image.Image,
    destination: str,
    *,
    overwrite: bool = False,
    position_ms: int | None = None,
    on_progress: Callable[[str, float | None], None] | None = None,
) -> ApplyResult:
    """Write a thumbnail to a staged copy, publishing only a successful result.

    Existing outputs require explicit overwrite or a receipt matching the complete
    source and output contents. Call this blocking service from a worker thread.

    Args:
        writer: Thumbnail embedding implementation.
        video: Source video, which is never modified.
        thumbnail: Selected thumbnail image.
        destination: Final output path.
        overwrite: Allow replacement of an unrelated output, but never the source.
        position_ms: Selected frame's timeline position.
        on_progress: Optional writer progress callback.

    Returns:
        ApplyResult including the final path on success and total elapsed time.
        Expected filesystem and writer failures are returned, not raised.
    """
    started = time.monotonic()
    result = ApplyResult(success=False)
    temporary: Path | None = None
    try:
        source = Path(video.path)
        output = Path(os.path.abspath(destination))
        _refuse_source_alias(source, output)
        source_digest = _digest(source)
        reuse = output.exists() and not overwrite
        if reuse and not _verified(output, source_digest):
            raise FileExistsError(f"Destination already exists: {output}")

        with tempfile.NamedTemporaryFile(
            dir=output.parent, prefix=f".{output.stem}.save-",
            suffix=output.suffix or source.suffix, delete=False,
        ) as stream:
            temporary = Path(stream.name)
        shutil.copyfile(source, temporary)
        if _digest(temporary) != source_digest:
            raise OSError("Source changed while copying; please try again.")
        result = writer.write(
            replace(video, path=str(temporary), is_writable=True),
            thumbnail, position_ms=position_ms, on_progress=on_progress,
        )
        result.output_path = None
        if not result.success:
            return result
        output_digest = _digest(temporary)
        _refuse_source_alias(source, output)
        if reuse and not _verified(output, source_digest):
            raise FileExistsError(f"Destination changed while saving: {output}")
        if overwrite or reuse:
            os.replace(temporary, output)
        else:
            # Atomic no-clobber publication, including a destination created
            # while the thumbnail writer was running.
            os.link(temporary, output)
        result.output_path = str(output)
        try:
            _write_receipt(output, source_digest, output_digest)
        except Exception:
            logger.warning("Could not save copy receipt for %s", output, exc_info=True)
        return result
    except Exception as exc:
        if isinstance(exc, FileExistsError):
            code = ApplyError.DESTINATION_EXISTS
        elif isinstance(exc, PermissionError):
            code = ApplyError.FILE_NOT_WRITABLE
        elif isinstance(exc, OSError) and exc.errno == errno.ENOSPC:
            code = ApplyError.DISK_FULL
        else:
            code = ApplyError.UNEXPECTED
        result = ApplyResult(success=False, error_code=code, error_message=str(exc))
        return result
    finally:
        result.elapsed_ms = int((time.monotonic() - started) * 1000)
        if temporary is not None:
            try:
                temporary.unlink(missing_ok=True)
            except OSError:
                logger.warning("Could not remove staged copy %s", temporary, exc_info=True)
