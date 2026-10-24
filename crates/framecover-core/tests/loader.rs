//! Integration tests for `FfprobeVideoLoader` (port of `test_video_loader.py`,
//! `test_format_detection.py` and the loader parts of `test_preview_workflow.py`).

mod common;

use std::path::{Path, PathBuf};

use framecover_core::error::Error;
use framecover_core::ffmpeg;
use framecover_core::loader::{FfprobeVideoLoader, VideoLoader};
use framecover_core::models::VideoFormat;
use framecover_core::mp4_tags::{FRAME_NUMBER_NAME, POSITION_MS_NAME, TAG_MEAN};
use mp4ameta::{Data, FreeformIdent, Tag};

fn load(path: &Path) -> framecover_core::Result<framecover_core::VideoFile> {
    FfprobeVideoLoader.load(path)
}

#[test]
fn loads_mp4_basic_properties() {
    let dir = tempfile::tempdir().unwrap();
    let vf = load(&common::sample_video(dir.path())).unwrap();
    assert_eq!(vf.format, VideoFormat::Mp4);
    assert_eq!(vf.duration_ms, 5000);
    assert_eq!((vf.width, vf.height), (320, 240));
    assert_eq!(vf.frame_step_ms, 40);
    assert!(vf.existing_thumbnail.is_none());
    assert!(vf.is_writable);
    assert!(vf.path.is_absolute());
    assert_eq!(vf.thumbnail_frame_number, None);
    assert_eq!(vf.thumbnail_position_ms, None);
    assert!(!vf.has_inserted_preview());
}

#[test]
fn loads_mp4_cover_art() {
    let dir = tempfile::tempdir().unwrap();
    let vf = load(&common::video_with_thumb(dir.path())).unwrap();
    let thumb = vf.existing_thumbnail.expect("cover art");
    assert_eq!(thumb.dimensions(), (320, 240));
    let px = thumb.get_pixel(160, 120).0;
    assert!(
        px[2] > 200 && px[0] < 40 && px[1] < 40,
        "expected blue, got {px:?}"
    );
    // The cover is not mistaken for the playable stream.
    assert_eq!((vf.width, vf.height, vf.duration_ms), (320, 240, 5000));
}

#[test]
fn loads_saved_thumbnail_source_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::sample_video(dir.path());
    let mut tag = Tag::read_from_path(&path).unwrap();
    tag.set_data(
        FreeformIdent::new_borrowed(TAG_MEAN, FRAME_NUMBER_NAME),
        Data::Utf8("42".into()),
    );
    tag.set_data(
        FreeformIdent::new_borrowed(TAG_MEAN, POSITION_MS_NAME),
        Data::Utf8("1680".into()),
    );
    tag.write_to_path(&path).unwrap();

    let vf = load(&path).unwrap();
    assert_eq!(vf.thumbnail_frame_number, Some(42));
    assert_eq!(vf.thumbnail_position_ms, Some(1680));
}

#[test]
fn read_only_file_is_not_writable() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::sample_video(dir.path());
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&path, perms.clone()).unwrap();

    let result = load(&path);

    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&path, perms).unwrap();
    assert!(!result.unwrap().is_writable);
}

#[test]
fn nonexistent_file_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    match load(&dir.path().join("ghost.mp4")) {
        Err(Error::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn text_file_fails_to_load() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("not_a_video.txt");
    std::fs::write(&bad, "hello").unwrap();
    assert!(load(&bad).is_err());
}

#[test]
fn audio_only_file_has_no_video_stream() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("audio.mp4");
    ffmpeg::ffmpeg([
        "-y",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=1",
        "-c:a",
        "aac",
        out.to_str().unwrap(),
    ])
    .unwrap();
    assert!(matches!(load(&out), Err(Error::NoVideoStream(_))));
}

#[test]
fn unsupported_container_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("clip.mpg");
    common::generate_video(&out, 1, 320, 240, &["-c:v", "mpeg2video", "-c:a", "mp2"]);
    assert!(matches!(load(&out), Err(Error::UnsupportedFormat(_))));
}

#[test]
fn detects_all_supported_formats() {
    let dir = tempfile::tempdir().unwrap();
    for (ext, expected) in [
        ("mp4", VideoFormat::Mp4),
        ("mov", VideoFormat::Mov),
        ("mkv", VideoFormat::Mkv),
        ("webm", VideoFormat::Webm),
        ("avi", VideoFormat::Avi),
        ("flv", VideoFormat::Flv),
    ] {
        let vf = load(&common::video_for_format(dir.path(), ext)).unwrap();
        assert_eq!(vf.format, expected, "{ext}");
        assert!(
            vf.duration_ms >= 4900 && vf.duration_ms <= 5100,
            "{ext}: {}",
            vf.duration_ms
        );
        assert_eq!((vf.width, vf.height), (320, 240), "{ext}");
        assert_eq!(vf.frame_step_ms, 40, "{ext}");
        assert!(vf.existing_thumbnail.is_none(), "{ext}");
    }
}

#[test]
fn mp4_renamed_to_unknown_extension_is_mp4() {
    let dir = tempfile::tempdir().unwrap();
    let mp4 = common::sample_video(dir.path());
    let renamed = dir.path().join("sample.xyz");
    std::fs::rename(&mp4, &renamed).unwrap();
    assert_eq!(load(&renamed).unwrap().format, VideoFormat::Mp4);
}

/// MKV/WebM with a PNG attachment. With an `image/*` mimetype ffmpeg's
/// matroska demuxer exposes it as an `attached_pic` video stream; with any
/// other mimetype it stays an `attachment` stream matched by file name.
fn mkv_with_attachment(dir: &Path, ext: &str, filename: &str, mimetype: &str) -> PathBuf {
    let base = common::video_for_format(dir, ext);
    let cover = dir.join("cover_src.png");
    ffmpeg::ffmpeg([
        "-y",
        "-f",
        "lavfi",
        "-i",
        "color=c=red:size=64x48:rate=1:duration=1",
        "-frames:v",
        "1",
        cover.to_str().unwrap(),
    ])
    .unwrap();
    let out = dir.join(format!("attached.{ext}"));
    let name_tag = format!("filename={filename}");
    let mime_tag = format!("mimetype={mimetype}");
    ffmpeg::ffmpeg([
        "-y",
        "-i",
        base.to_str().unwrap(),
        "-attach",
        cover.to_str().unwrap(),
        "-metadata:s:t",
        &mime_tag,
        "-metadata:s:t",
        &name_tag,
        "-map",
        "0",
        "-c",
        "copy",
        out.to_str().unwrap(),
    ])
    .unwrap();
    out
}

fn assert_red_cover(thumb: Option<image::RgbImage>) {
    let thumb = thumb.expect("cover attachment");
    assert_eq!(thumb.dimensions(), (64, 48));
    let [r, g, b] = thumb.get_pixel(10, 10).0;
    assert!(
        r > 240 && g < 16 && b < 16,
        "expected red, got {:?}",
        [r, g, b]
    );
}

#[test]
fn loads_mkv_image_attachment_as_attached_pic() {
    let dir = tempfile::tempdir().unwrap();
    let path = mkv_with_attachment(dir.path(), "mkv", "poster.png", "image/png");
    let vf = load(&path).unwrap();
    assert_eq!(vf.format, VideoFormat::Mkv);
    assert_red_cover(vf.existing_thumbnail);
}

#[test]
fn loads_named_cover_attachment() {
    let dir = tempfile::tempdir().unwrap();
    // (ffmpeg's WebM muxer drops attachments, so only MKV can be generated.)
    let path = mkv_with_attachment(dir.path(), "mkv", "Cover.PNG", "application/octet-stream");
    let vf = load(&path).unwrap();
    assert_eq!(vf.format, VideoFormat::Mkv);
    assert_red_cover(vf.existing_thumbnail);
}

#[test]
fn ignores_non_cover_named_attachment() {
    let dir = tempfile::tempdir().unwrap();
    let path = mkv_with_attachment(dir.path(), "mkv", "poster.png", "application/octet-stream");
    assert!(load(&path).unwrap().existing_thumbnail.is_none());
}

/// Fixture loads must match the Python reference in `tests/golden/`
/// (`<case>.json` -> `fixture.load`).
#[test]
fn fixture_loads_match_python_golden() {
    let golden_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let dir = tempfile::tempdir().unwrap();
    for case in ["mp4", "mov", "mkv", "avi", "flv", "webm", "mp4_with_thumb"] {
        let json = std::fs::read(golden_dir.join(format!("save_{case}.json"))).unwrap();
        let golden: serde_json::Value = serde_json::from_slice(&json).unwrap();
        let expected = &golden["fixture"]["load"];
        let sub = dir.path().join(case);
        std::fs::create_dir(&sub).unwrap();
        let path = match case {
            "mp4_with_thumb" => common::video_with_thumb(&sub),
            ext => common::video_for_format(&sub, ext),
        };
        let vf = load(&path).unwrap();
        let ctx = format!("save_{case}");
        assert_eq!(vf.format.name().to_lowercase(), expected["format"], "{ctx}");
        assert_eq!(vf.duration_ms, expected["duration_ms"], "{ctx}");
        assert_eq!(vf.width, expected["width"], "{ctx}");
        assert_eq!(vf.height, expected["height"], "{ctx}");
        assert_eq!(vf.frame_step_ms, expected["frame_step_ms"], "{ctx}");
        assert_eq!(vf.is_writable, expected["is_writable"], "{ctx}");
        assert_eq!(
            vf.existing_thumbnail.is_some(),
            expected["has_existing_thumbnail"],
            "{ctx}"
        );
        let size = vf
            .existing_thumbnail
            .as_ref()
            .map(|t| serde_json::json!([t.width(), t.height()]))
            .unwrap_or(serde_json::Value::Null);
        assert_eq!(size, expected["existing_thumbnail_size"], "{ctx}");
        assert_eq!(
            serde_json::json!(vf.thumbnail_frame_number),
            expected["thumbnail_frame_number"],
            "{ctx}"
        );
        assert_eq!(
            serde_json::json!(vf.thumbnail_position_ms),
            expected["thumbnail_position_ms"],
            "{ctx}"
        );
    }
}
