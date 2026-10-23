"""Check GitHub for a newer published FrameCover release."""

from __future__ import annotations

import json
import logging
import re
import urllib.error
import urllib.request
from collections.abc import Callable
from typing import Any, Protocol

from framecover.exceptions import UpdateCheckError
from framecover.models import UpdateInfo, UpdateStatus

__all__ = [
    "GITHUB_REPOSITORY",
    "RELEASES_URL",
    "GitHubUpdateChecker",
    "UpdateChecker",
    "parse_version",
]

GITHUB_REPOSITORY = "fgagarin/framecover"
RELEASES_URL = f"https://github.com/{GITHUB_REPOSITORY}/releases/latest"

_API_URL = f"https://api.github.com/repos/{GITHUB_REPOSITORY}/releases/latest"
_VERSION_RE = re.compile(r"^v?(\d+(?:\.\d+)*)")
_OFFLINE_MESSAGE = (
    "FrameCover couldn't reach GitHub to look for updates. Check your internet "
    "connection and click the update indicator to try again."
)
_UNAVAILABLE_MESSAGE = (
    "GitHub didn't answer the update check right now. FrameCover keeps working "
    "normally; click the update indicator later to try again."
)
_UNREADABLE_MESSAGE = (
    "FrameCover received an unexpected answer while looking for updates. You can "
    "check the releases page manually."
)

_log = logging.getLogger(__name__)

Opener = Callable[..., Any]


class UpdateChecker(Protocol):
    """Looks up the latest published release."""

    def check(self, current_version: str) -> UpdateInfo:
        """Compare ``current_version`` with the latest published release.

        Args:
            current_version: Version of the running application.

        Returns:
            The comparison result.

        Raises:
            UpdateCheckError: If the release information cannot be retrieved.
        """
        ...


def parse_version(version: str) -> tuple[int, ...]:
    """Parse a release version such as ``v1.2.3`` into comparable numbers.

    Args:
        version: Version string, optionally prefixed with ``v`` and followed by
            a pre-release or build suffix that is ignored.

    Returns:
        The numeric components with trailing zeros removed, so ``1.2`` and
        ``1.2.0`` compare equal.

    Raises:
        ValueError: If the string does not start with a numeric version.
    """
    match = _VERSION_RE.match(version.strip())
    if match is None:
        raise ValueError(f"Not a version number: {version!r}")
    parts = [int(part) for part in match.group(1).split(".")]
    while len(parts) > 1 and parts[-1] == 0:
        parts.pop()
    return tuple(parts)


class GitHubUpdateChecker:
    """Reads the latest release from the GitHub REST API."""

    def __init__(
        self, *, timeout: float = 5.0, opener: Opener = urllib.request.urlopen
    ) -> None:
        """Create the checker.

        Args:
            timeout: Network timeout in seconds.
            opener: ``urllib.request.urlopen``-compatible callable; replaceable
                in tests.
        """
        self._timeout = timeout
        self._opener = opener

    def check(self, current_version: str) -> UpdateInfo:
        """Compare ``current_version`` with the latest GitHub release.

        Args:
            current_version: Version of the running application.

        Returns:
            The comparison result. When nothing has been published yet the
            running version is reported as up to date.

        Raises:
            UpdateCheckError: If GitHub cannot be reached or answers with
                unusable data.
        """
        release = self._fetch_latest_release()
        if release is None:
            return UpdateInfo(
                UpdateStatus.UP_TO_DATE, current_version, None, RELEASES_URL
            )

        tag = release.get("tag_name")
        try:
            latest = parse_version(str(tag))
            current = parse_version(current_version)
        except ValueError as exc:
            raise UpdateCheckError(_UNREADABLE_MESSAGE) from exc

        if latest > current:
            status = UpdateStatus.UPDATE_AVAILABLE
        elif latest < current:
            status = UpdateStatus.NEWER_THAN_RELEASE
        else:
            status = UpdateStatus.UP_TO_DATE
        return UpdateInfo(
            status,
            current_version,
            str(tag).removeprefix("v"),
            _safe_release_url(release.get("html_url")),
        )

    def _fetch_latest_release(self) -> dict[str, Any] | None:
        request = urllib.request.Request(
            _API_URL,
            headers={
                "Accept": "application/vnd.github+json",
                "User-Agent": "FrameCover-update-check",
                "X-GitHub-Api-Version": "2022-11-28",
            },
        )
        try:
            with self._opener(request, timeout=self._timeout) as response:
                payload = json.loads(response.read().decode("utf-8"))
        except urllib.error.HTTPError as exc:
            if exc.code == 404:
                return None
            _log.warning("Update check failed with HTTP %s", exc.code)
            raise UpdateCheckError(_UNAVAILABLE_MESSAGE) from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            _log.warning("Update check could not reach GitHub: %s", exc)
            raise UpdateCheckError(_OFFLINE_MESSAGE) from exc
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise UpdateCheckError(_UNREADABLE_MESSAGE) from exc
        if not isinstance(payload, dict):
            raise UpdateCheckError(_UNREADABLE_MESSAGE)
        return payload


def _safe_release_url(url: object) -> str:
    """Only link to this repository's pages, never to arbitrary API data."""
    prefix = f"https://github.com/{GITHUB_REPOSITORY}/releases/"
    if isinstance(url, str) and url.startswith(prefix):
        return url
    return RELEASES_URL
