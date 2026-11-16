//! macOS system surfaces. Notifications and widgets go through the Swift
//! bridge (they need a bundled app: `mise run macos-app`). Live Activities do
//! not exist on macOS, so they fall back to a **menu bar item** showing the
//! compact presentation (symbol + live countdown) with a menu to open the app.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSAccessibility, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::NSString;
use prism_core::surfaces::{ActivityContent, PlatformRequest};
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
    window: Retained<objc2_app_kit::NSWindow>,
    title: String,
    /// Tray items by id.
    trays: HashMap<String, Retained<NSStatusItem>>,
    caps: Capabilities,
    activities: HashMap<ActivityId, StatusActivity>,
    /// Periodic background tasks by id (they repeat by themselves).
    schedulers: HashMap<String, Retained<objc2_foundation::NSBackgroundActivityScheduler>>,
}

fn first(node: &SNode, kind: NodeKind) -> Option<&SNode> {
    if node.kind == kind {
        return Some(node);
    }
    node.children.iter().find_map(|c| first(c, kind))
}

impl MacServices {
    pub fn new(
        mtm: MainThreadMarker,
        target: Retained<AnyObject>,
        window: Retained<objc2_app_kit::NSWindow>,
        title: String,
    ) -> Self {
        let bundled = unsafe { prism_bridge_init() };
        let has_group = objc2_foundation::NSBundle::mainBundle()
            .objectForInfoDictionaryKey(&NSString::from_str("PrismAppGroup"))
            .is_some();
        let (biometry, device_auth, apple) = prism_native::apple::security_capabilities();
        MacServices {
            mtm,
            target,
            window,
            title,
            trays: HashMap::new(),
            schedulers: HashMap::new(),
            caps: Capabilities {
                car: false,
                widgets: bundled && has_group,
                // Rendered as a menu bar item.
                live_activities: true,
                dynamic_island: false,
                notifications: bundled,
                tray: true,
                app_menu: true,
                location: true,
                motion: false,
                camera: false,
                photos: true,
                haptics: true,
                share: true,
                clipboard: true,
                cloud_sync: prism_native::apple::ICloudStorage::new().is_some()
                    && prism_native::apple::ICloudStorage::signed_in(),
                cloud_backup: false,
                // The login keychain, or the data protection keychain when
                // the app is signed with a profile.
                secure_storage: true,
                biometry,
                device_auth,
                sign_in_with_apple: apple,
                web_auth: true,
                // A bundled app declaring com.apple.developer.aps-environment.
                push: if bundled {
                    prism_native::apple::push::service()
                } else {
                    None
                },
                // WKWebView (#62).
                web_view: true,
                // NSDraggingDestination (#111).
                drop_files: true,
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
            .map(|a| &a.item)
            .chain(self.trays.values())
            .filter_map(|i| i.button(self.mtm))
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
            PlatformRequest::RegisterPush => {
                if self.caps.push.is_some() {
                    prism_native::apple::push::register()
                } else {
                    crate::imp::push_registered(Err(prism_core::DeviceError::Unavailable))
                }
            }
            PlatformRequest::UnregisterPush => prism_native::apple::push::unregister(),
            // No Live Activities on macOS (a menu bar item stands in).
            PlatformRequest::WatchActivityPushToStart => {}
            PlatformRequest::Notify(n) => {
                if !self.caps.notifications {
                    eprintln!(
                        "prism-macos: notification {:?} skipped: notifications need a bundled app (mise run macos-app)",
                        n.title
                    );
                    return;
                }
                unsafe { prism_notify(c(&n.to_json()).as_ptr()) }
            }
            PlatformRequest::CancelNotification(id) => {
                if self.caps.notifications {
                    unsafe { prism_notify_cancel(c(&id).as_ptr()) }
                }
            }
            PlatformRequest::ShowAlert(spec) => {
                crate::menus::show_alert(self.mtm, &self.window, &spec, crate::imp::dispatch)
            }
            PlatformRequest::SetAppMenu(menus) => {
                crate::menus::install_main_menu(self.mtm, &self.title, &self.target, &menus)
            }
            PlatformRequest::ScheduleBackgroundTask(task) => self.schedule(task),
            PlatformRequest::Device { id, call } => prism_native::apple::device_call(id, call),
            PlatformRequest::SetTray(tray) => {
                let mtm = self.mtm;
                let item = self.trays.entry(tray.id.clone()).or_insert_with(|| {
                    NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength)
                });
                if let Some(button) = item.button(mtm) {
                    let image = tray.icon.as_deref().and_then(|s| {
                        NSImage::imageWithSystemSymbolName_accessibilityDescription(
                            &NSString::from_str(s),
                            tray.tooltip.as_deref().map(NSString::from_str).as_deref(),
                        )
                    });
                    button.setImage(image.as_deref());
                    let title = tray
                        .title
                        .as_deref()
                        .map(|t| format!(" {t}"))
                        .unwrap_or_default();
                    button.setTitle(&NSString::from_str(&title));
                    button.setToolTip(tray.tooltip.as_deref().map(NSString::from_str).as_deref());
                    button.setAccessibilityLabel(
                        tray.tooltip.as_deref().map(NSString::from_str).as_deref(),
                    );
                }
                item.setMenu(Some(&crate::menus::build_menu(
                    mtm,
                    &tray.menu,
                    &self.target,
                )));
            }
            // No car screens on the Mac.
            PlatformRequest::SetCar(_) => {}
            // Fonts are registered by the backend, which draws texts again
            // (imp.rs, `load_font`); stylesheets are a web thing.
            PlatformRequest::LoadFont(_)
            | PlatformRequest::AddStylesheet(_)
            | PlatformRequest::RemoveStylesheet(_) => {}
            PlatformRequest::RemoveTray(id) => {
                if let Some(item) = self.trays.remove(&id) {
                    NSStatusBar::systemStatusBar().removeStatusItem(&item);
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

/// The APNs device token (hex) or a registration error (PrismPush.swift's
/// app delegate).
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

/// A push: 0 in the foreground, 1 tapped, 2 silent (macOS has no
/// completion handler: the app keeps running anyway).
#[unsafe(no_mangle)]
pub extern "C" fn prism_push_received(
    kind: i32,
    json: *const c_char,
    action: *const c_char,
    text: *const c_char,
    _completion: u64,
) {
    use prism_native::apple::push;
    let Some(json) = push::cstr(json) else { return };
    crate::imp::push_received(push::message(
        kind,
        &json,
        push::cstr(action),
        push::cstr(text),
    ));
}

/// Live Activity tokens do not exist on macOS (the bridge declares it).
#[unsafe(no_mangle)]
pub extern "C" fn prism_activity_push_token(
    _key: *const c_char,
    _kind: *const c_char,
    _token: *const c_char,
) {
}

/// Called by the Swift bridge when a web view's page posts a message or
/// finishes loading (#62). Dispatched on the next turn: WebKit may call
/// while Prism updates its views.
#[unsafe(no_mangle)]
pub extern "C" fn prism_web_view_event(id: u64, kind: i32, a: *const c_char, b: *const c_char) {
    if let Some(event) = unsafe { prism_native::apple::web::event(id, kind, a, b) } {
        dispatch2::DispatchQueue::main().exec_async(move || crate::imp::dispatch(event));
    }
}

/// Called by the Swift bridge for drop targets (#111): accepted files left
/// or came over a node, or were dropped (their copies). On the next turn:
/// AppKit calls during its drag session.
#[unsafe(no_mangle)]
pub extern "C" fn prism_drop_event(node: u64, kind: i32, files: *const c_char) {
    use prism_native::apple::drops::DropReport;
    match unsafe { prism_native::apple::drops::report(node, kind, files) } {
        Some(DropReport::Files(id, files)) => dispatch2::DispatchQueue::main()
            .exec_async(move || crate::imp::dispatch(prism_core::UiEvent::DropFiles(id, files))),
        Some(DropReport::Hover(id, on)) => dispatch2::DispatchQueue::main()
            .exec_async(move || crate::imp::report_drop_hover(id, on)),
        None => {}
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

/// Called by the Swift bridge when a notification is tapped or one of its
/// actions chosen (`action`, `text`, `link` may be null).
#[unsafe(no_mangle)]
pub extern "C" fn prism_notification_response(
    id: *const c_char,
    action: *const c_char,
    text: *const c_char,
    link: *const c_char,
) {
    let s = |p: *const c_char| {
        (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    };
    let Some(id) = s(id) else { return };
    let response = prism_core::NotificationResponse {
        id,
        action: s(action),
        text: s(text),
    };
    crate::imp::notification_response(response, s(link));
}

impl MacServices {
    /// `NSBackgroundActivityScheduler`: macOS runs the task about every
    /// `every` while the app is open, when the machine is not busy.
    fn schedule(&mut self, task: prism_core::BackgroundTask) {
        use objc2_foundation::{NSBackgroundActivityResult, NSBackgroundActivityScheduler};
        if self.schedulers.contains_key(&task.id) {
            return; // repeats by itself
        }
        let bundle = objc2_foundation::NSBundle::mainBundle()
            .bundleIdentifier()
            .map_or_else(
                || format!("dev.prism.{}", self.title.to_lowercase()),
                |b| b.to_string(),
            );
        let every = crate::imp::background_interval(task.every).as_secs_f64();
        let scheduler = NSBackgroundActivityScheduler::initWithIdentifier(
            NSBackgroundActivityScheduler::alloc(),
            &NSString::from_str(&format!("{bundle}.{}", task.id)),
        );
        scheduler.setRepeats(true);
        scheduler.setInterval(every);
        scheduler.setTolerance((every / 10.).max(1.));
        let id = task.id.clone();
        // Runs on a background queue: hand the task to the main thread and
        // report completion once the app's task is done.
        let block = block2::RcBlock::new(
            move |completion: objc2_foundation::NSBackgroundActivityCompletionHandler| {
                let (done, finished) = std::sync::mpsc::channel::<()>();
                let id = id.clone();
                dispatch2::DispatchQueue::main()
                    .exec_async(move || crate::imp::run_background(&id, done));
                let _ = finished.recv_timeout(std::time::Duration::from_secs(600));
                if let Some(completion) = unsafe { completion.as_ref() } {
                    completion.call((NSBackgroundActivityResult::Finished,));
                }
            },
        );
        unsafe { scheduler.scheduleWithBlock(&block) };
        self.schedulers.insert(task.id, scheduler);
    }
}
