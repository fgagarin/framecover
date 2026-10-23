"""Unit tests for the GitHub update checker and its Qt worker."""

from __future__ import annotations

import io
import json
import ssl
import urllib.error
from unittest.mock import MagicMock

import pytest

from framecover.core.update_checker import (
    RELEASES_URL,
    GitHubUpdateChecker,
    parse_version,
)
from framecover.exceptions import UpdateCheckError
from framecover.models import UpdateInfo, UpdateStatus
from framecover.ui.worker import UpdateCheckWorker

RELEASE_URL = "https://github.com/fgagarin/framecover/releases/tag/v0.2.0"


def _opener(payload: object | bytes = None, *, error: Exception | None = None):
    calls = []

    def opener(request, timeout, *, context):
        assert context.verify_mode == ssl.CERT_REQUIRED
        assert context.check_hostname
        assert context.get_ca_certs()
        calls.append((request, timeout))
        if error is not None:
            raise error
        body = payload if isinstance(payload, bytes) else json.dumps(payload).encode()
        return io.BytesIO(body)

    opener.calls = calls
    return opener


def _http_error(code: int) -> urllib.error.HTTPError:
    return urllib.error.HTTPError("https://api.github.com", code, "x", None, None)


@pytest.mark.parametrize(
    ("version", "expected"),
    [
        ("0.1.0", (0, 1)),
        ("v1.2.3", (1, 2, 3)),
        ("1.2", (1, 2)),
        ("v2.0.0-beta.1", (2,)),
        (" 10.4.1 ", (10, 4, 1)),
    ],
)
def test_parse_version(version, expected):
    assert parse_version(version) == expected


def test_parse_version_rejects_non_versions():
    with pytest.raises(ValueError):
        parse_version("latest")


@pytest.mark.parametrize(
    ("tag", "current", "status"),
    [
        ("v0.2.0", "0.1.0", UpdateStatus.UPDATE_AVAILABLE),
        ("v0.10.0", "0.9.0", UpdateStatus.UPDATE_AVAILABLE),
        ("v0.1.0", "0.1.0", UpdateStatus.UP_TO_DATE),
        ("v0.1", "0.1.0", UpdateStatus.UP_TO_DATE),
        ("v0.1.0", "0.2.0", UpdateStatus.NEWER_THAN_RELEASE),
    ],
)
def test_compares_latest_release_with_current_version(tag, current, status):
    opener = _opener({"tag_name": tag, "html_url": RELEASE_URL})

    info = GitHubUpdateChecker(opener=opener, timeout=3).check(current)

    assert info == UpdateInfo(status, current, tag.removeprefix("v"), RELEASE_URL)
    request, timeout = opener.calls[0]
    assert request.full_url == (
        "https://api.github.com/repos/fgagarin/framecover/releases/latest"
    )
    assert request.get_header("User-agent")
    assert timeout == 3


def test_no_published_release_counts_as_up_to_date():
    checker = GitHubUpdateChecker(opener=_opener(error=_http_error(404)))

    info = checker.check("0.1.0")

    assert info == UpdateInfo(UpdateStatus.UP_TO_DATE, "0.1.0", None, RELEASES_URL)


def test_bundled_certificates_work_without_system_roots(monkeypatch):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    assert not context.get_ca_certs()
    monkeypatch.setattr(ssl, "create_default_context", lambda: context)
    opener = _opener({"tag_name": "v0.2.0", "html_url": RELEASE_URL})

    info = GitHubUpdateChecker(opener=opener).check("0.1.0")

    assert info.status == UpdateStatus.UPDATE_AVAILABLE
    assert context.get_ca_certs()
    assert context.verify_mode == ssl.CERT_REQUIRED
    assert context.check_hostname


def test_foreign_release_links_fall_back_to_releases_page():
    opener = _opener({"tag_name": "v9.0.0", "html_url": "https://evil.example/x"})

    info = GitHubUpdateChecker(opener=opener).check("0.1.0")

    assert info.release_url == RELEASES_URL


@pytest.mark.parametrize(
    ("opener", "message"),
    [
        (_opener(error=urllib.error.URLError("offline")), "internet connection"),
        (_opener(error=TimeoutError()), "internet connection"),
        (
            _opener(
                error=urllib.error.URLError(
                    ssl.SSLCertVerificationError("certificate verify failed")
                )
            ),
            "date and time",
        ),
        (
            _opener(error=ssl.SSLCertVerificationError("certificate verify failed")),
            "date and time",
        ),
        (_opener(error=_http_error(403)), "try again"),
        (_opener(error=_http_error(500)), "try again"),
        (_opener(b"not json"), "releases page"),
        (_opener([1, 2]), "releases page"),
        (_opener({"tag_name": "nightly"}), "releases page"),
    ],
)
def test_failures_raise_user_friendly_errors(opener, message):
    with pytest.raises(UpdateCheckError, match=message):
        GitHubUpdateChecker(opener=opener).check("0.1.0")


def test_worker_emits_result():
    info = UpdateInfo(UpdateStatus.UP_TO_DATE, "0.1.0", None, RELEASES_URL)
    checker = MagicMock()
    checker.check.return_value = info
    worker = UpdateCheckWorker(checker, "0.1.0")
    finished = MagicMock()
    worker.signals.finished.connect(finished)

    worker.run()

    checker.check.assert_called_once_with("0.1.0")
    finished.assert_called_once_with(info)


@pytest.mark.parametrize(
    ("error", "message"),
    [
        (UpdateCheckError("Check your connection."), "Check your connection."),
        (RuntimeError("boom"), "Something went wrong while looking for updates"),
    ],
)
def test_worker_emits_user_message_on_failure(error, message):
    checker = MagicMock()
    checker.check.side_effect = error
    worker = UpdateCheckWorker(checker, "0.1.0")
    failed = MagicMock()
    worker.signals.error.connect(failed)

    worker.run()

    assert message in failed.call_args.args[0]
