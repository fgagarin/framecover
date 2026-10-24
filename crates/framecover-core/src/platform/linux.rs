//! Freedesktop (Linux/BSD) thumbnail cache invalidator.
//!
//! Writes a thumbnail-spec PNG to `$XDG_CACHE_HOME/thumbnails/large/` (by
//! default `~/.cache/thumbnails/large/`) and asks the file manager to show
//! the item via `org.freedesktop.FileManager1.ShowItems`, which makes
//! Nautilus/Dolphin/Nemo re-read it.
//!
//! Reference: <https://specifications.freedesktop.org/thumbnail-spec/latest/>

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

use image::{DynamicImage, RgbImage};

use super::{CacheInvalidator, run_with_timeout};
use crate::imaging;

/// Largest edge of a "large" freedesktop thumbnail.
pub const MAX_THUMB_SIZE: u32 = 256;

/// Writes an XDG thumbnail entry and notifies file managers via D-Bus.
#[derive(Debug, Clone)]
pub struct LinuxCacheInvalidator {
    /// `thumbnails/large` directory; `None` resolves it from the environment.
    pub thumbnail_dir: Option<PathBuf>,
    /// `dbus-send` executable (replaceable in tests).
    pub dbus_send: PathBuf,
}

impl Default for LinuxCacheInvalidator {
    fn default() -> Self {
        Self {
            thumbnail_dir: None,
            dbus_send: PathBuf::from("dbus-send"),
        }
    }
}

impl CacheInvalidator for LinuxCacheInvalidator {
    fn invalidate(&self, path: &Path, thumbnail: Option<&RgbImage>) {
        if let Err(e) = self.write_xdg_thumbnail(path, thumbnail) {
            log::warn!(
                "Failed to write XDG thumbnail for '{}': {e}",
                path.display()
            );
        }
        if let Err(e) = self.notify_dbus(path) {
            log::warn!("D-Bus notification failed for '{}': {e}", path.display());
        }
    }
}

impl LinuxCacheInvalidator {
    /// Write the thumbnail PNG; returns the file written.
    pub fn write_xdg_thumbnail(
        &self,
        path: &Path,
        thumbnail: Option<&RgbImage>,
    ) -> std::io::Result<PathBuf> {
        let abs = std::path::absolute(path)?;
        let uri = file_uri(&abs);
        let dir = match &self.thumbnail_dir {
            Some(dir) => dir.clone(),
            None => default_thumbnail_dir()
                .ok_or_else(|| std::io::Error::other("cannot determine the home directory"))?,
        };
        std::fs::create_dir_all(&dir)?;
        let dest = dir.join(thumbnail_file_name(&uri));

        let meta = std::fs::metadata(&abs)?;
        let mtime = meta
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let png = encode_thumbnail_png(thumbnail, &uri, mtime, meta.len())?;

        // Atomic replace: readers never observe a half-written PNG.
        let mut tmp = tempfile::Builder::new()
            .prefix(".framecover-")
            .suffix(".png.tmp")
            .tempfile_in(&dir)?;
        tmp.write_all(&png)?;
        tmp.as_file().sync_all()?;
        tmp.persist(&dest).map_err(|e| e.error)?;
        Ok(dest)
    }

    fn notify_dbus(&self, path: &Path) -> std::io::Result<()> {
        let uri = file_uri(&std::path::absolute(path)?);
        let mut cmd = Command::new(&self.dbus_send);
        cmd.args(show_items_args(&uri));
        let status = run_with_timeout(cmd, Duration::from_secs(3))?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "dbus-send exited with {status}"
            )))
        }
    }
}

/// `dbus-send` arguments for `FileManager1.ShowItems([uri], "")`.
pub fn show_items_args(uri: &str) -> Vec<String> {
    vec![
        "--session".into(),
        "--dest=org.freedesktop.FileManager1".into(),
        "--type=method_call".into(),
        "/org/freedesktop/FileManager1".into(),
        "org.freedesktop.FileManager1.ShowItems".into(),
        format!("array:string:{uri}"),
        "string:".into(),
    ]
}

/// `$XDG_CACHE_HOME/thumbnails/large`, falling back to `~/.cache/...`.
pub fn default_thumbnail_dir() -> Option<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .map(|h| PathBuf::from(h).join(".cache"))
        })?;
    Some(cache.join("thumbnails").join("large"))
}

/// `<md5(uri)>.png`, the spec's cache file name.
pub fn thumbnail_file_name(uri: &str) -> String {
    format!("{:x}.png", md5::compute(uri.as_bytes()))
}

/// RFC 3986 `file://` URI for an absolute path, escaped like GLib's
/// `g_filename_to_uri` so the MD5 matches what file managers compute.
pub fn file_uri(abs_path: &Path) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        abs_path.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes = abs_path.to_string_lossy().replace('\\', "/").into_bytes();

    let mut uri = String::from("file://");
    if bytes.first() != Some(&b'/') {
        uri.push('/');
    }
    for b in bytes {
        if b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// RGBA PNG (fit inside 256x256) with `Thumb::URI`, `Thumb::MTime` and
/// `Thumb::Size` tEXt chunks. A grey 1x1 placeholder is used without a frame.
pub fn encode_thumbnail_png(
    thumbnail: Option<&RgbImage>,
    uri: &str,
    mtime: u64,
    size: u64,
) -> std::io::Result<Vec<u8>> {
    let rgb = match thumbnail {
        Some(img) => imaging::thumbnail(img, MAX_THUMB_SIZE, MAX_THUMB_SIZE),
        None => RgbImage::from_pixel(1, 1, image::Rgb([128, 128, 128])),
    };
    let rgba = DynamicImage::ImageRgb8(rgb).into_rgba8();

    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, rgba.width(), rgba.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let to_io = |e: png::EncodingError| std::io::Error::other(e.to_string());
    encoder
        .add_text_chunk("Thumb::URI".into(), uri.into())
        .map_err(to_io)?;
    encoder
        .add_text_chunk("Thumb::MTime".into(), mtime.to_string())
        .map_err(to_io)?;
    encoder
        .add_text_chunk("Thumb::Size".into(), size.to_string())
        .map_err(to_io)?;
    let mut writer = encoder.write_header().map_err(to_io)?;
    writer.write_image_data(rgba.as_raw()).map_err(to_io)?;
    writer.finish().map_err(to_io)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_text_chunks(png_bytes: &[u8]) -> (u32, u32, png::ColorType, Vec<(String, String)>) {
        let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
        let reader = decoder.read_info().unwrap();
        let info = reader.info();
        let texts = info
            .uncompressed_latin1_text
            .iter()
            .map(|c| (c.keyword.clone(), c.text.clone()))
            .collect();
        (info.width, info.height, info.color_type, texts)
    }

    fn invalidator(dir: &Path) -> LinuxCacheInvalidator {
        LinuxCacheInvalidator {
            thumbnail_dir: Some(dir.to_path_buf()),
            dbus_send: PathBuf::from("framecover-missing-dbus-send"),
        }
    }

    #[test]
    fn xdg_thumbnail_path_uses_md5_of_uri() {
        let tmp = tempfile::tempdir().unwrap();
        let video = tmp.path().join("test.mp4");
        std::fs::write(&video, b"fake").unwrap();
        let thumbs = tmp.path().join("thumbs");

        let written = invalidator(&thumbs)
            .write_xdg_thumbnail(&video, None)
            .unwrap();

        let uri = file_uri(&std::path::absolute(&video).unwrap());
        let expected = format!("{:x}.png", md5::compute(uri.as_bytes()));
        assert_eq!(written, thumbs.join(expected));
        assert!(written.is_file());
    }

    #[test]
    fn thumbnail_written_with_xdg_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let video = tmp.path().join("clip.mp4");
        std::fs::write(&video, b"fake video").unwrap();
        let frame = RgbImage::from_pixel(640, 360, image::Rgb([200, 10, 10]));

        let written = invalidator(tmp.path())
            .write_xdg_thumbnail(&video, Some(&frame))
            .unwrap();

        let (w, h, color, texts) = read_text_chunks(&std::fs::read(&written).unwrap());
        assert_eq!((w, h), (256, 144));
        assert_eq!(color, png::ColorType::Rgba);
        let get = |k: &str| {
            texts
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(
            get("Thumb::URI").unwrap(),
            file_uri(&std::path::absolute(&video).unwrap())
        );
        assert_eq!(get("Thumb::Size").unwrap(), "10");
        let mtime: u64 = get("Thumb::MTime").unwrap().parse().unwrap();
        let expected = std::fs::metadata(&video)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(mtime, expected);
        // No temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn placeholder_used_without_frame() {
        let png_bytes = encode_thumbnail_png(None, "file:///x", 1, 2).unwrap();
        let (w, h, _, _) = read_text_chunks(&png_bytes);
        assert_eq!((w, h), (1, 1));
    }

    #[test]
    fn small_frames_are_not_upscaled() {
        let frame = RgbImage::new(100, 50);
        let (w, h, _, _) =
            read_text_chunks(&encode_thumbnail_png(Some(&frame), "u", 0, 0).unwrap());
        assert_eq!((w, h), (100, 50));
    }

    #[test]
    fn dbus_failure_does_not_raise() {
        let tmp = tempfile::tempdir().unwrap();
        let video = tmp.path().join("test.mp4");
        std::fs::write(&video, b"fake").unwrap();
        invalidator(tmp.path()).invalidate(&video, None); // must not panic
        // Missing video: both steps fail, still no panic.
        invalidator(tmp.path()).invalidate(&tmp.path().join("gone.mp4"), None);
    }

    #[cfg(unix)]
    #[test]
    fn file_uri_escapes_like_glib() {
        assert_eq!(
            file_uri(Path::new("/home/u/My Videos/ä#1.mp4")),
            "file:///home/u/My%20Videos/%C3%A4%231.mp4"
        );
        assert_eq!(file_uri(Path::new("/a/b-c_d.e~f")), "file:///a/b-c_d.e~f");
    }

    #[test]
    fn show_items_targets_file_manager() {
        let args = show_items_args("file:///a.mp4");
        assert!(args.contains(&"org.freedesktop.FileManager1.ShowItems".to_string()));
        assert!(args.contains(&"array:string:file:///a.mp4".to_string()));
    }
}
