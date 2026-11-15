//! Device APIs: permissions, location, motion sensors, photos and camera,
//! haptics, clipboard, sharing, opening URLs.
//!
//! One-shot calls are `async` (inside `cx.spawn`); sensors are streams with a
//! handler:
//!
//! ```ignore
//! cx.spawn(async move |this, cx| {
//!     if cx.request_permission(Permission::Location).await.is_granted() {
//!         let here = cx.current_location().await?;
//!         cx.update_entity(&this, |m, _| m.here = Some(here));
//!     }
//!     let photos = cx.pick_images(PickImages::new().limit(3)).await?;
//! });
//! let watch = cx.watch_motion(Duration::from_millis(50), |m, app| { … });
//! cx.haptic(Haptic::Success);
//! cx.copy_text("Hello");
//! cx.share(Share::text("Look").url("https://example.com"));
//! ```
//!
//! | | iOS | macOS | Web |
//! |---|---|---|---|
//! | Location | CoreLocation | CoreLocation | `navigator.geolocation` |
//! | Motion | CoreMotion | — | `devicemotion` / `deviceorientation` |
//! | Photos | PHPicker | open panel | `<input type=file>` |
//! | Camera | UIImagePickerController | — | `capture` input (mobile) |
//! | Haptics | UIFeedbackGenerator | trackpad (NSHapticFeedback) | `navigator.vibrate` |
//! | Clipboard, share, URLs | UIPasteboard, share sheet, `open` | NSPasteboard, sharing picker, NSWorkspace | Clipboard API, Web Share, `window.open` |
//!
//! On Apple platforms, each permission needs a usage text in Info.plist:
//! `prism` adds them from `[permissions]` in `Prism.toml`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::Duration;

use crate::app::App;
use crate::task::AsyncApp;

/// Something the user must allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Permission {
    /// While the app is in use.
    Location,
    /// Also in the background.
    LocationAlways,
    Camera,
    Photos,
    Microphone,
    Motion,
    Notifications,
}

impl Permission {
    pub fn name(self) -> &'static str {
        match self {
            Permission::Location => "location",
            Permission::LocationAlways => "location_always",
            Permission::Camera => "camera",
            Permission::Photos => "photos",
            Permission::Microphone => "microphone",
            Permission::Motion => "motion",
            Permission::Notifications => "notifications",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [
            Permission::Location,
            Permission::LocationAlways,
            Permission::Camera,
            Permission::Photos,
            Permission::Microphone,
            Permission::Motion,
            Permission::Notifications,
        ]
        .into_iter()
        .find(|p| p.name() == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PermissionState {
    /// Not asked yet: `request_permission` shows the system prompt.
    #[default]
    NotDetermined,
    Granted,
    /// Partially granted (iOS: selected photos only, approximate location…).
    Limited,
    Denied,
    /// The device or platform does not have it.
    Unavailable,
}

impl PermissionState {
    pub fn is_granted(self) -> bool {
        matches!(self, PermissionState::Granted | PermissionState::Limited)
    }

    pub fn name(self) -> &'static str {
        match self {
            PermissionState::NotDetermined => "not_determined",
            PermissionState::Granted => "granted",
            PermissionState::Limited => "limited",
            PermissionState::Denied => "denied",
            PermissionState::Unavailable => "unavailable",
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name {
            "granted" => PermissionState::Granted,
            "limited" => PermissionState::Limited,
            "denied" => PermissionState::Denied,
            "unavailable" => PermissionState::Unavailable,
            _ => PermissionState::NotDetermined,
        }
    }
}

/// A position on Earth.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Location {
    pub latitude: f64,
    pub longitude: f64,
    /// Meters above sea level.
    pub altitude: Option<f64>,
    /// Radius of uncertainty, in meters.
    pub accuracy: f64,
    /// Meters per second.
    pub speed: Option<f64>,
    /// Degrees from true north.
    pub heading: Option<f64>,
    /// Unix seconds.
    pub timestamp: f64,
}

/// One motion sample.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Motion {
    /// Acceleration the user gives the device, in g (gravity removed).
    pub acceleration: [f64; 3],
    /// Gravity direction, in g.
    pub gravity: [f64; 3],
    /// Gyroscope: rotation rate around x, y, z, in radians per second.
    pub rotation_rate: [f64; 3],
    /// Orientation: roll, pitch, yaw in radians.
    pub attitude: [f64; 3],
}

/// A file the user picked (photo, image…), copied where the app can read it.
#[derive(Clone, Debug, PartialEq)]
pub struct PickedFile {
    /// Usable with `picture(path)` and `std::fs` natively; a URL on the web.
    pub path: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
}

/// Options for [`AsyncApp::pick_images`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickImages {
    /// Maximum number of images (0 = no limit).
    pub limit: u32,
    /// Take a photo with the camera instead of choosing from the library.
    pub camera: bool,
}

impl Default for PickImages {
    fn default() -> Self {
        Self::new()
    }
}

impl PickImages {
    pub fn new() -> Self {
        PickImages {
            limit: 1,
            camera: false,
        }
    }
    pub fn limit(mut self, n: u32) -> Self {
        self.limit = n;
        self
    }
    pub fn camera(mut self) -> Self {
        self.camera = true;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Haptic {
    Selection,
    Light,
    Medium,
    Heavy,
    Success,
    Warning,
    Error,
}

impl Haptic {
    pub fn name(self) -> &'static str {
        match self {
            Haptic::Selection => "selection",
            Haptic::Light => "light",
            Haptic::Medium => "medium",
            Haptic::Heavy => "heavy",
            Haptic::Success => "success",
            Haptic::Warning => "warning",
            Haptic::Error => "error",
        }
    }
}

/// Content for the system share sheet.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Share {
    pub text: Option<String>,
    pub url: Option<String>,
    /// Local files (e.g. a [`PickedFile`] path).
    pub files: Vec<String>,
}

impl Share {
    pub fn text(text: impl Into<String>) -> Self {
        Share {
            text: Some(text.into()),
            ..Default::default()
        }
    }
    pub fn link(url: impl Into<String>) -> Self {
        Share {
            url: Some(url.into()),
            ..Default::default()
        }
    }
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
    pub fn file(mut self, path: impl Into<String>) -> Self {
        self.files.push(path.into());
        self
    }
}

/// How precise a location must be (precision costs battery and time).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Accuracy {
    /// About a kilometer.
    Coarse,
    #[default]
    Balanced,
    Best,
}

/// A call the app makes; backends answer with [`App::device_reply`].
#[derive(Clone, Debug, PartialEq)]
pub enum DeviceCall {
    PermissionStatus(Permission),
    RequestPermission(Permission),
    CurrentLocation(Accuracy),
    PickImages(PickImages),
    ReadClipboard,
    /// Starts a stream; events use this call's id.
    WatchLocation(Accuracy),
    WatchMotion(Duration),
    /// Stops the stream started by call `id`.
    StopWatch,
    Haptic(Haptic),
    CopyText(String),
    Share(Share),
    OpenUrl(String),
    /// Secure storage (see [`crate::security`]): the secret, or `None`.
    SecureGet {
        key: String,
        /// Shown when the item needs the device owner (biometrics).
        prompt: Option<String>,
        shared: bool,
    },
    SecureSet {
        key: String,
        value: String,
        options: crate::security::SecureOptions,
    },
    SecureDelete {
        key: String,
        shared: bool,
    },
    /// Biometrics and device passcode, right now.
    BiometryStatus,
    /// Asks the device owner to prove it's them.
    Authenticate(crate::security::AuthRequest),
    /// Sign in with Apple (AuthenticationServices).
    SignInWithApple(crate::security::AppleSignIn),
    /// The redirect URI the platform can catch for a web sign-in: a custom
    /// scheme (Apple, Android), a loopback port (Linux), the server (web).
    /// `Some(uri)` asks for that one.
    AuthRedirect(Option<String>),
    /// Opens `url` in the system's sign-in browser and answers the URL it
    /// was redirected to (`Text`).
    WebAuth(crate::security::WebAuthRequest),
}

/// A backend's answer to a [`DeviceCall`].
#[derive(Clone, Debug, PartialEq)]
pub enum DeviceValue {
    Permission(PermissionState),
    Location(Location),
    Files(Vec<PickedFile>),
    Text(Option<String>),
    /// Named values (biometry status, an Apple ID credential…).
    Fields(std::collections::BTreeMap<String, String>),
    Done,
}

/// Why a device call failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceError {
    /// The user refused the permission.
    Denied,
    /// The user dismissed the picker / prompt.
    Cancelled,
    /// Not supported on this platform or device.
    Unavailable,
    Failed(String),
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceError::Denied => f.write_str("permission denied"),
            DeviceError::Cancelled => f.write_str("cancelled"),
            DeviceError::Unavailable => f.write_str("not available on this device"),
            DeviceError::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for DeviceError {}

impl DeviceError {
    /// For backends: `denied`, `cancelled`, `unavailable` or a message.
    pub fn from_message(message: &str) -> Self {
        match message {
            "denied" => DeviceError::Denied,
            "cancelled" => DeviceError::Cancelled,
            "unavailable" => DeviceError::Unavailable,
            m => DeviceError::Failed(m.to_string()),
        }
    }
}

type Slot = Rc<RefCell<(Option<Result<DeviceValue, DeviceError>>, Option<Waker>)>>;

/// A sensor event delivered to a stream.
#[derive(Clone, Debug, PartialEq)]
pub enum DeviceEvent {
    Location(Location),
    Motion(Motion),
    Error(DeviceError),
}

type StreamHandler = Rc<dyn Fn(&DeviceEvent, &mut App)>;

/// Device state kept by the [`App`].
#[derive(Default)]
pub(crate) struct Device {
    next_id: u64,
    pending: HashMap<u64, Slot>,
    streams: HashMap<u64, StreamHandler>,
}

/// A running sensor stream; stop it with [`App::stop_watching`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Watch(pub u64);

impl App {
    fn device_call(&mut self, call: DeviceCall) -> u64 {
        self.device.next_id += 1;
        let id = self.device.next_id;
        self.surfaces
            .requests
            .push(crate::PlatformRequest::Device { id, call });
        id
    }

    /// For backends: the answer to call `id`.
    pub fn device_reply(&mut self, id: u64, result: Result<DeviceValue, DeviceError>) {
        if let Some(slot) = self.device.pending.remove(&id) {
            let waker = {
                let mut s = slot.borrow_mut();
                s.0 = Some(result);
                s.1.take()
            };
            if let Some(w) = waker {
                w.wake();
            }
        }
    }

    /// For backends: a sensor event for stream `id`.
    pub fn device_event(&mut self, id: u64, event: DeviceEvent) {
        if let Some(handler) = self.device.streams.get(&id).cloned() {
            handler(&event, self);
        }
    }

    /// Location updates (asks for permission if needed). Errors arrive as
    /// [`DeviceEvent::Error`].
    pub fn watch_location(
        &mut self,
        accuracy: Accuracy,
        f: impl Fn(&Result<Location, DeviceError>, &mut App) + 'static,
    ) -> Watch {
        let id = self.device_call(DeviceCall::WatchLocation(accuracy));
        self.device.streams.insert(
            id,
            Rc::new(move |e, app| match e {
                DeviceEvent::Location(l) => f(&Ok(*l), app),
                DeviceEvent::Error(err) => f(&Err(err.clone()), app),
                DeviceEvent::Motion(_) => {}
            }),
        );
        Watch(id)
    }

    /// Motion samples about every `interval` (gyroscope, accelerometer,
    /// orientation).
    pub fn watch_motion(
        &mut self,
        interval: Duration,
        f: impl Fn(&Result<Motion, DeviceError>, &mut App) + 'static,
    ) -> Watch {
        let id = self.device_call(DeviceCall::WatchMotion(interval));
        self.device.streams.insert(
            id,
            Rc::new(move |e, app| match e {
                DeviceEvent::Motion(m) => f(&Ok(*m), app),
                DeviceEvent::Error(err) => f(&Err(err.clone()), app),
                DeviceEvent::Location(_) => {}
            }),
        );
        Watch(id)
    }

    /// Stops a location or motion stream.
    pub fn stop_watching(&mut self, watch: Watch) {
        if self.device.streams.remove(&watch.0).is_some() {
            // The stream's id identifies it for the backend.
            self.surfaces.requests.push(crate::PlatformRequest::Device {
                id: watch.0,
                call: DeviceCall::StopWatch,
            });
        }
    }

    pub fn haptic(&mut self, kind: Haptic) {
        self.device_call(DeviceCall::Haptic(kind));
    }

    pub fn copy_text(&mut self, text: impl Into<String>) {
        self.device_call(DeviceCall::CopyText(text.into()));
    }

    /// Opens the system share sheet.
    pub fn share(&mut self, content: Share) {
        self.device_call(DeviceCall::Share(content));
    }

    /// Opens a web page, a `mailto:`/`tel:` link or another app.
    pub fn open_url(&mut self, url: impl Into<String>) {
        self.device_call(DeviceCall::OpenUrl(url.into()));
    }
}

/// Resolves when the backend answers a device call.
pub struct DeviceFuture {
    slot: Slot,
}

impl Future for DeviceFuture {
    type Output = Result<DeviceValue, DeviceError>;
    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let mut s = self.slot.borrow_mut();
        match s.0.take() {
            Some(r) => Poll::Ready(r),
            None => {
                s.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

impl AsyncApp {
    /// Sends a device call and waits for the answer.
    pub fn device(&self, call: DeviceCall) -> DeviceFuture {
        let slot: Slot = Rc::new(RefCell::new((None, None)));
        self.update(|app| {
            let id = app.device_call(call);
            app.device.pending.insert(id, slot.clone());
        });
        DeviceFuture { slot }
    }

    /// The permission's current state (no prompt).
    pub async fn permission(&self, p: Permission) -> PermissionState {
        match self.device(DeviceCall::PermissionStatus(p)).await {
            Ok(DeviceValue::Permission(s)) => s,
            _ => PermissionState::Unavailable,
        }
    }

    /// Asks the user (once; afterwards returns the saved choice).
    pub async fn request_permission(&self, p: Permission) -> PermissionState {
        match self.device(DeviceCall::RequestPermission(p)).await {
            Ok(DeviceValue::Permission(s)) => s,
            _ => PermissionState::Unavailable,
        }
    }

    /// The current position (asks for permission if needed).
    pub async fn current_location(&self) -> Result<Location, DeviceError> {
        self.location_with(Accuracy::Balanced).await
    }

    pub async fn location_with(&self, accuracy: Accuracy) -> Result<Location, DeviceError> {
        match self.device(DeviceCall::CurrentLocation(accuracy)).await? {
            DeviceValue::Location(l) => Ok(l),
            other => Err(DeviceError::Failed(format!("unexpected answer {other:?}"))),
        }
    }

    /// Lets the user choose images (or take a photo). Empty if they cancel.
    pub async fn pick_images(&self, options: PickImages) -> Result<Vec<PickedFile>, DeviceError> {
        match self.device(DeviceCall::PickImages(options)).await {
            Ok(DeviceValue::Files(f)) => Ok(f),
            Err(DeviceError::Cancelled) => Ok(Vec::new()),
            Err(e) => Err(e),
            Ok(other) => Err(DeviceError::Failed(format!("unexpected answer {other:?}"))),
        }
    }

    /// Takes a photo with the camera.
    pub async fn take_photo(&self) -> Result<Option<PickedFile>, DeviceError> {
        Ok(self
            .pick_images(PickImages::new().camera())
            .await?
            .into_iter()
            .next())
    }

    pub async fn read_clipboard(&self) -> Option<String> {
        match self.device(DeviceCall::ReadClipboard).await {
            Ok(DeviceValue::Text(t)) => t,
            _ => None,
        }
    }
}
