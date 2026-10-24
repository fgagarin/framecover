//! Port of `tests/integration/test_mkv_workflow.py` and `test_avi_workflow.py`.

mod common;
mod support;

use std::path::Path;
use std::time::Instant;

use framecover_core::probe;
use framecover_core::thumbnail_writer::{
    FormatDispatchThumbnailWriter, ThumbnailWriter, testing::with_simulated_ffmpeg_failure,
};

fn run_pipeline(path: &Path) -> (bool, Option<String>) {
    let video = support::load(path);
    let frame = support::extract_frame(path, 2500);
    let result = FormatDispatchThumbnailWriter.write(&video, &frame, None, None);
    (result.success, result.error_message)
}

fn has_cover_attachment(path: &Path) -> bool {
    probe::probe(path).unwrap().streams.iter().any(|s| {
        let name = s.tag("filename").unwrap_or("").to_lowercase();
        let named = matches!(name.as_str(), "cover.jpg" | "cover.png" | "cover.jpeg");
        named
            && ((s.is_video() && s.is_attached_pic())
                || matches!(s.codec_type.as_str(), "attachment" | "data"))
    })
}

fn dir_names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn mkv_full_pipeline_embeds_cover() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::video_for_format(dir.path(), "mkv");
    let original = support::load(&path);
    let t0 = Instant::now();
    let (success, message) = run_pipeline(&path);
    let elapsed = t0.elapsed().as_millis();
    assert!(success, "{message:?}");
    assert_eq!(message, None);
    assert!(has_cover_attachment(&path), "no cover attachment in MKV");
    assert!(elapsed < 5000, "{elapsed} ms");
    let updated = support::load(&path);
    assert!(updated.duration_ms.abs_diff(original.duration_ms) < 500);
    assert!(updated.existing_thumbnail.is_some());
    assert_eq!(dir_names(dir.path()), ["sample.mkv"]);
}

#[test]
fn webm_is_informational_noop() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::video_for_format(dir.path(), "webm");
    let original = std::fs::read(&path).unwrap();
    let before = support::load(&path);
    let t0 = Instant::now();
    let (success, message) = run_pipeline(&path);
    assert!(success);
    assert_eq!(
        message.as_deref(),
        Some(
            "WEBM does not support embedded cover art; Linux file manager icon updated via XDG cache"
        )
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(t0.elapsed().as_millis() < 5000);
    assert!(
        support::load(&path)
            .duration_ms
            .abs_diff(before.duration_ms)
            < 500
    );
}

#[test]
fn avi_pipeline_succeeds_with_note_and_keeps_duration() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::video_for_format(dir.path(), "avi");
    let original = support::load(&path);
    let (success, message) = run_pipeline(&path);
    assert!(success);
    assert_eq!(
        message.as_deref(),
        Some("AVI does not support embedded cover art; file manager will generate its own preview")
    );
    let updated = support::load(&path);
    assert!(updated.duration_ms.abs_diff(original.duration_ms) < 500);
    assert_eq!(dir_names(dir.path()), ["sample.avi"]);
}

#[test]
fn avi_atomicity_on_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::video_for_format(dir.path(), "avi");
    let original = std::fs::read(&path).unwrap();
    let video = support::load(&path);
    let frame = support::extract_frame(&path, 2500);
    let result = with_simulated_ffmpeg_failure(|| {
        FormatDispatchThumbnailWriter.write(&video, &frame, None, None)
    });
    assert!(!result.success);
    assert_eq!(
        result.error_code,
        Some(framecover_core::ApplyError::FfmpegError)
    );
    assert_eq!(
        result.error_message.as_deref(),
        Some("ffmpeg failed (exit 1). stderr: (none)")
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(dir_names(dir.path()), ["sample.avi"]);
}

#[test]
fn flv_pipeline_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::video_for_format(dir.path(), "flv");
    let original = std::fs::read(&path).unwrap();
    let (success, message) = run_pipeline(&path);
    assert!(success);
    assert!(message.unwrap().contains("does not support"));
    assert_eq!(std::fs::read(&path).unwrap(), original);
}
