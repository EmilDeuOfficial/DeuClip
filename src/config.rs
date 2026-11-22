//! Configuration as JSON at %APPDATA%\DeuClip\config.json.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const MIN_DURATION: u32 = 5;
pub const MAX_DURATION: u32 = 600;

/// A key combination, stored independently of the hotkey library.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeySpec {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    /// Key name in egui format (e.g. "F9", "A", "Num1").
    pub key: String,
}

impl HotkeySpec {
    pub fn new(ctrl: bool, alt: bool, shift: bool, win: bool, key: &str) -> Self {
        Self {
            ctrl,
            alt,
            shift,
            win,
            key: key.to_string(),
        }
    }

    /// Readable form, e.g. "Strg+Alt+F9".
    pub fn label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("Strg".into());
        }
        if self.alt {
            parts.push("Alt".into());
        }
        if self.shift {
            parts.push("Shift".into());
        }
        if self.win {
            parts.push("Win".into());
        }
        parts.push(crate::hotkeys::key_label(&self.key));
        parts.join("+")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    Low,
    Medium,
    High,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::Low, Quality::Medium, Quality::High];

    pub fn label(self) -> &'static str {
        match self {
            Quality::Low => "Niedrig (kleine Dateien)",
            Quality::Medium => "Mittel",
            Quality::High => "Hoch (große Dateien)",
        }
    }

    /// Quality value (CRF or CQ): lower is better.
    pub fn value(self) -> u32 {
        match self {
            Quality::Low => 30,
            Quality::Medium => 25,
            Quality::High => 16,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkey1: HotkeySpec,
    pub hotkey2: HotkeySpec,
    /// Opens the last saved clip.
    pub hotkey3: HotkeySpec,
    pub duration1: u32,
    pub duration2: u32,
    /// Output index of the monitor (matches output_idx of ddagrab).
    pub monitor_index: u32,
    pub output_dir: String,
    pub quality: Quality,
    pub fps: u32,
    /// Target height in pixels, 0 means native resolution.
    pub scale_height: u32,
    /// Empty means automatic search.
    pub ffmpeg_path: String,
    /// Short sound when a clip was saved or failed (audible in fullscreen games, too).
    pub sound: bool,
    /// Record the system sound (everything that plays on the default output device).
    pub audio_system: bool,
    /// Record the default microphone.
    pub audio_mic: bool,
    /// Look for a newer release on GitHub now and then.
    pub check_updates: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey1: HotkeySpec::new(true, true, false, false, "F9"),
            hotkey2: HotkeySpec::new(true, true, false, false, "F10"),
            hotkey3: HotkeySpec::new(true, true, false, false, "F11"),
            duration1: 30,
            duration2: 60,
            monitor_index: 0,
            output_dir: default_output_dir().to_string_lossy().into_owned(),
            quality: Quality::Medium,
            fps: 30,
            scale_height: 0,
            ffmpeg_path: String::new(),
            sound: true,
            audio_system: true,
            audio_mic: false,
            check_updates: true,
        }
    }
}

pub fn app_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.config_dir().join("DeuClip"))
        .unwrap_or_else(|| std::env::temp_dir().join("DeuClip"))
}

pub fn config_path() -> PathBuf {
    app_dir().join("config.json")
}

pub fn default_output_dir() -> PathBuf {
    if let Some(user) = directories::UserDirs::new() {
        if let Some(videos) = user.video_dir() {
            return videos.join("Clips");
        }
        return user.home_dir().join("Videos").join("Clips");
    }
    std::env::temp_dir().join("DeuClip").join("Clips")
}

impl Config {
    /// Loads the configuration. A missing or broken file yields the defaults.
    /// The second value is a warning for the user when the file was broken.
    pub fn load() -> (Self, Option<String>) {
        let path = config_path();
        let mut warning = None;
        let mut cfg = match fs::read_to_string(&path) {
            // Editors such as Notepad write a BOM that serde_json does not accept.
            Ok(text) => match serde_json::from_str::<Config>(text.trim_start_matches('\u{feff}')) {
                Ok(c) => c,
                Err(e) => {
                    log::error!("config.json is invalid, using the defaults: {e}");
                    warning = Some(
                        "Die Datei config.json ist ungültig. Es werden die Standardeinstellungen genutzt."
                            .to_string(),
                    );
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        };
        cfg.sanitize();
        (cfg, warning)
    }

    /// Clamps values into valid ranges.
    pub fn sanitize(&mut self) {
        self.duration1 = self.duration1.clamp(MIN_DURATION, MAX_DURATION);
        self.duration2 = self.duration2.clamp(MIN_DURATION, MAX_DURATION);
        if !(1..=240).contains(&self.fps) {
            self.fps = 30;
        }
        if self.output_dir.trim().is_empty() {
            self.output_dir = default_output_dir().to_string_lossy().into_owned();
        }
        if self.hotkey1 == self.hotkey2
            || self.hotkey1 == self.hotkey3
            || self.hotkey2 == self.hotkey3
        {
            let d = Config::default();
            self.hotkey1 = d.hotkey1;
            self.hotkey2 = d.hotkey2;
            self.hotkey3 = d.hotkey3;
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = app_dir();
        fs::create_dir_all(&dir)
            .map_err(|e| format!("Konfigurationsordner konnte nicht angelegt werden: {e}"))?;
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Konfiguration konnte nicht serialisiert werden: {e}"))?;
        fs::write(config_path(), text)
            .map_err(|e| format!("Konfiguration konnte nicht gespeichert werden: {e}"))
    }

    pub fn output_path(&self) -> PathBuf {
        PathBuf::from(&self.output_dir)
    }
}
