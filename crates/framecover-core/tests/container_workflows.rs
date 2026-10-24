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

/// `(codec_type, codec_name, filename tag)` of every stream.
fn stream_layout(path: &Path) -> Vec<(String, String, String)> {
    probe::probe(path)
        .unwrap()
        .streams
        .iter()
        .map(|s| {
            (
                s.codec_type.clone(),
                s.codec_name.clone().unwrap_or_default(),
                s.tag("filename").unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

fn layout(entries: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
    entries
        .iter()
        .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
        .collect()
}

#[test]
fn mkv_save_keeps_all_tracks_and_replaces_cover() {
    let dir = tempfile::tempdir().unwrap();
    let base = common::video_for_format(dir.path(), "mkv");
    let srt = dir.path().join("subs.srt");
    std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:02,000\nHello\n").unwrap();
    let font = dir.path().join("font.ttf");
    std::fs::write(&font, b"not really a font").unwrap();
    let path = dir.path().join("tracks.mkv");
    let s = |p: &Path| p.to_str().unwrap().to_owned();
    support::run_ffmpeg(&[
        "-y",
        "-i",
        &s(&base),
        "-i",
        &s(&srt),
        "-map",
        "0:v",
        "-map",
        "0:a",
        "-map",
        "0:a",
        "-map",
        "1",
        "-c",
        "copy",
        "-attach",
        &s(&font),
        "-metadata:s:t",
        "mimetype=application/x-truetype-font",
        "-metadata:s:t",
        "filename=font.ttf",
        &s(&path),
    ]);
    let expected_tracks = [
        ("video", "h264", ""),
        ("audio", "aac", ""),
        ("audio", "aac", ""),
        ("subtitle", "subrip", ""),
        ("attachment", "ttf", "font.ttf"),
    ];
    assert_eq!(stream_layout(&path), layout(&expected_tracks));

    let mut expected = expected_tracks.to_vec();
    expected.push(("video", "mjpeg", "cover.jpg"));
    // Saving twice replaces FrameCover's cover instead of adding another.
    for _ in 0..2 {
        let (success, message) = run_pipeline(&path);
        assert!(success, "{message:?}");
        assert_eq!(stream_layout(&path), layout(&expected));
        assert!(support::load(&path).existing_thumbnail.is_some());
    }
}

#[test]
fn avi_remux_keeps_all_audio_tracks() {
    let dir = tempfile::tempdir().unwrap();
    let base = common::video_for_format(dir.path(), "avi");
    let path = dir.path().join("two_audio.avi");
    let s = |p: &Path| p.to_str().unwrap().to_owned();
    support::run_ffmpeg(&[
        "-y",
        "-i",
        &s(&base),
        "-map",
        "0:v",
        "-map",
        "0:a",
        "-map",
        "0:a",
        "-c",
        "copy",
        &s(&path),
    ]);
    let (success, message) = run_pipeline(&path);
    assert!(success, "{message:?}");
    assert_eq!(support::codecs(&path, "audio").len(), 2);
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
