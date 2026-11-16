//! watchOS system surfaces: notifications and device APIs through the shared
//! Swift bridge, alerts through the SwiftUI host, background refresh through
//! WatchKit (`BGTaskScheduler` is `API_UNAVAILABLE(watchos)`).

use std::ffi::{CStr, CString, c_char};

use prism_core::surfaces::PlatformRequest;
use prism_core::{Capabilities, PlatformServices};

unsafe extern "C" {
    fn prism_bridge_init() -> bool;
    fn prism_notify(json: *const c_char);
    fn prism_notify_cancel(id: *const c_char);
    fn prism_notify_permission();
    fn prism_watch_bg_schedule(id: *const c_char, seconds: f64);
    fn prism_watch_bg_complete(id: *const c_char);
}

fn c(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

pub(crate) struct WatchServices {
    caps: Capabilities,
    /// Changes for the host (alerts).
    pub ops: Vec<serde_json::Value>,
}

impl WatchServices {
    pub fn new() -> Self {
        let notifications = unsafe { prism_bridge_init() };
        // No biometric sensor; the passcode and wrist detection check the owner.
        let (biometry, device_auth, apple) = prism_native::apple::security_capabilities();
        WatchServices {
            caps: Capabilities {
                car: false,
                notifications,
                location: true,
                motion: true,
                // No camera on a watch; photos come from the PhotosUI picker.
                camera: false,
                photos: true,
                haptics: true,
                share: true,
                cloud_sync: prism_native::apple::ICloudStorage::new().is_some()
                    && prism_native::apple::ICloudStorage::signed_in(),
                secure_storage: true,
                biometry,
                device_auth,
                sign_in_with_apple: apple,
                web_auth: true,
                // WKApplication.registerForRemoteNotifications (watchOS 6+).
                push: prism_native::apple::push::service(),
                ..Default::default()
            },
            ops: Vec::new(),
        }
    }
}

impl PlatformServices for WatchServices {
    fn capabilities(&self) -> Capabilities {
        self.caps
    }

    fn handle(&mut self, request: PlatformRequest) {
        match request {
            PlatformRequest::Notify(n) => {
                if self.caps.notifications {
                    unsafe { prism_notify(c(&n.to_json()).as_ptr()) }
                }
            }
            PlatformRequest::CancelNotification(id) => unsafe {
                prism_notify_cancel(c(&id).as_ptr())
            },
            PlatformRequest::RequestNotificationPermission => unsafe { prism_notify_permission() },
            PlatformRequest::RegisterPush => prism_native::apple::push::register(),
            PlatformRequest::UnregisterPush => prism_native::apple::push::unregister(),
            PlatformRequest::Device { id, call } => prism_native::apple::device_call(id, call),
            PlatformRequest::ShowAlert(a) => {
                let alert =
                    serde_json::from_str::<serde_json::Value>(&a.to_json()).unwrap_or_default();
                self.ops
                    .push(serde_json::json!({ "op": "alert", "alert": alert }));
            }
            PlatformRequest::ScheduleBackgroundTask(task) => unsafe {
                prism_watch_bg_schedule(
                    c(&task.id).as_ptr(),
                    crate::imp::background_interval(task.every).as_secs_f64(),
                )
            },
            // Widgets, Live Activities, tray and menu bar do not exist on the
            // watch (capabilities say so).
            PlatformRequest::ReloadWidgets(_)
            | PlatformRequest::StartActivity { .. }
            | PlatformRequest::UpdateActivity { .. }
            | PlatformRequest::EndActivity { .. }
            | PlatformRequest::SetTray(_)
            | PlatformRequest::RemoveTray(_)
            | PlatformRequest::SetAppMenu(_)
            | PlatformRequest::SetCar(_)
            | PlatformRequest::WatchActivityPushToStart => {}
            // Fonts are registered by the backend, which draws texts again
            // (imp.rs, `load_font`); stylesheets are a web thing.
            PlatformRequest::LoadFont(_)
            | PlatformRequest::AddStylesheet(_)
            | PlatformRequest::RemoveStylesheet(_)
            // A watch app has no title of its own to show.
            | PlatformRequest::SetTitle(_) => {}
        }
    }
}

fn cstr(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// A notification was tapped or one of its actions chosen.
#[unsafe(no_mangle)]
pub extern "C" fn prism_notification_response(
    id: *const c_char,
    action: *const c_char,
    text: *const c_char,
    link: *const c_char,
) {
    let Some(id) = cstr(id) else { return };
    let response = prism_core::NotificationResponse {
        id,
        action: cstr(action),
        text: cstr(text),
    };
    crate::imp::notification_response(response, cstr(link));
}

/// WatchKit woke the app for background task `id`.
#[unsafe(no_mangle)]
pub extern "C" fn prism_background_run(id: *const c_char) {
    if let Some(id) = cstr(id) {
        crate::imp::run_background(id);
    }
}

/// Unused on watchOS (WatchKit has no expiration callback); the bridge
/// declares it for iOS.
#[unsafe(no_mangle)]
pub extern "C" fn prism_background_expired(_id: *const c_char) {}

pub(crate) fn complete_background(id: &str) {
    // Silent pushes wait as `push:<completion>` (prism_push_received).
    if let Some(n) = id.strip_prefix("push:").and_then(|n| n.parse().ok()) {
        return prism_native::apple::push::complete(n, true);
    }
    unsafe { prism_watch_bg_complete(c(id).as_ptr()) }
}

/// The APNs device token (hex), or why registration failed.
#[unsafe(no_mangle)]
pub extern "C" fn prism_push_token(token: *const c_char, error: *const c_char) {
    use prism_native::apple::push;
    let result = match (push::cstr(token), push::cstr(error)) {
        (Some(hex), _) => Ok(push::subscription(&hex)),
        (None, e) => Err(prism_core::DeviceError::Failed(
            e.unwrap_or_else(|| "registration failed".into()),
        )),
    };
    crate::imp::push_registered(result);
}

/// A push: 0 in the foreground, 1 tapped, 2 silent (`completion`: WatchKit's
/// handler, called once the app's job finishes).
#[unsafe(no_mangle)]
pub extern "C" fn prism_push_received(
    kind: i32,
    json: *const c_char,
    action: *const c_char,
    text: *const c_char,
    completion: u64,
) {
    use prism_native::apple::push;
    let Some(json) = push::cstr(json) else { return };
    let message = push::message(kind, &json, push::cstr(action), push::cstr(text));
    crate::imp::push_received(message, completion);
}

/// No Live Activities on watchOS (the bridge declares it).
#[unsafe(no_mangle)]
pub extern "C" fn prism_activity_push_token(
    _key: *const c_char,
    _kind: *const c_char,
    _token: *const c_char,
) {
}
