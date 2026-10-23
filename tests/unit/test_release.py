"""Release lifecycle tests using temporary repositories and real git-changelog."""

from __future__ import annotations

import importlib.util
import os
import shutil
import stat
import subprocess
import tarfile
from pathlib import Path

import pytest

_ROOT = Path(__file__).resolve().parents[2]
_SPEC = importlib.util.spec_from_file_location("release_tools", _ROOT / "scripts/release.py")
assert _SPEC is not None and _SPEC.loader is not None
release = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(release)


def git(*args):
    return subprocess.run(
        ["git", *args], check=True, capture_output=True, text=True, encoding="utf-8"
    ).stdout.strip()


@pytest.fixture()
def repository(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    # Isolate test commits from the developer's global signing/hook configuration.
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(tmp_path / "no-global-config"))
    monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")
    git("init", "-b", "main")
    git("config", "user.name", "Release Tests")
    git("config", "user.email", "release@example.invalid")
    git("remote", "add", "origin", "https://github.com/example/project.git")
    config = (_ROOT / "pyproject.toml").read_text("utf-8").split("[tool.git-changelog]")[1]
    Path("pyproject.toml").write_text(
        '[project]\nname = "framecover"\nversion = "0.1.0"\n'
        'requires-python = ">=3.13"\n[tool.git-changelog]' + config, encoding="utf-8"
    )
    Path("src/framecover").mkdir(parents=True)
    Path("src/framecover/__init__.py").write_text('__version__ = "0.1.0"\n')
    write_lock("0.1.0")
    git("add", ".")
    git("commit", "-m", "feat: initial application")
    real_run = release._run

    def run(*command):
        if command == ("test-uv", "lock"):
            write_lock(release._project_version())
            return ""
        return real_run(*command)

    monkeypatch.setattr(release, "_run", run)
    return tmp_path


def commit(message, filename="change.txt"):
    Path(filename).write_text(message, encoding="utf-8")
    git("add", filename)
    git("commit", "-m", message)


def write_lock(version):
    Path("uv.lock").write_text(
        'version = 1\nrevision = 3\nrequires-python = ">=3.13"\n'
        '[[package]]\nname = "framecover"\n'
        f'version = "{version}"\nsource = {{ virtual = "." }}\n'
    )


def test_explicit_release_updates_versions_commit_and_tag(repository):
    release.create_release("v0.2.0", "test-uv")
    assert release._project_version() == "0.2.0"
    assert '__version__ = "0.2.0"' in Path("src/framecover/__init__.py").read_text()
    assert 'version = "0.2.0"' in Path("uv.lock").read_text()
    assert "## [v0.2.0]" in Path("CHANGELOG.md").read_text()
    assert git("status", "--porcelain") == ""
    assert git("cat-file", "-t", "v0.2.0") == "tag"
    assert git("log", "-1", "--format=%s") == "chore(release): v0.2.0"
    assert "Co-authored-by: Copilot" in git("log", "-1", "--format=%B")
    release.release_notes("v0.2.0", Path("notes"))
    notes = Path("notes/release-notes.md").read_text()
    assert "initial application" in notes
    assert "<!-- insertion marker -->" not in notes
    assert (Path("notes") / "CHANGELOG.md").exists()


@pytest.mark.parametrize(
    "message,expected",
    [("fix: resolve crash", "1.0.1"), ("feat: add formats", "1.1.0"),
     ("feat!: change interface", "2.0.0")],
)
def test_auto_version_comes_from_conventional_commits(repository, message, expected):
    release.create_release("1.0.0", "test-uv")
    commit(message)
    release.create_release("", "test-uv")
    assert git("tag", "--list", f"v{expected}") == f"v{expected}"
    release.release_notes(f"v{expected}", Path("notes"))
    notes = Path("notes/release-notes.md").read_text()
    assert "initial application" not in notes
    assert f"## [v{expected}]" in notes


def test_auto_first_release(repository):
    expected = release._version(release._changelog("--bump", "auto", "--bumped-version"))
    release.create_release("", "test-uv")
    assert release._project_version() == expected
    assert git("tag", "--list") == f"v{expected}"


def test_release_updates_real_uv_lockfile(repository):
    uv = shutil.which("uv")
    if uv is None:
        pytest.skip("uv is not available")
    release.create_release("0.2.0", uv)
    release.release_notes("v0.2.0", Path("notes"))
    assert git("status", "--porcelain") == "?? notes/"


def test_dirty_tree_is_rejected_without_commits(repository):
    commit_before = git("rev-parse", "HEAD")
    Path("untracked").write_text("keep")
    with pytest.raises(ValueError, match="clean"):
        release.create_release("0.2.0", "test-uv")
    assert git("rev-parse", "HEAD") == commit_before
    assert git("tag", "--list") == ""


@pytest.mark.parametrize("version", ["0.0.1", "1.0", "1.0.0-rc.1", "01.2.3", "-x"])
def test_invalid_or_decreasing_version_is_rejected(repository, version):
    with pytest.raises(ValueError):
        release.create_release(version, "test-uv")
    assert not Path("CHANGELOG.md").exists()
    assert git("status", "--porcelain") == ""


def test_existing_tag_cannot_be_released_again(repository):
    release.create_release("0.1.0", "test-uv")
    commit("fix: new change")
    with pytest.raises(ValueError, match="already tagged"):
        release.create_release("0.1.0", "test-uv")


def test_no_commits_since_last_release_is_rejected(repository):
    release.create_release("0.1.0", "test-uv")
    with pytest.raises(ValueError, match="No commits"):
        release.create_release("", "test-uv")


def test_notes_reject_mismatched_tag_version(repository):
    git("tag", "v1.0.0")
    with pytest.raises(ValueError, match="project version"):
        release.release_notes("v1.0.0", Path("notes"))
    assert not Path("notes").exists()


@pytest.mark.parametrize("path,expected", [
    ("src/framecover/__init__.py", "package version"),
    ("uv.lock", "lockfile version"),
])
def test_notes_reject_mismatched_package_or_lock_version(repository, path, expected):
    release.create_release("0.2.0", "test-uv")
    file = Path(path)
    file.write_text(file.read_text().replace("0.2.0", "0.1.0"))
    with pytest.raises(ValueError, match=expected):
        release.release_notes("v0.2.0", Path("notes"))


def test_notes_reject_tag_not_at_head(repository):
    release.create_release("0.2.0", "test-uv")
    commit("fix: change after tag")
    with pytest.raises(ValueError, match="point at HEAD"):
        release.release_notes("v0.2.0", Path("notes"))


def test_failed_lock_update_creates_no_commit_or_tag(repository, monkeypatch):
    before = git("rev-parse", "HEAD")
    real_run = release._run

    def run(*command):
        if command == ("test-uv", "lock"):
            raise subprocess.CalledProcessError(1, command)
        return real_run(*command)

    monkeypatch.setattr(release, "_run", run)
    with pytest.raises(subprocess.CalledProcessError):
        release.create_release("0.2.0", "test-uv")
    assert git("rev-parse", "HEAD") == before
    assert git("tag", "--list") == ""


@pytest.mark.parametrize("platform", ["linux", "macos", "windows"])
def test_native_distribution_packaging(repository, platform):
    Path("dist").mkdir()
    if platform == "macos":
        binary = Path("dist/framecover.app/Contents/MacOS/framecover")
        binary.parent.mkdir(parents=True)
    else:
        binary = Path("dist") / (
            "framecover.exe" if platform == "windows" else "framecover"
        )
    binary.write_bytes(b"native executable")
    binary.chmod(0o755)
    release.package_distribution(platform, Path("assets"))
    asset, = Path("assets").iterdir()
    assert asset.name.startswith(f"framecover-0.1.0-{platform}")
    if platform == "windows":
        assert asset.read_bytes() == binary.read_bytes()
    else:
        with tarfile.open(asset) as archive:
            member = archive.getmember(str(binary.relative_to("dist")).replace("\\", "/"))
            assert member.mode == stat.S_IMODE(binary.stat().st_mode)
            if os.name != "nt":
                assert member.mode & 0o111
            extracted = archive.extractfile(member)
            assert extracted is not None
            assert extracted.read() == b"native executable"


def test_missing_distribution_fails(repository):
    with pytest.raises(FileNotFoundError):
        release.package_distribution("linux", Path("assets"))
