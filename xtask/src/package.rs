//! `package <platform>`: build and assemble release assets named
//! `framecover-<version>-<platform>.<ext>` (the website and the v0.1.0
//! updater depend on these exact names).
//!
//! * windows: `framecover.exe` with FFmpeg embedded at build time
//!   (`FRAMECOVER_EMBED_FFMPEG_DIR`), copied to `framecover-<v>-windows.exe`;
//!   plus `framecover-<v>-windows-cli.zip` holding that exe as
//!   `framecover.exe` next to the console launcher `framecover.com`, ready to
//!   drop into a folder on `PATH` for command-line use.
//! * linux: `framecover-<v>-linux.tar.gz` containing `framecover/` with the
//!   binary, `ffmpeg`, `ffprobe` and `LICENSE`.
//! * macos: `framecover-<v>-macos.tar.gz` containing a universal (arm64 +
//!   x86_64) `framecover.app` with ffmpeg/ffprobe in `Contents/Resources`.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use flate2::Compression;
use flate2::write::GzEncoder;

use crate::ffmpeg::{tool_names, vendor_dir};
use crate::util::{Result, bail, cargo, root, run, status, workspace_version};

pub const BUNDLE_ID: &str = "com.framecover.app";
const BIN: &str = "framecover";
const MAC_TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];

pub fn package(
    platform: &str,
    output: Option<PathBuf>,
    skip_build: bool,
    binary: Option<PathBuf>,
) -> Result<()> {
    let version = workspace_version()?;
    let out_dir = output.unwrap_or_else(|| root().join("release-assets"));
    std::fs::create_dir_all(&out_dir)?;
    let name = format!("framecover-{version}-{platform}");
    let mtime = source_date_epoch();

    let asset = match platform {
        "windows" => {
            let ffmpeg_dir = require_ffmpeg("windows-x64")?;
            let exe = match binary {
                Some(b) => b,
                None => {
                    if !skip_build {
                        build(None, Some(&ffmpeg_dir))?;
                    }
                    target_binary(None, ".exe")
                }
            };
            let asset = out_dir.join(format!("{name}.exe"));
            std::fs::copy(&exe, &asset).map_err(|e| format!("{}: {e}", exe.display()))?;
            let console = exe.with_file_name("framecover-console.exe");
            let cli_zip = out_dir.join(format!("{name}-cli.zip"));
            write_zip(
                &cli_zip,
                &[
                    ("framecover.exe", exe.clone()),
                    ("framecover.com", console),
                    ("LICENSE", root().join("LICENSE")),
                    (
                        "FFMPEG-NOTICE.txt",
                        root().join("licenses/FFMPEG-NOTICE.txt"),
                    ),
                    (
                        "FFmpeg-GPL-3.0.txt",
                        root().join("licenses/FFmpeg-GPL-3.0.txt"),
                    ),
                ],
            )?;
            println!("Wrote {}", cli_zip.display());
            asset
        }
        "linux" => {
            let ffmpeg_platform = match std::env::consts::ARCH {
                "aarch64" => "linux-arm64",
                _ => "linux-x64",
            };
            let ffmpeg_dir = require_ffmpeg(ffmpeg_platform)?;
            let bin = match binary {
                Some(b) => b,
                None => {
                    if !skip_build {
                        build(None, None)?;
                    }
                    target_binary(None, "")
                }
            };
            let [ffmpeg, ffprobe] = tool_names(ffmpeg_platform);
            let entries = vec![
                Entry::dir("framecover/"),
                Entry::file("framecover/framecover", bin, 0o755),
                Entry::file("framecover/ffmpeg", ffmpeg_dir.join(ffmpeg), 0o755),
                Entry::file("framecover/ffprobe", ffmpeg_dir.join(ffprobe), 0o755),
                Entry::file("framecover/LICENSE", root().join("LICENSE"), 0o644),
                Entry::file(
                    "framecover/FFMPEG-NOTICE.txt",
                    root().join("licenses/FFMPEG-NOTICE.txt"),
                    0o644,
                ),
                Entry::file(
                    "framecover/FFmpeg-GPL-3.0.txt",
                    root().join("licenses/FFmpeg-GPL-3.0.txt"),
                    0o644,
                ),
            ];
            let asset = out_dir.join(format!("{name}.tar.gz"));
            write_tar_gz(&asset, &entries, mtime)?;
            asset
        }
        "macos" => {
            let work = root().join("target").join("macos-bundle");
            let _ = std::fs::remove_dir_all(&work);
            std::fs::create_dir_all(&work)?;
            let bin = match binary {
                Some(b) => b,
                None => {
                    let slices: Vec<PathBuf> = MAC_TARGETS
                        .iter()
                        .map(|t| {
                            if !skip_build {
                                build(Some(t), None)?;
                            }
                            Ok(target_binary(Some(t), ""))
                        })
                        .collect::<Result<_>>()?;
                    let universal = work.join(BIN);
                    lipo(&slices, &universal)?;
                    universal
                }
            };
            let arm = require_ffmpeg("macos-arm64")?;
            let x64 = require_ffmpeg("macos-x64")?;
            let mut tools = Vec::new();
            for tool in ["ffmpeg", "ffprobe"] {
                let universal = work.join(tool);
                lipo(&[arm.join(tool), x64.join(tool)], &universal)?;
                tools.push(universal);
            }

            let plist = work.join("Info.plist");
            std::fs::write(&plist, info_plist(&version))?;
            let pkginfo = work.join("PkgInfo");
            std::fs::write(&pkginfo, "APPL????")?;
            let app = "framecover.app/Contents";
            let entries = vec![
                Entry::dir("framecover.app/"),
                Entry::dir(&format!("{app}/")),
                Entry::dir(&format!("{app}/MacOS/")),
                Entry::dir(&format!("{app}/Resources/")),
                Entry::file(&format!("{app}/Info.plist"), plist, 0o644),
                Entry::file(&format!("{app}/PkgInfo"), pkginfo, 0o644),
                Entry::file(&format!("{app}/MacOS/{BIN}"), bin, 0o755),
                Entry::file(
                    &format!("{app}/Resources/icon.icns"),
                    root().join("icon.icns"),
                    0o644,
                ),
                Entry::file(&format!("{app}/Resources/ffmpeg"), tools[0].clone(), 0o755),
                Entry::file(&format!("{app}/Resources/ffprobe"), tools[1].clone(), 0o755),
                Entry::file(
                    &format!("{app}/Resources/LICENSE"),
                    root().join("LICENSE"),
                    0o644,
                ),
                Entry::file(
                    &format!("{app}/Resources/FFMPEG-NOTICE.txt"),
                    root().join("licenses/FFMPEG-NOTICE.txt"),
                    0o644,
                ),
                Entry::file(
                    &format!("{app}/Resources/FFmpeg-GPL-3.0.txt"),
                    root().join("licenses/FFmpeg-GPL-3.0.txt"),
                    0o644,
                ),
            ];
            let signed = sign_app(&work, &entries)?;
            let asset = out_dir.join(format!("{name}.tar.gz"));
            write_tar_gz(&asset, &signed, mtime)?;
            asset
        }
        other => bail!("unknown platform {other:?} (expected windows, linux or macos)"),
    };
    println!("Wrote {}", asset.display());
    Ok(())
}

fn require_ffmpeg(platform: &str) -> Result<PathBuf> {
    let dir = vendor_dir(platform);
    for name in tool_names(platform) {
        if !dir.join(&name).is_file() {
            bail!(
                "{} is missing; run `cargo xtask fetch-ffmpeg {platform}` first",
                dir.join(name).display()
            );
        }
    }
    Ok(dir)
}

fn build(target: Option<&str>, embed_ffmpeg: Option<&Path>) -> Result<()> {
    let mut cmd = Command::new(cargo());
    cmd.args(["build", "--release", "--locked", "-p", BIN]);
    if let Some(t) = target {
        cmd.args(["--target", t]);
    }
    if let Some(dir) = embed_ffmpeg {
        cmd.env("FRAMECOVER_EMBED_FFMPEG_DIR", dir);
    }
    status(&mut cmd)
}

fn target_binary(target: Option<&str>, ext: &str) -> PathBuf {
    let mut dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root().join("target"));
    if let Some(t) = target {
        dir = dir.join(t);
    }
    dir.join("release").join(format!("{BIN}{ext}"))
}

fn lipo(inputs: &[PathBuf], output: &Path) -> Result<()> {
    let mut cmd = Command::new("lipo");
    cmd.arg("-create").args(inputs).arg("-output").arg(output);
    status(&mut cmd).map_err(|e| format!("{e} (lipo is required to build the macOS bundle)").into())
}

/// Lay the bundle out on disk and ad-hoc sign it (required for arm64 code
/// after lipo/bundle changes). Returns the entries pointing at the signed
/// copies. Skipped when `codesign` is unavailable (non-macOS hosts).
fn sign_app(work: &Path, entries: &[Entry]) -> Result<Vec<Entry>> {
    if !cfg!(target_os = "macos") {
        println!("codesign not found; leaving the bundle unsigned");
        return Ok(entries.to_vec());
    }
    let stage = work.join("stage");
    let mut staged = Vec::new();
    for entry in entries {
        let dest = stage.join(&entry.path);
        match &entry.source {
            None => std::fs::create_dir_all(&dest)?,
            Some(src) => {
                std::fs::copy(src, &dest)?;
                crate::ffmpeg::set_executable(&dest)?;
                staged.push(Entry {
                    source: Some(dest.clone()),
                    ..entry.clone()
                });
                continue;
            }
        }
        staged.push(entry.clone());
    }
    let app = stage.join("framecover.app");
    for tool in ["ffmpeg", "ffprobe"] {
        let mut cmd = Command::new("codesign");
        cmd.args(["--force", "--sign", "-"])
            .arg(app.join("Contents/Resources").join(tool));
        status(&mut cmd)?;
    }
    let mut cmd = Command::new("codesign");
    cmd.args(["--force", "--sign", "-"]).arg(&app);
    status(&mut cmd)?;
    // Signing adds Contents/_CodeSignature/CodeResources.
    let sig = app.join("Contents/_CodeSignature/CodeResources");
    if sig.is_file() {
        staged.push(Entry::dir("framecover.app/Contents/_CodeSignature/"));
        staged.push(Entry::file(
            "framecover.app/Contents/_CodeSignature/CodeResources",
            sig,
            0o644,
        ));
    }
    Ok(staged)
}

pub fn info_plist(version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDisplayName</key>
	<string>FrameCover</string>
	<key>CFBundleExecutable</key>
	<string>{BIN}</string>
	<key>CFBundleIconFile</key>
	<string>icon.icns</string>
	<key>CFBundleIdentifier</key>
	<string>{BUNDLE_ID}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>FrameCover</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.video</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSHumanReadableCopyright</key>
	<string>FrameCover</string>
</dict>
</plist>
"#
    )
}

/// One tar member: a directory (`source == None`) or a file.
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: String,
    pub source: Option<PathBuf>,
    pub mode: u32,
}

impl Entry {
    pub fn dir(path: &str) -> Self {
        Self {
            path: path.to_owned(),
            source: None,
            mode: 0o755,
        }
    }

    pub fn file(path: &str, source: PathBuf, mode: u32) -> Self {
        Self {
            path: path.to_owned(),
            source: Some(source),
            mode,
        }
    }
}

/// Write a gzip'd tar with explicit modes (so executables keep `+x` even
/// when packaged on Windows) and a fixed mtime for reproducibility.
/// Write a deflate zip of `(name in archive, source file)` pairs.
fn write_zip(dest: &Path, files: &[(&str, PathBuf)]) -> Result<()> {
    let mut zip = zip::ZipWriter::new(File::create(dest)?);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, source) in files {
        let mut input = File::open(source).map_err(|e| format!("{}: {e}", source.display()))?;
        zip.start_file(*name, options)?;
        std::io::copy(&mut input, &mut zip)?;
    }
    zip.finish()?;
    Ok(())
}

pub fn write_tar_gz(dest: &Path, entries: &[Entry], mtime: u64) -> Result<()> {
    let tmp = dest.with_extension("tmp");
    {
        let gz = GzEncoder::new(File::create(&tmp)?, Compression::best());
        let mut tar = tar::Builder::new(gz);
        for entry in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mode(entry.mode);
            header.set_mtime(mtime);
            header.set_uid(0);
            header.set_gid(0);
            match &entry.source {
                None => {
                    header.set_entry_type(tar::EntryType::Directory);
                    header.set_size(0);
                    tar.append_data(&mut header, &entry.path, std::io::empty())?;
                }
                Some(src) => {
                    let file = File::open(src).map_err(|e| format!("{}: {e}", src.display()))?;
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_size(file.metadata()?.len());
                    tar.append_data(&mut header, &entry.path, file)?;
                }
            }
        }
        tar.into_inner()?.finish()?.flush()?;
    }
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

/// `SOURCE_DATE_EPOCH`, else the HEAD commit time, else 0.
fn source_date_epoch() -> u64 {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            run("git", &["log", "-1", "--format=%ct"])
                .ok()?
                .parse()
                .ok()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;
    use crate::ffmpeg::tests::tempdir;

    #[test]
    fn plist_has_bundle_metadata() {
        let plist = info_plist("1.2.3");
        assert!(plist.contains("<string>com.framecover.app</string>"));
        assert!(plist.contains("<key>CFBundleShortVersionString</key>\n\t<string>1.2.3</string>"));
        assert!(plist.contains("<string>icon.icns</string>"));
        assert!(plist.contains("<key>CFBundleExecutable</key>\n\t<string>framecover</string>"));
    }

    #[test]
    fn tarball_preserves_modes_and_layout() {
        let tmp = tempdir();
        let bin = tmp.join("bin");
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        let asset = tmp.join("framecover-0.1.0-linux.tar.gz");
        let entries = [
            Entry::dir("framecover/"),
            Entry::file("framecover/framecover", bin.clone(), 0o755),
            Entry::file("framecover/LICENSE", bin, 0o644),
        ];
        write_tar_gz(&asset, &entries, 1_700_000_000).unwrap();

        let mut archive =
            tar::Archive::new(flate2::read::GzDecoder::new(File::open(&asset).unwrap()));
        let mut seen = Vec::new();
        for e in archive.entries().unwrap() {
            let mut e = e.unwrap();
            let mut body = String::new();
            e.read_to_string(&mut body).unwrap();
            seen.push((
                e.path().unwrap().to_string_lossy().into_owned(),
                e.header().mode().unwrap(),
                e.header().mtime().unwrap(),
            ));
        }
        assert_eq!(
            seen,
            vec![
                ("framecover/".into(), 0o755, 1_700_000_000),
                ("framecover/framecover".into(), 0o755, 1_700_000_000),
                ("framecover/LICENSE".into(), 0o644, 1_700_000_000),
            ]
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
