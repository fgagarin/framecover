//! Windows Explorer integration via the Shell API.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use image::RgbImage;
use windows_sys::Win32::UI::Shell::{
    SHCNE_UPDATEITEM, SHCNF_PATHW, SHChangeNotify, SetCurrentProcessExplicitAppUserModelID,
    ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use super::CacheInvalidator;

/// Notifies Explorer (`SHChangeNotify`) so it drops the cached thumbnail.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsCacheInvalidator;

impl CacheInvalidator for WindowsCacheInvalidator {
    fn invalidate(&self, path: &Path, _thumbnail: Option<&RgbImage>) {
        let abs = match std::path::absolute(path) {
            Ok(abs) => abs,
            Err(e) => {
                log::warn!("SHChangeNotify skipped for '{}': {e}", path.display());
                return;
            }
        };
        let wide = to_wide(abs.as_os_str());
        // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the
        // call; SHCNF_PATHW tells the shell to read it as such. The second
        // item is unused for SHCNE_UPDATEITEM and may be null.
        unsafe {
            SHChangeNotify(
                SHCNE_UPDATEITEM as i32,
                SHCNF_PATHW,
                wide.as_ptr().cast(),
                std::ptr::null(),
            );
        }
    }
}

fn to_wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// `SetCurrentProcessExplicitAppUserModelID`; failures are logged.
pub fn set_app_user_model_id(id: &str) {
    let wide = to_wide(OsStr::new(id));
    // SAFETY: `wide` is a valid NUL-terminated UTF-16 string for the call.
    let hr = unsafe { SetCurrentProcessExplicitAppUserModelID(wide.as_ptr()) };
    if hr < 0 {
        log::warn!("SetCurrentProcessExplicitAppUserModelID failed: HRESULT {hr:#010x}");
    }
}

/// Open `target` with its default handler (`ShellExecuteW(..., "open", ...)`).
pub fn shell_open(target: &str) -> io::Result<()> {
    let verb = to_wide(OsStr::new("open"));
    let file = to_wide(OsStr::new(target));
    // SAFETY: all pointers are valid NUL-terminated UTF-16 strings or null.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values <= 32 are error codes (documented ShellExecute contract).
    if result as usize > 32 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "ShellExecuteW failed with code {}",
            result as usize
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_failure_does_not_raise() {
        let tmp = tempfile::tempdir().unwrap();
        let video = tmp.path().join("test.mp4");
        std::fs::write(&video, b"fake").unwrap();
        WindowsCacheInvalidator.invalidate(&video, None);
        WindowsCacheInvalidator.invalidate(&tmp.path().join("missing.mp4"), None);
        WindowsCacheInvalidator.invalidate(Path::new(""), None);
    }

    #[test]
    fn uses_documented_shell_constants() {
        // The Python port passed 0x1 (SHCNE_RENAMEITEM) by mistake.
        assert_eq!(SHCNE_UPDATEITEM, 0x2000);
        assert_eq!(SHCNF_PATHW, 0x0005);
    }

    #[test]
    fn wide_strings_are_nul_terminated() {
        assert_eq!(to_wide(OsStr::new("ab")), vec![97, 98, 0]);
    }
}
