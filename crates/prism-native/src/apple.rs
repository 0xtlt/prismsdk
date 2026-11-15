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

impl Formatter for FoundationFormatter {
    fn number(&self, locale: &Locale, value: f64, decimals: usize) -> String {
        let f = NSNumberFormatter::new();
        f.setLocale(Some(&ns_locale(locale)));
        f.setNumberStyle(NSNumberFormatterStyle::DecimalStyle);
        f.setMinimumFractionDigits(decimals);
        f.setMaximumFractionDigits(decimals);
        f.stringFromNumber(&NSNumber::new_f64(value))
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{value:.decimals$}"))
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
pub use bridge::{DeviceMessage, device_call, security_capabilities, set_device_sink};

/// Remote push (APNs) through the Swift bridge.
#[cfg(feature = "apple-bridge")]
#[path = "apple_push.rs"]
pub mod push;

/// Device APIs through the Swift bridge (`platform/apple/Bridge/PrismDevice.swift`).
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
        fn prism_device_haptic(kind: *const c_char);
        fn prism_device_copy(text: *const c_char);
        fn prism_device_paste(id: u64);
        fn prism_device_share(text: *const c_char, url: *const c_char, files: *const c_char);
        fn prism_device_open_url(url: *const c_char);
        // PrismSecurity.swift
        fn prism_secure_get(id: u64, key: *const c_char, prompt: *const c_char, shared: bool);
        fn prism_secure_set(
            id: u64,
            key: *const c_char,
            value: *const c_char,
            biometric: bool,
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
                DeviceCall::ReadClipboard => prism_device_paste(id),
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
                    options.biometric,
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
                    let nonce = r.nonce.as_deref().map(c);
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
        CAAnimation, CAAnimationGroup, CABasicAnimation, CACurrentMediaTime, CALayer,
        CAMediaTiming, CAMediaTimingFunction, CATransform3D, NSValueCATransform3DAdditions,
        kCAFillModeBoth,
    };
    use prism_core::{Curve, Transform, Transition};

    /// `t` as an affine transform around the layer's anchor point.
    pub fn affine(t: Transform) -> CGAffineTransform {
        let (s, r) = (t.scale as f64, (t.rotate as f64).to_radians());
        CGAffineTransform {
            a: s * r.cos(),
            b: s * r.sin(),
            c: -s * r.sin(),
            d: s * r.cos(),
            tx: t.x as f64,
            ty: t.y as f64,
        }
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

    /// Loops `layer` between `from` and `to` (opacity, transform), back and
    /// forth, without touching its model values.
    pub fn repeat(
        layer: &CALayer,
        from: (f32, CATransform3D),
        to: (f32, CATransform3D),
        t: Transition,
    ) {
        let key = NSString::from_str(KEY);
        layer.removeAnimationForKey(&key);
        let opacity = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("opacity")));
        let transform =
            CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("transform")));
        unsafe {
            opacity.setFromValue(Some(&NSNumber::new_f32(from.0)));
            opacity.setToValue(Some(&NSNumber::new_f32(to.0)));
            transform.setFromValue(Some(&NSValue::valueWithCATransform3D(from.1)));
            transform.setToValue(Some(&NSValue::valueWithCATransform3D(to.1)));
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
        group.setAutoreverses(true);
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
