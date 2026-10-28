//! FrameCover desktop application (Slint GUI) and command-line interface.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_os = "android"))]
fn main() {
    // `framecover <command> ...` runs the CLI; no arguments or file paths
    // open the window.
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if framecover::cli::is_cli_invocation(&args) {
        std::process::exit(framecover::cli::main(args));
    }
    framecover::ui::run();
}

/// The Android app is the library's `android_main`; this binary is unused.
#[cfg(target_os = "android")]
fn main() {}
