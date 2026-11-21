//! Persistent settings: %APPDATA%\blindmark\settings.json. Key and registry default to
//! %APPDATA%\blindmark\ as well. `BLINDMARK_DATA_DIR` moves all of it elsewhere, for tests.

use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Set when the unreadable settings file could not be copied aside: saving would overwrite
/// the only copy left, so every save in this run fails instead.
static SAVE_BLOCKED: AtomicBool = AtomicBool::new(false);

const BLOCKED_REASON: &str = "设置文件读不出来，原文件没能另存，本次运行不保存设置。";

/// Do not write settings for the rest of this run.
pub fn block_saving() {
    SAVE_BLOCKED.store(true, Ordering::Relaxed);
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub key_path: PathBuf,
    pub registry_dir: PathBuf,
    // embedding defaults
    pub strength: f32,
    pub jpeg: bool,
    pub jpeg_quality: u8,
    /// Empty = next to the original.
    pub out_dir: String,
    pub suffix: String,
    pub open_folder_after: bool,
    pub last_channel: String,
    // detection
    pub affine: bool,
    pub photo: bool,
    pub content: bool,
    pub strict: bool,
    /// 0 = automatic (all cores). Applied at start-up.
    pub threads: usize,
    // general
    /// 0..=3 = fixed tab, -1 = the tab used last.
    pub start_tab: i32,
    pub last_tab: i32,
    pub remember_window: bool,
    /// [left, top, right, bottom, maximized]
    pub window: Option<[i32; 5]>,
    /// Read an image from the clipboard when switching to the Detect tab.
    pub auto_clipboard: bool,
    /// Trusted-timestamp service used when the user ticks “申请可信时间戳”.
    #[serde(default = "default_tsa_url")]
    pub tsa_url: String,
}

fn default_tsa_url() -> String {
    blindmark::timestamp::DEFAULT_TSA.to_string()
}

/// The folder named by `BLINDMARK_DATA_DIR`, ignoring a blank value and surrounding blanks.
fn custom_data_dir(env: Option<OsString>) -> Option<PathBuf> {
    let v = env?;
    let s = v.to_string_lossy().trim().to_string();
    if s.is_empty() {
        return None;
    }
    Some(PathBuf::from(s))
}

/// Pure function, so it can be tested: a non-blank value wins (trimmed), otherwise
/// `dirs::config_dir()/blindmark`.
fn app_dir_from(env: Option<OsString>) -> PathBuf {
    custom_data_dir(env).unwrap_or_else(|| dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("blindmark"))
}

pub fn app_dir() -> PathBuf {
    app_dir_from(std::env::var_os("BLINDMARK_DATA_DIR"))
}

/// The folder in use when `BLINDMARK_DATA_DIR` is set, otherwise `None`.
pub fn test_data_dir() -> Option<PathBuf> {
    custom_data_dir(std::env::var_os("BLINDMARK_DATA_DIR"))
}

/// What went wrong while reading `settings.json`.
pub enum LoadProblem {
    /// The file exists but cannot be read or is not valid settings. `backup` is where the
    /// original was copied to (None when that copy failed). `detail` is the raw error; the
    /// fixed warning text does not show it.
    Unreadable {
        backup: Option<PathBuf>,
        #[allow(dead_code)]
        detail: String,
    },
}

/// Copy an unreadable settings file aside: `settings.json.bad`, or
/// `settings.json.bad-YYYYMMDD-HHMMSS` when that name is taken. Returns where it went.
fn backup_unreadable(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_string_lossy().to_string();
    let dir = path.parent()?;
    let mut to = dir.join(format!("{name}.bad"));
    if to.exists() {
        to = dir.join(format!("{name}.bad-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    }
    std::fs::copy(path, &to).ok().map(|_| to)
}

/// Pure version of [`Settings::load`], so it can be tested.
pub fn load_from(path: &Path) -> (Settings, Option<LoadProblem>) {
    let unreadable = |detail: String| {
        let backup = backup_unreadable(path);
        (Settings::default(), Some(LoadProblem::Unreadable { backup, detail }))
    };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Settings::default(), None),
        Err(e) => return unreadable(e.to_string()),
    };
    match serde_json::from_str(&text) {
        Ok(s) => (s, None),
        Err(e) => unreadable(e.to_string()),
    }
}

impl Default for Settings {
    fn default() -> Self {
        let d = app_dir();
        Settings {
            key_path: d.join("blindmark.key"),
            registry_dir: d.join("registry"),
            strength: blindmark::wm::DEFAULT_STRENGTH,
            jpeg: false,
            jpeg_quality: 95,
            out_dir: String::new(),
            suffix: "_wm".into(),
            open_folder_after: true,
            last_channel: String::new(),
            affine: true,
            photo: true,
            content: true,
            strict: false,
            threads: 0,
            start_tab: 0,
            last_tab: 0,
            remember_window: true,
            window: None,
            auto_clipboard: false,
            tsa_url: default_tsa_url(),
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        app_dir().join("settings.json")
    }

    pub fn load() -> (Settings, Option<LoadProblem>) {
        load_from(&Self::path())
    }

    pub fn save(&self) -> Result<(), String> {
        if SAVE_BLOCKED.load(Ordering::Relaxed) {
            return Err(BLOCKED_REASON.into());
        }
        self.save_to(&Self::path())
    }

    /// Write `settings.json.tmp` next to the target, then rename it into place.
    fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let s = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let mut tmp = path.as_os_str().to_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, s).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn detect_options(&self) -> blindmark::DetectOptions {
        blindmark::DetectOptions {
            affine: self.affine,
            photo: self.photo,
            content: self.content,
            z_thresh: if self.strict { blindmark::wm::Z_THRESH_STRICT } else { blindmark::wm::Z_THRESH },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under `target\tmp`, as required by the task card.
    fn tmp(tag: &str) -> PathBuf {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("tmp").join(format!("w-t-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn data_dir_from_env() {
        let default = app_dir_from(None);
        assert_eq!(default.file_name().unwrap(), "blindmark");
        assert_eq!(app_dir_from(Some(OsString::from("D:\\x"))), PathBuf::from("D:\\x"));
        assert_eq!(app_dir_from(Some(OsString::from("  D:\\x  "))), PathBuf::from("D:\\x"));
        assert_eq!(app_dir_from(Some(OsString::from("  "))), default);
    }

    #[test]
    fn load_missing_file_is_default() {
        let d = tmp("missing");
        let (s, p) = load_from(&d.join("settings.json"));
        assert!(s == Settings::default());
        assert!(p.is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn load_valid_file_keeps_values_and_defaults_tsa() {
        let d = tmp("valid");
        let path = d.join("settings.json");
        let text = r#"{
  "key_path": "C:\\keys\\a.key",
  "registry_dir": "C:\\reg",
  "strength": 24.0,
  "jpeg": true,
  "jpeg_quality": 90,
  "timestamp": true
}"#;
        std::fs::write(&path, text).unwrap();
        let (s, p) = load_from(&path);
        assert!(p.is_none());
        assert_eq!(s.key_path, PathBuf::from("C:\\keys\\a.key"));
        assert_eq!(s.registry_dir, PathBuf::from("C:\\reg"));
        assert_eq!(s.strength, 24.0);
        assert!(s.jpeg);
        assert_eq!(s.jpeg_quality, 90);
        assert_eq!(s.tsa_url, blindmark::timestamp::DEFAULT_TSA);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn load_invalid_file_backs_up_once() {
        let d = tmp("invalid");
        let path = d.join("settings.json");
        std::fs::write(&path, "{\"strength\": 20.0").unwrap();
        let first = std::fs::read(&path).unwrap();

        let (s, p) = load_from(&path);
        assert!(s == Settings::default());
        let backup = match p {
            Some(LoadProblem::Unreadable { backup: Some(b), detail }) => {
                assert!(!detail.is_empty());
                b
            }
            _ => panic!("expected Unreadable with a backup"),
        };
        assert_eq!(backup.file_name().unwrap(), "settings.json.bad");
        assert_eq!(std::fs::read(&backup).unwrap(), first);

        // A second failure must not touch the first backup.
        std::fs::write(&path, "not json at all").unwrap();
        let second = std::fs::read(&path).unwrap();
        let (s2, p2) = load_from(&path);
        assert!(s2 == Settings::default());
        let backup2 = match p2 {
            Some(LoadProblem::Unreadable { backup: Some(b), detail }) => {
                assert!(!detail.is_empty());
                b
            }
            _ => panic!("expected Unreadable with a second backup"),
        };
        assert_ne!(backup2, backup);
        let name = backup2.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("settings.json.bad-"), "{name}");
        assert_eq!(std::fs::read(&backup2).unwrap(), second);
        assert_eq!(std::fs::read(&backup).unwrap(), first);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn save_then_load_round_trips_tsa_url() {
        let d = tmp("roundtrip");
        let path = d.join("settings.json");
        let mut s = Settings::default();
        s.tsa_url = "https://example.invalid/".into();
        s.save_to(&path).unwrap();
        assert!(!d.join("settings.json.tmp").exists());
        let (back, p) = load_from(&path);
        assert!(p.is_none());
        assert_eq!(back.tsa_url, "https://example.invalid/");
        let _ = std::fs::remove_dir_all(&d);
    }
}
