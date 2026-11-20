//! Notifications (`cx.send_notification`): Windows App SDK app
//! notifications (`AppNotificationManager`), which work unpackaged and
//! self-contained. The toast's XML comes from `toast.rs`: title, subtitle
//! and body, buttons (critical for destructive ones), reply fields, sound.
//!
//! **Registration.** `Register()` gives the unpackaged app an identity for
//! notifications, per user (`HKCU\Software\Classes\AppUserModelId\…`, and
//! the COM server a click starts: the executable). It runs the first time
//! the app notifies or asks about notifications, then at every startup (a
//! marker in the app's data directory says so), so a click on an earlier
//! notification reaches the running app instead of starting another one;
//! and when a click launched the app. The app unregisters its COM
//! server when it quits (`Unregister`), so clicks then start it again;
//! `PRISM_NOTIFICATIONS_CLEANUP=1` removes everything instead
//! (`UnregisterAll`, and its notifications from the notification center):
//! tests leave nothing behind.
//!
//! **Clicks** on the notification or a button reach the app while it runs
//! (`NotificationInvoked`, on a worker thread) or start it
//! (`AppInstance::GetActivatedEventArgs`): `on_notification_response`
//! gets the id, the button and a reply's text, and the window comes
//! forward unless the button is a background one.
//!
//! Delays are timers in the app (like on Linux: no scheduled notifications
//! for an unpackaged app). The notification's badge goes on the taskbar
//! button (`taskbar.rs`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use prism_core::surfaces::Notification;
use prism_core::{NotificationResponse, PermissionState};
use windows::Foundation::TypedEventHandler;
use windows::Win32::UI::WindowsAndMessaging::{
    IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
};
use windows_core::{HSTRING, Interface, Result};

use crate::shell;
use crate::toast::{self, Clicked};
use crate::win::{env, log};
use crate::winui::bindings::Microsoft::Windows::AppLifecycle::{
    AppInstance, ExtendedActivationKind,
};
use crate::winui::bindings::Microsoft::Windows::AppNotifications::{
    AppNotification, AppNotificationActivatedEventArgs, AppNotificationManager,
    AppNotificationSetting,
};

thread_local! {
    /// `None`: not tried yet; `Some(false)`: it failed.
    static REGISTERED: Cell<Option<bool>> = const { Cell::new(None) };
    /// `<data>\notifications`: the app has registered before.
    static MARKER: RefCell<Option<std::path::PathBuf>> = const { RefCell::new(None) };
    /// Delayed notifications: id → timer.
    static PENDING: RefCell<HashMap<String, usize>> = RefCell::new(HashMap::new());
}

/// Whether app notifications work here (`AppNotificationManager::IsSupported`).
pub fn supported() -> bool {
    AppNotificationManager::IsSupported().unwrap_or_else(|e| {
        log!("notifications: {e}");
        false
    })
}

/// Whether a click on a notification started this process.
fn launched_by_click() -> bool {
    std::env::args().any(|a| a.starts_with("----AppNotificationActivated:"))
}

/// At startup: registers when the app has notified before or a click
/// launched it, and hands that click to the app once it runs.
pub fn start(data: &std::path::Path) {
    let marker = data.join("notifications");
    let before = marker.exists();
    MARKER.with(|m| *m.borrow_mut() = Some(marker));
    let clicked = launched_by_click();
    if !(before || clicked) || !register() || !clicked {
        return;
    }
    let args = AppInstance::GetCurrent().and_then(|i| i.GetActivatedEventArgs());
    match args {
        Ok(a) if a.Kind().ok() == Some(ExtendedActivationKind::AppNotification) => {
            match a
                .Data()
                .and_then(|d| d.cast::<AppNotificationActivatedEventArgs>())
            {
                Ok(args) => invoked(&args),
                Err(e) => log!("notification activation: {e}"),
            }
        }
        Ok(_) => {}
        Err(e) => log!("notification activation: {e}"),
    }
}

/// Registers the app for notifications (once).
pub fn register() -> bool {
    if let Some(done) = REGISTERED.get() {
        return done;
    }
    let r = (|| -> Result<()> {
        let manager = AppNotificationManager::Default()?;
        // Before `Register`: the COM server delivers clicks from then on.
        manager.NotificationInvoked(&TypedEventHandler::new(
            |_, args: windows_core::Ref<AppNotificationActivatedEventArgs>| {
                if let Some(args) = args.as_ref() {
                    invoked(args);
                }
                Ok(())
            },
        ))?;
        manager.Register()
    })();
    let ok = match r {
        Ok(()) => {
            MARKER.with(|m| {
                if let Some(path) = m.borrow().as_ref() {
                    let _ = std::fs::write(path, "");
                }
            });
            true
        }
        Err(e) => {
            log!("notifications: cannot register the app: {e}");
            false
        }
    };
    REGISTERED.set(Some(ok));
    ok
}

/// A click on a notification or a button (any thread).
fn invoked(args: &AppNotificationActivatedEventArgs) {
    let argument = args.Argument().map(|a| a.to_string()).unwrap_or_default();
    let Some(clicked) = toast::parse(&argument) else {
        return;
    };
    let text = args.UserInput().ok().and_then(|inputs| {
        let key = HSTRING::from(toast::input_id(&clicked.action));
        inputs
            .HasKey(&key)
            .ok()
            .filter(|has| *has)
            .and_then(|_| inputs.Lookup(&key).ok())
            .map(|v| v.to_string())
    });
    shell::on_ui(move || respond(clicked, text));
}

fn respond(c: Clicked, text: Option<String>) {
    if c.foreground {
        let main = crate::imp::main_hwnd();
        unsafe {
            if IsIconic(main).as_bool() {
                let _ = ShowWindow(main, SW_RESTORE);
            }
            let _ = SetForegroundWindow(main);
        }
    }
    let response = NotificationResponse {
        id: c.id,
        action: (!c.action.is_empty()).then_some(c.action),
        text,
    };
    let link = (!c.link.is_empty()).then_some(c.link);
    crate::imp::defer(Box::new(move |s| {
        let patches = s.runtime.notification_response(response, link);
        crate::imp::commit(s, patches);
    }));
}

/// Shows `n` now, or after its delay.
pub fn notify(n: Notification) {
    cancel_pending(&n.id);
    match n.delay {
        Some(d) if !d.is_zero() => {
            let id = n.id.clone();
            let ms = d.as_millis().min(u32::MAX as u128) as u32;
            let timer = shell::timer(ms, move || {
                PENDING.with(|p| p.borrow_mut().remove(&n.id));
                show(&n);
            });
            PENDING.with(|p| p.borrow_mut().insert(id, timer));
        }
        _ => show(&n),
    }
}

fn show(n: &Notification) {
    if !register() {
        return;
    }
    let r = (|| -> Result<()> {
        let notification = AppNotification::CreateInstance(&HSTRING::from(toast::payload(n)))?;
        // The same id replaces the notification shown before.
        notification.SetTag(&HSTRING::from(toast::short(&n.id)))?;
        if let Some(t) = n.thread.as_deref().filter(|t| !t.is_empty()) {
            notification.SetGroup(&HSTRING::from(toast::short(t)))?;
        }
        let manager = AppNotificationManager::Default()?;
        manager.Show(&notification)?;
        if env("PRISM_TRACE").is_some() {
            log!(
                "notification {:?} shown (id {}, setting {:?})",
                n.id,
                notification.Id()?,
                manager.Setting()?
            );
        }
        Ok(())
    })();
    if let Err(e) = r {
        log!("notification {:?}: {e}", n.id);
    }
    if let Some(b) = n.badge {
        crate::taskbar::set_badge(crate::imp::main_hwnd(), Some(b));
    }
}

fn cancel_pending(id: &str) {
    if let Some(timer) = PENDING.with(|p| p.borrow_mut().remove(id)) {
        shell::cancel(timer);
    }
}

/// Withdraws a pending or shown notification.
pub fn cancel(id: &str) {
    cancel_pending(id);
    if REGISTERED.get() != Some(true) {
        return;
    }
    let r = AppNotificationManager::Default()
        .and_then(|m| m.RemoveByTagAsync(&HSTRING::from(toast::short(id))));
    if let Err(e) = r {
        log!("notification {id:?}: {e}");
    }
}

/// Whether the user lets the app notify (Settings › System ›
/// Notifications).
pub fn permission() -> PermissionState {
    if !register() {
        return PermissionState::Unavailable;
    }
    match AppNotificationManager::Default().and_then(|m| m.Setting()) {
        Ok(AppNotificationSetting::Enabled) => PermissionState::Granted,
        Ok(AppNotificationSetting::Unsupported) => PermissionState::Unavailable,
        Ok(_) => PermissionState::Denied,
        Err(e) => {
            log!("notifications: {e}");
            PermissionState::Unavailable
        }
    }
}

/// The app quits: its COM server goes (clicks start the app again), or
/// everything with `PRISM_NOTIFICATIONS_CLEANUP=1`.
pub fn quit() {
    PENDING.with(|p| {
        for (_, t) in p.borrow_mut().drain() {
            shell::cancel(t);
        }
    });
    if REGISTERED.get() != Some(true) {
        return;
    }
    let Ok(manager) = AppNotificationManager::Default() else {
        return;
    };
    if env("PRISM_NOTIFICATIONS_CLEANUP").is_some() {
        if let Ok(op) = manager.RemoveAllAsync() {
            let _ = op.join();
        }
        match manager.UnregisterAll() {
            Ok(()) => log!("notifications: registration removed"),
            Err(e) => log!("notifications: UnregisterAll: {e}"),
        }
        MARKER.with(|m| {
            if let Some(path) = m.borrow().as_ref() {
                let _ = std::fs::remove_file(path);
            }
        });
    } else if let Err(e) = manager.Unregister() {
        log!("notifications: Unregister: {e}");
    }
}
