//! GitHub release update check (port of `core/update_checker.py`).
//!
//! # TLS trust
//!
//! The request goes through `ureq` + `rustls`. Certificates are first checked
//! with the operating system's verifier (`rustls-platform-verifier`: Windows
//! CryptoAPI, macOS Security.framework, the system CA bundle on Linux), so
//! corporate/managed networks that install their own root CA for HTTPS
//! filtering keep working. If that verification fails, the check is retried
//! once with the bundled Mozilla roots (`webpki-roots`) — this covers machines
//! whose system store is missing or broken (minimal Linux installs, the
//! reason the Python build bundled `certifi`). Only if both fail is the
//! certificate message shown.

use std::time::Duration;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::models::{UpdateInfo, UpdateStatus};

/// GitHub `owner/name` of the project.
pub const GITHUB_REPOSITORY: &str = "fgagarin/framecover";
/// Human-facing page with the latest release.
pub const RELEASES_URL: &str = "https://github.com/fgagarin/framecover/releases/latest";
/// REST endpoint queried for the latest release.
pub const API_URL: &str = "https://api.github.com/repos/fgagarin/framecover/releases/latest";
/// Default network timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

const OFFLINE_MESSAGE: &str = "FrameCover couldn't reach GitHub to look for updates. Check your internet \
connection and click the update indicator to try again.";
const UNAVAILABLE_MESSAGE: &str = "GitHub didn't answer the update check right now. FrameCover keeps working \
normally; click the update indicator later to try again.";
const UNREADABLE_MESSAGE: &str = "FrameCover received an unexpected answer while looking for updates. You can \
check the releases page manually.";
const CERTIFICATE_MESSAGE: &str = "FrameCover couldn't verify GitHub's secure connection. Check your computer's \
date and time. If you use a managed network, ask your administrator about \
HTTPS filtering. You can also check for an official update on the project's \
releases page in your browser.";

/// Request headers sent to the GitHub API.
pub const REQUEST_HEADERS: [(&str, &str); 3] = [
    ("Accept", "application/vnd.github+json"),
    ("User-Agent", "FrameCover-update-check"),
    ("X-GitHub-Api-Version", "2022-11-28"),
];

/// Looks up the latest published release.
pub trait UpdateChecker: Send + Sync {
    /// Compare `current_version` with the latest published release.
    ///
    /// Errors are [`Error::UpdateCheck`] carrying a message for end users.
    fn check(&self, current_version: &str) -> Result<UpdateInfo>;
}

/// Parse a release version such as `v1.2.3` into comparable numbers.
///
/// An optional `v` prefix and any pre-release/build suffix are ignored;
/// trailing zeros are removed so `1.2` and `1.2.0` compare equal.
pub fn parse_version(version: &str) -> Result<Vec<u64>, String> {
    let s = version.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = Vec::new();
    let mut rest = s;
    loop {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            if parts.is_empty() {
                return Err(format!("Not a version number: {version:?}"));
            }
            break;
        }
        let number = rest[..digits]
            .parse::<u64>()
            .map_err(|_| format!("Not a version number: {version:?}"))?;
        parts.push(number);
        rest = &rest[digits..];
        // Continue only on `.<digit>`, like the regex `\d+(?:\.\d+)*`.
        match rest.strip_prefix('.') {
            Some(next) if next.starts_with(|c: char| c.is_ascii_digit()) => rest = next,
            _ => break,
        }
    }
    while parts.len() > 1 && parts.last() == Some(&0) {
        parts.pop();
    }
    Ok(parts)
}

/// A single HTTP GET as issued by [`GitHubUpdateChecker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub timeout: Duration,
}

/// Transport-level failure of an [`HttpRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The server answered with a non-success status code.
    Status(u16),
    /// The TLS certificate could not be verified.
    Certificate(String),
    /// DNS, connection, timeout or I/O failure.
    Network(String),
}

/// Performs the HTTP request and returns the response body.
pub type Fetcher = dyn Fn(&HttpRequest) -> Result<Vec<u8>, FetchError> + Send + Sync;

/// Reads the latest release from the GitHub REST API.
pub struct GitHubUpdateChecker {
    timeout: Duration,
    fetch: Box<Fetcher>,
}

impl Default for GitHubUpdateChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl GitHubUpdateChecker {
    /// Checker using the real network with the default 5 s timeout.
    pub fn new() -> Self {
        Self::with_fetcher(DEFAULT_TIMEOUT, Box::new(ureq_fetch))
    }

    /// Checker with a custom timeout and transport (used by tests).
    pub fn with_fetcher(timeout: Duration, fetch: Box<Fetcher>) -> Self {
        Self { timeout, fetch }
    }

    fn fetch_latest_release(&self) -> Result<Option<serde_json::Map<String, Value>>> {
        let request = HttpRequest {
            url: API_URL.to_owned(),
            headers: REQUEST_HEADERS
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            timeout: self.timeout,
        };
        let body = match (self.fetch)(&request) {
            Ok(body) => body,
            Err(FetchError::Status(404)) => return Ok(None),
            Err(FetchError::Status(code)) => {
                log::warn!("Update check failed with HTTP {code}");
                return Err(Error::UpdateCheck(UNAVAILABLE_MESSAGE.into()));
            }
            Err(FetchError::Certificate(detail)) => {
                log::warn!("Update check could not verify GitHub's certificate: {detail}");
                return Err(Error::UpdateCheck(CERTIFICATE_MESSAGE.into()));
            }
            Err(FetchError::Network(detail)) => {
                log::warn!("Update check could not reach GitHub: {detail}");
                return Err(Error::UpdateCheck(OFFLINE_MESSAGE.into()));
            }
        };
        let unreadable = || Error::UpdateCheck(UNREADABLE_MESSAGE.into());
        let text = std::str::from_utf8(&body).map_err(|_| unreadable())?;
        match serde_json::from_str::<Value>(text).map_err(|_| unreadable())? {
            Value::Object(map) => Ok(Some(map)),
            _ => Err(unreadable()),
        }
    }
}

impl UpdateChecker for GitHubUpdateChecker {
    fn check(&self, current_version: &str) -> Result<UpdateInfo> {
        let Some(release) = self.fetch_latest_release()? else {
            return Ok(UpdateInfo {
                status: UpdateStatus::UpToDate,
                current_version: current_version.to_owned(),
                latest_version: None,
                release_url: RELEASES_URL.to_owned(),
            });
        };

        let tag = match release.get("tag_name") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => "None".to_owned(),
        };
        let unreadable = |_| Error::UpdateCheck(UNREADABLE_MESSAGE.into());
        let latest = parse_version(&tag).map_err(unreadable)?;
        let current = parse_version(current_version).map_err(unreadable)?;

        let status = match latest.cmp(&current) {
            std::cmp::Ordering::Greater => UpdateStatus::UpdateAvailable,
            std::cmp::Ordering::Less => UpdateStatus::NewerThanRelease,
            std::cmp::Ordering::Equal => UpdateStatus::UpToDate,
        };
        Ok(UpdateInfo {
            status,
            current_version: current_version.to_owned(),
            latest_version: Some(tag.strip_prefix('v').unwrap_or(&tag).to_owned()),
            release_url: safe_release_url(release.get("html_url")),
        })
    }
}

/// Only link to this repository's release pages, never to arbitrary API data.
fn safe_release_url(url: Option<&Value>) -> String {
    let prefix = format!("https://github.com/{GITHUB_REPOSITORY}/releases/");
    match url {
        Some(Value::String(s)) if s.starts_with(&prefix) => s.clone(),
        _ => RELEASES_URL.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Real transport
// ---------------------------------------------------------------------------

fn ureq_fetch(request: &HttpRequest) -> Result<Vec<u8>, FetchError> {
    match fetch_with_roots(request, ureq::tls::RootCerts::PlatformVerifier) {
        Err(FetchError::Certificate(platform_err)) => {
            log::info!(
                "System certificate verification failed ({platform_err}); retrying with bundled roots"
            );
            fetch_with_roots(request, ureq::tls::RootCerts::WebPki)
        }
        other => other,
    }
}

fn fetch_with_roots(
    request: &HttpRequest,
    roots: ureq::tls::RootCerts,
) -> Result<Vec<u8>, FetchError> {
    let tls = ureq::tls::TlsConfig::builder().root_certs(roots).build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(request.timeout))
        .build()
        .into();
    let mut builder = agent.get(&request.url);
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    let mut response = builder.call().map_err(|e| classify(&e))?;
    response
        .body_mut()
        .with_config()
        .limit(10 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| classify(&e))
}

fn classify(err: &ureq::Error) -> FetchError {
    match err {
        ureq::Error::StatusCode(code) => FetchError::Status(*code),
        ureq::Error::Rustls(e) if is_certificate_error(e) => FetchError::Certificate(e.to_string()),
        ureq::Error::Io(io) if io_is_certificate_error(io) => {
            FetchError::Certificate(io.to_string())
        }
        other => FetchError::Network(other.to_string()),
    }
}

fn io_is_certificate_error(io: &std::io::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = io.get_ref().map(|e| e as _);
    while let Some(e) = source {
        if let Some(tls) = e.downcast_ref::<rustls::Error>() {
            return is_certificate_error(tls);
        }
        if let Some(inner) = e.downcast_ref::<ureq::Error>() {
            return matches!(classify(inner), FetchError::Certificate(_));
        }
        source = e.source();
    }
    false
}

fn is_certificate_error(err: &rustls::Error) -> bool {
    matches!(
        err,
        rustls::Error::InvalidCertificate(_)
            | rustls::Error::InvalidCertRevocationList(_)
            | rustls::Error::NoCertificatesPresented
            | rustls::Error::UnsupportedNameType
    ) || matches!(
        err,
        rustls::Error::AlertReceived(alert) if matches!(
            alert,
            rustls::AlertDescription::BadCertificate
                | rustls::AlertDescription::UnknownCA
                | rustls::AlertDescription::CertificateExpired
                | rustls::AlertDescription::CertificateUnknown
        )
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    const RELEASE_URL: &str = "https://github.com/fgagarin/framecover/releases/tag/v0.2.0";

    type Calls = Arc<Mutex<Vec<HttpRequest>>>;

    fn fetcher(result: Result<Vec<u8>, FetchError>) -> (Box<Fetcher>, Calls) {
        let calls: Calls = Arc::default();
        let seen = calls.clone();
        let f = move |req: &HttpRequest| {
            seen.lock().unwrap().push(req.clone());
            result.clone()
        };
        (Box::new(f), calls)
    }

    fn json_fetcher(payload: Value) -> (Box<Fetcher>, Calls) {
        fetcher(Ok(payload.to_string().into_bytes()))
    }

    fn checker(result: Result<Vec<u8>, FetchError>) -> GitHubUpdateChecker {
        GitHubUpdateChecker::with_fetcher(DEFAULT_TIMEOUT, fetcher(result).0)
    }

    #[test]
    fn parses_versions() {
        let cases: [(&str, &[u64]); 7] = [
            ("0.1.0", &[0, 1]),
            ("v1.2.3", &[1, 2, 3]),
            ("1.2", &[1, 2]),
            ("v2.0.0-beta.1", &[2]),
            (" 10.4.1 ", &[10, 4, 1]),
            ("0", &[0]),
            ("1.2.", &[1, 2]),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_version(input).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn rejects_non_versions() {
        for bad in ["latest", "", "v", "vv1", "None"] {
            assert!(parse_version(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn compares_latest_release_with_current_version() {
        let cases = [
            ("v0.2.0", "0.1.0", UpdateStatus::UpdateAvailable),
            ("v0.10.0", "0.9.0", UpdateStatus::UpdateAvailable),
            ("v0.1.0", "0.1.0", UpdateStatus::UpToDate),
            ("v0.1", "0.1.0", UpdateStatus::UpToDate),
            ("v0.1.0", "0.2.0", UpdateStatus::NewerThanRelease),
        ];
        for (tag, current, status) in cases {
            let (fetch, calls) =
                json_fetcher(serde_json::json!({"tag_name": tag, "html_url": RELEASE_URL}));
            let info = GitHubUpdateChecker::with_fetcher(Duration::from_secs(3), fetch)
                .check(current)
                .unwrap();

            assert_eq!(
                info,
                UpdateInfo {
                    status,
                    current_version: current.into(),
                    latest_version: Some(tag.trim_start_matches('v').into()),
                    release_url: RELEASE_URL.into(),
                }
            );
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            let req = &calls[0];
            assert_eq!(
                req.url,
                "https://api.github.com/repos/fgagarin/framecover/releases/latest"
            );
            assert!(
                req.headers
                    .iter()
                    .any(|(k, v)| k == "User-Agent" && !v.is_empty())
            );
            assert!(
                req.headers
                    .iter()
                    .any(|(k, v)| k == "Accept" && v == "application/vnd.github+json")
            );
            assert_eq!(req.timeout, Duration::from_secs(3));
        }
    }

    #[test]
    fn default_timeout_is_five_seconds() {
        let (fetch, calls) = json_fetcher(serde_json::json!({"tag_name": "v0.1.0"}));
        GitHubUpdateChecker::with_fetcher(DEFAULT_TIMEOUT, fetch)
            .check("0.1.0")
            .unwrap();
        assert_eq!(calls.lock().unwrap()[0].timeout, Duration::from_secs(5));
    }

    #[test]
    fn no_published_release_counts_as_up_to_date() {
        let info = checker(Err(FetchError::Status(404)))
            .check("0.1.0")
            .unwrap();
        assert_eq!(
            info,
            UpdateInfo {
                status: UpdateStatus::UpToDate,
                current_version: "0.1.0".into(),
                latest_version: None,
                release_url: RELEASES_URL.into(),
            }
        );
    }

    #[test]
    fn foreign_release_links_fall_back_to_releases_page() {
        for url in [
            serde_json::json!("https://evil.example/x"),
            serde_json::json!("https://github.com/someone/else/releases/tag/v9"),
            serde_json::json!(42),
            Value::Null,
        ] {
            let (fetch, _) =
                json_fetcher(serde_json::json!({"tag_name": "v9.0.0", "html_url": url}));
            let info = GitHubUpdateChecker::with_fetcher(DEFAULT_TIMEOUT, fetch)
                .check("0.1.0")
                .unwrap();
            assert_eq!(info.release_url, RELEASES_URL);
        }
    }

    #[test]
    fn failures_raise_user_friendly_errors() {
        let cases: Vec<(Result<Vec<u8>, FetchError>, &str)> = vec![
            (
                Err(FetchError::Network("offline".into())),
                "internet connection",
            ),
            (
                Err(FetchError::Network("timed out".into())),
                "internet connection",
            ),
            (
                Err(FetchError::Certificate("certificate verify failed".into())),
                "date and time",
            ),
            (Err(FetchError::Status(403)), "try again"),
            (Err(FetchError::Status(500)), "try again"),
            (Ok(b"not json".to_vec()), "releases page"),
            (Ok(vec![0xff, 0xfe, 0x00]), "releases page"),
            (Ok(b"[1, 2]".to_vec()), "releases page"),
            (Ok(br#"{"tag_name": "nightly"}"#.to_vec()), "releases page"),
            (Ok(br#"{"name": "no tag"}"#.to_vec()), "releases page"),
        ];
        for (result, needle) in cases {
            let err = checker(result.clone()).check("0.1.0").unwrap_err();
            match err {
                Error::UpdateCheck(message) => {
                    assert!(message.contains(needle), "{result:?}: {message}")
                }
                other => panic!("unexpected error {other:?}"),
            }
        }
    }

    #[test]
    fn unparseable_current_version_is_unreadable() {
        let (fetch, _) = json_fetcher(serde_json::json!({"tag_name": "v1.0.0"}));
        let err = GitHubUpdateChecker::with_fetcher(DEFAULT_TIMEOUT, fetch)
            .check("dev")
            .unwrap_err();
        assert!(err.to_string().contains("releases page"));
    }

    #[test]
    fn classifies_ureq_errors() {
        assert_eq!(
            classify(&ureq::Error::StatusCode(404)),
            FetchError::Status(404)
        );
        assert!(matches!(
            classify(&ureq::Error::HostNotFound),
            FetchError::Network(_)
        ));
        let cert = rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer);
        assert!(matches!(
            classify(&ureq::Error::Rustls(cert.clone())),
            FetchError::Certificate(_)
        ));
        // Handshake failures surface as io::Error wrapping the rustls error.
        let io = std::io::Error::new(std::io::ErrorKind::InvalidData, cert);
        assert!(matches!(
            classify(&ureq::Error::Io(io)),
            FetchError::Certificate(_)
        ));
        let io = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        assert!(matches!(
            classify(&ureq::Error::Io(io)),
            FetchError::Network(_)
        ));
    }

    /// Hits the real GitHub API; run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "network"]
    fn live_check_reaches_github() {
        let info = GitHubUpdateChecker::new().check("0.0.1").unwrap();
        assert_ne!(info.status, UpdateStatus::NewerThanRelease);
    }
}
