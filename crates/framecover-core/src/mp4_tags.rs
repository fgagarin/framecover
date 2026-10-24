//! MP4 metadata: FrameCover's frame-position freeform tags and `covr`
//! (port of `core/thumbnail_metadata.py` + `_write_mp4_covr_tag`).
//!
//! Only the `moov/udta/meta/ilst` item list is rewritten; chapters, tracks and
//! media data are left untouched (chunk offsets are fixed up by `mp4ameta`
//! when the `moov` atom grows in front of `mdat`).

use std::path::Path;

use mp4ameta::{Data, DataIdent, Img, ReadConfig, Tag, WriteConfig};

use crate::error::{Error, Result};

/// Stable on-disk identifiers from before the FrameCover rename.
pub const TAG_MEAN: &str = "com.apple.iTunes";
pub const FRAME_NUMBER_NAME: &str = "video-thumbnailer-frame-number";
pub const POSITION_MS_NAME: &str = "video-thumbnailer-position-ms";

fn ident(name: &'static str) -> DataIdent {
    DataIdent::freeform(TAG_MEAN, name)
}

fn map_err(err: mp4ameta::Error) -> Error {
    match err.kind {
        mp4ameta::ErrorKind::Io(io) => Error::Io(io),
        _ => Error::Other(format!("MP4 metadata error: {}", err.description)),
    }
}

const READ_ITEMS: ReadConfig = ReadConfig {
    read_meta_items: true,
    read_image_data: true,
    ..ReadConfig::NONE
};

const WRITE_ITEMS: WriteConfig = WriteConfig {
    write_meta_items: true,
    ..WriteConfig::NONE
};

/// Read the saved frame number and timeline position from an MP4 container.
///
/// Any read/parse failure yields `(None, None)`, like the Python version.
pub fn read_thumbnail_source(path: &Path) -> (Option<u64>, Option<u64>) {
    let cfg = ReadConfig {
        read_image_data: false,
        ..READ_ITEMS
    };
    let Ok(tag) = Tag::read_with_path(path, &cfg) else {
        return (None, None);
    };
    (
        read_integer(&tag, FRAME_NUMBER_NAME),
        read_integer(&tag, POSITION_MS_NAME),
    )
}

/// Save the selected frame source as MP4 freeform metadata (ASCII decimal,
/// UTF-8 data type — mutagen's `MP4FreeForm` default).
pub fn write_thumbnail_source(path: &Path, frame_number: u64, position_ms: u64) -> Result<()> {
    let mut tag = Tag::read_with_path(path, &READ_ITEMS).map_err(map_err)?;
    tag.set_data(
        ident(FRAME_NUMBER_NAME),
        Data::Utf8(frame_number.to_string()),
    );
    tag.set_data(ident(POSITION_MS_NAME), Data::Utf8(position_ms.to_string()));
    tag.write_with_path(path, &WRITE_ITEMS).map_err(map_err)
}

/// Write JPEG cover art into the MP4 `covr` tag (data type 13), replacing any
/// existing artwork. Telegram clients read this rather than `attached_pic`.
pub fn write_covr(path: &Path, jpeg: &[u8]) -> Result<()> {
    let mut tag = Tag::read_with_path(path, &READ_ITEMS).map_err(map_err)?;
    tag.set_artwork(Img::jpeg(jpeg.to_vec()));
    tag.write_with_path(path, &WRITE_ITEMS).map_err(map_err)
}

fn data_bytes(data: &Data) -> &[u8] {
    match data {
        Data::Utf8(s) | Data::Utf16(s) => s.as_bytes(),
        Data::Reserved(b)
        | Data::Jpeg(b)
        | Data::Png(b)
        | Data::Bmp(b)
        | Data::BeSigned(b)
        | Data::Unknown { data: b, .. } => b,
    }
}

fn read_integer(tag: &Tag, name: &'static str) -> Option<u64> {
    let first = tag.data_of(&ident(name)).next()?;
    parse_python_int(data_bytes(first))
}

/// Mirror `int(bytes.decode("ascii"))` for non-negative values: ASCII only,
/// surrounding whitespace and a leading sign allowed, `_` between digits.
fn parse_python_int(bytes: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(bytes).ok().filter(|s| s.is_ascii())?;
    let text = text.trim_matches(|c: char| c.is_ascii_whitespace() || c == '\x0b' || c == '\x0c');
    let (negative, digits) = match text.as_bytes().first()? {
        b'+' => (false, &text[1..]),
        b'-' => (true, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.bytes().all(|b| b.is_ascii_digit() || b == b'_')
    {
        return None;
    }
    let cleaned: String = digits.chars().filter(|&c| c != '_').collect();
    let value: u128 = cleaned.parse().ok()?;
    if negative {
        return (value == 0).then_some(0);
    }
    u64::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_int_semantics() {
        assert_eq!(parse_python_int(b"42"), Some(42));
        assert_eq!(parse_python_int(b" 7\n"), Some(7));
        assert_eq!(parse_python_int(b"+3"), Some(3));
        assert_eq!(parse_python_int(b"1_000"), Some(1000));
        assert_eq!(parse_python_int(b"-0"), Some(0));
        assert_eq!(parse_python_int(b"-5"), None);
        assert_eq!(parse_python_int(b""), None);
        assert_eq!(parse_python_int(b"1.5"), None);
        assert_eq!(parse_python_int(b"_1"), None);
        assert_eq!(parse_python_int("١".as_bytes()), None);
    }
}
