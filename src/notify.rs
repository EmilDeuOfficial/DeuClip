//! Windows toast notifications. Every notification runs in its own thread.

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
                log::warn!("Notification failed: {e:?}");
            }
        });
    if let Err(e) = spawned {
        log::warn!("Could not start the notification thread: {e}");
    }
}

/// Success message. `partial` marks an incomplete buffer.
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

/// Plain information, e.g. that DeuClip keeps running in the tray.
pub fn info(title: &str, message: &str) {
    show(title.to_string(), message.to_string(), None);
}

/// Problems found while starting (e.g. with Windows): FFmpeg missing, hotkey taken, ...
pub fn startup_problems(problems: &[String]) {
    let Some(first) = problems.first() else { return };
    let rest = if problems.len() > 1 {
        Some(problems[1..].join(" "))
    } else {
        None
    };
    show("DeuClip: Problem beim Start".to_string(), first.clone(), rest);
}

pub fn error(message: &str) {
    show(
        "DeuClip: Fehler".to_string(),
        message.to_string(),
        None,
    );
}
