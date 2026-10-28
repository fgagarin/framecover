//! Copy-first saving with SHA-256 receipts (port of `core/save_copy.py`).
//!
//! The source video is never modified. The thumbnail is written into a staged
//! copy next to the destination, which is published only on success:
//!
//! * without `overwrite`, publication is exclusive (hardlink, or `O_EXCL`
//!   copy + fsync on filesystems without hardlinks), so a racing file is
//!   never clobbered;
//! * an existing destination is reused only when its receipt
//!   (`.<name>.video-thumbnailer.json`) proves it is an earlier output for the
//!   same source content; it is captured in a private backup first and
//!   restored if anything goes wrong (or retained and reported).

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use image::RgbImage;
use log::warn;
use sha2::{Digest, Sha256};

use crate::models::{ApplyError, ApplyResult, ProgressFn, VideoFile};
use crate::thumbnail_writer::ThumbnailWriter;

/// Suggested copy destination: `<stem>-thumbnail<ext>` next to `source`.
///
/// The UI also applies this to a conflicting destination for its
/// "Add another suffix" choice (`a-thumbnail.mp4` → `a-thumbnail-thumbnail.mp4`).
pub fn default_copy_path(source: &Path) -> PathBuf {
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let suffix = source
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    source.with_file_name(format!("{stem}-thumbnail{suffix}"))
}

/// Alias of [`default_copy_path`] for the "Add another suffix" conflict choice.
pub fn next_copy_path(destination: &Path) -> PathBuf {
    default_copy_path(destination)
}

/// Stable on-disk receipt name from before the FrameCover rename.
pub fn receipt_path(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    destination.with_file_name(format!(".{name}.video-thumbnailer.json"))
}

/// Write a thumbnail to a staged copy, publishing only a successful result.
///
/// Existing outputs require explicit `overwrite` or a receipt matching the
/// complete source and output contents. Blocking: call from a worker thread.
/// Expected filesystem and writer failures are returned, never panicked.
pub fn save_copy(
    writer: &dyn ThumbnailWriter,
    video: &VideoFile,
    thumbnail: &RgbImage,
    destination: &Path,
    overwrite: bool,
    position_ms: Option<u64>,
    on_progress: Option<ProgressFn>,
) -> ApplyResult {
    save_copy_with(
        &RealOps,
        writer,
        video,
        thumbnail,
        destination,
        overwrite,
        position_ms,
        on_progress,
    )
}

// ---------------------------------------------------------------------------
// Filesystem seam (the Python tests monkeypatch these functions)
// ---------------------------------------------------------------------------

pub(crate) trait SaveOps {
    fn digest(&self, path: &Path) -> io::Result<String> {
        digest(path)
    }
    fn copy_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        copy_file(from, to)
    }
    fn link(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::hard_link(from, to)
    }
    fn copy_stream(&self, from: &mut File, to: &mut File) -> io::Result<()> {
        io::copy(from, to).map(drop)
    }
    fn replace(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }
    fn publish_exclusive(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn write_receipt(&self, destination: &Path, source: &str, output: &str) -> io::Result<()>;
}

struct RealOps;

impl SaveOps for RealOps {
    fn publish_exclusive(&self, from: &Path, to: &Path) -> io::Result<()> {
        publish_exclusive(self, from, to)
    }
    fn write_receipt(&self, destination: &Path, source: &str, output: &str) -> io::Result<()> {
        write_receipt(self, destination, source, output)
    }
}

fn digest(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
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

/// `shutil.copyfile`: contents only (no permission bits, unlike `fs::copy`).
fn copy_file(from: &Path, to: &Path) -> io::Result<()> {
    let mut src = File::open(from)?;
    let mut dst = File::create(to)?;
    io::copy(&mut src, &mut dst)?;
    dst.flush()
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Python exception classes the save flow distinguishes.
#[derive(Debug)]
enum SaveError {
    /// `FileExistsError` raised by the flow itself.
    Exists(String),
    Io(io::Error),
}

impl From<io::Error> for SaveError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exists(m) => f.write_str(m),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

fn is_permission_error(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::PermissionDenied
        || (cfg!(windows) && matches!(err.raw_os_error(), Some(32 | 33)))
}

fn error_code(err: &SaveError) -> ApplyError {
    match err {
        SaveError::Exists(_) => ApplyError::DestinationExists,
        SaveError::Io(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            ApplyError::DestinationExists
        }
        SaveError::Io(e) if is_permission_error(e) => ApplyError::FileNotWritable,
        SaveError::Io(e) if e.kind() == io::ErrorKind::StorageFull => ApplyError::DiskFull,
        SaveError::Io(_) => ApplyError::Unexpected,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
}

/// `Path.resolve()` (non-strict).
fn resolve(path: &Path) -> PathBuf {
    if let Ok(p) = std::fs::canonicalize(path) {
        return p;
    }
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match (abs.parent(), abs.file_name()) {
        (Some(parent), Some(name)) => std::fs::canonicalize(parent)
            .map(|p| p.join(name))
            .unwrap_or(abs),
        _ => abs,
    }
}

#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let (ma, mb) = (std::fs::metadata(a)?, std::fs::metadata(b)?);
    Ok(ma.dev() == mb.dev() && ma.ino() == mb.ino())
}

#[cfg(windows)]
fn same_file(a: &Path, b: &Path) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    fn id(path: &Path) -> io::Result<(u32, u32, u32)> {
        let file = File::open(path)?;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: valid open handle and a properly sized out-structure.
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((
            info.dwVolumeSerialNumber,
            info.nFileIndexHigh,
            info.nFileIndexLow,
        ))
    }
    Ok(id(a)? == id(b)?)
}

#[cfg(not(any(unix, windows)))]
fn same_file(a: &Path, b: &Path) -> io::Result<bool> {
    Ok(std::fs::canonicalize(a)? == std::fs::canonicalize(b)?)
}

fn refuse_source_alias(source: &Path, destination: &Path) -> Result<(), SaveError> {
    if is_symlink(destination)
        || resolve(source) == resolve(destination)
        || (destination.exists() && same_file(source, destination)?)
    {
        return Err(SaveError::Exists(
            "Destination must not alias the source or be a symlink.".to_owned(),
        ));
    }
    Ok(())
}

/// Python `receipt.get(key) == 1` for a JSON value (`True` and `1.0` count).
fn equals_one(v: Option<&serde_json::Value>) -> bool {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_f64() == Some(1.0),
        Some(serde_json::Value::Bool(b)) => *b,
        _ => false,
    }
}

fn read_receipt(destination: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    let text = std::fs::read(receipt_path(destination)).ok()?;
    let text = String::from_utf8(text).ok()?;
    match serde_json::from_str::<serde_json::Value>(&text).ok()? {
        serde_json::Value::Object(map) => Some(map),
        _ => None,
    }
}

fn str_field<'a>(
    map: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    map.get(key).and_then(serde_json::Value::as_str)
}

fn verified(ops: &dyn SaveOps, destination: &Path, source_digest: &str) -> bool {
    if is_symlink(destination) || !destination.is_file() {
        return false;
    }
    let Some(receipt) = read_receipt(destination) else {
        return false;
    };
    equals_one(receipt.get("version"))
        && str_field(&receipt, "source_sha256") == Some(source_digest)
        && ops
            .digest(destination)
            .is_ok_and(|d| str_field(&receipt, "output_sha256") == Some(d.as_str()))
}

/// Hardlink errors meaning "this filesystem cannot hardlink" (FAT/exFAT,
/// cross-device, missing privilege): EOPNOTSUPP/ENOTSUP/ENOSYS/EPERM/EXDEV,
/// Windows 1 (invalid function), 50 (not supported), 1314 (privilege).
/// On Android SELinux denies apps `link()` everywhere with EACCES.
fn hardlink_unsupported(err: &io::Error) -> bool {
    use io::ErrorKind::{CrossesDevices, Unsupported};
    if matches!(err.kind(), Unsupported | CrossesDevices) {
        return true;
    }
    if cfg!(windows) {
        matches!(err.raw_os_error(), Some(1 | 50 | 1314))
    } else if cfg!(target_os = "android") {
        matches!(err.raw_os_error(), Some(1 | 13))
    } else {
        // EPERM is 1 on every Unix; ENOSYS/EOPNOTSUPP map to `Unsupported`.
        matches!(err.raw_os_error(), Some(1))
    }
}

/// Publish `source` at `destination` without clobbering an existing file:
/// a hardlink, or an exclusive copy where the filesystem cannot hardlink.
pub(crate) fn publish_without_clobbering(source: &Path, destination: &Path) -> io::Result<()> {
    publish_exclusive(&RealOps, source, destination)
}

fn publish_exclusive(ops: &dyn SaveOps, source: &Path, destination: &Path) -> io::Result<()> {
    match ops.link(source, destination) {
        Ok(()) => return Ok(()),
        Err(err) => {
            if !hardlink_unsupported(&err) || is_symlink(source) || !source.is_file() {
                return Err(err);
            }
        }
    }

    // FAT/exFAT cannot publish via hardlink. Exclusive creation still prevents
    // clobbering; unlike a hardlink, readers can see the copy before completion.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(destination)?;
    let copied = (|| -> io::Result<()> {
        let mut original = File::open(source)?;
        ops.copy_stream(&mut original, &mut output)?;
        output.flush()?;
        output.sync_all()
    })();
    // A pathname cleanup could delete a concurrent writer's replacement, so a
    // partial output is retained and reported instead.
    copied.map_err(|exc| {
        io::Error::new(
            exc.kind(),
            format!(
                "Copy failed; possible incomplete output retained at: {}. {exc}",
                destination.display()
            ),
        )
    })
}

fn restore_backup(ops: &dyn SaveOps, backup: &Path, destination: &Path) -> Option<String> {
    if let Err(err) = ops.publish_exclusive(backup, destination) {
        warn!(
            "Previous destination retained at {}: {err}",
            backup.display()
        );
        return Some(format!(
            " Previous destination retained at: {}",
            backup.display()
        ));
    }
    if let Err(err) = std::fs::remove_file(backup) {
        warn!(
            "Restored backup also retained at {}: {err}",
            backup.display()
        );
    }
    None
}

/// Receipt JSON in Python `json.dump` layout.
fn receipt_json(source_digest: &str, output_digest: &str) -> String {
    format!(
        r#"{{"version": 1, "source_sha256": "{source_digest}", "output_sha256": "{output_digest}"}}"#
    )
}

fn write_receipt(
    ops: &dyn SaveOps,
    destination: &Path,
    source_digest: &str,
    output_digest: &str,
) -> io::Result<()> {
    let dir = parent_dir(destination);
    let name = destination
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut tmp = tempfile::Builder::new()
        .prefix(&format!(".{name}.receipt-"))
        .suffix(".json")
        .rand_bytes(8)
        .tempfile_in(&dir)?;
    tmp.write_all(receipt_json(source_digest, output_digest).as_bytes())?;
    tmp.flush()?;
    let tmp = tmp.into_temp_path();
    ops.replace(&tmp, &receipt_path(destination))?;
    let _ = tmp.keep();
    Ok(())
}

fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Run `f` as a progress stage (start/finish reported even on error).
fn stage<T>(on_progress: &mut Option<ProgressFn>, name: &str, f: impl FnOnce() -> T) -> T {
    let Some(cb) = on_progress.as_mut() else {
        return f();
    };
    let started = Instant::now();
    cb(name, None);
    let out = f();
    if let Some(cb) = on_progress.as_mut() {
        cb(name, Some(started.elapsed().as_secs_f64()));
    }
    out
}

// ---------------------------------------------------------------------------
// Flow
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Cleanup {
    temporary: Option<tempfile::TempPath>,
    backup: Option<PathBuf>,
    backup_directory: Option<PathBuf>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn save_copy_with(
    ops: &dyn SaveOps,
    writer: &dyn ThumbnailWriter,
    video: &VideoFile,
    thumbnail: &RgbImage,
    destination: &Path,
    overwrite: bool,
    position_ms: Option<u64>,
    mut on_progress: Option<ProgressFn>,
) -> ApplyResult {
    let started = Instant::now();
    let mut cleanup = Cleanup::default();
    let output = std::path::absolute(destination).unwrap_or_else(|_| destination.to_path_buf());

    let outcome = run_flow(
        ops,
        writer,
        video,
        thumbnail,
        &output,
        overwrite,
        position_ms,
        &mut on_progress,
        &mut cleanup,
    );

    let mut result = match outcome {
        Ok(result) => result,
        Err(err) => {
            let mut recovery = String::new();
            if let Some(backup) = cleanup.backup.take() {
                recovery = restore_backup(ops, &backup, &output).unwrap_or_default();
            }
            ApplyResult::failure(error_code(&err), format!("{err}{recovery}"))
        }
    };
    result.elapsed_ms = started.elapsed().as_millis() as u64;
    if let Some(temporary) = cleanup.temporary.take() {
        let shown = temporary.to_path_buf();
        if let Err(err) = temporary.close()
            && err.kind() != io::ErrorKind::NotFound
        {
            warn!("Could not remove staged copy {}: {err}", shown.display());
        }
    }
    if let Some(dir) = cleanup.backup_directory.take() {
        // A retained backup must remain available for recovery.
        let _ = std::fs::remove_dir(dir);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn run_flow(
    ops: &dyn SaveOps,
    writer: &dyn ThumbnailWriter,
    video: &VideoFile,
    thumbnail: &RgbImage,
    output: &Path,
    overwrite: bool,
    position_ms: Option<u64>,
    on_progress: &mut Option<ProgressFn>,
    cleanup: &mut Cleanup,
) -> Result<ApplyResult, SaveError> {
    let source = video.path.as_path();
    refuse_source_alias(source, output)?;
    let (source_digest, reuse) = stage(on_progress, "Verifying source and destination", || {
        let source_digest = ops.digest(source)?;
        let reuse = output.exists() && !overwrite;
        if reuse && !verified(ops, output, &source_digest) {
            return Err(SaveError::Exists(format!(
                "Destination already exists: {}",
                output.display()
            )));
        }
        Ok((source_digest, reuse))
    })?;

    let stem = output
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let suffix = output
        .extension()
        .or_else(|| source.extension())
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let temporary = tempfile::Builder::new()
        .prefix(&format!(".{stem}.save-"))
        .suffix(&suffix)
        .rand_bytes(8)
        .tempfile_in(parent_dir(output))?
        .into_temp_path();
    let temp_path = temporary.to_path_buf();
    cleanup.temporary = Some(temporary);

    stage(on_progress, "Copying source video", || {
        ops.copy_file(source, &temp_path)?;
        if ops.digest(&temp_path)? != source_digest {
            return Err(SaveError::Io(io::Error::other(
                "Source changed while copying; please try again.",
            )));
        }
        Ok(())
    })?;

    let staged = VideoFile {
        path: temp_path.clone(),
        is_writable: true,
        ..video.clone()
    };
    let mut result = writer.write(
        &staged,
        thumbnail,
        position_ms,
        on_progress.as_mut().map(|cb| &mut **cb as ProgressFn<'_>),
    );
    result.output_path = None;
    if !result.success {
        return Ok(result);
    }

    let output_digest = stage(on_progress, "Verifying saved copy", || {
        let output_digest = ops.digest(&temp_path)?;
        refuse_source_alias(source, output)?;
        if reuse {
            let changed = || {
                SaveError::Exists(format!(
                    "Copy receipt changed while saving: {}",
                    output.display()
                ))
            };
            let receipt = read_receipt(output).ok_or_else(changed)?;
            if !equals_one(receipt.get("version"))
                || str_field(&receipt, "source_sha256") != Some(source_digest.as_str())
            {
                return Err(changed());
            }
            let name = output
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let backup_directory = tempfile::Builder::new()
                .prefix(&format!(".{name}.backup-"))
                .rand_bytes(8)
                .tempdir_in(parent_dir(output))?
                .keep();
            cleanup.backup_directory = Some(backup_directory.clone());
            let captured = backup_directory.join(&name);
            ops.replace(output, &captured)?;
            cleanup.backup = Some(captured.clone());
            refuse_source_alias(source, &captured)?;
            let expected = str_field(&receipt, "output_sha256");
            if !captured.is_file() || Some(ops.digest(&captured)?.as_str()) != expected {
                return Err(SaveError::Exists(format!(
                    "Destination changed while saving: {}",
                    output.display()
                )));
            }
        }
        Ok(output_digest)
    })?;

    if overwrite {
        ops.replace(&temp_path, output)?;
    } else {
        ops.publish_exclusive(&temp_path, output)?;
    }
    result.output_path = Some(output.to_path_buf());
    if let Some(backup) = cleanup.backup.take()
        && let Err(err) = std::fs::remove_file(&backup)
    {
        warn!(
            "Could not remove verified backup {}: {err}",
            backup.display()
        );
    }
    if let Err(err) = ops.write_receipt(output, &source_digest, &output_digest) {
        warn!(
            "Could not save copy receipt for {}: {err}",
            output.display()
        );
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
