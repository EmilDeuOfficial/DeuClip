//! Tray icon with a menu. Lets DeuClip keep running in the background and offers the
//! most used actions without opening the window.

use eframe::egui;
use std::sync::mpsc::Sender;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// What the user asked for through the tray icon (or a second program start).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Quit,
    /// Saves a clip with the length of hotkey 1 or 2 (slot 0 or 1).
    SaveClip(usize),
    OpenLast,
    OpenFolder,
    TogglePause,
}

pub struct Tray {
    icon: TrayIcon,
    save: [MenuItem; 2],
    pause: MenuItem,
}

impl Tray {
    /// Must be created on the main thread. Events are sent through `tx` and wake the GUI.
    pub fn new(ctx: egui::Context, tx: Sender<TrayAction>) -> Result<Tray, String> {
        let menu_err = |e: tray_icon::menu::Error| format!("Tray menu could not be built: {e}");

        let open = MenuItem::new("DeuClip öffnen", true, None);
        let save1 = MenuItem::new("Clip speichern", true, None);
        let save2 = MenuItem::new("Clip speichern", true, None);
        let last = MenuItem::new("Letzten Clip öffnen", true, None);
        let folder = MenuItem::new("Clips-Ordner öffnen", true, None);
        let pause = MenuItem::new("Aufnahme pausieren", true, None);
        let quit = MenuItem::new("Beenden", true, None);
        let sep1 = PredefinedMenuItem::separator();
        let sep2 = PredefinedMenuItem::separator();
        let sep3 = PredefinedMenuItem::separator();

        let menu = Menu::new();
        menu.append_items(&[
            &open, &sep1, &save1, &save2, &last, &folder, &sep2, &pause, &sep3, &quit,
        ])
        .map_err(menu_err)?;

        let actions: Vec<(MenuId, TrayAction)> = vec![
            (open.id().clone(), TrayAction::Show),
            (save1.id().clone(), TrayAction::SaveClip(0)),
            (save2.id().clone(), TrayAction::SaveClip(1)),
            (last.id().clone(), TrayAction::OpenLast),
            (folder.id().clone(), TrayAction::OpenFolder),
            (pause.id().clone(), TrayAction::TogglePause),
            (quit.id().clone(), TrayAction::Quit),
        ];

        {
            let tx = tx.clone();
            let ctx = ctx.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                if let Some((_, action)) = actions.iter().find(|(id, _)| *id == event.id) {
                    let _ = tx.send(*action);
                    ctx.request_repaint();
                }
            }));
        }
        {
            let tx = tx.clone();
            let ctx = ctx.clone();
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                let show = matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } | TrayIconEvent::DoubleClick { .. }
                );
                if show {
                    let _ = tx.send(TrayAction::Show);
                    ctx.request_repaint();
                }
            }));
        }

        let data = crate::theme::app_icon();
        let icon = tray_icon::Icon::from_rgba(data.rgba, data.width, data.height)
            .map_err(|e| format!("Tray icon is invalid: {e}"))?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("DeuClip")
            .with_icon(icon)
            .build()
            .map_err(|e| format!("Tray icon could not be created: {e}"))?;
        Ok(Tray {
            icon: tray,
            save: [save1, save2],
            pause,
        })
    }

    /// Updates the menu texts: clip lengths of the two save entries and the pause entry.
    pub fn update(&self, durations: [u32; 2], paused: bool) {
        for (item, d) in self.save.iter().zip(durations) {
            item.set_text(format!("Clip speichern ({d} s)"));
        }
        self.pause.set_text(if paused {
            "Aufnahme fortsetzen"
        } else {
            "Aufnahme pausieren"
        });
        let tip = if paused {
            "DeuClip (Aufnahme pausiert)"
        } else {
            "DeuClip"
        };
        let _ = self.icon.set_tooltip(Some(tip));
    }
}
