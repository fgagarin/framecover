//! Project automation (`cargo xtask <command>`), replacing `scripts/release.py`
//! and the Makefile targets of the Python version.

mod android;
mod ffmpeg;
mod package;
mod release;
mod util;

use std::path::PathBuf;
use std::process::ExitCode;

use util::{Result, bail};

const USAGE: &str = "\
Usage: cargo xtask <command> [options]

Commands:
  fetch-ffmpeg [PLATFORM...|all] [--force]
        Download the pinned FFmpeg builds (xtask/ffmpeg-pins.toml) into
        vendor/ffmpeg/<platform>/, verifying SHA-256. Defaults to the
        host platform(s); macOS fetches both arm64 and x64.
        Platforms: windows-x64 linux-x64 linux-arm64 macos-arm64 macos-x64
  changelog
        Regenerate CHANGELOG.md with git-cliff (cliff.toml).
  release create [--version X.Y.Z]
        Bump the workspace version, regenerate CHANGELOG.md, commit
        `chore(release): vX.Y.Z` and create an annotated tag. Never pushes.
        Without --version the bump is derived from Conventional Commits.
  release notes <TAG> [--output DIR]
        Validate that TAG matches the Cargo version and points at HEAD, then
        write DIR/CHANGELOG.md and DIR/release-notes.md (default
        release-assets/) and print the notes.
  package <windows|linux|macos> [--output DIR] [--skip-build] [--binary PATH]
        Build the release binary and produce
        DIR/framecover-<version>-<platform>.<exe|tar.gz> (default DIR:
        release-assets/). Requires fetch-ffmpeg for the platform first.
  android [--abi arm64-v8a|x86_64]... [--debug] [--output DIR]
        Build the Android app library (cargo ndk) and APK (Gradle) as
        DIR/framecover-<version>-android.apk (default DIR: release-assets/).
        Needs the NDK, SDK, Gradle and scripts/android/build-ffmpeg.sh
        output; the image from scripts/android/Dockerfile has them all.

Environment:
  GIT_CLIFF   git-cliff executable (default: git-cliff on PATH)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(args: &[String]) -> Result<()> {
    let mut args = Args::new(args);
    match args.next_positional().as_deref() {
        Some("fetch-ffmpeg") => {
            let force = args.flag("--force");
            let platforms = args.rest_positional()?;
            ffmpeg::fetch(&platforms, force)
        }
        Some("changelog") => {
            args.finish()?;
            release::changelog()
        }
        Some("release") => match args.next_positional().as_deref() {
            Some("create") => {
                let version = args.value("--version")?;
                args.finish()?;
                release::create(version.as_deref())
            }
            Some("notes") => {
                let output = args.value("--output")?.map(PathBuf::from);
                let Some(tag) = args.next_positional() else {
                    bail!("release notes: missing <TAG>\n\n{USAGE}");
                };
                args.finish()?;
                release::notes(&tag, output)
            }
            other => bail!("unknown release subcommand {other:?}\n\n{USAGE}"),
        },
        Some("package") => {
            let output = args.value("--output")?.map(PathBuf::from);
            let binary = args.value("--binary")?.map(PathBuf::from);
            let skip_build = args.flag("--skip-build");
            let Some(platform) = args.next_positional() else {
                bail!("package: missing <PLATFORM>\n\n{USAGE}");
            };
            args.finish()?;
            package::package(&platform, output, skip_build, binary)
        }
        Some("android") => {
            let output = args.value("--output")?.map(PathBuf::from);
            let debug = args.flag("--debug");
            let mut abis = Vec::new();
            while let Some(abi) = args.value("--abi")? {
                abis.push(abi);
            }
            args.finish()?;
            android::build(&abis, debug, output)
        }
        Some("help" | "--help" | "-h") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => bail!("unknown command {other:?}\n\n{USAGE}"),
    }
}

/// Minimal argument parser: flags and `--key value` options may appear
/// anywhere; the remaining words are positionals in order.
struct Args {
    items: Vec<Option<String>>,
}

impl Args {
    fn new(args: &[String]) -> Self {
        Self {
            items: args.iter().cloned().map(Some).collect(),
        }
    }

    fn flag(&mut self, name: &str) -> bool {
        let mut found = false;
        for item in &mut self.items {
            if item.as_deref() == Some(name) {
                *item = None;
                found = true;
            }
        }
        found
    }

    fn value(&mut self, name: &str) -> Result<Option<String>> {
        let prefix = format!("{name}=");
        for i in 0..self.items.len() {
            let Some(item) = self.items[i].clone() else {
                continue;
            };
            if let Some(v) = item.strip_prefix(&prefix) {
                self.items[i] = None;
                return Ok(Some(v.to_owned()));
            }
            if item == name {
                self.items[i] = None;
                match self.items.get_mut(i + 1).and_then(Option::take) {
                    Some(v) => return Ok(Some(v)),
                    None => bail!("{name} needs a value"),
                }
            }
        }
        Ok(None)
    }

    fn next_positional(&mut self) -> Option<String> {
        self.items
            .iter_mut()
            .find(|i| i.as_deref().is_some_and(|s| !s.starts_with("--")))
            .and_then(Option::take)
    }

    fn rest_positional(&mut self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        while let Some(p) = self.next_positional() {
            out.push(p);
        }
        self.finish()?;
        Ok(out)
    }

    fn finish(&self) -> Result<()> {
        let left: Vec<&str> = self.items.iter().flatten().map(String::as_str).collect();
        if left.is_empty() {
            Ok(())
        } else {
            bail!("unexpected arguments: {}\n\n{USAGE}", left.join(" "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Args {
        Args::new(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    fn parses_options_anywhere() {
        let mut a = args("package --output out linux --skip-build");
        assert_eq!(a.next_positional().as_deref(), Some("package"));
        assert_eq!(a.value("--output").unwrap().as_deref(), Some("out"));
        assert!(a.flag("--skip-build"));
        assert_eq!(a.next_positional().as_deref(), Some("linux"));
        a.finish().unwrap();
    }

    #[test]
    fn supports_equals_syntax_and_rejects_leftovers() {
        let mut a = args("release create --version=1.2.3 --bogus");
        a.next_positional();
        a.next_positional();
        assert_eq!(a.value("--version").unwrap().as_deref(), Some("1.2.3"));
        assert!(a.finish().is_err());
    }

    #[test]
    fn missing_value_is_an_error() {
        assert!(args("--version").value("--version").is_err());
    }
}
