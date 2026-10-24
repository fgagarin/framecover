//! Port of `tests/unit/test_save_copy.py`. The Python tests monkeypatch
//! `os.link`, `os.replace`, `_digest`, `shutil.copyfile(obj)`,
//! `_publish_exclusive` and `_write_receipt`; here the same points are hooks
//! on the [`SaveOps`] seam.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::models::VideoFormat;

type PathHook = Box<dyn Fn(&Path, &Path) -> io::Result<()>>;
type DigestHook = Box<dyn Fn(&Path) -> io::Result<String>>;
type StreamHook = Box<dyn Fn(&mut File, &mut File) -> io::Result<()>>;
type PublishHook = Box<dyn Fn(&Hooks, &Path, &Path) -> io::Result<()>>;
type Events = Rc<RefCell<Vec<(String, Option<f64>)>>>;

#[derive(Default)]
struct Hooks {
    link: Option<PathHook>,
    replace: Option<PathHook>,
    copy_file: Option<PathHook>,
    digest: Option<DigestHook>,
    copy_stream: Option<StreamHook>,
    publish: Option<PublishHook>,
    receipt: Option<Box<dyn Fn() -> io::Result<()>>>,
}

impl SaveOps for Hooks {
    fn digest(&self, path: &Path) -> io::Result<String> {
        match &self.digest {
            Some(h) => h(path),
            None => digest(path),
        }
    }
    fn copy_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        match &self.copy_file {
            Some(h) => h(from, to),
            None => copy_file(from, to),
        }
    }
    fn link(&self, from: &Path, to: &Path) -> io::Result<()> {
        match &self.link {
            Some(h) => h(from, to),
            None => std::fs::hard_link(from, to),
        }
    }
    fn copy_stream(&self, from: &mut File, to: &mut File) -> io::Result<()> {
        match &self.copy_stream {
            Some(h) => h(from, to),
            None => io::copy(from, to).map(drop),
        }
    }
    fn replace(&self, from: &Path, to: &Path) -> io::Result<()> {
        match &self.replace {
            Some(h) => h(from, to),
            None => std::fs::rename(from, to),
        }
    }
    fn publish_exclusive(&self, from: &Path, to: &Path) -> io::Result<()> {
        match &self.publish {
            Some(h) => h(self, from, to),
            None => publish_exclusive(self, from, to),
        }
    }
    fn write_receipt(&self, destination: &Path, s: &str, o: &str) -> io::Result<()> {
        match &self.receipt {
            Some(h) => h(),
            None => write_receipt(self, destination, s, o),
        }
    }
}

type WriteFn = Box<dyn FnMut(&VideoFile) -> ApplyResult + Send>;

struct FakeWriter {
    write: Mutex<WriteFn>,
    calls: AtomicUsize,
    last_position: Mutex<Option<u64>>,
}

impl FakeWriter {
    fn set(&self, f: impl FnMut(&VideoFile) -> ApplyResult + Send + 'static) {
        *self.write.lock().unwrap() = Box::new(f);
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ThumbnailWriter for FakeWriter {
    fn write(
        &self,
        video: &VideoFile,
        _thumbnail: &RgbImage,
        position_ms: Option<u64>,
        _on_progress: Option<ProgressFn<'_>>,
    ) -> ApplyResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last_position.lock().unwrap() = position_ms;
        (self.write.lock().unwrap())(video)
    }
    fn reencode_reason(&self, _video: &VideoFile) -> Option<String> {
        None
    }
}

struct Setup {
    _dir: tempfile::TempDir,
    dir: PathBuf,
    source: PathBuf,
    destination: PathBuf,
    video: VideoFile,
    thumbnail: RgbImage,
    writer: FakeWriter,
}

fn default_write(
    source: PathBuf,
    destination: PathBuf,
) -> impl FnMut(&VideoFile) -> ApplyResult + Send {
    move |staged| {
        let path = &staged.path;
        assert_eq!(path.parent(), destination.parent());
        assert_eq!(path.extension().unwrap(), "mp4");
        assert!(path != &source && path != &destination);
        assert_eq!(
            std::fs::read(path).unwrap(),
            std::fs::read(&source).unwrap()
        );
        assert!(staged.is_writable);
        std::fs::write(path, b"modified video contents").unwrap();
        ApplyResult {
            success: true,
            duration_shift_ms: 40,
            ..ApplyResult::default()
        }
    }
}

fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(tmp.path()).unwrap();
    let source = dir.join("source.mp4");
    std::fs::write(&source, b"original video contents").unwrap();
    let destination = dir.join("saved.mp4");
    let video = VideoFile {
        path: source.clone(),
        format: VideoFormat::Mp4,
        duration_ms: 5000,
        width: 320,
        height: 240,
        existing_thumbnail: None,
        is_writable: false,
        frame_step_ms: 40,
        thumbnail_frame_number: None,
        thumbnail_position_ms: None,
    };
    let writer = FakeWriter {
        write: Mutex::new(Box::new(default_write(source.clone(), destination.clone()))),
        calls: AtomicUsize::new(0),
        last_position: Mutex::new(None),
    };
    Setup {
        _dir: tmp,
        dir,
        source,
        destination,
        video,
        thumbnail: RgbImage::new(10, 10),
        writer,
    }
}

impl Setup {
    fn save_with(
        &self,
        ops: &dyn SaveOps,
        overwrite: bool,
        position_ms: Option<u64>,
    ) -> ApplyResult {
        save_copy_with(
            ops,
            &self.writer,
            &self.video,
            &self.thumbnail,
            &self.destination,
            overwrite,
            position_ms,
            None,
        )
    }
    fn save(&self) -> ApplyResult {
        self.save_with(&Hooks::default(), false, None)
    }
    fn save_overwrite(&self) -> ApplyResult {
        self.save_with(&Hooks::default(), true, None)
    }
    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
    fn backups(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(&self.dir).unwrap() {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            if name.starts_with('.') && name.contains(".backup-") && p.is_dir() {
                for inner in std::fs::read_dir(&p).unwrap() {
                    out.push(inner.unwrap().path());
                }
            }
        }
        out
    }
    fn backup_dirs(&self) -> usize {
        self.names()
            .iter()
            .filter(|n| n.contains(".backup-"))
            .count()
    }
}

fn read(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap()
}

fn eopnotsupp() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "Hard links are not supported")
}

fn enospc() -> io::Error {
    io::Error::new(io::ErrorKind::StorageFull, "disk full")
}

fn in_backup_dir(p: &Path) -> bool {
    p.parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n.to_string_lossy().contains(".backup-"))
}

#[test]
fn success_preserves_source_and_verifies_repeat() {
    let s = setup();
    let result = s.save_with(&Hooks::default(), false, Some(120));
    assert!(result.success, "{:?}", result.error_message);
    assert_eq!(result.output_path.as_deref(), Some(s.destination.as_path()));
    assert_eq!(result.duration_shift_ms, 40);
    assert_eq!(read(&s.destination), b"modified video contents");
    assert_eq!(read(&s.source), b"original video contents");
    assert_eq!(*s.writer.last_position.lock().unwrap(), Some(120));
    let receipt_text = std::fs::read_to_string(receipt_path(&s.destination)).unwrap();
    let receipt: serde_json::Value = serde_json::from_str(&receipt_text).unwrap();
    assert_eq!(receipt["source_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(receipt["output_sha256"].as_str().unwrap().len(), 64);
    // Python json.dump layout and key order.
    assert!(receipt_text.starts_with(r#"{"version": 1, "source_sha256": ""#));
    assert!(receipt_text.contains(r#"", "output_sha256": ""#));
    assert!(s.save().success);
    assert_eq!(s.writer.calls(), 2);
    assert!(!s.names().iter().any(|n| n.contains(".save-")));
}

#[test]
fn failed_write_leaves_source_and_destination_intact() {
    for existing in [false, true] {
        let s = setup();
        if existing {
            std::fs::write(&s.destination, b"unrelated output").unwrap();
        }
        s.writer.set(|video| {
            std::fs::write(&video.path, b"partial mutation").unwrap();
            ApplyResult::failure(ApplyError::FfmpegError, "writer failed")
        });
        let result = s.save_overwrite();
        assert!(!result.success);
        assert_eq!(result.output_path, None);
        assert_eq!(result.error_code, Some(ApplyError::FfmpegError));
        assert_eq!(read(&s.source), b"original video contents");
        if existing {
            assert_eq!(read(&s.destination), b"unrelated output");
            assert_eq!(s.names(), ["saved.mp4", "source.mp4"]);
        } else {
            assert!(!s.destination.exists());
            assert_eq!(s.names(), ["source.mp4"]);
        }
    }
}

#[test]
fn unrelated_output_requires_explicit_overwrite() {
    for receipt in [
        None,
        Some("{"),
        Some("[]"),
        Some("null"),
        Some(r#"{"version": 1}"#),
        Some(r#"{"version": 2}"#),
    ] {
        let s = setup();
        std::fs::write(&s.destination, b"unrelated output").unwrap();
        if let Some(content) = receipt {
            std::fs::write(receipt_path(&s.destination), content).unwrap();
        }
        let result = s.save();
        assert_eq!(result.error_code, Some(ApplyError::DestinationExists));
        assert!(!result.success);
        assert_eq!(read(&s.destination), b"unrelated output");
        assert_eq!(s.writer.calls(), 0);
        assert!(s.save_overwrite().success);
        assert_eq!(read(&s.source), b"original video contents");
    }
}

#[test]
fn stale_receipt_rejects_same_size_content_changes() {
    for changed_source in [true, false] {
        let s = setup();
        assert!(s.save().success);
        let path = if changed_source {
            &s.source
        } else {
            &s.destination
        };
        let len = read(path).len();
        std::fs::write(path, vec![b'x'; len]).unwrap();
        let calls = s.writer.calls();
        assert_eq!(s.save().error_code, Some(ApplyError::DestinationExists));
        assert_eq!(s.writer.calls(), calls);
        assert_eq!(read(path), vec![b'x'; len]);
    }
}

#[test]
fn refuses_source_aliases() {
    for alias in ["same", "hardlink", "symlink", "relative"] {
        for overwrite in [false, true] {
            let s = setup();
            let destination = match alias {
                "same" => s.source.clone(),
                "relative" => s.dir.join(".").join("source.mp4"),
                "hardlink" => {
                    std::fs::hard_link(&s.source, &s.destination).unwrap();
                    s.destination.clone()
                }
                _ => {
                    #[cfg(unix)]
                    let made = std::os::unix::fs::symlink(&s.source, &s.destination);
                    #[cfg(windows)]
                    let made = std::os::windows::fs::symlink_file(&s.source, &s.destination);
                    if made.is_err() {
                        eprintln!("symlinks not permitted; skipping symlink alias");
                        continue;
                    }
                    s.destination.clone()
                }
            };
            let result = save_copy(
                &s.writer,
                &s.video,
                &s.thumbnail,
                &destination,
                overwrite,
                None,
                None,
            );
            assert_eq!(
                result.error_code,
                Some(ApplyError::DestinationExists),
                "{alias} overwrite={overwrite}"
            );
            assert_eq!(read(&s.source), b"original video contents");
            assert_eq!(s.writer.calls(), 0);
        }
    }
}

#[test]
fn no_clobber_publication_catches_racing_destination() {
    let s = setup();
    let mut inner = default_write(s.source.clone(), s.destination.clone());
    let dest = s.destination.clone();
    s.writer.set(move |v| {
        let r = inner(v);
        std::fs::write(&dest, b"racing output").unwrap();
        r
    });
    let result = s.save();
    assert_eq!(result.error_code, Some(ApplyError::DestinationExists));
    assert_eq!(read(&s.destination), b"racing output");
    assert!(!receipt_path(&s.destination).exists());
    assert_eq!(s.names().len(), 2);
}

#[test]
fn verified_output_changed_during_write_is_not_replaced() {
    let s = setup();
    assert!(s.save().success);
    let mut inner = default_write(s.source.clone(), s.destination.clone());
    let dest = s.destination.clone();
    s.writer.set(move |v| {
        let r = inner(v);
        std::fs::write(&dest, b"racing output").unwrap();
        r
    });
    assert_eq!(s.save().error_code, Some(ApplyError::DestinationExists));
    assert_eq!(read(&s.destination), b"racing output");
}

#[test]
fn receipt_failure_warns_and_cannot_authorize_reuse() {
    let s = setup();
    let hooks = Hooks {
        receipt: Some(Box::new(|| Err(io::Error::other("receipt is unwritable")))),
        ..Hooks::default()
    };
    assert!(s.save_with(&hooks, false, None).success);
    assert!(!receipt_path(&s.destination).exists());
    assert_eq!(s.save().error_code, Some(ApplyError::DestinationExists));
}

#[test]
fn copy_failure_cleans_staging_file() {
    let s = setup();
    let hooks = Hooks {
        copy_file: Some(Box::new(|_, _| Err(enospc()))),
        ..Hooks::default()
    };
    let result = s.save_with(&hooks, false, None);
    assert_eq!(result.error_code, Some(ApplyError::DiskFull));
    assert!(!s.destination.exists());
    assert_eq!(s.names(), ["source.mp4"]);
    assert_eq!(s.writer.calls(), 0);
}

#[test]
fn progress_starts_before_hashing_and_copying() {
    let s = setup();
    let events: Events = Rc::default();
    let (e1, e2) = (events.clone(), events.clone());
    let hooks = Hooks {
        digest: Some(Box::new(move |p| {
            let last = e1.borrow().last().cloned().unwrap();
            assert!(last.1.is_none());
            assert!(
                [
                    "Verifying source and destination",
                    "Copying source video",
                    "Verifying saved copy"
                ]
                .contains(&last.0.as_str())
            );
            digest(p)
        })),
        copy_file: Some(Box::new(move |a, b| {
            assert_eq!(
                e2.borrow().last().cloned().unwrap(),
                ("Copying source video".to_owned(), None)
            );
            copy_file(a, b)
        })),
        ..Hooks::default()
    };
    let sink = events.clone();
    let mut cb =
        move |name: &str, elapsed: Option<f64>| sink.borrow_mut().push((name.to_owned(), elapsed));
    let result = save_copy_with(
        &hooks,
        &s.writer,
        &s.video,
        &s.thumbnail,
        &s.destination,
        false,
        None,
        Some(&mut cb),
    );
    assert!(result.success);
    let events = events.borrow();
    let started: Vec<&str> = events
        .iter()
        .filter(|(_, e)| e.is_none())
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(
        started,
        [
            "Verifying source and destination",
            "Copying source video",
            "Verifying saved copy"
        ]
    );
    assert!(events.iter().all(|(_, e)| e.is_none_or(|e| e >= 0.0)));
}

#[test]
fn unsupported_hardlinks_use_exclusive_copy() {
    for failure in [
        eopnotsupp as fn() -> io::Error,
        || io::Error::from(io::ErrorKind::CrossesDevices),
        || {
            if cfg!(windows) {
                io::Error::from_raw_os_error(1314)
            } else {
                io::Error::from_raw_os_error(1)
            }
        },
    ] {
        let s = setup();
        let hooks = Hooks {
            link: Some(Box::new(move |_, _| Err(failure()))),
            ..Hooks::default()
        };
        let result = s.save_with(&hooks, false, None);
        assert!(result.success, "{:?}", result.error_message);
        assert_eq!(result.output_path.as_deref(), Some(s.destination.as_path()));
        assert_eq!(read(&s.destination), b"modified video contents");
        assert_eq!(read(&s.source), b"original video contents");
        assert!(s.save_with(&hooks, false, None).success);
        assert_eq!(s.names().len(), 3);
    }
}

#[test]
fn other_link_errors_are_not_treated_as_unsupported() {
    let s = setup();
    let hooks = Hooks {
        link: Some(Box::new(|_, _| {
            Err(io::Error::from(io::ErrorKind::AlreadyExists))
        })),
        ..Hooks::default()
    };
    let result = s.save_with(&hooks, false, None);
    assert_eq!(result.error_code, Some(ApplyError::DestinationExists));
    assert!(!s.destination.exists());
}

#[test]
fn unsupported_hardlinks_still_refuse_racing_destination() {
    let s = setup();
    let dest = s.destination.clone();
    let hooks = Hooks {
        link: Some(Box::new(move |_, _| {
            std::fs::write(&dest, b"racing output").unwrap();
            Err(eopnotsupp())
        })),
        ..Hooks::default()
    };
    let result = s.save_with(&hooks, false, None);
    assert_eq!(result.error_code, Some(ApplyError::DestinationExists));
    assert_eq!(read(&s.destination), b"racing output");
}

fn partial_copy_hooks() -> Hooks {
    Hooks {
        link: Some(Box::new(|_, _| Err(eopnotsupp()))),
        copy_stream: Some(Box::new(|_, out| {
            out.write_all(b"partial")?;
            Err(enospc())
        })),
        ..Hooks::default()
    }
}

#[test]
fn exclusive_copy_failure_retains_and_reports_partial_output() {
    let s = setup();
    let result = s.save_with(&partial_copy_hooks(), false, None);
    assert_eq!(result.error_code, Some(ApplyError::DiskFull));
    assert_eq!(read(&s.destination), b"partial");
    assert!(
        result
            .error_message
            .unwrap()
            .contains(&s.destination.display().to_string())
    );
    assert_eq!(read(&s.source), b"original video contents");
}

#[test]
fn exclusive_copy_failure_preserves_racing_replacement() {
    let s = setup();
    let dest = s.destination.clone();
    let hooks = Hooks {
        link: Some(Box::new(|_, _| Err(eopnotsupp()))),
        copy_stream: Some(Box::new(move |_, _out| {
            let replacement = dest.with_extension("racing");
            std::fs::write(&replacement, b"racing output")?;
            // Windows cannot replace a file that is still open; Python closes
            // the output first, which the `&mut File` seam cannot express.
            if cfg!(windows) {
                return Err(enospc());
            }
            std::fs::rename(&replacement, &dest)?;
            Err(enospc())
        })),
        ..Hooks::default()
    };
    assert!(!s.save_with(&hooks, false, None).success);
    if cfg!(not(windows)) {
        assert_eq!(read(&s.destination), b"racing output");
    } else {
        // The partial output is retained, never unlinked.
        assert!(s.destination.exists());
    }
}

#[test]
fn verified_backup_restored_when_publish_fails() {
    let s = setup();
    assert!(s.save().success);
    let hooks = Hooks {
        publish: Some(Box::new(|ops, from, to| {
            if from
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".save-")
            {
                return Err(enospc());
            }
            publish_exclusive(ops, from, to)
        })),
        ..Hooks::default()
    };
    let result = s.save_with(&hooks, false, None);
    assert_eq!(result.error_code, Some(ApplyError::DiskFull));
    assert_eq!(read(&s.destination), b"modified video contents");
    assert_eq!(s.backup_dirs(), 0);
}

#[test]
fn replacement_during_captured_digest_is_not_clobbered() {
    let s = setup();
    assert!(s.save().success);
    let dest = s.destination.clone();
    let hooks = Hooks {
        digest: Some(Box::new(move |p| {
            let value = digest(p);
            if in_backup_dir(p) {
                std::fs::write(&dest, b"racing unrelated output").unwrap();
            }
            value
        })),
        ..Hooks::default()
    };
    let result = s.save_with(&hooks, false, None);
    assert_eq!(result.error_code, Some(ApplyError::DestinationExists));
    assert_eq!(read(&s.destination), b"racing unrelated output");
    let message = result.error_message.unwrap();
    assert!(message.contains("Previous destination retained at:"));
    let backups = s.backups();
    assert_eq!(backups.len(), 1);
    assert_eq!(read(&backups[0]), b"modified video contents");
    assert!(message.contains(&backups[0].display().to_string()));
}

#[test]
fn unrelated_capture_is_restored_on_digest_mismatch() {
    let s = setup();
    assert!(s.save().success);
    let dest = s.destination.clone();
    let hooks = Hooks {
        replace: Some(Box::new(move |from, to| {
            if in_backup_dir(to) {
                std::fs::write(&dest, b"unrelated replacement").unwrap();
            }
            std::fs::rename(from, to)
        })),
        ..Hooks::default()
    };
    let result = s.save_with(&hooks, false, None);
    assert_eq!(result.error_code, Some(ApplyError::DestinationExists));
    assert_eq!(read(&s.destination), b"unrelated replacement");
    assert_eq!(s.backup_dirs(), 0);
}

#[test]
fn interrupted_fallback_copy_preserves_previous_backup() {
    let s = setup();
    assert!(s.save().success);
    let result = s.save_with(&partial_copy_hooks(), false, None);
    assert_eq!(result.error_code, Some(ApplyError::DiskFull));
    assert_eq!(read(&s.destination), b"partial");
    let message = result.error_message.unwrap();
    assert!(message.contains(&s.destination.display().to_string()));
    let backups = s.backups();
    assert_eq!(backups.len(), 1);
    assert_eq!(read(&backups[0]), b"modified video contents");
    assert!(message.contains(&backups[0].display().to_string()));
    assert_eq!(read(&s.source), b"original video contents");
}

#[test]
fn unrelated_capture_retained_when_racer_blocks_restoration() {
    let s = setup();
    assert!(s.save().success);
    let (d1, d2) = (s.destination.clone(), s.destination.clone());
    let hooks = Hooks {
        replace: Some(Box::new(move |from, to| {
            if in_backup_dir(to) {
                std::fs::write(&d1, b"first unrelated replacement").unwrap();
            }
            std::fs::rename(from, to)
        })),
        digest: Some(Box::new(move |p| {
            let value = digest(p);
            if in_backup_dir(p) {
                std::fs::write(&d2, b"second unrelated replacement").unwrap();
            }
            value
        })),
        ..Hooks::default()
    };
    let result = s.save_with(&hooks, false, None);
    assert_eq!(result.error_code, Some(ApplyError::DestinationExists));
    assert_eq!(read(&s.destination), b"second unrelated replacement");
    let backups = s.backups();
    assert_eq!(backups.len(), 1);
    assert_eq!(read(&backups[0]), b"first unrelated replacement");
    assert!(
        result
            .error_message
            .unwrap()
            .contains(&backups[0].display().to_string())
    );
}

#[cfg(unix)]
#[test]
fn captured_source_symlink_is_restored_or_retained_safely() {
    for blocked_restore in [false, true] {
        let s = setup();
        assert!(s.save().success);
        let racing_link = s.dir.join("racing-source-alias");
        std::os::unix::fs::symlink(&s.source, &racing_link).unwrap();
        let dest = s.destination.clone();
        let hooks = Hooks {
            replace: Some(Box::new(move |from, to| {
                if in_backup_dir(to) {
                    std::fs::rename(&racing_link, &dest)?;
                    std::fs::rename(from, to)?;
                    if blocked_restore {
                        std::fs::write(&dest, b"racing output")?;
                    }
                    Ok(())
                } else {
                    std::fs::rename(from, to)
                }
            })),
            ..Hooks::default()
        };
        let result = s.save_with(&hooks, false, None);
        assert_eq!(result.error_code, Some(ApplyError::DestinationExists));
        assert_eq!(read(&s.source), b"original video contents");
        if !blocked_restore && is_symlink(&s.destination) {
            assert_eq!(std::fs::read_link(&s.destination).unwrap(), s.source);
            assert_eq!(s.backup_dirs(), 0);
        } else {
            let backups = s.backups();
            assert_eq!(backups.len(), 1);
            assert!(is_symlink(&backups[0]));
            assert!(
                result
                    .error_message
                    .unwrap()
                    .contains(&backups[0].display().to_string())
            );
            if blocked_restore {
                assert_eq!(read(&s.destination), b"racing output");
            }
        }
    }
}

#[test]
fn copy_paths() {
    assert_eq!(
        default_copy_path(Path::new("/v/a.b.mp4")),
        Path::new("/v/a.b-thumbnail.mp4")
    );
    assert_eq!(
        next_copy_path(Path::new("/v/a-thumbnail.mp4")),
        Path::new("/v/a-thumbnail-thumbnail.mp4")
    );
    assert_eq!(
        default_copy_path(Path::new("/v/noext")),
        Path::new("/v/noext-thumbnail")
    );
    assert_eq!(
        receipt_path(Path::new("/v/a.mp4")),
        Path::new("/v/.a.mp4.video-thumbnailer.json")
    );
}

#[test]
fn receipt_layout_matches_json_dump() {
    assert_eq!(
        receipt_json("aa", "bb"),
        r#"{"version": 1, "source_sha256": "aa", "output_sha256": "bb"}"#
    );
}
