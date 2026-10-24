//! Video file loading (port of `core/video_loader.py`).
//!
//! A single `ffprobe` run provides the container format, stream layout,
//! duration, frame rate and FrameCover's saved frame-source tags; embedded
//! cover art is pulled out with one extra `ffmpeg` run only when present.

use std::path::Path;

use image::RgbImage;

use crate::error::{Error, Result};
use crate::ffmpeg::{self, Tool};
use crate::imaging;
use crate::models::{VideoFile, VideoFormat};
use crate::mp4_tags::{FRAME_NUMBER_NAME, POSITION_MS_NAME};
use crate::probe::{self, ProbeInfo, Rational, Stream};

/// Loads a video file and returns a populated [`VideoFile`].
pub trait VideoLoader: Send + Sync {
    /// Errors: `Io(NotFound)`, `UnsupportedFormat`, `NoVideoStream`.
    fn load(&self, path: &Path) -> Result<VideoFile>;
}

/// ffprobe-backed loader.
#[derive(Debug, Default, Clone)]
pub struct FfprobeVideoLoader;

impl VideoLoader for FfprobeVideoLoader {
    fn load(&self, path: &Path) -> Result<VideoFile> {
        let abs_path = std::path::absolute(path)?;
        if !abs_path.exists() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Video file not found: '{}'", abs_path.display()),
            )));
        }

        let info = probe::probe(&abs_path)?;
        let format = detect_format(&info.format.format_name, &abs_path);
        if format == VideoFormat::Unsupported {
            return Err(Error::UnsupportedFormat(abs_path));
        }

        let Some(stream) = info.first_playable_video() else {
            return Err(Error::NoVideoStream(abs_path));
        };

        let duration_ms = duration_ms(stream, &info);
        let width = stream.width.unwrap_or(0);
        let height = stream.height.unwrap_or(0);
        let frame_step_ms = frame_step_ms(stream.effective_rate(), stream.base_rate());
        let existing_thumbnail = extract_existing_thumbnail(&abs_path, &info);
        let is_writable = is_writable(&abs_path);
        let (thumbnail_frame_number, thumbnail_position_ms) = if format.is_mp4_family() {
            saved_thumbnail_source(&info)
        } else {
            (None, None)
        };

        Ok(VideoFile {
            path: abs_path,
            format,
            duration_ms,
            width,
            height,
            existing_thumbnail,
            is_writable,
            frame_step_ms,
            thumbnail_frame_number,
            thumbnail_position_ms,
        })
    }
}

/// Map a libavformat format name (e.g. `mov,mp4,m4a,3gp,3g2,mj2`) to a
/// [`VideoFormat`], using the file extension to split combined names.
pub fn detect_format(format_name: &str, path: &Path) -> VideoFormat {
    let lower = format_name.to_lowercase();
    let names: Vec<&str> = lower.split(',').map(str::trim).collect();
    let has = |n: &str| names.contains(&n);
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    if has("matroska") && has("webm") {
        return if ext == "webm" {
            VideoFormat::Webm
        } else {
            VideoFormat::Mkv
        };
    }
    if has("mov") && has("mp4") {
        return match ext.as_str() {
            "mov" | "m4v" => VideoFormat::Mov,
            // `.mp4` and unknown extensions: MP4 is the most common container.
            _ => VideoFormat::Mp4,
        };
    }
    names
        .iter()
        .find_map(|name| match *name {
            "mp4" => Some(VideoFormat::Mp4),
            "mov" | "quicktime" => Some(VideoFormat::Mov),
            "matroska" => Some(VideoFormat::Mkv),
            "webm" => Some(VideoFormat::Webm),
            "avi" => Some(VideoFormat::Avi),
            "flv" => Some(VideoFormat::Flv),
            _ => None,
        })
        .unwrap_or(VideoFormat::Unsupported)
}

/// Stream duration if known, else container duration; truncated to ms.
///
/// For a stream with a drifted average frame rate (see
/// [`Stream::effective_rate`]) the reported duration is too short by the
/// squashed frames, so it is raised to `nb_frames / r_frame_rate` when the
/// frame count is known.
fn duration_ms(stream: &Stream, info: &ProbeInfo) -> u64 {
    let reported = reported_duration_ms(stream, info);
    if stream.has_rate_drift()
        && let (Some(frames), Some(rate)) = (stream.nb_frames(), stream.base_rate())
    {
        return reported.max(to_ms(frames as f64 * 1000.0 / rate.to_f64()));
    }
    reported
}

fn reported_duration_ms(stream: &Stream, info: &ProbeInfo) -> u64 {
    if let (Some(ts), Some(tb)) = (stream.duration_ts(), stream.time_base()) {
        // Same float arithmetic as `float(stream.duration) * float(time_base) * 1000`.
        return to_ms(ts as f64 * tb.to_f64() * 1000.0);
    }
    if let Some(secs) = stream.duration_secs() {
        return to_ms(secs * 1000.0);
    }
    if let Some(secs) = info.format.duration_secs() {
        // ffprobe prints AV_TIME_BASE (µs) values with 6 decimals.
        let micros = (secs * 1_000_000.0).round();
        return if micros > 0.0 {
            micros as u64 / 1000
        } else {
            0
        };
    }
    0
}

fn to_ms(value: f64) -> u64 {
    if value.is_finite() && value > 0.0 {
        value as u64
    } else {
        0
    }
}

/// Approximate frame duration: average rate, then base rate, else 40 ms.
/// Rounds half to even like Python's `round`.
pub fn frame_step_ms(average: Option<Rational>, base: Option<Rational>) -> u64 {
    [average, base]
        .into_iter()
        .flatten()
        .map(Rational::to_f64)
        .find(|fps| fps.is_finite() && *fps > 0.0)
        .map(|fps| ((1000.0 / fps).round_ties_even() as u64).max(1))
        .unwrap_or(40)
}

const COVER_ATTACHMENT_NAMES: [&str; 4] = ["cover.jpg", "cover.png", "folder.jpg", "cover.jpeg"];

/// Embedded cover art: MP4/MOV `attached_pic` streams or MKV/WebM
/// attachments named like a cover. Failures are treated as "no cover".
fn extract_existing_thumbnail(path: &Path, info: &ProbeInfo) -> Option<RgbImage> {
    for stream in &info.streams {
        if stream.is_video() && stream.is_attached_pic() {
            match attached_pic(path, stream.index) {
                Ok(img) => return Some(img),
                Err(e) => log::debug!("attached_pic {} unreadable: {e}", stream.index),
            }
        }
        if matches!(stream.codec_type.as_str(), "attachment" | "data") {
            let name = stream.tag("filename").unwrap_or_default().to_lowercase();
            if COVER_ATTACHMENT_NAMES.contains(&name.as_str()) {
                match attachment(path, stream.index) {
                    Ok(img) => return Some(img),
                    Err(e) => log::debug!("attachment {} unreadable: {e}", stream.index),
                }
            }
        }
    }
    None
}

fn attached_pic(path: &Path, index: usize) -> Result<RgbImage> {
    let run = |codec: &[&str]| -> Result<Vec<u8>> {
        let mut cmd = ffmpeg::command(Tool::Ffmpeg)?;
        cmd.args(["-hide_banner", "-nostdin", "-v", "error", "-i"])
            .arg(path)
            .args(["-map", &format!("0:{index}"), "-frames:v", "1"])
            .args(codec)
            .args(["-f", "image2pipe", "-"]);
        Ok(ffmpeg::run(&mut cmd)?.stdout)
    };
    // Stream-copy the picture packet and decode it ourselves.
    if let Ok(bytes) = run(&["-c", "copy"])
        && let Ok(img) = imaging::decode_rgb(&bytes)
    {
        return Ok(img);
    }
    // A codec the image crate cannot read (BMP, GIF…): let ffmpeg transcode.
    imaging::decode_rgb(&run(&["-c:v", "png", "-pix_fmt", "rgb24"])?)
}

fn attachment(path: &Path, index: usize) -> Result<RgbImage> {
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("attachment");
    let mut cmd = ffmpeg::command(Tool::Ffmpeg)?;
    cmd.args(["-hide_banner", "-nostdin", "-v", "error"])
        .arg(format!("-dump_attachment:{index}"))
        .arg(&target)
        .arg("-i")
        .arg(path);
    // ffmpeg dumps the attachment, then exits non-zero because no output
    // file was given; only the dumped file matters.
    let _ = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    let bytes = std::fs::read(&target)?;
    if bytes.is_empty() {
        return Err(Error::Other("empty attachment".into()));
    }
    imaging::decode_rgb(&bytes)
}

/// Mirrors `os.access(path, W_OK)`.
fn is_writable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.permissions().readonly() {
        return false;
    }
    if cfg!(windows) {
        // On Windows `os.access` only consults the read-only attribute; opening
        // for write would also fail for files merely held open by a player.
        true
    } else {
        // Honour owner/group bits and ACLs. Opening without truncation does
        // not modify the file.
        std::fs::OpenOptions::new().write(true).open(path).is_ok()
    }
}

/// FrameCover's saved frame-source freeform tags
/// (`----:com.apple.iTunes:video-thumbnailer-*`). ffprobe's mov demuxer
/// reports freeform atoms as plain format tags keyed by their name.
fn saved_thumbnail_source(info: &ProbeInfo) -> (Option<u64>, Option<u64>) {
    let read = |name: &str| {
        info.format
            .tags
            .get(name)
            .and_then(|v| parse_non_negative(v))
    };
    (read(FRAME_NUMBER_NAME), read(POSITION_MS_NAME))
}

/// Python `int(text)` restricted to non-negative values.
fn parse_non_negative(text: &str) -> Option<u64> {
    let value: i128 = text.trim().replace('_', "").parse().ok()?;
    u64::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOV: &str = "mov,mp4,m4a,3gp,3g2,mj2";

    fn fmt(name: &str, path: &str) -> VideoFormat {
        detect_format(name, Path::new(path))
    }

    #[test]
    fn detects_formats() {
        assert_eq!(fmt(MOV, "/path/to/video.mp4"), VideoFormat::Mp4);
        assert_eq!(fmt(MOV, "/path/to/video.mov"), VideoFormat::Mov);
        assert_eq!(fmt(MOV, "/path/to/video.M4V"), VideoFormat::Mov);
        assert_eq!(fmt(MOV, "/path/to/video.xyz"), VideoFormat::Mp4);
        assert_eq!(fmt(MOV, "/path/to/video"), VideoFormat::Mp4);
        assert_eq!(fmt("matroska,webm", "/path/to/video.mkv"), VideoFormat::Mkv);
        assert_eq!(
            fmt("matroska,webm", "/path/to/video.webm"),
            VideoFormat::Webm
        );
        assert_eq!(fmt("matroska,webm", "/path/to/video.bin"), VideoFormat::Mkv);
        assert_eq!(fmt("avi", "/path/to/video.avi"), VideoFormat::Avi);
        assert_eq!(fmt("flv", "/path/to/video.flv"), VideoFormat::Flv);
        assert_eq!(fmt("rm", "/path/to/video.rm"), VideoFormat::Unsupported);
        assert_eq!(fmt("ipod,mp4", "/x.mp4"), VideoFormat::Mp4);
    }

    fn r(text: &str) -> Option<Rational> {
        Rational::parse(text).filter(|r| r.num > 0)
    }

    #[test]
    fn frame_step_rules() {
        assert_eq!(frame_step_ms(r("25/1"), r("25/1")), 40);
        assert_eq!(frame_step_ms(r("30000/1001"), None), 33);
        assert_eq!(frame_step_ms(None, r("60/1")), 17);
        assert_eq!(frame_step_ms(None, None), 40);
        // Python rounds half to even: 1000/80 = 12.5 -> 12.
        assert_eq!(frame_step_ms(r("80/1"), None), 12);
        assert_eq!(frame_step_ms(r("100000/1"), None), 1);
        // Raw average of a drifted file; the loader passes the effective rate.
        assert_eq!(frame_step_ms(r("1575/62"), r("25/1")), 39);
    }

    #[test]
    fn drifted_average_uses_base_rate_and_frame_count() {
        // Output of the old concat fast path (golden README, behaviour 1).
        let json = br#"{"streams":[{"index":0,"codec_type":"video","time_base":"1/12800",
            "avg_frame_rate":"1575/62","r_frame_rate":"25/1","nb_frames":"126",
            "duration_ts":63488,"duration":"4.960000"}],
            "format":{"format_name":"mov","duration":"5.040000"}}"#;
        let info = probe::parse(json).unwrap();
        let s = &info.streams[0];
        assert_eq!(frame_step_ms(s.effective_rate(), s.base_rate()), 40);
        assert_eq!(duration_ms(s, &info), 5040);
    }

    #[test]
    fn parses_tag_integers() {
        assert_eq!(parse_non_negative("42"), Some(42));
        assert_eq!(parse_non_negative(" +7 "), Some(7));
        assert_eq!(parse_non_negative("-1"), None);
        assert_eq!(parse_non_negative("abc"), None);
    }

    #[test]
    fn duration_prefers_stream() {
        let json = br#"{"streams":[{"index":0,"codec_type":"video","time_base":"1/12800",
            "duration_ts":64000,"duration":"5.000000"}],
            "format":{"format_name":"mov","duration":"5.023000"}}"#;
        let info = probe::parse(json).unwrap();
        assert_eq!(duration_ms(&info.streams[0], &info), 5000);

        let json = br#"{"streams":[{"index":0,"codec_type":"video"}],
            "format":{"format_name":"matroska,webm","duration":"5.023000"}}"#;
        let info = probe::parse(json).unwrap();
        assert_eq!(duration_ms(&info.streams[0], &info), 5023);
    }
}
