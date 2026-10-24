//! Compiles the Slint UI, optionally embeds compressed FFmpeg binaries
//! (`FRAMECOVER_EMBED_FFMPEG_DIR`) and, on Windows, embeds the executable
//! icon and application manifest.

const WINDOWS_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="FrameCover.Application" version="1.0.0.0"/>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage>
    </windowsSettings>
  </application>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
</assembly>
"#;

fn main() {
    slint_build::compile("ui/app.slint").expect("Slint UI failed to compile");
    embed_ffmpeg();

    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=../../icon.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../icon.ico")
            .set("FileDescription", "FrameCover")
            .set("ProductName", "FrameCover")
            .set("CompanyName", "Fedor Gagarin")
            .set(
                "LegalCopyright",
                "Copyright (c) Fedor Gagarin. MIT License.",
            )
            .set_manifest(WINDOWS_MANIFEST);
        if let Err(err) = res.compile() {
            println!("cargo:warning=could not embed Windows resources: {err}");
        }
    }
}

/// When `FRAMECOVER_EMBED_FFMPEG_DIR` names a directory holding `ffmpeg` and
/// `ffprobe` for the target, compress both into one solid xz stream and
/// generate `embedded_ffmpeg.rs` describing it (see `src/embedded.rs`).
/// The two executables share most of their code, so a dictionary large
/// enough to span both halves the payload (~330 MB to ~50 MB for FFmpeg 8).
fn embed_ffmpeg() {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use std::path::PathBuf;

    println!("cargo:rerun-if-env-changed=FRAMECOVER_EMBED_FFMPEG_DIR");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let generated = out_dir.join("embedded_ffmpeg.rs");
    let Some(dir) = std::env::var_os("FRAMECOVER_EMBED_FFMPEG_DIR").filter(|d| !d.is_empty())
    else {
        std::fs::write(&generated, "pub const EMBEDDED: Option<Embedded> = None;\n")
            .expect("write embedded_ffmpeg.rs");
        return;
    };
    let dir = PathBuf::from(dir);
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let suffix = if windows { ".exe" } else { "" };
    let names = [format!("ffmpeg{suffix}"), format!("ffprobe{suffix}")];

    let mut payload = Vec::new();
    let mut sizes = Vec::new();
    for name in &names {
        let path = dir.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = std::fs::read(&path).unwrap_or_else(|e| {
            panic!(
                "FRAMECOVER_EMBED_FFMPEG_DIR: cannot read {}: {e}",
                path.display()
            )
        });
        sizes.push(bytes.len());
        payload.extend_from_slice(&bytes);
    }
    let digest = hex::encode(Sha256::digest(&payload));

    let xz_path = out_dir.join("ffmpeg.bin.xz");
    let stamp = out_dir.join("ffmpeg.bin.xz.sha256");
    // Compression takes minutes; reuse the previous output for the same input.
    if std::fs::read_to_string(&stamp).ok().as_deref() != Some(digest.as_str()) {
        let mut options = liblzma::stream::LzmaOptions::new_preset(6).expect("lzma preset");
        options.dict_size(256 << 20);
        let mut filters = liblzma::stream::Filters::new();
        filters.lzma2(&options);
        let stream =
            liblzma::stream::Stream::new_stream_encoder(&filters, liblzma::stream::Check::Crc64)
                .expect("xz encoder");
        let file = std::fs::File::create(&xz_path).expect("create ffmpeg.bin.xz");
        let mut encoder =
            liblzma::write::XzEncoder::new_stream(std::io::BufWriter::new(file), stream);
        encoder.write_all(&payload).expect("compress FFmpeg");
        encoder
            .finish()
            .expect("finish xz")
            .flush()
            .expect("flush xz");
        std::fs::write(&stamp, &digest).expect("write stamp");
    }

    let code = format!(
        "pub const EMBEDDED: Option<Embedded> = Some(Embedded {{\n    \
         xz: include_bytes!(r\"{}\"),\n    \
         files: &[(\"{}\", {}), (\"{}\", {})],\n    \
         sha256: \"{digest}\",\n}});\n",
        xz_path.display(),
        names[0],
        sizes[0],
        names[1],
        sizes[1],
    );
    std::fs::write(&generated, code).expect("write embedded_ffmpeg.rs");
}
