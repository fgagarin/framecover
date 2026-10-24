//! Frame extraction at 2.5 s from 5-second SD, 1080p and 4K H.264 MP4s
//! (port of `tests/benchmarks/test_frame_extraction_perf.py`).
//!
//! Run with `cargo bench -p framecover-core --bench frame_extraction`.
//! The 500 ms budget itself is asserted by the ignored test
//! `extraction_within_500ms_budget` in `tests/frames.rs`.

use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use framecover_core::ffmpeg;
use framecover_core::frames::{FfmpegFrameExtractor, FrameExtractor};
use framecover_core::loader::{FfprobeVideoLoader, VideoLoader};

fn bench_extraction(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut group = c.benchmark_group("frame_extract");
    group
        .sample_size(10)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(8));
    for (label, w, h) in [("sd", 320, 240), ("1080p", 1920, 1080), ("4k", 3840, 2160)] {
        let path = dir.path().join(format!("sample_{label}.mp4"));
        let src = format!("testsrc=size={w}x{h}:rate=25:duration=5");
        ffmpeg::ffmpeg([
            "-y",
            "-f",
            "lavfi",
            "-i",
            &src,
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=5",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-pix_fmt",
            "yuv420p",
            path.to_str().unwrap(),
        ])
        .expect("generate benchmark video");
        let video = FfprobeVideoLoader.load(&path).expect("load");
        group.bench_function(label, |b| {
            b.iter(|| FfmpegFrameExtractor.extract(&video, 2500).expect("extract"))
        });
    }
    group.finish();
}

criterion_group!(benches, bench_extraction);
criterion_main!(benches);
