//! `fetch-ffmpeg`: download pinned FFmpeg builds and extract ffmpeg/ffprobe.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::util::{Result, bail, root};

/// Platforms with pins in `xtask/ffmpeg-pins.toml`.
pub const PLATFORMS: [&str; 5] = [
    "windows-x64",
    "linux-x64",
    "linux-arm64",
    "macos-arm64",
    "macos-x64",
];

/// Pinned archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archive {
    pub url: String,
    pub sha256: String,
}

/// `vendor/ffmpeg/<platform>`.
pub fn vendor_dir(platform: &str) -> PathBuf {
    root().join("vendor").join("ffmpeg").join(platform)
}

/// File names of the two tools for `platform`.
pub fn tool_names(platform: &str) -> [String; 2] {
    let ext = if platform.starts_with("windows") {
        ".exe"
    } else {
        ""
    };
    [format!("ffmpeg{ext}"), format!("ffprobe{ext}")]
}

/// Pin platforms needed on this host (macOS needs both for a universal build).
pub fn host_platforms() -> Result<Vec<&'static str>> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => vec!["windows-x64"],
        ("linux", "x86_64") => vec!["linux-x64"],
        ("linux", "aarch64") => vec!["linux-arm64"],
        ("macos", _) => vec!["macos-arm64", "macos-x64"],
        (os, arch) => bail!("no pinned FFmpeg build for {os}/{arch}"),
    })
}

pub fn load_pins(text: &str) -> Result<Vec<(String, Vec<Archive>)>> {
    let doc: toml_edit::DocumentMut = text.parse()?;
    let mut out = Vec::new();
    for (platform, item) in doc.iter() {
        let Some(array) = item.get("archives").and_then(|a| a.as_array()) else {
            bail!("[{platform}] needs an `archives` array");
        };
        let mut archives = Vec::new();
        for entry in array.iter() {
            let table = entry
                .as_inline_table()
                .ok_or_else(|| format!("[{platform}] archives must be inline tables"))?;
            let get = |k: &str| {
                table
                    .get(k)
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .ok_or_else(|| format!("[{platform}] archive missing `{k}`"))
            };
            let archive = Archive {
                url: get("url")?,
                sha256: get("sha256")?.to_ascii_lowercase(),
            };
            if archive.sha256.len() != 64 || !archive.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            {
                bail!("[{platform}] invalid sha256 for {}", archive.url);
            }
            if !archive.url.starts_with("https://") {
                bail!("[{platform}] only https URLs are allowed: {}", archive.url);
            }
            archives.push(archive);
        }
        out.push((platform.to_owned(), archives));
    }
    Ok(out)
}

pub fn fetch(requested: &[String], force: bool) -> Result<()> {
    let pins = load_pins(&std::fs::read_to_string(
        root().join("xtask/ffmpeg-pins.toml"),
    )?)?;
    let platforms: Vec<String> = if requested.is_empty() {
        host_platforms()?.into_iter().map(String::from).collect()
    } else if requested.iter().any(|p| p == "all") {
        pins.iter().map(|(p, _)| p.clone()).collect()
    } else {
        requested.to_vec()
    };
    for platform in &platforms {
        let Some((_, archives)) = pins.iter().find(|(p, _)| p == platform) else {
            bail!(
                "no pin for platform {platform:?} (known: {})",
                PLATFORMS.join(", ")
            );
        };
        fetch_platform(platform, archives, force)?;
    }
    if let [platform] = platforms.as_slice() {
        let dir = vendor_dir(platform);
        let [ffmpeg, ffprobe] = tool_names(platform);
        println!("To use these binaries for tests:");
        println!("  FRAMECOVER_FFMPEG={}", dir.join(ffmpeg).display());
        println!("  FRAMECOVER_FFPROBE={}", dir.join(ffprobe).display());
    }
    Ok(())
}

fn fetch_platform(platform: &str, archives: &[Archive], force: bool) -> Result<()> {
    let dir = vendor_dir(platform);
    let stamp_path = dir.join(".pins");
    let stamp: String = archives
        .iter()
        .map(|a| format!("{}  {}\n", a.sha256, a.url))
        .collect();
    let names = tool_names(platform);
    let present = names.iter().all(|n| dir.join(n).is_file());
    if !force && present && std::fs::read_to_string(&stamp_path).ok().as_deref() == Some(&stamp) {
        println!("{platform}: up to date ({})", dir.display());
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    let cache = root().join("vendor").join("ffmpeg").join(".cache");
    std::fs::create_dir_all(&cache)?;

    let mut found = Vec::new();
    for archive in archives {
        let file = download_verified(archive, &cache)?;
        found.extend(extract_tools(&file, &names, &dir)?);
    }
    for name in &names {
        if !found.contains(name) {
            bail!("{platform}: {name} not found in the pinned archives");
        }
    }
    std::fs::write(&stamp_path, stamp)?;
    println!(
        "{platform}: installed {} into {}",
        names.join(", "),
        dir.display()
    );
    Ok(())
}

/// Download `archive` into `cache` (reusing a verified copy) and return its path.
fn download_verified(archive: &Archive, cache: &Path) -> Result<PathBuf> {
    let base = archive.url.rsplit('/').next().unwrap_or("archive");
    let dest = cache.join(format!("{}-{base}", &archive.sha256[..16]));
    if dest.is_file() && sha256_file(&dest)? == archive.sha256 {
        return Ok(dest);
    }
    println!("Downloading {}", archive.url);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .user_agent("framecover-xtask")
        .build()
        .into();
    let response = agent
        .get(&archive.url)
        .call()
        .map_err(|e| format!("download of {} failed: {e}", archive.url))?;
    let mut reader = response.into_body().into_reader();
    let part = dest.with_extension("part");
    let mut out = BufWriter::new(File::create(&part)?);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])?;
    }
    out.flush()?;
    drop(out);
    let actual = hex::encode(hasher.finalize());
    if actual != archive.sha256 {
        let _ = std::fs::remove_file(&part);
        bail!(
            "SHA-256 mismatch for {}\n  expected {}\n  actual   {actual}",
            archive.url,
            archive.sha256
        );
    }
    std::fs::rename(&part, &dest)?;
    Ok(dest)
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = BufReader::new(File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Extract entries whose file name is one of `names` into `dest`; returns
/// the names extracted.
pub fn extract_tools(archive: &Path, names: &[String], dest: &Path) -> Result<Vec<String>> {
    let file_name = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let mut found = Vec::new();
    let mut install = |name: &str, reader: &mut dyn Read| -> Result<()> {
        let target = dest.join(name);
        let tmp = dest.join(format!(".{name}.tmp"));
        {
            let mut out = BufWriter::new(File::create(&tmp)?);
            std::io::copy(reader, &mut out)?;
            out.flush()?;
        }
        set_executable(&tmp)?;
        std::fs::rename(&tmp, &target)?;
        found.push(name.to_owned());
        Ok(())
    };
    let wanted = |path: &str| -> Option<String> {
        let base = path.rsplit(['/', '\\']).next()?;
        names.iter().find(|n| n.as_str() == base).cloned()
    };

    if file_name.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(BufReader::new(File::open(archive)?))?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            let path = entry.name()?.into_owned();
            if let Some(name) = wanted(&path) {
                install(&name, &mut entry)?;
            }
        }
    } else if file_name.ends_with(".tar.xz") {
        // Decompress to a temporary tar next to the archive, then scan it.
        let tar_path = archive.with_extension("tar.tmp");
        {
            let mut input = BufReader::new(File::open(archive)?);
            let mut output = BufWriter::new(File::create(&tar_path)?);
            lzma_rs::xz_decompress(&mut input, &mut output)
                .map_err(|e| format!("xz decompression of {} failed: {e}", archive.display()))?;
            output.flush()?;
        }
        let result = (|| -> Result<()> {
            let mut tar = tar::Archive::new(BufReader::new(File::open(&tar_path)?));
            for entry in tar.entries()? {
                let mut entry = entry?;
                if !entry.header().entry_type().is_file() {
                    continue;
                }
                let path = entry.path()?.to_string_lossy().into_owned();
                if let Some(name) = wanted(&path) {
                    install(&name, &mut entry)?;
                }
            }
            Ok(())
        })();
        let _ = std::fs::remove_file(&tar_path);
        result?;
    } else {
        bail!("unsupported archive format: {file_name}");
    }
    Ok(found)
}

#[cfg(unix)]
pub fn set_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
pub fn set_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn pins_file_is_valid_and_complete() {
        let pins =
            load_pins(&std::fs::read_to_string(root().join("xtask/ffmpeg-pins.toml")).unwrap())
                .unwrap();
        for platform in PLATFORMS {
            let (_, archives) = pins.iter().find(|(p, _)| p == platform).unwrap();
            assert!(!archives.is_empty(), "{platform}");
            for a in archives {
                assert!(
                    !a.url.contains("/latest/"),
                    "pins must not float: {}",
                    a.url
                );
            }
        }
    }

    #[test]
    fn rejects_bad_pins() {
        assert!(load_pins("[x]\narchives = [{ url = \"https://a\", sha256 = \"zz\" }]").is_err());
        let sha = "a".repeat(64);
        assert!(
            load_pins(&format!(
                "[x]\narchives = [{{ url = \"http://a\", sha256 = \"{sha}\" }}]"
            ))
            .is_err()
        );
        assert!(load_pins("[x]\nurl = 1").is_err());
    }

    #[test]
    fn extracts_tools_from_zip_regardless_of_directory() {
        let tmp = tempdir();
        let zip_path = tmp.join("build.zip");
        {
            let mut zip = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("ffmpeg-x/bin/ffmpeg.exe", opts).unwrap();
            zip.write_all(b"FF").unwrap();
            zip.start_file("ffmpeg-x/bin/ffprobe.exe", opts).unwrap();
            zip.write_all(b"FP").unwrap();
            zip.start_file("ffmpeg-x/bin/ffplay.exe", opts).unwrap();
            zip.write_all(b"no").unwrap();
            zip.finish().unwrap();
        }
        let out = tmp.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let names = tool_names("windows-x64");
        let found = extract_tools(&zip_path, &names, &out).unwrap();
        assert_eq!(found, names.to_vec());
        assert_eq!(std::fs::read(out.join("ffmpeg.exe")).unwrap(), b"FF");
        assert!(!out.join("ffplay.exe").exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn sha256_matches_known_vector() {
        let tmp = tempdir();
        let f = tmp.join("abc");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(
            sha256_file(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    pub(crate) fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "xtask-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
