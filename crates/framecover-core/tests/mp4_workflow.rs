//! Port of `tests/integration/test_mp4_workflow.py` and
//! `test_atomic_robustness.py`.

mod common;
mod support;

use std::path::Path;
use std::time::Instant;

use framecover_core::atomic_write::with_simulated_replace_failure;
use framecover_core::save_copy::save_copy;
use framecover_core::thumbnail_writer::{FormatDispatchThumbnailWriter, ThumbnailWriter};
use sha2::{Digest, Sha256};

fn sha(path: &Path) -> String {
    hex::encode(Sha256::digest(std::fs::read(path).unwrap()))
}

#[test]
fn copy_save_preserves_original_and_reuses_without_duration_growth() {
    let dir = tempfile::tempdir().unwrap();
    let sample = common::sample_video(dir.path());
    let original_digest = sha(&sample);
    let video = support::load(&sample);
    let destination = dir.path().join("thumbnail-copy.mp4");
    let frame = support::extract_frame(&sample, 1000);
    let writer = FormatDispatchThumbnailWriter;

    let first = save_copy(
        &writer,
        &video,
        &frame,
        &destination,
        false,
        Some(1000),
        None,
    );
    assert!(first.success, "{:?}", first.error_message);
    let destination = std::path::absolute(&destination).unwrap();
    assert_eq!(first.output_path.as_deref(), Some(destination.as_path()));
    let reopened = support::load(&destination);
    assert!(reopened.existing_thumbnail.is_some());
    assert_eq!(
        reopened.thumbnail_position_ms,
        Some(1000 + first.duration_shift_ms)
    );
    assert!(support::has_attached_pic_stream(&destination));

    let second_frame = support::extract_frame(&sample, 2000);
    let second = save_copy(
        &FormatDispatchThumbnailWriter,
        &support::load(&sample),
        &second_frame,
        &destination,
        false,
        Some(2000),
        None,
    );
    assert!(second.success, "{:?}", second.error_message);
    assert_eq!(second.output_path.as_deref(), Some(destination.as_path()));
    let repeated = support::load(&destination);
    assert!(repeated.existing_thumbnail.is_some());
    assert_eq!(
        repeated.thumbnail_position_ms,
        Some(2000 + second.duration_shift_ms)
    );
    assert_eq!(repeated.duration_ms, reopened.duration_ms);
    assert_eq!(sha(&sample), original_digest);
}

#[test]
fn apostrophe_in_path_keeps_concat_fast_path() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("it's here");
    std::fs::create_dir(&dir).unwrap();
    let target = common::sample_video(&dir);
    let video = support::load(&target);
    let frame = support::extract_frame(&target, 1000);
    let result = FormatDispatchThumbnailWriter.write(&video, &frame, Some(1000), None);
    assert!(result.success, "{:?}", result.error_message);
    // A non-zero shift proves the concat path ran instead of the re-encode fallback.
    assert!(result.duration_shift_ms > 0);
}

#[test]
fn mp4_full_pipeline_success() {
    let dir = tempfile::tempdir().unwrap();
    let target = common::sample_video(dir.path());
    let start = Instant::now();
    let video = support::load(&target);
    let frame = support::extract_frame(&target, 2500);
    let result = FormatDispatchThumbnailWriter.write(&video, &frame, None, None);
    let elapsed = start.elapsed().as_millis();
    assert!(result.success, "{:?}", result.error_message);
    assert!(elapsed < 10_000, "Pipeline took {elapsed}ms");
    assert!(support::has_attached_pic_stream(&target));
}

#[test]
fn mov_full_pipeline_success() {
    let dir = tempfile::tempdir().unwrap();
    let mov = dir.path().join("sample.mov");
    let mut extra = common::H264_AAC.to_vec();
    extra.extend(["-ar", "44100"]);
    common::generate_video(&mov, 5, 320, 240, &extra);
    let start = Instant::now();
    let video = support::load(&mov);
    let frame = support::extract_frame(&mov, 2500);
    let mut stages = Vec::new();
    let mut cb = |n: &str, e: Option<f64>| {
        if e.is_none() {
            stages.push(n.to_owned())
        }
    };
    let result = FormatDispatchThumbnailWriter.write(&video, &frame, Some(2500), Some(&mut cb));
    let elapsed = start.elapsed().as_millis();
    assert!(result.success, "{:?}", result.error_message);
    assert!(elapsed < 10_000, "Pipeline took {elapsed}ms");
    assert!(stages.iter().any(|s| s == "Encoding video"));
    assert!(!stages.iter().any(|s| s == "Writing cover art"));
    assert!(support::has_attached_pic_stream(&mov));
    assert_eq!(support::load(&mov).thumbnail_position_ms, Some(2500));
}

#[test]
fn pipeline_under_3_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let target = common::sample_video(dir.path());
    let start = Instant::now();
    let video = support::load(&target);
    let frame = support::extract_frame(&target, 0);
    FormatDispatchThumbnailWriter.write(&video, &frame, None, None);
    let elapsed = start.elapsed().as_millis();
    assert!(elapsed < 3000, "Full pipeline took {elapsed}ms");
}

// --- test_atomic_robustness.py ---------------------------------------------

fn leftover_tmp(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".vt_"))
        .collect()
}

#[test]
fn failed_replace_returns_failure_and_leaves_original() {
    let dir = tempfile::tempdir().unwrap();
    let target = common::sample_video(dir.path());
    let original = std::fs::read(&target).unwrap();
    let video = support::load(&target);
    let frame = support::extract_frame(&target, 0);
    let result = with_simulated_replace_failure(|| {
        FormatDispatchThumbnailWriter.write(&video, &frame, None, None)
    });
    assert!(!result.success);
    assert_eq!(
        result.error_code,
        Some(framecover_core::ApplyError::Unexpected)
    );
    assert_eq!(
        result.error_message.as_deref(),
        Some("Unexpected OS error: simulated replace failure")
    );
    assert_eq!(std::fs::read(&target).unwrap(), original);
    // Python checks `.vt_*.tmp`; every writer temp is cleaned up too.
    assert_eq!(leftover_tmp(dir.path()), Vec::<String>::new());
}
