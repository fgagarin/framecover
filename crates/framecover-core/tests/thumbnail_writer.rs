//! Port of `tests/unit/test_thumbnail_writer.py` against real ffmpeg.
//! `support::load` stands in for the loader (developed separately).

mod common;
mod support;

use std::path::Path;

use framecover_core::models::{ApplyError, VideoFile, VideoFormat};
use framecover_core::thumbnail_writer::{
    FormatDispatchThumbnailWriter, ThumbnailWriter, concat_ineligibility_reason,
};
use image::RgbImage;

fn writer() -> FormatDispatchThumbnailWriter {
    FormatDispatchThumbnailWriter
}

fn write_collecting(
    video: &VideoFile,
    img: &RgbImage,
    position_ms: Option<u64>,
) -> (framecover_core::ApplyResult, Vec<(String, Option<f64>)>) {
    let mut events = Vec::new();
    let mut cb = |name: &str, elapsed: Option<f64>| events.push((name.to_owned(), elapsed));
    let result = writer().write(video, img, position_ms, Some(&mut cb));
    (result, events)
}

fn started(events: &[(String, Option<f64>)]) -> Vec<String> {
    events
        .iter()
        .filter(|(_, e)| e.is_none())
        .map(|(n, _)| n.clone())
        .collect()
}

fn red() -> RgbImage {
    RgbImage::from_pixel(320, 240, image::Rgb([240, 20, 20]))
}

fn green() -> RgbImage {
    RgbImage::from_pixel(320, 240, image::Rgb([20, 240, 20]))
}

#[test]
fn reencode_reason_uses_concat_eligibility() {
    let dir = tempfile::tempdir().unwrap();
    let video = support::load(&common::sample_video(dir.path()));
    assert_eq!(writer().reencode_reason(&video), None);
    assert_eq!(concat_ineligibility_reason(&video.path).unwrap(), None);

    // A VP9 stream in an MP4 (PyAV was mocked in Python; use a real file).
    let vp9 = dir.path().join("vp9.mp4");
    common::generate_video(
        &vp9,
        1,
        320,
        240,
        &["-c:v", "libvpx-vp9", "-pix_fmt", "yuv420p", "-c:a", "aac"],
    );
    let reason = writer().reencode_reason(&support::load(&vp9)).unwrap();
    assert!(reason.contains("VP9"), "{reason}");
    assert!(reason.contains("H.264"), "{reason}");
}

#[test]
fn reencode_reason_for_non_mp4() {
    let dir = tempfile::tempdir().unwrap();
    let mov = support::load(&common::video_for_format(dir.path(), "mov"));
    assert_eq!(
        writer().reencode_reason(&mov).as_deref(),
        Some("The fast copy path supports .mp4 files only.")
    );
    let mkv = support::load(&common::video_for_format(dir.path(), "mkv"));
    assert_eq!(writer().reencode_reason(&mkv), None);
}

#[test]
fn av1_mp4_prepends_frame_without_reencoding_source() {
    if !support::has_encoder("libaom-av1") {
        eprintln!("libaom-av1 unavailable; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample_av1.mp4");
    support::run_ffmpeg(&[
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x240:rate=25:duration=3",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=3",
        "-c:v",
        "libaom-av1",
        "-cpu-used",
        "8",
        "-row-mt",
        "1",
        "-threads",
        "4",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-video_track_timescale",
        "90000",
        path.to_str().unwrap(),
    ]);
    let video = support::load(&path);
    assert_eq!(writer().reencode_reason(&video), None);
    let source_first_mean = support::mean(&support::first_playable_frame(&path));
    let source_packets = support::video_packets(&path);

    let (result, events) = write_collecting(&video, &red(), Some(1000));
    assert!(result.success, "{:?}", result.error_message);
    assert_eq!(result.duration_shift_ms, 40);
    let stages = started(&events);
    assert!(stages.iter().any(|s| s == "Copying video streams"));
    assert!(!stages.iter().any(|s| s == "Encoding video"));
    assert_eq!(support::codecs(&path, "video")[0], "av1");
    let output_packets = support::video_packets(&path);
    assert_eq!(output_packets[1..], source_packets[..]);
    assert_eq!(support::frame_count(&path), 76);
    assert_eq!(support::codecs(&path, "audio"), ["aac"]);
    let frames_after = support::video_frames(&path);
    assert_eq!(frames_after.len(), 76);
    let _ = source_first_mean; // decoded-frame equality is checked via packets
    let mean = support::mean(&support::first_playable_frame(&path));
    assert!(mean[0] > mean[1] + 80.0 && mean[0] > mean[2] + 80.0);
    let reopened = support::load(&path);
    assert_eq!(
        reopened.duration_ms,
        video.duration_ms + result.duration_shift_ms
    );
    assert_eq!(reopened.frame_step_ms, video.frame_step_ms);
    assert_eq!(reopened.thumbnail_position_ms, Some(1040));
    assert_eq!(
        reopened.thumbnail_frame_number,
        Some((1040f64 / video.frame_step_ms as f64).round_ties_even() as u64)
    );
}

#[test]
fn write_mp4_success() {
    let dir = tempfile::tempdir().unwrap();
    let video = support::load(&common::sample_video(dir.path()));
    let result = writer().write(&video, &common::sample_image(), None, None);
    assert!(result.success, "{:?}", result.error_message);
    // No temp files left behind.
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["sample.mp4"]);
}

#[test]
fn write_reports_timed_stages() {
    let dir = tempfile::tempdir().unwrap();
    let video = support::load(&common::sample_video(dir.path()));
    let (result, events) = write_collecting(&video, &common::sample_image(), None);
    assert!(result.success);
    let started = started(&events);
    let completed: Vec<String> = events
        .iter()
        .filter(|(_, e)| e.is_some())
        .map(|(n, _)| n.clone())
        .collect();
    assert_eq!(started, completed);
    assert_eq!(
        started,
        [
            "Preparing thumbnail",
            "Inspecting video streams",
            "Preparing temporary files",
            "Encoding preview frame",
            "Checking encoded frame",
            "Copying video streams",
            "Writing cover art",
            "Writing frame metadata",
            "Cleaning up temporary files",
        ]
    );
    assert!(events.iter().all(|(_, e)| e.is_none_or(|e| e >= 0.0)));
}

#[test]
fn write_mp4_adds_covr_tag() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::sample_video(dir.path());
    let result = writer().write(&support::load(&path), &common::sample_image(), None, None);
    assert!(result.success);
    let tag = mp4ameta::Tag::read_from_path(&path).unwrap();
    assert_eq!(tag.artworks().count(), 1);
}

#[test]
fn write_mp4_saves_selected_frame_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::sample_video(dir.path());
    let video = support::load(&path);
    let result = writer().write(&video, &common::sample_image(), Some(1680), None);
    assert!(result.success);
    let reopened = support::load(&path);
    let expected = 1680 + result.duration_shift_ms;
    assert_eq!(reopened.thumbnail_position_ms, Some(expected));
    assert_eq!(
        reopened.thumbnail_frame_number,
        Some((expected as f64 / video.frame_step_ms as f64).round_ties_even() as u64)
    );
}

#[test]
fn replacing_mp4_preview_does_not_reencode_or_extend_video() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::sample_video(dir.path());
    let first = writer().write(&support::load(&path), &red(), Some(400), None);
    assert!(first.success, "{:?}", first.error_message);

    let before = support::load(&path);
    assert!(before.has_inserted_preview());
    let frames_before = support::frame_count(&path);

    let (second, events) = write_collecting(&before, &green(), Some(800));
    assert!(second.success, "{:?}", second.error_message);
    assert_eq!(second.duration_shift_ms, 0);
    let stages = started(&events);
    assert!(
        stages
            .iter()
            .any(|s| s == "Removing previous preview frame")
    );
    assert!(stages.iter().any(|s| s == "Copying video streams"));
    assert!(!stages.iter().any(|s| s == "Encoding video"));
    assert_eq!(support::frame_count(&path), frames_before);
    let mean = support::mean(&support::first_playable_frame(&path));
    assert!(mean[1] > mean[0] + 80.0 && mean[1] > mean[2] + 80.0);
    assert_eq!(support::load(&path).thumbnail_position_ms, Some(800));
}

#[test]
fn write_mp4_sets_selected_frame_as_first_video_frame() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::sample_video(dir.path());
    let video = support::load(&path);
    let source_packets = support::video_packets(&path);
    let source_audio = support::audio_packets(&path);

    let result = writer().write(&video, &red(), None, None);
    assert!(result.success);

    let mean = support::mean(&support::first_playable_frame(&path));
    assert!(mean[0] > 150.0);
    assert!(mean[0] > mean[1] + 80.0 && mean[0] > mean[2] + 80.0);
    assert!(support::frame_count(&path) >= 100);
    let output_packets = support::video_packets(&path);
    // The concat demuxer can insert parameter sets into the first source IDR.
    assert_eq!(output_packets[2..], source_packets[1..]);
    assert_eq!(support::audio_packets(&path), source_audio);
}

fn make_read_only(path: &Path, read_only: bool) {
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_readonly(read_only);
    std::fs::set_permissions(path, perms).unwrap();
}

#[test]
fn write_read_only_returns_error_and_leaves_file_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("readonly.mp4");
    common::generate_video(&path, 5, 320, 240, common::H264_AAC);
    make_read_only(&path, true);
    let original = std::fs::read(&path).unwrap();
    let video = support::load(&path);
    let result = writer().write(&video, &common::sample_image(), None, None);
    make_read_only(&path, false);
    assert!(!result.success);
    assert_eq!(result.error_code, Some(ApplyError::FileNotWritable));
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn write_unsupported_format_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let video = VideoFile {
        path: dir.path().join("placeholder.rm"),
        format: VideoFormat::Unsupported,
        duration_ms: 3000,
        width: 320,
        height: 240,
        existing_thumbnail: None,
        is_writable: true,
        frame_step_ms: 40,
        thumbnail_frame_number: None,
        thumbnail_position_ms: None,
    };
    let result = writer().write(&video, &common::sample_image(), None, None);
    assert!(!result.success);
    assert_eq!(result.error_code, Some(ApplyError::UnsupportedFormat));
}

#[test]
fn write_thumbnail_is_scaled() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::sample_video(dir.path());
    let large = RgbImage::from_pixel(3840, 2160, image::Rgb([255, 0, 0]));
    let result = writer().write(&support::load(&path), &large, None, None);
    assert!(result.success);
    let cover = support::attached_pic_bytes(&path).expect("attached_pic stream");
    let cover = framecover_core::imaging::decode_rgb(&cover).unwrap();
    assert!(cover.width() <= 640 && cover.height() <= 360);
}

#[test]
fn existing_foreign_cover_forces_full_reencode() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::video_with_thumb(dir.path());
    let video = support::load(&path);
    assert!(video.existing_thumbnail.is_some());
    assert_eq!(
        writer().reencode_reason(&video).as_deref(),
        Some("An attached-picture video stream is already present.")
    );
    let (result, events) = write_collecting(&video, &red(), Some(2000));
    assert!(result.success, "{:?}", result.error_message);
    // The re-encode inserts the poster as an extra frame, like the fast path.
    assert_eq!(result.duration_shift_ms, 40);
    assert!(started(&events).iter().any(|s| s == "Encoding video"));
    let reopened = support::load(&path);
    assert_eq!(reopened.thumbnail_position_ms, Some(2040));
    assert_eq!(reopened.thumbnail_frame_number, Some(51));
}

#[test]
fn flv_returns_informational_success_without_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.flv");
    support::run_ffmpeg(&[
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x240:rate=25:duration=3",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=3",
        "-c:v",
        "libx264",
        "-c:a",
        "aac",
        "-pix_fmt",
        "yuv420p",
        "-ar",
        "44100",
        path.to_str().unwrap(),
    ]);
    let original = std::fs::read(&path).unwrap();
    let result = writer().write(&support::load(&path), &common::sample_image(), None, None);
    assert!(result.success);
    assert_eq!(
        result.error_message.as_deref(),
        Some(
            "FLV files can't hold cover art; the video is unchanged and only the file \
             manager's thumbnail is refreshed."
        )
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
}
