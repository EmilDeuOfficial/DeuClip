#![windows_subsystem = "windows"]

mod app;
mod autostart;
mod capture;
mod config;
mod hotkeys;
mod logging;
mod notify;
mod theme;

use eframe::egui;

fn main() {
    logging::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DeuClip")
            .with_inner_size([660.0, 860.0])
            .with_min_inner_size([600.0, 560.0])
            .with_icon(theme::app_icon()),
        ..Default::default()
    };

    let result = eframe::run_native(
        "DeuClip",
        options,
        Box::new(|cc| Ok(Box::new(app::DeuClipApp::new(cc)))),
    );
    if let Err(e) = result {
        log::error!("GUI beendet mit Fehler: {e}");
    }
    log::logger().flush();
}
