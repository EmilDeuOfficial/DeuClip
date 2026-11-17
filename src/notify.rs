//! Windows-Toast-Benachrichtigungen. Jede Meldung laeuft in einem eigenen Thread.

use winrt_notification::Toast;

fn show(title: String, line1: String, line2: Option<String>) {
    let spawned = std::thread::Builder::new()
        .name("deuclip-toast".into())
        .spawn(move || {
            let mut toast = Toast::new(Toast::POWERSHELL_APP_ID)
                .title(&title)
                .text1(&line1);
            if let Some(l2) = &line2 {
                toast = toast.text2(l2);
            }
            if let Err(e) = toast.show() {
                log::warn!("Benachrichtigung fehlgeschlagen: {e:?}");
            }
        });
    if let Err(e) = spawned {
        log::warn!("Benachrichtigungs-Thread konnte nicht gestartet werden: {e}");
    }
}

/// Erfolgsmeldung. `partial` markiert einen unvollstaendigen Puffer.
pub fn clip_saved(file_name: &str, partial: bool, got_secs: u32, wanted_secs: u32) {
    let hint = if partial {
        Some(format!(
            "Puffer reichte nicht aus: {got_secs} s statt {wanted_secs} s gespeichert."
        ))
    } else {
        None
    };
    show("Clip gespeichert".to_string(), file_name.to_string(), hint);
}

pub fn error(message: &str) {
    show(
        "DeuClip: Fehler".to_string(),
        message.to_string(),
        None,
    );
}
