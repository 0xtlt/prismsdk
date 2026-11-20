//! The services' own hidden window (`services.rs`): work run later from
//! the message loop, work sent from other threads, timers, and the tray
//! icons' messages.
//!
//! Modal dialogs (pickers, the tray's menu) run their own message loop:
//! they start from here, never while the app's state is in use, so the
//! window keeps handling Prism's events behind them. Answers go back to the
//! state with `imp::defer`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::win::{hwnd, hwnd_value, log};

/// A boxed job in `lParam`.
const WM_RUN: u32 = WM_APP + 1;
/// The tray icons' callback message (`tray.rs`).
pub const WM_TRAY: u32 = WM_APP + 2;

/// The window, for other threads.
static WINDOW: AtomicIsize = AtomicIsize::new(0);

type Job = Box<dyn FnOnce()>;

thread_local! {
    static TIMERS: RefCell<HashMap<usize, Job>> = RefCell::new(HashMap::new());
    static NEXT_TIMER: std::cell::Cell<usize> = const { std::cell::Cell::new(100) };
    /// `TaskbarCreated`: Explorer started again (the tray icons are gone).
    static TASKBAR_CREATED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Creates the window (once, on the UI thread).
pub fn create() -> HWND {
    let existing = WINDOW.load(Ordering::Acquire);
    if existing != 0 {
        return hwnd(existing);
    }
    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class = w!("PrismServices");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        // A hidden top-level window, not a message-only one: those miss
        // the `TaskbarCreated` broadcast.
        let h = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class,
            w!("Prism services"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .unwrap_or_else(|e| {
            log!("services window: {e}");
            HWND::default()
        });
        TASKBAR_CREATED.set(RegisterWindowMessageW(w!("TaskbarCreated")));
        WINDOW.store(hwnd_value(h), Ordering::Release);
        h
    }
}

pub fn window() -> HWND {
    hwnd(WINDOW.load(Ordering::Acquire))
}

fn post(job: Job) {
    let raw = Box::into_raw(Box::new(job));
    let posted = unsafe { PostMessageW(Some(window()), WM_RUN, WPARAM(0), LPARAM(raw as isize)) };
    if posted.is_err() {
        // SAFETY: not posted, so still ours.
        drop(unsafe { Box::from_raw(raw) });
    }
}

/// Runs `f` from the message loop, once the current work is done (UI
/// thread only).
pub fn later(f: impl FnOnce() + 'static) {
    post(Box::new(f));
}

/// Runs `f` on the UI thread (from any thread).
pub fn on_ui(f: impl FnOnce() + Send + 'static) {
    post(Box::new(f));
}

/// Runs `f` once after `ms` milliseconds (UI thread). Returns the timer's
/// id for [`cancel`].
pub fn timer(ms: u32, f: impl FnOnce() + 'static) -> usize {
    let id = NEXT_TIMER.get();
    NEXT_TIMER.set(id + 1);
    TIMERS.with(|t| t.borrow_mut().insert(id, Box::new(f)));
    unsafe {
        SetTimer(Some(window()), id, ms.max(1), None);
    }
    id
}

pub fn cancel(id: usize) {
    TIMERS.with(|t| t.borrow_mut().remove(&id));
    unsafe {
        let _ = KillTimer(Some(window()), id);
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_RUN => {
                let job = Box::from_raw(lp.0 as *mut Job);
                job();
                LRESULT(0)
            }
            WM_TIMER => {
                let _ = KillTimer(Some(h), wp.0);
                if let Some(f) = TIMERS.with(|t| t.borrow_mut().remove(&wp.0)) {
                    f();
                }
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                let what = if lp.0 != 0 {
                    windows::core::PCWSTR(lp.0 as *const u16)
                        .to_string()
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                // Light or dark taskbar, accent color.
                if what == "ImmersiveColorSet" {
                    crate::tray::retheme();
                    crate::taskbar::retheme();
                }
                LRESULT(0)
            }
            WM_TRAY => {
                crate::tray::message(wp, lp);
                LRESULT(0)
            }
            m if m != 0 && m == TASKBAR_CREATED.get() => {
                crate::tray::taskbar_created();
                crate::taskbar::retheme();
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}
