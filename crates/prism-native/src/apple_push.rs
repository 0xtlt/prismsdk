//! APNs through the Swift bridge (`prism-apple-build/apple/Bridge/PrismPush.swift`),
//! shared by the iOS, macOS and watchOS backends.

use std::ffi::{CStr, CString, c_char, c_void};

use prism_core::{PushEvent, PushMessage, PushService, PushSubscription};

unsafe extern "C" {
    fn prism_push_register();
    fn prism_push_unregister();
    fn prism_push_complete(id: u64, new_data: bool);
    fn prism_push_payload(info: *const c_void) -> *mut c_char;
    fn prism_push_environment() -> *mut c_char;
    fn prism_activity_watch_start_tokens();
    fn free(p: *mut c_void);
}

fn take(p: *mut c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { free(p.cast()) };
    Some(s)
}

/// `PrismApsEnvironment` from Info.plist (`prism` writes it with the
/// aps-environment entitlement): push is configured for this build.
pub fn environment() -> Option<String> {
    take(unsafe { prism_push_environment() })
}

/// Push capability: APNs when the build declares an aps-environment.
pub fn service() -> Option<PushService> {
    environment().map(|_| PushService::Apns)
}

pub fn register() {
    unsafe { prism_push_register() }
}

pub fn unregister() {
    unsafe { prism_push_unregister() }
}

pub fn watch_activity_start_tokens() {
    unsafe { prism_activity_watch_start_tokens() }
}

/// Tells watchOS a silent push was handled (no-op elsewhere).
pub fn complete(id: u64, new_data: bool) {
    unsafe { prism_push_complete(id, new_data) }
}

/// A device token (hex) as the subscription for the app's server: the topic
/// is the bundle id; development builds and simulators use the sandbox.
pub fn subscription(hex: &str) -> PushSubscription {
    let mut sub = PushSubscription::new(PushService::Apns, hex);
    sub.topic = objc2_foundation::NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|b| b.to_string());
    sub.sandbox = environment().is_none_or(|e| e != "production") || super::is_simulator();
    sub
}

/// The payload JSON of a userInfo dictionary (`NSDictionary *`).
///
/// # Safety
/// `info` must be a valid `NSDictionary`.
pub unsafe fn payload(info: *const c_void) -> String {
    take(unsafe { prism_push_payload(info) }).unwrap_or_else(|| "{}".into())
}

/// The message for a payload from the bridge (`kind`: 0 foreground, 1
/// tapped, 2 silent).
pub fn message(kind: i32, json: &str, action: Option<String>, text: Option<String>) -> PushMessage {
    let event = match kind {
        1 => PushEvent::Opened,
        2 => PushEvent::Background,
        _ => PushEvent::Received,
    };
    let mut m = PushMessage::from_payload(event, json)
        .map(|(m, _)| m)
        .unwrap_or_else(|| PushMessage::new(event));
    m.event = event;
    m.service = Some(PushService::Apns);
    m.action = action;
    m.text = text;
    m
}

/// C strings from the bridge (null or NUL-terminated, valid for the call:
/// only the `prism_push_*` callbacks use it).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn cstr(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

pub fn c(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}
