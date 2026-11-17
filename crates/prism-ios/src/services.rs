//! iOS system surfaces: widgets (WidgetKit), Live Activities (lock screen +
//! Dynamic Island via ActivityKit) and notifications, through the Swift bridge
//! in `prism-apple-build` (`apple/Bridge`).

use std::ffi::{CStr, CString, c_char};
use std::time::UNIX_EPOCH;

use prism_core::surfaces::{ActivityContent, PlatformRequest};
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
        push: bool,
    ) -> bool;
    fn prism_activity_update(key: *const c_char, json: *const c_char, stale: f64);
    fn prism_activity_end(key: *const c_char, json: *const c_char, immediately: bool);
    /// The car's template stack (JSON, prism-core `car`).
    fn prism_car_set(json: *const c_char);
    fn prism_notify(json: *const c_char);
    fn prism_notify_cancel(id: *const c_char);
    fn prism_notify_permission();
    fn prism_set_badge(count: i32);
    fn prism_bg_register();
    fn prism_bg_schedule(
        id: *const c_char,
        seconds: f64,
        processing: bool,
        network: bool,
        power: bool,
    ) -> bool;
    fn prism_bg_complete(id: *const c_char, success: bool);
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
    controller: objc2::rc::Retained<objc2_ui_kit::UIViewController>,
}

impl IosServices {
    pub fn new(
        dynamic_island: bool,
        controller: objc2::rc::Retained<objc2_ui_kit::UIViewController>,
    ) -> Self {
        let notifications = unsafe { prism_bridge_init() };
        let live = unsafe { prism_activity_supported() };
        let info = |key: &str| {
            objc2_foundation::NSBundle::mainBundle()
                .objectForInfoDictionaryKey(&objc2_foundation::NSString::from_str(key))
                .is_some()
        };
        // tvOS has no widgets, sensors, pickers, haptics or share sheet;
        // visionOS no haptics or camera for apps (UIKit headers).
        let tv = cfg!(target_os = "tvos");
        let vision = cfg!(target_os = "visionos");
        // Keychain, LocalAuthentication (none on tvOS), Sign in with Apple.
        let (biometry, device_auth, apple) = prism_native::apple::security_capabilities();
        IosServices {
            caps: Capabilities {
                // The bundle declares a CarPlay scene (Prism.toml / register_car).
                car: info("PrismCarPlay"),
                widgets: !tv && info("PrismAppGroup"),
                live_activities: live,
                dynamic_island: live && dynamic_island,
                notifications,
                tray: false,
                app_menu: false,
                location: true,
                motion: !tv,
                // Simulators have no camera.
                camera: !tv && !vision && !cfg!(target_abi = "sim"),
                photos: !tv,
                haptics: !tv && !vision,
                share: !tv,
                clipboard: !tv,
                cloud_sync: prism_native::apple::ICloudStorage::new().is_some()
                    && prism_native::apple::ICloudStorage::signed_in(),
                cloud_backup: false,
                secure_storage: true,
                biometry,
                device_auth,
                sign_in_with_apple: apple,
                // ASWebAuthenticationSession (tvOS 16+ included).
                web_auth: true,
                // The build declares aps-environment (`[push]` in Prism.toml).
                push: prism_native::apple::push::service(),
                // WKWebView (#62); tvOS has no WebKit.
                web_view: cfg!(not(target_os = "tvos")),
                // UIDropInteraction (#111); none on tvOS.
                // ASAuthorizationPlatformPublicKeyCredentialProvider (#94).
                passkeys: prism_native::apple::passkeys_available(),
                drop_files: cfg!(not(target_os = "tvos")),
            },
            controller,
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
                        content.push_updates,
                    )
                };
                if !ok {
                    eprintln!("prism-ios: Live Activity {kind:?} not started");
                    // Bundled without the widget extension that draws them.
                    let declared = objc2_foundation::NSBundle::mainBundle()
                        .objectForInfoDictionaryKey(&objc2_foundation::NSString::from_str(
                            "NSSupportsLiveActivities",
                        ))
                        .is_some();
                    if !declared && cfg!(target_os = "ios") {
                        eprintln!(
                            "prism-ios: the app was bundled without Live Activities: call cx.enable_live_activities() at startup"
                        );
                    }
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
                unsafe { prism_notify(c(&n.to_json()).as_ptr()) }
            }
            PlatformRequest::CancelNotification(id) => unsafe {
                prism_notify_cancel(c(&id).as_ptr())
            },
            PlatformRequest::RequestNotificationPermission => unsafe { prism_notify_permission() },
            // The app icon's badge (#239).
            PlatformRequest::SetBadge(n) => unsafe {
                prism_set_badge(n.map_or(-1, |n| n.min(i32::MAX as u32) as i32))
            },
            PlatformRequest::RegisterPush => prism_native::apple::push::register(),
            PlatformRequest::UnregisterPush => prism_native::apple::push::unregister(),
            PlatformRequest::WatchActivityPushToStart => {
                prism_native::apple::push::watch_activity_start_tokens()
            }
            PlatformRequest::ShowAlert(spec) => {
                let mtm = objc2::MainThreadMarker::new().expect("main thread");
                crate::menus::show_alert(mtm, &self.controller, &spec, crate::imp::dispatch)
            }
            PlatformRequest::Device { id, call } => prism_native::apple::device_call(id, call),
            PlatformRequest::ScheduleBackgroundTask(task) => {
                let every = crate::imp::background_interval(task.every).as_secs_f64();
                let (processing, network, power) = match task.kind {
                    prism_core::BackgroundKind::Refresh => (false, false, false),
                    prism_core::BackgroundKind::Processing {
                        requires_network,
                        requires_power,
                    } => (true, requires_network, requires_power),
                };
                let id = full_task_id(&task.id);
                let ok = unsafe {
                    prism_bg_schedule(c(&id).as_ptr(), every, processing, network, power)
                };
                if !ok {
                    eprintln!(
                        "prism-ios: background task {:?} not scheduled (declared in Info.plist? simulators cannot run them)",
                        task.id
                    );
                }
            }
            // No menu bar or tray on iOS (capabilities say so).
            PlatformRequest::SetTray(_)
            | PlatformRequest::RemoveTray(_)
            | PlatformRequest::SetAppMenu(_)
            | PlatformRequest::SetMenuBar(_) => {}
            PlatformRequest::SetCar(json) => unsafe { prism_car_set(c(&json).as_ptr()) },
            // Fonts are registered by the backend, which draws texts again
            // (imp.rs, `load_font`); stylesheets are a web thing.
            PlatformRequest::LoadFont(_)
            | PlatformRequest::AddStylesheet(_)
            | PlatformRequest::RemoveStylesheet(_) => {}
            // The scene's title: its name in the iPad's multitasking switcher.
            PlatformRequest::SetTitle(title) => crate::imp::set_scene_title(title.as_deref()),
            // The system closes apps on phones and tablets: nothing to ask.
            PlatformRequest::SetUnsavedChanges(_) => {}
        }
    }
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

/// Called by the Swift bridge for videos (#207): its size is known (lay
/// out again) or it played to the end.
#[unsafe(no_mangle)]
pub extern "C" fn prism_video_event(id: u64, kind: i32) {
    dispatch2::DispatchQueue::main().exec_async(move || {
        match prism_native::apple::video::event(id, kind) {
            Some(event) => crate::imp::dispatch(event),
            None => crate::imp::relayout_all(),
        }
    });
}

/// Called by the Swift bridge for drop targets (#111): accepted files left
/// or came over a node, or were dropped (their copies). On the next turn.
#[cfg(not(target_os = "tvos"))]
#[unsafe(no_mangle)]
pub extern "C" fn prism_drop_event(node: u64, kind: i32, files: *const c_char) {
    use prism_native::apple::drops::DropReport;
    match unsafe { prism_native::apple::drops::report(node, kind, files) } {
        Some(DropReport::Files(id, files)) => dispatch2::DispatchQueue::main()
            .exec_async(move || crate::imp::dispatch(prism_core::UiEvent::DropFiles(id, files))),
        Some(DropReport::Hover(id, on)) => {
            crate::imp::set_interaction(id, prism_core::Interaction::DropHover, on)
        }
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

/// The APNs device token (hex), from the app delegate (imp.rs), or why
/// registration failed. Also exported for the bridge.
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

/// A push in the foreground (0) or tapped (1), from the notification
/// delegate (PrismBridge.swift). Silent pushes (2) come from the app
/// delegate (imp.rs).
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
    let message = push::message(kind, &json, push::cstr(action), push::cstr(text));
    crate::imp::push_received(message);
}

/// A Live Activity's push token (`key` null: the push-to-start token).
#[unsafe(no_mangle)]
pub extern "C" fn prism_activity_push_token(
    key: *const c_char,
    kind: *const c_char,
    token: *const c_char,
) {
    use prism_native::apple::push;
    let Some(token) = push::cstr(token) else {
        return;
    };
    let token = prism_core::ActivityPushToken {
        activity: push::cstr(key)
            .and_then(|k| k.parse().ok())
            .map(prism_core::ActivityId),
        kind: push::cstr(kind).unwrap_or_default(),
        token,
    };
    crate::imp::with_runtime(|rt| rt.activity_push_token(token));
}

/// A Live Activity the app did not start in this run (a push started it, or
/// an earlier run did) joins the app's activities: the id to use for it,
/// `requested` (its attributes' id, 0: none) when the bridge has no live
/// activity under it, or a new one.
#[unsafe(no_mangle)]
pub extern "C" fn prism_activity_adopt(requested: u64) -> u64 {
    let mut id = 0;
    crate::imp::with_runtime(|rt| {
        id = rt
            .app
            .adopt_activity(Some(prism_core::ActivityId(requested)))
            .0;
        Vec::new()
    });
    id
}

/// CarPlay (PrismCarPlay.swift): connected or not, a selection, Back.
#[unsafe(no_mangle)]
pub extern "C" fn prism_car_connected(connected: bool) {
    crate::imp::with_runtime(|rt| rt.car_connected(connected));
}

#[unsafe(no_mangle)]
pub extern "C" fn prism_car_select(id: u64) {
    crate::imp::with_runtime(|rt| rt.car_select(id));
}

#[unsafe(no_mangle)]
pub extern "C" fn prism_car_back(depth: u64) {
    crate::imp::with_runtime(|rt| rt.car_back(depth as usize));
}

/// Installs the notification delegate (idempotent).
pub(crate) fn init_bridge() -> bool {
    unsafe { prism_bridge_init() }
}

/// Registers BGTaskScheduler handlers (must run before launch completes).
pub(crate) fn register_background_tasks() {
    unsafe { prism_bg_register() }
}

/// `sync` → `<bundle id>.sync`, as declared in Info.plist.
pub(crate) fn full_task_id(id: &str) -> String {
    let bundle = objc2_foundation::NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|b| b.to_string())
        .unwrap_or_default();
    format!("{bundle}.{id}")
}

pub(crate) fn short_task_id(full: &str) -> String {
    let bundle = objc2_foundation::NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|b| format!("{b}."))
        .unwrap_or_default();
    full.strip_prefix(&bundle).unwrap_or(full).to_string()
}

/// Tells iOS a background task finished.
pub(crate) fn complete_background_task(full_id: &str, success: bool) {
    unsafe { prism_bg_complete(c(full_id).as_ptr(), success) }
}

fn cstr(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// Called by iOS (through the Swift bridge) to run a background task.
#[unsafe(no_mangle)]
pub extern "C" fn prism_background_run(id: *const c_char) {
    if let Some(full) = cstr(id) {
        crate::imp::run_background(&short_task_id(&full), crate::imp::Completion::System(full));
    }
}

/// The system's time for a background task ran out.
#[unsafe(no_mangle)]
pub extern "C" fn prism_background_expired(id: *const c_char) {
    if let Some(full) = cstr(id) {
        crate::imp::expire_background(&full);
    }
}

// --- Widget actions (Button(intent:) in widgets, #47) ---

#[cfg(not(target_os = "tvos"))]
unsafe extern "C" {
    fn prism_widget_actions_install();
    fn prism_widget_actions_take() -> *mut c_char;
    fn prism_widget_action_done(token: u64);
    fn free(p: *mut std::ffi::c_void);
}

/// From launch: widget button intents iOS runs in the app's process reach
/// [`prism_widget_action`]. tvOS has no widgets.
pub(crate) fn install_widget_actions() {
    #[cfg(not(target_os = "tvos"))]
    unsafe {
        prism_widget_actions_install()
    }
}

/// Actions the widget extension queued because it ran an intent itself
/// (the app runs them when it comes to the foreground).
pub(crate) fn take_queued_widget_actions() -> Vec<prism_core::WidgetAction> {
    #[cfg(not(target_os = "tvos"))]
    {
        let p = unsafe { prism_widget_actions_take() };
        if p.is_null() {
            return Vec::new();
        }
        let json = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        unsafe { free(p.cast()) };
        prism_core::WidgetAction::list_from_json(&json)
    }
    #[cfg(target_os = "tvos")]
    Vec::new()
}

/// Tells the intent that ran widget action `token` that its job finished.
pub(crate) fn widget_action_done(token: u64) {
    #[cfg(not(target_os = "tvos"))]
    unsafe {
        prism_widget_action_done(token)
    }
    #[cfg(target_os = "tvos")]
    let _ = token;
}

/// A widget button's intent runs in the app (iOS launched it in the
/// background if needed): the app's `on_widget_action` job, then
/// `prism_widget_action_done(token)`.
#[unsafe(no_mangle)]
pub extern "C" fn prism_widget_action(
    kind: *const c_char,
    action: *const c_char,
    payload: *const c_char,
    token: u64,
) {
    let action = prism_core::WidgetAction {
        kind: cstr(kind).unwrap_or_default(),
        action: cstr(action).unwrap_or_default(),
        payload: cstr(payload).unwrap_or_default(),
    };
    crate::imp::run_widget_action(action, crate::imp::Completion::WidgetAction(token));
}
