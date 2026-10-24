//! Write-side test helpers: a minimal probe-based stand-in for the loader
//! (which is developed separately) plus ffmpeg-based inspection utilities
//! replacing the PyAV calls of the Python tests.
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

use framecover_core::ffmpeg::{self, Tool};
use framecover_core::models::{VideoFile, VideoFormat};
use framecover_core::{imaging, mp4_tags, probe};
use image::RgbImage;

/// Build a [`VideoFile`] the way `PyAVVideoLoader.load` does.
pub fn load(path: &Path) -> VideoFile {
    let path = std::path::absolute(path).unwrap();
    let info = probe::probe(&path).expect("probe");
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let format = match ext.as_str() {
        "mp4" | "m4v" => VideoFormat::Mp4,
        "mov" => VideoFormat::Mov,
        "mkv" => VideoFormat::Mkv,
        "avi" => VideoFormat::Avi,
        "webm" => VideoFormat::Webm,
        "flv" => VideoFormat::Flv,
        _ => VideoFormat::Unsupported,
    };
    let stream = info.first_playable_video().expect("playable video");
    let duration_ms = stream
        .duration_secs()
        .or_else(|| info.format.duration_secs())
        .map(|s| (s * 1000.0) as u64)
        .unwrap_or(0);
    let frame_step_ms = stream
        .effective_rate()
        .or_else(|| stream.base_rate())
        .map(|r| ((1000.0 / r.to_f64()).round() as u64).max(1))
        .unwrap_or(40);
    let (frame_number, position_ms) = if format.is_mp4_family() {
        mp4_tags::read_thumbnail_source(&path)
    } else {
        (None, None)
    };
    let is_writable = std::fs::metadata(&path)
        .map(|m| !m.permissions().readonly())
        .unwrap_or(false);
    VideoFile {
        format,
        duration_ms,
        width: stream.width.unwrap_or(0),
        height: stream.height.unwrap_or(0),
        existing_thumbnail: existing_thumbnail(&path),
        is_writable,
        frame_step_ms,
        thumbnail_frame_number: frame_number,
        thumbnail_position_ms: position_ms,
        path,
    }
}

/// First attached picture (MP4 `attached_pic` or MKV cover attachment).
pub fn existing_thumbnail(path: &Path) -> Option<RgbImage> {
    let info = probe::probe(path).ok()?;
    let stream = info.streams.iter().find(|s| {
        (s.is_video() && s.is_attached_pic())
            || (matches!(s.codec_type.as_str(), "attachment" | "data")
                && matches!(
                    s.tag("filename").map(str::to_lowercase).as_deref(),
                    Some("cover.jpg" | "cover.png" | "folder.jpg" | "cover.jpeg")
                ))
    })?;
    let bytes = if stream.codec_type == "attachment" {
        let dir = tempfile::tempdir().ok()?;
        let out = dir.path().join("cover.bin");
        let mut cmd = ffmpeg::command(Tool::Ffmpeg).ok()?;
        cmd.current_dir(dir.path())
            .arg(format!("-dump_attachment:{}", stream.index))
            .arg(&out)
            .arg("-i")
            .arg(path);
        let _ = cmd.output();
        std::fs::read(&out).ok()?
    } else {
        attached_pic_bytes(path)?
    };
    imaging::decode_rgb(&bytes).ok()
}

/// Encoded cover bytes of the attached picture stream, if any.
pub fn attached_pic_bytes(path: &Path) -> Option<Vec<u8>> {
    let info = probe::probe(path).ok()?;
    let stream = info
        .streams
        .iter()
        .find(|s| s.is_video() && s.is_attached_pic())?;
    let map = format!("0:{}", stream.index);
    let out = ffmpeg::ffmpeg([
        "-v",
        "error",
        "-i",
        path.to_str().unwrap(),
        "-map",
        &map,
        "-c",
        "copy",
        "-f",
        "image2pipe",
        "-",
    ])
    .ok()?;
    Some(out.stdout)
}

pub fn has_attached_pic_stream(path: &Path) -> bool {
    probe::probe(path).unwrap().has_attached_pic()
}

fn playable_index(path: &Path) -> usize {
    probe::probe(path)
        .unwrap()
        .first_playable_video()
        .expect("playable")
        .index
}

/// Decode the first frame of the first playable video stream as RGB.
pub fn first_playable_frame(path: &Path) -> RgbImage {
    let info = probe::probe(path).unwrap();
    let s = info.first_playable_video().unwrap();
    let (w, h) = (s.width.unwrap(), s.height.unwrap());
    let map = format!("0:{}", s.index);
    let out = ffmpeg::ffmpeg([
        "-v",
        "error",
        "-i",
        path.to_str().unwrap(),
        "-map",
        &map,
        "-frames:v",
        "1",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-",
    ])
    .unwrap();
    RgbImage::from_raw(w, h, out.stdout).expect("frame size")
}

/// Stand-in for the frame extractor (developed separately): decode the frame
/// at `offset_ms` of the first playable stream as RGB.
pub fn extract_frame(path: &Path, offset_ms: u64) -> RgbImage {
    let info = probe::probe(path).unwrap();
    let s = info.first_playable_video().unwrap();
    let (w, h) = (s.width.unwrap(), s.height.unwrap());
    let map = format!("0:{}", s.index);
    let ss = format!("{:.3}", offset_ms as f64 / 1000.0);
    let out = ffmpeg::ffmpeg([
        "-v",
        "error",
        "-ss",
        &ss,
        "-i",
        path.to_str().unwrap(),
        "-map",
        &map,
        "-frames:v",
        "1",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-",
    ])
    .unwrap();
    RgbImage::from_raw(w, h, out.stdout).expect("frame size")
}

/// Number of decoded frames of the first playable video stream.
pub fn frame_count(path: &Path) -> usize {
    video_frames(path).len()
}

/// Per-channel mean, like `PIL.ImageStat.Stat(img).mean`.
pub fn mean(img: &RgbImage) -> [f64; 3] {
    let mut sum = [0f64; 3];
    for p in img.pixels() {
        for (acc, &v) in sum.iter_mut().zip(&p.0) {
            *acc += v as f64;
        }
    }
    let n = (img.width() * img.height()) as f64;
    sum.map(|s| s / n)
}

/// `framemd5` lines (size + hash) of the first playable video stream;
/// `copy = true` hashes packets, otherwise decoded frames.
fn framemd5(path: &Path, copy: bool, stream: &str) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-v".into(),
        "error".into(),
        "-i".into(),
        path.to_str().unwrap().into(),
        "-map".into(),
        stream.into(),
    ];
    if copy {
        args.extend(["-c".into(), "copy".into()]);
    }
    args.extend(["-f".into(), "framemd5".into(), "-".into()]);
    let out = ffmpeg::ffmpeg(&args).unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            (f.len() >= 6 && f[4] != "0").then(|| format!("{}:{}", f[4], f[5]))
        })
        .collect()
}

/// Non-empty packet hashes of the first playable video stream.
pub fn video_packets(path: &Path) -> Vec<String> {
    framemd5(path, true, &format!("0:{}", playable_index(path)))
}

/// Decoded frame hashes of the first playable video stream.
pub fn video_frames(path: &Path) -> Vec<String> {
    framemd5(path, false, &format!("0:{}", playable_index(path)))
}

/// Non-empty packet hashes of the first audio stream.
pub fn audio_packets(path: &Path) -> Vec<String> {
    framemd5(path, true, "0:a:0")
}

/// Run ffmpeg with raw args (fixture generation).
pub fn run_ffmpeg(args: &[&str]) {
    ffmpeg::ffmpeg(args).expect("ffmpeg");
}

/// Names of stream codecs of a type (`video`, `audio`).
pub fn codecs(path: &Path, codec_type: &str) -> Vec<String> {
    probe::probe(path)
        .unwrap()
        .streams
        .iter()
        .filter(|s| s.codec_type == codec_type)
        .filter_map(|s| s.codec_name.clone())
        .collect()
}

/// Whether ffmpeg has an encoder (to skip AV1 tests on minimal builds).
pub fn has_encoder(name: &str) -> bool {
    ffmpeg::available_encoders()
        .map(|e| e.iter().any(|n| n == name))
        .unwrap_or(false)
}

/// Plain `Command` for ffprobe with arbitrary args.
pub fn ffprobe() -> Command {
    ffmpeg::command(Tool::Ffprobe).unwrap()
}
