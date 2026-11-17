//! Konfiguration als JSON unter %APPDATA%\DeuClip\config.json.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const MIN_DURATION: u32 = 5;
pub const MAX_DURATION: u32 = 600;

/// Eine Tastenkombination, unabhaengig von der Hotkey-Bibliothek gespeichert.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeySpec {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    /// Name der Taste im egui-Format (z.B. "F9", "A", "Num1").
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

    /// Lesbare Darstellung, z.B. "Strg+Alt+F9".
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

    /// Qualitaetswert (CRF bzw. CQ): kleiner ist besser.
    pub fn value(self) -> u32 {
        match self {
            Quality::Low => 30,
            Quality::Medium => 25,
            Quality::High => 21,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkey1: HotkeySpec,
    pub hotkey2: HotkeySpec,
    pub duration1: u32,
    pub duration2: u32,
    /// Ausgabeindex des Monitors (entspricht output_idx bei ddagrab).
    pub monitor_index: u32,
    pub output_dir: String,
    pub quality: Quality,
    pub fps: u32,
    /// Zielhoehe in Pixeln, 0 bedeutet native Aufloesung.
    pub scale_height: u32,
    /// Leer bedeutet automatische Suche.
    pub ffmpeg_path: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey1: HotkeySpec::new(true, true, false, false, "F9"),
            hotkey2: HotkeySpec::new(true, true, false, false, "F10"),
            duration1: 30,
            duration2: 60,
            monitor_index: 0,
            output_dir: default_output_dir().to_string_lossy().into_owned(),
            quality: Quality::Medium,
            fps: 30,
            scale_height: 0,
            ffmpeg_path: String::new(),
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
    /// Laedt die Konfiguration. Fehlende oder defekte Dateien ergeben Standardwerte.
    pub fn load() -> Self {
        let path = config_path();
        let mut cfg = match fs::read_to_string(&path) {
            // Editoren wie Notepad schreiben ein BOM, das serde_json nicht akzeptiert.
            Ok(text) => match serde_json::from_str::<Config>(text.trim_start_matches('\u{feff}')) {
                Ok(c) => c,
                Err(e) => {
                    log::error!("config.json ist ungueltig, Standardwerte werden genutzt: {e}");
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        };
        cfg.sanitize();
        cfg
    }

    /// Bringt Werte in gueltige Bereiche.
    pub fn sanitize(&mut self) {
        self.duration1 = self.duration1.clamp(MIN_DURATION, MAX_DURATION);
        self.duration2 = self.duration2.clamp(MIN_DURATION, MAX_DURATION);
        if !(1..=240).contains(&self.fps) {
            self.fps = 30;
        }
        if self.output_dir.trim().is_empty() {
            self.output_dir = default_output_dir().to_string_lossy().into_owned();
        }
        if self.hotkey1 == self.hotkey2 {
            let d = Config::default();
            self.hotkey1 = d.hotkey1;
            self.hotkey2 = d.hotkey2;
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
