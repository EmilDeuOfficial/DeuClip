//! Tray icon with a small menu. Lets DeuClip keep running in the background.

use eframe::egui;
use std::sync::mpsc::Sender;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// What the user asked for through the tray icon (or a second program start).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
}

impl Tray {
    /// Must be created on the main thread. Events are sent through `tx` and wake the GUI.
    pub fn new(ctx: egui::Context, tx: Sender<TrayAction>) -> Result<Tray, String> {
        let menu = Menu::new();
        let open = MenuItem::new("DeuClip öffnen", true, None);
        let quit = MenuItem::new("Beenden", true, None);
        menu.append(&open)
            .and_then(|_| menu.append(&quit))
            .map_err(|e| format!("Tray menu could not be built: {e}"))?;
        let open_id = open.id().clone();
        let quit_id = quit.id().clone();

        {
            let tx = tx.clone();
            let ctx = ctx.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let action = if event.id == open_id {
                    Some(TrayAction::Show)
                } else if event.id == quit_id {
                    Some(TrayAction::Quit)
                } else {
                    None
                };
                if let Some(action) = action {
                    let _ = tx.send(action);
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
        Ok(Tray { _icon: tray })
    }
}
