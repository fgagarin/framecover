"""Create local releases and prepare tagged distribution assets."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tarfile
import tomllib
from pathlib import Path

from semver import Version

__all__ = ["main"]

_RELEASE_FILES = (
    "CHANGELOG.md", "pyproject.toml", "src/video_thumbnailer/__init__.py", "uv.lock",
)
_VERSION_PATTERN = r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"


def _run(*command: str) -> str:
    return subprocess.run(
        command, check=True, text=True, encoding="utf-8", stdout=subprocess.PIPE
    ).stdout.strip()


def _changelog(*arguments: str) -> str:
    return _run(sys.executable, "-m", "git_changelog", *arguments)


def _version(value: str) -> str:
    version = value.removeprefix("v")
    if re.fullmatch(_VERSION_PATTERN, version) is None:
        raise ValueError("Expected a stable version X.Y.Z (optional v prefix).")
    return version


def _project_version() -> str:
    with Path("pyproject.toml").open("rb") as stream:
        return str(tomllib.load(stream)["project"]["version"])


def _replace_version(path: Path, pattern: str, version: str) -> None:
    content, count = re.subn(
        pattern, rf'\g<1>"{version}"',
        path.read_text(encoding="utf-8"), flags=re.MULTILINE,
    )
    if count != 1:
        raise ValueError(f"Expected exactly one version declaration in {path}.")
    path.write_text(content, encoding="utf-8")


def create_release(requested: str, uv: str) -> None:
    """Generate release metadata, commit it and create an annotated local tag."""
    if _run("git", "status", "--porcelain"):
        raise ValueError("Working tree must be clean; commit or stash changes first.")
    if _run("git", "rev-parse", "--is-shallow-repository") == "true":
        raise ValueError("Full Git history is required; run git fetch --unshallow.")
    _run("git", "symbolic-ref", "--quiet", "HEAD")
    current = _version(_project_version())
    tags = _run("git", "tag", "--list").splitlines()
    version_tags = [
        name for name in tags
        if re.fullmatch(f"v?{_VERSION_PATTERN}", name)
    ]
    if version_tags:
        base = max(version_tags, key=lambda name: Version.parse(_version(name)))
        if not _run("git", "log", f"{base}..HEAD", "--format=%H"):
            raise ValueError("No commits since the latest release.")
    version = _version(
        requested or _changelog("--bump", "auto", "--bumped-version")
    )
    tag = f"v{version}"
    if tag in tags or version in tags:
        raise ValueError(f"Version {version} is already tagged.")
    if Version.parse(version) < Version.parse(current):
        raise ValueError(f"Version must not be lower than {current}.")
    if version_tags:
        latest = Version.parse(_version(base))
        if Version.parse(version) <= latest:
            raise ValueError(f"Version must be greater than latest tag {latest}.")
    # Render before editing version files so invalid history cannot partially bump.
    changelog = _changelog("--bump", tag)
    _replace_version(
        Path("pyproject.toml"), r'^(version\s*=\s*)"[^"]+"', version
    )
    _replace_version(
        Path("src/video_thumbnailer/__init__.py"),
        r'^(__version__\s*=\s*)"[^"]+"', version,
    )
    Path("CHANGELOG.md").write_text(changelog + "\n", encoding="utf-8")
    _run(uv, "lock")
    _run("git", "add", "--", *_RELEASE_FILES)
    _run(
        "git", "commit", "-m", f"chore(release): {tag}",
        "-m", "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>",
    )
    _run("git", "tag", "-a", tag, "-m", f"Release {tag}")
    print(f"Created {tag} locally. Review, then publish with:")
    print(f"  git push --atomic origin HEAD refs/tags/{tag}")


def release_notes(tag: str, output: Path) -> None:
    """Validate tagged versions and generate full changelog and release notes."""
    version = _version(tag)
    if tag != f"v{version}":
        raise ValueError("Release tags must use the vX.Y.Z format.")
    if _project_version() != version:
        raise ValueError("Tag and project version do not match.")
    declaration = re.search(
        r'^__version__\s*=\s*"([^"]+)"',
        Path("src/video_thumbnailer/__init__.py").read_text("utf-8"),
        flags=re.MULTILINE,
    )
    if declaration is None or declaration[1] != version:
        raise ValueError("Tag and package version do not match.")
    with Path("uv.lock").open("rb") as stream:
        packages = tomllib.load(stream)["package"]
    if not any(
        item["name"] == "video-thumbnailer" and item["version"] == version
        for item in packages
    ):
        raise ValueError("Tag and lockfile version do not match.")
    if _run("git", "rev-parse", f"{tag}^{{commit}}") != _run(
        "git", "rev-parse", "HEAD"
    ):
        raise ValueError("Release tag must point at HEAD.")
    changelog = _changelog()
    sections = re.split(r"(?=^## )", changelog, flags=re.MULTILINE)
    notes = next(
        (section for section in sections if section.startswith(f"## [{tag}]")),
        None,
    )
    if notes is None:
        raise ValueError(f"git-changelog did not produce an entry for {tag}.")
    output.mkdir(parents=True, exist_ok=True)
    (output / "CHANGELOG.md").write_text(changelog + "\n", encoding="utf-8")
    notes = notes.replace("<!-- insertion marker -->", "").strip()
    (output / "release-notes.md").write_text(notes + "\n", encoding="utf-8")


def package_distribution(platform: str, output: Path) -> None:
    """Package a native PyInstaller build, preserving Unix executable modes."""
    version = _version(_project_version())
    name = f"video-thumbnailer-{version}-{platform}"
    output.mkdir(parents=True, exist_ok=True)
    if platform == "windows":
        (output / f"{name}.exe").write_bytes(
            Path("dist/video-thumbnailer.exe").read_bytes()
        )
    else:
        source = Path("dist") / (
            "video-thumbnailer.app" if platform == "macos" else "video-thumbnailer"
        )
        with tarfile.open(output / f"{name}.tar.gz", "w:gz") as archive:
            archive.add(source, arcname=source.name)


def main() -> None:
    """Run local release creation, tag validation/notes, or native packaging."""
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create = commands.add_parser("create")
    create.add_argument("--version", default="")
    create.add_argument("--uv", default="uv")
    notes = commands.add_parser("notes")
    notes.add_argument("--tag", required=True)
    notes.add_argument("--output", type=Path, default=Path("release-assets"))
    package = commands.add_parser("package")
    package.add_argument(
        "--platform", choices=["linux", "macos", "windows"], required=True
    )
    package.add_argument("--output", type=Path, default=Path("release-assets"))
    args = parser.parse_args()
    try:
        if args.command == "create":
            create_release(args.version, args.uv)
        elif args.command == "notes":
            release_notes(args.tag, args.output)
        else:
            package_distribution(args.platform, args.output)
    except (ValueError, OSError, subprocess.CalledProcessError) as exc:
        parser.exit(1, f"Release failed: {exc}\n")


if __name__ == "__main__":
    main()
