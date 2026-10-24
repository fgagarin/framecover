//! Atomic file replacement (port of `core/atomic_write.py`).
//!
//! The temp file is created in the *same directory* as the target so the final
//! rename stays on one filesystem (`rename(2)` on POSIX, `MoveFileExW` with
//! `MOVEFILE_REPLACE_EXISTING` on Windows). If any step fails, the target is
//! left untouched and the temp file is removed.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use crate::error::Result;

thread_local! {
    static FAIL_REPLACE: Cell<bool> = const { Cell::new(false) };
}

/// Test seam: run `f` with every final rename on this thread failing with
/// `"simulated replace failure"` (the Python tests patch `os.replace`).
#[doc(hidden)]
pub fn with_simulated_replace_failure<R>(f: impl FnOnce() -> R) -> R {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            FAIL_REPLACE.with(|c| c.set(self.0));
        }
    }
    let _reset = Reset(FAIL_REPLACE.with(|c| c.replace(true)));
    f()
}

/// `os.replace` equivalent honouring the test seam.
pub(crate) fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    if FAIL_REPLACE.with(Cell::get) {
        return Err(std::io::Error::other("simulated replace failure"));
    }
    std::fs::rename(from, to)
}

/// Absolute form of `path`, resolving a symlinked target like `Path.resolve()`.
fn resolve(path: &Path) -> Result<PathBuf> {
    let abs = std::path::absolute(path)?;
    if abs
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Ok(std::fs::canonicalize(&abs)?);
    }
    Ok(abs)
}

/// Atomically replace `target` using `write_fn`.
///
/// 1. Create `.vt_*.tmp` in the target's directory.
/// 2. If `copy_existing` and the target exists, copy it into the temp file.
/// 3. Call `write_fn(temp)`.
/// 4. Rename the temp file over the target.
///
/// On any error the temp file is deleted and the error returned; `target` is
/// never modified in that case.
pub fn atomic_replace(
    target: &Path,
    write_fn: impl FnOnce(&Path) -> Result<()>,
    copy_existing: bool,
) -> Result<()> {
    let abs_target = resolve(target)?;
    let directory = abs_target
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let tmp = tempfile::Builder::new()
        .prefix(".vt_")
        .suffix(".tmp")
        .rand_bytes(8)
        .tempfile_in(&directory)?;
    // Close our handle (Windows would otherwise block ffmpeg/rename) but keep
    // the path; `TempPath` deletes it on drop unless persisted.
    let tmp_path = tmp.into_temp_path();

    let outcome = (|| -> Result<()> {
        if copy_existing && abs_target.exists() {
            std::fs::copy(&abs_target, &tmp_path)?;
        }
        write_fn(&tmp_path)?;
        replace(&tmp_path, &abs_target)?;
        Ok(())
    })();

    match outcome {
        Ok(()) => {
            // Already renamed away: forget the path instead of deleting it.
            let _ = tmp_path.keep();
            Ok(())
        }
        Err(err) => {
            let _ = tmp_path.close();
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".vt_") && n.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn success_writes_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.dat");
        std::fs::write(&target, b"original").unwrap();
        atomic_replace(&target, |p| Ok(std::fs::write(p, b"updated")?), true).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"updated");
    }

    #[test]
    fn copies_existing_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.dat");
        std::fs::write(&target, b"original").unwrap();
        atomic_replace(
            &target,
            |p| {
                assert_eq!(std::fs::read(p)?, b"original");
                Ok(())
            },
            true,
        )
        .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
    }

    #[test]
    fn can_skip_copying_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.dat");
        std::fs::write(&target, b"original").unwrap();
        atomic_replace(
            &target,
            |p| {
                assert_eq!(std::fs::read(p)?, b"");
                Ok(std::fs::write(p, b"updated")?)
            },
            false,
        )
        .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"updated");
    }

    #[test]
    fn failure_leaves_target_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.dat");
        std::fs::write(&target, b"original").unwrap();
        let err = atomic_replace(
            &target,
            |_| Err(Error::Other("disk full simulation".into())),
            true,
        )
        .unwrap_err();
        assert!(err.to_string().contains("disk full"));
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
    }

    #[test]
    fn failure_cleans_up_temp() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.dat");
        std::fs::write(&target, b"original").unwrap();
        let result = atomic_replace(
            &target,
            |p| {
                std::fs::write(p, b"partial")?;
                Err(Error::Other("interleaved failure".into()))
            },
            true,
        );
        assert!(result.is_err());
        assert_eq!(leftovers(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn simulated_replace_failure_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.dat");
        std::fs::write(&target, b"original").unwrap();
        let result = with_simulated_replace_failure(|| {
            atomic_replace(&target, |p| Ok(std::fs::write(p, b"new")?), true)
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("simulated replace failure")
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
        assert!(leftovers(dir.path()).is_empty());
        // The seam is scoped.
        atomic_replace(&target, |p| Ok(std::fs::write(p, b"new")?), true).unwrap();
    }

    #[test]
    fn temp_in_same_directory() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.dat");
        std::fs::write(&target, b"original").unwrap();
        let mut seen = None;
        atomic_replace(
            &target,
            |p| {
                seen = Some(p.parent().unwrap().to_path_buf());
                let name = p.file_name().unwrap().to_string_lossy();
                assert!(name.starts_with(".vt_") && name.ends_with(".tmp"));
                Ok(std::fs::write(p, b"updated")?)
            },
            true,
        )
        .unwrap();
        assert_eq!(
            std::fs::canonicalize(seen.unwrap()).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
    }

    #[test]
    fn creates_target_if_missing() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("new_file.dat");
        atomic_replace(&target, |p| Ok(std::fs::write(p, b"brand new")?), true).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"brand new");
    }
}
