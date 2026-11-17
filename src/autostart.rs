//! Autostart via the registry Run key of the current user.

use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
use winreg::RegKey;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "DeuClip";

pub fn is_enabled() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(RUN_KEY)
        .ok()
        .and_then(|k| k.get_value::<String, _>(VALUE_NAME).ok())
        .is_some()
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)
        .map_err(|e| format!("Registry-Schluessel konnte nicht geoeffnet werden: {e}"))?;
    if enabled {
        let exe = std::env::current_exe()
            .map_err(|e| format!("Pfad der EXE nicht ermittelbar: {e}"))?;
        let value = format!("\"{}\"", exe.display());
        key.set_value(VALUE_NAME, &value)
            .map_err(|e| format!("Autostart konnte nicht gesetzt werden: {e}"))
    } else {
        match key.delete_value(VALUE_NAME) {
            Ok(()) => Ok(()),
            // A missing value is not an error.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Autostart konnte nicht entfernt werden: {e}")),
        }
    }
}
