//! Locating and running the bundled `ffmpeg` / `ffprobe` executables.
//!
//! Lookup order for each tool:
//! 1. `FRAMECOVER_FFMPEG` / `FRAMECOVER_FFPROBE` environment variables
//!    (`IMAGEIO_FFMPEG_EXE` is honoured for ffmpeg for compatibility);
//! 2. a path registered with [`set_bundled_dir`] (the app extracts embedded
//!    binaries there on Windows);
//! 3. next to the running executable, then `../Resources` (macOS `.app`);
//! 4. `vendor/ffmpeg/<platform>` (from `cargo xtask fetch-ffmpeg`), then
//!    `vendor/ffmpeg`, in the source tree (development builds);
//! 5. `PATH`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

use crate::error::{Error, Result};

/// Which executable to locate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Ffmpeg,
    Ffprobe,
}

impl Tool {
    fn stem(self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::Ffprobe => "ffprobe",
        }
    }

    fn env_vars(self) -> &'static [&'static str] {
        match self {
            Self::Ffmpeg => &["FRAMECOVER_FFMPEG", "IMAGEIO_FFMPEG_EXE"],
            Self::Ffprobe => &["FRAMECOVER_FFPROBE"],
        }
    }

    fn file_name(self) -> String {
        format!("{}{}", self.stem(), std::env::consts::EXE_SUFFIX)
    }
}

type DirProvider = Box<dyn Fn() -> Option<PathBuf> + Send + Sync>;

static BUNDLED_DIR: OnceLock<DirProvider> = OnceLock::new();

/// Register a directory holding bundled binaries (called once at startup).
pub fn set_bundled_dir(dir: PathBuf) {
    set_bundled_dir_provider(move || Some(dir.clone()));
}

/// Register a function returning the bundled-binaries directory. It may
/// block, e.g. until the app has finished unpacking embedded binaries, so
/// every lookup waits for them instead of falling through to `PATH`.
pub fn set_bundled_dir_provider(provider: impl Fn() -> Option<PathBuf> + Send + Sync + 'static) {
    let _ = BUNDLED_DIR.set(Box::new(provider));
}

/// Locate `tool`, returning an error if no candidate exists.
pub fn locate(tool: Tool) -> Result<PathBuf> {
    for var in tool.env_vars() {
        if let Some(value) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            let path = PathBuf::from(&value);
            if path.components().count() > 1 || path.is_file() {
                return Ok(path);
            }
            // A bare command name such as `ffmpeg`: resolve through PATH.
            if let Some(found) = search_path(&value) {
                return Ok(found);
            }
        }
    }

    let name = tool.file_name();
    let mut candidates = Vec::new();
    if let Some(dir) = BUNDLED_DIR.get().and_then(|provider| provider()) {
        candidates.push(dir.join(&name));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(&name));
        candidates.push(dir.join("ffmpeg").join(&name));
        candidates.push(dir.join("../Resources").join(&name));
    }
    let vendor = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/ffmpeg");
    if let Some(platform) = vendor_platform() {
        candidates.push(vendor.join(platform).join(&name));
    }
    candidates.push(vendor.join(&name));
    if let Some(found) = candidates.into_iter().find(|p| p.is_file()) {
        return Ok(found);
    }
    search_path(OsStr::new(tool.stem())).ok_or_else(|| {
        Error::FfmpegMissing(format!(
            "could not find {name}; set {} to its location",
            tool.env_vars()[0]
        ))
    })
}

/// `vendor/ffmpeg/<platform>` directory name used by `cargo xtask fetch-ffmpeg`.
fn vendor_platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("windows-x64"),
        ("linux", "x86_64") => Some("linux-x64"),
        ("linux", "aarch64") => Some("linux-arm64"),
        ("macos", "aarch64") => Some("macos-arm64"),
        ("macos", "x86_64") => Some("macos-x64"),
        _ => None,
    }
}

fn search_path(name: &OsStr) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let mut file = PathBuf::from(name);
    if file.extension().is_none() && !std::env::consts::EXE_SUFFIX.is_empty() {
        file.set_extension(&std::env::consts::EXE_SUFFIX[1..]);
    }
    std::env::split_paths(&path)
        .map(|dir| dir.join(&file))
        .find(|p| p.is_file())
}

/// Build a [`Command`] for `tool` that never opens a console window.
pub fn command(tool: Tool) -> Result<Command> {
    Ok(no_window(Command::new(locate(tool)?)))
}

/// Apply `CREATE_NO_WINDOW` on Windows; a no-op elsewhere.
pub fn no_window(mut cmd: Command) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null());
    cmd
}

/// Run `cmd`, capturing output; non-zero exit becomes [`Error::Ffmpeg`].
pub fn run(cmd: &mut Command) -> Result<Output> {
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::FfmpegMissing(e.to_string()),
            _ => Error::Io(e),
        })?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(Error::Ffmpeg {
            code: output.status.code().unwrap_or(-1),
            stderr: tail(&String::from_utf8_lossy(&output.stderr), 4000).to_owned(),
        })
    }
}

/// Convenience: run ffmpeg with `-hide_banner -nostdin` and `args`.
pub fn ffmpeg<I, S>(args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = command(Tool::Ffmpeg)?;
    cmd.args(["-hide_banner", "-nostdin"]).args(args);
    run(&mut cmd)
}

fn tail(s: &str, max: usize) -> &str {
    let s = s.trim();
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// Names of the encoders the located ffmpeg supports (`ffmpeg -encoders`).
pub fn available_encoders() -> Result<Vec<String>> {
    let out = ffmpeg(["-encoders"])?;
    Ok(parse_encoders(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_encoders(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let flags = fields.next()?;
            let name = fields.next()?;
            (flags.len() == 6 && flags != "------").then(|| name.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_encoder_list() {
        let text = "Encoders:\n V..... = Video\n ------\n V....D libx264  libx264 H.264\n A....D aac   AAC\n";
        assert_eq!(parse_encoders(text), vec!["=", "libx264", "aac"]);
    }

    #[test]
    fn tail_respects_char_boundaries() {
        assert_eq!(tail("абв", 2), "в");
        assert_eq!(tail("  x  ", 10), "x");
    }
}
