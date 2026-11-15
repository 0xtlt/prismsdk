//! What the app runs on: device kind, OS version, screen, orientation,
//! foldable posture, browser.
//!
//! ```ignore
//! let d = cx.device_info();
//! if d.is_tablet() && d.is_landscape() { two_panes() } else { one_pane() }
//! if d.os.version.at_least(18, 0) { use_new_api() }
//! if let Some(fold) = d.fold { /* keep content off the hinge */ }
//! if let Some(b) = &d.browser { /* web: Safari 18, installed as an app… */ }
//! // Styles:
//! div().flex_col().landscape(|s| s.flex_row())
//!     .posture(Posture::Tabletop, |s| s.justify_end())
//! ```
//!
//! Everything re-renders when it changes (rotation, fold, split view);
//! `on_device_info_change` runs on every change.
//!
//! | | iOS / iPadOS | macOS | Web |
//! |---|---|---|---|
//! | kind | idiom | desktop | user agent + touch |
//! | os / model | `systemVersion`, `hw.machine` | `operatingSystemVersion`, `hw.model` | user agent (client hints when available) |
//! | inches / ppi | screen pixels and the model's ppi (table from Xcode's simulator profiles) | `CGDisplayScreenSize` | estimated |
//! | orientation | interface orientation | screen shape | `screen.orientation` |
//! | posture / fold | `UIHingeInteraction` (iOS 27.1+), division reserved regions | – | `devicePosture`, viewport segments |
//! | window mode | split view / slide over | fullscreen or windowed | standalone (installed) or browser |

use std::fmt;
use std::rc::Rc;

use crate::app::App;
use crate::platform::Platform;

/// The shape of the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum DeviceKind {
    /// Phones.
    Phone,
    /// Tablets (iPad, Android tablets).
    Tablet,
    /// Computers, laptops.
    Desktop,
    /// TVs (Apple TV, Android TV).
    Tv,
    /// Watches (Apple Watch, Wear OS).
    Watch,
    /// Headsets (Apple Vision Pro): windows in space.
    Headset,
    #[default]
    /// Not reported yet (the web before the page reports it).
    Unknown,
}

impl DeviceKind {
    /// Lowercase name (`"phone"`, `"tablet"`…), as used by the web client.
    pub fn name(self) -> &'static str {
        match self {
            DeviceKind::Phone => "phone",
            DeviceKind::Tablet => "tablet",
            DeviceKind::Desktop => "desktop",
            DeviceKind::Tv => "tv",
            DeviceKind::Watch => "watch",
            DeviceKind::Headset => "headset",
            DeviceKind::Unknown => "unknown",
        }
    }

    /// Parses [`DeviceKind::name`]; anything else is `Unknown`.
    pub fn from_name(name: &str) -> Self {
        match name {
            "phone" => DeviceKind::Phone,
            "tablet" => DeviceKind::Tablet,
            "desktop" => DeviceKind::Desktop,
            "tv" => DeviceKind::Tv,
            "watch" => DeviceKind::Watch,
            "headset" => DeviceKind::Headset,
            _ => DeviceKind::Unknown,
        }
    }
}

/// Portrait (taller than wide) or landscape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Orientation {
    #[default]
    /// Taller than wide (or square).
    Portrait,
    /// Wider than tall.
    Landscape,
}

impl Orientation {
    /// The orientation of a `width` × `height` area.
    pub fn of(width: f32, height: f32) -> Self {
        if width > height {
            Orientation::Landscape
        } else {
            Orientation::Portrait
        }
    }

    /// `"portrait"` or `"landscape"`.
    pub fn name(self) -> &'static str {
        match self {
            Orientation::Portrait => "portrait",
            Orientation::Landscape => "landscape",
        }
    }
}

/// How the device is turned, from its natural position, in degrees
/// clockwise: 0, 90, 180 or 270. Phones: 0 is upright, 180 upside down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Rotation(pub u16);

/// How a foldable is folded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Posture {
    /// Not foldable, or fully open.
    #[default]
    Flat,
    /// Half open with a vertical hinge, like a book.
    Book,
    /// Half open with a horizontal hinge, standing like a laptop: content on
    /// top, controls below.
    Tabletop,
    /// Folded shut: the app is on the outer screen.
    Closed,
}

impl Posture {
    /// Lowercase name (`"flat"`, `"book"`, `"tabletop"`, `"closed"`).
    pub fn name(self) -> &'static str {
        match self {
            Posture::Flat => "flat",
            Posture::Book => "book",
            Posture::Tabletop => "tabletop",
            Posture::Closed => "closed",
        }
    }

    /// Parses [`Posture::name`] (`"folded"` is `Closed`); anything else is `Flat`.
    pub fn from_name(name: &str) -> Self {
        match name {
            "book" => Posture::Book,
            "tabletop" => Posture::Tabletop,
            "closed" | "folded" => Posture::Closed,
            _ => Posture::Flat,
        }
    }
}

/// The hinge (or gap between two screens) of a foldable, in window
/// coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Fold {
    /// Left edge, in points.
    pub x: f32,
    /// Top edge, in points.
    pub y: f32,
    /// Width, in points.
    pub width: f32,
    /// Height, in points.
    pub height: f32,
    /// A vertical hinge splits the window into left and right panes.
    pub vertical: bool,
    /// Content under the hinge is hidden (two screens, like Surface Duo),
    /// rather than just bent (one flexible screen).
    pub occluding: bool,
}

/// How the app's window shares the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum WindowMode {
    /// The whole screen (phones, fullscreen Mac windows, installed web apps).
    #[default]
    Fullscreen,
    /// Next to another app (iPad Split View, Android split screen).
    Split,
    /// A narrow window floating over another app (iPad Slide Over).
    SlideOver,
    /// A movable window (desktop, iPad Stage Manager, a browser tab).
    Windowed,
}

impl WindowMode {
    /// Lowercase name (`"fullscreen"`, `"split"`, `"slide-over"`, `"windowed"`).
    pub fn name(self) -> &'static str {
        match self {
            WindowMode::Fullscreen => "fullscreen",
            WindowMode::Split => "split",
            WindowMode::SlideOver => "slide-over",
            WindowMode::Windowed => "windowed",
        }
    }

    /// Parses [`WindowMode::name`]; anything else is `Fullscreen`.
    pub fn from_name(name: &str) -> Self {
        match name {
            "split" => WindowMode::Split,
            "slide-over" => WindowMode::SlideOver,
            "windowed" => WindowMode::Windowed,
            _ => WindowMode::Fullscreen,
        }
    }
}

/// `major.minor.patch`, compared numerically (`18.10 > 18.9`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Version {
    /// Major version (`18` in `18.2.1`).
    pub major: u32,
    /// Minor version (`2` in `18.2.1`).
    pub minor: u32,
    /// Patch version (`1` in `18.2.1`).
    pub patch: u32,
}

impl Version {
    /// `major.minor.patch`.
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Version {
            major,
            minor,
            patch,
        }
    }

    /// `"18.2"`, `"10_15_7"`, `"131.0.6778.86"` (extra parts are ignored).
    pub fn parse(s: &str) -> Self {
        let mut parts = s
            .split(['.', '_'])
            .map(|p| {
                p.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .chain(std::iter::repeat(0));
        Version::new(
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
        )
    }

    /// `version.at_least(17, 4)`.
    pub fn at_least(self, major: u32, minor: u32) -> bool {
        self >= Version::new(major, minor, 0)
    }

    /// Whether a version was reported (`0.0.0` means unknown).
    pub fn is_known(self) -> bool {
        self != Version::default()
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.minor, self.patch) {
            (_, 0) => write!(f, "{}.{}", self.major, self.minor),
            _ => write!(f, "{}.{}.{}", self.major, self.minor, self.patch),
        }
    }
}

/// An operating system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum OsName {
    /// iOS (iPhones).
    Ios,
    /// iPads report iPadOS (same version numbers as iOS).
    Ipados,
    /// macOS.
    Macos,
    /// Android (phones, tablets, Wear OS, Android TV).
    Android,
    /// Windows.
    Windows,
    /// Linux.
    Linux,
    /// ChromeOS.
    ChromeOs,
    /// tvOS.
    Tvos,
    /// watchOS.
    Watchos,
    /// visionOS.
    Visionos,
    #[default]
    /// Not known (yet).
    Unknown,
}

impl OsName {
    /// Display name (`"iOS"`, `"macOS"`, `"ChromeOS"`…).
    pub fn name(self) -> &'static str {
        match self {
            OsName::Ios => "iOS",
            OsName::Ipados => "iPadOS",
            OsName::Macos => "macOS",
            OsName::Android => "Android",
            OsName::Windows => "Windows",
            OsName::Linux => "Linux",
            OsName::ChromeOs => "ChromeOS",
            OsName::Tvos => "tvOS",
            OsName::Watchos => "watchOS",
            OsName::Visionos => "visionOS",
            OsName::Unknown => "Unknown",
        }
    }

    /// Parses an OS name, case-insensitively (`"mac os x"`, `"xros"` too); anything else is `Unknown`.
    pub fn from_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "ios" => OsName::Ios,
            "ipados" => OsName::Ipados,
            "macos" | "mac os x" | "os x" => OsName::Macos,
            "android" => OsName::Android,
            "windows" => OsName::Windows,
            "linux" => OsName::Linux,
            "chromeos" | "chrome os" => OsName::ChromeOs,
            "tvos" => OsName::Tvos,
            "watchos" => OsName::Watchos,
            "visionos" | "xros" => OsName::Visionos,
            _ => OsName::Unknown,
        }
    }
}

/// The OS the device runs (on the web: the visitor's device, not the server).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Os {
    /// Which OS.
    pub name: OsName,
    /// Its version (`0.0.0` when unknown).
    pub version: Version,
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.version.is_known() {
            write!(f, "{} {}", self.name.name(), self.version)
        } else {
            f.write_str(self.name.name())
        }
    }
}

/// The screen the window is on.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct ScreenInfo {
    /// Points (CSS pixels on the web), in the current orientation.
    pub width: f32,
    /// Points (CSS pixels on the web), in the current orientation.
    pub height: f32,
    /// Physical pixels per point.
    pub scale: f32,
    /// Diagonal in inches, when known (or estimated on the web).
    pub diagonal: Option<f32>,
    /// Physical pixels per inch, when known.
    pub ppi: Option<f32>,
    /// Maximum refresh rate in Hz (120 on ProMotion screens).
    pub refresh_rate: Option<f32>,
    /// A notch, Dynamic Island or camera hole cuts into the screen.
    pub cutout: bool,
}

/// A browser engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Engine {
    /// Safari, and every iOS browser.
    WebKit,
    /// Chrome, Edge, Opera, Samsung Internet, Brave.
    Blink,
    /// Firefox.
    Gecko,
    #[default]
    /// Not recognized.
    Unknown,
}

impl Engine {
    /// Display name (`"WebKit"`, `"Blink"`, `"Gecko"`, `"Unknown"`).
    pub fn name(self) -> &'static str {
        match self {
            Engine::WebKit => "WebKit",
            Engine::Blink => "Blink",
            Engine::Gecko => "Gecko",
            Engine::Unknown => "Unknown",
        }
    }
}

/// The browser showing the app (web only).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Browser {
    /// `Safari`, `Chrome`, `Firefox`, `Edge`, `Samsung Internet`, `Opera`…
    pub name: String,
    /// The browser's version.
    pub version: Version,
    /// The browser's rendering engine.
    pub engine: Engine,
    /// Installed as an app (added to the home screen / dock): no browser UI.
    pub standalone: bool,
}

/// Everything about the device; see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DeviceInfo {
    /// Phone, tablet, desktop, TV, watch or headset.
    pub kind: DeviceKind,
    /// The OS and its version.
    pub os: Os,
    /// Hardware identifier (`iPhone17,1`, `Mac15,6`), when known.
    pub model: Option<String>,
    /// Marketing name (`iPhone 16 Pro`), when known.
    pub model_name: Option<String>,
    /// Running in a simulator or emulator.
    pub simulator: bool,
    /// The screen's orientation (the window's own shape is
    /// [`App::viewport`]).
    pub orientation: Orientation,
    /// How the device is turned from its natural position.
    pub rotation: Rotation,
    /// The screen the window is on.
    pub screen: ScreenInfo,
    /// How a foldable is folded ([`Posture::Flat`] otherwise).
    pub posture: Posture,
    /// How far a foldable's hinge is open, in degrees (0 closed, 180 flat),
    /// when the platform reports it.
    pub hinge_angle: Option<f32>,
    /// The hinge of a foldable, when the window spans it.
    pub fold: Option<Fold>,
    /// How the window shares the screen.
    pub window_mode: WindowMode,
    /// Primary input is touch (phones, tablets, touch screens).
    pub touch: bool,
    /// Web only: always set there.
    pub browser: Option<Browser>,
}

impl DeviceInfo {
    /// What is known before the platform reports anything.
    pub fn guess(platform: Platform, viewport: (f32, f32)) -> Self {
        let (kind, os, touch) = match platform {
            Platform::Ios => (DeviceKind::Phone, OsName::Ios, true),
            Platform::Android => (DeviceKind::Phone, OsName::Android, true),
            Platform::Macos => (DeviceKind::Desktop, OsName::Macos, false),
            Platform::Windows => (DeviceKind::Desktop, OsName::Windows, false),
            Platform::Linux => (DeviceKind::Desktop, OsName::Linux, false),
            Platform::Web => (DeviceKind::Unknown, OsName::Unknown, false),
            Platform::Tvos => (DeviceKind::Tv, OsName::Tvos, false),
            Platform::Watchos => (DeviceKind::Watch, OsName::Watchos, true),
            // Eyes and pinches: UIKit sees them as touches.
            Platform::Visionos => (DeviceKind::Headset, OsName::Visionos, true),
        };
        DeviceInfo {
            kind,
            os: Os {
                name: os,
                version: Version::default(),
            },
            orientation: Orientation::of(viewport.0, viewport.1),
            screen: ScreenInfo {
                width: viewport.0,
                height: viewport.1,
                scale: 1.,
                ..Default::default()
            },
            window_mode: if platform.is_desktop() || platform.is_web() {
                WindowMode::Windowed
            } else {
                WindowMode::Fullscreen
            },
            touch,
            browser: platform.is_web().then(Browser::default),
            ..Default::default()
        }
    }

    /// The device is a phone.
    pub fn is_phone(&self) -> bool {
        self.kind == DeviceKind::Phone
    }
    /// The device is a tablet.
    pub fn is_tablet(&self) -> bool {
        self.kind == DeviceKind::Tablet
    }
    /// The device is a computer.
    pub fn is_desktop(&self) -> bool {
        self.kind == DeviceKind::Desktop
    }
    /// The screen is wider than tall.
    pub fn is_landscape(&self) -> bool {
        self.orientation == Orientation::Landscape
    }
    /// The screen is taller than wide (or square).
    pub fn is_portrait(&self) -> bool {
        self.orientation == Orientation::Portrait
    }
    /// A foldable that is currently bent or spans a hinge.
    pub fn is_folded(&self) -> bool {
        self.posture != Posture::Flat || self.fold.is_some()
    }
    /// Running in a browser (the web backend).
    pub fn is_browser(&self) -> bool {
        self.browser.is_some()
    }

    /// `iPhone 16 Pro · iOS 18.2 · 6.3″`, for diagnostics.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(name) = self.model_name.as_ref().or(self.model.as_ref()) {
            parts.push(name.clone());
        } else {
            parts.push(self.kind.name().to_string());
        }
        parts.push(self.os.to_string());
        if let Some(b) = &self.browser {
            if b.version.is_known() {
                parts.push(format!("{} {}", b.name, b.version.major));
            } else if !b.name.is_empty() {
                parts.push(b.name.clone());
            }
        }
        if let Some(d) = self.screen.diagonal {
            parts.push(format!("{d:.1}″"));
        }
        parts.join(" · ")
    }
}

type ChangeHandler = Rc<dyn Fn(&DeviceInfo, &mut App)>;

/// Device information kept by the [`App`].
#[derive(Default)]
pub(crate) struct DeviceInfoState {
    /// `None` until the platform reports (then [`DeviceInfo::guess`]).
    pub info: Option<DeviceInfo>,
    pub handlers: Vec<ChangeHandler>,
}

impl App {
    /// The device the app runs on; see [`DeviceInfo`].
    pub fn device_info(&self) -> DeviceInfo {
        self.device_info
            .info
            .clone()
            .unwrap_or_else(|| DeviceInfo::guess(self.platform(), self.viewport()))
    }

    /// Runs `f` whenever the device information changes (rotation, fold,
    /// split view). The tree already re-renders.
    pub fn on_device_info_change(&mut self, f: impl Fn(&DeviceInfo, &mut App) + 'static) {
        self.device_info.handlers.push(Rc::new(f));
    }

    pub(crate) fn style_context(&self) -> crate::style::StyleContext {
        crate::style::StyleContext {
            breakpoint: self.breakpoint(),
            platform: Some(self.platform()),
            orientation: self.orientation(),
            posture: self.posture(),
            kind: self.device_kind(),
        }
    }

    pub(crate) fn orientation(&self) -> Orientation {
        match &self.device_info.info {
            Some(info) => info.orientation,
            None => Orientation::of(self.viewport().0, self.viewport().1),
        }
    }

    pub(crate) fn posture(&self) -> Posture {
        self.device_info
            .info
            .as_ref()
            .map_or(Posture::Flat, |i| i.posture)
    }

    pub(crate) fn device_kind(&self) -> DeviceKind {
        self.device_info
            .info
            .as_ref()
            .map_or_else(|| self.device_info().kind, |i| i.kind)
    }

    pub(crate) fn set_device_info(&mut self, info: DeviceInfo) {
        if self.device_info.info.as_ref() == Some(&info) {
            return;
        }
        self.device_info.info = Some(info.clone());
        self.refresh();
        for handler in self.device_info.handlers.clone() {
            handler(&info, self);
        }
    }
}

// --- User agents ------------------------------------------------------------------------

/// Device information from a browser's `User-Agent` (the web server's first
/// guess, refined by the page's report). `touch` is `navigator.maxTouchPoints > 0`
/// when known: iPads ask for desktop sites and look like Macs otherwise.
pub fn parse_user_agent(ua: &str, touch: Option<bool>) -> DeviceInfo {
    let has = |s: &str| ua.contains(s);
    let after = |marker: &str| -> Option<&str> {
        let i = ua.find(marker)? + marker.len();
        let rest = &ua[i..];
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '_'))
            .unwrap_or(rest.len());
        Some(&rest[..end]).filter(|s| !s.is_empty())
    };
    let version = |marker: &str| after(marker).map(Version::parse).unwrap_or_default();

    // OS and kind.
    let mac_touch = has("Macintosh") && touch == Some(true);
    let (os, mut kind) = if has("iPad") || mac_touch {
        // iPadOS 13+ sends a Mac user agent with its Safari version.
        let v = if has("iPad") {
            version("OS ")
        } else {
            version("Version/")
        };
        (
            Os {
                name: OsName::Ipados,
                version: v,
            },
            DeviceKind::Tablet,
        )
    } else if has("iPhone") || has("iPod") {
        (
            Os {
                name: OsName::Ios,
                version: version("OS "),
            },
            DeviceKind::Phone,
        )
    } else if has("Android") {
        let kind = if has("Mobile") {
            DeviceKind::Phone
        } else {
            DeviceKind::Tablet
        };
        (
            Os {
                name: OsName::Android,
                version: version("Android "),
            },
            kind,
        )
    } else if has("CrOS") {
        (
            Os {
                name: OsName::ChromeOs,
                version: Version::default(),
            },
            DeviceKind::Desktop,
        )
    } else if has("Macintosh") || has("Mac OS X") {
        (
            Os {
                name: OsName::Macos,
                version: version("Mac OS X "),
            },
            DeviceKind::Desktop,
        )
    } else if has("Windows") {
        (
            Os {
                name: OsName::Windows,
                version: version("Windows NT "),
            },
            DeviceKind::Desktop,
        )
    } else if has("Linux") {
        (
            Os {
                name: OsName::Linux,
                version: Version::default(),
            },
            DeviceKind::Desktop,
        )
    } else {
        (Os::default(), DeviceKind::Unknown)
    };
    if has("SmartTV") || has("SMART-TV") || has("AppleTV") || has("CrKey") {
        kind = DeviceKind::Tv;
    }
    let ios = matches!(os.name, OsName::Ios | OsName::Ipados);

    // Browser: the most specific marker wins (Edge and Opera also say Chrome).
    let (name, v, engine) = if let Some(v) = after("EdgiOS/").or(after("Edg/")).or(after("EdgA/")) {
        ("Edge", v, Engine::Blink)
    } else if let Some(v) = after("OPR/").or(after("OPT/")) {
        ("Opera", v, Engine::Blink)
    } else if let Some(v) = after("SamsungBrowser/") {
        ("Samsung Internet", v, Engine::Blink)
    } else if let Some(v) = after("FxiOS/").or(after("Firefox/")) {
        ("Firefox", v, Engine::Gecko)
    } else if let Some(v) = after("CriOS/").or(after("Chrome/")) {
        ("Chrome", v, Engine::Blink)
    } else if let Some(v) = after("Version/").filter(|_| has("Safari")) {
        ("Safari", v, Engine::WebKit)
    } else if has("AppleWebKit") {
        ("WebKit", "", Engine::WebKit)
    } else {
        ("", "", Engine::Unknown)
    };
    // Every browser on iOS uses WebKit.
    let engine = if ios { Engine::WebKit } else { engine };

    let touch = touch.unwrap_or(matches!(kind, DeviceKind::Phone | DeviceKind::Tablet));
    DeviceInfo {
        kind,
        os,
        model: (os.name == OsName::Android)
            .then(|| android_model(ua))
            .flatten(),
        touch,
        window_mode: WindowMode::Windowed,
        browser: Some(Browser {
            name: name.into(),
            version: Version::parse(v),
            engine,
            standalone: false,
        }),
        screen: ScreenInfo {
            scale: 1.,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// `Pixel 8` from `Linux; Android 14; Pixel 8)`.
fn android_model(ua: &str) -> Option<String> {
    let start = ua.find("Android")?;
    let inside = &ua[start..ua[start..].find(')').map(|e| start + e)?];
    let model = inside.split(';').nth(1)?.trim();
    let model = model.split(" Build/").next().unwrap_or(model).trim();
    (!model.is_empty() && model != "K" && model != "wv").then(|| model.to_string())
}

/// Estimated diagonal in inches for a screen of `width`×`height` CSS pixels
/// (CSS defines 96 px per inch; phones and tablets use about 160 and 132).
pub fn estimate_diagonal(kind: DeviceKind, width: f32, height: f32) -> Option<f32> {
    let per_inch = match kind {
        DeviceKind::Phone => 160.,
        DeviceKind::Tablet => 132.,
        DeviceKind::Desktop => 110.,
        _ => return None,
    };
    let d = (width * width + height * height).sqrt() / per_inch;
    (d > 0.).then_some((d * 10.).round() / 10.)
}

/// An Apple device, from the table generated out of Xcode's simulator
/// profiles (`scripts/apple-models.py`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppleModel {
    /// `iPhone 16 Pro`, `iPhone Duo`.
    pub name: &'static str,
    /// Built-in displays: (width px, height px, pixels per inch). Foldables
    /// have more than one.
    pub screens: &'static [(u32, u32, f32)],
}

impl AppleModel {
    /// The device has more than one built-in display (a foldable).
    pub fn is_foldable(&self) -> bool {
        self.screens.len() > 1
    }

    /// Whether display `index` is the smallest (a foldable's outer screen).
    pub fn is_smallest(&self, index: usize) -> bool {
        let area = |&(w, h, _): &(u32, u32, f32)| u64::from(w) * u64::from(h);
        let min = self.screens.iter().map(area).min();
        self.screens.get(index).map(area) == min
    }

    /// The built-in display of `width`×`height` pixels (either orientation):
    /// its index and pixels per inch.
    pub fn screen(&self, width: u32, height: u32) -> Option<(usize, f32)> {
        let (a, b) = (width.min(height), width.max(height));
        self.screens
            .iter()
            .position(|&(w, h, _)| (w.min(h), w.max(h)) == (a, b))
            .map(|i| (i, self.screens[i].2))
    }
}

/// Looks up an Apple model id (`iPhone17,1`). Simulators report the
/// simulated model.
pub fn apple_model(id: &str) -> Option<AppleModel> {
    crate::apple_models::MODELS
        .iter()
        .find(|(m, _, _)| *m == id)
        .map(|&(_, name, screens)| AppleModel { name, screens })
}

/// The diagonal in inches of `width`×`height` pixels at `ppi`, rounded to a tenth.
pub fn diagonal_inches(width: f32, height: f32, ppi: f32) -> Option<f32> {
    let d = (width * width + height * height).sqrt() / ppi;
    (ppi > 0. && d > 0.).then_some((d * 10.).round() / 10.)
}
