//! macOS system surfaces. Notifications and widgets go through the Swift
//! bridge (they need a bundled app: `mise run macos-app`). Live Activities do
//! not exist on macOS, so they fall back to a **menu bar item** showing the
//! compact presentation (symbol + live countdown) with a menu to open the app.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSAccessibility, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::NSString;
use prism_core::surfaces::{ActivityContent, PlatformRequest, json_str};
use prism_core::{ActivityId, Capabilities, NodeKind, PlatformServices, SNode, format_remaining};

unsafe extern "C" {
    fn prism_bridge_init() -> bool;
    fn prism_widgets_write(kind: *const c_char, json: *const c_char) -> bool;
    fn prism_notify(json: *const c_char);
    fn prism_notify_cancel(id: *const c_char);
    fn prism_notify_permission();
}

fn c(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

struct StatusActivity {
    item: Retained<NSStatusItem>,
    /// Countdown end (Unix seconds) shown as the title, if any.
    countdown: Option<f64>,
    text: String,
    link: Option<String>,
}

pub struct MacServices {
    mtm: MainThreadMarker,
    target: Retained<AnyObject>,
    caps: Capabilities,
    activities: HashMap<ActivityId, StatusActivity>,
}

fn first(node: &SNode, kind: NodeKind) -> Option<&SNode> {
    if node.kind == kind {
        return Some(node);
    }
    node.children.iter().find_map(|c| first(c, kind))
}

impl MacServices {
    pub fn new(mtm: MainThreadMarker, target: Retained<AnyObject>) -> Self {
        let bundled = unsafe { prism_bridge_init() };
        let has_group = objc2_foundation::NSBundle::mainBundle()
            .objectForInfoDictionaryKey(&NSString::from_str("PrismAppGroup"))
            .is_some();
        MacServices {
            mtm,
            target,
            caps: Capabilities {
                widgets: bundled && has_group,
                // Rendered as a menu bar item.
                live_activities: true,
                dynamic_island: false,
                notifications: bundled,
            },
            activities: HashMap::new(),
        }
    }

    /// Link of the activity whose menu item was chosen.
    pub fn link_for(&self, tag: isize) -> Option<String> {
        self.activities.get(&ActivityId(tag as u64))?.link.clone()
    }

    /// Titles of the menu bar items (dev automation).
    pub fn status_titles(&self) -> Vec<String> {
        self.activities
            .values()
            .filter_map(|a| a.item.button(self.mtm))
            .map(|b| b.title().to_string())
            .collect()
    }

    pub fn tick(&self) {
        for a in self.activities.values() {
            if let (Some(end), Some(button)) = (a.countdown, a.item.button(self.mtm)) {
                button.setTitle(&NSString::from_str(&format!(" {}", format_remaining(end))));
            }
        }
    }

    fn show(&mut self, id: ActivityId, content: &ActivityContent) {
        let mtm = self.mtm;
        let entry = self.activities.entry(id).or_insert_with(|| StatusActivity {
            item: NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength),
            countdown: None,
            text: String::new(),
            link: None,
        });
        let compact = [
            &content.compact_leading,
            &content.compact_trailing,
            &content.minimal,
        ];
        let symbol = compact
            .iter()
            .filter_map(|n| n.as_ref())
            .find_map(|n| first(n, NodeKind::Image))
            .map(|n| n.icon.clone());
        entry.countdown = compact
            .iter()
            .filter_map(|n| n.as_ref())
            .find_map(|n| first(n, NodeKind::Timer))
            .and_then(|n| n.value);
        entry.text = content
            .compact_trailing
            .as_ref()
            .and_then(|n| first(n, NodeKind::Text))
            .map_or(content.summary.clone(), |n| n.text.clone());
        entry.link = content.link.clone();

        if let Some(button) = entry.item.button(mtm) {
            let image = symbol.and_then(|s| {
                NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(&s),
                    Some(&NSString::from_str(&content.summary)),
                )
            });
            button.setImage(image.as_deref());
            let title = match entry.countdown {
                Some(end) => format!(" {}", format_remaining(end)),
                None => format!(" {}", entry.text),
            };
            button.setTitle(&NSString::from_str(&title));
            button.setAccessibilityLabel(Some(&NSString::from_str(&content.summary)));
        }

        let menu = NSMenu::new(mtm);
        let summary = NSMenuItem::new(mtm);
        summary.setTitle(&NSString::from_str(&content.summary));
        summary.setEnabled(false);
        menu.addItem(&summary);
        if content.link.is_some() {
            let open = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str("Open"),
                    Some(sel!(onStatusItem:)),
                    &NSString::new(),
                )
            };
            open.setTag(id.0 as isize);
            unsafe { open.setTarget(Some(&self.target)) };
            menu.addItem(&open);
        }
        entry.item.setMenu(Some(&menu));
    }
}

impl PlatformServices for MacServices {
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
                    unsafe { prism_widgets_write(c(&w.kind).as_ptr(), c(&w.to_json()).as_ptr()) };
                }
            }
            PlatformRequest::StartActivity { id, content, .. }
            | PlatformRequest::UpdateActivity { id, content } => self.show(id, &content),
            PlatformRequest::EndActivity { id, .. } => {
                if let Some(a) = self.activities.remove(&id) {
                    NSStatusBar::systemStatusBar().removeStatusItem(&a.item);
                }
            }
            PlatformRequest::Notify(n) => {
                if !self.caps.notifications {
                    eprintln!(
                        "prism-macos: notification {:?} skipped: notifications need a bundled app (mise run macos-app)",
                        n.title
                    );
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
            PlatformRequest::CancelNotification(id) => {
                if self.caps.notifications {
                    unsafe { prism_notify_cancel(c(&id).as_ptr()) }
                }
            }
            PlatformRequest::RequestNotificationPermission => {
                if self.caps.notifications {
                    unsafe { prism_notify_permission() }
                }
            }
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
