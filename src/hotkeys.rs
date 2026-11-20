//! Global hotkeys: registration, key mapping and the listener thread.

use crate::capture::Command;
use crate::config::HotkeySpec;
use eframe::egui::Key;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

macro_rules! key_map {
    ($key:expr; $($k:ident => $c:ident),* $(,)?) => {
        match $key {
            $(Key::$k => Some(Code::$c),)*
            _ => None,
        }
    };
}

/// Maps an egui key to the hotkey code.
pub fn code_for(key: Key) -> Option<Code> {
    key_map!(key;
        A => KeyA, B => KeyB, C => KeyC, D => KeyD, E => KeyE, F => KeyF, G => KeyG,
        H => KeyH, I => KeyI, J => KeyJ, K => KeyK, L => KeyL, M => KeyM, N => KeyN,
        O => KeyO, P => KeyP, Q => KeyQ, R => KeyR, S => KeyS, T => KeyT, U => KeyU,
        V => KeyV, W => KeyW, X => KeyX, Y => KeyY, Z => KeyZ,
        Num0 => Digit0, Num1 => Digit1, Num2 => Digit2, Num3 => Digit3, Num4 => Digit4,
        Num5 => Digit5, Num6 => Digit6, Num7 => Digit7, Num8 => Digit8, Num9 => Digit9,
        F1 => F1, F2 => F2, F3 => F3, F4 => F4, F5 => F5, F6 => F6,
        F7 => F7, F8 => F8, F9 => F9, F10 => F10, F11 => F11, F12 => F12,
        F13 => F13, F14 => F14, F15 => F15, F16 => F16, F17 => F17, F18 => F18,
        F19 => F19, F20 => F20, F21 => F21, F22 => F22, F23 => F23, F24 => F24,
        ArrowUp => ArrowUp, ArrowDown => ArrowDown, ArrowLeft => ArrowLeft, ArrowRight => ArrowRight,
        Space => Space, Enter => Enter, Tab => Tab, Backspace => Backspace,
        Insert => Insert, Delete => Delete, Home => Home, End => End,
        PageUp => PageUp, PageDown => PageDown,
        Comma => Comma, Period => Period, Minus => Minus, Semicolon => Semicolon,
        Slash => Slash, Backslash => Backslash, Backtick => Backquote,
        OpenBracket => BracketLeft, CloseBracket => BracketRight, Quote => Quote,
        Equals => Equal,
    )
}

/// Readable key name for display.
pub fn key_label(name: &str) -> String {
    match name {
        "Num0" => "0".into(),
        "Num1" => "1".into(),
        "Num2" => "2".into(),
        "Num3" => "3".into(),
        "Num4" => "4".into(),
        "Num5" => "5".into(),
        "Num6" => "6".into(),
        "Num7" => "7".into(),
        "Num8" => "8".into(),
        "Num9" => "9".into(),
        other => other.to_string(),
    }
}

/// Converts the stored combination into a registrable hotkey.
pub fn to_hotkey(spec: &HotkeySpec) -> Option<HotKey> {
    let key = Key::from_name(&spec.key)?;
    let code = code_for(key)?;
    let mut mods = Modifiers::empty();
    if spec.ctrl {
        mods |= Modifiers::CONTROL;
    }
    if spec.alt {
        mods |= Modifiers::ALT;
    }
    if spec.shift {
        mods |= Modifiers::SHIFT;
    }
    if spec.win {
        mods |= Modifiers::META;
    }
    let mods = if mods.is_empty() { None } else { Some(mods) };
    Some(HotKey::new(mods, code))
}

/// Checks via Win32 whether a Windows key is currently pressed.
/// egui does not report the Win key as a modifier.
pub fn win_key_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LWIN, VK_RWIN};
    unsafe {
        (GetAsyncKeyState(VK_LWIN.0 as i32) as u16 & 0x8000) != 0
            || (GetAsyncKeyState(VK_RWIN.0 as i32) as u16 & 0x8000) != 0
    }
}

struct Shared {
    ids: Mutex<[Option<u32>; 3]>,
    durations: Mutex<[u32; 2]>,
}

pub struct HotkeyService {
    manager: Option<GlobalHotKeyManager>,
    registered: [Option<HotKey>; 3],
    shared: Arc<Shared>,
    pub init_error: Option<String>,
}

impl HotkeyService {
    /// Must be created on the main thread (which runs the message loop).
    pub fn new(commands: Sender<Command>, durations: [u32; 2]) -> Self {
        let shared = Arc::new(Shared {
            ids: Mutex::new([None, None, None]),
            durations: Mutex::new(durations),
        });

        let (manager, init_error) = match GlobalHotKeyManager::new() {
            Ok(m) => (Some(m), None),
            Err(e) => {
                log::error!("Could not create the hotkey manager: {e}");
                (
                    None,
                    Some(format!("Globale Hotkeys sind nicht verfuegbar: {e}")),
                )
            }
        };

        let thread_shared = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("deuclip-hotkeys".into())
            .spawn(move || listener(thread_shared, commands));
        if let Err(e) = spawned {
            log::error!("Could not start the hotkey thread: {e}");
        }

        Self {
            manager,
            registered: [None, None, None],
            shared,
            init_error,
        }
    }

    pub fn set_durations(&self, durations: [u32; 2]) {
        if let Ok(mut d) = self.shared.durations.lock() {
            *d = durations;
        }
    }

    /// Registers the hotkey of a slot again. Errors are worded for display.
    pub fn set_slot(&mut self, slot: usize, spec: &HotkeySpec) -> Result<(), String> {
        self.clear_slot(slot);
        let Some(manager) = self.manager.as_ref() else {
            return Err(self
                .init_error
                .clone()
                .unwrap_or_else(|| "Hotkey-Manager nicht verfuegbar".to_string()));
        };
        let Some(hotkey) = to_hotkey(spec) else {
            return Err(format!("Die Taste '{}' wird nicht unterstuetzt.", spec.key));
        };
        match manager.register(hotkey) {
            Ok(()) => {
                self.registered[slot] = Some(hotkey);
                if let Ok(mut ids) = self.shared.ids.lock() {
                    ids[slot] = Some(hotkey.id());
                }
                log::info!("Hotkey {} registered: {}", slot + 1, spec.label());
                Ok(())
            }
            Err(e) => {
                log::warn!("Hotkey {} ({}) could not be registered: {e}", slot + 1, spec.label());
                Err(format!(
                    "{} konnte nicht registriert werden. Vermutlich ist sie von einer anderen Anwendung belegt. ({e})",
                    spec.label()
                ))
            }
        }
    }

    fn clear_slot(&mut self, slot: usize) {
        if let Some(old) = self.registered[slot].take() {
            if let Some(manager) = self.manager.as_ref() {
                if let Err(e) = manager.unregister(old) {
                    log::warn!("Hotkey {} could not be released: {e}", slot + 1);
                }
            }
        }
        if let Ok(mut ids) = self.shared.ids.lock() {
            ids[slot] = None;
        }
    }

    /// Releases all hotkeys (e.g. while recording a new combination).
    pub fn suspend(&mut self) {
        self.clear_slot(0);
        self.clear_slot(1);
        self.clear_slot(2);
    }
}

fn listener(shared: Arc<Shared>, commands: Sender<Command>) {
    let receiver = GlobalHotKeyEvent::receiver();
    let mut last: [Option<Instant>; 3] = [None, None, None];
    while let Ok(event) = receiver.recv() {
        if event.state != HotKeyState::Pressed {
            continue;
        }
        let slot = match shared.ids.lock() {
            Ok(ids) => ids.iter().position(|id| *id == Some(event.id)),
            Err(_) => None,
        };
        let Some(slot) = slot else { continue };
        // Ignore key repeat while the key is held.
        if let Some(t) = last[slot] {
            if t.elapsed() < Duration::from_millis(1500) {
                continue;
            }
        }
        last[slot] = Some(Instant::now());
        // Slot 3 opens the last clip, the others save a clip.
        if slot == 2 {
            log::info!("Hotkey 3 triggered (open last clip)");
            crate::lastclip::open_last();
            continue;
        }
        let duration = shared.durations.lock().map(|d| d[slot]).unwrap_or(30);
        log::info!("Hotkey {} triggered ({} s)", slot + 1, duration);
        if commands.send(Command::Save { slot, duration }).is_err() {
            break;
        }
    }
}
