//! FFmpeg binaries embedded into the executable at build time (Windows
//! single-file release). They are unpacked once per FFmpeg build into the
//! user's local data directory and registered with the core's binary lookup.
//!
//! Unpacking takes a couple of seconds on first launch, so it runs on a
//! background thread; core lookups block on it rather than falling through
//! to a system FFmpeg.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Compressed `ffmpeg` + `ffprobe`, concatenated in `files` order.
pub struct Embedded {
    pub xz: &'static [u8],
    /// `(file name, uncompressed size)` of each binary.
    pub files: &'static [(&'static str, usize)],
    /// SHA-256 of the uncompressed payload; names the unpack directory.
    pub sha256: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/embedded_ffmpeg.rs"));

static UNPACKED: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    let embedded = EMBEDDED.as_ref()?;
    let root = dirs::data_local_dir()?.join("FrameCover").join("ffmpeg");
    match unpack(embedded, &root) {
        Ok(dir) => Some(dir),
        Err(err) => {
            log::error!(
                "could not unpack the bundled FFmpeg into {}: {err}",
                root.display()
            );
            None
        }
    }
});

/// Start unpacking (if this build embeds FFmpeg) and register the result
/// with the core's FFmpeg lookup.
pub fn install() {
    if EMBEDDED.is_none() {
        return;
    }
    framecover_core::ffmpeg::set_bundled_dir_provider(|| UNPACKED.clone());
    std::thread::spawn(|| LazyLock::force(&UNPACKED));
}

fn unpack(embedded: &Embedded, root: &Path) -> std::io::Result<PathBuf> {
    let dir = root.join(&embedded.sha256[..16]);
    if is_complete(embedded, &dir) {
        return Ok(dir);
    }
    std::fs::create_dir_all(root)?;
    let staging = tempfile::Builder::new()
        .prefix(".unpack-")
        .tempdir_in(root)?;
    let mut decoder = liblzma::read::XzDecoder::new(embedded.xz);
    for (name, size) in embedded.files {
        let path = staging.path().join(name);
        let mut file = std::fs::File::create(&path)?;
        let copied = std::io::copy(&mut (&mut decoder).take(*size as u64), &mut file)?;
        if copied != *size as u64 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("embedded {name} is truncated"),
            ));
        }
        file.sync_all()?;
        set_executable(&path)?;
    }
    if dir.exists() {
        // An incomplete copy (e.g. a file deleted by the user) would block the rename.
        let _ = std::fs::remove_dir_all(&dir);
    }
    match std::fs::rename(staging.path(), &dir) {
        Ok(()) => {
            // The directory now lives at `dir`; don't let the guard delete it.
            let _ = staging.keep();
        }
        // Another FrameCover instance won the race; use its copy.
        Err(_) if is_complete(embedded, &dir) => {}
        Err(err) => return Err(err),
    }
    remove_stale(root, &dir);
    Ok(dir)
}

fn is_complete(embedded: &Embedded, dir: &Path) -> bool {
    embedded.files.iter().all(|(name, size)| {
        std::fs::metadata(dir.join(name)).is_ok_and(|m| m.is_file() && m.len() == *size as u64)
    })
}

/// Delete unpacked copies of other FFmpeg builds (best effort: a copy still
/// in use by a running older instance simply stays until next time).
fn remove_stale(root: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path != keep && path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

#[cfg(unix)]
fn set_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn embedded(payload: &[u8], files: &'static [(&'static str, usize)]) -> Embedded {
        let mut encoder = liblzma::write::XzEncoder::new(Vec::new(), 6);
        encoder.write_all(payload).unwrap();
        let xz: &'static [u8] = Box::leak(encoder.finish().unwrap().into_boxed_slice());
        Embedded {
            xz,
            files,
            sha256: "0123456789abcdef0123",
        }
    }

    #[test]
    fn unpacks_once_and_splits_files() {
        let root = tempfile::tempdir().unwrap();
        let e = embedded(b"FFMPEGprobe!", &[("ffmpeg.exe", 6), ("ffprobe.exe", 6)]);
        let dir = unpack(&e, root.path()).unwrap();
        assert_eq!(dir, root.path().join("0123456789abcdef"));
        assert_eq!(std::fs::read(dir.join("ffmpeg.exe")).unwrap(), b"FFMPEG");
        assert_eq!(std::fs::read(dir.join("ffprobe.exe")).unwrap(), b"probe!");
        // A second call reuses the existing copy.
        std::fs::write(dir.join("marker"), b"x").unwrap();
        assert_eq!(unpack(&e, root.path()).unwrap(), dir);
        assert!(dir.join("marker").exists());
    }

    #[test]
    fn replaces_incomplete_copy_and_removes_stale_builds() {
        let root = tempfile::tempdir().unwrap();
        let stale = root.path().join("ffffffffffffffff");
        std::fs::create_dir_all(&stale).unwrap();
        let e = embedded(b"abcdef", &[("ffmpeg", 3), ("ffprobe", 3)]);
        let partial = root.path().join("0123456789abcdef");
        std::fs::create_dir_all(&partial).unwrap();
        std::fs::write(partial.join("ffmpeg"), b"a").unwrap();
        let dir = unpack(&e, root.path()).unwrap();
        assert_eq!(std::fs::read(dir.join("ffmpeg")).unwrap(), b"abc");
        assert_eq!(std::fs::read(dir.join("ffprobe")).unwrap(), b"def");
        assert!(!stale.exists());
    }

    #[test]
    fn truncated_payload_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        let e = embedded(b"abc", &[("ffmpeg", 3), ("ffprobe", 3)]);
        assert!(unpack(&e, root.path()).is_err());
        assert!(!root.path().join("0123456789abcdef").exists());
    }
}
