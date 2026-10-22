"""Copy-first saves preserve originals and require verified output provenance."""

from __future__ import annotations

import errno
import json
from pathlib import Path
from unittest.mock import MagicMock

import pytest
from PIL import Image

from video_thumbnailer.core import save_copy as service
from video_thumbnailer.models import ApplyError, ApplyResult, VideoFile, VideoFormat

__all__: list[str] = []


@pytest.fixture
def setup_copy(tmp_path):
    source = tmp_path / "source.mp4"
    source.write_bytes(b"original video contents")
    destination = tmp_path / "saved.mp4"
    video = VideoFile(str(source), VideoFormat.MP4, 5000, 320, 240, None, False)
    thumbnail = Image.new("RGB", (10, 10))
    writer = MagicMock()

    def write(staged_video, image, **kwargs):
        staged = Path(staged_video.path)
        assert staged.parent == destination.parent
        assert staged.suffix == ".mp4"
        assert staged != source and staged != destination
        assert staged.read_bytes() == source.read_bytes()
        assert staged_video.is_writable
        assert image is thumbnail
        staged.write_bytes(b"modified video contents")
        return ApplyResult(success=True, duration_shift_ms=40)

    writer.write.side_effect = write
    return source, destination, video, thumbnail, writer


def _save(setup_copy, **kwargs):
    _, destination, video, thumbnail, writer = setup_copy
    return service.save_copy(writer, video, thumbnail, str(destination), **kwargs)


def _receipt(destination):
    return destination.with_name(f".{destination.name}.video-thumbnailer.json")


def test_success_preserves_source_and_verifies_repeat(setup_copy):
    source, destination, _, _, writer = setup_copy
    result = _save(setup_copy, position_ms=120)
    assert result.success
    assert result.output_path == str(destination)
    assert result.duration_shift_ms == 40
    assert destination.read_bytes() == b"modified video contents"
    assert source.read_bytes() == b"original video contents"
    assert writer.write.call_args.kwargs["position_ms"] == 120
    receipt = json.loads(_receipt(destination).read_text())
    assert len(receipt["source_sha256"]) == len(receipt["output_sha256"]) == 64
    assert _save(setup_copy).success
    assert writer.write.call_count == 2
    assert not list(destination.parent.glob("*.save-*"))


@pytest.mark.parametrize("failure", ["result", "exception"])
@pytest.mark.parametrize("existing", [False, True])
def test_failed_write_leaves_source_and_destination_intact(
    setup_copy, failure, existing,
):
    source, destination, _, _, writer = setup_copy
    if existing:
        destination.write_bytes(b"unrelated output")

    def fail(video, *args, **kwargs):
        Path(video.path).write_bytes(b"partial mutation")
        if failure == "exception":
            raise RuntimeError("writer failed")
        return ApplyResult(False, ApplyError.FFMPEG_ERROR, "writer failed")

    writer.write.side_effect = fail
    result = _save(setup_copy, overwrite=True)
    assert not result.success
    assert result.output_path is None
    assert source.read_bytes() == b"original video contents"
    if existing:
        assert destination.read_bytes() == b"unrelated output"
    else:
        assert not destination.exists()
    assert sorted(p.name for p in source.parent.iterdir()) == (
        ["saved.mp4", "source.mp4"] if existing else ["source.mp4"]
    )


@pytest.mark.parametrize(
    "receipt_content", [None, "{", "[]", "null", '{"version": 1}', '{"version": 2}'],
)
def test_unrelated_output_requires_explicit_overwrite(setup_copy, receipt_content):
    source, destination, _, _, writer = setup_copy
    destination.write_bytes(b"unrelated output")
    if receipt_content is not None:
        _receipt(destination).write_text(receipt_content)
    result = _save(setup_copy)
    assert result.error_code is ApplyError.DESTINATION_EXISTS
    assert not result.success
    assert destination.read_bytes() == b"unrelated output"
    writer.write.assert_not_called()
    assert _save(setup_copy, overwrite=True).success
    assert source.read_bytes() == b"original video contents"


@pytest.mark.parametrize("changed", ["source", "output"])
def test_stale_receipt_rejects_same_size_content_changes(setup_copy, changed):
    source, destination, _, _, writer = setup_copy
    assert _save(setup_copy).success
    path = source if changed == "source" else destination
    original = path.read_bytes()
    path.write_bytes(b"x" * len(original))
    writer.write.reset_mock()
    assert _save(setup_copy).error_code is ApplyError.DESTINATION_EXISTS
    writer.write.assert_not_called()
    assert path.read_bytes() == b"x" * len(original)


@pytest.mark.parametrize("alias", ["same", "hardlink", "symlink", "relative"])
@pytest.mark.parametrize("overwrite", [False, True])
def test_refuses_source_aliases(setup_copy, alias, overwrite):
    source, destination, video, thumbnail, writer = setup_copy
    if alias == "same":
        destination = source
    elif alias == "relative":
        destination = source.parent / "." / source.name
    elif alias == "hardlink":
        destination.hardlink_to(source)
    else:
        destination.symlink_to(source)
    result = service.save_copy(
        writer, video, thumbnail, str(destination), overwrite=overwrite,
    )
    assert result.error_code is ApplyError.DESTINATION_EXISTS
    assert source.read_bytes() == b"original video contents"
    writer.write.assert_not_called()


def test_no_clobber_publication_catches_racing_destination(setup_copy):
    _, destination, _, _, writer = setup_copy
    original_write = writer.write.side_effect

    def racing_write(*args, **kwargs):
        result = original_write(*args, **kwargs)
        destination.write_bytes(b"racing output")
        return result

    writer.write.side_effect = racing_write
    result = _save(setup_copy)
    assert result.error_code is ApplyError.DESTINATION_EXISTS
    assert destination.read_bytes() == b"racing output"
    assert not _receipt(destination).exists()
    assert len(list(destination.parent.iterdir())) == 2


def test_verified_output_changed_during_write_is_not_replaced(setup_copy):
    _, destination, _, _, writer = setup_copy
    assert _save(setup_copy).success
    original_write = writer.write.side_effect

    def racing_write(*args, **kwargs):
        result = original_write(*args, **kwargs)
        destination.write_bytes(b"racing output")
        return result

    writer.write.side_effect = racing_write
    assert _save(setup_copy).error_code is ApplyError.DESTINATION_EXISTS
    assert destination.read_bytes() == b"racing output"


def test_receipt_failure_warns_and_cannot_authorize_reuse(setup_copy, monkeypatch, caplog):
    _, destination, _, _, _ = setup_copy

    def fail(*args):
        raise OSError("receipt is unwritable")

    monkeypatch.setattr(service, "_write_receipt", fail)
    assert _save(setup_copy).success
    assert "Could not save copy receipt" in caplog.text
    assert not _receipt(destination).exists()
    assert _save(setup_copy).error_code is ApplyError.DESTINATION_EXISTS


def test_copy_failure_cleans_staging_file(setup_copy, monkeypatch):
    source, destination, _, _, writer = setup_copy

    def fail(*args):
        raise OSError(errno.ENOSPC, "disk full")

    monkeypatch.setattr(service.shutil, "copyfile", fail)
    result = _save(setup_copy)
    assert result.error_code is ApplyError.DISK_FULL
    assert not destination.exists()
    assert list(source.parent.iterdir()) == [source]
    writer.write.assert_not_called()


def test_elapsed_includes_hash_and_copy(setup_copy, monkeypatch):
    times = iter([10.0, 12.5])
    monkeypatch.setattr(service.time, "monotonic", lambda: next(times))
    assert _save(setup_copy).elapsed_ms == 2500
