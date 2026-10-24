//! Integration tests for `FfmpegFrameStream`: every frame it returns must be
//! pixel-identical to what `FfmpegFrameExtractor` returns for the same
//! position, whatever the request order.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use framecover_core::VideoFile;
use framecover_core::error::Error;
use framecover_core::ffmpeg;
use framecover_core::frame_stream::{FfmpegFrameStream, FrameSource};
use framecover_core::frames::{FfmpegFrameExtractor, FrameExtractor};
use framecover_core::loader::{FfprobeVideoLoader, VideoLoader};

/// Run ffmpeg with `-y` and the given arguments, writing `out`.
fn encode(out: &Path, args: &[&str]) -> PathBuf {
    let mut all = vec!["-y", "-v", "error"];
    all.extend_from_slice(args);
    all.push(out.to_str().unwrap());
    ffmpeg::ffmpeg(&all).expect("generate fixture");
    out.to_owned()
}

/// Request `positions` in order from one stream and compare each frame
/// with a one-off extraction.
fn assert_matches_extractor(video: &VideoFile, positions: &[u64]) -> FfmpegFrameStream {
    let mut stream = FfmpegFrameStream::new(video.clone());
    for &ms in positions {
        let streamed = stream
            .frame_at(ms)
            .unwrap_or_else(|e| panic!("stream @ {ms}: {e}"));
        let extracted = FfmpegFrameExtractor
            .extract(video, ms)
            .unwrap_or_else(|e| panic!("extractor @ {ms}: {e}"));
        assert!(
            streamed == extracted,
            "frame mismatch at {ms} ms in {}",
            video.path.display()
        );
    }
    assert_eq!(stream.fallbacks(), 0, "stream fell back to the extractor");
    stream
}

/// Forward steps, backward steps, jumps and the end of the video.
fn scrub_positions(video: &VideoFile) -> Vec<u64> {
    let step = video.frame_step_ms;
    let end = video.duration_ms;
    let mut positions = vec![0, 1, step / 2, step / 2 + 1, step, 1000];
    positions.extend((1..=8).map(|i| 1000 + i * step));
    positions.extend((1..=8).map(|i| 3000 - i * step));
    positions.extend([end / 2, 4000, 400, 401, 399, end, end - 1, end - step, 0]);
    positions.extend((1..=4).map(|i| end - i * step));
    positions.retain(|&p| p <= end);
    positions
}

fn load(path: &Path) -> VideoFile {
    FfprobeVideoLoader.load(path).unwrap()
}

#[test]
fn matches_extractor_with_b_frames() {
    let dir = tempfile::tempdir().unwrap();
    let video = load(&common::sample_video(dir.path()));
    let stream = assert_matches_extractor(&video, &scrub_positions(&video));
    // Steps and cached windows do not need a session per request.
    assert!(
        stream.sessions_started() < 15,
        "{}",
        stream.sessions_started()
    );
}

#[test]
fn matches_extractor_without_b_frames() {
    let dir = tempfile::tempdir().unwrap();
    let path = encode(
        &dir.path().join("intra.mp4"),
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=25:duration=5",
            "-c:v",
            "libx264",
            "-bf",
            "0",
            "-g",
            "10",
            "-pix_fmt",
            "yuv420p",
        ],
    );
    let video = load(&path);
    assert_matches_extractor(&video, &scrub_positions(&video));
}

/// 29.97 fps in a 1/1000 (Matroska) and a 1/30000 (MP4) time base, with
/// positions whose seek point falls exactly on, or one ms around, frame
/// timestamps.
#[test]
fn matches_extractor_at_ntsc_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    for ext in ["mp4", "mkv"] {
        let path = encode(
            &dir.path().join(format!("ntsc.{ext}")),
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=160x120:rate=30000/1001:duration=3",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ],
        );
        let video = load(&path);
        assert_eq!(video.frame_step_ms, 33);
        // Seek point = t - 16.5 ms; frame k is at k * 1001 / 30 ms.
        let mut positions = Vec::new();
        for k in [1u64, 2, 15, 30, 45, 61] {
            let t = (k * 1001 * 1000 / 30 + 16_500) / 1000;
            positions.extend([t - 1, t, t + 1]);
        }
        positions.extend([517, 516, 518, 1500, 1000]);
        assert_matches_extractor(&video, &positions);
    }
}

#[test]
fn matches_extractor_for_variable_frame_rate() {
    let dir = tempfile::tempdir().unwrap();
    let path = encode(
        &dir.path().join("vfr.mp4"),
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=25:duration=4",
            "-vf",
            "select='not(mod(n\\,3))+not(mod(n\\,5))'",
            "-fps_mode",
            "vfr",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ],
    );
    let video = load(&path);
    let positions: Vec<u64> = (0..60)
        .map(|i| i * 17)
        .chain((0..30).map(|i| 2500 - i * 23))
        .filter(|&p| p <= video.duration_ms)
        .collect();
    assert_matches_extractor(&video, &positions);
}

/// A container start time that is not zero shifts every timestamp.
#[test]
fn matches_extractor_with_start_offset() {
    let dir = tempfile::tempdir().unwrap();
    for ext in ["mkv", "mp4"] {
        let path = encode(
            &dir.path().join(format!("offset.{ext}")),
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x240:rate=25:duration=4",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-output_ts_offset",
                "0.3337",
            ],
        );
        let video = load(&path);
        assert_matches_extractor(&video, &scrub_positions(&video));
    }
}

#[test]
fn matches_extractor_for_vp9_webm() {
    let dir = tempfile::tempdir().unwrap();
    let video = load(&common::video_for_format(dir.path(), "webm"));
    assert_matches_extractor(&video, &scrub_positions(&video));
}

#[test]
fn backward_steps_hit_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let video = load(&common::sample_video(dir.path()));
    let mut stream = FfmpegFrameStream::new(video.clone());
    stream.frame_at(3000).unwrap();
    for i in 1..=20 {
        stream.frame_at(3000 - i * video.frame_step_ms).unwrap();
    }
    // One exact seek, then one window covering the next steps.
    assert_eq!(stream.sessions_started(), 2);
    for i in 1..=50 {
        stream.frame_at(3000 + i * video.frame_step_ms).unwrap();
    }
    // Forward steps read on from a running session.
    assert!(
        stream.sessions_started() <= 3,
        "{}",
        stream.sessions_started()
    );
}

#[test]
fn end_of_video_returns_last_frame() {
    let dir = tempfile::tempdir().unwrap();
    let video = load(&common::sample_video(dir.path()));
    // Jump straight to the end first: no session has seen the end yet.
    let stream = assert_matches_extractor(
        &video,
        &[video.duration_ms, video.duration_ms - 1, 4990, 4500],
    );
    assert_eq!(stream.fallbacks(), 0);
}

#[test]
fn beyond_duration_is_invalid_input() {
    let dir = tempfile::tempdir().unwrap();
    let video = load(&common::sample_video(dir.path()));
    let mut stream = FfmpegFrameStream::new(video.clone());
    let err = stream.frame_at(video.duration_ms + 1).unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)), "{err:?}");
}

#[test]
fn small_cache_still_matches() {
    let dir = tempfile::tempdir().unwrap();
    let video = load(&common::sample_video(dir.path()));
    // Room for three 320x240 frames only.
    let mut stream = FfmpegFrameStream::with_cache_budget(video.clone(), 3 * 320 * 240 * 3);
    for ms in scrub_positions(&video) {
        let extracted = FfmpegFrameExtractor.extract(&video, ms).unwrap();
        assert!(stream.frame_at(ms).unwrap() == extracted, "{ms}");
    }
    assert_eq!(stream.fallbacks(), 0);
}

#[test]
fn missing_file_is_extraction_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::sample_video(dir.path());
    let video = load(&path);
    std::fs::remove_file(&path).unwrap();
    let err = FfmpegFrameStream::new(video).frame_at(1000).unwrap_err();
    assert!(matches!(err, Error::Extraction { .. }), "{err:?}");
}

#[test]
fn ignores_attached_picture_stream() {
    let dir = tempfile::tempdir().unwrap();
    let video = load(&common::video_with_thumb(dir.path()));
    assert_matches_extractor(&video, &[0, 2000, 2040, 1960, video.duration_ms]);
}

// ---------------------------------------------------------------------------
// Performance (ignored by default)
// ---------------------------------------------------------------------------

fn frames_per_second(count: usize, elapsed: Duration) -> f64 {
    count as f64 / elapsed.as_secs_f64()
}

fn time(mut f: impl FnMut(u64), positions: &[u64]) -> f64 {
    let started = Instant::now();
    for &ms in positions {
        f(ms);
    }
    frames_per_second(positions.len(), started.elapsed())
}

/// Deterministic pseudo-random positions in `[0, end]`.
fn random_positions(count: usize, end: u64) -> Vec<u64> {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % (end + 1)
        })
        .collect()
}

/// Prints frames per second for sequential forward steps, backward steps
/// and random jumps, stream versus one-off extraction.
#[test]
#[ignore = "slow: generates 720p, 1080p and 4K fixtures; run with --ignored --nocapture"]
fn scrubbing_throughput() {
    let dir = tempfile::tempdir().unwrap();
    for (label, w, h) in [
        ("720p", 1280, 720),
        ("1080p", 1920, 1080),
        ("4k", 3840, 2160),
    ] {
        let src = format!("testsrc2=size={w}x{h}:rate=25:duration=12");
        let path = encode(
            &dir.path().join(format!("{label}.mp4")),
            &[
                "-f",
                "lavfi",
                "-i",
                &src,
                "-vf",
                "noise=alls=6:allf=t",
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-g",
                "50",
                "-pix_fmt",
                "yuv420p",
            ],
        );
        let video = load(&path);
        let step = video.frame_step_ms;
        let forward: Vec<u64> = (0..100).map(|i| 3000 + i * step).collect();
        let backward: Vec<u64> = (0..50).map(|i| 9000 - i * step).collect();
        let jumps = random_positions(40, video.duration_ms);

        let mut stream = FfmpegFrameStream::new(video.clone());
        let fwd = time(|ms| drop(stream.frame_at(ms).unwrap()), &forward);
        let mut stream = FfmpegFrameStream::new(video.clone());
        let back = time(|ms| drop(stream.frame_at(ms).unwrap()), &backward);
        let mut stream = FfmpegFrameStream::new(video.clone());
        let jump = time(|ms| drop(stream.frame_at(ms).unwrap()), &jumps);

        let extract = |ms| drop(FfmpegFrameExtractor.extract(&video, ms).unwrap());
        let x_fwd = time(extract, &forward[..20]);
        let x_back = time(extract, &backward[..20]);
        let x_jump = time(extract, &jumps);
        eprintln!(
            "{label}: forward {fwd:.1} fps (extractor {x_fwd:.1}), \
             backward {back:.1} fps (extractor {x_back:.1}), \
             jumps {jump:.1} fps (extractor {x_jump:.1})"
        );
    }
}
