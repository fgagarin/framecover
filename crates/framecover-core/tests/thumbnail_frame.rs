//! Android galleries take a video's thumbnail from the largest of its first
//! 20 sync samples, not from the cover art: the inserted frame is padded to
//! be that sample, and the video still decodes cleanly.

mod support;

use std::path::{Path, PathBuf};

use framecover_core::ffmpeg;
use framecover_core::loader::{FfprobeVideoLoader, VideoLoader};
use framecover_core::thumbnail_writer::{FormatDispatchThumbnailWriter, ThumbnailWriter};
use image::RgbImage;

/// Three seconds of 320x240 `testsrc2` with a keyframe every 10 frames.
fn fixture(dir: &Path, name: &str, video: &[&str]) -> PathBuf {
    let out = dir.join(name);
    let mut args = vec![
        "-y",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x240:rate=25:duration=3",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=3",
    ];
    args.extend_from_slice(video);
    args.extend(["-c:a", "aac", out.to_str().unwrap()]);
    ffmpeg::ffmpeg(&args).expect("generate fixture");
    out
}

/// Sizes of the video's keyframe packets, in decoding order.
fn keyframe_sizes(path: &Path) -> Vec<u64> {
    let out = support::ffprobe()
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=size,flags",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains(",K"))
        .map(|l| l.split(',').next().unwrap().parse().unwrap())
        .collect()
}

fn save(path: &Path, image: &RgbImage) {
    let video = FfprobeVideoLoader.load(path).unwrap();
    let result = FormatDispatchThumbnailWriter.write(&video, image, None, None);
    assert!(result.success, "{:?}", result.error_message);
}

fn assert_thumbnail_frame(path: &Path, name: &str, colour: [f64; 3]) {
    let sizes = keyframe_sizes(path);
    let largest_other = sizes[1..sizes.len().min(20)].iter().max().unwrap();
    assert!(sizes[0] > *largest_other, "{name}: {sizes:?}");
    // The filler is invisible: the video decodes without errors and the
    // first frame is the chosen picture.
    let decode = ffmpeg::ffmpeg([
        "-v",
        "error",
        "-i",
        path.to_str().unwrap(),
        "-f",
        "null",
        "-",
    ])
    .unwrap();
    assert!(
        decode.stderr.is_empty(),
        "{name}: {}",
        String::from_utf8_lossy(&decode.stderr)
    );
    let mean = support::mean(&support::first_playable_frame(path));
    for (got, want) in mean.iter().zip(colour) {
        assert!((got - want).abs() < 12.0, "{name}: {mean:?}");
    }
}

#[test]
fn inserted_frame_is_the_largest_keyframe() {
    let dir = tempfile::tempdir().unwrap();
    let red = RgbImage::from_pixel(320, 240, image::Rgb([230, 20, 20]));
    let green = RgbImage::from_pixel(320, 240, image::Rgb([20, 200, 20]));
    let mut cases = vec![
        (
            "h264.mp4",
            vec!["-c:v", "libx264", "-g", "10", "-pix_fmt", "yuv420p"],
        ),
        // No fast path for 4:2:2: the whole video is re-encoded.
        (
            "h264_422.mp4",
            vec!["-c:v", "libx264", "-g", "10", "-pix_fmt", "yuv422p"],
        ),
    ];
    if support::has_encoder("libx265") {
        cases.push((
            "hevc.mp4",
            vec![
                "-c:v",
                "libx265",
                "-x265-params",
                "log-level=error:keyint=10",
                "-pix_fmt",
                "yuv420p",
                "-tag:v",
                "hvc1",
            ],
        ));
    }
    for (name, video) in cases {
        let path = fixture(dir.path(), name, &video);
        save(&path, &red);
        assert_thumbnail_frame(&path, name, [230.0, 20.0, 20.0]);
        // Replacing the frame pads the new one as well.
        save(&path, &green);
        assert_thumbnail_frame(&path, name, [20.0, 200.0, 20.0]);
    }
}
