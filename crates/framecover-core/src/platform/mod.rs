//! OS integration: file-manager thumbnail cache invalidation (port of
//! `platform/`), "show in folder", opening URLs and the Windows taskbar
//! identity.
//!
//! The XDG (`linux.rs`) and macOS (`macos.rs`) invalidators only shell out
//! and write files, so they compile — and are unit-tested — on every OS;
//! [`get_cache_invalidator`] picks the one matching the running platform.

use std::io;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use image::RgbImage;

pub mod linux;
pub mod macos;
#[cfg(windows)]
pub mod windows;

/// Notify the host OS file manager to refresh the thumbnail of a file.
///
/// Implementations never fail: problems are logged as warnings only.
pub trait CacheInvalidator: Send + Sync {
    /// Trigger a thumbnail refresh for `path`.
    ///
    /// `thumbnail` is the newly applied frame; the Linux implementation
    /// stores it in the freedesktop thumbnail cache.
    fn invalidate(&self, path: &Path, thumbnail: Option<&RgbImage>);
}

/// The invalidator type used on the current platform.
#[cfg(windows)]
pub type NativeCacheInvalidator = windows::WindowsCacheInvalidator;
/// The invalidator type used on the current platform.
#[cfg(target_os = "macos")]
pub type NativeCacheInvalidator = macos::MacOSCacheInvalidator;
/// The invalidator type used on the current platform (freedesktop systems).
#[cfg(all(unix, not(target_os = "macos")))]
pub type NativeCacheInvalidator = linux::LinuxCacheInvalidator;
/// No file-manager integration is known for this platform.
#[cfg(not(any(unix, windows)))]
pub type NativeCacheInvalidator = NoopCacheInvalidator;

/// Return the [`CacheInvalidator`] for the current platform.
pub fn get_cache_invalidator() -> Box<dyn CacheInvalidator> {
    Box::new(NativeCacheInvalidator::default())
}

/// Does nothing; used on platforms without a known file manager protocol.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopCacheInvalidator;

impl CacheInvalidator for NoopCacheInvalidator {
    fn invalidate(&self, _path: &Path, _thumbnail: Option<&RgbImage>) {}
}

// ---------------------------------------------------------------------------
// Show in folder / open URL / app identity
// ---------------------------------------------------------------------------

/// Open the system file manager with `path` selected ("Show in folder").
///
/// On Linux this may block for up to ~3 s while the D-Bus call is tried, so
/// call it off the UI thread.
pub fn reveal_in_file_manager(path: &Path) -> io::Result<()> {
    let path = std::path::absolute(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // explorer parses its own command line: `/select,"C:\a b\c.mp4"` must be
        // passed verbatim (std's quoting would wrap the whole argument).
        let mut cmd = Command::new("explorer");
        cmd.raw_arg(format!("/select,\"{}\"", path.display()));
        // explorer.exe exits with 1 even on success, so don't check the status.
        spawn_detached(cmd)
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        cmd.arg("-R").arg(&path);
        spawn_detached(cmd)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let uri = linux::file_uri(&path);
        let mut dbus = Command::new("dbus-send");
        dbus.args(linux::show_items_args(&uri));
        match run_with_timeout(dbus, Duration::from_secs(3)) {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => log::info!("FileManager1.ShowItems failed ({status}); using xdg-open"),
            Err(e) => log::info!("dbus-send unavailable ({e}); using xdg-open"),
        }
        let dir = if path.is_dir() {
            path.as_path()
        } else {
            path.parent().unwrap_or(Path::new("/"))
        };
        let mut cmd = Command::new("xdg-open");
        cmd.arg(dir);
        spawn_detached(cmd)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no file manager",
        ))
    }
}

/// Open an `http(s)` URL in the default browser.
///
/// Other schemes are rejected so data from the network can never launch
/// local programs or files.
pub fn open_url(url: &str) -> io::Result<()> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://"))
        || url.chars().any(char::is_control)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to open non-web URL {url:?}"),
        ));
    }
    #[cfg(windows)]
    {
        windows::shell_open(url)
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        cmd.arg(url);
        spawn_detached(cmd)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(url);
        spawn_detached(cmd)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no browser"))
    }
}

/// Windows AppUserModelID, so the taskbar groups windows under FrameCover's
/// own icon instead of the host process.
pub const APP_USER_MODEL_ID: &str = "FrameCover.Application";

/// Set the process AppUserModelID (Windows only; a no-op elsewhere).
/// Call once at startup before any window is created.
pub fn set_app_user_model_id() {
    #[cfg(windows)]
    windows::set_app_user_model_id(APP_USER_MODEL_ID);
}

// ---------------------------------------------------------------------------
// Process helpers
// ---------------------------------------------------------------------------

/// Start `cmd` without waiting for it; output is discarded. A background
/// thread reaps the child so no zombie is left behind on Unix.
fn spawn_detached(mut cmd: Command) -> io::Result<()> {
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = crate::ffmpeg::no_window(cmd).spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Run `cmd` to completion with output discarded, killing it after `timeout`.
pub(crate) fn run_with_timeout(mut cmd: Command, timeout: Duration) -> io::Result<ExitStatus> {
    let program = cmd.get_program().to_owned();
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = crate::ffmpeg::no_window(cmd).spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{program:?} timed out after {timeout:?}"),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_native_invalidator() {
        // Mirrors test_platform_init: the factory yields this OS's implementation.
        fn assert_native(_: NativeCacheInvalidator) {}
        assert_native(NativeCacheInvalidator::default());
        let invalidator = get_cache_invalidator();
        // Never panics, even for a file that does not exist.
        let dir = tempfile::tempdir().unwrap();
        invalidator.invalidate(&dir.path().join("missing.mp4"), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_uses_shell_notify_invalidator() {
        let _: windows::WindowsCacheInvalidator = NativeCacheInvalidator::default();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_uses_quicklook_invalidator() {
        let _: macos::MacOSCacheInvalidator = NativeCacheInvalidator::default();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_uses_xdg_invalidator() {
        let _: linux::LinuxCacheInvalidator = NativeCacheInvalidator::default();
    }

    #[test]
    fn open_url_rejects_non_web_schemes() {
        for url in [
            "file:///etc/passwd",
            "C:\\Windows\\notepad.exe",
            "javascript:alert(1)",
            "https://x\n--y",
            "",
        ] {
            let err = open_url(url).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{url}");
        }
    }

    #[test]
    fn run_with_timeout_reports_missing_program() {
        let cmd = Command::new("framecover-definitely-missing-program");
        assert!(run_with_timeout(cmd, Duration::from_secs(1)).is_err());
    }

    #[test]
    fn set_app_user_model_id_does_not_panic() {
        set_app_user_model_id();
    }
}
