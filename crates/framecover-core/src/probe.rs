//! Container/stream inspection via `ffprobe -of json`.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::ffmpeg::{self, Tool};

/// An exact rational number such as a time base (`1/90000`) or frame rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational {
    pub num: i64,
    pub den: i64,
}

impl Rational {
    /// Parse `"num/den"` (or a bare integer). Returns `None` for `0/0` or junk.
    pub fn parse(text: &str) -> Option<Self> {
        let (num, den) = match text.split_once('/') {
            Some((n, d)) => (n.trim().parse().ok()?, d.trim().parse().ok()?),
            None => (text.trim().parse().ok()?, 1),
        };
        (den != 0).then_some(Self { num, den })
    }

    pub fn to_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }
}

impl std::fmt::Display for Rational {
    /// Formats like Python's `fractions.Fraction`: `30` or `30000/1001`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let g = gcd(self.num.unsigned_abs(), self.den.unsigned_abs()).max(1) as i64;
        let (mut n, mut d) = (self.num / g, self.den / g);
        if d < 0 {
            n = -n;
            d = -d;
        }
        if d == 1 {
            write!(f, "{n}")
        } else {
            write!(f, "{n}/{d}")
        }
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Stream disposition flags we care about.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Disposition {
    #[serde(default)]
    pub attached_pic: u8,
}

/// One stream as reported by ffprobe.
#[derive(Debug, Clone, Deserialize)]
pub struct Stream {
    pub index: usize,
    #[serde(default)]
    pub codec_type: String,
    #[serde(default)]
    pub codec_name: Option<String>,
    #[serde(default)]
    pub codec_long_name: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub pix_fmt: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    time_base: Option<String>,
    #[serde(default)]
    avg_frame_rate: Option<String>,
    #[serde(default)]
    r_frame_rate: Option<String>,
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    duration_ts: Option<i64>,
    #[serde(default)]
    pub disposition: Disposition,
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

impl Stream {
    pub fn is_video(&self) -> bool {
        self.codec_type == "video"
    }

    pub fn is_attached_pic(&self) -> bool {
        self.disposition.attached_pic != 0
    }

    /// A video stream that is real playable video, not cover art.
    pub fn is_playable_video(&self) -> bool {
        self.is_video() && !self.is_attached_pic()
    }

    pub fn time_base(&self) -> Option<Rational> {
        self.time_base.as_deref().and_then(Rational::parse)
    }

    /// Average frame rate (`avg_frame_rate`), `None` if unknown/zero.
    pub fn average_rate(&self) -> Option<Rational> {
        self.avg_frame_rate
            .as_deref()
            .and_then(Rational::parse)
            .filter(|r| r.num > 0)
    }

    /// Base/real frame rate (`r_frame_rate`), `None` if unknown/zero.
    pub fn base_rate(&self) -> Option<Rational> {
        self.r_frame_rate
            .as_deref()
            .and_then(Rational::parse)
            .filter(|r| r.num > 0)
    }

    /// Stream duration in `time_base` units (`duration_ts`), if reported.
    pub fn duration_ts(&self) -> Option<i64> {
        self.duration_ts
    }

    /// Stream duration in seconds, if reported.
    pub fn duration_secs(&self) -> Option<f64> {
        self.duration.as_deref()?.parse().ok()
    }

    /// Case-insensitive tag lookup (ffprobe preserves container tag case).
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

/// Container-level information.
#[derive(Debug, Clone, Deserialize)]
pub struct Format {
    #[serde(default)]
    pub format_name: String,
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

impl Format {
    pub fn duration_secs(&self) -> Option<f64> {
        self.duration.as_deref()?.parse().ok()
    }
}

/// Full ffprobe result for a file.
#[derive(Debug, Clone, Deserialize)]
pub struct ProbeInfo {
    #[serde(default)]
    pub streams: Vec<Stream>,
    pub format: Format,
}

impl ProbeInfo {
    pub fn playable_video(&self) -> impl Iterator<Item = &Stream> {
        self.streams.iter().filter(|s| s.is_playable_video())
    }

    pub fn first_playable_video(&self) -> Option<&Stream> {
        self.playable_video().next()
    }

    pub fn has_attached_pic(&self) -> bool {
        self.streams
            .iter()
            .any(|s| s.is_video() && s.is_attached_pic())
    }
}

/// Run ffprobe on `path`. Fails with [`Error::Other`] if the file cannot be
/// parsed as media at all.
pub fn probe(path: &Path) -> Result<ProbeInfo> {
    let mut cmd = ffmpeg::command(Tool::Ffprobe)?;
    cmd.args([
        "-v",
        "error",
        "-of",
        "json",
        "-show_format",
        "-show_streams",
        "-i",
    ])
    .arg(path);
    let out = ffmpeg::run(&mut cmd).map_err(|e| match e {
        Error::Ffmpeg { stderr, .. } => {
            Error::Other(format!("Could not read '{}': {stderr}", path.display()))
        }
        other => other,
    })?;
    parse(&out.stdout)
}

/// Parse ffprobe JSON output.
pub fn parse(json: &[u8]) -> Result<ProbeInfo> {
    serde_json::from_slice(json).map_err(|e| Error::Other(format!("invalid ffprobe output: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_parse_and_display() {
        assert_eq!(
            Rational::parse("1/90000"),
            Some(Rational { num: 1, den: 90000 })
        );
        assert_eq!(Rational::parse("0/0"), None);
        assert_eq!(Rational::parse("60/2").unwrap().to_string(), "30");
        assert_eq!(
            Rational::parse("30000/1001").unwrap().to_string(),
            "30000/1001"
        );
        assert_eq!(Rational::parse("25").unwrap().to_string(), "25");
    }

    #[test]
    fn parses_ffprobe_json() {
        let json = br#"{
          "streams": [
            {"index": 0, "codec_type": "video", "codec_name": "h264", "profile": "High",
             "pix_fmt": "yuv420p", "width": 320, "height": 240, "time_base": "1/12800",
             "avg_frame_rate": "25/1", "r_frame_rate": "25/1", "duration": "2.000000",
             "disposition": {"default": 1, "attached_pic": 0}},
            {"index": 1, "codec_type": "audio", "codec_name": "aac", "time_base": "1/44100",
             "avg_frame_rate": "0/0", "disposition": {"attached_pic": 0}},
            {"index": 2, "codec_type": "video", "codec_name": "mjpeg", "width": 64, "height": 64,
             "disposition": {"attached_pic": 1}, "tags": {"FILENAME": "cover.jpg"}}
          ],
          "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2", "duration": "2.023000"}
        }"#;
        let info = parse(json).unwrap();
        assert_eq!(info.playable_video().count(), 1);
        assert!(info.has_attached_pic());
        let v = info.first_playable_video().unwrap();
        assert_eq!(v.time_base(), Some(Rational { num: 1, den: 12800 }));
        assert_eq!(v.average_rate().unwrap().to_string(), "25");
        assert_eq!(info.streams[1].average_rate(), None);
        assert_eq!(info.streams[2].tag("filename"), Some("cover.jpg"));
        assert_eq!(info.format.duration_secs(), Some(2.023));
    }
}
