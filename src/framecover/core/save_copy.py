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
from contextlib import contextmanager
from dataclasses import replace
from pathlib import Path
from typing import TYPE_CHECKING, Callable, Generator

from framecover.models import ApplyError, ApplyResult, VideoFile

if TYPE_CHECKING:
    from PIL import Image

    from framecover.core.thumbnail_writer import FormatDispatchThumbnailWriter

__all__ = ["save_copy"]

logger = logging.getLogger(__name__)


@contextmanager
def _stage(
    name: str, callback: Callable[[str, float | None], None] | None,
) -> Generator[None]:
    if callback is None:
        yield
        return
    started = time.monotonic()
    callback(name, None)
    try:
        yield
    finally:
        callback(name, time.monotonic() - started)


def _digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def _receipt_path(destination: Path) -> Path:
    # Stable on-disk name from before the FrameCover rename, so copies saved by
    # earlier versions are still recognised.
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


def _publish_exclusive(source: Path, destination: Path) -> None:
    try:
        os.link(source, destination, follow_symlinks=False)
        return
    except (OSError, NotImplementedError) as exc:
        unsupported = (
            isinstance(exc, NotImplementedError)
            or getattr(exc, "errno", None) in {
                errno.EOPNOTSUPP, errno.ENOTSUP, errno.ENOSYS, errno.EPERM, errno.EXDEV,
            }
            or getattr(exc, "winerror", None) in {1, 50, 1314}
        )
        if not unsupported or source.is_symlink() or not source.is_file():
            raise

    # FAT/exFAT cannot publish via hardlink. Exclusive creation still prevents
    # clobbering; unlike a hardlink, readers can see the copy before completion.
    # No portable filesystem API locks out arbitrary external writers.
    fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "wb") as output, source.open("rb") as original:
            shutil.copyfileobj(original, output)
            output.flush()
            os.fsync(output.fileno())
    except Exception as exc:
        # A pathname cleanup could delete a concurrent writer's replacement.
        raise OSError(
            getattr(exc, "errno", None) or errno.EIO,
            f"Copy failed; possible incomplete output retained at: "
            f"{destination}. {exc}",
        ) from exc


def _restore_backup(backup: Path, destination: Path) -> str | None:
    try:
        _publish_exclusive(backup, destination)
    except Exception:
        logger.warning("Previous destination retained at %s", backup, exc_info=True)
        return f" Previous destination retained at: {backup}"
    try:
        backup.unlink()
    except OSError:
        logger.warning("Restored backup also retained at %s", backup, exc_info=True)
    return None


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
    Verified outputs are captured in a private backup before replacement, so a
    racing unrelated output is never overwritten. Failed restoration retains that
    backup and reports its path. Filesystems without hardlinks use exclusive-create
    copying: no clobbering, but publication is not atomically visible to readers.
    Interrupted exclusive copies are retained and reported for manual recovery.
    Arbitrary external writers cannot be universally locked out by portable APIs.

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
    backup: Path | None = None
    backup_directory: Path | None = None
    try:
        source = Path(video.path)
        output = Path(os.path.abspath(destination))
        _refuse_source_alias(source, output)
        with _stage("Verifying source and destination", on_progress):
            source_digest = _digest(source)
            reuse = output.exists() and not overwrite
            if reuse and not _verified(output, source_digest):
                raise FileExistsError(f"Destination already exists: {output}")

        with tempfile.NamedTemporaryFile(
            dir=output.parent, prefix=f".{output.stem}.save-",
            suffix=output.suffix or source.suffix, delete=False,
        ) as stream:
            temporary = Path(stream.name)
        with _stage("Copying source video", on_progress):
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
        with _stage("Verifying saved copy", on_progress):
            output_digest = _digest(temporary)
            _refuse_source_alias(source, output)
            if reuse:
                try:
                    receipt = json.loads(
                        _receipt_path(output).read_text(encoding="utf-8")
                    )
                except (OSError, ValueError, TypeError) as exc:
                    raise FileExistsError(
                        f"Copy receipt changed while saving: {output}"
                    ) from exc
                if (
                    not isinstance(receipt, dict)
                    or receipt.get("version") != 1
                    or receipt.get("source_sha256") != source_digest
                ):
                    raise FileExistsError(
                        f"Copy receipt changed while saving: {output}"
                    )
                backup_directory = Path(tempfile.mkdtemp(
                    dir=output.parent, prefix=f".{output.name}.backup-",
                ))
                captured = backup_directory / output.name
                os.replace(output, captured)
                backup = captured
                _refuse_source_alias(source, backup)
                if (
                    not backup.is_file()
                    or _digest(backup) != receipt.get("output_sha256")
                ):
                    raise FileExistsError(f"Destination changed while saving: {output}")
        if overwrite:
            os.replace(temporary, output)
        else:
            _publish_exclusive(temporary, output)
        result.output_path = str(output)
        if backup is not None:
            try:
                backup.unlink()
            except OSError:
                logger.warning(
                    "Could not remove verified backup %s", backup, exc_info=True
                )
            backup = None
        try:
            _write_receipt(output, source_digest, output_digest)
        except Exception:
            logger.warning("Could not save copy receipt for %s", output, exc_info=True)
        return result
    except Exception as exc:
        recovery_message = ""
        if backup is not None:
            recovery_message = _restore_backup(backup, output) or ""
        if isinstance(exc, FileExistsError):
            code = ApplyError.DESTINATION_EXISTS
        elif isinstance(exc, PermissionError):
            code = ApplyError.FILE_NOT_WRITABLE
        elif isinstance(exc, OSError) and exc.errno == errno.ENOSPC:
            code = ApplyError.DISK_FULL
        else:
            code = ApplyError.UNEXPECTED
        result = ApplyResult(
            success=False, error_code=code,
            error_message=str(exc) + recovery_message,
        )
        return result
    finally:
        result.elapsed_ms = int((time.monotonic() - started) * 1000)
        if temporary is not None:
            try:
                temporary.unlink(missing_ok=True)
            except OSError:
                logger.warning(
                    "Could not remove staged copy %s", temporary, exc_info=True
                )
        if backup_directory is not None:
            try:
                backup_directory.rmdir()
            except OSError:
                pass  # A retained backup must remain available for recovery.
