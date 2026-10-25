//! Whether a command line asks for the CLI or the window. Shared by the app
//! and the Windows console launcher (`src/bin/framecover-console.rs`), which
//! includes this file, so it must not depend on anything else in the crate.

use std::ffi::OsString;

/// Command names that select the CLI (plus `help`, provided by clap).
pub const COMMANDS: &[&str] = &["set", "info", "convert", "extract", "check-update", "help"];

/// Whether `args` (including the program name) ask for the CLI rather than
/// the window.
pub fn is_cli_invocation(args: &[OsString]) -> bool {
    let Some(first) = args.get(1).and_then(|a| a.to_str()) else {
        return false;
    };
    // macOS used to pass `-psn_<serial>` to apps launched from Finder.
    COMMANDS.contains(&first) || (first.starts_with('-') && !first.starts_with("-psn_"))
}
