# FrameCover Development Guidelines

FrameCover is a Rust desktop app (Slint GUI) that embeds a chosen video frame
as the file's cover art. Video work is done by bundled `ffmpeg`/`ffprobe`
executables run as child processes.

## Project structure

```text
crates/framecover-core/   # everything except the GUI: probing, frame extraction,
                          # thumbnail writing, safe save-copy, MP4 conversion,
                          # update check, OS thumbnail-cache integration
crates/framecover-app/    # the `framecover` binary: Slint UI (ui/*.slint),
                          # controller state machine, background workers
xtask/                    # fetch-ffmpeg, changelog, release, package
site/                     # landing page (GitHub Pages)
```

## Build, test, lint

```bash
cargo xtask fetch-ffmpeg   # pinned FFmpeg into vendor/ffmpeg/<platform>/ (needed by tests)
cargo run -p framecover    # run the app
cargo test --workspace
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

`make` targets wrap the same commands (`make help`).

## Conventions

- Conventional Commits (`feat:`, `fix:`, `feat!:`); CHANGELOG.md is generated
  by git-cliff via `cargo xtask changelog` / `cargo xtask release create`.
- Core services sit behind small traits (`VideoLoader`, `FrameExtractor`,
  `ThumbnailWriter`, …) so the app can be tested with fakes; the GUI wires the
  real implementations only in `crates/framecover-app/src/services.rs`.
- Keep on-disk compatibility: MP4 freeform tags
  `----:com.apple.iTunes:video-thumbnailer-frame-number` / `-position-ms`, the
  receipt name `.<name>.video-thumbnailer.json`, and release asset names
  `framecover-<version>-<platform>.<ext>` (used by the site and the updater).
