//! The app icon's badge (`cx.set_badge`, a notification's `badge`): an
//! overlay on the window's taskbar button (`ITaskbarList3::SetOverlayIcon`),
//! the number on a disc of the accent color, as Windows 11 draws badges.
//! Its text is the overlay's description, which Narrator reads.

use std::cell::Cell;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON, SM_CXSMICON};
use windows::core::HSTRING;

use crate::win::{hwnd, hwnd_value, log};

thread_local! {
    static BADGE: Cell<Option<u32>> = const { Cell::new(None) };
    static WINDOW: Cell<isize> = const { Cell::new(0) };
}

/// Shows `count` on `window`'s taskbar button; `None` or 0 clears it.
pub fn set_badge(window: HWND, count: Option<u32>) {
    let count = count.filter(|n| *n > 0);
    WINDOW.set(hwnd_value(window));
    BADGE.set(count);
    apply();
    // The taskbar button may not exist yet at startup: again in a moment.
    crate::shell::timer(1500, apply);
}

fn apply() {
    let window = hwnd(WINDOW.get());
    if window.is_invalid() {
        return;
    }
    let count = BADGE.get();
    unsafe {
        let list: ITaskbarList3 = match CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) {
            Ok(l) => l,
            Err(e) => return log!("taskbar badge: {e}"),
        };
        if list.HrInit().is_err() {
            return;
        }
        let Some(n) = count else {
            let _ = list.SetOverlayIcon(window, HICON::default(), &HSTRING::new());
            return;
        };
        let size = GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForWindow(window).max(96));
        let accent = crate::system::accent();
        let label = crate::icons::badge_label(n);
        let Some(icon) = crate::icons::badge(&label, size, accent.base, ink(accent.base)) else {
            return;
        };
        if let Err(e) = list.SetOverlayIcon(window, icon, &HSTRING::from(label.as_str())) {
            log!("taskbar badge: {e}");
        }
        // The taskbar keeps its own copy.
        let _ = DestroyIcon(icon);
    }
}

/// White or black, whichever reads on `fill`.
fn ink(fill: prism_core::Color) -> prism_core::Color {
    let luma = 0.2126 * fill.r + 0.7152 * fill.g + 0.0722 * fill.b;
    if luma > 0.6 {
        prism_core::Color::BLACK
    } else {
        prism_core::Color::WHITE
    }
}

/// The accent color changed, or Explorer restarted.
pub fn retheme() {
    if BADGE.get().is_some() {
        apply();
    }
}
