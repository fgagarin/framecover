//! Shared helpers for integration tests: synthetic videos generated with the
//! located ffmpeg (port of `tests/conftest.py`).
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use framecover_core::ffmpeg;
use image::RgbImage;

/// Generate a `testsrc` + `sine` video of `duration` seconds at 25 fps.
pub fn generate_video(output: &Path, duration: u32, width: u32, height: u32, extra: &[&str]) {
    let src = format!("testsrc=size={width}x{height}:rate=25:duration={duration}");
    let sine = format!("sine=frequency=440:duration={duration}");
    let mut args: Vec<&str> = vec!["-y", "-f", "lavfi", "-i", &src, "-f", "lavfi", "-i", &sine];
    args.extend_from_slice(extra);
    let out = output.to_str().expect("utf-8 path");
    args.push(out);
    ffmpeg::ffmpeg(&args).expect("generate fixture video");
}

pub const H264_AAC: &[&str] = &["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p"];
pub const VP9_OPUS: &[&str] = &["-c:v", "libvpx-vp9", "-c:a", "libopus", "-b:v", "200k"];

/// 5-second 320x240 H.264/AAC MP4.
pub fn sample_video(dir: &Path) -> PathBuf {
    let out = dir.join("sample.mp4");
    generate_video(&out, 5, 320, 240, H264_AAC);
    out
}

/// Fixture for a container extension: mp4, mov, mkv, avi, flv (H.264/AAC) or webm (VP9/Opus).
pub fn video_for_format(dir: &Path, ext: &str) -> PathBuf {
    let out = dir.join(format!("sample.{ext}"));
    match ext {
        "mp4" | "mov" | "mkv" | "avi" | "flv" => generate_video(&out, 5, 320, 240, H264_AAC),
        "webm" => generate_video(&out, 5, 320, 240, VP9_OPUS),
        other => panic!("unsupported fixture format {other}"),
    }
    out
}

/// MP4 with an extra JPEG `attached_pic` stream (blue 320x240 cover).
pub fn video_with_thumb(dir: &Path) -> PathBuf {
    let base = dir.join("base.mp4");
    generate_video(&base, 5, 320, 240, H264_AAC);
    let cover = dir.join("cover.jpg");
    ffmpeg::ffmpeg([
        "-y", "-f", "lavfi", "-i", "color=c=blue:size=320x240:rate=1:duration=1",
        "-frames:v", "1", "-q:v", "2", cover.to_str().unwrap(),
    ])
    .expect("cover");
    let out = dir.join("with_thumb.mp4");
    ffmpeg::ffmpeg([
        "-y", "-i", base.to_str().unwrap(), "-i", cover.to_str().unwrap(),
        "-map", "0", "-map", "1", "-c", "copy", "-disposition:v:1", "attached_pic",
        out.to_str().unwrap(),
    ])
    .expect("attach cover");
    out
}

/// 320x240 solid-blue RGB image.
pub fn sample_image() -> RgbImage {
    RgbImage::from_pixel(320, 240, image::Rgb([0, 0, 200]))
}
