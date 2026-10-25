//! Console launcher for the FrameCover CLI on Windows (shipped as
//! `framecover.com` next to `framecover.exe`).
//!
//! `framecover.exe` is a GUI-subsystem program, so an interactive cmd.exe or
//! PowerShell prompt doesn't wait for it and its output lands after the next
//! prompt. This console-subsystem launcher is found first when the user types
//! `framecover` (`.COM` precedes `.EXE` in `PATHEXT`). For CLI commands it
//! runs the app with the console's standard handles, waits, and returns its
//! exit code; Ctrl+C stops the app too. Without a command (open the window)
//! it starts the app and returns at once.

#[path = "../cli_args.rs"]
mod cli_args;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Exit code when the app executable can't be found or started.
const EXIT_LAUNCH_FAILED: u8 = 2;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    let Some(app) = std::env::current_exe().ok().and_then(|me| find_app(&me)) else {
        eprintln!(
            "framecover: could not find framecover.exe next to this launcher; \
             keep framecover.com and framecover.exe in the same folder."
        );
        return ExitCode::from(EXIT_LAUNCH_FAILED);
    };
    let mut command = Command::new(&app);
    command.args(&args[1..]);

    if !cli_args::is_cli_invocation(&args) {
        // Opening the window: don't hold the console.
        return match command.spawn() {
            Ok(_) => ExitCode::SUCCESS,
            Err(err) => launch_failed(&app, &err),
        };
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return launch_failed(&app, &err),
    };
    ctrl_c::stop_child_on_interrupt(&child);
    match child.wait() {
        Ok(status) => status
            .code()
            .and_then(|c| u8::try_from(c).ok())
            .map_or(ExitCode::FAILURE, ExitCode::from),
        Err(err) => launch_failed(&app, &err),
    }
}

fn launch_failed(app: &Path, err: &std::io::Error) -> ExitCode {
    eprintln!("framecover: could not start {}: {err}", app.display());
    ExitCode::from(EXIT_LAUNCH_FAILED)
}

/// The app next to `launcher`: `framecover.exe`, or else a renamed download
/// such as `framecover-0.3.0-windows.exe` (the newest-named one wins).
fn find_app(launcher: &Path) -> Option<PathBuf> {
    let dir = launcher.parent()?;
    let exe = |stem: &str| format!("{stem}{}", std::env::consts::EXE_SUFFIX);
    let preferred = dir.join(exe("framecover"));
    if preferred.is_file() && preferred != launcher {
        return Some(preferred);
    }
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.as_path() != launcher && path.is_file())
        .filter(|path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let lower = name.to_ascii_lowercase();
            lower.starts_with("framecover")
                && lower.ends_with(&exe("").to_ascii_lowercase())
                && !lower.starts_with("framecover-console")
        })
        .collect();
    candidates.sort();
    candidates.pop()
}

#[cfg(windows)]
mod ctrl_c {
    use std::os::windows::io::AsRawHandle;
    use std::sync::atomic::{AtomicIsize, Ordering};

    use windows_sys::Win32::Foundation::{FALSE, HANDLE, TRUE};
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    use windows_sys::Win32::System::Threading::TerminateProcess;

    /// The running app's process handle (0 when none).
    static CHILD: AtomicIsize = AtomicIsize::new(0);

    /// The app is a GUI-subsystem process that isn't attached to this
    /// console, so it doesn't receive Ctrl+C itself; end it here.
    pub fn stop_child_on_interrupt(child: &std::process::Child) {
        CHILD.store(child.as_raw_handle() as isize, Ordering::SeqCst);
        // SAFETY: registers a plain function as the console control handler.
        unsafe {
            SetConsoleCtrlHandler(Some(handler), TRUE);
        }
    }

    unsafe extern "system" fn handler(_ctrl_type: u32) -> i32 {
        let handle = CHILD.load(Ordering::SeqCst);
        if handle != 0 {
            // SAFETY: the handle belongs to the `Child`, which `main` keeps
            // alive while waiting; terminating an exited process is harmless.
            unsafe {
                TerminateProcess(handle as HANDLE, 130);
            }
        }
        // Let the default handler end the launcher too.
        FALSE
    }
}

#[cfg(not(windows))]
mod ctrl_c {
    /// Elsewhere the app shares the terminal's process group and gets the
    /// signal itself.
    pub fn stop_child_on_interrupt(_child: &std::process::Child) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"").unwrap();
        path
    }

    #[test]
    fn prefers_framecover_exe_next_to_the_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = touch(dir.path(), "framecover.com");
        let exe = format!("framecover{}", std::env::consts::EXE_SUFFIX);
        let app = touch(dir.path(), &exe);
        touch(
            dir.path(),
            &format!("framecover-0.2.0-windows{}", std::env::consts::EXE_SUFFIX),
        );
        assert_eq!(find_app(&launcher), Some(app));
    }

    #[test]
    fn falls_back_to_a_renamed_download() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = touch(dir.path(), "framecover.com");
        let suffix = std::env::consts::EXE_SUFFIX;
        touch(dir.path(), &format!("framecover-0.2.0-windows{suffix}"));
        let newest = touch(dir.path(), &format!("framecover-0.3.0-windows{suffix}"));
        touch(dir.path(), &format!("framecover-console{suffix}"));
        touch(dir.path(), "other.exe");
        if suffix.is_empty() {
            // Every regular file "ends with" the empty suffix off Windows.
            return;
        }
        assert_eq!(find_app(&launcher), Some(newest));
    }

    #[test]
    fn missing_app_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = touch(dir.path(), "framecover.com");
        assert_eq!(find_app(&launcher), None);
    }
}
