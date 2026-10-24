//! Inserted preview frames: the concat fast path must not disturb the
//! source's DTS (B-frame delay), and the full re-encode must insert the poster
//! as an extra frame too, so outputs keep the source frame rate and gain
//! exactly one frame.

mod common;
mod support;

use std::path::{Path, PathBuf};

use framecover_core::loader::{FfprobeVideoLoader, VideoLoader};
use framecover_core::thumbnail_writer::{FormatDispatchThumbnailWriter, ThumbnailWriter};
use framecover_core::{ffmpeg, imaging, mp4_tags};
use image::RgbImage;
use serde_json::Value;

#[derive(Debug, Clone)]
struct Packet {
    pts: i64,
    dts: i64,
    duration: i64,
    flags: String,
}

fn ffprobe_json(args: &[&str], path: &Path) -> Value {
    let out = support::ffprobe()
        .args(["-v", "error", "-of", "json"])
        .args(args)
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success(), "ffprobe failed on {}", path.display());
    serde_json::from_slice(&out.stdout).unwrap()
}

fn packets(path: &Path, stream: &str) -> Vec<Packet> {
    let v = ffprobe_json(
        &[
            "-select_streams",
            stream,
            "-show_entries",
            "packet=pts,dts,duration,flags",
        ],
        path,
    );
    v["packets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| Packet {
            pts: p["pts"].as_i64().unwrap(),
            dts: p["dts"].as_i64().unwrap(),
            duration: p["duration"].as_i64().unwrap_or(0),
            flags: p["flags"].as_str().unwrap_or("").to_owned(),
        })
        .collect()
}

fn video_stream(path: &Path) -> Value {
    let v = ffprobe_json(&["-select_streams", "v:0", "-show_streams"], path);
    v["streams"][0].clone()
}

fn red() -> RgbImage {
    RgbImage::from_pixel(320, 240, image::Rgb([240, 20, 20]))
}

fn save(path: &Path, position_ms: u64) -> framecover_core::ApplyResult {
    let video = FfprobeVideoLoader.load(path).unwrap();
    let result = FormatDispatchThumbnailWriter.write(&video, &red(), Some(position_ms), None);
    assert!(result.success, "{:?}", result.error_message);
    result
}

/// DTS strictly increasing in whole frames, every packet one frame long, and
/// the first packet a non-discarded keyframe presented at the start.
fn assert_regular(path: &Path, step: i64) {
    let pkts = packets(path, "v:0");
    for w in pkts.windows(2) {
        assert_eq!(
            w[1].dts - w[0].dts,
            step,
            "irregular DTS in {}: {:?} -> {:?}",
            path.display(),
            w[0],
            w[1]
        );
    }
    for p in &pkts {
        assert_eq!(p.duration, step, "packet duration {p:?}");
        assert!(p.dts <= p.pts, "DTS after PTS: {p:?}");
        assert!(!p.flags.contains('D'), "discarded packet {p:?}");
    }
    let first = &pkts[0];
    assert!(first.flags.contains('K'), "preview is not a keyframe");
    assert_eq!(first.pts, pkts.iter().map(|p| p.pts).min().unwrap());
}

/// The first decoded frame is the (red) preview, not the source.
fn assert_preview_first(path: &Path) {
    let frame = support::first_playable_frame(path);
    let [r, g, b] = support::mean(&frame);
    assert!(
        r > 200.0 && g < 60.0 && b < 60.0,
        "first frame mean {r} {g} {b}"
    );
}

/// Output video = preview + the source packets, each one frame later.
fn assert_shifted_by_one_frame(source: &Path, output: &Path, step: i64) {
    let (src, out) = (packets(source, "v:0"), packets(output, "v:0"));
    assert_eq!(out.len(), src.len() + 1);
    let src_start = src.iter().map(|p| p.pts).min().unwrap();
    let out_start = out[0].pts;
    for (s, o) in src.iter().zip(&out[1..]) {
        assert_eq!(o.pts - out_start, s.pts - src_start + step);
    }
    let (src_hashes, out_hashes) = (
        support::video_packets(source),
        support::video_packets(output),
    );
    // The source's first keyframe gains in-band parameter sets (the preview's
    // differ); every other packet is copied byte for byte.
    assert_eq!(&out_hashes[2..], &src_hashes[1..]);

    let (sv, ov) = (video_stream(source), video_stream(output));
    assert_eq!(ov["avg_frame_rate"], sv["avg_frame_rate"]);
    assert_eq!(ov["r_frame_rate"], sv["r_frame_rate"]);
    assert_eq!(
        ov["duration_ts"].as_i64().unwrap(),
        sv["duration_ts"].as_i64().unwrap() + step
    );
}

fn audio_start_secs(path: &Path) -> f64 {
    let a = packets(path, "a:0");
    let v = ffprobe_json(&["-select_streams", "a:0", "-show_streams"], path);
    let tb =
        framecover_core::probe::Rational::parse(v["streams"][0]["time_base"].as_str().unwrap())
            .unwrap();
    a[0].pts as f64 * tb.to_f64()
}

fn copy_of(source: &Path, name: &str) -> PathBuf {
    let target = source.with_file_name(name);
    std::fs::copy(source, &target).unwrap();
    target
}

fn check_first_save(source: &Path, step: i64, fps: u64) {
    let target = copy_of(source, "target.mp4");
    let result = save(&target, 2000);
    assert_eq!(result.duration_shift_ms, 1000 / fps);
    assert_regular(&target, step);
    assert_preview_first(&target);
    assert_shifted_by_one_frame(source, &target, step);
    let shift = audio_start_secs(&target) - audio_start_secs(source);
    assert!(
        (shift - 1.0 / fps as f64).abs() < 0.002,
        "audio shift {shift}"
    );

    let reloaded = FfprobeVideoLoader.load(&target).unwrap();
    assert_eq!(reloaded.frame_step_ms, 1000 / fps);
    let original = FfprobeVideoLoader.load(source).unwrap();
    assert_eq!(reloaded.duration_ms, original.duration_ms + 1000 / fps);
}

#[test]
fn b_frame_source_keeps_frame_rate() {
    let dir = tempfile::tempdir().unwrap();
    let source = common::sample_video(dir.path());
    // x264 defaults: B-frames, so the source starts at a negative DTS.
    assert!(packets(&source, "v:0")[0].dts < 0);
    check_first_save(&source, 512, 25);
}

#[test]
fn source_without_b_frames() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("nob.mp4");
    common::generate_video(
        &source,
        5,
        320,
        240,
        &[
            "-c:v", "libx264", "-bf", "0", "-c:a", "aac", "-pix_fmt", "yuv420p",
        ],
    );
    assert_eq!(packets(&source, "v:0")[0].dts, 0);
    check_first_save(&source, 512, 25);
}

#[test]
fn av1_source() {
    if !support::has_encoder("libaom-av1") {
        eprintln!("libaom-av1 unavailable; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("av1.mp4");
    common::generate_video(
        &source,
        2,
        160,
        120,
        &[
            "-c:v",
            "libaom-av1",
            "-cpu-used",
            "8",
            "-row-mt",
            "1",
            "-crf",
            "35",
            "-b:v",
            "0",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
        ],
    );
    let target = copy_of(&source, "target.mp4");
    let step = video_stream(&source)["duration_ts"].as_i64().unwrap()
        / packets(&source, "v:0").len() as i64;
    let result = save(&target, 1000);
    assert_eq!(result.duration_shift_ms, 40);
    assert_regular(&target, step);
    assert_preview_first(&target);
    assert_shifted_by_one_frame(&source, &target, step);
}

#[test]
fn resave_replaces_preview_and_keeps_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let source = common::sample_video(dir.path());
    let target = copy_of(&source, "target.mp4");
    save(&target, 2000);
    let result = save(&target, 3000);
    assert_eq!(result.duration_shift_ms, 0);
    assert_regular(&target, 512);
    assert_preview_first(&target);
    assert_shifted_by_one_frame(&source, &target, 512);
    let video = FfprobeVideoLoader.load(&target).unwrap();
    assert_eq!(video.thumbnail_position_ms, Some(3000));
    assert_eq!(video.thumbnail_frame_number, Some(75));
}

/// What earlier versions (Python and the first Rust port) wrote: the preview
/// concatenated without DTS repair, so the muxer squashed the leading DTS.
fn legacy_saved_file(dir: &Path, source: &Path) -> PathBuf {
    let poster = dir.join("poster.png");
    std::fs::write(&poster, imaging::encode_png(&red()).unwrap()).unwrap();
    let preview = dir.join("preview.mp4");
    let p = |p: &Path| p.to_str().unwrap().to_owned();
    ffmpeg::ffmpeg([
        "-y",
        "-loop",
        "1",
        "-framerate",
        "25/1",
        "-i",
        &p(&poster),
        "-frames:v",
        "1",
        "-vf",
        "scale=320:240:flags=lanczos",
        "-an",
        "-sn",
        "-c:v",
        "libx264",
        "-profile:v",
        "high",
        "-pix_fmt",
        "yuv420p",
        "-video_track_timescale",
        "12800",
        "-f",
        "mp4",
        &p(&preview),
    ])
    .unwrap();
    let manifest = dir.join("concat.txt");
    let posix = |path: &Path| p(path).replace('\\', "/");
    std::fs::write(
        &manifest,
        format!(
            "file '{}'\nduration 0.04\nfile '{}'\n",
            posix(&preview),
            posix(source)
        ),
    )
    .unwrap();
    let cover = dir.join("cover.jpg");
    std::fs::write(&cover, imaging::cover_jpeg(&red()).unwrap()).unwrap();
    let legacy = dir.join("legacy.mp4");
    ffmpeg::ffmpeg([
        "-y",
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
        &p(&manifest),
        "-itsoffset",
        "0.04",
        "-i",
        &p(source),
        "-i",
        &p(&cover),
        "-map",
        "0:v:0",
        "-map",
        "1:a?",
        "-map",
        "2:v:0",
        "-c",
        "copy",
        "-disposition:v:1",
        "attached_pic",
        "-f",
        "mp4",
        &p(&legacy),
    ])
    .unwrap();
    // Old versions wrote the tags with the drifted step: round(2040/39) = 52.
    mp4_tags::write_thumbnail_source(&legacy, 52, 2040).unwrap();
    legacy
}

#[test]
fn resave_repairs_legacy_squashed_file() {
    let dir = tempfile::tempdir().unwrap();
    let source = common::sample_video(dir.path());
    let legacy = legacy_saved_file(dir.path(), &source);
    // The bug being repaired: 1-tick packets and a drifted average rate.
    let before = packets(&legacy, "v:0");
    assert!(
        before.iter().take(3).any(|p| p.duration < 512),
        "{before:?}"
    );
    assert_ne!(video_stream(&legacy)["avg_frame_rate"], "25/1");

    // The loader already corrects the drift (r_frame_rate, nb_frames).
    let video = FfprobeVideoLoader.load(&legacy).unwrap();
    assert!(video.has_inserted_preview());
    assert_eq!(video.frame_step_ms, 40);
    assert_eq!(video.duration_ms, 5040);

    let result = save(&legacy, 3000);
    assert_eq!(result.duration_shift_ms, 0);
    assert_regular(&legacy, 512);
    assert_preview_first(&legacy);
    assert_shifted_by_one_frame(&source, &legacy, 512);
    let video = FfprobeVideoLoader.load(&legacy).unwrap();
    assert_eq!(video.thumbnail_frame_number, Some(75));
}

// --- Full re-encode fallback ------------------------------------------------

fn stream_duration_secs(path: &Path, stream: &str) -> f64 {
    let v = ffprobe_json(&["-select_streams", stream, "-show_streams"], path);
    v["streams"][0]["duration"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

/// Mean colour of decoded frame `n` of the first playable video stream.
fn frame_mean(path: &Path, n: usize) -> [f64; 3] {
    let info = framecover_core::probe::probe(path).unwrap();
    let s = info.first_playable_video().unwrap();
    let (w, h) = (s.width.unwrap(), s.height.unwrap());
    let select = format!("select=eq(n\\,{n})");
    let map = format!("0:{}", s.index);
    let out = ffmpeg::ffmpeg([
        "-v",
        "error",
        "-i",
        path.to_str().unwrap(),
        "-map",
        &map,
        "-vf",
        &select,
        "-frames:v",
        "1",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-",
    ])
    .unwrap();
    support::mean(&RgbImage::from_raw(w, h, out.stdout).unwrap())
}

fn assert_close(actual: [f64; 3], expected: [f64; 3], what: &str) {
    assert!(
        actual
            .iter()
            .zip(&expected)
            .all(|(a, e)| (a - e).abs() < 8.0),
        "{what}: {actual:?} vs {expected:?}"
    );
}

#[test]
fn reencode_inserts_preview_instead_of_overwriting_frame_zero() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("yuv444p.mp4");
    common::generate_video(
        &source,
        5,
        320,
        240,
        &["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv444p"],
    );
    let target = copy_of(&source, "target.mp4");
    let video = FfprobeVideoLoader.load(&target).unwrap();
    assert!(
        FormatDispatchThumbnailWriter
            .reencode_reason(&video)
            .is_some()
    );
    let source_frame0 = frame_mean(&source, 0);

    let result = save(&target, 2000);
    assert_eq!(result.duration_shift_ms, 40);
    assert_eq!(
        support::frame_count(&target),
        support::frame_count(&source) + 1
    );
    assert_regular(&target, packets(&target, "v:0")[0].duration);
    assert_preview_first(&target);
    // Frame 1 is the source's frame 0, not the poster.
    assert_close(frame_mean(&target, 1), source_frame0, "frame 1");
    assert_eq!(video_stream(&target)["avg_frame_rate"], "25/1");
    let grown = stream_duration_secs(&target, "v:0") - stream_duration_secs(&source, "v:0");
    assert!((grown - 0.04).abs() < 0.002, "video grew by {grown}");
    let shift = audio_start_secs(&target) - audio_start_secs(&source);
    assert!((shift - 0.04).abs() < 0.002, "audio shift {shift}");
    let reloaded = FfprobeVideoLoader.load(&target).unwrap();
    assert_eq!(reloaded.thumbnail_position_ms, Some(2040));
    assert_eq!(reloaded.thumbnail_frame_number, Some(51));

    // A re-save replaces the inserted frame instead of adding another.
    let frames = support::frame_count(&target);
    let blue = RgbImage::from_pixel(320, 240, image::Rgb([20, 20, 240]));
    let result = FormatDispatchThumbnailWriter.write(&reloaded, &blue, Some(3000), None);
    assert!(result.success, "{:?}", result.error_message);
    assert_eq!(result.duration_shift_ms, 0);
    assert_eq!(support::frame_count(&target), frames);
    let [r, g, b] = frame_mean(&target, 0);
    assert!(b > 200.0 && r < 60.0 && g < 60.0, "first frame {r} {g} {b}");
    assert_close(
        frame_mean(&target, 1),
        source_frame0,
        "frame 1 after re-save",
    );
    let shift = audio_start_secs(&target) - audio_start_secs(&source);
    assert!(
        (shift - 0.04).abs() < 0.002,
        "audio shift after re-save {shift}"
    );
    let reloaded = FfprobeVideoLoader.load(&target).unwrap();
    assert_eq!(reloaded.thumbnail_position_ms, Some(3000));
    assert_eq!(reloaded.thumbnail_frame_number, Some(75));
}

/// Earlier versions' re-encode painted the poster over frame 0 inside the
/// first GOP; such a preview cannot be stripped by stream copy, so a re-save
/// re-encodes and trims it instead of growing the file or breaking decoding.
#[test]
fn resave_of_legacy_overlay_reencode_trims_first_frame() {
    let dir = tempfile::tempdir().unwrap();
    let source = common::sample_video(dir.path());
    let poster = dir.path().join("poster.png");
    std::fs::write(&poster, imaging::encode_png(&red()).unwrap()).unwrap();
    let cover = dir.path().join("cover.jpg");
    std::fs::write(&cover, imaging::cover_jpeg(&red()).unwrap()).unwrap();
    let legacy = dir.path().join("legacy.mp4");
    let p = |p: &Path| p.to_str().unwrap().to_owned();
    ffmpeg::ffmpeg([
        "-y",
        "-i",
        &p(&source),
        "-loop",
        "1",
        "-i",
        &p(&poster),
        "-i",
        &p(&cover),
        "-filter_complex",
        "[1:v][0:v]scale2ref=flags=lanczos[poster][base];\
         [base][poster]overlay=shortest=1:enable='eq(n,0)'[v]",
        "-map",
        "[v]",
        "-map",
        "0:a?",
        "-map",
        "2:v",
        "-c:v:0",
        "libx264",
        "-preset:v:0",
        "veryfast",
        "-pix_fmt:v:0",
        "yuv420p",
        "-c:v:1",
        "copy",
        "-disposition:v:1",
        "attached_pic",
        "-c:a",
        "copy",
        "-f",
        "mp4",
        &p(&legacy),
    ])
    .unwrap();
    mp4_tags::write_thumbnail_source(&legacy, 50, 2000).unwrap();
    let frames = support::frame_count(&legacy);
    let video = FfprobeVideoLoader.load(&legacy).unwrap();
    assert!(video.has_inserted_preview());
    assert!(
        FormatDispatchThumbnailWriter
            .reencode_reason(&video)
            .unwrap()
            .contains("cannot be removed without re-encoding")
    );

    let blue = RgbImage::from_pixel(320, 240, image::Rgb([20, 20, 240]));
    let result = FormatDispatchThumbnailWriter.write(&video, &blue, Some(3000), None);
    assert!(result.success, "{:?}", result.error_message);
    assert_eq!(result.duration_shift_ms, 0);
    assert_eq!(support::frame_count(&legacy), frames);
    let [r, g, b] = frame_mean(&legacy, 0);
    assert!(b > 200.0 && r < 60.0 && g < 60.0, "first frame {r} {g} {b}");
    assert_close(
        frame_mean(&legacy, 1),
        frame_mean(&source, 1),
        "frame 1 is still the source's frame 1",
    );
    // The new output can be re-saved by stream copy again.
    let video = FfprobeVideoLoader.load(&legacy).unwrap();
    assert_eq!(FormatDispatchThumbnailWriter.reencode_reason(&video), None);
}
