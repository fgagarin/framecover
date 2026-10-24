//! Port of `tests/integration/test_video_conversion.py`: real conversion
//! followed by the thumbnail-editing workflow.

mod common;
mod support;

use framecover_core::converter::FfmpegVideoConverter;
use framecover_core::models::VideoFormat;
use framecover_core::thumbnail_writer::{FormatDispatchThumbnailWriter, ThumbnailWriter};

fn conversion_then_thumbnail(codec: &str, profile: Option<&str>, transcode: bool) {
    let dir = tempfile::tempdir().unwrap();
    let ffv1 = codec == "ffv1";
    let source = dir
        .path()
        .join(if ffv1 { "source.mkv" } else { "source.m2ts" });
    let target = dir.path().join("converted.mp4");
    let mut args = vec![
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=96x64:rate=25:duration=1",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=1",
        "-c:v",
        codec,
    ];
    if let Some(p) = profile {
        args.extend(["-profile:v", p]);
    }
    args.extend([
        "-c:a",
        if ffv1 { "pcm_s16le" } else { "aac" },
        "-pix_fmt",
        "yuv420p",
        "-f",
        if ffv1 { "matroska" } else { "mpegts" },
        source.to_str().unwrap(),
    ]);
    support::run_ffmpeg(&args);
    let original = std::fs::read(&source).unwrap();

    let mut stages = Vec::new();
    let mut cb = |s: &str| stages.push(s.to_owned());
    let out = FfmpegVideoConverter
        .convert(&source, &target, false, Some(&mut cb))
        .unwrap();
    assert_eq!(
        std::fs::canonicalize(&out).unwrap(),
        std::fs::canonicalize(&target).unwrap()
    );
    assert!(!out.to_string_lossy().starts_with(r"\\?\"));
    assert_eq!(std::fs::read(&source).unwrap(), original);
    assert_eq!(stages.len(), if transcode { 2 } else { 1 }, "{stages:?}");
    assert_eq!(support::codecs(&target, "video"), ["h264"]);
    assert_eq!(support::codecs(&target, "audio"), ["aac"]);

    let mut video = support::load(&target);
    assert_eq!(video.format, VideoFormat::Mp4);
    assert!(
        video.duration_ms.abs_diff(1000) <= 100,
        "{}",
        video.duration_ms
    );
    assert_eq!((video.width, video.height), (96, 64));
    let frame = support::extract_frame(&target, 400);
    let writer = FormatDispatchThumbnailWriter;
    assert_eq!(writer.reencode_reason(&video), None);
    let original_packets = support::video_packets(&target);
    let original_frames = support::video_frames(&target);
    for position in [400, 600] {
        let mut save_stages = Vec::new();
        let mut cb = |n: &str, e: Option<f64>| {
            if e.is_none() {
                save_stages.push(n.to_owned())
            }
        };
        let result = writer.write(&video, &frame, Some(position), Some(&mut cb));
        assert!(result.success, "{:?}", result.error_message);
        assert!(save_stages.iter().any(|s| s == "Encoding preview frame"));
        assert!(save_stages.iter().any(|s| s == "Copying video streams"));
        assert!(!save_stages.iter().any(|s| s == "Encoding video"));
        let packets = support::video_packets(&target);
        // The concat demuxer can insert parameter sets into the first source IDR.
        assert_eq!(packets[2..], original_packets[1..]);
        let frames = support::video_frames(&target);
        assert_eq!(frames.len(), original_frames.len() + 1);
        assert_eq!(frames[1..], original_frames[..]);
        video = support::load(&target);
        assert!(video.existing_thumbnail.is_some());
    }
    assert_eq!(std::fs::read(&source).unwrap(), original);
}

#[test]
fn h264_high() {
    conversion_then_thumbnail("libx264", Some("high"), false);
}

#[test]
fn h264_baseline() {
    conversion_then_thumbnail("libx264", Some("baseline"), false);
}

#[test]
fn h264_main() {
    conversion_then_thumbnail("libx264", Some("main"), false);
}

#[test]
fn mpeg2() {
    conversion_then_thumbnail("mpeg2video", None, true);
}

#[test]
fn ffv1() {
    conversion_then_thumbnail("ffv1", None, true);
}

#[test]
fn conversion_preserves_optional_audio_tracks() {
    for audio_tracks in [0usize, 2] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.m2ts");
        let target = dir.path().join("converted.mp4");
        let mut args: Vec<String> = [
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=96x64:rate=25:duration=1",
        ]
        .map(String::from)
        .into();
        let mut maps: Vec<String> = vec!["-map".into(), "0:v:0".into()];
        for index in 0..audio_tracks {
            args.extend(["-f", "lavfi", "-i"].map(String::from));
            args.push(format!("sine=frequency={}:duration=1", 440 + index * 100));
            maps.extend(["-map".into(), format!("{}:a:0", index + 1)]);
        }
        args.extend(maps);
        args.extend(
            [
                "-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p", "-f", "mpegts",
            ]
            .map(String::from),
        );
        args.push(source.to_str().unwrap().into());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        support::run_ffmpeg(&refs);
        FfmpegVideoConverter
            .convert(&source, &target, false, None)
            .unwrap();
        assert_eq!(support::codecs(&target, "video").len(), 1);
        assert_eq!(support::codecs(&target, "audio").len(), audio_tracks);
    }
}

#[test]
fn audio_only_input_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("audio.m4a");
    support::run_ffmpeg(&[
        "-y",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=1",
        "-c:a",
        "aac",
        source.to_str().unwrap(),
    ]);
    let err = FfmpegVideoConverter
        .convert(&source, &dir.path().join("out.mp4"), false, None)
        .unwrap_err();
    assert!(
        matches!(err, framecover_core::Error::NoVideoStream(_)),
        "{err}"
    );
}
