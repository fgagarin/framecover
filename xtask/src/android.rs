//! `android`: build the Android app (`framecover-<version>-android.apk`).
//!
//! 1. `cargo ndk` builds the app library (`libframecover.so`, with FFmpeg
//!    linked in through `framecover-core`'s `libav` feature) for each ABI and
//!    copies it to `android/build/jniLibs/<abi>/`;
//! 2. Gradle builds `android/` (MainActivity plus those libraries).
//!
//! Needs the Android NDK and SDK, `cargo ndk`, Gradle and the FFmpeg
//! libraries from `scripts/android/build-ffmpeg.sh`; the image built from
//! `scripts/android/Dockerfile` has all of them.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::util::{Result, bail, cargo, parse_semver, root, status, workspace_version};

/// `(ABI, Rust target)` pairs the APK ships.
pub const ABIS: [(&str, &str); 2] = [
    ("arm64-v8a", "aarch64-linux-android"),
    ("x86_64", "x86_64-linux-android"),
];

/// Minimum API level (MediaStore `RELATIVE_PATH`); matches android/app/build.gradle.
const MIN_SDK: &str = "29";

pub fn build(abis: &[String], debug: bool, output: Option<PathBuf>) -> Result<()> {
    let selected: Vec<(&str, &str)> = if abis.is_empty() {
        ABIS.to_vec()
    } else {
        abis.iter()
            .map(|abi| {
                ABIS.iter()
                    .copied()
                    .find(|(name, _)| name == abi)
                    .ok_or_else(|| format!("unknown ABI {abi:?} (expected arm64-v8a or x86_64)"))
            })
            .collect::<std::result::Result<_, _>>()?
    };
    let version = workspace_version()?;
    let (major, minor, patch) = parse_semver(&version)?;
    let profile = if debug { "debug" } else { "release" };

    let jni_libs = root().join("android/build/jniLibs");
    if jni_libs.exists() {
        std::fs::remove_dir_all(&jni_libs)?;
    }
    for (abi, target) in &selected {
        let libav = root()
            .join("vendor/ffmpeg/libav")
            .join(format!("android-{abi}"));
        if !libav.join("libs.txt").is_file() {
            bail!(
                "{} is missing; build FFmpeg first:\n  \
                 docker build -t framecover-android scripts/android\n  \
                 docker run --rm -v \"$PWD:/src\" framecover-android \
                 scripts/android/build-ffmpeg.sh android-{abi}",
                libav.display()
            );
        }
        let mut cmd = Command::new(cargo());
        cmd.args(["ndk", "-t", abi, "--platform", MIN_SDK, "rustc", "--locked"])
            .args(["-p", "framecover", "--lib", "--crate-type", "cdylib"]);
        if !debug {
            cmd.arg("--release");
        }
        status(&mut cmd)?;
        let built = target_dir()
            .join(target)
            .join(profile)
            .join("libframecover.so");
        let dest = jni_libs.join(abi);
        std::fs::create_dir_all(&dest)?;
        std::fs::copy(&built, dest.join("libframecover.so"))
            .map_err(|e| format!("cannot copy {}: {e}", built.display()))?;
    }

    let task = if debug {
        "assembleDebug"
    } else {
        "assembleRelease"
    };
    status(
        Command::new(gradle())
            .args(["-p", "android", "--no-daemon", task])
            .arg(format!("-PversionName={version}"))
            .arg(format!(
                "-PversionCode={}",
                major * 10_000 + minor * 100 + patch
            )),
    )?;

    let apk = root()
        .join("android/app/build/outputs/apk")
        .join(profile)
        .join(format!("app-{profile}.apk"));
    let out_dir = output.unwrap_or_else(|| root().join("release-assets"));
    std::fs::create_dir_all(&out_dir)?;
    let suffix = if debug { "-debug" } else { "" };
    let dest = out_dir.join(format!("framecover-{version}-android{suffix}.apk"));
    std::fs::copy(&apk, &dest).map_err(|e| format!("cannot copy {}: {e}", apk.display()))?;
    println!("{}", dest.display());
    Ok(())
}

/// Cargo's target directory (`CARGO_TARGET_DIR` or `<root>/target`).
fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|dir| {
            if dir.is_relative() {
                root().join(dir)
            } else {
                dir
            }
        })
        .unwrap_or_else(|| root().join("target"))
}

/// `GRADLE`, else a wrapper in android/ if one exists, else `gradle` on PATH.
fn gradle() -> String {
    if let Some(gradle) = std::env::var_os("GRADLE") {
        return gradle.to_string_lossy().into_owned();
    }
    let wrapper = root().join("android").join(if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    });
    if Path::new(&wrapper).is_file() {
        return wrapper.to_string_lossy().into_owned();
    }
    "gradle".into()
}
