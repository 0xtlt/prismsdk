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
//! | | iOS | macOS | Web | Android | Linux | Windows |
//! |---|---|---|---|---|---|---|
//! | Location | CoreLocation | CoreLocation | `navigator.geolocation` | `LocationManager` | GeoClue2 | `Geolocator` |
//! | Motion | CoreMotion | — | `devicemotion` / `deviceorientation` | `SensorManager` | — | `Windows.Devices.Sensors` |
//! | Photos | PHPicker | open panel | `<input type=file>` | photo picker | file chooser | `IFileOpenDialog` |
//! | Camera | UIImagePickerController | — | `capture` input (mobile) | `ACTION_IMAGE_CAPTURE` | — | the Camera app's capture UI |
//! | Haptics | UIFeedbackGenerator | trackpad (NSHapticFeedback) | `navigator.vibrate` | `VibrationEffect` | — | `VibrationDevice` |
//! | Clipboard, share, URLs | UIPasteboard, share sheet, `open` | NSPasteboard, sharing picker, NSWorkspace | Clipboard API, Web Share, `window.open` | `ClipboardManager`, `ACTION_SEND`, intents | GDK clipboard, —, the default app | Win32 clipboard, Share dialog, `ShellExecuteW` |
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

/// Options for [`AsyncApp::pick_folder`] (#225).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PickFolder {
    /// The picker's prompt (macOS, Linux): "Import".
    pub title: Option<String>,
}

impl PickFolder {
    /// A folder picker with the platform's prompt.
    pub fn new() -> Self {
        Self::default()
    }
    /// The picker's prompt, where the platform shows one (macOS, Linux).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

/// A folder the user picked, with every file in it (#225).
#[derive(Clone, Debug, PartialEq)]
pub struct PickedFolder {
    /// The folder's name (`Export`).
    pub name: String,
    /// Its path where the files are read in place (macOS, Linux, Windows); `None`
    /// where they were copied for the app (iOS, Android, the web).
    pub path: Option<String>,
    /// Its files, sub-folders included (hidden ones left out). Each one's
    /// `name` is its path in the folder (`notes/2024/a.md`, `/` between
    /// folders); read them with [`PickedFile::read`].
    pub files: Vec<PickedFile>,
}

/// The MIME type a backend gives the folder itself, first in the files it
/// answers a [`DeviceCall::PickFolder`] with.
pub const FOLDER_MIME: &str = "inode/directory";

impl PickedFolder {
    /// The folder in a backend's answer (#225): the folder first
    /// ([`FOLDER_MIME`]), then its files named by their path in it. Without
    /// that first entry (the web), the names start with the folder's name,
    /// which is taken off. Hidden files and folders (`.git`, `.DS_Store`)
    /// are left out. `None` for no answer at all.
    pub fn from_files(mut files: Vec<PickedFile>) -> Option<PickedFolder> {
        let (name, path) = match files.first() {
            Some(f) if f.mime == FOLDER_MIME => {
                let f = files.remove(0);
                (f.name, (!f.path.is_empty()).then_some(f.path))
            }
            Some(f) => {
                let root = f.name.split('/').next().unwrap_or_default().to_string();
                for f in &mut files {
                    if let Some(rest) = f.name.strip_prefix(&format!("{root}/")) {
                        f.name = rest.to_string();
                    }
                }
                (root, None)
            }
            None => return None,
        };
        files.retain(|f| !f.name.split('/').any(|s| s.starts_with('.')));
        Some(PickedFolder { name, path, files })
    }
}

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

/// Forgets the bytes behind `url` (the web backend deleted an old upload).
#[doc(hidden)]
pub fn forget_local_file(url: &str) {
    if let Ok(mut m) = LOCAL_FILES.lock()
        && let Some(m) = m.as_mut()
    {
        m.remove(url);
    }
}

/// Files [`register_local_file`] kept under `from` are now under `to`
/// (the web backend moved a browser's data to its new id).
#[doc(hidden)]
pub fn move_local_files(from: &std::path::Path, to: &std::path::Path) {
    if let Ok(mut m) = LOCAL_FILES.lock()
        && let Some(m) = m.as_mut()
    {
        for path in m.values_mut() {
            if let Ok(rest) = path.strip_prefix(from) {
                *path = to.join(rest);
            }
        }
    }
}

/// Where [`register_local_file`] keeps the bytes behind `url` (tests).
#[doc(hidden)]
pub fn local_file(url: &str) -> Option<std::path::PathBuf> {
    LOCAL_FILES
        .lock()
        .ok()
        .and_then(|m| m.as_ref()?.get(url).cloned())
}

/// Whether [`register_local_file`] knows `url` (tests).
#[doc(hidden)]
pub fn has_local_file(url: &str) -> bool {
    LOCAL_FILES
        .lock()
        .ok()
        .is_some_and(|m| m.as_ref().is_some_and(|m| m.contains_key(url)))
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

/// A file the app holds, for the user to keep ([`AsyncApp::save_file`],
/// #210): bytes it has in memory (fetched with a token, generated) or a
/// file of its own (in its cache or data directory).
#[derive(Clone, Debug, PartialEq)]
pub struct SaveFile {
    /// What is saved.
    pub source: SaveSource,
    /// The name offered to the user (`report.pdf`).
    pub name: String,
    /// The MIME type; guessed from the name's extension when `None`.
    pub mime: Option<String>,
}

/// What a [`SaveFile`] holds.
#[derive(Clone, PartialEq)]
pub enum SaveSource {
    /// Bytes in memory.
    Bytes(Vec<u8>),
    /// A file on the device (natively) or on the server (web).
    Path(std::path::PathBuf),
}

impl std::fmt::Debug for SaveSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveSource::Bytes(b) => write!(f, "Bytes({} bytes)", b.len()),
            SaveSource::Path(p) => f.debug_tuple("Path").field(p).finish(),
        }
    }
}

impl SaveFile {
    /// `bytes`, offered as `name`.
    pub fn bytes(bytes: impl Into<Vec<u8>>, name: impl Into<String>) -> Self {
        SaveFile {
            source: SaveSource::Bytes(bytes.into()),
            name: name.into(),
            mime: None,
        }
    }

    /// The file at `path`, offered under its own name (see [`name`](Self::name)).
    pub fn path(path: impl Into<std::path::PathBuf>) -> Self {
        let path = path.into();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        SaveFile {
            source: SaveSource::Path(path),
            name,
            mime: None,
        }
    }

    /// Offers it as `name` instead.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Its MIME type (`application/pdf`), when the extension doesn't say.
    pub fn mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = Some(mime.into());
        self
    }

    /// The name as a file name: no path separators or control characters,
    /// `file` when empty.
    pub fn file_name(&self) -> String {
        let name: String = self
            .name
            .chars()
            .map(|c| {
                if matches!(c, '/' | '\\') || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let name = name.trim().trim_start_matches('.');
        if name.is_empty() {
            "file".into()
        } else {
            name.into()
        }
    }

    /// The MIME type: the one given, else the extension's, else
    /// `application/octet-stream`.
    pub fn mime_type(&self) -> String {
        if let Some(m) = &self.mime {
            return m.clone();
        }
        let ext = self
            .name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "pdf" => "application/pdf",
            "json" => "application/json",
            "zip" => "application/zip",
            "txt" | "log" => "text/plain",
            "csv" => "text/csv",
            "md" => "text/markdown",
            "html" | "htm" => "text/html",
            "xml" => "application/xml",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "heic" => "image/heic",
            "svg" => "image/svg+xml",
            "mp4" | "m4v" => "video/mp4",
            "mov" => "video/quicktime",
            "mp3" => "audio/mpeg",
            "m4a" => "audio/mp4",
            "wav" => "audio/wav",
            "ics" => "text/calendar",
            "vcf" => "text/vcard",
            "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            _ => "application/octet-stream",
        }
        .into()
    }

    /// Its bytes (reads the file of a [`SaveSource::Path`]).
    pub fn read(&self) -> std::io::Result<std::borrow::Cow<'_, [u8]>> {
        match &self.source {
            SaveSource::Bytes(b) => Ok(std::borrow::Cow::Borrowed(b)),
            SaveSource::Path(p) => std::fs::read(p).map(std::borrow::Cow::Owned),
        }
    }

    /// Writes it to `dest`, replacing what is there; returns the size.
    pub fn write_to(&self, dest: &std::path::Path) -> std::io::Result<u64> {
        match &self.source {
            SaveSource::Bytes(b) => std::fs::write(dest, b).map(|_| b.len() as u64),
            SaveSource::Path(p) if p == dest => std::fs::metadata(p).map(|m| m.len()),
            SaveSource::Path(p) => std::fs::copy(p, dest),
        }
    }
}

/// Where [`AsyncApp::save_file`] put the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedFile {
    /// The file's path where the platform says it (macOS, Linux, Windows,
    /// iOS and iPadOS): for [`App::reveal_path`]. `None` on the web (the browser
    /// downloads it) and on Android (a document of the storage provider).
    pub path: Option<std::path::PathBuf>,
    /// Its name, as the user chose it where the platform says it.
    pub name: String,
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
    /// Opens a folder picker (#225); answers `Files`: the folder first
    /// ([`FOLDER_MIME`]), then its files named by their path in it.
    PickFolder(PickFolder),
    /// The clipboard's text.
    ReadClipboard,
    /// The clipboard in all its types (#246): answers
    /// [`DeviceValue::Clipboard`], or `Files` each named by its type, or
    /// `Text(None)` when it is empty.
    ReadClipboardItem,
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
    /// Puts every type of the item on the clipboard (#246). No answer.
    WriteClipboard(crate::clipboard::ClipboardItem),
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
    /// scheme (Apple, Android), a loopback port (Linux, Windows), the server (web).
    /// `Some(uri)` asks for that one.
    AuthRedirect(Option<String>),
    /// Opens `url` in the system's sign-in browser and answers the URL it
    /// was redirected to (`Text`).
    WebAuth(crate::security::WebAuthRequest),
    /// A WebAuthn ceremony with the app's own relying party's options
    /// (#94); answers the credential as JSON (`Text`).
    WebAuthn(crate::security::WebAuthnRequest),

    // --- Opening a URL in a chosen place (web: this tab or a new one) ---
    /// Opens a URL: on the web in this tab or a new one; natively like
    /// [`DeviceCall::OpenUrl`] (the system decides).
    OpenUrlIn(String, crate::request::UrlTarget),

    // --- Handing a file to the user (#210) ---
    /// Lets the user keep a file: a save panel (macOS, Linux, Windows), "Save to
    /// Files" (iOS, iPadOS, visionOS), `ACTION_CREATE_DOCUMENT` (Android), a
    /// download (web). Answers `Files` with the saved file (an empty path
    /// where the platform doesn't say it), `Cancelled` if the user cancels.
    SaveFile(SaveFile),
    /// Shows a file in the system's file manager, selected (macOS, Linux).
    /// No answer.
    RevealPath(std::path::PathBuf),

    // --- Cookies and the live session (web) ---
    /// Sets a cookie in the browser ([`App::set_cookie`]); natively
    /// [`DeviceError::Unavailable`].
    SetCookie(crate::request::Cookie),
    /// Ends the live session and opens a new one ([`App::reconnect`]); nothing
    /// natively. No answer.
    Reconnect,
    /// Gives the browser a new id and moves its data to it
    /// ([`AsyncApp::renew_browser_id`](crate::AsyncApp::renew_browser_id)).
    /// Answers `Done`; natively there is no browser id: `Done` at once.
    RenewBrowserId,

    // --- In-app purchases (see crate::store, #306) ---
    /// The store's products among these ids: answers `Text`, a JSON array
    /// ([`Product::list_from_json`](crate::store::Product::list_from_json)).
    StoreProducts(Vec<String>),
    /// Buys a product with the store's sheet: answers `Text` `purchased` or
    /// `pending` ([`PurchaseOutcome::name`](crate::store::PurchaseOutcome::name)),
    /// `Err(Cancelled)` when the user cancels.
    Purchase(crate::store::Purchase),
    /// What the user owns now (verified): answers `Text`, a JSON array of
    /// product ids.
    StoreEntitlements,
    /// Restores purchases (StoreKit's `AppStore.sync`), then answers like
    /// [`StoreEntitlements`](DeviceCall::StoreEntitlements).
    RestorePurchases,
    /// A stream: [`DeviceEvent::Entitlements`] with what the user owns, at
    /// once and whenever it changes.
    WatchEntitlements,
    /// The web: a URL of the server a checkout page comes back to (`Text`),
    /// with a state of its own. Natively `Unavailable`.
    StoreReturnUrl,
    /// The web: opens the app's store's checkout page `url` in a popup and
    /// answers the URL it came back to (`Text`), under `return_url`;
    /// `Err(Cancelled)` if the user closed it. Natively `Unavailable`.
    StoreCheckout {
        /// The checkout page.
        url: String,
        /// The URL it comes back to ([`StoreReturnUrl`](DeviceCall::StoreReturnUrl)).
        return_url: String,
    },
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
    /// Answer to `ReadClipboardItem` from backends that hold the data.
    Clipboard(crate::clipboard::ClipboardItem),
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
    /// An error the platform names (#189): `name` is the web's
    /// `DOMException` name where platforms share one (`"InvalidStateError"`:
    /// the passkey being registered is already on this device), `message`
    /// its text. Match on [`name`](DeviceError::name), never on the text.
    Platform {
        /// The error's name, as the web calls it.
        name: String,
        /// The platform's message.
        message: String,
    },
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceError::Denied => f.write_str("permission denied"),
            DeviceError::Cancelled => f.write_str("cancelled"),
            DeviceError::Unavailable => f.write_str("not available on this device"),
            DeviceError::Failed(m) => f.write_str(m),
            DeviceError::Platform { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for DeviceError {}

impl DeviceError {
    /// The marker between an error's name and its message in backend
    /// replies (#189): `"InvalidStateError\u{1f}The credential exists"`.
    pub const NAME_SEPARATOR: char = '\u{1f}';

    /// For backends: `denied`, `cancelled`, `unavailable`, a message, or a
    /// named error as `name` + [`NAME_SEPARATOR`](Self::NAME_SEPARATOR) +
    /// message.
    pub fn from_message(message: &str) -> Self {
        match message {
            "denied" => DeviceError::Denied,
            "cancelled" => DeviceError::Cancelled,
            "unavailable" => DeviceError::Unavailable,
            m => match m.split_once(Self::NAME_SEPARATOR) {
                Some((name, message)) if !name.is_empty() => DeviceError::Platform {
                    name: name.to_string(),
                    message: message.to_string(),
                },
                _ => DeviceError::Failed(m.to_string()),
            },
        }
    }

    /// The platform's name for the error (#189), when it gave one.
    pub fn name(&self) -> Option<&str> {
        match self {
            DeviceError::Platform { name, .. } => Some(name),
            _ => None,
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
    /// Picked files on their way to the app (#112): `sent` of `total`
    /// bytes have arrived (web uploads).
    Upload {
        /// Bytes received so far, every file included.
        sent: u64,
        /// Bytes of every picked file.
        total: u64,
    },
    /// What the user owns now, by product id (in-app purchases, #306).
    Entitlements(Vec<String>),
}

/// Picked files being sent to the app, for [`App::cancel_upload`]: the
/// [`UploadProgress::upload`] of [`AsyncApp::pick_files_with_progress`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Upload(pub u64);

/// How far picked files are on their way to the app (#112). On the web the
/// browser sends them over the app's connection; natively they are already
/// on the device, so no progress is reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadProgress {
    /// The upload, to cancel it ([`App::cancel_upload`]).
    pub upload: Upload,
    /// Bytes received so far, every file included.
    pub sent: u64,
    /// Bytes of every picked file.
    pub total: u64,
}

impl UploadProgress {
    /// `sent / total`, from 0 to 1 (1 for empty files).
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            1.
        } else {
            (self.sent.min(self.total) as f64 / self.total as f64) as f32
        }
    }
}

pub(crate) type StreamHandler = Rc<dyn Fn(&DeviceEvent, &mut App)>;

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
    pub(crate) fn device_call(&mut self, call: DeviceCall) -> u64 {
        self.device.next_id += 1;
        let id = self.device.next_id;
        self.surfaces
            .requests
            .push(crate::PlatformRequest::Device { id, call });
        id
    }

    /// Delivers the events of stream `id` to `handler`.
    pub(crate) fn device_stream(&mut self, id: u64, handler: StreamHandler) {
        self.device.streams.insert(id, handler);
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
                DeviceEvent::Motion(_)
                | DeviceEvent::Upload { .. }
                | DeviceEvent::Entitlements(_) => {}
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
                DeviceEvent::Location(_)
                | DeviceEvent::Upload { .. }
                | DeviceEvent::Entitlements(_) => {}
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

    /// Stops sending the picked files of `upload` (#112):
    /// [`pick_files_with_progress`](AsyncApp::pick_files_with_progress)
    /// then gives no files, and what arrived is deleted. Nothing once the
    /// files are the app's, or natively (they are already on the device).
    pub fn cancel_upload(&mut self, upload: Upload) {
        self.stop_watching(Watch(upload.0));
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

    /// Shows `path` in the system's file manager with the file selected
    /// (#210): the Finder on macOS, the desktop's file manager on Linux
    /// (`org.freedesktop.FileManager1`, else its folder opens), Explorer on
    /// Windows. Nothing on the web or mobile platforms.
    pub fn reveal_path(&mut self, path: impl Into<std::path::PathBuf>) {
        self.device_call(DeviceCall::RevealPath(path.into()));
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

    /// Like [`pick_files`](Self::pick_files), calling `progress` while the
    /// chosen files reach the app (#112): on the web the browser sends them
    /// over the app's connection, which takes a while for a large photo on
    /// a phone. The first call (`sent == 0`) comes once the user has chosen;
    /// [`App::cancel_upload`] with its [`UploadProgress::upload`] stops it,
    /// and this then answers no files. Natively the files are already on
    /// the device: no progress is reported.
    pub async fn pick_files_with_progress(
        &self,
        options: PickFiles,
        progress: impl Fn(&UploadProgress, &mut App) + 'static,
    ) -> Result<Vec<PickedFile>, DeviceError> {
        let slot: Slot = Rc::new(RefCell::new((None, None)));
        let id = self.update(|app| {
            let id = app.device_call(DeviceCall::PickFiles(options));
            app.device.pending.insert(id, slot.clone());
            app.device.streams.insert(
                id,
                Rc::new(move |e, app| {
                    if let DeviceEvent::Upload { sent, total } = *e {
                        let upload = Upload(id);
                        progress(
                            &UploadProgress {
                                upload,
                                sent,
                                total,
                            },
                            app,
                        );
                    }
                }),
            );
            id
        });
        let result = DeviceFuture { slot }.await;
        self.update(|app| app.device.streams.remove(&id));
        match result {
            Ok(DeviceValue::Files(f)) => Ok(f),
            Err(DeviceError::Cancelled) => Ok(Vec::new()),
            Err(e) => Err(e),
            Ok(other) => Err(DeviceError::Failed(format!("unexpected answer {other:?}"))),
        }
    }

    /// Lets the user keep a file the app holds (#210), with the platform's
    /// own UI: a save panel on macOS, Linux and Windows (the user picks the name and
    /// folder), "Save to Files" on iOS, iPadOS and visionOS, the storage
    /// picker (`ACTION_CREATE_DOCUMENT`) on Android, a download on the web
    /// (bytes the server holds, so no bearer header is needed). `None` if
    /// the user cancels; on the web, `Some` once the download starts.
    /// [`DeviceError::Unavailable`] on tvOS and watchOS.
    ///
    /// ```ignore
    /// let saved = cx.save_file(SaveFile::bytes(pdf, "report.pdf")).await?;
    /// if let Some(path) = saved.and_then(|s| s.path) {
    ///     cx.update(|app| app.reveal_path(path));
    /// }
    /// ```
    pub async fn save_file(&self, file: SaveFile) -> Result<Option<SavedFile>, DeviceError> {
        match self.device(DeviceCall::SaveFile(file)).await {
            Ok(DeviceValue::Files(f)) => Ok(f.into_iter().next().map(|f| SavedFile {
                path: (!f.path.is_empty()).then(|| std::path::PathBuf::from(f.path)),
                name: f.name,
            })),
            Err(DeviceError::Cancelled) => Ok(None),
            Err(e) => Err(e),
            Ok(other) => Err(DeviceError::Failed(format!("unexpected answer {other:?}"))),
        }
    }

    /// Lets the user choose a folder and gives its files with their tree
    /// (#225): an export of documents, a folder of images. `None` if they
    /// cancel. Each file's `name` is its path in the folder; read it with
    /// [`PickedFile::read`]. Natively on the desktop the files are read in
    /// place ([`PickedFolder::path`]); on iOS, iPadOS, Android and the web
    /// they are copied for the app (the web uploads them, like
    /// [`pick_files`](Self::pick_files)). `NSOpenPanel` on macOS, the Files
    /// app's folder picker on iOS, `ACTION_OPEN_DOCUMENT_TREE` on Android,
    /// GTK's folder chooser on Linux, `IFileOpenDialog` on Windows,
    /// `<input webkitdirectory>` on the web;
    /// [`DeviceError::Unavailable`] on tvOS and watchOS.
    pub async fn pick_folder(
        &self,
        options: PickFolder,
    ) -> Result<Option<PickedFolder>, DeviceError> {
        match self.device(DeviceCall::PickFolder(options)).await {
            Ok(DeviceValue::Files(f)) => Ok(PickedFolder::from_files(f)),
            Err(DeviceError::Cancelled) => Ok(None),
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
