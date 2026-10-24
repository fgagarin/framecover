//! macOS Quick Look / Finder thumbnail cache invalidator.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use image::RgbImage;

use super::{CacheInvalidator, run_with_timeout};

/// Clears the Quick Look cache, bumps the file's mtime and re-imports it
/// into Spotlight so Finder regenerates the icon preview.
#[derive(Debug, Clone)]
pub struct MacOSCacheInvalidator {
    /// `qlmanage` executable (replaceable in tests).
    pub qlmanage: PathBuf,
    /// `mdimport` executable (replaceable in tests).
    pub mdimport: PathBuf,
}

impl Default for MacOSCacheInvalidator {
    fn default() -> Self {
        Self {
            qlmanage: PathBuf::from("qlmanage"),
            mdimport: PathBuf::from("mdimport"),
        }
    }
}

const TIMEOUT: Duration = Duration::from_secs(10);

impl CacheInvalidator for MacOSCacheInvalidator {
    fn invalidate(&self, path: &Path, _thumbnail: Option<&RgbImage>) {
        let mut ql = Command::new(&self.qlmanage);
        ql.args(["-r", "cache"]);
        if let Err(e) = run_with_timeout(ql, TIMEOUT) {
            log::warn!("qlmanage failed: {e}");
        }

        if let Err(e) = touch(path) {
            log::warn!(
                "Updating the modification time failed for '{}': {e}",
                path.display()
            );
        }

        let mut md = Command::new(&self.mdimport);
        md.arg(path);
        if let Err(e) = run_with_timeout(md, TIMEOUT) {
            log::warn!("mdimport failed for '{}': {e}", path.display());
        }
    }
}

/// Set the file's modification time to now (like `os.utime(path, None)`).
fn touch(path: &Path) -> std::io::Result<()> {
    let file = std::fs::OpenOptions::new().write(true).open(path)?;
    file.set_modified(SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subprocess_failure_does_not_raise() {
        let tmp = tempfile::tempdir().unwrap();
        let video = tmp.path().join("test.mp4");
        std::fs::write(&video, b"fake").unwrap();
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&video)
            .unwrap()
            .set_modified(old)
            .unwrap();

        let invalidator = MacOSCacheInvalidator {
            qlmanage: "framecover-missing-qlmanage".into(),
            mdimport: "framecover-missing-mdimport".into(),
        };
        invalidator.invalidate(&video, None); // must not panic

        let mtime = std::fs::metadata(&video).unwrap().modified().unwrap();
        assert!(mtime > old, "mtime should be bumped");
        // Contents untouched.
        assert_eq!(std::fs::read(&video).unwrap(), b"fake");
    }

    #[test]
    fn missing_file_does_not_raise() {
        let tmp = tempfile::tempdir().unwrap();
        MacOSCacheInvalidator {
            qlmanage: "framecover-missing-qlmanage".into(),
            mdimport: "framecover-missing-mdimport".into(),
        }
        .invalidate(&tmp.path().join("gone.mp4"), None);
    }
}
