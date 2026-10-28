//! FrameCover application: the Slint GUI, the command-line interface and,
//! on Android, the `android_main` entry point (see `android.rs`).

#[cfg(target_os = "android")]
mod android;
#[cfg(not(target_os = "android"))]
pub mod cli;
#[cfg(not(target_os = "android"))]
mod cli_args;
mod controller;
#[cfg(not(target_os = "android"))]
mod embedded;
mod error_report;
mod filmstrip;
mod frame_worker;
mod services;
mod settings;
mod timeline;
pub mod ui;
#[cfg(not(target_os = "android"))]
mod window_fit;
mod workers;

slint::include_modules!();
