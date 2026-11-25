//! Shared by the iOS and macOS backends: Foundation-based formatting and the
//! system environment (locale, time zone).

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{
    NSDate, NSDateFormatter, NSDateFormatterStyle, NSLocale, NSNumber, NSNumberFormatter,
    NSNumberFormatterStyle, NSString, NSTimeZone,
};
use prism_core::{DateStyle, Formatter, Locale};

/// Numbers and dates formatted by Foundation (calendar, digits and time
/// zone of the user).
#[derive(Default)]
pub struct FoundationFormatter;

fn ns_locale(locale: &Locale) -> Retained<NSLocale> {
    NSLocale::initWithLocaleIdentifier(NSLocale::alloc(), &NSString::from_str(locale.tag()))
}

thread_local! {
    /// Number formatters by locale and decimals: making one costs far more
    /// than formatting with it (a list of numbers being scrolled).
    static NUMBER_FORMATTERS: std::cell::RefCell<
        std::collections::HashMap<(String, usize), Retained<NSNumberFormatter>>,
    > = Default::default();
}

impl Formatter for FoundationFormatter {
    fn number(&self, locale: &Locale, value: f64, decimals: usize) -> String {
        NUMBER_FORMATTERS.with(|cache| {
            let mut cache = cache.borrow_mut();
            let f = cache
                .entry((locale.tag().to_string(), decimals))
                .or_insert_with(|| {
                    let f = NSNumberFormatter::new();
                    f.setLocale(Some(&ns_locale(locale)));
                    f.setNumberStyle(NSNumberFormatterStyle::DecimalStyle);
                    f.setMinimumFractionDigits(decimals);
                    f.setMaximumFractionDigits(decimals);
                    f
                });
            f.stringFromNumber(&NSNumber::new_f64(value))
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("{value:.decimals$}"))
        })
    }

    fn date(&self, locale: &Locale, unix: f64, style: DateStyle) -> String {
        let f = NSDateFormatter::new();
        let style = match style {
            DateStyle::Short => NSDateFormatterStyle::ShortStyle,
            DateStyle::Medium => NSDateFormatterStyle::MediumStyle,
            DateStyle::Long => NSDateFormatterStyle::LongStyle,
        };
        f.setLocale(Some(&ns_locale(locale)));
        f.setDateStyle(style);
        f.setTimeStyle(NSDateFormatterStyle::NoStyle);
        f.stringFromDate(&NSDate::dateWithTimeIntervalSince1970(unix))
            .to_string()
    }

    fn time(&self, locale: &Locale, unix: f64) -> String {
        let f = NSDateFormatter::new();
        f.setLocale(Some(&ns_locale(locale)));
        f.setDateStyle(NSDateFormatterStyle::NoStyle);
        f.setTimeStyle(NSDateFormatterStyle::ShortStyle);
        f.stringFromDate(&NSDate::dateWithTimeIntervalSince1970(unix))
            .to_string()
    }
}

/// The user's preferred language (`PRISM_LOCALE` overrides it).
pub fn system_locale() -> Locale {
    if let Ok(tag) = std::env::var("PRISM_LOCALE") {
        return Locale::new(&tag);
    }
    NSLocale::preferredLanguages()
        .firstObject()
        .map(|s| Locale::new(&s.to_string()))
        .unwrap_or_default()
}

/// Seconds east of UTC.
pub fn utc_offset() -> i32 {
    NSTimeZone::localTimeZone().secondsFromGMT() as i32
}

unsafe extern "C" {
    fn sysctlbyname(
        name: *const std::ffi::c_char,
        oldp: *mut std::ffi::c_void,
        oldlenp: *mut usize,
        newp: *mut std::ffi::c_void,
        newlen: usize,
    ) -> std::ffi::c_int;
}

/// A string `sysctl` value (`hw.machine`, `hw.model`).
pub fn sysctl_string(name: &str) -> Option<String> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut len = 0usize;
    // SAFETY: a size query, then a read into a buffer of that size.
    unsafe {
        if sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        ) != 0
            || len == 0
        {
            return None;
        }
        let mut buf = vec![0u8; len];
        if sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        buf.truncate(len);
        let s = String::from_utf8_lossy(&buf)
            .trim_end_matches('\0')
            .to_string();
        (!s.is_empty()).then_some(s)
    }
}

/// The OS version (`18.2`, `26.1`).
pub fn os_version() -> prism_core::Version {
    let v = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
    prism_core::Version::new(
        v.majorVersion as u32,
        v.minorVersion as u32,
        v.patchVersion as u32,
    )
}

/// Running in the iOS Simulator.
pub fn is_simulator() -> bool {
    cfg!(target_abi = "sim") || std::env::var_os("SIMULATOR_MODEL_IDENTIFIER").is_some()
}

/// The hardware model (`iPhone17,1`, `Mac15,6`); simulators report the
/// simulated device.
pub fn model_id() -> Option<String> {
    if let Ok(id) = std::env::var("SIMULATOR_MODEL_IDENTIFIER") {
        return Some(id);
    }
    if cfg!(target_os = "macos") {
        sysctl_string("hw.model")
    } else {
        sysctl_string("hw.machine")
    }
}

#[cfg(feature = "apple-bridge")]
pub use bridge::{
    DeviceMessage, device_call, passkeys_available, security_capabilities, set_device_sink,
};

/// The configurations of a configurable widget kind that the widget
/// extension recorded in the App Group (the instances the system shows): the
/// source for [`prism_core::App::set_widget_configurations_source`].
#[cfg(feature = "apple-bridge")]
pub fn widget_configurations(kind: &str) -> Vec<prism_core::WidgetConfig> {
    use std::ffi::{CStr, CString, c_char, c_void};
    unsafe extern "C" {
        fn prism_widgets_configurations(kind: *const c_char) -> *mut c_char;
        fn free(p: *mut c_void);
    }
    let Ok(kind) = CString::new(kind) else {
        return Vec::new();
    };
    let p = unsafe { prism_widgets_configurations(kind.as_ptr()) };
    if p.is_null() {
        return Vec::new();
    }
    let json = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { free(p.cast()) };
    prism_core::WidgetConfig::list_from_json(&json)
}

/// Remote push (APNs) through the Swift bridge.
#[cfg(feature = "apple-bridge")]
#[path = "apple_push.rs"]
pub mod push;

/// Web views (#62): the `WKWebView` host of the Swift bridge
/// (`apple/Bridge/PrismWebView.swift`), shared by the macOS and UIKit
/// backends. tvOS has no WebKit: `WebHost::new` returns `None` there.
#[cfg(all(feature = "apple-bridge", not(target_os = "watchos")))]
/// Videos (#207): the Swift bridge's AVKit player in a host view.
pub mod video {
    use std::ffi::{CString, c_char, c_void};
    use std::ptr::NonNull;

    use prism_core::{NodeId, UiEvent};

    unsafe extern "C" {
        fn prism_video_new(id: u64) -> *mut c_void;
        fn prism_video_load(host: *mut c_void, src: *const c_char);
        fn prism_video_config(
            host: *mut c_void,
            controls: bool,
            autoplay: bool,
            looping: bool,
            muted: bool,
        );
        fn prism_video_size(host: *mut c_void, w: *mut f64, h: *mut f64) -> bool;
        fn prism_video_release(host: *mut c_void);
    }

    /// A video's host view (an `NSView` / `UIView` holding the player).
    /// Dropping it stops the playback.
    pub struct VideoHost(NonNull<c_void>);

    impl VideoHost {
        /// The host view of node `id`, or `None` without AVKit (watchOS).
        pub fn new(id: NodeId) -> Option<VideoHost> {
            NonNull::new(unsafe { prism_video_new(id) }).map(VideoHost)
        }

        /// The host view (borrowed).
        pub fn view_ptr(&self) -> *mut c_void {
            self.0.as_ptr()
        }

        /// The source: a URL (assets resolved to a file URL), then one
        /// `name: value` header per line.
        pub fn load(&self, src: &str) {
            let c = CString::new(src.replace('\0', "")).unwrap_or_default();
            unsafe { prism_video_load(self.0.as_ptr(), c.as_ptr()) }
        }

        /// How it plays (`prism_core::VideoConfig`).
        pub fn configure(&self, c: &prism_core::VideoConfig) {
            unsafe {
                prism_video_config(self.0.as_ptr(), c.controls, c.autoplay, c.looping, c.muted)
            }
        }

        /// The video's size in points, once known.
        pub fn natural_size(&self) -> Option<(f32, f32)> {
            let (mut w, mut h) = (0., 0.);
            unsafe { prism_video_size(self.0.as_ptr(), &mut w, &mut h) }
                .then_some((w as f32, h as f32))
        }
    }

    impl Drop for VideoHost {
        fn drop(&mut self) {
            unsafe { prism_video_release(self.0.as_ptr()) }
        }
    }

    /// A video's measure: its natural size scaled to `max_width`, 16:9 at
    /// that width (or 320 wide) before it is known.
    pub fn measure(host: &VideoHost, max_width: Option<f32>) -> (f32, f32) {
        match host.natural_size() {
            Some((w, h)) => {
                let shown = max_width.map_or(w, |m| m.min(w));
                (shown, shown * h / w)
            }
            None => {
                let w = max_width.unwrap_or(320.);
                (w, w * 9. / 16.)
            }
        }
    }

    /// What the bridge reported: `Some(Ended)` at the end; `None` for a
    /// size (the backend lays out again).
    pub fn event(id: NodeId, kind: i32) -> Option<UiEvent> {
        (kind == 1).then_some(UiEvent::Ended(id))
    }
}

pub mod web {
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::ptr::NonNull;

    use prism_core::{NodeId, UiEvent};

    unsafe extern "C" {
        fn prism_web_view_new(id: u64) -> *mut c_void;
        fn prism_web_view_load(host: *mut c_void, url: *const c_char);
        fn prism_web_view_config(host: *mut c_void, config: *const c_char);
        fn prism_web_view_post(host: *mut c_void, json: *const c_char);
        fn prism_web_view_reload(host: *mut c_void);
        fn prism_web_view_release(host: *mut c_void);
    }

    /// A web view's host view (an `NSView` or `UIView` filling its
    /// superview, holding the `WKWebView`). Dropping it stops the page.
    pub struct WebHost(NonNull<c_void>);

    fn c(s: &str) -> CString {
        CString::new(s.replace('\0', "")).unwrap_or_default()
    }

    impl WebHost {
        /// The host view of node `id`, or `None` without WebKit (tvOS).
        pub fn new(id: NodeId) -> Option<WebHost> {
            NonNull::new(unsafe { prism_web_view_new(id) }).map(WebHost)
        }

        /// The host view, an `NSView` / `UIView` (borrowed).
        pub fn view_ptr(&self) -> *mut c_void {
            self.0.as_ptr()
        }

        /// Loads `url` (on the next turn of the main loop, with the config
        /// set in the same update).
        pub fn load(&self, url: &str) {
            unsafe { prism_web_view_load(self.0.as_ptr(), c(url).as_ptr()) }
        }

        /// The options (`prism_core::web_view::WebViewConfig`, encoded).
        pub fn configure(&self, config: &str) {
            unsafe { prism_web_view_config(self.0.as_ptr(), c(config).as_ptr()) }
        }

        /// A message (JSON) for the page, once loaded, if it shows an
        /// allowed origin.
        pub fn post(&self, json: &str) {
            unsafe { prism_web_view_post(self.0.as_ptr(), c(json).as_ptr()) }
        }

        /// Reloads the page.
        pub fn reload(&self) {
            unsafe { prism_web_view_reload(self.0.as_ptr()) }
        }
    }

    impl Drop for WebHost {
        fn drop(&mut self) {
            unsafe { prism_web_view_release(self.0.as_ptr()) }
        }
    }

    /// What the bridge reports through `prism_web_view_event` (which each
    /// backend exports): kind 0 a message (`a`: JSON, `b`: origin), kind 1
    /// a finished load (`a`: URL).
    ///
    /// # Safety
    /// `a` and `b` are valid C strings.
    pub unsafe fn event(id: u64, kind: i32, a: *const c_char, b: *const c_char) -> Option<UiEvent> {
        let s = |p: *const c_char| {
            (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
        };
        match kind {
            0 => Some(UiEvent::WebMessage(id, s(a)?, s(b).unwrap_or_default())),
            1 => Some(UiEvent::WebLoad(id, s(a)?)),
            _ => None,
        }
    }
}

/// Drop targets (#111) through the Swift bridge (`apple/Bridge/PrismDrop.swift`):
/// on macOS the backend's views implement `NSDraggingDestination` and the
/// bridge reads the pasteboard; on iOS, iPadOS and visionOS the bridge adds
/// a `UIDropInteraction`. tvOS and watchOS have no drag and drop.
#[cfg(all(
    feature = "apple-bridge",
    not(any(target_os = "watchos", target_os = "tvos"))
))]
pub mod drops {
    use std::ffi::{CStr, CString, c_char, c_void};

    use prism_core::{NodeId, PickedFile};

    unsafe extern "C" {
        #[cfg(target_os = "macos")]
        fn prism_drop_accepts(info: *const c_void, types: *const c_char) -> bool;
        #[cfg(target_os = "macos")]
        fn prism_drop_perform(node: u64, info: *const c_void, types: *const c_char) -> bool;
        #[cfg(not(target_os = "macos"))]
        fn prism_drop_target(view: *const c_void, node: u64, types: *const c_char);
    }

    fn lines(types: &[String]) -> CString {
        CString::new(types.join("\n").replace('\0', "")).unwrap_or_default()
    }

    /// Whether the drag (an `NSDraggingInfo`) holds files `types` accept.
    ///
    /// # Safety
    /// `info` is a live `NSDraggingInfo`.
    #[cfg(target_os = "macos")]
    pub unsafe fn accepts(info: *const c_void, types: &[String]) -> bool {
        unsafe { prism_drop_accepts(info, lines(types).as_ptr()) }
    }

    /// Copies the dropped files `types` accept where the app can read them,
    /// then reports them for `node` (see [`report`]). `false` if none.
    ///
    /// # Safety
    /// `info` is a live `NSDraggingInfo`.
    #[cfg(target_os = "macos")]
    pub unsafe fn perform(node: NodeId, info: *const c_void, types: &[String]) -> bool {
        unsafe { prism_drop_perform(node, info, lines(types).as_ptr()) }
    }

    /// Makes `view` (a `UIView`, borrowed) a drop target of `types` for
    /// `node`, or not (`None`).
    ///
    /// # Safety
    /// `view` is a live `UIView`, on the main thread.
    #[cfg(not(target_os = "macos"))]
    pub unsafe fn set_target(view: *const c_void, node: NodeId, types: Option<&[String]>) {
        let types = types.map(lines);
        let ptr = types.as_ref().map_or(std::ptr::null(), |t| t.as_ptr());
        unsafe { prism_drop_target(view, node, ptr) }
    }

    /// What the bridge reports through `prism_drop_event` (each backend
    /// exports it).
    #[derive(Debug, PartialEq)]
    pub enum DropReport {
        /// Accepted files came over the node (`true`) or left it.
        Hover(NodeId, bool),
        /// Files were dropped: their copies.
        Files(NodeId, Vec<PickedFile>),
        /// Dropped files could not be copied (#290): the details.
        Failed(NodeId, String),
    }

    /// `kind` 0 / 1: files left / came over `node`; 2: dropped, `files`
    /// one per line (`path\tname\tmime\tsize`); 3: some could not be
    /// copied, `files` is `failed\tdropped`.
    ///
    /// # Safety
    /// `files` is null or a valid C string.
    pub unsafe fn report(node: u64, kind: i32, files: *const c_char) -> Option<DropReport> {
        match kind {
            0 | 1 => Some(DropReport::Hover(node, kind == 1)),
            2 if !files.is_null() => {
                let text = unsafe { CStr::from_ptr(files) }.to_string_lossy();
                Some(DropReport::Files(node, parse(&text)))
            }
            3 if !files.is_null() => {
                let text = unsafe { CStr::from_ptr(files) }.to_string_lossy();
                let (failed, dropped) = text.split_once('\t')?;
                Some(DropReport::Failed(
                    node,
                    prism_core::DropError::unread(failed.parse().ok()?, dropped.parse().ok()?),
                ))
            }
            _ => None,
        }
    }

    fn parse(text: &str) -> Vec<PickedFile> {
        text.lines()
            .filter_map(|line| {
                let mut p = line.split('\t');
                Some(PickedFile {
                    path: p.next()?.to_string(),
                    name: p.next()?.to_string(),
                    mime: p.next()?.to_string(),
                    size: p.next()?.parse().unwrap_or(0),
                })
            })
            .collect()
    }
}

/// Device APIs through the Swift bridge (`apple/Bridge/PrismDevice.swift` in `prism-apple-build`).
#[cfg(feature = "apple-bridge")]
mod bridge {
    use std::ffi::{CStr, CString, c_char};
    use std::sync::OnceLock;

    use prism_core::device::{
        Accuracy, DeviceCall, DeviceError, DeviceEvent, DeviceValue, Location, Motion,
        PermissionState, PickedFile,
    };

    unsafe extern "C" {
        fn prism_device_permission(id: u64, name: *const c_char, ask: bool);
        fn prism_device_location(id: u64, accuracy: i32, stream: bool);
        fn prism_device_motion_start(id: u64, interval: f64);
        fn prism_device_stop(id: u64);
        fn prism_device_pick_images(id: u64, limit: u32, camera: bool);
        fn prism_device_pick_files(id: u64, types: *const c_char, multiple: bool);
        fn prism_device_pick_folder(id: u64, title: *const c_char);
        fn prism_device_haptic(kind: *const c_char);
        fn prism_device_copy(text: *const c_char);
        fn prism_device_paste(id: u64);
        fn prism_device_paste_item(id: u64);
        fn prism_device_copy_item(entries: *const c_char);
        fn prism_device_share(text: *const c_char, url: *const c_char, files: *const c_char);
        fn prism_device_open_url(url: *const c_char);
        fn prism_device_save_file(id: u64, path: *const c_char, name: *const c_char);
        fn prism_device_reveal(path: *const c_char);
        // PrismSecurity.swift
        fn prism_secure_get(id: u64, key: *const c_char, prompt: *const c_char, shared: bool);
        fn prism_secure_set(
            id: u64,
            key: *const c_char,
            value: *const c_char,
            biometric: bool,
            confirm_owner: bool,
            shared: bool,
        );
        fn prism_secure_delete(id: u64, key: *const c_char, shared: bool);
        fn prism_biometry_kind() -> i32;
        fn prism_device_auth_available() -> bool;
        fn prism_biometry_status(id: u64);
        fn prism_authenticate(id: u64, reason: *const c_char, passcode: bool);
        fn prism_apple_sign_in(
            id: u64,
            email: bool,
            name: bool,
            nonce: *const c_char,
            state: *const c_char,
        );
        fn prism_apple_sign_in_available() -> bool;
        fn prism_auth_redirect(id: u64, preferred: *const c_char);
        fn prism_web_auth(id: u64, url: *const c_char, redirect: *const c_char, ephemeral: bool);
        fn prism_webauthn(id: u64, create: bool, conditional: bool, options: *const c_char);
        fn prism_passkeys_available() -> bool;
        // PrismPurchases.swift (StoreKit 2, #306)
        fn prism_store_products(id: u64, ids: *const c_char);
        fn prism_store_purchase(id: u64, product: *const c_char, account: *const c_char);
        fn prism_store_entitlements(id: u64, restore: bool);
        fn prism_store_watch(id: u64);
    }

    /// What the Keychain and LocalAuthentication offer here:
    /// `(biometry, device_auth, sign_in_with_apple)`.
    pub fn security_capabilities() -> (prism_core::BiometryKind, bool, bool) {
        use prism_core::BiometryKind as K;
        unsafe {
            let kind = match prism_biometry_kind() {
                1 => K::TouchId,
                2 => K::FaceId,
                3 => K::OpticId,
                _ => K::None,
            };
            (
                kind,
                prism_device_auth_available(),
                prism_apple_sign_in_available(),
            )
        }
    }

    /// Passkeys of the app's own relying party (#94): Apple platforms but
    /// the watch.
    pub fn passkeys_available() -> bool {
        unsafe { prism_passkeys_available() }
    }

    /// What the bridge reports back, for the backend to hand to its runtime.
    pub enum DeviceMessage {
        Reply(u64, Result<DeviceValue, DeviceError>),
        Event(u64, DeviceEvent),
    }

    static SINK: OnceLock<fn(DeviceMessage)> = OnceLock::new();

    /// Where answers go (the backend's main-thread dispatcher).
    pub fn set_device_sink(f: fn(DeviceMessage)) {
        let _ = SINK.set(f);
    }

    fn send(m: DeviceMessage) {
        if let Some(f) = SINK.get() {
            f(m)
        }
    }

    fn c(s: &str) -> CString {
        CString::new(s.replace('\0', "")).unwrap_or_default()
    }

    /// Forwards a device call to the platform.
    pub fn device_call(id: u64, call: DeviceCall) {
        let accuracy = |a: Accuracy| match a {
            Accuracy::Coarse => 0,
            Accuracy::Balanced => 1,
            Accuracy::Best => 2,
        };
        unsafe {
            match call {
                DeviceCall::PermissionStatus(p) => {
                    prism_device_permission(id, c(p.name()).as_ptr(), false)
                }
                DeviceCall::RequestPermission(p) => {
                    prism_device_permission(id, c(p.name()).as_ptr(), true)
                }
                DeviceCall::CurrentLocation(a) => prism_device_location(id, accuracy(a), false),
                DeviceCall::WatchLocation(a) => prism_device_location(id, accuracy(a), true),
                DeviceCall::WatchMotion(every) => {
                    prism_device_motion_start(id, every.as_secs_f64())
                }
                DeviceCall::StopWatch => prism_device_stop(id),
                DeviceCall::PickImages(o) => prism_device_pick_images(id, o.limit, o.camera),
                // Types one per line: MIME types or `.ext`.
                DeviceCall::PickFiles(o) => {
                    prism_device_pick_files(id, c(&o.types.join("\n")).as_ptr(), o.multiple)
                }
                // A folder and its files (#225).
                DeviceCall::PickFolder(o) => {
                    let title = o.title.as_deref().map(c);
                    prism_device_pick_folder(
                        id,
                        title.as_ref().map_or(std::ptr::null(), |t| t.as_ptr()),
                    )
                }
                DeviceCall::ReadClipboard => prism_device_paste(id),
                // #246: the bridge answers files named by their type.
                DeviceCall::ReadClipboardItem => prism_device_paste_item(id),
                // One entry per line: type \t base64.
                DeviceCall::WriteClipboard(item) => {
                    let lines: Vec<String> = item
                        .entries()
                        .map(|(ty, data)| format!("{ty}\t{}", prism_core::clipboard::base64(data)))
                        .collect();
                    prism_device_copy_item(c(&lines.join("\n")).as_ptr())
                }
                DeviceCall::Haptic(h) => prism_device_haptic(c(h.name()).as_ptr()),
                DeviceCall::CopyText(t) => prism_device_copy(c(&t).as_ptr()),
                DeviceCall::Share(s) => {
                    let text = s.text.as_deref().map(c);
                    let url = s.url.as_deref().map(c);
                    let files = (!s.files.is_empty()).then(|| c(&s.files.join("\n")));
                    let ptr =
                        |o: &Option<CString>| o.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
                    prism_device_share(ptr(&text), ptr(&url), ptr(&files))
                }
                DeviceCall::OpenUrl(u) => prism_device_open_url(c(&u).as_ptr()),
                // #210: a file on disk with the offered name; the bridge
                // removes a staged copy once done.
                DeviceCall::SaveFile(f) => match crate::save::stage(&f, &std::env::temp_dir()) {
                    Ok(path) => prism_device_save_file(
                        id,
                        c(&path.to_string_lossy()).as_ptr(),
                        c(&f.file_name()).as_ptr(),
                    ),
                    Err(e) => send(DeviceMessage::Reply(
                        id,
                        Err(DeviceError::Failed(e.to_string())),
                    )),
                },
                DeviceCall::RevealPath(p) => prism_device_reveal(c(&p.to_string_lossy()).as_ptr()),
                // No tabs here: the system opens it (Safari, Mail, another app).
                DeviceCall::OpenUrlIn(u, _) => prism_device_open_url(c(&u).as_ptr()),
                DeviceCall::SecureGet {
                    key,
                    prompt,
                    shared,
                } => {
                    let prompt = prompt.as_deref().map(c);
                    prism_secure_get(
                        id,
                        c(&key).as_ptr(),
                        prompt.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                        shared,
                    )
                }
                DeviceCall::SecureSet {
                    key,
                    value,
                    options,
                } => prism_secure_set(
                    id,
                    c(&key).as_ptr(),
                    c(&value).as_ptr(),
                    options.biometric || options.confirm_owner,
                    options.confirm_owner,
                    options.shared,
                ),
                DeviceCall::SecureDelete { key, shared } => {
                    prism_secure_delete(id, c(&key).as_ptr(), shared)
                }
                DeviceCall::BiometryStatus => prism_biometry_status(id),
                DeviceCall::Authenticate(r) => {
                    prism_authenticate(id, c(&r.reason).as_ptr(), r.passcode)
                }
                DeviceCall::SignInWithApple(r) => {
                    // Apple gets the nonce hashed (the token's `nonce` claim).
                    let nonce = r.apple_nonce().as_deref().map(c);
                    let state = r.state.as_deref().map(c);
                    let ptr =
                        |o: &Option<CString>| o.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
                    prism_apple_sign_in(id, r.email, r.full_name, ptr(&nonce), ptr(&state))
                }
                DeviceCall::AuthRedirect(preferred) => {
                    let p = preferred.as_deref().map(c);
                    prism_auth_redirect(id, p.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()))
                }
                DeviceCall::WebAuth(r) => {
                    prism_web_auth(id, c(&r.url).as_ptr(), c(&r.redirect).as_ptr(), r.ephemeral)
                }
                // Passkeys of the app's own relying party (#94).
                DeviceCall::WebAuthn(r) => {
                    prism_webauthn(id, r.create, r.conditional, c(&r.options).as_ptr())
                }
                // No cookies natively (secure storage keeps secrets), no live
                // session to renew.
                DeviceCall::SetCookie(_) => {
                    send(DeviceMessage::Reply(id, Err(DeviceError::Unavailable)));
                }
                DeviceCall::Reconnect => {}
                // No browser id natively: nothing to renew.
                DeviceCall::RenewBrowserId => {
                    send(DeviceMessage::Reply(id, Ok(DeviceValue::Done)));
                }
                // In-app purchases (#306), StoreKit 2: ids one per line.
                DeviceCall::StoreProducts(ids) => {
                    prism_store_products(id, c(&ids.join("\n")).as_ptr())
                }
                DeviceCall::Purchase(p) => {
                    let account = p.account_token.as_deref().map(c);
                    prism_store_purchase(
                        id,
                        c(&p.product).as_ptr(),
                        account.as_ref().map_or(std::ptr::null(), |a| a.as_ptr()),
                    )
                }
                DeviceCall::StoreEntitlements => prism_store_entitlements(id, false),
                DeviceCall::RestorePurchases => prism_store_entitlements(id, true),
                DeviceCall::WatchEntitlements => prism_store_watch(id),
                // A checkout page coming back to the server is a web thing.
                DeviceCall::StoreReturnUrl | DeviceCall::StoreCheckout { .. } => {
                    send(DeviceMessage::Reply(id, Err(DeviceError::Unavailable)));
                }
            }
        }
    }

    fn string(p: *const c_char) -> Option<String> {
        (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }

    fn some(v: f64) -> Option<f64> {
        (!v.is_nan()).then_some(v)
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn prism_device_reply_permission(id: u64, state: *const c_char) {
        let state = PermissionState::from_name(&string(state).unwrap_or_default());
        send(DeviceMessage::Reply(id, Ok(DeviceValue::Permission(state))));
    }

    #[unsafe(no_mangle)]
    #[allow(clippy::too_many_arguments)]
    pub extern "C" fn prism_device_reply_location(
        id: u64,
        event: bool,
        lat: f64,
        lon: f64,
        alt: f64,
        accuracy: f64,
        speed: f64,
        heading: f64,
        time: f64,
    ) {
        let l = Location {
            latitude: lat,
            longitude: lon,
            altitude: some(alt),
            accuracy,
            speed: some(speed),
            heading: some(heading),
            timestamp: time,
        };
        send(if event {
            DeviceMessage::Event(id, DeviceEvent::Location(l))
        } else {
            DeviceMessage::Reply(id, Ok(DeviceValue::Location(l)))
        });
    }

    #[unsafe(no_mangle)]
    #[allow(clippy::too_many_arguments)]
    pub extern "C" fn prism_device_motion(
        id: u64,
        ax: f64,
        ay: f64,
        az: f64,
        gx: f64,
        gy: f64,
        gz: f64,
        rx: f64,
        ry: f64,
        rz: f64,
        roll: f64,
        pitch: f64,
        yaw: f64,
    ) {
        send(DeviceMessage::Event(
            id,
            DeviceEvent::Motion(Motion {
                acceleration: [ax, ay, az],
                gravity: [gx, gy, gz],
                rotation_rate: [rx, ry, rz],
                attitude: [roll, pitch, yaw],
            }),
        ));
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn prism_device_reply_files(id: u64, lines: *const c_char) {
        let files = string(lines)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let mut p = line.split('\t');
                Some(PickedFile {
                    path: p.next()?.to_string(),
                    name: p.next()?.to_string(),
                    mime: p.next()?.to_string(),
                    size: p.next()?.parse().unwrap_or(0),
                })
            })
            .collect();
        send(DeviceMessage::Reply(id, Ok(DeviceValue::Files(files))));
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn prism_device_reply_text(id: u64, text: *const c_char) {
        send(DeviceMessage::Reply(
            id,
            Ok(DeviceValue::Text(string(text))),
        ));
    }

    /// A JSON object of strings (`DeviceValue::Fields`).
    #[unsafe(no_mangle)]
    pub extern "C" fn prism_device_reply_fields(id: u64, json: *const c_char) {
        let fields = string(json)
            .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
            .and_then(|v| v.as_object().cloned())
            .map(|o| {
                o.into_iter()
                    .filter_map(|(k, v)| Some((k, v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        send(DeviceMessage::Reply(id, Ok(DeviceValue::Fields(fields))));
    }

    /// What the user owns now, for the entitlements stream `id`: a JSON
    /// array of product ids (PrismPurchases.swift).
    #[unsafe(no_mangle)]
    pub extern "C" fn prism_store_event(id: u64, json: *const c_char) {
        let ids = string(json)
            .and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok())
            .unwrap_or_default();
        send(DeviceMessage::Event(id, DeviceEvent::Entitlements(ids)));
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn prism_device_reply_error(id: u64, event: bool, message: *const c_char) {
        let error = DeviceError::from_message(&string(message).unwrap_or_default());
        send(if event {
            DeviceMessage::Event(id, DeviceEvent::Error(error))
        } else {
            DeviceMessage::Reply(id, Err(error))
        });
    }
}

/// `cx.cloud_storage()` on Apple platforms: the iCloud key-value store
/// (synced between the user's devices, 1 MB). Needs the
/// `com.apple.developer.ubiquity-kvstore-identifier` entitlement
/// (`[ios] icloud = true` in Prism.toml).
pub struct ICloudStorage {
    store: Retained<objc2_foundation::NSUbiquitousKeyValueStore>,
}

impl ICloudStorage {
    /// The iCloud store if the bundle enables it (Info.plist `PrismICloud`),
    /// otherwise `None` (use local storage instead).
    pub fn new() -> Option<Self> {
        let enabled = objc2_foundation::NSBundle::mainBundle()
            .objectForInfoDictionaryKey(&NSString::from_str("PrismICloud"))
            .is_some();
        if !enabled {
            return None;
        }
        let store = objc2_foundation::NSUbiquitousKeyValueStore::defaultStore();
        // Fetch what other devices wrote since the last launch.
        store.synchronize();
        Some(ICloudStorage { store })
    }

    /// Whether an iCloud account is signed in (otherwise the store stays local).
    pub fn signed_in() -> bool {
        objc2_foundation::NSFileManager::defaultManager()
            .ubiquityIdentityToken()
            .is_some()
    }
}

impl prism_core::Storage for ICloudStorage {
    fn get(&self, key: &str) -> Option<String> {
        self.store
            .stringForKey(&NSString::from_str(key))
            .map(|s| s.to_string())
    }
    fn set(&mut self, key: &str, value: &str) {
        self.store
            .setString_forKey(Some(&NSString::from_str(value)), &NSString::from_str(key));
        self.store.synchronize();
    }
    fn remove(&mut self, key: &str) {
        self.store.removeObjectForKey(&NSString::from_str(key));
        self.store.synchronize();
    }
    fn keys(&self) -> Vec<String> {
        self.store
            .dictionaryRepresentation()
            .allKeys()
            .iter()
            .map(|k| k.to_string())
            .collect()
    }
}

/// The keys of an `NSUbiquitousKeyValueStoreDidChangeExternallyNotification`.
pub fn changed_cloud_keys(n: &objc2_foundation::NSNotification) -> Vec<String> {
    let Some(info) = n.userInfo() else {
        return Vec::new();
    };
    let key = unsafe { objc2_foundation::NSUbiquitousKeyValueStoreChangedKeysKey };
    info.objectForKey(key)
        .and_then(|keys| keys.downcast::<objc2_foundation::NSArray>().ok())
        .map(|keys| {
            keys.iter()
                .filter_map(|k| k.downcast::<NSString>().ok().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Core Animation pieces of the UIKit and AppKit backends' motion.
pub mod motion {
    use objc2::rc::Retained;
    use objc2_core_foundation::CGAffineTransform;
    use objc2_foundation::{NSArray, NSNumber, NSString, NSValue};
    use objc2_quartz_core::{
        CAAnimation, CAAnimationGroup, CABasicAnimation, CACurrentMediaTime, CAKeyframeAnimation,
        CALayer, CAMediaTiming, CAMediaTimingFunction, CATransform3D,
        NSValueCATransform3DAdditions, kCAFillModeBoth,
    };
    use prism_core::{Curve, Transform, Transition};

    /// `t` as an affine transform around the layer's anchor point.
    pub fn affine(t: Transform) -> CGAffineTransform {
        matrix(t.matrix())
    }

    pub fn matrix(m: [f64; 6]) -> CGAffineTransform {
        CGAffineTransform {
            a: m[0],
            b: m[1],
            c: m[2],
            d: m[3],
            tx: m[4],
            ty: m[5],
        }
    }

    /// The curve of `t` (springs as ease-in-out: loops do not overshoot).
    pub fn timing(t: Transition) -> Retained<CAMediaTimingFunction> {
        let (x1, y1, x2, y2) = match t.curve {
            Curve::Spring => Transition::ease(0).bezier(),
            _ => t.bezier(),
        };
        CAMediaTimingFunction::functionWithControlPoints(x1, y1, x2, y2)
    }

    const KEY: &str = "prism.repeat";

    /// Evenly timed steps from `from` to `to`, field by field: enough of
    /// them that a matrix interpolated between two neighbours follows the
    /// turn (0° to 360° is a full turn, not "no change").
    pub fn steps(from: Transform, to: Transform) -> Vec<Transform> {
        let turn = (to.rotate - from.rotate).abs();
        let n = if turn == 0. {
            1
        } else {
            ((turn / 30.).ceil() as usize).max(2)
        };
        (0..=n)
            .map(|i| from.lerp(to, i as f32 / n as f32))
            .collect()
    }

    /// Loops `layer` from `from` to `to` (opacity, transform), back and
    /// forth, or starting over when `restart`, without touching its model
    /// values. `m` turns a transform into the layer's matrix.
    pub fn repeat(
        layer: &CALayer,
        from: (f32, Transform),
        to: (f32, Transform),
        t: Transition,
        restart: bool,
        m: impl Fn(Transform) -> CATransform3D,
    ) {
        let key = NSString::from_str(KEY);
        layer.removeAnimationForKey(&key);
        let opacity = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("opacity")));
        let transform =
            CAKeyframeAnimation::animationWithKeyPath(Some(&NSString::from_str("transform")));
        let frames: Vec<Retained<NSValue>> = steps(from.1, to.1)
            .into_iter()
            .map(|tr| unsafe { NSValue::valueWithCATransform3D(m(tr)) })
            .collect();
        unsafe {
            opacity.setFromValue(Some(&NSNumber::new_f32(from.0)));
            opacity.setToValue(Some(&NSNumber::new_f32(to.0)));
            let frames: Vec<&objc2::runtime::AnyObject> =
                frames.iter().map(|v| v.as_ref() as _).collect();
            transform.setValues(Some(&NSArray::from_slice(&frames)));
        }
        let group = CAAnimationGroup::new();
        let parts: [Retained<CAAnimation>; 2] = [
            Retained::into_super(Retained::into_super(opacity)),
            Retained::into_super(Retained::into_super(transform)),
        ];
        group.setAnimations(Some(&NSArray::from_retained_slice(&parts)));
        let duration = t.duration.max(0.01) as f64;
        for part in &parts {
            part.setDuration(duration);
        }
        group.setDuration(duration);
        group.setAutoreverses(!restart);
        group.setRepeatCount(f32::INFINITY);
        group.setTimingFunction(Some(&timing(t)));
        // Kept when the app goes to the background or the view moves.
        group.setRemovedOnCompletion(false);
        if t.delay > 0. {
            group.setBeginTime(CACurrentMediaTime() + t.delay as f64);
            group.setFillMode(unsafe { kCAFillModeBoth });
        }
        layer.addAnimation_forKey(&group, Some(&key));
    }

    pub fn stop_repeat(layer: &CALayer) {
        layer.removeAnimationForKey(&NSString::from_str(KEY));
    }

    /// Animates `layer`'s transform from `from` to its current (model) value.
    pub fn animate_transform(layer: &CALayer, from: CATransform3D, t: Transition) {
        let a = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("transform")));
        unsafe { a.setFromValue(Some(&NSValue::valueWithCATransform3D(from))) };
        a.setDuration(t.duration.max(0.01) as f64);
        a.setTimingFunction(Some(&timing_overshoot(t)));
        if t.delay > 0. {
            a.setBeginTime(CACurrentMediaTime() + t.delay as f64);
            a.setFillMode(unsafe { kCAFillModeBoth });
        }
        layer.addAnimation_forKey(&a, Some(&NSString::from_str("prism.transform")));
    }

    /// The curve of `t`, springs overshooting.
    pub fn timing_overshoot(t: Transition) -> Retained<CAMediaTimingFunction> {
        let (x1, y1, x2, y2) = t.bezier();
        CAMediaTimingFunction::functionWithControlPoints(x1, y1, x2, y2)
    }

    pub fn transform3d(t: CGAffineTransform) -> CATransform3D {
        CATransform3D::new_affine_transform(t)
    }
}

/// The Core Animation corner mask for `corners` (leading = left: prism-native
/// already mirrored right-to-left layouts). `y_down`: the layer's y axis
/// points down (UIKit, and AppKit layers with flipped geometry).
pub fn corner_mask(corners: prism_core::Corners, y_down: bool) -> objc2_quartz_core::CACornerMask {
    use objc2_quartz_core::CACornerMask as M;
    use prism_core::Corners as C;
    let (top_left, top_right, bottom_right, bottom_left) = if y_down {
        (
            M::LayerMinXMinYCorner,
            M::LayerMaxXMinYCorner,
            M::LayerMaxXMaxYCorner,
            M::LayerMinXMaxYCorner,
        )
    } else {
        (
            M::LayerMinXMaxYCorner,
            M::LayerMaxXMaxYCorner,
            M::LayerMaxXMinYCorner,
            M::LayerMinXMinYCorner,
        )
    };
    let mut mask = M::empty();
    for (corner, bit) in [
        (C::TOP_LEADING, top_left),
        (C::TOP_TRAILING, top_right),
        (C::BOTTOM_TRAILING, bottom_right),
        (C::BOTTOM_LEADING, bottom_left),
    ] {
        if corners.contains(corner) {
            mask |= bit;
        }
    }
    mask
}

/// The bytes of 3D scene blob `id` for the Swift scene player
/// (`prism_core::scene3d::blobs`): calls `cb(ctx, bytes, len)` before
/// returning, or `cb(ctx, NULL, 0)` when the blob is unknown or gone. The
/// bytes are valid during the call only.
///
/// # Safety
///
/// `id` is null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prism_scene_blob(
    id: *const std::ffi::c_char,
    ctx: *mut std::ffi::c_void,
    cb: extern "C" fn(*mut std::ffi::c_void, *const u8, usize),
) {
    let id = (!id.is_null())
        .then(|| unsafe { std::ffi::CStr::from_ptr(id) }.to_str().ok())
        .flatten();
    match id.and_then(prism_core::scene3d::blobs::get) {
        Some((bytes, _)) => cb(ctx, bytes.as_ptr(), bytes.len()),
        None => cb(ctx, std::ptr::null(), 0),
    }
}
