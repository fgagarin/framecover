//! Portrait phone video: landscape frames plus a display rotation. Frames are
//! shown upright, and a save keeps the rotation, stores the inserted frame
//! like the source's frames and writes an upright cover.

mod support;

use std::path::{Path, PathBuf};

use framecover_core::ffmpeg;
use framecover_core::frame_stream::{FfmpegFrameStream, FrameSource};
use framecover_core::frames::{FfmpegFrameExtractor, FrameExtractor};
use framecover_core::loader::{FfprobeVideoLoader, VideoLoader};
use framecover_core::probe;
use framecover_core::thumbnail_writer::{FormatDispatchThumbnailWriter, ThumbnailWriter};
use image::{Rgb, RgbImage};

const RED: Rgb<u8> = Rgb([230, 20, 20]);
const BLUE: Rgb<u8> = Rgb([20, 20, 230]);

/// Two seconds of 320x240 `testsrc2` (+ audio) encoded with `video`, then
/// tagged with a display rotation of `ccw` degrees (ffprobe's `rotation`).
fn rotated(dir: &Path, name: &str, ccw: i32, video: &[&str]) -> PathBuf {
    let plain = dir.join(format!("plain-{name}"));
    let mut args = vec![
        "-y",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x240:rate=25:duration=2",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=2",
    ];
    args.extend_from_slice(video);
    args.extend(["-c:a", "aac", plain.to_str().unwrap()]);
    ffmpeg::ffmpeg(&args).expect("generate fixture");
    let out = dir.join(name);
    let rotation = ccw.to_string();
    ffmpeg::ffmpeg([
        "-y",
        "-v",
        "error",
        "-display_rotation",
        &rotation,
        "-i",
        plain.to_str().unwrap(),
        "-c",
        "copy",
        out.to_str().unwrap(),
    ])
    .expect("tag rotation");
    out
}

/// Upright 240x320 picture: red top half, blue bottom half.
fn marker() -> RgbImage {
    RgbImage::from_fn(240, 320, |_, y| if y < 160 { RED } else { BLUE })
}

/// A frame as players show it (ffmpeg's autorotation), decoded as PNG.
fn shown_frame(path: &Path, seconds: &str) -> RgbImage {
    let out = ffmpeg::ffmpeg([
        "-v",
        "error",
        "-ss",
        seconds,
        "-i",
        path.to_str().unwrap(),
        "-map",
        "0:V:0",
        "-frames:v",
        "1",
        "-c:v",
        "png",
        "-f",
        "image2pipe",
        "-",
    ])
    .unwrap();
    image::load_from_memory(&out.stdout).unwrap().into_rgb8()
}

fn mean_abs_diff(a: &RgbImage, b: &RgbImage) -> f64 {
    assert_eq!(a.dimensions(), b.dimensions());
    let sum: u64 = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    sum as f64 / a.as_raw().len() as f64
}

/// Mean colour of rows `rows` of `img`.
fn band_mean(img: &RgbImage, rows: std::ops::Range<u32>) -> [f64; 3] {
    let band = image::imageops::crop_imm(img, 0, rows.start, img.width(), rows.len() as u32);
    support::mean(&band.to_image())
}

fn rotation_of(path: &Path) -> u32 {
    probe::probe(path)
        .unwrap()
        .first_playable_video()
        .unwrap()
        .rotation()
}

#[test]
fn rotated_video_is_shown_and_saved_upright() {
    let dir = tempfile::tempdir().unwrap();
    let fast: &[&str] = &["-c:v", "libx264", "-pix_fmt", "yuv420p"];
    // 4:2:2 has no fast path: the whole video is re-encoded.
    let reencode: &[&str] = &["-c:v", "libx264", "-pix_fmt", "yuv422p"];
    // Phones write -90 (turn clockwise); +90 turns the other way.
    for (ccw, cw) in [(-90, 90), (90, 270)] {
        for (kind, video) in [("fast", fast), ("reencode", reencode)] {
            let name = format!("{kind}{ccw}.mp4");
            let source = rotated(dir.path(), &name, ccw, video);
            assert_eq!(rotation_of(&source), cw, "{name}");

            let video = FfprobeVideoLoader.load(&source).unwrap();
            assert_eq!((video.width, video.height), (240, 320), "{name}");
            let expected = shown_frame(&source, "1");
            let extracted = FfmpegFrameExtractor.extract(&video, 1000).unwrap();
            assert!(mean_abs_diff(&extracted, &expected) < 2.0, "{name}");
            let streamed = FfmpegFrameStream::new(video.clone())
                .frame_at(1000)
                .unwrap();
            assert_eq!(streamed, extracted, "{name}");

            let result = FormatDispatchThumbnailWriter.write(&video, &marker(), Some(1000), None);
            assert!(result.success, "{name}: {:?}", result.error_message);
            assert_eq!(rotation_of(&source), cw, "{name}: rotation lost");
            // The inserted frame plays upright: red on top, blue below.
            let first = shown_frame(&source, "0");
            assert_eq!(first.dimensions(), (240, 320), "{name}");
            let [r, _, b] = band_mean(&first, 20..140);
            assert!(r > 180.0 && b < 70.0, "{name}: top {r} {b}");
            let [r, _, b] = band_mean(&first, 180..300);
            assert!(b > 180.0 && r < 70.0, "{name}: bottom {r} {b}");
            // So does the rest of the video, and the cover is upright too.
            let saved = FfprobeVideoLoader.load(&source).unwrap();
            assert_eq!((saved.width, saved.height), (240, 320), "{name}");
            let cover = saved.existing_thumbnail.expect("cover");
            assert!(
                cover.height() > cover.width(),
                "{name}: cover {:?}",
                cover.dimensions()
            );
            let after = shown_frame(&source, "1.04");
            assert!(mean_abs_diff(&after, &expected) < 6.0, "{name}");
        }
    }
}
