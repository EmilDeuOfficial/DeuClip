//! Update check against the GitHub releases of DeuClip.
//!
//! Uses the `curl.exe` that ships with Windows 10 and 11, so no HTTP library is needed.
//! Only the release list of this repository is queried, no personal data is sent.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const API_URL: &str = "https://api.github.com/repos/EmilDeuOfficial/DeuClip/releases/latest";
/// Downloads are only accepted from the releases of this repository.
const DOWNLOAD_PREFIX: &str = "https://github.com/EmilDeuOfficial/DeuClip/releases/download/";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug)]
pub struct Release {
    /// Version as shown to the user, e.g. "1.1".
    pub version: String,
    /// Direct link to the installer, if the release has one.
    pub setup_url: Option<String>,
    /// Release page with the notes.
    pub notes_url: String,
}

/// Version of this build in the short form of the README (1.0.0 is shown as 1.0).
pub fn current_version() -> String {
    // DEUCLIP_TEST_VERSION only exists to test the update dialog.
    let test = std::env::var("DEUCLIP_TEST_VERSION").ok();
    let text = test.unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    let mut parts: Vec<&str> = text.split('.').collect();
    while parts.len() > 2 && parts.last() == Some(&"0") {
        parts.pop();
    }
    parts.join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v1.1", "1.0"));
        assert!(is_newer("2.0", "1.9.9"));
        assert!(is_newer("1.0.1", "1.0"));
        assert!(!is_newer("v1.0", "1.0"));
        assert!(!is_newer("1.0.0", "1.0"));
        assert!(!is_newer("0.9", "1.0"));
    }
}

fn parse(version: &str) -> Vec<u32> {
    version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split('.')
        .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// true if `candidate` is a higher version than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (a, b) = (parse(candidate), parse(current));
    let len = a.len().max(b.len());
    for i in 0..len {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

fn curl() -> Command {
    let mut c = Command::new("curl.exe");
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// Asks GitHub for the latest release. `Ok(None)` means this version is current.
pub fn check() -> Result<Option<Release>, String> {
    let output = curl()
        .args([
            "-sS",
            "-L",
            "--max-time",
            "20",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: DeuClip",
            API_URL,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl.exe konnte nicht gestartet werden: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Keine Verbindung zu GitHub: {}", err.trim()));
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Antwort von GitHub nicht lesbar: {e}"))?;
    let tag = json
        .get("tag_name")
        .and_then(|t| t.as_str())
        .ok_or_else(|| "GitHub hat keine Version geliefert.".to_string())?;
    if !is_newer(tag, &current_version()) {
        return Ok(None);
    }
    let setup_url = json
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|asset| {
                let name = asset.get("name")?.as_str()?;
                let url = asset.get("browser_download_url")?.as_str()?;
                (name == "DeuClip_Setup.exe" && url.starts_with(DOWNLOAD_PREFIX))
                    .then(|| url.to_string())
            })
        });
    let notes_url = json
        .get("html_url")
        .and_then(|u| u.as_str())
        .unwrap_or("https://github.com/EmilDeuOfficial/DeuClip/releases/latest")
        .to_string();
    Ok(Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        setup_url,
        notes_url,
    }))
}

/// Downloads the installer into the temp folder.
pub fn download(url: &str) -> Result<PathBuf, String> {
    if !url.starts_with(DOWNLOAD_PREFIX) {
        return Err("Der Download-Link gehört nicht zu den DeuClip-Releases.".to_string());
    }
    let dir = std::env::temp_dir().join("DeuClip_update");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Temp-Ordner nicht anlegbar: {e}"))?;
    let target = dir.join("DeuClip_Setup.exe");
    let _ = std::fs::remove_file(&target);
    let output = curl()
        .args(["-sS", "-L", "--max-time", "600", "-o"])
        .arg(&target)
        .arg(url)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl.exe konnte nicht gestartet werden: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Download fehlgeschlagen: {}", err.trim()));
    }
    // Plausibility: a Windows program of reasonable size.
    let bytes = std::fs::read(&target).map_err(|e| format!("Download nicht lesbar: {e}"))?;
    if bytes.len() < 500_000 || &bytes[..2] != b"MZ" {
        let _ = std::fs::remove_file(&target);
        return Err("Die heruntergeladene Datei ist kein gültiger Installer.".to_string());
    }
    Ok(target)
}

/// Starts the installer. It replaces DeuClip and starts it again (RESTARTAPP).
pub fn run_setup(path: &Path) -> Result<(), String> {
    Command::new(path)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/RESTARTAPP=1"])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Installer konnte nicht gestartet werden: {e}"))
}
