//! Locating and running the bundled `ffmpeg` / `ffprobe` executables.
//!
//! With the `libav` feature (Android) the same [`Invocation`]s run inside
//! this process through `framecover-libav` instead; see [`run`].
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

/// One ffmpeg / ffprobe invocation. [`run`] runs it to completion: as a
/// child process, or inside this process with the `libav` feature (Android,
/// where no executables can be started). [`Invocation::into_command`] gives
/// the child-process [`Command`] for callers that stream its output.
#[derive(Debug, Clone)]
pub struct Invocation {
    tool: Tool,
    args: Vec<std::ffi::OsString>,
}

impl Invocation {
    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    /// Run to completion and capture the output, whatever the exit status
    /// (like [`Command::output`]; [`run`] also checks the status).
    pub fn output(&mut self) -> Result<Output> {
        #[cfg(not(feature = "libav"))]
        return self.process_output();
        #[cfg(feature = "libav")]
        return in_process::run(self);
    }

    /// [`Invocation::output`] from a child process, whatever the backend.
    fn process_output(&self) -> Result<Output> {
        self.clone()
            .into_command()?
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Error::FfmpegMissing(e.to_string()),
                _ => Error::Io(e),
            })
    }

    /// A child process running this invocation that never opens a console window.
    pub fn into_command(self) -> Result<Command> {
        let mut cmd = no_window(Command::new(locate(self.tool)?));
        cmd.args(self.args);
        Ok(cmd)
    }
}

/// Start an invocation of `tool`. Fails early with [`Error::FfmpegMissing`]
/// if the executable cannot be found (never with the `libav` feature).
pub fn command(tool: Tool) -> Result<Invocation> {
    #[cfg(not(feature = "libav"))]
    locate(tool)?;
    Ok(Invocation {
        tool,
        args: Vec::new(),
    })
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

/// Run `invocation`, capturing output; non-zero exit becomes [`Error::Ffmpeg`].
pub fn run(invocation: &mut Invocation) -> Result<Output> {
    check_status(invocation.output()?)
}

fn check_status(output: Output) -> Result<Output> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(Error::Ffmpeg {
            code: output.status.code().unwrap_or(-1),
            stderr: tail(&String::from_utf8_lossy(&output.stderr), 4000).to_owned(),
        })
    }
}

/// Convenience: run the ffmpeg *executable* with `-hide_banner -nostdin` and
/// `args`. Even with the `libav` feature this starts the located binary: it
/// is for test fixtures and tooling, which need encoders and lavfi sources
/// the in-process build leaves out. Library code uses [`command`] and [`run`].
pub fn ffmpeg<I, S>(args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Invocation {
        tool: Tool::Ffmpeg,
        args: Vec::new(),
    };
    cmd.args(["-hide_banner", "-nostdin"]).args(args);
    check_status(cmd.process_output()?)
}

/// The `libav` backend: the same invocations, run by the ffmpeg / ffprobe
/// front ends linked into this process (`framecover-libav`).
#[cfg(feature = "libav")]
mod in_process {
    use std::ffi::OsString;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{ExitStatus, Output};

    use framecover_libav as libav;

    use super::{Invocation, Tool};
    use crate::error::Result;

    pub(super) fn run(invocation: &Invocation) -> Result<Output> {
        let args = &invocation.args;
        // `-encoders` / `-decoders` print plain listings to stdout; build
        // the same lines from the codec registry.
        if let Some(listing) = args.iter().find(|a| *a == "-encoders" || *a == "-decoders") {
            return Ok(Output {
                status: ExitStatus::from_raw(0),
                stdout: codec_listing(listing == "-encoders").into_bytes(),
                stderr: Vec::new(),
            });
        }
        // Standard output becomes a temporary file: ffprobe writes its report
        // with `-o`, and ffmpeg's `-` output (always the last argument) is
        // replaced by the file's path.
        let dir = tempfile::tempdir()?;
        let stdout_path = dir.path().join("stdout");
        let mut args: Vec<OsString> = args.clone();
        match invocation.tool {
            Tool::Ffprobe => {
                args.splice(
                    0..0,
                    [OsString::from("-o"), stdout_path.clone().into_os_string()],
                );
            }
            Tool::Ffmpeg => {
                if let Some(last) = args.last_mut().filter(|a| *a == "-") {
                    *last = stdout_path.clone().into_os_string();
                }
            }
        }
        let tool = match invocation.tool {
            Tool::Ffmpeg => libav::Tool::Ffmpeg,
            Tool::Ffprobe => libav::Tool::Ffprobe,
        };
        let out = libav::run_tool(tool, &args);
        Ok(Output {
            status: ExitStatus::from_raw(out.code << 8),
            stdout: std::fs::read(&stdout_path).unwrap_or_default(),
            stderr: out.stderr.into_bytes(),
        })
    }

    /// Lines shaped like `ffmpeg -encoders` output (` V..... name  Long name`).
    fn codec_listing(encoders: bool) -> String {
        let mut text = String::from(if encoders {
            "Encoders:\n"
        } else {
            "Decoders:\n"
        });
        text.push_str(" ------\n");
        for codec in libav::codecs(encoders) {
            text.push_str(&format!(
                " {}..... {:<20} {}\n",
                codec.kind, codec.name, codec.long_name
            ));
        }
        text
    }
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
