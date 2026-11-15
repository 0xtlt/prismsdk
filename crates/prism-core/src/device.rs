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
//! let watch = cx.watch_motion(Duration::from_millis(50), |m, app| { /* … */ });
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
    /// Taking photos with the camera.
    Camera,
    /// Reading the photo library.
    Photos,
    /// Recording audio.
    Microphone,
    /// Motion sensors (iOS: motion and fitness activity).
    Motion,
    /// Showing notifications.
    Notifications,
}

impl Permission {
    /// Stable lowercase name, as backends report it (`"location_always"`).
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

    /// The permission named `name` (see [`Permission::name`]).
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

/// Whether a [`Permission`] is granted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PermissionState {
    /// Not asked yet: `request_permission` shows the system prompt.
    #[default]
    NotDetermined,
    /// Allowed.
    Granted,
    /// Partially granted (iOS: selected photos only, approximate location…).
    Limited,
    /// Refused: only the system settings can change it now.
    Denied,
    /// The device or platform does not have it.
    Unavailable,
}

impl PermissionState {
    /// `Granted` or `Limited`.
    pub fn is_granted(self) -> bool {
        matches!(self, PermissionState::Granted | PermissionState::Limited)
    }

    /// Stable lowercase name, as backends report it (`"not_determined"`).
    pub fn name(self) -> &'static str {
        match self {
            PermissionState::NotDetermined => "not_determined",
            PermissionState::Granted => "granted",
            PermissionState::Limited => "limited",
            PermissionState::Denied => "denied",
            PermissionState::Unavailable => "unavailable",
        }
    }

    /// The state named `name`; unknown names are `NotDetermined`.
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
    /// Degrees, north positive.
    pub latitude: f64,
    /// Degrees, east positive.
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
    /// File name.
    pub name: String,
    /// MIME type (`image/jpeg`).
    pub mime: String,
    /// Size in bytes.
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
    /// One image from the library.
    pub fn new() -> Self {
        PickImages {
            limit: 1,
            camera: false,
        }
    }
    /// Lets the user choose up to `n` images (0 = no limit).
    pub fn limit(mut self, n: u32) -> Self {
        self.limit = n;
        self
    }
    /// Takes a photo with the camera instead.
    pub fn camera(mut self) -> Self {
        self.camera = true;
        self
    }
}

// --- Files (#18) ---------------------------------------------------------------

/// Options for [`AsyncApp::pick_files`]: which files, and how many.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PickFiles {
    /// Accepted types: MIME types (`"image/*"`, `"application/pdf"`) or
    /// extensions (`".csv"`). Empty: any file.
    pub types: Vec<String>,
    /// Several files can be chosen.
    pub multiple: bool,
}

impl PickFiles {
    /// One file of any type.
    pub fn new() -> Self {
        Self::default()
    }
    /// Only these types (`["image/*", ".pdf"]`).
    pub fn types<S: Into<String>>(mut self, types: impl IntoIterator<Item = S>) -> Self {
        self.types = types.into_iter().map(Into::into).collect();
        self
    }
    /// Lets the user choose several files.
    pub fn multiple(mut self) -> Self {
        self.multiple = true;
        self
    }
    /// Whether a file named `name` of type `mime` is accepted.
    pub fn accepts(&self, name: &str, mime: &str) -> bool {
        self.types.is_empty()
            || self.types.iter().any(|t| {
                let t = t.trim().to_ascii_lowercase();
                if let Some(ext) = t.strip_prefix('.') {
                    name.to_ascii_lowercase().ends_with(&format!(".{ext}"))
                } else if let Some(family) = t.strip_suffix("/*") {
                    mime.to_ascii_lowercase().starts_with(&format!("{family}/"))
                } else {
                    mime.eq_ignore_ascii_case(&t)
                }
            })
    }
}

/// Files whose `path` is a URL (web uploads) and where their bytes are.
static LOCAL_FILES: std::sync::Mutex<Option<HashMap<String, std::path::PathBuf>>> =
    std::sync::Mutex::new(None);

/// Remembers where the bytes of a picked file whose `path` is a URL are
/// (the web backend saves uploads on the server), for [`PickedFile::read`].
#[doc(hidden)]
pub fn register_local_file(url: &str, file: std::path::PathBuf) {
    if let Ok(mut m) = LOCAL_FILES.lock() {
        m.get_or_insert_with(HashMap::new)
            .insert(url.to_string(), file);
    }
}

impl PickedFile {
    /// The file's bytes (natively the picked file; on the web the copy the
    /// browser uploaded to the server).
    pub fn read(&self) -> std::io::Result<Vec<u8>> {
        let local = LOCAL_FILES
            .lock()
            .ok()
            .and_then(|m| m.as_ref()?.get(&self.path).cloned());
        let path = local.unwrap_or_else(|| {
            std::path::PathBuf::from(self.path.strip_prefix("file://").unwrap_or(&self.path))
        });
        std::fs::read(path)
    }
}

/// A haptic feedback pattern (`UIFeedbackGenerator` styles on iOS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Haptic {
    /// A selection changed (a picker tick).
    Selection,
    /// A light impact.
    Light,
    /// A medium impact.
    Medium,
    /// A heavy impact.
    Heavy,
    /// A task succeeded.
    Success,
    /// A task produced a warning.
    Warning,
    /// A task failed.
    Error,
}

impl Haptic {
    /// Stable lowercase name, as backends receive it (`"success"`).
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
    /// Text to share.
    pub text: Option<String>,
    /// A link to share.
    pub url: Option<String>,
    /// Local files (e.g. a [`PickedFile`] path).
    pub files: Vec<String>,
}

impl Share {
    /// Shares `text`.
    pub fn text(text: impl Into<String>) -> Self {
        Share {
            text: Some(text.into()),
            ..Default::default()
        }
    }
    /// Shares the link `url`.
    pub fn link(url: impl Into<String>) -> Self {
        Share {
            url: Some(url.into()),
            ..Default::default()
        }
    }
    /// Adds a link.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
    /// Adds a local file.
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
    /// About a hundred meters: the usual choice.
    #[default]
    Balanced,
    /// The most precise the device can do (GPS).
    Best,
}

/// A call the app makes; backends answer with [`App::device_reply`].
#[derive(Clone, Debug, PartialEq)]
pub enum DeviceCall {
    /// The permission's current state, without prompting.
    PermissionStatus(Permission),
    /// Shows the system prompt if the permission was never asked.
    RequestPermission(Permission),
    /// One position.
    CurrentLocation(Accuracy),
    /// Opens the photo picker or the camera.
    PickImages(PickImages),
    /// Opens the file picker (`<input type=file>`, `UIDocumentPickerViewController`,
    /// `NSOpenPanel`, `GtkFileChooserNative`, `ACTION_OPEN_DOCUMENT`).
    PickFiles(PickFiles),
    /// The clipboard's text.
    ReadClipboard,
    /// Starts a stream; events use this call's id.
    WatchLocation(Accuracy),
    /// Starts a motion stream, one sample about every interval.
    WatchMotion(Duration),
    /// Stops the stream started by call `id`.
    StopWatch,
    /// Plays a haptic pattern.
    Haptic(Haptic),
    /// Puts text on the clipboard.
    CopyText(String),
    /// Opens the share sheet.
    Share(Share),
    /// Opens a URL with the system (browser, mail, another app).
    OpenUrl(String),
    /// Secure storage (see [`crate::security`]): the secret, or `None`.
    SecureGet {
        /// The item's name.
        key: String,
        /// Shown when the item needs the device owner (biometrics).
        prompt: Option<String>,
        /// Use the store shared with the app's extensions (see [`SecureOptions::shared`](crate::security::SecureOptions::shared)).
        shared: bool,
    },
    /// Secure storage (see [`crate::security`]): stores a secret, replacing the previous one.
    SecureSet {
        /// The item's name.
        key: String,
        /// The secret.
        value: String,
        /// How the secret is kept (biometric access, shared store).
        options: crate::security::SecureOptions,
    },
    /// Secure storage (see [`crate::security`]): removes a secret.
    SecureDelete {
        /// The item's name.
        key: String,
        /// Use the store shared with the app's extensions (see [`SecureOptions::shared`](crate::security::SecureOptions::shared)).
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

    // --- Opening a URL in a chosen place (web: this tab or a new one) ---
    /// Opens a URL: on the web in this tab or a new one; natively like
    /// [`DeviceCall::OpenUrl`] (the system decides).
    OpenUrlIn(String, crate::request::UrlTarget),
}

/// A backend's answer to a [`DeviceCall`].
#[derive(Clone, Debug, PartialEq)]
pub enum DeviceValue {
    /// Answer to a permission call.
    Permission(PermissionState),
    /// Answer to `CurrentLocation`.
    Location(Location),
    /// Answer to `PickImages`.
    Files(Vec<PickedFile>),
    /// Answer to `ReadClipboard` (`None` when it holds no text).
    Text(Option<String>),
    /// Named values (biometry status, an Apple ID credential…).
    Fields(std::collections::BTreeMap<String, String>),
    /// The call completed without a value.
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
    /// Anything else, with the platform's message.
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
    /// A new position.
    Location(Location),
    /// A new motion sample.
    Motion(Motion),
    /// The stream failed (permission denied, sensor unavailable).
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

    /// Plays a haptic pattern (no-op where unsupported).
    pub fn haptic(&mut self, kind: Haptic) {
        self.device_call(DeviceCall::Haptic(kind));
    }

    /// Puts `text` on the clipboard.
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

    /// Opens `url` in `target`. On the web: this tab (leaving the app, e.g.
    /// for the host's sign-out page) or a new one; [`UrlTarget::Auto`] is
    /// what [`open_url`](App::open_url) does (this tab for the page's own
    /// origin, a new tab otherwise). Natively the system opens it whatever
    /// the target.
    ///
    /// [`UrlTarget::Auto`]: crate::request::UrlTarget::Auto
    pub fn open_url_in(&mut self, url: impl Into<String>, target: crate::request::UrlTarget) {
        self.device_call(DeviceCall::OpenUrlIn(url.into(), target));
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

    /// The current position at a given accuracy (asks for permission if needed).
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

    /// Lets the user choose files (documents, photos from the Files app).
    /// Empty if they cancel. Read them with [`PickedFile::read`].
    pub async fn pick_files(&self, options: PickFiles) -> Result<Vec<PickedFile>, DeviceError> {
        match self.device(DeviceCall::PickFiles(options)).await {
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

    /// The clipboard's text, `None` if empty, not text, or unavailable.
    pub async fn read_clipboard(&self) -> Option<String> {
        match self.device(DeviceCall::ReadClipboard).await {
            Ok(DeviceValue::Text(t)) => t,
            _ => None,
        }
    }
}
