//! iOS system surfaces: widgets (WidgetKit), Live Activities (lock screen +
//! Dynamic Island via ActivityKit) and notifications, through the Swift bridge
//! in `platform/apple/Bridge`.

use std::ffi::{CStr, CString, c_char};
use std::time::UNIX_EPOCH;

use prism_core::surfaces::{ActivityContent, PlatformRequest, json_str};
use prism_core::{Capabilities, PlatformServices};

unsafe extern "C" {
    fn prism_bridge_init() -> bool;
    fn prism_widgets_write(kind: *const c_char, json: *const c_char) -> bool;
    fn prism_activity_supported() -> bool;
    fn prism_activity_start(
        key: *const c_char,
        kind: *const c_char,
        json: *const c_char,
        stale: f64,
    ) -> bool;
    fn prism_activity_update(key: *const c_char, json: *const c_char, stale: f64);
    fn prism_activity_end(key: *const c_char, json: *const c_char, immediately: bool);
    fn prism_notify(json: *const c_char);
    fn prism_notify_cancel(id: *const c_char);
    fn prism_notify_permission();
}

fn c(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

fn stale(content: &ActivityContent) -> f64 {
    content
        .stale_after
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0., |d| d.as_secs_f64())
}

pub struct IosServices {
    caps: Capabilities,
}

impl IosServices {
    pub fn new(dynamic_island: bool) -> Self {
        let notifications = unsafe { prism_bridge_init() };
        let live = unsafe { prism_activity_supported() };
        let info = |key: &str| {
            objc2_foundation::NSBundle::mainBundle()
                .objectForInfoDictionaryKey(&objc2_foundation::NSString::from_str(key))
                .is_some()
        };
        IosServices {
            caps: Capabilities {
                widgets: info("PrismAppGroup"),
                live_activities: live,
                dynamic_island: live && dynamic_island,
                notifications,
            },
        }
    }
}

impl PlatformServices for IosServices {
    fn capabilities(&self) -> Capabilities {
        self.caps
    }

    fn handle(&mut self, request: PlatformRequest) {
        match request {
            PlatformRequest::ReloadWidgets(snaps) => {
                if !self.caps.widgets {
                    return;
                }
                for w in snaps {
                    let ok = unsafe {
                        prism_widgets_write(c(&w.kind).as_ptr(), c(&w.to_json()).as_ptr())
                    };
                    if !ok {
                        eprintln!(
                            "prism-ios: could not write widget {:?} (App Group missing?)",
                            w.kind
                        );
                    }
                }
            }
            PlatformRequest::StartActivity { id, kind, content } => {
                let ok = unsafe {
                    prism_activity_start(
                        c(&id.0.to_string()).as_ptr(),
                        c(&kind).as_ptr(),
                        c(&content.to_json()).as_ptr(),
                        stale(&content),
                    )
                };
                if !ok {
                    eprintln!("prism-ios: Live Activity {kind:?} not started");
                }
            }
            PlatformRequest::UpdateActivity { id, content } => unsafe {
                prism_activity_update(
                    c(&id.0.to_string()).as_ptr(),
                    c(&content.to_json()).as_ptr(),
                    stale(&content),
                )
            },
            PlatformRequest::EndActivity {
                id,
                content,
                immediately,
            } => {
                let json = content.map(|c_| c(&c_.to_json()));
                unsafe {
                    prism_activity_end(
                        c(&id.0.to_string()).as_ptr(),
                        json.as_ref().map_or(std::ptr::null(), |j| j.as_ptr()),
                        immediately,
                    )
                }
            }
            PlatformRequest::Notify(n) => {
                if !self.caps.notifications {
                    return;
                }
                let opt = |s: &Option<String>| s.as_deref().map_or("null".into(), json_str);
                let json = format!(
                    r#"{{"id":{},"title":{},"subtitle":{},"body":{},"link":{},"delay":{},"sound":{},"badge":{},"thread":{}}}"#,
                    json_str(&n.id),
                    json_str(&n.title),
                    opt(&n.subtitle),
                    json_str(&n.body),
                    opt(&n.link),
                    n.delay.map_or(0., |d| d.as_secs_f64()),
                    n.sound,
                    n.badge.map_or("null".into(), |b| b.to_string()),
                    opt(&n.thread),
                );
                unsafe { prism_notify(c(&json).as_ptr()) }
            }
            PlatformRequest::CancelNotification(id) => unsafe {
                prism_notify_cancel(c(&id).as_ptr())
            },
            PlatformRequest::RequestNotificationPermission => unsafe { prism_notify_permission() },
        }
    }
}

/// Called by the Swift bridge when a notification is tapped.
#[unsafe(no_mangle)]
pub extern "C" fn prism_open_link(path: *const c_char) {
    if path.is_null() {
        return;
    }
    let path = unsafe { CStr::from_ptr(path) }
        .to_string_lossy()
        .into_owned();
    crate::imp::open_link(&path);
}
