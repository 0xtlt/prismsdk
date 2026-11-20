//! Small Win32 helpers shared by the modules.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::WindowsAndMessaging::WM_APP;

use crate::scene::Rect;

/// Runs the queued UI events (`imp::dispatch`).
pub const WM_PRISM_DISPATCH: u32 = WM_APP + 1;
/// `RuntimeOptions::wake_ui`: polls the runtime.
pub const WM_PRISM_WAKE: u32 = WM_APP + 2;
/// UI Automation's Invoke, posted to the UI thread (node id in `wParam`).
pub const WM_PRISM_INVOKE: u32 = WM_APP + 3;
/// A picture finished loading off the UI thread.
pub const WM_PRISM_IMAGE: u32 = WM_APP + 4;
/// Shows the pending menu (`input::show_pending_menu`).
pub const WM_PRISM_MENU: u32 = WM_APP + 5;
/// Moves the keyboard focus on (`wParam` 1: backwards), from a field.
pub const WM_PRISM_FOCUS: u32 = WM_APP + 6;

/// Timer ids (`SetTimer` on the main window).
pub const TIMER_RUNTIME: usize = 1;
pub const TIMER_SECOND: usize = 2;
pub const TIMER_AUTOMATION: usize = 3;
pub const TIMER_SNAPSHOT: usize = 4;

/// A NUL-terminated UTF-16 copy of `s`.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// `HWND` from the integer the scene keeps.
pub fn hwnd(v: isize) -> HWND {
    HWND(v as *mut c_void)
}

pub fn hwnd_value(h: HWND) -> isize {
    h.0 as isize
}

pub fn loword(v: usize) -> u16 {
    (v & 0xffff) as u16
}

pub fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xffff) as u16
}

/// Signed client coordinates of a mouse message.
pub fn point_of(lparam: LPARAM) -> (i32, i32) {
    (
        (lparam.0 & 0xffff) as i16 as i32,
        ((lparam.0 >> 16) & 0xffff) as i16 as i32,
    )
}

/// A non-empty environment variable.
pub fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Seconds from an environment variable, else `default`.
pub fn env_secs(name: &str, default: f64) -> f64 {
    env(name).and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// A rectangle in view units as client pixels, rounded outward.
pub fn px_rect(r: Rect, scale: f32) -> RECT {
    RECT {
        left: (r.x * scale).floor() as i32,
        top: (r.y * scale).floor() as i32,
        right: (r.right() * scale).ceil() as i32,
        bottom: (r.bottom() * scale).ceil() as i32,
    }
}

/// Asks for `r` (view units) to be repainted.
pub fn invalidate(h: HWND, r: Option<Rect>, scale: f32) {
    unsafe {
        let _ = match r {
            Some(r) => InvalidateRect(Some(h), Some(&px_rect(r.inflate(1.), scale)), false),
            None => InvalidateRect(Some(h), None, false),
        };
    }
}

/// Logs to stderr, like the other backends (`prism-windows: …`).
macro_rules! log {
    ($($t:tt)*) => { eprintln!("prism-windows: {}", format!($($t)*)) };
}
pub(crate) use log;
