//! GUI: main window, status bar and the wiring to engine, hotkeys and configuration.

use crate::capture::{self, Command, Engine, Event, MonitorInfo, RecState, Settings};
use crate::config::{Config, HotkeySpec, Quality, MAX_DURATION, MIN_DURATION};
use crate::hotkeys::{self, HotkeyService};
use crate::instance::Instance;
use crate::tray::{Tray, TrayAction};
use crate::updater::{self, Release};
use crate::{autostart, lastclip, notify, overlay, theme};
use eframe::egui::{
    self, vec2, Align, Button, CornerRadius, Frame, Label, Layout, Margin, RichText, Stroke,
    TextEdit,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

enum Picked {
    Folder(PathBuf),
    Ffmpeg(PathBuf),
    Cancelled,
}

enum UpdateMsg {
    Checked {
        result: Result<Option<Release>, String>,
        /// Started by the user: errors are shown.
        manual: bool,
    },
    Downloaded(Result<PathBuf, String>),
}

enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release),
    Failed(String),
}

type CaptureKey = (Option<PathBuf>, u32, u32, u32, Quality, [bool; 3]);

pub struct DeuClipApp {
    cfg: Config,
    monitors: Vec<MonitorInfo>,
    ffmpeg: Option<PathBuf>,
    engine: Engine,
    ui_rx: Receiver<Event>,
    hotkeys: HotkeyService,
    rec_state: RecState,
    /// Last message for the status bar: text and error flag.
    info: Option<(String, bool)>,
    dur_text: [String; 2],
    dur_err: [Option<String>; 2],
    hk_err: [Option<String>; 3],
    capturing: Option<usize>,
    picked_tx: Sender<Picked>,
    picked_rx: Receiver<Picked>,
    picker_open: bool,
    autostart: bool,
    autostart_err: Option<String>,
    applied_key: Option<CaptureKey>,
    tray: Option<Tray>,
    tray_rx: Receiver<TrayAction>,
    /// true once the user chose to quit: closing the window then really exits.
    quitting: bool,
    hide_notice_shown: bool,
    /// Last texts given to the tray menu: clip lengths and pause state.
    tray_labels: (u32, u32, bool),
    update: UpdateState,
    update_tx: Sender<UpdateMsg>,
    update_rx: Receiver<UpdateMsg>,
    /// Version the user was already told about by a notification.
    update_notified: Option<String>,
    updates_on: Arc<AtomicBool>,
    /// Number of frames in which the window must still be hidden after an autostart.
    hide_frames_left: u8,
    _instance: Instance,
}

impl DeuClipApp {
    pub fn new(cc: &eframe::CreationContext<'_>, instance: Instance, background: bool) -> Self {
        theme::apply(&cc.egui_ctx);
        overlay::init();

        let (mut cfg, cfg_warning) = Config::load();
        // Problems found while starting, reported when DeuClip starts hidden (autostart).
        let mut problems: Vec<String> = Vec::new();
        problems.extend(cfg_warning);
        let monitors = capture::list_monitors();
        if !monitors.iter().any(|m| m.index == cfg.monitor_index) {
            problems.push(
                "Der gespeicherte Bildschirm wurde nicht gefunden. Es wird der Hauptbildschirm genutzt."
                    .to_string(),
            );
            cfg.monitor_index = monitors
                .iter()
                .find(|m| m.primary)
                .or_else(|| monitors.first())
                .map(|m| m.index)
                .unwrap_or(0);
        }

        // Engine -> dispatcher -> GUI. The dispatcher shows notifications even
        // while the window is minimized.
        let (ev_tx, ev_rx) = channel::<Event>();
        let (ui_tx, ui_rx) = channel::<Event>();
        let engine = Engine::spawn(ev_tx);
        let ctx = cc.egui_ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("deuclip-dispatch".into())
            .spawn(move || {
                let mut had_error = false;
                let mut was_paused = false;
                for ev in ev_rx {
                    match &ev {
                        Event::Saved {
                            path,
                            partial,
                            got_secs,
                            wanted_secs,
                            ..
                        } => {
                            let name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.to_string_lossy().into_owned());
                            lastclip::set_last(path.clone());
                            notify::clip_saved(path, *partial, *got_secs, *wanted_secs);
                            let title = if *partial {
                                format!("Clip gespeichert ({got_secs} s von {wanted_secs} s)")
                            } else {
                                "Clip gespeichert".to_string()
                            };
                            overlay::show_clip(&title, &format!("{name}\nZum Öffnen klicken"));
                            notify::beep(true);
                        }
                        Event::SaveFailed { message, .. } => {
                            notify::error(message);
                            overlay::show(false, "Clip nicht gespeichert", message);
                            notify::beep(false);
                        }
                        Event::Notice(message) => {
                            overlay::show(false, "Ton nicht verfügbar", message);
                        }
                        Event::State(RecState::Error(message)) => {
                            had_error = true;
                            notify::error(message);
                            overlay::show(false, "Aufnahme-Problem", message);
                        }
                        Event::State(RecState::Paused) => {
                            was_paused = true;
                            overlay::show(true, "Aufnahme pausiert", "Fortsetzen im Tray-Menü.");
                        }
                        Event::State(RecState::Running { .. }) => {
                            if had_error {
                                had_error = false;
                                was_paused = false;
                                notify::info("DeuClip", "Die Aufnahme läuft wieder.");
                                overlay::show(true, "Aufnahme läuft wieder", "");
                            } else if was_paused {
                                was_paused = false;
                                overlay::show(true, "Aufnahme fortgesetzt", "");
                            }
                        }
                        Event::State(_) => {}
                    }
                    if ui_tx.send(ev).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        if let Err(e) = spawned {
            log::error!("Could not start the dispatcher thread: {e}");
        }

        let hotkeys = HotkeyService::new(engine.sender(), [cfg.duration1, cfg.duration2]);
        let (picked_tx, picked_rx) = channel();

        // Tray icon and the signal of a second program start.
        let (tray_tx, tray_rx) = channel::<TrayAction>();
        let tray = match Tray::new(cc.egui_ctx.clone(), tray_tx.clone()) {
            Ok(t) => Some(t),
            Err(e) => {
                log::error!("{e}");
                None
            }
        };
        {
            let tx = tray_tx.clone();
            let ctx = cc.egui_ctx.clone();
            instance.listen(move || {
                let _ = tx.send(TrayAction::Show);
                ctx.request_repaint();
            });
        }
        let tray_ok = tray.is_some();
        // Without a tray icon there would be no way back, so the window is shown anyway.
        if background && tray.is_none() {
            cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        }
        autostart::refresh();

        // Background update check: shortly after the start, then every 6 hours.
        let (update_tx, update_rx) = channel::<UpdateMsg>();
        let updates_on = Arc::new(AtomicBool::new(cfg.check_updates));
        {
            let tx = update_tx.clone();
            let ctx = cc.egui_ctx.clone();
            let on = Arc::clone(&updates_on);
            let spawned = std::thread::Builder::new()
                .name("deuclip-update".into())
                .spawn(move || {
                    std::thread::sleep(Duration::from_secs(25));
                    loop {
                        if on.load(Ordering::Relaxed) {
                            let result = updater::check();
                            let _ = tx.send(UpdateMsg::Checked { result, manual: false });
                            ctx.request_repaint();
                        }
                        std::thread::sleep(Duration::from_secs(6 * 3600));
                    }
                });
            if let Err(e) = spawned {
                log::warn!("Could not start the update thread: {e}");
            }
        }

        let mut app = Self {
            dur_text: [cfg.duration1.to_string(), cfg.duration2.to_string()],
            cfg,
            monitors,
            ffmpeg: None,
            engine,
            ui_rx,
            hotkeys,
            rec_state: RecState::Stopped,
            info: None,
            dur_err: [None, None],
            hk_err: [None, None, None],
            capturing: None,
            picked_tx,
            picked_rx,
            picker_open: false,
            autostart: autostart::is_enabled(),
            autostart_err: None,
            applied_key: None,
            tray,
            tray_rx,
            quitting: false,
            hide_notice_shown: false,
            tray_labels: (0, 0, false),
            update: UpdateState::Idle,
            update_tx,
            update_rx,
            update_notified: None,
            updates_on,
            hide_frames_left: if background && tray_ok { 4 } else { 0 },
            _instance: instance,
        };
        app.apply_hotkey(0);
        app.apply_hotkey(1);
        app.apply_hotkey(2);
        app.commit();

        if app.ffmpeg.is_none() {
            problems.push(
                "FFmpeg wurde nicht gefunden. Lege ffmpeg.exe neben DeuClip.exe oder wähle den Pfad im Fenster."
                    .to_string(),
            );
        }
        for slot in 0..3 {
            if let Some(e) = &app.hk_err[slot] {
                problems.push(format!("{}: {}", Self::slot_name(slot), e));
            }
        }
        if let Some(e) = &app.hotkeys.init_error {
            problems.push(e.clone());
        }
        if app.tray.is_none() {
            problems.push(
                "Das Tray-Symbol konnte nicht erstellt werden. DeuClip bleibt als Fenster sichtbar."
                    .to_string(),
            );
        }
        if background && !problems.is_empty() {
            log::warn!("Problems at startup: {}", problems.join(" | "));
            notify::startup_problems(&problems);
            overlay::show(false, "Problem beim Start", &problems[0]);
        }
        app
    }

    // ----- State and configuration ---------------------------------------

    fn spec(&self, slot: usize) -> &HotkeySpec {
        match slot {
            0 => &self.cfg.hotkey1,
            1 => &self.cfg.hotkey2,
            _ => &self.cfg.hotkey3,
        }
    }

    /// Name of a hotkey slot in messages.
    fn slot_name(slot: usize) -> String {
        if slot == 2 {
            "Clip öffnen".to_string()
        } else {
            format!("Hotkey {}", slot + 1)
        }
    }

    fn set_spec(&mut self, slot: usize, spec: HotkeySpec) {
        match slot {
            0 => self.cfg.hotkey1 = spec,
            1 => self.cfg.hotkey2 = spec,
            _ => self.cfg.hotkey3 = spec,
        }
    }

    fn duration(&self, slot: usize) -> u32 {
        if slot == 0 {
            self.cfg.duration1
        } else {
            self.cfg.duration2
        }
    }

    fn apply_hotkey(&mut self, slot: usize) {
        let spec = self.spec(slot).clone();
        self.hk_err[slot] = self.hotkeys.set_slot(slot, &spec).err();
    }

    /// Saves the configuration, reports changes to the engine and restarts the
    /// capture when a capture parameter changed.
    fn commit(&mut self) {
        if let Err(e) = self.cfg.save() {
            log::error!("{e}");
            self.info = Some((e, true));
        }
        self.hotkeys
            .set_durations([self.cfg.duration1, self.cfg.duration2]);
        notify::set_sound(self.cfg.sound);
        self.updates_on.store(self.cfg.check_updates, Ordering::Relaxed);
        lastclip::set_output_dir(self.cfg.output_path());
        let retention = self.cfg.duration1.max(self.cfg.duration2) as u64 + capture::RESERVE_SECS;
        self.engine.send(Command::Configure {
            output_dir: self.cfg.output_path(),
            retention_secs: retention,
        });

        // The on-screen notification appears on the recorded monitor.
        if let Some(m) = self.monitors.iter().find(|m| m.index == self.cfg.monitor_index) {
            overlay::set_anchor(m.x + m.width / 2, m.y + m.height / 2);
        }

        self.ffmpeg = capture::find_ffmpeg(&self.cfg.ffmpeg_path);
        let key: CaptureKey = (
            self.ffmpeg.clone(),
            self.cfg.monitor_index,
            self.cfg.fps,
            self.cfg.scale_height,
            self.cfg.quality,
            [
                self.cfg.audio_system,
                self.cfg.audio_all_devices,
                self.cfg.audio_mic,
            ],
        );
        if self.applied_key.as_ref() != Some(&key) {
            self.applied_key = Some(key);
            self.restart_capture();
        }
    }

    fn restart_capture(&mut self) {
        match &self.ffmpeg {
            Some(path) => {
                // Never upscale: from the native monitor height on, no scaling is applied.
                let native = self
                    .monitors
                    .iter()
                    .find(|m| m.index == self.cfg.monitor_index)
                    .map(|m| m.height)
                    .unwrap_or(0);
                let scale_height = if native > 0 && self.cfg.scale_height as i32 >= native {
                    0
                } else {
                    self.cfg.scale_height
                };
                self.engine.send(Command::Start(Settings {
                    ffmpeg: path.clone(),
                    monitor_index: self.cfg.monitor_index,
                    fps: self.cfg.fps,
                    scale_height,
                    quality: self.cfg.quality,
                    audio_system: self.cfg.audio_system,
                    audio_all_devices: self.cfg.audio_all_devices,
                    audio_mic: self.cfg.audio_mic,
                }));
            }
            None => {
                self.engine.send(Command::Stop);
                self.rec_state = RecState::Stopped;
            }
        }
    }

    fn drain_events(&mut self) {
        while let Ok(ev) = self.ui_rx.try_recv() {
            match ev {
                Event::State(s) => self.rec_state = s,
                Event::Notice(m) => self.info = Some((m, true)),
                Event::Saved {
                    slot,
                    path,
                    partial,
                    got_secs,
                    wanted_secs,
                } => {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let n = slot + 1;
                    let text = if partial {
                        format!("Hotkey {n}: {name} gespeichert ({got_secs} s von {wanted_secs} s)")
                    } else {
                        format!("Hotkey {n}: {name} gespeichert")
                    };
                    self.info = Some((text, false));
                }
                Event::SaveFailed { slot, message } => {
                    self.info = Some((format!("Hotkey {}: {message}", slot + 1), true))
                }
            }
        }
        while let Ok(m) = self.update_rx.try_recv() {
            match m {
                UpdateMsg::Checked { result, manual } => match result {
                    Ok(Some(release)) => {
                        if !manual && self.update_notified.as_deref() != Some(release.version.as_str()) {
                            self.update_notified = Some(release.version.clone());
                            notify::info(
                                "DeuClip: Update verfügbar",
                                &format!(
                                    "Version {} ist verfügbar. Öffne das Fenster zum Aktualisieren.",
                                    release.version
                                ),
                            );
                        }
                        if !matches!(self.update, UpdateState::Downloading(_)) {
                            self.update = UpdateState::Available(release);
                        }
                    }
                    Ok(None) => {
                        if !matches!(self.update, UpdateState::Downloading(_)) {
                            self.update = UpdateState::UpToDate;
                        }
                    }
                    Err(e) => {
                        log::warn!("Update check failed: {e}");
                        if manual {
                            self.update = UpdateState::Failed(e);
                        } else if matches!(self.update, UpdateState::Checking) {
                            self.update = UpdateState::Idle;
                        }
                    }
                },
                UpdateMsg::Downloaded(Ok(path)) => match updater::run_setup(&path) {
                    // The installer replaces DeuClip and starts it again.
                    Ok(()) => self.quit(),
                    Err(e) => self.update = UpdateState::Failed(e),
                },
                UpdateMsg::Downloaded(Err(e)) => self.update = UpdateState::Failed(e),
            }
        }
        while let Ok(p) = self.picked_rx.try_recv() {
            self.picker_open = false;
            match p {
                Picked::Folder(path) => {
                    self.cfg.output_dir = path.to_string_lossy().into_owned();
                    self.commit();
                }
                Picked::Ffmpeg(path) => {
                    self.cfg.ffmpeg_path = path.to_string_lossy().into_owned();
                    self.commit();
                }
                Picked::Cancelled => {}
            }
        }
    }

    // ----- Hotkey recording ----------------------------------------------

    fn start_capture(&mut self, slot: usize) {
        self.hotkeys.suspend();
        self.capturing = Some(slot);
        self.hk_err[slot] = None;
    }

    fn end_capture(&mut self) {
        self.capturing = None;
        self.apply_hotkey(0);
        self.apply_hotkey(1);
        self.apply_hotkey(2);
    }

    fn handle_capture(&mut self, ui: &egui::Ui) {
        let Some(slot) = self.capturing else { return };
        let events = ui.input(|i| i.events.clone());
        for ev in events {
            let egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } = ev
            else {
                continue;
            };
            if key == egui::Key::Escape {
                self.end_capture();
                return;
            }
            if hotkeys::code_for(key).is_none() {
                continue;
            }
            let spec = HotkeySpec {
                ctrl: modifiers.ctrl,
                alt: modifiers.alt,
                shift: modifiers.shift,
                win: hotkeys::win_key_down(),
                key: key.name().to_string(),
            };
            let taken = (0..3).find(|&o| o != slot && spec == *self.spec(o));
            if let Some(other) = taken {
                self.end_capture();
                // Set after end_capture because apply_hotkey overwrites the error message.
                self.hk_err[slot] = Some(format!(
                    "Diese Kombination ist bereits für \"{}\" vergeben.",
                    Self::slot_name(other)
                ));
            } else {
                self.set_spec(slot, spec);
                self.end_capture();
                if let Err(e) = self.cfg.save() {
                    self.info = Some((e, true));
                }
            }
            return;
        }
    }

    // ----- File dialogs in their own thread ------------------------------

    fn pick_folder(&mut self, ctx: &egui::Context) {
        if self.picker_open {
            return;
        }
        self.picker_open = true;
        let tx = self.picked_tx.clone();
        let ctx = ctx.clone();
        let start = self.cfg.output_path();
        let spawned = std::thread::Builder::new()
            .name("deuclip-dialog".into())
            .spawn(move || {
                let mut dialog = rfd::FileDialog::new().set_title("Ausgabeordner wählen");
                if start.is_dir() {
                    dialog = dialog.set_directory(&start);
                }
                let msg = match dialog.pick_folder() {
                    Some(p) => Picked::Folder(p),
                    None => Picked::Cancelled,
                };
                let _ = tx.send(msg);
                ctx.request_repaint();
            });
        if spawned.is_err() {
            self.picker_open = false;
        }
    }

    fn pick_ffmpeg(&mut self, ctx: &egui::Context) {
        if self.picker_open {
            return;
        }
        self.picker_open = true;
        let tx = self.picked_tx.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("deuclip-dialog".into())
            .spawn(move || {
                let dialog = rfd::FileDialog::new()
                    .set_title("ffmpeg.exe wählen")
                    .add_filter("FFmpeg", &["exe"]);
                let msg = match dialog.pick_file() {
                    Some(p) => Picked::Ffmpeg(p),
                    None => Picked::Cancelled,
                };
                let _ = tx.send(msg);
                ctx.request_repaint();
            });
        if spawned.is_err() {
            self.picker_open = false;
        }
    }

    // ----- Updates -------------------------------------------------------

    fn check_updates_now(&mut self, ctx: &egui::Context) {
        self.update = UpdateState::Checking;
        let tx = self.update_tx.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("deuclip-update-check".into())
            .spawn(move || {
                let result = updater::check();
                let _ = tx.send(UpdateMsg::Checked { result, manual: true });
                ctx.request_repaint();
            });
        if spawned.is_err() {
            self.update = UpdateState::Idle;
        }
    }

    fn start_update(&mut self, ctx: &egui::Context) {
        let UpdateState::Available(release) = &self.update else { return };
        let release = release.clone();
        let Some(url) = release.setup_url.clone() else {
            open_url(&release.notes_url);
            return;
        };
        self.update = UpdateState::Downloading(release);
        let tx = self.update_tx.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("deuclip-update-download".into())
            .spawn(move || {
                let result = updater::download(&url);
                let _ = tx.send(UpdateMsg::Downloaded(result));
                ctx.request_repaint();
            });
        if spawned.is_err() {
            self.update = UpdateState::Failed("Download-Thread nicht startbar.".to_string());
        }
    }

    fn update_banner(&mut self, ui: &mut egui::Ui) {
        let (release, downloading) = match &self.update {
            UpdateState::Available(r) => (r.clone(), false),
            UpdateState::Downloading(r) => (r.clone(), true),
            _ => return,
        };
        let ctx = ui.ctx().clone();
        Frame::new()
            .fill(theme::CARD)
            .stroke(Stroke::new(1.0, theme::ACCENT))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(format!("Update verfügbar: Version {}", release.version))
                        .strong()
                        .color(theme::ACCENT),
                );
                ui.label(Self::dim(&format!(
                    "Installiert: Version {}",
                    updater::current_version()
                )));
                if downloading {
                    ui.label(Self::dim("Lade und installiere das Update ..."));
                } else {
                    ui.horizontal(|ui| {
                        if theme::primary_button(ui, "Jetzt aktualisieren").clicked() {
                            self.start_update(&ctx);
                        }
                        if ui.button("Was ist neu").clicked() {
                            open_url(&release.notes_url);
                        }
                    });
                }
            });
    }

    // ----- Tray and background -------------------------------------------

    fn show_window(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn handle_tray(&mut self, ctx: &egui::Context) {
        while let Ok(action) = self.tray_rx.try_recv() {
            match action {
                TrayAction::Show => self.show_window(ctx),
                TrayAction::Quit => self.quit(),
                TrayAction::SaveClip(slot) => {
                    self.engine.send(Command::Save {
                        slot,
                        duration: self.duration(slot),
                    });
                }
                TrayAction::OpenLast => lastclip::open_last(),
                TrayAction::OpenFolder => self.open_output_folder(),
                TrayAction::TogglePause => self.toggle_pause(),
            }
        }
        // Keep the menu texts current.
        let labels = (
            self.cfg.duration1,
            self.cfg.duration2,
            self.rec_state == RecState::Paused,
        );
        if labels != self.tray_labels {
            self.tray_labels = labels;
            if let Some(tray) = &self.tray {
                tray.update([labels.0, labels.1], labels.2);
            }
        }
    }

    fn toggle_pause(&mut self) {
        if self.rec_state == RecState::Paused {
            self.engine.send(Command::Resume);
        } else if self.ffmpeg.is_some() {
            self.engine.send(Command::Pause);
        }
    }

    fn open_output_folder(&mut self) {
        let dir = self.cfg.output_path();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.info = Some((format!("Ordner nicht anlegbar: {e}"), true));
        } else if let Err(e) = std::process::Command::new("explorer").arg(&dir).spawn() {
            self.info = Some((format!("Explorer nicht startbar: {e}"), true));
        }
    }

    /// Closing the window hides it in the tray. Only "Beenden" really exits.
    fn handle_close(&mut self, ctx: &egui::Context) {
        if self.quitting || self.tray.is_none() {
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            if self.capturing.is_some() {
                self.end_capture();
            }
            if !self.hide_notice_shown {
                self.hide_notice_shown = true;
                notify::info(
                    "DeuClip läuft im Hintergrund",
                    "Öffne es über das Symbol im Infobereich. Beenden: Rechtsklick, Beenden.",
                );
            }
        }
    }

    /// Stops the capture and exits the process.
    fn quit(&mut self) -> ! {
        self.quitting = true;
        self.tray = None;
        self.hotkeys.suspend();
        self.engine.shutdown();
        log::info!("DeuClip exited");
        log::logger().flush();
        std::process::exit(0)
    }

    // ----- User interface ------------------------------------------------

    fn dim(text: &str) -> RichText {
        RichText::new(text).color(theme::TEXT_DIM).size(13.0)
    }

    fn error_text(text: &str) -> RichText {
        RichText::new(text).color(theme::ERROR).size(13.0)
    }

    fn ffmpeg_banner(&self, ui: &mut egui::Ui) {
        Frame::new()
            .fill(theme::CARD)
            .stroke(Stroke::new(1.0, theme::ERROR))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("FFmpeg nicht gefunden")
                        .strong()
                        .color(theme::ERROR),
                );
                ui.label(
                    RichText::new(
                        "Lege ffmpeg.exe neben DeuClip.exe, nimm es in den PATH auf oder wähle \
                         den Pfad unten im Bereich System. Download: ffmpeg.org (Windows-Build \
                         ab Version 6.0).",
                    )
                    .color(theme::TEXT_DIM),
                );
            });
    }

    fn hotkeys_card(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        theme::card(ui, "Hotkeys", |ui| {
            egui::Grid::new("hotkey_grid")
                .num_columns(4)
                .spacing([14.0, 10.0])
                .show(ui, |ui| {
                    ui.label(Self::dim(""));
                    ui.label(Self::dim("Tastenkombination"));
                    ui.label(Self::dim("Clip-Dauer"));
                    ui.label(Self::dim(""));
                    ui.end_row();

                    for slot in 0..2 {
                        ui.label(format!("Hotkey {}", slot + 1));

                        let capturing = self.capturing == Some(slot);
                        let text = if capturing {
                            "Taste drücken ...".to_string()
                        } else {
                            self.spec(slot).label()
                        };
                        let mut button =
                            Button::new(RichText::new(text).strong()).min_size(vec2(200.0, 30.0));
                        if capturing {
                            button = button.stroke(Stroke::new(1.5, theme::ACCENT));
                        }
                        if ui.add(button).clicked() {
                            if capturing {
                                self.end_capture();
                            } else {
                                self.start_capture(slot);
                            }
                        }

                        ui.horizontal(|ui| {
                            let r = ui.add(
                                TextEdit::singleline(&mut self.dur_text[slot])
                                    .desired_width(56.0)
                                    .align(egui::Align2::CENTER_CENTER),
                            );
                            ui.label(Self::dim("Sekunden"));
                            if r.changed() {
                                match self.dur_text[slot].trim().parse::<u32>() {
                                    Ok(v) if (MIN_DURATION..=MAX_DURATION).contains(&v) => {
                                        if slot == 0 {
                                            self.cfg.duration1 = v;
                                        } else {
                                            self.cfg.duration2 = v;
                                        }
                                        self.dur_err[slot] = None;
                                        changed = true;
                                    }
                                    _ => {
                                        self.dur_err[slot] = Some(format!(
                                            "Dauer {}: bitte eine ganze Zahl von {} bis {} eingeben.",
                                            slot + 1,
                                            MIN_DURATION,
                                            MAX_DURATION
                                        ));
                                    }
                                }
                            }
                            if r.lost_focus() && self.dur_err[slot].is_some() {
                                self.dur_text[slot] = self.duration(slot).to_string();
                                self.dur_err[slot] = None;
                            }
                        });

                        if ui
                            .button("Jetzt speichern")
                            .on_hover_text("Löst diesen Hotkey ohne Tastendruck aus")
                            .clicked()
                        {
                            self.engine.send(Command::Save {
                                slot,
                                duration: self.duration(slot),
                            });
                        }
                        ui.end_row();
                    }

                    // Third hotkey: open the last clip.
                    ui.label("Clip öffnen");
                    let capturing = self.capturing == Some(2);
                    let text = if capturing {
                        "Taste drücken ...".to_string()
                    } else {
                        self.spec(2).label()
                    };
                    let mut button =
                        Button::new(RichText::new(text).strong()).min_size(vec2(200.0, 30.0));
                    if capturing {
                        button = button.stroke(Stroke::new(1.5, theme::ACCENT));
                    }
                    if ui.add(button).clicked() {
                        if capturing {
                            self.end_capture();
                        } else {
                            self.start_capture(2);
                        }
                    }
                    ui.label(Self::dim("öffnet den letzten Clip"));
                    if ui.button("Jetzt öffnen").clicked() {
                        lastclip::open_last();
                    }
                    ui.end_row();
                });

            if self.capturing.is_some() {
                ui.label(Self::dim(
                    "Gewünschte Kombination drücken. Esc bricht ab.",
                ));
            }
            for slot in 0..3 {
                if let Some(e) = &self.hk_err[slot] {
                    ui.label(Self::error_text(&format!("{}: {}", Self::slot_name(slot), e)));
                }
            }
            for slot in 0..2 {
                if let Some(e) = &self.dur_err[slot] {
                    ui.label(Self::error_text(e));
                }
            }
            if let Some(e) = &self.hotkeys.init_error {
                ui.label(Self::error_text(e));
            }
        });
        changed
    }

    fn capture_card(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        theme::card(ui, "Aufnahme", |ui| {
            let paused = self.rec_state == RecState::Paused;
            let label = if paused { "Aufnahme fortsetzen" } else { "Aufnahme pausieren" };
            if ui.add_enabled(self.ffmpeg.is_some(), Button::new(label)).clicked() {
                self.toggle_pause();
            }
            egui::Grid::new("capture_grid")
                .num_columns(2)
                .spacing([14.0, 10.0])
                .show(ui, |ui| {
                    ui.label("Bildschirm");
                    let options: Vec<(u32, String)> = self
                        .monitors
                        .iter()
                        .map(|m| (m.index, m.label()))
                        .collect();
                    changed |= theme::dropdown(
                        ui,
                        "monitor",
                        340.0,
                        &options,
                        &mut self.cfg.monitor_index,
                    );
                    ui.end_row();

                    ui.label("Bildrate");
                    let mut fps_options: Vec<(u32, String)> = [24u32, 30, 60]
                        .iter()
                        .map(|f| (*f, format!("{f} FPS")))
                        .collect();
                    if !fps_options.iter().any(|(f, _)| *f == self.cfg.fps) {
                        fps_options.push((self.cfg.fps, format!("{} FPS", self.cfg.fps)));
                    }
                    changed |= theme::dropdown(ui, "fps", 340.0, &fps_options, &mut self.cfg.fps);
                    ui.end_row();

                    ui.label("Clip-Auflösung");
                    let mut scale_options: Vec<(u32, String)> = vec![
                        (0, "Wie Bildschirm (keine Skalierung)".to_string()),
                        (2160, "2160p (4K)".to_string()),
                        (1440, "1440p (QHD)".to_string()),
                        (1080, "1080p (Full HD)".to_string()),
                        (720, "720p (HD)".to_string()),
                        (480, "480p (SD)".to_string()),
                    ];
                    if !scale_options.iter().any(|(h, _)| *h == self.cfg.scale_height) {
                        scale_options.push((
                            self.cfg.scale_height,
                            format!("{}p (benutzerdefiniert)", self.cfg.scale_height),
                        ));
                    }
                    changed |= theme::dropdown(
                        ui,
                        "scale",
                        340.0,
                        &scale_options,
                        &mut self.cfg.scale_height,
                    );
                    ui.end_row();

                    ui.label("Qualität");
                    let quality_options: Vec<(Quality, String)> = Quality::ALL
                        .iter()
                        .map(|q| (*q, q.label().to_string()))
                        .collect();
                    changed |= theme::dropdown(
                        ui,
                        "quality",
                        340.0,
                        &quality_options,
                        &mut self.cfg.quality,
                    );
                    ui.end_row();
                });
            ui.add_space(4.0);
            let mut system_audio = self.cfg.audio_system;
            if theme::checkbox(ui, &mut system_audio, "Systemton aufnehmen").changed() {
                self.cfg.audio_system = system_audio;
                changed = true;
            }
            if self.cfg.audio_system {
                ui.horizontal(|ui| {
                    ui.add_space(26.0);
                    let mut all = self.cfg.audio_all_devices;
                    if theme::checkbox(ui, &mut all, "Alle Ausgabegeräte aufnehmen")
                        .on_hover_text(
                            "Aus: nur das Windows-Standardgerät. Reicht ein virtueller Mixer den Ton \
                             an ein zweites Gerät weiter, ist er mit dieser Option doppelt zu hören.",
                        )
                        .changed()
                    {
                        self.cfg.audio_all_devices = all;
                        changed = true;
                    }
                });
            }
            let mut mic_audio = self.cfg.audio_mic;
            if theme::checkbox(ui, &mut mic_audio, "Mikrofon aufnehmen").changed() {
                self.cfg.audio_mic = mic_audio;
                changed = true;
            }
            ui.label(Self::dim(
                "Kleinere Clip-Auflösung spart Speicherplatz und Last. 4K nimmt nur bei einem 4K-Bildschirm in voller Größe auf, es wird nie hochskaliert. Änderungen starten die Aufnahme neu.",
            ));
        });
        changed
    }

    fn output_card(&mut self, ui: &mut egui::Ui) {
        theme::card(ui, "Ausgabeordner", |ui| {
            Frame::new()
                .fill(theme::INPUT)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(10, 7))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(Label::new(RichText::new(&self.cfg.output_dir)).truncate());
                });
            ui.horizontal(|ui| {
                if theme::primary_button(ui, "Ordner wählen ...").clicked() {
                    self.pick_folder(&ui.ctx().clone());
                }
                if ui.button("Im Explorer öffnen").clicked() {
                    let dir = self.cfg.output_path();
                    if let Err(e) = std::fs::create_dir_all(&dir) {
                        self.info = Some((format!("Ordner nicht anlegbar: {e}"), true));
                    } else if let Err(e) = std::process::Command::new("explorer").arg(&dir).spawn()
                    {
                        self.info = Some((format!("Explorer nicht startbar: {e}"), true));
                    }
                }
            });
        });
    }

    fn system_card(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        theme::card(ui, "System", |ui| {
            ui.label(Self::dim("FFmpeg"));
            match &self.ffmpeg {
                Some(p) => {
                    ui.add(Label::new(RichText::new(p.to_string_lossy().into_owned())).truncate());
                }
                None => {
                    ui.label(Self::error_text("Nicht gefunden"));
                }
            }
            ui.horizontal(|ui| {
                if ui.button("ffmpeg.exe wählen ...").clicked() {
                    self.pick_ffmpeg(&ui.ctx().clone());
                }
                if ui.button("Erneut suchen").clicked() {
                    changed = true;
                }
                if !self.cfg.ffmpeg_path.is_empty() && ui.button("Automatisch suchen").clicked() {
                    self.cfg.ffmpeg_path.clear();
                    changed = true;
                }
            });
            ui.add_space(4.0);
            let mut sound = self.cfg.sound;
            if theme::checkbox(ui, &mut sound, "Ton beim Speichern (auch im Vollbild hörbar)").changed() {
                self.cfg.sound = sound;
                changed = true;
            }
            let mut auto = self.autostart;
            if theme::checkbox(ui, &mut auto, "Mit Windows starten (im Hintergrund)").changed() {
                match autostart::set_enabled(auto) {
                    Ok(()) => {
                        self.autostart = auto;
                        self.autostart_err = None;
                    }
                    Err(e) => {
                        log::error!("{e}");
                        self.autostart_err = Some(e);
                    }
                }
            }
            if let Some(e) = &self.autostart_err {
                ui.label(Self::error_text(e));
            }
            ui.add_space(4.0);
            let mut check = self.cfg.check_updates;
            if theme::checkbox(ui, &mut check, "Automatisch nach Updates suchen").changed() {
                self.cfg.check_updates = check;
                changed = true;
            }
            ui.horizontal(|ui| {
                let busy = matches!(self.update, UpdateState::Checking | UpdateState::Downloading(_));
                if ui
                    .add_enabled(!busy, Button::new("Jetzt nach Updates suchen"))
                    .clicked()
                {
                    self.check_updates_now(&ui.ctx().clone());
                }
                let status = match &self.update {
                    UpdateState::Idle => format!("Installiert: Version {}", updater::current_version()),
                    UpdateState::Checking => "Suche ...".to_string(),
                    UpdateState::UpToDate => "DeuClip ist aktuell.".to_string(),
                    UpdateState::Available(r) => format!("Version {} ist verfügbar.", r.version),
                    UpdateState::Downloading(_) => "Lade Update ...".to_string(),
                    UpdateState::Failed(e) => e.clone(),
                };
                let failed = matches!(self.update, UpdateState::Failed(_));
                let text = if failed { Self::error_text(&status) } else { Self::dim(&status) };
                ui.add(Label::new(text).truncate());
            });
            ui.add_space(4.0);
            ui.label(Self::dim(
                "Das Schließen des Fensters lässt DeuClip im Hintergrund weiterlaufen.",
            ));
            if ui.button("DeuClip beenden").clicked() {
                self.quit();
            }
        });
        changed
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (dot, text, is_error) = match &self.rec_state {
                RecState::Running { encoder } => {
                    let monitor = self
                        .monitors
                        .iter()
                        .find(|m| m.index == self.cfg.monitor_index)
                        .map(|m| m.short_name())
                        .unwrap_or_default();
                    (
                        theme::ACCENT,
                        format!("Aufnahme läuft  |  {monitor}  |  {encoder}"),
                        false,
                    )
                }
                RecState::Starting => (theme::TEXT_DIM, "Aufnahme startet ...".to_string(), false),
                RecState::Paused => (theme::TEXT_DIM, "Aufnahme pausiert".to_string(), false),
                RecState::Reconnecting => (
                    theme::TEXT_DIM,
                    "Aufnahme unterbrochen, verbinde neu ...".to_string(),
                    false,
                ),
                RecState::Stopped => {
                    let t = if self.ffmpeg.is_none() {
                        "Gestoppt: FFmpeg fehlt"
                    } else {
                        "Aufnahme gestoppt"
                    };
                    (theme::TEXT_DIM, t.to_string(), false)
                }
                RecState::Error(m) => (theme::ERROR, format!("Fehler: {m}"), true),
            };
            theme::status_dot(ui, dot);
            let color = if is_error { theme::ERROR } else { theme::TEXT };
            let label = ui.add(Label::new(RichText::new(&text).color(color)).truncate());
            if is_error {
                label.on_hover_text(&text);
            }
            if !is_error {
                if let Some((msg, err)) = &self.info {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let color = if *err { theme::ERROR } else { theme::TEXT_DIM };
                        ui.add(Label::new(RichText::new(msg).color(color).size(13.0)).truncate());
                    });
                }
            }
        });
    }
}

/// Opens a web page in the default browser.
fn open_url(url: &str) {
    if let Err(e) = std::process::Command::new("explorer.exe").arg(url).spawn() {
        log::warn!("Could not open {url}: {e}");
    }
}

impl eframe::App for DeuClipApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.handle_tray(ctx);
        self.handle_close(ctx);
        // eframe shows the window after its first frame. After an autostart it is hidden
        // again in the first frames, so DeuClip starts in the background.
        if self.hide_frames_left > 0 {
            self.hide_frames_left -= 1;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            ctx.request_repaint();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_capture(ui);
        self.handle_close(&ui.ctx().clone());

        egui::Panel::bottom("status_bar")
            .frame(
                Frame::new()
                    .fill(theme::CARD)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .inner_margin(Margin::symmetric(16, 9)),
            )
            .show(ui, |ui| self.status_bar(ui));

        let mut changed = false;
        egui::CentralPanel::default()
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(16)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("DeuClip").heading().color(theme::TEXT).strong());
                            ui.label(Self::dim("Instant Replay für Windows"));
                        });
                        if self.ffmpeg.is_none() {
                            self.ffmpeg_banner(ui);
                        }
                        self.update_banner(ui);
                        changed |= self.hotkeys_card(ui);
                        changed |= self.capture_card(ui);
                        self.output_card(ui);
                        changed |= self.system_card(ui);
                    });
            });

        if changed {
            self.commit();
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::clear_color()
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.hotkeys.suspend();
        self.engine.shutdown();
        log::info!("DeuClip exited");
    }
}
