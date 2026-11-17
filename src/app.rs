//! GUI: main window, status bar and the wiring to engine, hotkeys and configuration.

use crate::capture::{self, Command, Engine, Event, MonitorInfo, RecState, Settings};
use crate::config::{Config, HotkeySpec, Quality, MAX_DURATION, MIN_DURATION};
use crate::hotkeys::{self, HotkeyService};
use crate::{autostart, notify, theme};
use eframe::egui::{
    self, vec2, Align, Button, CornerRadius, Frame, Label, Layout, Margin, RichText, Stroke,
    TextEdit,
};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

enum Picked {
    Folder(PathBuf),
    Ffmpeg(PathBuf),
    Cancelled,
}

type CaptureKey = (Option<PathBuf>, u32, u32, u32, Quality);

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
    hk_err: [Option<String>; 2],
    capturing: Option<usize>,
    picked_tx: Sender<Picked>,
    picked_rx: Receiver<Picked>,
    picker_open: bool,
    autostart: bool,
    autostart_err: Option<String>,
    applied_key: Option<CaptureKey>,
}

impl DeuClipApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);

        let mut cfg = Config::load();
        let monitors = capture::list_monitors();
        if !monitors.iter().any(|m| m.index == cfg.monitor_index) {
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
                            notify::clip_saved(&name, *partial, *got_secs, *wanted_secs);
                        }
                        Event::SaveFailed { message, .. } => notify::error(message),
                        Event::State(RecState::Error(message)) => notify::error(message),
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
            hk_err: [None, None],
            capturing: None,
            picked_tx,
            picked_rx,
            picker_open: false,
            autostart: autostart::is_enabled(),
            autostart_err: None,
            applied_key: None,
        };
        app.apply_hotkey(0);
        app.apply_hotkey(1);
        app.commit();
        app
    }

    // ----- State and configuration ---------------------------------------

    fn spec(&self, slot: usize) -> &HotkeySpec {
        if slot == 0 {
            &self.cfg.hotkey1
        } else {
            &self.cfg.hotkey2
        }
    }

    fn set_spec(&mut self, slot: usize, spec: HotkeySpec) {
        if slot == 0 {
            self.cfg.hotkey1 = spec;
        } else {
            self.cfg.hotkey2 = spec;
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
        let retention = self.cfg.duration1.max(self.cfg.duration2) as u64 + capture::RESERVE_SECS;
        self.engine.send(Command::Configure {
            output_dir: self.cfg.output_path(),
            retention_secs: retention,
        });

        self.ffmpeg = capture::find_ffmpeg(&self.cfg.ffmpeg_path);
        let key: CaptureKey = (
            self.ffmpeg.clone(),
            self.cfg.monitor_index,
            self.cfg.fps,
            self.cfg.scale_height,
            self.cfg.quality,
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
            let other = 1 - slot;
            if spec == *self.spec(other) {
                self.end_capture();
                // Set after end_capture because apply_hotkey overwrites the error message.
                self.hk_err[slot] = Some(format!(
                    "Diese Kombination ist bereits Hotkey {} zugewiesen.",
                    other + 1
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
                });

            if self.capturing.is_some() {
                ui.label(Self::dim(
                    "Gewünschte Kombination drücken. Esc bricht ab.",
                ));
            }
            for slot in 0..2 {
                if let Some(e) = &self.hk_err[slot] {
                    ui.label(Self::error_text(&format!("Hotkey {}: {}", slot + 1, e)));
                }
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
            let mut auto = self.autostart;
            if theme::checkbox(ui, &mut auto, "Mit Windows starten").changed() {
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

impl eframe::App for DeuClipApp {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_capture(ui);

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
