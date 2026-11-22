//! Remote push for an app shipped as an MSIX (`prism build windows
//! --msix`): Windows Push Notification Services through the Windows App
//! SDK's `PushNotificationManager`. WNS needs package identity, so the zip
//! build (unpackaged, self-contained) answers "unavailable", as does a
//! package built without `[push] wns_remote_id` (Prism.toml), which the
//! build compiles in (`PRISM_WNS_REMOTE_ID`).
//!
//! - At startup, `PushReceived` is handled and the manager registered (the
//!   package's manifest names the COM server WNS starts the app with,
//!   `----WindowsAppRuntimePushServer:`); a push that started the app
//!   (`ExtendedActivationKind::Push`) is delivered once it runs.
//! - `register_for_push` creates a channel for the remote id (the Entra app
//!   registration's object id): its URI is the token (`PushService::Wns`).
//!   A channel lasts 30 days: the app asks again at each start, and a day
//!   before the channel's `ExpirationTime` while it runs, the new URI going
//!   to `on_push_token` again.
//! - Pushes are raw notifications carrying the Prism payload
//!   (`prism_push::wns`): visible ones become notifications while the app
//!   is in the background; silent ones run `on_background_push`.

use std::cell::RefCell;

use prism_core::{DeviceError, PushEvent, PushMessage, PushService, PushSubscription};
use windows::Foundation::TypedEventHandler;
use windows_core::{GUID, Interface, Result};
use windows_future::{AsyncOperationWithProgressCompletedHandler, AsyncStatus};

use crate::shell;
use crate::win::log;
use crate::winui::bindings::Microsoft::Windows::AppLifecycle::{
    AppInstance, ExtendedActivationKind,
};
use crate::winui::bindings::Microsoft::Windows::PushNotifications::{
    PushNotificationChannel, PushNotificationChannelStatus, PushNotificationCreateChannelResult,
    PushNotificationCreateChannelStatus, PushNotificationManager,
    PushNotificationReceivedEventArgs,
};

/// `[push] wns_remote_id`, compiled in by `prism build windows --msix`.
const REMOTE_ID: Option<&str> = option_env!("PRISM_WNS_REMOTE_ID");

thread_local! {
    /// The channel the app got (closed by `unregister`).
    static CHANNEL: RefCell<Option<PushNotificationChannel>> = const { RefCell::new(None) };
    /// The manager registered at startup (unregistered at quit).
    static REGISTERED: RefCell<bool> = const { RefCell::new(false) };
    /// The timer that renews the channel before it expires.
    static RENEWAL: RefCell<Option<usize>> = const { RefCell::new(None) };
}

/// Windows' `DateTime` ticks (100 ns since 1601) at the Unix epoch.
const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;

/// Now, in `DateTime` ticks.
fn now_ticks() -> i64 {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    UNIX_EPOCH_TICKS + (since.as_nanos() / 100).min(i64::MAX as u128) as i64
}

/// How long to wait before renewing a channel that expires at `expires`
/// (ticks), now being `now`: a day before it expires (half the time left
/// when less than two days remain), at least a minute, and at most what a
/// Win32 timer holds (about 24.8 days: the renewal then simply comes early).
fn renew_after_ms(expires: i64, now: i64) -> u32 {
    const MS: i64 = 10_000;
    const DAY: i64 = 24 * 3600 * 1000;
    let left = expires.saturating_sub(now) / MS;
    let wait = if left > 2 * DAY { left - DAY } else { left / 2 };
    wait.clamp(60_000, 0x7FFF_FFFF) as u32
}

/// Renews the channel `ms` from now (replacing a renewal planned before).
fn plan_renewal(ms: u32) {
    cancel_renewal();
    let id = shell::timer(ms, || {
        RENEWAL.with(|r| r.borrow_mut().take());
        log!("push: renewing the WNS channel before it expires");
        register();
    });
    RENEWAL.with(|r| *r.borrow_mut() = Some(id));
}

fn cancel_renewal() {
    if let Some(id) = RENEWAL.with(|r| r.borrow_mut().take()) {
        shell::cancel(id);
    }
}

/// Whether the process has package identity (an MSIX).
pub fn packaged() -> bool {
    windows::ApplicationModel::Package::Current().is_ok()
}

/// The remote id as a GUID.
fn remote_id() -> Option<GUID> {
    let id = REMOTE_ID?;
    GUID::try_from(id).ok()
}

/// What stands in the way of push here, if anything.
pub fn missing() -> Option<&'static str> {
    if !packaged() {
        return Some(
            "Windows push notifications (WNS) need package identity: build an MSIX \
             (prism build windows --msix)",
        );
    }
    if remote_id().is_none() {
        return Some(
            "WNS needs the app registration's object id: [push] wns_remote_id in Prism.toml",
        );
    }
    match PushNotificationManager::IsSupported() {
        Ok(true) => None,
        _ => Some("the Windows App SDK says push is not supported here"),
    }
}

/// The push service, when push works here.
pub fn service() -> Option<PushService> {
    missing().is_none().then_some(PushService::Wns)
}

/// Whether WNS started this process to deliver a push.
fn launched_by_push() -> bool {
    std::env::args().any(|a| a.starts_with("----WindowsAppRuntimePushServer:"))
}

/// At startup (UI thread): handles pushes, registers the manager, and
/// delivers the push that started the app, if one did.
pub fn start() {
    if service().is_none() {
        if launched_by_push() {
            log!("push: started by WNS, but {}", missing().unwrap_or(""));
        }
        return;
    }
    let r = (|| -> Result<()> {
        let manager = PushNotificationManager::Default()?;
        manager.PushReceived(&TypedEventHandler::new(
            |_, args: windows_core::Ref<PushNotificationReceivedEventArgs>| {
                if let Some(args) = args.as_ref() {
                    received(args, PushEvent::Received);
                }
                Ok(())
            },
        ))?;
        manager.Register()?;
        REGISTERED.with(|r| *r.borrow_mut() = true);
        let args = AppInstance::GetCurrent()?.GetActivatedEventArgs()?;
        if args.Kind()? == ExtendedActivationKind::Push {
            let push = args.Data()?.cast::<PushNotificationReceivedEventArgs>()?;
            received(&push, PushEvent::Received);
        }
        Ok(())
    })();
    if let Err(e) = r {
        log!("push: {e}");
    }
}

/// A push (any thread): the payload goes to the app on the UI thread.
fn received(args: &PushNotificationReceivedEventArgs, event: PushEvent) {
    let payload = match args.Payload() {
        Ok(p) => p.to_vec(),
        Err(e) => {
            log!("push: {e}");
            return;
        }
    };
    let Some(message) = message(&payload, event) else {
        log!(
            "push: a payload Prism does not read ({} bytes)",
            payload.len()
        );
        return;
    };
    shell::on_ui(move || deliver(message));
}

/// The Prism payload (`prism_push::wns`, UTF-8 JSON) as a message.
fn message(payload: &[u8], event: PushEvent) -> Option<PushMessage> {
    let text = std::str::from_utf8(payload).ok()?;
    let (mut m, _silent) = PushMessage::from_payload(event, text)?;
    m.service = Some(PushService::Wns);
    Some(m)
}

fn deliver(message: PushMessage) {
    crate::imp::defer(Box::new(move |s| {
        let patches = if message.event == PushEvent::Background {
            s.runtime.run_background_push(message);
            s.runtime.poll()
        } else {
            s.runtime.push_received(message, true)
        };
        crate::imp::commit(s, patches);
    }));
}

/// `cx.register_for_push()`: a channel for the remote id; its URI goes to
/// `on_push_token`.
pub fn register() {
    // `services.rs` says why, once.
    if missing().is_some() {
        answer(Err(DeviceError::Unavailable));
        return;
    }
    let Some(id) = remote_id() else { return };
    let r = (|| -> Result<()> {
        let op = PushNotificationManager::Default()?.CreateChannelAsync(id)?;
        op.SetCompleted(&AsyncOperationWithProgressCompletedHandler::<
            PushNotificationCreateChannelResult,
            PushNotificationCreateChannelStatus,
        >::new(|op, status| {
            let result = match (op.as_ref(), status) {
                (Some(op), AsyncStatus::Completed) => op.GetResults().map_err(|e| e.message()),
                (_, status) => Err(format!("channel request {status:?}")),
            };
            let outcome = result.and_then(|r| {
                if r.Status().ok() == Some(PushNotificationChannelStatus::CompletedSuccess) {
                    r.Channel().map_err(|e| e.message())
                } else {
                    let e = r.ExtendedError().map(|h| h.message()).unwrap_or_default();
                    Err(format!("WNS channel: {e}"))
                }
            });
            let send = match outcome {
                Ok(channel) => match channel.Uri().and_then(|u| u.ToString()) {
                    Ok(uri) => {
                        // Unknown: renew in a day, as if it expired in two.
                        let expires = channel
                            .ExpirationTime()
                            .map(|t| t.UniversalTime)
                            .unwrap_or_else(|_| now_ticks() + 2 * 24 * 3600 * 10_000_000);
                        Ok((uri.to_string(), channel, expires))
                    }
                    Err(e) => Err(e.message()),
                },
                Err(e) => Err(e),
            };
            shell::on_ui(move || match send {
                Ok((uri, channel, expires)) => {
                    CHANNEL.with(|slot| *slot.borrow_mut() = Some(channel));
                    plan_renewal(renew_after_ms(expires, now_ticks()));
                    answer(Ok(PushSubscription::new(PushService::Wns, uri)));
                }
                Err(e) => {
                    log!("push: {e}");
                    answer(Err(DeviceError::Failed(e)));
                }
            });
            Ok(())
        }))?;
        Ok(())
    })();
    if let Err(e) = r {
        log!("push: {e}");
        answer(Err(DeviceError::Failed(e.message())));
    }
}

fn answer(result: std::result::Result<PushSubscription, DeviceError>) {
    crate::imp::defer(Box::new(move |s| {
        let patches = s.runtime.push_registered(result);
        crate::imp::commit(s, patches);
    }));
}

/// `cx.unregister_for_push()`: closes the channel.
pub fn unregister() {
    cancel_renewal();
    if let Some(channel) = CHANNEL.with(|c| c.borrow_mut().take())
        && let Err(e) = channel.Close()
    {
        log!("push: {e}");
    }
}

/// The app quits: later pushes start it again.
pub fn quit() {
    if REGISTERED.with(|r| r.replace(false))
        && let Err(e) = PushNotificationManager::Default().and_then(|m| m.Unregister())
    {
        log!("push: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_prism_payloads() {
        let m = message(
            br#"{"title":"Order shipped","link":"/orders/42","data":{"n":3}}"#,
            PushEvent::Received,
        )
        .unwrap();
        assert_eq!(m.event, PushEvent::Received);
        assert_eq!(m.service, Some(PushService::Wns));
        assert_eq!(m.title.as_deref(), Some("Order shipped"));
        assert_eq!(m.link.as_deref(), Some("/orders/42"));
        assert_eq!(m.get("n"), Some("3"));
        let silent = message(br#"{"silent":true}"#, PushEvent::Received).unwrap();
        assert_eq!(silent.event, PushEvent::Background);
        assert!(message(b"\xff", PushEvent::Received).is_none());
        assert!(message(b"not json", PushEvent::Received).is_none());
    }

    #[test]
    fn renews_a_day_before_expiry() {
        const MIN: i64 = 60 * 10_000_000;
        const DAY: i64 = 24 * 60 * MIN;
        let now = now_ticks();
        // 30 days: a Win32 timer holds 24.8 days, so it comes early.
        assert_eq!(renew_after_ms(now + 30 * DAY, now), 0x7FFF_FFFF);
        assert_eq!(renew_after_ms(now + 10 * DAY, now), 9 * 24 * 3600 * 1000);
        assert_eq!(renew_after_ms(now + DAY, now), 12 * 3600 * 1000);
        assert_eq!(renew_after_ms(now + MIN, now), 60_000);
        assert_eq!(renew_after_ms(now - DAY, now), 60_000);
        // The epoch constant: 1970-01-01 in DateTime ticks.
        assert!(now > UNIX_EPOCH_TICKS + 56 * 365 * DAY);
    }

    /// The test process is not packaged: push says why.
    #[test]
    fn unpackaged_processes_have_no_push() {
        assert!(!packaged());
        assert_eq!(service(), None);
        assert!(missing().unwrap().contains("package identity"));
    }
}
