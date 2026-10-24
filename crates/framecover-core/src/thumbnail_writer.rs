//! Thumbnail embedding (port of `core/thumbnail_writer.py`).
//!
//! * MP4 (`.mp4`): concat fast path — encode the selected frame as a one-frame
//!   clip matching the source codec parameters and prepend it with the concat
//!   demuxer and stream copy (no source re-encode). A previously inserted
//!   preview is stripped first so repeated saves keep the duration. The
//!   output's leading DTS are repaired so B-frame delay (or squashed DTS from
//!   earlier versions) cannot skew the frame rate. Falls back
//!   to a full H.264 re-encode that overlays the frame on frame 0.
//! * MOV: always the full re-encode path.
//! * MKV: cover attached with `-attach`.
//! * AVI: remuxed (no cover art support).
//! * WebM / FLV: no container change.
//!
//! All mutations go through [`atomic_replace`]; the original is never left
//! half-written.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Instant;

use image::RgbImage;
use log::{debug, info};

use crate::atomic_write::{self, atomic_replace};
use crate::error::{Error, Result};
use crate::imaging;
use crate::models::{ApplyError, ApplyResult, ProgressFn, Stages, VideoFile, VideoFormat};
use crate::mp4_tags;
use crate::probe::{self, ProbeInfo, Rational, Stream};

/// Embeds a thumbnail in place. Never fails: errors are in the [`ApplyResult`].
pub trait ThumbnailWriter: Send + Sync {
    fn write(
        &self,
        video: &VideoFile,
        thumbnail: &RgbImage,
        position_ms: Option<u64>,
        on_progress: Option<ProgressFn<'_>>,
    ) -> ApplyResult;

    /// Known reason saving this MP4/MOV will re-encode, or `None` for the fast path.
    fn reencode_reason(&self, video: &VideoFile) -> Option<String>;
}

/// Format-dispatching writer (MP4/MOV concat fast path or re-encode, MKV attach,
/// AVI remux, WebM/FLV no-op).
#[derive(Debug, Default, Clone)]
pub struct FormatDispatchThumbnailWriter;

// ---------------------------------------------------------------------------
// ffmpeg invocation + test seams
// ---------------------------------------------------------------------------

/// Test seams for the write side: simulated ffmpeg failures and recording of
/// the exact argv passed to ffmpeg (for golden comparisons). Thread-local, so
/// parallel tests do not interfere. Not part of the stable API.
#[doc(hidden)]
pub mod testing {
    use std::cell::{Cell, RefCell};
    use std::ffi::OsString;
    use std::process::Output;

    use crate::error::{Error, Result};
    use crate::ffmpeg::{self, Tool};

    /// One recorded ffmpeg invocation.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RecordedCommand {
        /// Arguments after the executable.
        pub args: Vec<String>,
        /// Exit code (0 on success; -1 when the process could not start).
        pub returncode: i32,
        /// Lines of the concat manifest for `-f concat` invocations.
        pub concat_manifest: Option<Vec<String>>,
    }

    thread_local! {
        static FAIL: Cell<bool> = const { Cell::new(false) };
        static RECORD: RefCell<Option<Vec<RecordedCommand>>> = const { RefCell::new(None) };
    }

    /// Run `f` with every ffmpeg invocation (writer and converter) on this
    /// thread failing with exit code 1 and no stderr — the Python tests patch
    /// `subprocess.run` to raise `CalledProcessError(1, "ffmpeg")`.
    pub fn with_simulated_ffmpeg_failure<R>(f: impl FnOnce() -> R) -> R {
        struct Reset(bool);
        impl Drop for Reset {
            fn drop(&mut self) {
                FAIL.with(|c| c.set(self.0));
            }
        }
        let _reset = Reset(FAIL.with(|c| c.replace(true)));
        f()
    }

    /// Run `f`, returning its result and every ffmpeg invocation it made.
    pub fn record_ffmpeg_commands<R>(f: impl FnOnce() -> R) -> (R, Vec<RecordedCommand>) {
        let previous = RECORD.with(|r| r.replace(Some(Vec::new())));
        let out = f();
        let recorded = RECORD.with(|r| r.replace(previous)).unwrap_or_default();
        (out, recorded)
    }

    fn concat_manifest(args: &[OsString]) -> Option<Vec<String>> {
        let is_concat = args.windows(2).any(|w| w[0] == "-f" && w[1] == "concat");
        if !is_concat {
            return None;
        }
        let input = args.windows(2).find(|w| w[0] == "-i")?[1].clone();
        let text = std::fs::read_to_string(input).ok()?;
        Some(text.lines().map(str::to_owned).collect())
    }

    /// Run the located ffmpeg with exactly `args` (no extra flags, no console).
    pub(crate) fn run_ffmpeg(args: Vec<OsString>) -> Result<Output> {
        let result = if FAIL.with(Cell::get) {
            Err(Error::Ffmpeg {
                code: 1,
                stderr: String::new(),
            })
        } else {
            ffmpeg::command(Tool::Ffmpeg).and_then(|mut cmd| {
                cmd.args(&args);
                ffmpeg::run(&mut cmd)
            })
        };
        RECORD.with(|r| {
            if let Some(log) = r.borrow_mut().as_mut() {
                log.push(RecordedCommand {
                    args: args
                        .iter()
                        .map(|a| a.to_string_lossy().into_owned())
                        .collect(),
                    returncode: match &result {
                        Ok(_) => 0,
                        Err(Error::Ffmpeg { code, .. }) => code.to_owned(),
                        Err(_) => -1,
                    },
                    concat_manifest: concat_manifest(&args),
                });
            }
        });
        result
    }
}

fn run_ffmpeg(args: Vec<OsString>) -> Result<()> {
    testing::run_ffmpeg(args).map(drop)
}

macro_rules! args {
    ($($x:expr),* $(,)?) => {
        vec![$(OsString::from(AsRef::<OsStr>::as_ref(&$x))),*]
    };
}

// ---------------------------------------------------------------------------
// Concat fast-path eligibility
// ---------------------------------------------------------------------------

/// `{"baseline", "constrained baseline", "main", "high"}` → encoder profile.
fn h264_profile(profile: &str) -> Option<&'static str> {
    match profile {
        "baseline" | "constrained baseline" => Some("baseline"),
        "main" => Some("main"),
        "high" => Some("high"),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConcatCodec {
    H264,
    Av1,
}

/// Parameters the one-frame preview clip must match.
#[derive(Debug, Clone, PartialEq)]
struct ConcatParams {
    codec: ConcatCodec,
    width: u32,
    height: u32,
    profile: &'static str,
    /// The stream's effective frame rate ([`Stream::effective_rate`]) as
    /// `num/den` (e.g. `25/1`) like `str()` of PyAV 19's `AVRational`, unlike
    /// `fractions.Fraction`/[`Rational`]'s Display.
    frame_rate: String,
    /// Seconds per frame, `1 / float(frame_rate)`.
    frame_duration: f64,
    video_track_timescale: i64,
}

impl ConcatParams {
    /// One frame in `video_track_timescale` ticks.
    fn frame_ticks(&self) -> i64 {
        (self.frame_duration * self.video_track_timescale as f64).round() as i64
    }
}

/// ffprobe reports unnamed profiles as `unknown` or a number; PyAV as `None`.
fn named_profile(stream: &Stream) -> Option<&str> {
    stream.profile.as_deref().filter(|p| {
        !p.is_empty()
            && *p != "unknown"
            && !p
                .trim_start_matches('-')
                .bytes()
                .all(|b| b.is_ascii_digit())
    })
}

fn named_pix_fmt(stream: &Stream) -> Option<&str> {
    stream
        .pix_fmt
        .as_deref()
        .filter(|p| !p.is_empty() && *p != "unknown" && *p != "none")
}

/// Python `repr()` of a `str`.
fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `repr()` of PyAV 19's `AVRational` (raw, unreduced like libav's value).
fn py_repr_rational(r: Option<Rational>) -> String {
    match r {
        Some(r) => format!("AVRational({}, {})", r.num, r.den),
        None => "None".to_owned(),
    }
}

/// Long name of the default *decoder* for `codec` — what PyAV's
/// `codec_context.codec.long_name` reports. It can differ from ffprobe's
/// codec-descriptor `codec_long_name` (HEVC: "HEVC (High Efficiency Video
/// Coding)" vs "H.265 / HEVC (High Efficiency Video Coding)").
fn decoder_long_name(codec: &str) -> Option<String> {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static DECODERS: OnceLock<HashMap<String, String>> = OnceLock::new();
    DECODERS
        .get_or_init(|| {
            let Ok(out) = crate::ffmpeg::ffmpeg(["-decoders"]) else {
                return HashMap::new();
            };
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|line| {
                    let line = line.trim_start();
                    let (flags, rest) = line.split_once(char::is_whitespace)?;
                    let (name, desc) = rest.trim_start().split_once(char::is_whitespace)?;
                    (flags.len() == 6 && flags != "------")
                        .then(|| (name.to_owned(), desc.trim().to_owned()))
                })
                .collect()
        })
        .get(codec)
        .cloned()
}

fn concat_decision(
    info: &ProbeInfo,
    path: &Path,
    replace_existing_preview: bool,
) -> (Option<ConcatParams>, Option<String>) {
    let playable: Vec<&Stream> = info.playable_video().collect();
    if playable.len() != 1 {
        debug!(
            "concat fast path skipped for '{}': found {} playable video stream(s), expected exactly 1",
            path.display(),
            playable.len()
        );
        return (
            None,
            Some(format!(
                "Expected one playable video stream; found {}.",
                playable.len()
            )),
        );
    }
    if info.has_attached_pic() && !replace_existing_preview {
        debug!(
            "concat fast path skipped for '{}': file already has an attached-picture video stream",
            path.display()
        );
        return (
            None,
            Some("An attached-picture video stream is already present.".to_owned()),
        );
    }

    let stream = playable[0];
    let codec_name = stream.codec_name.as_deref().unwrap_or("");
    let raw_profile = named_profile(stream);
    let profile = raw_profile.unwrap_or("").to_lowercase();
    let is_av1 = codec_name == "av1";
    let frame_rate = stream.effective_rate();
    let mut reasons: Vec<String> = Vec::new();
    if codec_name != "h264" && !is_av1 {
        let long_name = decoder_long_name(codec_name)
            .or_else(|| stream.codec_long_name.clone().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| codec_name.to_owned());
        reasons.push(format!(
            "video codec is {long_name} (fast copy supports H.264 and AV1 only)"
        ));
    }
    if (is_av1 && profile != "main") || (!is_av1 && h264_profile(&profile).is_none()) {
        reasons.push(format!(
            "profile is {}",
            py_repr_str(raw_profile.unwrap_or("(none)"))
        ));
    }
    let pix_fmt = named_pix_fmt(stream);
    if pix_fmt != Some("yuv420p") {
        reasons.push(format!(
            "pixel format is {}, not yuv420p",
            pix_fmt.map_or_else(|| "None".to_owned(), py_repr_str)
        ));
    }
    let (width, height) = (stream.width.unwrap_or(0), stream.height.unwrap_or(0));
    if width == 0 || height == 0 {
        reasons.push(format!("invalid dimensions {width}x{height}"));
    }
    let time_base = stream.time_base();
    if time_base.is_none_or(|tb| tb.num != 1) {
        reasons.push(format!(
            "unsupported video time base {}",
            py_repr_rational(time_base)
        ));
    }
    if frame_rate.is_none_or(|r| r.to_f64() <= 0.0) {
        reasons.push(format!(
            "invalid frame rate {}",
            py_repr_rational(frame_rate)
        ));
    }
    if !reasons.is_empty() {
        let joined = reasons.join("; ");
        debug!(
            "concat fast path skipped for '{}': {joined}",
            path.display()
        );
        return (None, Some(format!("{joined}.")));
    }

    let (Some(frame_rate), Some(time_base)) = (frame_rate, time_base) else {
        unreachable!("checked above");
    };
    (
        Some(ConcatParams {
            codec: if is_av1 {
                ConcatCodec::Av1
            } else {
                ConcatCodec::H264
            },
            width,
            height,
            profile: if is_av1 {
                "main"
            } else {
                h264_profile(&profile).unwrap_or("high")
            },
            frame_rate: format!("{}/{}", frame_rate.num, frame_rate.den),
            frame_duration: 1.0 / frame_rate.to_f64(),
            video_track_timescale: time_base.den,
        }),
        None,
    )
}

fn concat_decision_for(
    path: &Path,
    replace_existing_preview: bool,
) -> Result<(Option<ConcatParams>, Option<String>)> {
    let info = probe::probe(path)?;
    Ok(concat_decision(&info, path, replace_existing_preview))
}

/// Inspect an MP4 (without a previously inserted preview) for compatibility
/// with stream-copy preview insertion. Returns the reason conversion is
/// needed, or `None` if concatenation is supported. Errors if the file cannot
/// be probed at all.
pub fn concat_ineligibility_reason(path: &Path) -> Result<Option<String>> {
    Ok(concat_decision_for(path, false)?.1)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Python's `round()` on a true division (ties to even).
fn py_round_div(num: u64, den: u64) -> u64 {
    (num as f64 / den.max(1) as f64).round_ties_even() as u64
}

/// C `printf("%.{precision}g")`, as used by Python's `f"{x:.12g}"`.
fn format_g(x: f64, precision: usize) -> String {
    if x == 0.0 {
        return "0".to_owned();
    }
    if !x.is_finite() {
        return x.to_string();
    }
    let p = precision.max(1);
    let sci = format!("{:.*e}", p - 1, x);
    let (mantissa, exp) = sci.split_once('e').expect("exponent");
    let exp: i32 = exp.parse().expect("exponent value");
    let strip = |s: &str| -> String {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            s.to_owned()
        }
    };
    if exp < -4 || exp >= p as i32 {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip(mantissa), exp.abs())
    } else {
        let decimals = (p as i32 - 1 - exp).max(0) as usize;
        strip(&format!("{x:.decimals$}"))
    }
}

/// Quote a path for an FFmpeg concat demuxer manifest (`Path.as_posix()`).
///
/// Backslashes are literal inside ffmpeg's single quotes, so an apostrophe
/// closes the quote, is escaped, and reopens it (`'it'\''s'`). The Python
/// version's `\'` broke the fast path for such paths.
fn concat_quote(path: &Path) -> String {
    let text = path.to_string_lossy();
    let posix = if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    };
    format!("'{}'", posix.replace('\'', r"'\''"))
}

/// `tempfile.mkstemp(dir=dir, prefix=prefix, suffix=suffix)`, deleted on drop.
fn mkstemp(dir: &Path, prefix: &str, suffix: &str) -> Result<tempfile::TempPath> {
    Ok(tempfile::Builder::new()
        .prefix(prefix)
        .suffix(suffix)
        .rand_bytes(8)
        .tempfile_in(dir)?
        .into_temp_path())
}

fn video_dir(path: &Path) -> Result<PathBuf> {
    let abs = std::path::absolute(path)?;
    Ok(abs
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(".")))
}

/// Lower-cased extension including the dot, or `""`.
fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}

fn is_permission_error(err: &std::io::Error) -> bool {
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        return true;
    }
    // Python maps ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION to EACCES.
    cfg!(windows) && matches!(err.raw_os_error(), Some(32 | 33))
}

fn is_disk_full(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::StorageFull
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

impl ThumbnailWriter for FormatDispatchThumbnailWriter {
    fn write(
        &self,
        video: &VideoFile,
        thumbnail: &RgbImage,
        position_ms: Option<u64>,
        on_progress: Option<ProgressFn<'_>>,
    ) -> ApplyResult {
        let start = Instant::now();
        let shown = video.path.display();

        if !video.is_writable {
            return ApplyResult::failure(
                ApplyError::FileNotWritable,
                format!(
                    "'{shown}' is not writable. Copy the file to a writable location and try again."
                ),
            );
        }
        if video.format == VideoFormat::Unsupported {
            return ApplyResult::failure(
                ApplyError::UnsupportedFormat,
                format!(
                    "Format {} is not supported. Supported formats: MP4, MOV.",
                    video.format.name()
                ),
            );
        }

        let mut stages = Stages::new(on_progress);
        stages.start("Preparing thumbnail");

        let outcome = (|| -> Result<ApplyResult> {
            // Preserve the selected frame for zero-frame preview fallbacks.
            let frame_bytes = imaging::encode_png(thumbnail)?;
            let jpeg_bytes = imaging::cover_jpeg(thumbnail)?;

            match video.format {
                VideoFormat::Webm | VideoFormat::Flv => {
                    stages.finish();
                    Ok(ApplyResult {
                        success: true,
                        elapsed_ms: elapsed_ms(start),
                        error_message: Some(format!(
                            "{} does not support embedded cover art; \
                             Linux file manager icon updated via XDG cache",
                            video.format.name()
                        )),
                        ..ApplyResult::default()
                    })
                }
                VideoFormat::Mp4 | VideoFormat::Mov => {
                    let duration_shift_ms = embed_mp4_mov(
                        &video.path,
                        &jpeg_bytes,
                        &frame_bytes,
                        &mut stages,
                        Mp4Options {
                            position_ms,
                            frame_number: position_ms
                                .map(|p| py_round_div(p, video.frame_step_ms.max(1))),
                            frame_step_ms: video.frame_step_ms,
                            write_mp4_covr: video.format == VideoFormat::Mp4,
                            replace_existing_preview: video.has_inserted_preview(),
                        },
                    )?;
                    stages.finish();
                    Ok(ApplyResult {
                        success: true,
                        elapsed_ms: elapsed_ms(start),
                        duration_shift_ms,
                        ..ApplyResult::default()
                    })
                }
                VideoFormat::Mkv => {
                    embed_mkv(&video.path, &jpeg_bytes, &mut stages)?;
                    stages.finish();
                    Ok(ApplyResult {
                        success: true,
                        elapsed_ms: elapsed_ms(start),
                        ..ApplyResult::default()
                    })
                }
                VideoFormat::Avi => {
                    remux_avi(&video.path, &mut stages)?;
                    stages.finish();
                    Ok(ApplyResult {
                        success: true,
                        elapsed_ms: elapsed_ms(start),
                        error_message: Some(
                            "AVI does not support embedded cover art; \
                             file manager will generate its own preview"
                                .to_owned(),
                        ),
                        ..ApplyResult::default()
                    })
                }
                VideoFormat::Unsupported => unreachable!("rejected above"),
            }
        })();

        stages.finish();
        match outcome {
            Ok(result) => result,
            Err(err) => map_error(err, &video.path),
        }
    }

    fn reencode_reason(&self, video: &VideoFile) -> Option<String> {
        if !video.format.is_mp4_family() {
            return None;
        }
        if extension(&video.path) != ".mp4" {
            return Some("The fast copy path supports .mp4 files only.".to_owned());
        }
        match concat_decision_for(&video.path, video.has_inserted_preview()) {
            Ok((_, reason)) => reason,
            // Python lets the PyAV error propagate; the UI only shows text.
            Err(err) => Some(format!("Could not inspect the video streams: {err}")),
        }
    }
}

fn map_error(err: Error, path: &Path) -> ApplyResult {
    match err {
        Error::Io(io) if is_permission_error(&io) => ApplyResult::failure(
            ApplyError::FileNotWritable,
            format!("Permission denied writing '{}': {io}", path.display()),
        ),
        Error::Ffmpeg { code, stderr } => ApplyResult::failure(
            ApplyError::FfmpegError,
            format!(
                "ffmpeg failed (exit {code}). stderr: {}",
                if stderr.is_empty() { "(none)" } else { &stderr }
            ),
        ),
        Error::Io(io) if is_disk_full(&io) => ApplyResult::failure(
            ApplyError::DiskFull,
            "Not enough disk space to write the updated file. Free space and retry.",
        ),
        Error::Io(io) => {
            ApplyResult::failure(ApplyError::Unexpected, format!("Unexpected OS error: {io}"))
        }
        other => ApplyResult::failure(ApplyError::Unexpected, format!("Unexpected error: {other}")),
    }
}

struct Mp4Options {
    position_ms: Option<u64>,
    frame_number: Option<u64>,
    frame_step_ms: u64,
    write_mp4_covr: bool,
    replace_existing_preview: bool,
}

/// Temp files used by the MP4/MOV path (all `.vt_*` in the video's dir).
struct Mp4Temps {
    cover: tempfile::TempPath,
    frame: tempfile::TempPath,
    preview: tempfile::TempPath,
    concat: tempfile::TempPath,
    stripped: tempfile::TempPath,
}

/// Embed a preview and cover art into an MP4 or MOV file.
///
/// Returns the milliseconds the concat fast path shifted every original frame
/// forward by prepending a new frame; 0 for replacements and re-encodes.
fn embed_mp4_mov(
    video_path: &Path,
    jpeg_bytes: &[u8],
    frame_bytes: &[u8],
    stages: &mut Stages<'_>,
    opts: Mp4Options,
) -> Result<u64> {
    let dir = video_dir(video_path)?;
    let mut video_ext = extension(video_path);
    if video_ext.is_empty() {
        video_ext = ".mp4".to_owned();
    }
    stages.start("Inspecting video streams");
    let replace_existing_preview = opts.replace_existing_preview && video_ext == ".mp4";
    let concat_params = if video_ext == ".mp4" {
        concat_decision_for(video_path, replace_existing_preview)?.0
    } else {
        debug!(
            "concat fast path skipped for '{}': extension {} is not .mp4",
            video_path.display(),
            py_repr_str(&video_ext)
        );
        None
    };

    stages.start("Preparing temporary files");
    let temps = Mp4Temps {
        cover: mkstemp(&dir, ".vt_cover_", ".jpg")?,
        frame: mkstemp(&dir, ".vt_frame_", ".png")?,
        preview: mkstemp(&dir, ".vt_preview_", ".mp4")?,
        concat: mkstemp(&dir, ".vt_concat_", ".txt")?,
        stripped: mkstemp(&dir, ".vt_stripped_", ".mp4")?,
    };
    std::fs::write(&temps.cover, jpeg_bytes)?;
    std::fs::write(&temps.frame, frame_bytes)?;

    let mut shift_ms = 0u64;
    let output_format = "mp4";
    atomic_replace(
        video_path,
        |tmp_video_path| {
            if let Some(params) = &concat_params
                && write_concat(
                    video_path,
                    tmp_video_path,
                    &temps,
                    output_format,
                    params,
                    stages,
                    replace_existing_preview,
                )?
            {
                // Initial saves prepend a frame; replacements keep the duration.
                shift_ms = if replace_existing_preview {
                    0
                } else {
                    (params.frame_duration * 1000.0).round_ties_even() as u64
                };
                let shifted_position_ms = opts.position_ms.map(|p| p + shift_ms);
                let shifted_frame_number = match shifted_position_ms {
                    Some(p) => Some(py_round_div(p, opts.frame_step_ms.max(1))),
                    None => opts.frame_number,
                };
                if opts.write_mp4_covr {
                    stages.start("Writing cover art");
                    mp4_tags::write_covr(tmp_video_path, jpeg_bytes)?;
                }
                stages.start("Writing frame metadata");
                write_source_metadata(tmp_video_path, shifted_frame_number, shifted_position_ms)?;
                return Ok(());
            }

            info!(
                "Full re-encode required for '{}' (concat fast path unavailable)",
                video_path.display()
            );
            stages.start("Encoding video");
            run_ffmpeg(args![
                "-y",
                "-i",
                video_path,
                "-loop",
                "1",
                "-i",
                temps.frame,
                "-i",
                temps.cover,
                "-filter_complex",
                "[1:v][0:v]scale2ref=flags=lanczos[poster][base];\
                 [base][poster]overlay=shortest=1:enable='eq(n,0)'[v]",
                "-map",
                "[v]",
                "-map",
                "0:a?",
                "-map",
                "0:s?",
                "-map",
                "2:v",
                "-map_metadata",
                "0",
                "-map_chapters",
                "0",
                "-c:v:0",
                "libx264",
                "-preset:v:0",
                "veryfast",
                "-crf:v:0",
                "23",
                "-pix_fmt:v:0",
                "yuv420p",
                "-c:v:1",
                "copy",
                "-disposition:v:1",
                "attached_pic",
                "-c:a",
                "copy",
                "-c:s",
                "copy",
                "-f",
                output_format,
                tmp_video_path,
            ])?;
            if opts.write_mp4_covr {
                stages.start("Writing cover art");
                mp4_tags::write_covr(tmp_video_path, jpeg_bytes)?;
            }
            stages.start("Writing frame metadata");
            write_source_metadata(tmp_video_path, opts.frame_number, opts.position_ms)
        },
        false,
    )?;
    stages.start("Cleaning up temporary files");
    drop(temps);
    Ok(shift_ms)
}

fn write_source_metadata(
    path: &Path,
    frame_number: Option<u64>,
    position_ms: Option<u64>,
) -> Result<()> {
    if let (Some(frame_number), Some(position_ms)) = (frame_number, position_ms) {
        mp4_tags::write_thumbnail_source(path, frame_number, position_ms)?;
    }
    Ok(())
}

fn preview_matches(preview: &ProbeInfo, params: &ConcatParams) -> (bool, String) {
    let Some(stream) = preview.streams.iter().find(|s| s.is_video()) else {
        return (false, "no video stream".to_owned());
    };
    let codec_name = stream.codec_name.as_deref().unwrap_or("");
    let mut profile = named_profile(stream).unwrap_or("").to_lowercase();
    if params.codec == ConcatCodec::H264
        && let Some(mapped) = h264_profile(&profile)
    {
        profile = mapped.to_owned();
    }
    let time_base = stream.time_base();
    let codec_ok = match params.codec {
        ConcatCodec::Av1 => codec_name == "av1",
        ConcatCodec::H264 => codec_name == "h264",
    };
    let ok = codec_ok
        && profile == params.profile
        && named_pix_fmt(stream) == Some("yuv420p")
        && stream.width == Some(params.width)
        && stream.height == Some(params.height)
        && time_base.is_some_and(|tb| tb.num == 1 && tb.den == params.video_track_timescale);
    let detail = format!(
        "codec={codec_name}, profile={:?}, pix_fmt={:?}, size={}x{}, time_base={}; \
         expected profile={}, size={}x{}, time_base=1/{}",
        named_profile(stream),
        named_pix_fmt(stream),
        stream.width.unwrap_or(0),
        stream.height.unwrap_or(0),
        py_repr_rational(time_base),
        params.profile,
        params.width,
        params.height,
        params.video_track_timescale
    );
    (ok, detail)
}

// ---------------------------------------------------------------------------
// Leading-DTS repair for the concat output
// ---------------------------------------------------------------------------
//
// The preview clip's only packet has `pts = dts = 0`. A source with B-frames
// starts with negative DTS (x264 default: two frames of reorder delay), which
// the concat demuxer shifts by one frame — still below the preview's 0. The
// mp4 muxer then "fixes" the non-monotonic DTS by squashing the first packets
// to 1-tick durations, which shortens the track by the reorder delay and skews
// `avg_frame_rate` (e.g. `6275/249` instead of `25/1`). Files written that way
// by earlier versions carry the squashed DTS, which a re-save copies along.
//
// Fix: inspect the first packets exactly as the final ffmpeg run will see them
// (ffprobe on the concat manifest) and, when needed, rewrite the leading DTS
// with `setts` so they step back one frame at a time from the first packet that
// is still regular. PTS (and so presentation order and the preview at t=0) are
// untouched. `pts=PTS` must be explicit: `setts` otherwise sets PTS from its
// `ts` expression (default `TS`) too.

/// Timestamps of one demuxed packet, in the stream time base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
struct PacketTs {
    pts: Option<i64>,
    dts: Option<i64>,
    duration: Option<i64>,
}

/// Leading packets of the concat output inspected for the DTS repair.
const DTS_HEAD_PACKETS: usize = 32;

/// First [`DTS_HEAD_PACKETS`] video packets the concat demuxer produces.
fn concat_head(manifest: &Path) -> Result<Vec<PacketTs>> {
    #[derive(serde::Deserialize)]
    struct Packets {
        #[serde(default)]
        packets: Vec<PacketTs>,
    }
    let mut cmd = crate::ffmpeg::command(crate::ffmpeg::Tool::Ffprobe)?;
    cmd.args(["-v", "error", "-f", "concat", "-safe", "0"])
        .args(["-select_streams", "v:0", "-read_intervals"])
        .arg(format!("%+#{DTS_HEAD_PACKETS}"))
        .args([
            "-show_entries",
            "packet=pts,dts,duration",
            "-of",
            "json",
            "-i",
        ])
        .arg(manifest);
    let out = crate::ffmpeg::run(&mut cmd)?;
    let parsed: Packets = serde_json::from_slice(&out.stdout)
        .map_err(|e| Error::Other(format!("invalid ffprobe packet output: {e}")))?;
    Ok(parsed.packets)
}

/// Rewrite of the first `count` packets' DTS to
/// `first packet's PTS + offset - (count - N) * step`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DtsRepair {
    count: usize,
    /// DTS of packet `count` relative to the PTS of packet 0.
    offset: i64,
    step: i64,
}

impl DtsRepair {
    fn setts(&self) -> String {
        let Self {
            count,
            offset,
            step,
        } = *self;
        format!("setts=pts=PTS:dts='if(lt(N,{count}),STARTPTS+({offset})-({count}-N)*{step},DTS)'")
    }
}

/// Plan the leading-DTS repair for `head` (packet 0 is the preview), with
/// `step` ticks per frame. `Ok(None)`: DTS already advance normally.
///
/// A packet is irregular when its DTS advances less than half a frame over the
/// previous one (non-monotonic or muxer-squashed). Everything up to the last
/// irregular packet is rewritten to whole-frame steps ending one frame before
/// the first regular packet.
fn plan_dts_repair(head: &[PacketTs], step: i64) -> std::result::Result<Option<DtsRepair>, String> {
    if step <= 0 {
        return Err(format!("invalid frame step {step}"));
    }
    let dts: Vec<i64> = head
        .iter()
        .map(|p| p.dts)
        .collect::<Option<_>>()
        .ok_or("a leading packet has no DTS")?;
    let Some(start_pts) = head.first().and_then(|p| p.pts) else {
        return Err("the preview packet has no PTS".to_owned());
    };
    let Some(last_irregular) = (1..dts.len())
        .rev()
        .find(|&i| 2 * (dts[i] - dts[i - 1]) < step)
    else {
        return Ok(None);
    };
    let count = last_irregular + 1;
    if count >= dts.len() {
        return Err(format!(
            "DTS are irregular throughout the first {} packets",
            dts.len()
        ));
    }
    let repair = DtsRepair {
        count,
        offset: dts[count] - start_pts,
        step,
    };
    for (n, packet) in head.iter().enumerate().take(count) {
        let new_dts = start_pts + repair.offset - (count - n) as i64 * step;
        if let Some(pts) = packet.pts
            && new_dts > pts
        {
            return Err(format!(
                "repaired DTS {new_dts} of packet {n} would follow its PTS {pts}"
            ));
        }
    }
    Ok(Some(repair))
}

/// `-bsf:v:0 setts=…` arguments repairing the concat output's leading DTS, or
/// none when they are already regular (or cannot be repaired: the muxer then
/// squashes them as before, which still plays).
fn dts_repair_args(manifest: &Path, params: &ConcatParams, video_path: &Path) -> Vec<OsString> {
    let head = match concat_head(manifest) {
        Ok(head) => head,
        Err(err) => {
            log::warn!(
                "could not inspect concat timestamps for '{}': {err}",
                video_path.display()
            );
            return Vec::new();
        }
    };
    let step = head
        .first()
        .and_then(|p| p.duration)
        .filter(|d| *d > 0)
        .unwrap_or_else(|| params.frame_ticks());
    match plan_dts_repair(&head, step) {
        Ok(Some(repair)) => {
            debug!(
                "repairing the first {} DTS of '{}' ({repair:?})",
                repair.count,
                video_path.display()
            );
            args!["-bsf:v:0", repair.setts()]
        }
        Ok(None) => Vec::new(),
        Err(reason) => {
            log::warn!(
                "leading DTS of '{}' left unrepaired: {reason}",
                video_path.display()
            );
            Vec::new()
        }
    }
}

/// Prepend a matching frame, optionally replacing the existing preview.
/// Returns `false` (no output written) when the encoded preview does not
/// match the source parameters.
fn write_concat(
    video_path: &Path,
    output_path: &Path,
    temps: &Mp4Temps,
    output_format: &str,
    params: &ConcatParams,
    stages: &mut Stages<'_>,
    replace_existing_preview: bool,
) -> Result<bool> {
    let duration_text = format_g(params.frame_duration, 12);
    let encoder_args: Vec<OsString> = match params.codec {
        ConcatCodec::Av1 => args![
            "-c:v",
            "libaom-av1",
            "-cpu-used",
            "8",
            "-row-mt",
            "1",
            "-threads",
            "4",
            "-profile:v",
            "0",
            "-crf",
            "23",
            "-b:v",
            "0",
            "-pix_fmt",
            "yuv420p",
        ],
        ConcatCodec::H264 => args![
            "-c:v",
            "libx264",
            "-profile:v",
            params.profile,
            "-pix_fmt",
            "yuv420p"
        ],
    };

    stages.start("Encoding preview frame");
    let mut preview_args = args![
        "-y",
        "-loop",
        "1",
        "-framerate",
        params.frame_rate,
        "-i",
        temps.frame,
        "-frames:v",
        "1",
        "-vf",
        format!("scale={}:{}:flags=lanczos", params.width, params.height),
        "-an",
        "-sn",
    ];
    preview_args.extend(encoder_args);
    preview_args.extend(args![
        "-video_track_timescale",
        params.video_track_timescale.to_string(),
        "-f",
        "mp4",
        temps.preview,
    ]);
    run_ffmpeg(preview_args)?;

    stages.start("Checking encoded frame");
    let preview = probe::probe(&temps.preview)?;
    let (ok, detail) = preview_matches(&preview, params);
    if !ok {
        debug!(
            "concat fast path skipped for '{}': encoded preview frame did not match source params ({detail})",
            video_path.display()
        );
        return Ok(false);
    }

    let mut concat_source: &Path = video_path;
    if replace_existing_preview {
        let source = probe::probe(video_path)?;
        let playable_index = source
            .first_playable_video()
            .map(|s| s.index)
            .ok_or_else(|| Error::NoVideoStream(video_path.to_path_buf()))?;
        stages.start("Removing previous preview frame");
        run_ffmpeg(args![
            "-y",
            "-i",
            video_path,
            "-map",
            format!("0:{playable_index}"),
            "-c:v",
            "copy",
            "-bsf:v",
            "noise=drop='eq(n,0)'",
            "-an",
            "-sn",
            "-f",
            "mp4",
            temps.stripped,
        ])?;
        concat_source = &temps.stripped;
    }

    let manifest = format!(
        "file {}\nduration {duration_text}\nfile {}\n",
        concat_quote(&temps.preview),
        concat_quote(concat_source)
    );
    std::fs::write(&temps.concat, manifest)?;
    let dts_repair = dts_repair_args(&temps.concat, params, video_path);

    stages.start("Copying video streams");
    let mut copy_args = args!["-y", "-f", "concat", "-safe", "0", "-i", temps.concat];
    if !replace_existing_preview {
        copy_args.extend(args!["-itsoffset", duration_text]);
    }
    copy_args.extend(args![
        "-i",
        video_path,
        "-i",
        temps.cover,
        "-map",
        "0:v:0",
        "-map",
        "1:a?",
        "-map",
        "1:s?",
        "-map",
        "2:v:0",
        "-map_metadata",
        "1",
        "-map_chapters",
        "1",
        "-c",
        "copy",
    ]);
    copy_args.extend(dts_repair);
    copy_args.extend(args![
        "-disposition:v:1",
        "attached_pic",
        "-f",
        output_format,
        output_path,
    ]);
    run_ffmpeg(copy_args)?;
    Ok(true)
}

/// Run `ffmpeg … out` into a `.vt_out_*{ext}` file and move it over `tmp`.
fn remux_into(
    tmp: &Path,
    dir: &Path,
    ext: &str,
    build: impl FnOnce(&Path) -> Vec<OsString>,
) -> Result<()> {
    let out = mkstemp(dir, ".vt_out_", ext)?;
    run_ffmpeg(build(&out))?;
    atomic_write::replace(&out, tmp)?;
    // Renamed away: nothing left to delete.
    let _ = out.keep();
    Ok(())
}

/// Embed cover art into an MKV file using `ffmpeg -attach`.
fn embed_mkv(video_path: &Path, jpeg_bytes: &[u8], stages: &mut Stages<'_>) -> Result<()> {
    stages.start("Preparing temporary files");
    let dir = video_dir(video_path)?;
    let mut ext = extension(video_path);
    if ext.is_empty() {
        ext = ".mkv".to_owned();
    }
    let cover = mkstemp(&dir, ".vt_cover_", ".jpg")?;
    std::fs::write(&cover, jpeg_bytes)?;
    atomic_replace(
        video_path,
        |tmp| {
            remux_into(tmp, &dir, &ext, |out| {
                stages.start("Attaching cover art");
                args![
                    "-y",
                    "-i",
                    tmp,
                    "-attach",
                    cover,
                    "-metadata:s:t",
                    "mimetype=image/jpeg",
                    "-metadata:s:t",
                    "filename=cover.jpg",
                    "-c",
                    "copy",
                    out,
                ]
            })
        },
        true,
    )?;
    stages.start("Cleaning up temporary files");
    drop(cover);
    Ok(())
}

/// Re-mux an AVI file in place (no cover art; preserves all streams).
fn remux_avi(video_path: &Path, stages: &mut Stages<'_>) -> Result<()> {
    stages.start("Preparing temporary files");
    let dir = video_dir(video_path)?;
    atomic_replace(
        video_path,
        |tmp| {
            remux_into(tmp, &dir, ".avi", |out| {
                stages.start("Remuxing video");
                args!["-y", "-i", tmp, "-c", "copy", out]
            })
        },
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_g_matches_python() {
        assert_eq!(format_g(1.0 / 25.0, 12), "0.04");
        assert_eq!(format_g(1.0 / (30000.0 / 1001.0), 12), "0.0333666666667");
        assert_eq!(format_g(1.0 / 24.0, 12), "0.0416666666667");
        assert_eq!(format_g(1.0 / 60.0, 12), "0.0166666666667");
        assert_eq!(format_g(1.0, 12), "1");
        assert_eq!(format_g(2.5, 12), "2.5");
        assert_eq!(format_g(0.00001234, 12), "1.234e-05");
        assert_eq!(format_g(1e13, 12), "1e+13");
        assert_eq!(format_g(123456789012.0, 12), "123456789012");
    }

    #[test]
    fn python_rounding_is_banker() {
        assert_eq!(py_round_div(1020, 40), 26); // 25.5 → 26
        assert_eq!(py_round_div(1060, 40), 26); // 26.5 → 26
        assert_eq!(py_round_div(1680, 40), 42);
        assert_eq!(py_round_div(1000, 33), 30);
    }

    #[test]
    fn repr_helpers() {
        assert_eq!(py_repr_str("High 4:4:4"), "'High 4:4:4'");
        assert_eq!(py_repr_str("it's"), "\"it's\"");
        assert_eq!(py_repr_rational(None), "None");
        assert_eq!(
            py_repr_rational(Some(Rational { num: 2, den: 50 })),
            "AVRational(2, 50)"
        );
    }

    #[test]
    fn concat_quote_escapes() {
        let quoted = concat_quote(Path::new("/tmp/it's.mp4"));
        assert_eq!(quoted, r"'/tmp/it'\''s.mp4'");
        if cfg!(windows) {
            assert_eq!(concat_quote(Path::new(r"C:\a\b.mp4")), "'C:/a/b.mp4'");
        }
    }

    fn probe_json(stream: &str, extra: &str) -> ProbeInfo {
        let json =
            format!(r#"{{"streams": [{stream}{extra}], "format": {{"format_name": "mov,mp4"}}}}"#);
        probe::parse(json.as_bytes()).unwrap()
    }

    const H264: &str = r#"{"index": 0, "codec_type": "video", "codec_name": "h264",
        "codec_long_name": "H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10", "profile": "High",
        "pix_fmt": "yuv420p", "width": 320, "height": 240, "time_base": "1/12800",
        "avg_frame_rate": "25/1", "r_frame_rate": "25/1", "disposition": {"attached_pic": 0}}"#;
    const COVER: &str = r#", {"index": 1, "codec_type": "video", "codec_name": "mjpeg",
        "width": 64, "height": 64, "disposition": {"attached_pic": 1}}"#;

    #[test]
    fn eligible_h264() {
        let info = probe_json(H264, "");
        let (params, reason) = concat_decision(&info, Path::new("x.mp4"), false);
        assert_eq!(reason, None);
        let params = params.unwrap();
        assert_eq!(params.codec, ConcatCodec::H264);
        assert_eq!(params.profile, "high");
        assert_eq!(params.frame_rate, "25/1");
        assert_eq!(params.frame_duration, 0.04);
        assert_eq!(params.video_track_timescale, 12800);
    }

    #[test]
    fn attached_pic_requires_replacement_flag() {
        let info = probe_json(H264, COVER);
        let (params, reason) = concat_decision(&info, Path::new("x.mp4"), false);
        assert!(params.is_none());
        assert_eq!(
            reason.as_deref(),
            Some("An attached-picture video stream is already present.")
        );
        assert!(concat_decision(&info, Path::new("x.mp4"), true).0.is_some());
    }

    #[test]
    fn vp9_reason_mentions_codec() {
        let vp9 = r#"{"index": 0, "codec_type": "video", "codec_name": "vp9",
            "codec_long_name": "Google VP9", "profile": "Main", "pix_fmt": "yuv420p",
            "width": 1920, "height": 1080, "time_base": "1/1000", "avg_frame_rate": "60/1",
            "disposition": {"attached_pic": 0}}"#;
        let (_, reason) = concat_decision(&probe_json(vp9, ""), Path::new("x.mp4"), false);
        assert_eq!(
            reason.as_deref(),
            Some("video codec is Google VP9 (fast copy supports H.264 and AV1 only).")
        );
    }

    #[test]
    fn collects_all_reasons() {
        let bad = r#"{"index": 0, "codec_type": "video", "codec_name": "mpeg2video",
            "codec_long_name": "MPEG-2 video", "profile": "unknown", "pix_fmt": "yuv422p",
            "width": 0, "height": 240, "time_base": "2/90001", "avg_frame_rate": "0/0",
            "disposition": {"attached_pic": 0}}"#;
        let (_, reason) = concat_decision(&probe_json(bad, ""), Path::new("x.mp4"), false);
        assert_eq!(
            reason.as_deref(),
            Some(
                "video codec is MPEG-2 video (fast copy supports H.264 and AV1 only); \
                 profile is '(none)'; pixel format is 'yuv422p', not yuv420p; \
                 invalid dimensions 0x240; unsupported video time base AVRational(2, 90001); \
                 invalid frame rate None."
            )
        );
    }

    #[test]
    fn stream_count_reason() {
        let two = format!("{H264}, {}", H264.replace("\"index\": 0", "\"index\": 1"));
        let (_, reason) = concat_decision(&probe_json(&two, ""), Path::new("x.mp4"), false);
        assert_eq!(
            reason.as_deref(),
            Some("Expected one playable video stream; found 2.")
        );
    }

    #[test]
    fn av1_requires_main_profile() {
        let av1 = r#"{"index": 0, "codec_type": "video", "codec_name": "av1",
            "codec_long_name": "Alliance for Open Media AV1", "profile": "High",
            "pix_fmt": "yuv420p", "width": 320, "height": 240, "time_base": "1/90000",
            "avg_frame_rate": "30000/1001", "disposition": {"attached_pic": 0}}"#;
        let (_, reason) = concat_decision(&probe_json(av1, ""), Path::new("x.mp4"), false);
        assert_eq!(reason.as_deref(), Some("profile is 'High'."));
        let ok = av1.replace("\"High\"", "\"Main\"");
        let (params, _) = concat_decision(&probe_json(&ok, ""), Path::new("x.mp4"), false);
        let params = params.unwrap();
        assert_eq!(params.codec, ConcatCodec::Av1);
        assert_eq!(params.frame_rate, "30000/1001");
        assert_eq!(params.video_track_timescale, 90000);
    }

    fn head(ts: &[(i64, i64)]) -> Vec<PacketTs> {
        ts.iter()
            .map(|&(pts, dts)| PacketTs {
                pts: Some(pts),
                dts: Some(dts),
                duration: Some(512),
            })
            .collect()
    }

    #[test]
    fn dts_repair_for_b_frame_source() {
        // Preview (0/0), then an x264 source shifted by one frame.
        let h = head(&[(0, 0), (512, -512), (1536, 0), (1024, 512), (3584, 1024)]);
        let repair = plan_dts_repair(&h, 512).unwrap().unwrap();
        assert_eq!(
            repair,
            DtsRepair {
                count: 2,
                offset: 0,
                step: 512
            }
        );
        assert_eq!(
            repair.setts(),
            "setts=pts=PTS:dts='if(lt(N,2),STARTPTS+(0)-(2-N)*512,DTS)'"
        );
    }

    #[test]
    fn dts_repair_for_squashed_resave() {
        // A file saved by earlier versions, its preview stripped: DTS 1, 2.
        let h = head(&[(0, 0), (512, 1), (1536, 2), (1024, 512), (3584, 1024)]);
        let repair = plan_dts_repair(&h, 512).unwrap().unwrap();
        assert_eq!((repair.count, repair.offset), (3, 512));
        // New DTS: -1024, -512, 0 — all at or before their PTS.
    }

    #[test]
    fn dts_repair_not_needed_without_reordering() {
        let h = head(&[(0, 0), (512, 512), (1024, 1024), (1536, 1536)]);
        assert_eq!(plan_dts_repair(&h, 512), Ok(None));
        // Tiny steps (time base = 1/fps) still count as regular.
        let h = head(&[(0, 0), (1, 1), (2, 2)]);
        assert_eq!(plan_dts_repair(&h, 1), Ok(None));
    }

    #[test]
    fn dts_repair_refuses_impossible_plans() {
        // DTS would have to follow PTS.
        let h = head(&[(0, 0), (0, 0), (10_000, 10_000)]);
        assert!(plan_dts_repair(&h, 512).is_err());
        // Never regular within the window.
        let h = head(&[(0, 0), (512, 1), (1024, 2)]);
        assert!(plan_dts_repair(&h, 512).is_err());
        let mut h = head(&[(0, 0), (512, 512)]);
        h[1].dts = None;
        assert!(plan_dts_repair(&h, 512).is_err());
    }

    #[test]
    fn error_mapping() {
        let p = Path::new("v.mp4");
        let r = map_error(
            Error::Ffmpeg {
                code: 1,
                stderr: String::new(),
            },
            p,
        );
        assert_eq!(r.error_code, Some(ApplyError::FfmpegError));
        assert_eq!(
            r.error_message.as_deref(),
            Some("ffmpeg failed (exit 1). stderr: (none)")
        );
        let r = map_error(
            Error::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
            p,
        );
        assert_eq!(r.error_code, Some(ApplyError::FileNotWritable));
        assert!(
            r.error_message
                .unwrap()
                .starts_with("Permission denied writing 'v.mp4': ")
        );
        let r = map_error(
            Error::Io(std::io::Error::from(std::io::ErrorKind::StorageFull)),
            p,
        );
        assert_eq!(r.error_code, Some(ApplyError::DiskFull));
        let r = map_error(Error::Io(std::io::Error::other("boom")), p);
        assert_eq!(
            r.error_message.as_deref(),
            Some("Unexpected OS error: boom")
        );
        let r = map_error(Error::Other("boom".into()), p);
        assert_eq!(r.error_message.as_deref(), Some("Unexpected error: boom"));
    }

    #[test]
    fn rejects_unwritable_and_unsupported() {
        let mut video = VideoFile {
            path: PathBuf::from("placeholder.rm"),
            format: VideoFormat::Unsupported,
            duration_ms: 3000,
            width: 320,
            height: 240,
            existing_thumbnail: None,
            is_writable: true,
            frame_step_ms: 40,
            thumbnail_frame_number: None,
            thumbnail_position_ms: None,
        };
        let img = RgbImage::new(4, 4);
        let w = FormatDispatchThumbnailWriter;
        let r = w.write(&video, &img, None, None);
        assert_eq!(r.error_code, Some(ApplyError::UnsupportedFormat));
        assert_eq!(
            r.error_message.as_deref(),
            Some("Format UNSUPPORTED is not supported. Supported formats: MP4, MOV.")
        );
        video.is_writable = false;
        let r = w.write(&video, &img, None, None);
        assert_eq!(r.error_code, Some(ApplyError::FileNotWritable));
        assert_eq!(
            r.error_message.as_deref(),
            Some(
                "'placeholder.rm' is not writable. Copy the file to a writable location and try again."
            )
        );
    }
}
