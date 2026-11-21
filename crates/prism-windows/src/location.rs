//! Location: Windows' location service (`Windows.Devices.Geolocation`,
//! `Geolocator`), which works for an unpackaged app. Its positions come
//! from the PC's GNSS receiver if it has one, else from Wi-Fi networks and
//! the IP address (Windows' own location provider).
//!
//! - **capability**: the location service (`lfsvc`) is installed and
//!   its start type is not « disabled » (it starts on demand, so it need
//!   not be running), no policy turns location off, and the `Geolocator` says
//!   it can give positions (`LocationStatus` is not `NotAvailable`);
//! - **permission**: Settings › Privacy & security › Location, read
//!   without asking (`AppCapability("location").CheckAccess`): the
//!   device's switch, the user's, the desktop apps' one. A desktop app
//!   never runs « in the background » the way a phone's does, so
//!   `LocationAlways` is the same permission. `request_permission` asks
//!   with `Geolocator::RequestAccessAsync`, which shows Windows' prompt
//!   where Windows has one (Windows 11 24H2 and later ask once per app);
//! - **positions**: `GetGeopositionAsync` with the web's settings
//!   (`device.rs`); a watch is a `Geolocator` with `PositionChanged` and
//!   `StatusChanged`, its moves filtered by `MovementThreshold`.
//!
//! Errors: access refused (the setting is off, or the user said no) is
//! `Denied`; no provider is `Unavailable`; no position in time is "location
//! unknown", as on Apple platforms.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use prism_core::{Accuracy, DeviceError, DeviceEvent, PermissionState};
use windows::Devices::Geolocation::{
    GeolocationAccessStatus, Geolocator, Geoposition, PositionAccuracy, PositionChangedEventArgs,
    PositionStatus, StatusChangedEventArgs,
};
use windows::Foundation::{IReference, PropertyValue, TimeSpan, TypedEventHandler};
use windows::Security::Authorization::AppCapabilityAccess::{
    AppCapability, AppCapabilityAccessStatus,
};
use windows::Win32::Foundation::{E_ACCESSDENIED, ERROR_TIMEOUT};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QUERY_SERVICE_CONFIGW, QueryServiceConfigW,
    SC_MANAGER_CONNECT, SERVICE_DISABLED, SERVICE_QUERY_CONFIG,
};
use windows::core::{HSTRING, Interface, PCWSTR, w};

use crate::device::{self, Settings};
use crate::win::{log, wide};

/// The consent store of Settings › Privacy & security.
const CONSENT: &str =
    r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore";

/// A string value of the registry; `None` when it is not there.
pub fn registry_string(root: HKEY, key: &str, name: &str) -> Option<String> {
    let (key, name) = (wide(key), wide(name));
    let mut buf = [0u16; 64];
    let mut size = std::mem::size_of_val(&buf) as u32;
    unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    let len = (size as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

fn registry_dword(root: HKEY, key: &str, name: &str) -> Option<u32> {
    let (key, name) = (wide(key), wide(name));
    let mut value = 0u32;
    let mut size = 4u32;
    unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    Some(value)
}

/// The privacy setting `capability` (`location`, `webcam`) for the device,
/// the user and `app` (`NonPackaged` for desktop apps, a package family).
pub fn consent(capability: &str, app: &str) -> PermissionState {
    let key = format!(r"{CONSENT}\{capability}");
    let values = [
        registry_string(HKEY_LOCAL_MACHINE, &key, "Value"),
        registry_string(HKEY_CURRENT_USER, &key, "Value"),
        registry_string(HKEY_CURRENT_USER, &format!(r"{key}\{app}"), "Value"),
    ];
    device::consent(&values.iter().map(Option::as_deref).collect::<Vec<_>>())
}

/// A policy turns location off (`LocationAndSensors`).
fn policy_off(name: &str) -> bool {
    let key = r"SOFTWARE\Policies\Microsoft\Windows\LocationAndSensors";
    [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER]
        .into_iter()
        .any(|root| registry_dword(root, key, name) == Some(1))
}

/// Whether service `name` is installed and may start.
fn service_usable(name: PCWSTR) -> bool {
    unsafe {
        let Ok(scm) = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) else {
            // No answer: assume it is there.
            return true;
        };
        let usable = match OpenServiceW(scm, name, SERVICE_QUERY_CONFIG) {
            Ok(service) => {
                let mut needed = 0u32;
                let _ = QueryServiceConfigW(service, None, 0, &mut needed);
                let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
                let config = buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
                let ok = QueryServiceConfigW(service, Some(config), needed, &mut needed).is_ok();
                let _ = CloseServiceHandle(service);
                !ok || (*config).dwStartType != SERVICE_DISABLED
            }
            Err(_) => false,
        };
        let _ = CloseServiceHandle(scm);
        usable
    }
}

/// What the PC can do: see the module's doc. Blocking (a few
/// milliseconds): called off the UI thread at startup.
pub fn present() -> bool {
    if policy_off("DisableLocation") {
        log!("location: turned off by policy");
        return false;
    }
    if !service_usable(w!("lfsvc")) {
        log!("location: the location service (lfsvc) is missing or disabled");
        return false;
    }
    match Geolocator::new().and_then(|g| g.LocationStatus()) {
        Ok(PositionStatus::NotAvailable) => {
            log!("location: no location provider");
            false
        }
        Ok(_) => true,
        Err(e) => {
            log!("location: {}", e.message());
            false
        }
    }
}

/// The location setting, without asking.
pub fn permission() -> PermissionState {
    let status = AppCapability::Create(&HSTRING::from("location")).and_then(|c| c.CheckAccess());
    match status {
        Ok(AppCapabilityAccessStatus::Allowed) => PermissionState::Granted,
        Ok(AppCapabilityAccessStatus::UserPromptRequired) => PermissionState::NotDetermined,
        Ok(AppCapabilityAccessStatus::DeniedByUser | AppCapabilityAccessStatus::DeniedBySystem) => {
            PermissionState::Denied
        }
        // Windows 10 before 1903 has no `AppCapability`: the settings.
        _ => consent("location", "NonPackaged"),
    }
}

/// Asks for location access (Windows' prompt where it has one), then
/// gives the state to `done` on the UI thread. UI thread.
pub fn request(done: impl FnOnce(PermissionState) + Send + 'static) {
    let op = match Geolocator::RequestAccessAsync() {
        Ok(op) => op,
        Err(e) => {
            log!("location: {}", e.message());
            return done(permission());
        }
    };
    // Always answers, so a watch waiting for it is started or removed.
    crate::win::when("location", &op, move |r| {
        let state = match r {
            Ok(GeolocationAccessStatus::Allowed) => PermissionState::Granted,
            Ok(GeolocationAccessStatus::Denied) => PermissionState::Denied,
            // Unspecified, or no answer: what the settings say.
            _ => permission(),
        };
        crate::shell::on_ui(move || done(state));
    });
}

fn locator(s: &Settings) -> windows::core::Result<Geolocator> {
    let g = Geolocator::new()?;
    match s.meters {
        Some(m) => {
            let m: IReference<u32> = PropertyValue::CreateUInt32(m)?.cast()?;
            g.SetDesiredAccuracyInMeters(&m)?;
        }
        None => g.SetDesiredAccuracy(PositionAccuracy::High)?,
    }
    Ok(g)
}

fn reading(p: &Geoposition) -> windows::core::Result<prism_core::Location> {
    let c = p.Coordinate()?;
    let at = c.Point()?.Position()?;
    let value = |r: windows::core::Result<IReference<f64>>| r.and_then(|r| r.Value()).ok();
    Ok(device::location(&device::Reading {
        latitude: at.Latitude,
        longitude: at.Longitude,
        altitude: at.Altitude,
        altitude_accuracy: value(c.AltitudeAccuracy()),
        accuracy: c.Accuracy()?,
        speed: value(c.Speed()),
        heading: value(c.Heading()),
        timestamp_ticks: c.Timestamp()?.UniversalTime,
    }))
}

/// A failed position as Prism's error.
fn failure(e: &windows::core::Error) -> DeviceError {
    if e.code() == E_ACCESSDENIED {
        DeviceError::Denied
    } else if e.code() == ERROR_TIMEOUT.to_hresult() {
        DeviceError::Failed("location unknown".into())
    } else {
        DeviceError::Failed(format!("location: {}", e.message()))
    }
}

/// One position (blocking): call off the UI thread.
pub fn current(accuracy: Accuracy) -> Result<prism_core::Location, DeviceError> {
    let s = device::settings(accuracy);
    let g = locator(&s).map_err(|e| failure(&e))?;
    let position = g
        .GetGeopositionAsyncWithAgeAndTimeout(
            TimeSpan {
                Duration: device::ticks(s.max_age_s),
            },
            TimeSpan {
                Duration: device::ticks(s.timeout_s),
            },
        )
        .and_then(|op| op.join());
    match position {
        Ok(p) => reading(&p).map_err(|e| failure(&e)),
        Err(e) => Err(match g.LocationStatus() {
            Ok(PositionStatus::Disabled) => DeviceError::Denied,
            Ok(PositionStatus::NotAvailable) => DeviceError::Unavailable,
            Ok(PositionStatus::NoData) => DeviceError::Failed("location unknown".into()),
            _ => failure(&e),
        }),
    }
}

fn status_name(s: PositionStatus) -> &'static str {
    match s {
        PositionStatus::Ready => "Ready",
        PositionStatus::Initializing => "Initializing",
        PositionStatus::NoData => "NoData",
        PositionStatus::Disabled => "Disabled",
        PositionStatus::NotInitialized => "NotInitialized",
        PositionStatus::NotAvailable => "NotAvailable",
        _ => "?",
    }
}

/// `AppCapabilityAccessStatus`'s name (camera.rs's too).
pub fn access_name(s: AppCapabilityAccessStatus) -> &'static str {
    match s {
        AppCapabilityAccessStatus::DeniedBySystem => "DeniedBySystem",
        AppCapabilityAccessStatus::NotDeclaredByApp => "NotDeclaredByApp",
        AppCapabilityAccessStatus::DeniedByUser => "DeniedByUser",
        AppCapabilityAccessStatus::UserPromptRequired => "UserPromptRequired",
        AppCapabilityAccessStatus::Allowed => "Allowed",
        _ => "?",
    }
}

/// What `PRISM_DEVICE_REPORT` says of location: the checks behind the
/// capability and the permission and, only when access is already allowed
/// (asking would show Windows' prompt), one position and a watch's first
/// event. Where the PC is stays out of it: only accuracy and age.
/// Blocking (up to 40 s): call off the UI thread.
pub fn report() -> serde_json::Value {
    let access = AppCapability::Create(&HSTRING::from("location")).and_then(|c| c.CheckAccess());
    let status = Geolocator::new().and_then(|g| g.LocationStatus());
    let allowed = matches!(access, Ok(AppCapabilityAccessStatus::Allowed));
    let position = match allowed {
        true => match current(Accuracy::Balanced) {
            Ok(l) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0., |d| d.as_secs_f64());
                serde_json::json!({
                    "ok": true,
                    "accuracy_m": l.accuracy.round(),
                    "age_s": (now - l.timestamp).round(),
                    "altitude": l.altitude.is_some(),
                })
            }
            Err(e) => serde_json::json!({ "error": format!("{e:?}") }),
        },
        false => NOT_ASKED.into(),
    };
    let watch = match allowed {
        true => watch_report(),
        false => NOT_ASKED.into(),
    };
    serde_json::json!({
        "policy_disable_location": policy_off("DisableLocation"),
        "policy_disable_windows_provider": policy_off("DisableWindowsLocationProvider"),
        "service_lfsvc": service_usable(w!("lfsvc")),
        "locator_status": status.map_or_else(|e| e.message(), |s| status_name(s).into()),
        "access": access.map_or_else(|e| e.message(), |a| access_name(a).into()),
        "settings": format!("{:?}", consent("location", "NonPackaged")),
        "permission": format!("{:?}", permission()),
        "position": position,
        "watch": watch,
    })
}

const NOT_ASKED: &str = "not asked: access is not allowed, and asking shows a prompt";

/// A watch's first event (20 s at most), without where it says the PC is.
fn watch_report() -> serde_json::Value {
    let (tx, rx) = std::sync::mpsc::channel();
    let started = Instant::now();
    let sink: Sink = Arc::new(move |e| {
        let _ = tx.send((started.elapsed(), e));
    });
    let watch = match start(Accuracy::Balanced, sink) {
        Ok(w) => w,
        Err(e) => return serde_json::json!({ "error": format!("{:?}", failure(&e)) }),
    };
    let first = rx.recv_timeout(Duration::from_secs(20));
    drop(watch);
    match first {
        Ok((after, DeviceEvent::Location(l))) => serde_json::json!({
            "ok": true,
            "first_after_s": (after.as_secs_f64() * 10.).round() / 10.,
            "accuracy_m": l.accuracy.round(),
        }),
        Ok((_, DeviceEvent::Error(e))) => serde_json::json!({ "error": format!("{e:?}") }),
        Ok(_) => serde_json::json!({ "error": "not a location event" }),
        Err(_) => serde_json::json!({ "error": "no event in 20 s" }),
    }
}

/// Where a watch's events go (from Windows' threads).
type Sink = Arc<dyn Fn(DeviceEvent) + Send + Sync>;

/// A watch: its `Geolocator` and handlers, removed when it is dropped.
struct Watch {
    locator: Geolocator,
    position: i64,
    status: i64,
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.locator.RemovePositionChanged(self.position);
        let _ = self.locator.RemoveStatusChanged(self.status);
    }
}

thread_local! {
    /// Watches by call id; `None` while access is being asked.
    static WATCHES: RefCell<HashMap<u64, Option<Watch>>> = RefCell::new(HashMap::new());
}

/// Starts watch `id`: `send` gets its events on the UI thread. UI thread.
pub fn watch(id: u64, accuracy: Accuracy, send: fn(u64, DeviceEvent)) {
    WATCHES.with(|w| w.borrow_mut().insert(id, None));
    request(move |state| {
        // Stopped while Windows asked.
        if !WATCHES.with(|w| w.borrow().contains_key(&id)) {
            return;
        }
        // Not refused: the position says (a desktop app may get one
        // without a prompt).
        if state == PermissionState::Denied {
            WATCHES.with(|w| w.borrow_mut().remove(&id));
            return send(id, DeviceEvent::Error(DeviceError::Denied));
        }
        let sink: Sink = Arc::new(move |e| crate::shell::on_ui(move || deliver(id, e, send)));
        match start(accuracy, sink) {
            Ok(watch) => {
                WATCHES.with(|w| w.borrow_mut().insert(id, Some(watch)));
            }
            Err(e) => {
                WATCHES.with(|w| w.borrow_mut().remove(&id));
                send(id, DeviceEvent::Error(failure(&e)));
            }
        }
    });
}

fn start(accuracy: Accuracy, sink: Sink) -> windows::core::Result<Watch> {
    let s = device::settings(accuracy);
    let g = locator(&s)?;
    g.SetMovementThreshold(s.movement_m)?;
    // The handlers run on Windows' threads.
    let to = sink.clone();
    let position = g.PositionChanged(
        &TypedEventHandler::<Geolocator, PositionChangedEventArgs>::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                let e = match args.Position().and_then(|p| reading(&p)) {
                    Ok(l) => DeviceEvent::Location(l),
                    Err(e) => DeviceEvent::Error(failure(&e)),
                };
                to(e);
            }
            Ok(())
        }),
    )?;
    let status = g.StatusChanged(
        &TypedEventHandler::<Geolocator, StatusChangedEventArgs>::new(move |_, args| {
            let e = match args.as_ref().map(|a| a.Status()) {
                Some(Ok(PositionStatus::Disabled)) => DeviceError::Denied,
                Some(Ok(PositionStatus::NotAvailable)) => DeviceError::Unavailable,
                _ => return Ok(()),
            };
            sink(DeviceEvent::Error(e));
            Ok(())
        }),
    )?;
    Ok(Watch {
        locator: g,
        position,
        status,
    })
}

/// Sends a watch's event unless it was stopped meanwhile. UI thread.
fn deliver(id: u64, e: DeviceEvent, send: fn(u64, DeviceEvent)) {
    if WATCHES.with(|w| w.borrow().contains_key(&id)) {
        send(id, e);
    }
}

/// Stops watch `id` (nothing if it is not one). UI thread.
pub fn stop(id: u64) {
    let watch = WATCHES.with(|w| w.borrow_mut().remove(&id));
    drop(watch);
}

/// Stops every watch (quitting). UI thread.
pub fn stop_all() {
    let all: Vec<_> = WATCHES.with(|w| w.borrow_mut().drain().collect());
    drop(all);
}
