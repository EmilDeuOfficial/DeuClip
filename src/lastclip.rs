//! The last saved clip: remembered for the "open last clip" hotkey and for clicks on the
//! notifications. Without a clip from this session the newest clip of the output folder is used.

use crate::overlay;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

static LAST: Mutex<Option<PathBuf>> = Mutex::new(None);
static OUTPUT_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Remembers the clip that was just saved.
pub fn set_last(path: PathBuf) {
    if let Ok(mut l) = LAST.lock() {
        *l = Some(path);
    }
}

/// Remembers the output folder (used when no clip was saved in this session yet).
pub fn set_output_dir(dir: PathBuf) {
    if let Ok(mut d) = OUTPUT_DIR.lock() {
        *d = Some(dir);
    }
}

/// The newest clip_*.mp4 of a folder.
fn newest_clip(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let name = path.file_name()?.to_str()?.to_string();
        if !(name.starts_with("clip_") && name.ends_with(".mp4")) {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().map(|(t, _)| modified > *t).unwrap_or(true) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, p)| p)
}

fn last_path() -> Option<PathBuf> {
    let remembered = LAST.lock().ok().and_then(|l| l.clone()).filter(|p| p.is_file());
    remembered.or_else(|| {
        let dir = OUTPUT_DIR.lock().ok().and_then(|d| d.clone())?;
        newest_clip(&dir)
    })
}

/// Opens the last clip in the default video player.
pub fn open_last() {
    match last_path() {
        Some(path) => open(&path),
        None => {
            log::info!("Open last clip: no clip found");
            overlay::show(false, "Kein Clip gefunden", "Es wurde noch kein Clip gespeichert.");
        }
    }
}

fn open(path: &Path) {
    log::info!("Opening clip: {}", path.display());
    if let Err(e) = std::process::Command::new("explorer.exe").arg(path).spawn() {
        log::warn!("Could not open the clip: {e}");
        overlay::show(false, "Clip nicht geöffnet", &e.to_string());
    }
}
