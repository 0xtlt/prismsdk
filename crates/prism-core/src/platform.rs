//! Platform-specific UI: show something only on some platforms, or style it
//! differently there.
//!
//! ```ignore
//! text("Swipe left to delete").only(Platform::MOBILE)
//! text("Right-click for more").except(Platform::MOBILE)
//! div().p(16.).on(Platform::Web, |s| s.p(24.))
//! match cx.platform() { Platform::Ios => …, Platform::Web => …, _ => … }
//! ```
//!
//! Excluded elements are left out of the tree (no hidden view is created).
//! For code that only *compiles* on one platform, use `cfg`:
//! `#[cfg(target_os = "ios")]`, `#[cfg(feature = "web")]` (your app's feature).

use std::ops::BitOr;

/// Where the app runs. The web backend reports `Web` whatever the server's OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    Ios,
    Android,
    Macos,
    Windows,
    Linux,
    Web,
    Tvos,
    Watchos,
    /// Apple Vision Pro.
    Visionos,
}

impl Platform {
    pub const MOBILE: Platforms =
        Platforms(1 << Platform::Ios as u16 | 1 << Platform::Android as u16);
    pub const DESKTOP: Platforms = Platforms(
        1 << Platform::Macos as u16 | 1 << Platform::Windows as u16 | 1 << Platform::Linux as u16,
    );
    pub const APPLE: Platforms = Platforms(
        1 << Platform::Ios as u16
            | 1 << Platform::Macos as u16
            | 1 << Platform::Tvos as u16
            | 1 << Platform::Watchos as u16
            | 1 << Platform::Visionos as u16,
    );
    /// TVs (a remote, no touch): tvOS.
    pub const TV: Platforms = Platforms(1 << Platform::Tvos as u16);
    /// Watches: watchOS.
    pub const WEARABLE: Platforms = Platforms(1 << Platform::Watchos as u16);
    /// Headsets: visionOS.
    pub const SPATIAL: Platforms = Platforms(1 << Platform::Visionos as u16);
    pub const ALL: Platforms = Platforms((1 << 9) - 1);
    /// Everything but the web.
    pub const NATIVE: Platforms = Platforms(Platform::ALL.0 & !(1 << Platform::Web as u16));

    /// The platform this binary was compiled for (native backends).
    pub const fn compiled() -> Platform {
        if cfg!(target_os = "visionos") {
            Platform::Visionos
        } else if cfg!(target_os = "tvos") {
            Platform::Tvos
        } else if cfg!(target_os = "watchos") {
            Platform::Watchos
        } else if cfg!(target_os = "ios") {
            Platform::Ios
        } else if cfg!(target_os = "android") {
            Platform::Android
        } else if cfg!(target_os = "macos") {
            Platform::Macos
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }

    pub fn is_mobile(self) -> bool {
        Platform::MOBILE.contains(self)
    }
    pub fn is_desktop(self) -> bool {
        Platform::DESKTOP.contains(self)
    }
    pub fn is_web(self) -> bool {
        self == Platform::Web
    }
    pub fn is_apple(self) -> bool {
        Platform::APPLE.contains(self)
    }

    pub fn name(self) -> &'static str {
        match self {
            Platform::Ios => "ios",
            Platform::Android => "android",
            Platform::Macos => "macos",
            Platform::Windows => "windows",
            Platform::Linux => "linux",
            Platform::Web => "web",
            Platform::Tvos => "tvos",
            Platform::Watchos => "watchos",
            Platform::Visionos => "visionos",
        }
    }
}

/// A set of platforms: `Platform::Ios | Platform::Web`, `Platform::MOBILE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Platforms(u16);

impl Platforms {
    pub fn contains(self, p: Platform) -> bool {
        self.0 & (1 << p as u16) != 0
    }
    /// Every platform except these.
    pub fn complement(self) -> Platforms {
        Platforms(!self.0 & Platform::ALL.0)
    }
}

impl From<Platform> for Platforms {
    fn from(p: Platform) -> Self {
        Platforms(1 << p as u16)
    }
}

impl BitOr for Platform {
    type Output = Platforms;
    fn bitor(self, rhs: Platform) -> Platforms {
        Platforms::from(self) | Platforms::from(rhs)
    }
}

impl BitOr<Platform> for Platforms {
    type Output = Platforms;
    fn bitor(self, rhs: Platform) -> Platforms {
        self | Platforms::from(rhs)
    }
}

impl BitOr for Platforms {
    type Output = Platforms;
    fn bitor(self, rhs: Platforms) -> Platforms {
        Platforms(self.0 | rhs.0)
    }
}

impl crate::App {
    /// Where the app runs (`Web` for the web backend).
    pub fn platform(&self) -> Platform {
        self.window.platform
    }
}
