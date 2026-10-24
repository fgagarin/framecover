//! Integration tests for `FfmpegFrameExtractor` (port of
//! `test_frame_extractor.py`, the core parts of `test_preview_workflow.py`
//! and the 500 ms budget from `tests/benchmarks/test_frame_extraction_perf.py`).

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use framecover_core::error::Error;
use framecover_core::ffmpeg::{self, Tool};
use framecover_core::frames::{FfmpegFrameExtractor, FrameExtractor};
use framecover_core::imaging;
use framecover_core::loader::{FfprobeVideoLoader, VideoLoader};
use image::RgbImage;

/// Every frame of the first playable stream with its timestamp in seconds,
/// decoded sequentially from the start (the reference for "nearest frame").
fn all_frames(path: &Path, out_dir: &Path) -> Vec<(f64, RgbImage)> {
    let mut probe = ffmpeg::command(Tool::Ffprobe).unwrap();
    probe
        .args(["-v", "error", "-select_streams", "V:0", "-show_entries"])
        .args(["frame=best_effort_timestamp_time", "-of", "csv=p=0"])
        .arg(path);
    let out = ffmpeg::run(&mut probe).unwrap();
    let times: Vec<f64> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().trim_end_matches(',').parse().ok())
        .collect();

    let pattern = out_dir.join("f%05d.png");
    ffmpeg::ffmpeg([
        "-v",
        "error",
        "-noautorotate",
        "-i",
        path.to_str().unwrap(),
        "-map",
        "0:V:0",
        "-fps_mode",
        "passthrough",
        "-pix_fmt",
        "rgb24",
        pattern.to_str().unwrap(),
    ])
    .unwrap();
    let frames: Vec<(f64, RgbImage)> = times
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let file = out_dir.join(format!("f{:05}.png", i + 1));
            (
                *t,
                imaging::decode_rgb(&std::fs::read(file).unwrap()).unwrap(),
            )
        })
        .collect();
    assert_eq!(frames.len(), 125);
    frames
}

#[test]
fn seek_matches_sequential_decode() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::sample_video(dir.path());
    let frames_dir = dir.path().join("frames");
    std::fs::create_dir(&frames_dir).unwrap();
    let frames = all_frames(&path, &frames_dir);
    let video = FfprobeVideoLoader.load(&path).unwrap();

    for offset_ms in [0u64, 19, 20, 21, 40, 1234, 2500, 4960, 4979, 4999, 5000] {
        let target = offset_ms as f64 / 1000.0;
        // `min_by` keeps the first (earlier) frame on ties, like Python's `min`.
        let (_, expected) = frames
            .iter()
            .min_by(|a, b| (a.0 - target).abs().total_cmp(&(b.0 - target).abs()))
            .unwrap();
        let actual = FfmpegFrameExtractor.extract(&video, offset_ms).unwrap();
        assert!(actual == *expected, "frame mismatch at {offset_ms} ms");
    }
}

#[test]
fn adjacent_positions_return_different_frames() {
    let dir = tempfile::tempdir().unwrap();
    let video = FfprobeVideoLoader
        .load(&common::sample_video(dir.path()))
        .unwrap();
    let first = FfmpegFrameExtractor.extract(&video, 0).unwrap();
    let second = FfmpegFrameExtractor
        .extract(&video, video.frame_step_ms)
        .unwrap();
    assert!(first != second);
}

#[test]
fn extracts_at_zero_mid_and_end() {
    let dir = tempfile::tempdir().unwrap();
    let video = FfprobeVideoLoader
        .load(&common::sample_video(dir.path()))
        .unwrap();
    for offset in [0, video.duration_ms / 2, 3000, video.duration_ms] {
        let img = FfmpegFrameExtractor.extract(&video, offset).unwrap();
        assert_eq!(
            (img.width(), img.height()),
            (video.width, video.height),
            "{offset}"
        );
    }
}

#[test]
fn beyond_duration_is_invalid_input() {
    let dir = tempfile::tempdir().unwrap();
    let video = FfprobeVideoLoader
        .load(&common::sample_video(dir.path()))
        .unwrap();
    let err = FfmpegFrameExtractor
        .extract(&video, video.duration_ms + 1)
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)), "{err:?}");
}

#[test]
fn ignores_attached_picture_stream() {
    let dir = tempfile::tempdir().unwrap();
    let video = FfprobeVideoLoader
        .load(&common::video_with_thumb(dir.path()))
        .unwrap();
    assert!(video.existing_thumbnail.is_some());
    let frame = FfmpegFrameExtractor.extract(&video, 2000).unwrap();
    assert_eq!(frame.dimensions(), (320, 240));
    // testsrc is colourful, unlike the solid-blue cover.
    assert!(frame != *video.existing_thumbnail.as_ref().unwrap());
}

#[test]
fn extracts_from_every_container() {
    let dir = tempfile::tempdir().unwrap();
    for ext in ["mp4", "mov", "mkv", "webm", "avi", "flv"] {
        let video = FfprobeVideoLoader
            .load(&common::video_for_format(dir.path(), ext))
            .unwrap();
        for offset in [0, 2500, video.duration_ms] {
            let img = FfmpegFrameExtractor
                .extract(&video, offset)
                .unwrap_or_else(|e| panic!("{ext} @ {offset}: {e}"));
            assert_eq!(img.dimensions(), (320, 240), "{ext} @ {offset}");
        }
    }
}

#[test]
fn missing_file_is_extraction_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::sample_video(dir.path());
    let video = FfprobeVideoLoader.load(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let err = FfmpegFrameExtractor.extract(&video, 1000).unwrap_err();
    assert!(
        matches!(
            err,
            Error::Extraction {
                offset_ms: 1000,
                ..
            }
        ),
        "{err:?}"
    );
}

/// Worst of several extractions at 2.5 s must stay under the 500 ms budget
/// (Python's `tests/benchmarks/test_frame_extraction_perf.py`).
#[test]
#[ignore = "slow: generates 1080p and 4K fixtures; run with --ignored"]
fn extraction_within_500ms_budget() {
    let dir = tempfile::tempdir().unwrap();
    for (label, w, h) in [("sd", 320, 240), ("1080p", 1920, 1080), ("4k", 3840, 2160)] {
        let path = dir.path().join(format!("sample_{label}.mp4"));
        common::generate_video(&path, 5, w, h, common::H264_AAC);
        let video = FfprobeVideoLoader.load(&path).unwrap();
        let mut worst = Duration::ZERO;
        for _ in 0..5 {
            let started = Instant::now();
            let img = FfmpegFrameExtractor.extract(&video, 2500).unwrap();
            worst = worst.max(started.elapsed());
            assert_eq!(img.dimensions(), (w, h));
        }
        eprintln!("{label}: worst {worst:?}");
        assert!(
            worst < Duration::from_millis(500),
            "{label}: worst {worst:?} exceeds 500 ms"
        );
    }
}
