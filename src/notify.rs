//! Windows toast notifications and sound feedback. Every notification runs in its own thread.
//!
//! The toasts are built directly with the Windows notification API: a saved clip carries
//! its file as the activation target, so a click on the toast opens the clip.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::HSTRING;
use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::Win32::System::Diagnostics::Debug::Beep;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

/// App id of Windows PowerShell: toasts of unpackaged apps need a registered id.
const APP_ID: &str = r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";

static SOUND: AtomicBool = AtomicBool::new(true);

/// Turns the sound feedback on or off.
pub fn set_sound(on: bool) {
    SOUND.store(on, Ordering::Relaxed);
}

/// Short beep: two rising tones for success, one low tone for a failure. Games in exclusive
/// fullscreen hide every notification window, the sound still works there.
pub fn beep(ok: bool) {
    if !SOUND.load(Ordering::Relaxed) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("deuclip-beep".into())
        .spawn(move || unsafe {
            if ok {
                let _ = Beep(880, 90);
                let _ = Beep(1320, 130);
            } else {
                let _ = Beep(330, 320);
            }
        });
    if let Err(e) = spawned {
        log::warn!("Could not start the sound thread: {e}");
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// file:///C:/Users/... with percent-encoding for everything that is not plain.
fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file:///");
    for b in text.bytes() {
        let plain = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/' | b':');
        if plain {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn build_xml(title: &str, line1: &str, line2: Option<&str>, open: Option<&Path>) -> String {
    let launch = open
        .map(|p| format!(" activationType=\"protocol\" launch=\"{}\"", xml_escape(&file_uri(p))))
        .unwrap_or_default();
    let second = line2
        .map(|l| format!("<text>{}</text>", xml_escape(l)))
        .unwrap_or_default();
    format!(
        "<toast{launch}><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text>{second}</binding></visual></toast>",
        xml_escape(title),
        xml_escape(line1),
    )
}

fn show_toast(xml: &str) -> windows::core::Result<()> {
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?;
    notifier.Show(&toast)
}

fn show(title: String, line1: String, line2: Option<String>, open: Option<std::path::PathBuf>) {
    let spawned = std::thread::Builder::new()
        .name("deuclip-toast".into())
        .spawn(move || {
            // Already initialized is fine, the result is not needed.
            unsafe {
                let _ = RoInitialize(RO_INIT_MULTITHREADED);
            }
            let xml = build_xml(&title, &line1, line2.as_deref(), open.as_deref());
            if let Err(e) = show_toast(&xml) {
                log::warn!("Notification failed: {e:?}");
            }
        });
    if let Err(e) = spawned {
        log::warn!("Could not start the notification thread: {e}");
    }
}

/// Success message. `partial` marks an incomplete buffer. A click opens the clip.
pub fn clip_saved(path: &Path, partial: bool, got_secs: u32, wanted_secs: u32) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let hint = if partial {
        Some(format!(
            "Puffer reichte nicht aus: {got_secs} s statt {wanted_secs} s gespeichert."
        ))
    } else {
        None
    };
    show(
        "Clip gespeichert".to_string(),
        name,
        hint,
        Some(path.to_path_buf()),
    );
}

/// Plain information, e.g. that DeuClip keeps running in the tray.
pub fn info(title: &str, message: &str) {
    show(title.to_string(), message.to_string(), None, None);
}

/// Problems found while starting (e.g. with Windows): FFmpeg missing, hotkey taken, ...
pub fn startup_problems(problems: &[String]) {
    let Some(first) = problems.first() else { return };
    let rest = if problems.len() > 1 {
        Some(problems[1..].join(" "))
    } else {
        None
    };
    show("DeuClip: Problem beim Start".to_string(), first.clone(), rest, None);
}

pub fn error(message: &str) {
    show("DeuClip: Fehler".to_string(), message.to_string(), None, None);
}
