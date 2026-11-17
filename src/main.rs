#![windows_subsystem = "windows"]

mod app;
mod autostart;
mod capture;
mod config;
mod hotkeys;
mod instance;
mod logging;
mod notify;
mod overlay;
mod theme;
mod tray;

use eframe::egui;

fn main() {
    logging::init();

    // A second start only brings the running instance to the front.
    let Some(instance) = instance::acquire() else {
        return;
    };

    // Autostart passes --background: start hidden in the tray.
    let background = std::env::args().any(|a| a == autostart::BACKGROUND_ARG);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DeuClip")
            .with_inner_size([660.0, 860.0])
            .with_min_inner_size([600.0, 560.0])
            .with_icon(theme::app_icon())
            .with_visible(!background),
        ..Default::default()
    };

    let result = eframe::run_native(
        "DeuClip",
        options,
        Box::new(move |cc| Ok(Box::new(app::DeuClipApp::new(cc, instance, background)))),
    );
    if let Err(e) = result {
        log::error!("GUI exited with an error: {e}");
    }
    log::logger().flush();
}
