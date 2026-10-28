//! Compile `csrc/fc_shim.c` and link the static FFmpeg build for the target.
//!
//! The libraries come from `scripts/android/build-ffmpeg.sh`, which writes
//! `vendor/ffmpeg/libav/<platform>/` with `include/`, `lib/` and `libs.txt`
//! (the `pkg-config --static --libs` line, `-lfftools` first).
//! `FRAMECOVER_LIBAV_DIR` overrides that directory.

use std::env;
use std::path::PathBuf;

fn platform(target: &str) -> Option<&'static str> {
    Some(match target {
        "aarch64-linux-android" => "android-arm64-v8a",
        "x86_64-linux-android" => "android-x86_64",
        "x86_64-unknown-linux-gnu" => "linux-x64",
        _ => return None,
    })
}

fn main() {
    println!("cargo:rerun-if-changed=csrc/fc_shim.c");
    println!("cargo:rerun-if-env-changed=FRAMECOVER_LIBAV_DIR");

    let target = env::var("TARGET").expect("TARGET");
    let explicit = env::var_os("FRAMECOVER_LIBAV_DIR").map(PathBuf::from);
    let dir = match (&explicit, platform(&target)) {
        (Some(dir), _) => dir.clone(),
        (None, Some(platform)) => {
            let manifest =
                PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
            manifest.join("../../vendor/ffmpeg/libav").join(platform)
        }
        (None, None) => {
            // Workspace-wide builds (`cargo clippy --workspace` on Windows,
            // macOS): the crate compiles (empty) but links nothing; only
            // framecover-core's `libav` feature uses it.
            return;
        }
    };
    let libs_txt = dir.join("libs.txt");
    println!("cargo:rerun-if-changed={}", libs_txt.display());
    let libs = match std::fs::read_to_string(&libs_txt) {
        Ok(libs) => libs,
        Err(e) if explicit.is_some() => {
            panic!("framecover-libav: cannot read {} ({e})", libs_txt.display())
        }
        Err(_) => {
            println!(
                "cargo:warning=framecover-libav: {} is missing; build FFmpeg with \
                 scripts/android/build-ffmpeg.sh (see scripts/android/Dockerfile)",
                libs_txt.display()
            );
            return;
        }
    };

    cc::Build::new()
        .file("csrc/fc_shim.c")
        .include(dir.join("include"))
        .flag_if_supported("-std=c11")
        .warnings(true)
        .compile("fc_shim");

    let lib_dir = dir.join("lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    let android = target.contains("android");
    for token in libs.split_whitespace() {
        if let Some(path) = token.strip_prefix("-L") {
            println!("cargo:rustc-link-search=native={path}");
        } else if let Some(name) = token.strip_prefix("-l") {
            // Bionic has no separate pthread / rt libraries.
            if android && matches!(name, "pthread" | "rt") {
                continue;
            }
            if lib_dir.join(format!("lib{name}.a")).exists() {
                println!("cargo:rustc-link-lib=static={name}");
            } else {
                println!("cargo:rustc-link-lib={name}");
            }
        } else if token == "-pthread" && !android {
            println!("cargo:rustc-link-lib=pthread");
        }
    }
}
