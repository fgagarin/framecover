//! Persisted preferences: a small JSON file in the OS config directory.
//!
//! The Python app stored `lastOpenDirectory` with `QSettings("FrameCover",
//! "FrameCover")` and, before the rename, `QSettings("Video Thumbnailer",
//! "Video Thumbnailer")`. On first run the value is migrated (best effort)
//! from those stores:
//!
//! * Windows: `HKCU\Software\FrameCover\FrameCover`, then
//!   `HKCU\Software\Video Thumbnailer\Video Thumbnailer` (value `lastOpenDirectory`).
//! * Linux/BSD: `$XDG_CONFIG_HOME/FrameCover/FrameCover.conf`, then
//!   `$XDG_CONFIG_HOME/Video Thumbnailer/Video Thumbnailer.conf` (`[General]` section).
//! * macOS: TODO — the plist stores (`~/Library/Preferences/com.framecover.FrameCover.plist`)
//!   are binary plists and are not migrated.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const KEY: &str = "lastOpenDirectory";
/// `(organization, application)` names of the Qt stores, newest first.
const LEGACY_STORES: [(&str, &str); 2] = [
    ("FrameCover", "FrameCover"),
    ("Video Thumbnailer", "Video Thumbnailer"),
];

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Data {
    #[serde(
        rename = "lastOpenDirectory",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    last_open_directory: Option<String>,
}

/// Reads a `lastOpenDirectory` value from a pre-Rust settings store.
pub trait LegacyStore {
    fn last_open_directory(&self) -> Option<String>;
}

#[derive(Debug)]
pub struct Settings {
    path: Option<PathBuf>,
    data: Data,
}

impl Settings {
    /// Default location: `<config dir>/FrameCover/settings.json`.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("FrameCover").join("settings.json"))
    }

    /// Load from the default location, migrating the Qt settings on first run.
    pub fn load() -> Self {
        let legacy: Vec<Box<dyn LegacyStore>> = LEGACY_STORES
            .iter()
            .map(|&(org, app)| qt_store(org, app))
            .collect();
        match Self::default_path() {
            Some(path) => Self::load_from(&path, &legacy),
            None => Self {
                path: None,
                data: Data::default(),
            },
        }
    }

    /// Load `path`; when it has no value yet, take the first legacy value
    /// found and persist it.
    pub fn load_from(path: &Path, legacy: &[Box<dyn LegacyStore>]) -> Self {
        let data = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| match serde_json::from_str::<Data>(&text) {
                Ok(data) => Some(data),
                Err(err) => {
                    log::warn!("ignoring unreadable settings {}: {err}", path.display());
                    None
                }
            })
            .unwrap_or_default();
        let mut settings = Self {
            path: Some(path.to_owned()),
            data,
        };
        if settings.data.last_open_directory.is_none()
            && let Some(value) = legacy.iter().find_map(|store| store.last_open_directory())
        {
            log::info!("migrated {KEY} from the previous settings store");
            settings.data.last_open_directory = Some(value);
            settings.save();
        }
        settings
    }

    /// The saved directory when it still exists, else the home directory.
    pub fn last_open_directory(&self) -> PathBuf {
        self.data
            .last_open_directory
            .as_deref()
            .filter(|dir| !dir.is_empty() && Path::new(dir).is_dir())
            .map(PathBuf::from)
            .unwrap_or_else(home_dir)
    }

    pub fn set_last_open_directory(&mut self, directory: &Path) {
        self.data.last_open_directory = Some(directory.to_string_lossy().into_owned());
        self.save();
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let result = (|| -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let json = serde_json::to_string_pretty(&self.data).map_err(std::io::Error::other)?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, json)?;
            std::fs::rename(&tmp, path)
        })();
        if let Err(err) = result {
            log::warn!("could not save settings to {}: {err}", path.display());
        }
    }
}

pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Legacy QSettings store for `(organization, application)` on this OS.
fn qt_store(organization: &str, application: &str) -> Box<dyn LegacyStore> {
    #[cfg(windows)]
    {
        Box::new(RegistryStore(format!(
            "Software\\{organization}\\{application}"
        )))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home_dir().join(".config"));
        Box::new(IniStore(
            base.join(organization).join(format!("{application}.conf")),
        ))
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        // TODO(macOS): read ~/Library/Preferences/com.<org>.<app>.plist.
        let _ = (organization, application);
        Box::new(NoStore)
    }
}

#[allow(dead_code)]
struct NoStore;

impl LegacyStore for NoStore {
    fn last_open_directory(&self) -> Option<String> {
        None
    }
}

#[cfg(windows)]
struct RegistryStore(String);

#[cfg(windows)]
impl LegacyStore for RegistryStore {
    fn last_open_directory(&self) -> Option<String> {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(&self.0)
            .ok()?;
        key.get_value::<String, _>(KEY)
            .ok()
            .filter(|v| !v.is_empty())
    }
}

/// QSettings IniFormat file (`[General]` section).
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
pub struct IniStore(pub PathBuf);

impl LegacyStore for IniStore {
    fn last_open_directory(&self) -> Option<String> {
        let text = std::fs::read_to_string(&self.0).ok()?;
        parse_qt_ini_value(&text, "General", KEY).filter(|v| !v.is_empty())
    }
}

/// Value of `key` in `[section]` of a QSettings INI file, unescaped.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
pub fn parse_qt_ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.to_owned();
        } else if current == section
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            return Some(unescape_qt_value(v.trim()));
        }
    }
    None
}

#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
fn unescape_qt_value(raw: &str) -> String {
    let raw = raw
        .strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .unwrap_or(raw);
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('x') => {
                let hex: String = std::iter::from_fn(|| chars.next_if(|c| c.is_ascii_hexdigit()))
                    .take(4)
                    .collect();
                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Option<String>);
    impl LegacyStore for Fixed {
        fn last_open_directory(&self) -> Option<String> {
            self.0.clone()
        }
    }

    #[test]
    fn invalid_saved_directory_falls_back_to_home() {
        let dir = tempfile::tempdir().unwrap();
        for value in [None, Some(""), Some("missing-directory")] {
            let path = dir.path().join("s.json");
            let data = Data {
                last_open_directory: value.map(str::to_owned),
            };
            std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
            let settings = Settings::load_from(&path, &[]);
            assert_eq!(settings.last_open_directory(), home_dir());
        }
        let path = dir.path().join("garbage.json");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(
            Settings::load_from(&path, &[]).last_open_directory(),
            home_dir()
        );
    }

    #[test]
    fn existing_saved_directory_is_restored_and_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg").join("settings.json");
        let mut settings = Settings::load_from(&path, &[]);
        settings.set_last_open_directory(dir.path());
        let reloaded = Settings::load_from(&path, &[]);
        assert_eq!(reloaded.last_open_directory(), dir.path());
    }

    #[test]
    fn legacy_settings_are_migrated_once() {
        let dir = tempfile::tempdir().unwrap();
        let legacy_dir = dir.path().to_string_lossy().into_owned();
        let path = dir.path().join("settings.json");
        let stores: Vec<Box<dyn LegacyStore>> =
            vec![Box::new(Fixed(None)), Box::new(Fixed(Some(legacy_dir)))];
        let settings = Settings::load_from(&path, &stores);
        assert_eq!(settings.last_open_directory(), dir.path());
        assert!(path.exists(), "migrated value is persisted");

        // An existing value wins over the legacy one.
        let current = dir.path().join("current.json");
        std::fs::write(&current, r#"{"lastOpenDirectory":"current"}"#).unwrap();
        let settings = Settings::load_from(&current, &stores);
        assert_eq!(
            settings.data.last_open_directory.as_deref(),
            Some("current")
        );
    }

    #[test]
    fn qt_ini_values_are_unescaped() {
        let ini = "[General]\nlastOpenDirectory=/home/u/Videos/\\x65e5\\x672c\nother=1\n";
        assert_eq!(
            parse_qt_ini_value(ini, "General", KEY).as_deref(),
            Some("/home/u/Videos/日本")
        );
        let quoted = "[General]\nlastOpenDirectory=\"/home/u/a, b\"\n";
        assert_eq!(
            parse_qt_ini_value(quoted, "General", KEY).as_deref(),
            Some("/home/u/a, b")
        );
        assert_eq!(
            parse_qt_ini_value("[Other]\nlastOpenDirectory=x", "General", KEY),
            None
        );
    }

    #[test]
    fn ini_store_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("FrameCover.conf");
        std::fs::write(&file, "[General]\nlastOpenDirectory=/videos\n").unwrap();
        assert_eq!(
            IniStore(file).last_open_directory().as_deref(),
            Some("/videos")
        );
    }
}
