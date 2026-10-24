//! FrameCover desktop application (Slint GUI).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod controller;
mod error_report;
mod filmstrip;
mod services;
mod settings;
mod timeline;
mod ui;
mod workers;

slint::include_modules!();

fn main() {
    ui::run();
}
