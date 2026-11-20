//! Small on-screen notification at the bottom left: "clip saved" or "failed".
//!
//! A borderless, always-on-top window that never takes focus. It runs in its own thread,
//! so it also works while the main window is hidden in the tray, and it is excluded from
//! screen capture so it never ends up in a clip. A "clip saved" notification can be
//! clicked to open the clip, every other notification lets mouse clicks pass through.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW,
    EndPaint, FillRect, GetMonitorInfoW, InvalidateRect, MonitorFromPoint, SelectObject,
    SetBkMode, SetTextColor, SetWindowRgn, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_TOP, DT_WORDBREAK, FW_NORMAL,
    FW_SEMIBOLD, HDC, MONITORINFO, MONITOR_DEFAULTTOPRIMARY, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetMessageW,
    GetWindowLongPtrW, KillTimer, LoadCursorW, PostMessageW, RegisterClassW,
    SetCursor, SetLayeredWindowAttributes, SetTimer, SetWindowDisplayAffinity, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, TranslateMessage, GWL_EXSTYLE, HWND_TOPMOST, IDC_ARROW, IDC_HAND,
    LWA_ALPHA, MA_NOACTIVATE, MSG, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE,
    WDA_EXCLUDEFROMCAPTURE, WM_APP, WM_ERASEBKGND, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_PAINT,
    WM_SETCURSOR, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

const BG: (u8, u8, u8) = (0x25, 0x25, 0x25);
const BORDER: (u8, u8, u8) = (0x3A, 0x3A, 0x3A);
const ACCENT: (u8, u8, u8) = (0x22, 0xC5, 0x5E);
const ERROR: (u8, u8, u8) = (0xC8, 0x6B, 0x66);
const TEXT: (u8, u8, u8) = (0xE5, 0xE5, 0xE5);
const TEXT_DIM: (u8, u8, u8) = (0x9A, 0x9A, 0x9A);

const SHOW_MS: u32 = 4500;
const TIMER_HIDE: usize = 1;
const TIMER_FADE: usize = 2;
/// Re-asserts the top position, some games keep pushing their own window to the top.
const TIMER_TOP: usize = 3;

struct State {
    ok: bool,
    /// A click opens the last clip.
    clickable: bool,
    title: String,
    detail: String,
    alpha: u8,
}

static STATE: Mutex<State> = Mutex::new(State {
    ok: true,
    clickable: false,
    title: String::new(),
    detail: String::new(),
    alpha: 255,
});
/// A point on the screen that should show the notification (center of the recorded monitor).
static ANCHOR: Mutex<Option<(i32, i32)>> = Mutex::new(None);
static HWND_RAW: AtomicIsize = AtomicIsize::new(0);
/// A notification that arrived before the window existed.
static PENDING: AtomicBool = AtomicBool::new(false);

fn rgb(c: (u8, u8, u8)) -> COLORREF {
    COLORREF(c.0 as u32 | (c.1 as u32) << 8 | (c.2 as u32) << 16)
}

fn scale() -> f32 {
    (unsafe { GetDpiForSystem() } as f32 / 96.0).max(1.0)
}

/// Starts the overlay thread. Call once at startup.
pub fn init() {
    let spawned = std::thread::Builder::new()
        .name("deuclip-overlay".into())
        .spawn(|| unsafe { run() });
    if let Err(e) = spawned {
        log::warn!("Could not start the overlay thread: {e}");
    }
}

/// Sets the screen position where the notification appears (any point on that monitor).
pub fn set_anchor(x: i32, y: i32) {
    if let Ok(mut a) = ANCHOR.lock() {
        *a = Some((x, y));
    }
}

/// Shows the notification for a few seconds. A new one replaces the current one.
pub fn show(ok: bool, title: &str, detail: &str) {
    post(ok, false, title, detail);
}

/// Like `show` for a saved clip: a click on the notification opens the clip.
pub fn show_clip(title: &str, detail: &str) {
    post(true, true, title, detail);
}

fn post(ok: bool, clickable: bool, title: &str, detail: &str) {
    if let Ok(mut s) = STATE.lock() {
        s.ok = ok;
        s.clickable = clickable;
        s.title = title.to_string();
        s.detail = detail.to_string();
    }
    let raw = HWND_RAW.load(Ordering::SeqCst);
    if raw != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(raw as *mut _)), WM_APP, WPARAM(0), LPARAM(0));
        }
    } else {
        PENDING.store(true, Ordering::SeqCst);
    }
}

unsafe fn run() {
    let hinst = match GetModuleHandleW(None) {
        Ok(h) => h,
        Err(e) => {
            log::warn!("Overlay: no module handle: {e}");
            return;
        }
    };
    let class = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: hinst.into(),
        lpszClassName: w!("DeuClipOverlay"),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        ..Default::default()
    };
    if RegisterClassW(&class) == 0 {
        log::warn!("Overlay: window class could not be registered");
        return;
    }
    let hwnd = match CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
        w!("DeuClipOverlay"),
        w!("DeuClip notification"),
        WS_POPUP,
        0,
        0,
        10,
        10,
        None,
        None,
        Some(hinst.into()),
        None,
    ) {
        Ok(h) => h,
        Err(e) => {
            log::warn!("Overlay: window could not be created: {e}");
            return;
        }
    };
    // Keep the notification out of screen captures, so it never appears in a clip.
    // DEUCLIP_OVERLAY_IN_CAPTURE=1 keeps it visible to screenshots (for testing).
    if std::env::var_os("DEUCLIP_OVERLAY_IN_CAPTURE").is_none() {
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
    }
    HWND_RAW.store(hwnd.0 as isize, Ordering::SeqCst);
    if PENDING.swap(false, Ordering::SeqCst) {
        let _ = PostMessageW(Some(hwnd), WM_APP, WPARAM(0), LPARAM(0));
    }

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

fn is_clickable() -> bool {
    STATE.lock().map(|s| s.clickable).unwrap_or(false)
}

unsafe fn present(hwnd: HWND) {
    let s = scale();
    let (w, h) = ((350.0 * s) as i32, (84.0 * s) as i32);
    let margin = (24.0 * s) as i32;

    let anchor = ANCHOR.lock().ok().and_then(|a| *a).unwrap_or((0, 0));
    let monitor = MonitorFromPoint(POINT { x: anchor.0, y: anchor.1 }, MONITOR_DEFAULTTOPRIMARY);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let area = if GetMonitorInfoW(monitor, &mut info).as_bool() {
        info.rcWork
    } else {
        RECT { left: 0, top: 0, right: 1280, bottom: 720 }
    };
    let (x, y) = (area.left + margin, area.bottom - margin - h);

    if let Ok(mut st) = STATE.lock() {
        st.alpha = 255;
    }
    // Only a clickable notification takes mouse clicks, all others let them pass through.
    let mut ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    if is_clickable() {
        ex &= !(WS_EX_TRANSPARENT.0 as isize);
    } else {
        ex |= WS_EX_TRANSPARENT.0 as isize;
    }
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex);

    let _ = KillTimer(Some(hwnd), TIMER_FADE);
    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
    let radius = (10.0 * s) as i32;
    let region = CreateRoundRectRgn(0, 0, w + 1, h + 1, radius * 2, radius * 2);
    let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    // The system owns the region after this call.
    SetWindowRgn(hwnd, Some(region), true);
    let _ = InvalidateRect(Some(hwnd), None, true);
    SetTimer(Some(hwnd), TIMER_HIDE, SHOW_MS, None);
    SetTimer(Some(hwnd), TIMER_TOP, 200, None);
}

unsafe fn hide(hwnd: HWND) {
    let _ = KillTimer(Some(hwnd), TIMER_HIDE);
    let _ = KillTimer(Some(hwnd), TIMER_FADE);
    let _ = KillTimer(Some(hwnd), TIMER_TOP);
    let _ = ShowWindow(hwnd, SW_HIDE);
}

unsafe fn fill(hdc: HDC, rect: &RECT, color: (u8, u8, u8)) {
    let brush = CreateSolidBrush(rgb(color));
    FillRect(hdc, rect, brush);
    let _ = DeleteObject(brush.into());
}

unsafe fn draw_text(hdc: HDC, text: &str, rect: &mut RECT, color: (u8, u8, u8), px: i32, weight: i32, flags: u32) {
    // DrawTextW must not get an empty buffer.
    if text.is_empty() {
        return;
    }
    let font = CreateFontW(
        -px,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        CLEARTYPE_QUALITY,
        DEFAULT_PITCH.0 as u32,
        w!("Segoe UI"),
    );
    let old = SelectObject(hdc, font.into());
    SetTextColor(hdc, rgb(color));
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    DrawTextW(hdc, &mut wide, rect, windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(flags));
    SelectObject(hdc, old);
    let _ = DeleteObject(font.into());
}

unsafe fn paint(hwnd: HWND, hdc: HDC) {
    let s = scale();
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);
    let (ok, title, detail) = match STATE.lock() {
        Ok(st) => (st.ok, st.title.clone(), st.detail.clone()),
        Err(_) => (true, String::new(), String::new()),
    };
    if title.is_empty() {
        return;
    }
    let accent = if ok { ACCENT } else { ERROR };

    // Border, background and the accent bar on the left.
    fill(hdc, &client, BORDER);
    let inner = RECT { left: 1, top: 1, right: client.right - 1, bottom: client.bottom - 1 };
    fill(hdc, &inner, BG);
    let bar = RECT { left: 1, top: 1, right: 1 + (6.0 * s) as i32, bottom: client.bottom - 1 };
    fill(hdc, &bar, accent);

    SetBkMode(hdc, TRANSPARENT);
    let left = (22.0 * s) as i32;
    let right = client.right - (14.0 * s) as i32;
    let mut title_rect = RECT { left, top: (11.0 * s) as i32, right, bottom: (36.0 * s) as i32 };
    draw_text(
        hdc,
        &title,
        &mut title_rect,
        TEXT,
        (16.0 * s) as i32,
        FW_SEMIBOLD.0 as i32,
        (DT_LEFT | DT_TOP | DT_SINGLELINE | DT_END_ELLIPSIS).0,
    );
    let mut detail_rect = RECT { left, top: (37.0 * s) as i32, right, bottom: client.bottom - (8.0 * s) as i32 };
    draw_text(
        hdc,
        &detail,
        &mut detail_rect,
        TEXT_DIM,
        (13.0 * s) as i32,
        FW_NORMAL.0 as i32,
        (DT_LEFT | DT_TOP | DT_WORDBREAK | DT_END_ELLIPSIS).0,
    );
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_APP => {
            present(hwnd);
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_HIDE {
                let _ = KillTimer(Some(hwnd), TIMER_HIDE);
                SetTimer(Some(hwnd), TIMER_FADE, 16, None);
            } else if wparam.0 == TIMER_TOP {
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            } else if wparam.0 == TIMER_FADE {
                let alpha = match STATE.lock() {
                    Ok(mut st) => {
                        st.alpha = st.alpha.saturating_sub(24);
                        st.alpha
                    }
                    Err(_) => 0,
                };
                if alpha == 0 {
                    hide(hwnd);
                } else {
                    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
                }
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            paint(hwnd, hdc);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        // A click never activates the window, so a game keeps its focus.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_SETCURSOR => {
            let id = if is_clickable() { IDC_HAND } else { IDC_ARROW };
            if let Ok(cursor) = LoadCursorW(None, id) {
                SetCursor(Some(cursor));
            }
            LRESULT(1)
        }
        WM_LBUTTONUP => {
            if is_clickable() {
                hide(hwnd);
                crate::lastclip::open_last();
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
