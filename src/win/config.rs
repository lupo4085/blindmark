//! Persistent settings: %APPDATA%\blindmark\settings.json. Key and registry default to
//! %APPDATA%\blindmark\ as well.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    /// Request an RFC 3161 timestamp for every distribution batch.
    pub timestamp: bool,
    /// Time-Stamp Authority endpoint.
    pub tsa_url: String,
}

pub fn app_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("blindmark")
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
            timestamp: true,
            tsa_url: blindmark::timestamp::DEFAULT_TSA.to_string(),
        }
    }
}

impl Settings {
    fn path() -> PathBuf {
        app_dir().join("settings.json")
    }

    pub fn load() -> Settings {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, s);
        }
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
