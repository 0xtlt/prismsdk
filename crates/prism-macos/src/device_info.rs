//! `cx.device_info()` on macOS: OS version, Mac model, the window's screen
//! (physical size from `CGDisplayScreenSize`, refresh rate, notch),
//! fullscreen.

use objc2::MainThreadOnly;
use objc2_app_kit::{NSScreen, NSWindow, NSWindowStyleMask};
use objc2_foundation::{NSNumber, NSString};
use prism_core::{DeviceInfo, DeviceKind, Orientation, Os, OsName, ScreenInfo, WindowMode};

#[repr(C)]
struct CGSize {
    width: f64,
    height: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    /// Millimetres; zero if unknown (some projectors, virtual displays).
    fn CGDisplayScreenSize(display: u32) -> CGSize;
}

/// The physical diagonal of `screen` in inches.
fn diagonal(screen: &NSScreen) -> Option<f32> {
    let description = screen.deviceDescription();
    let number = description.objectForKey(&NSString::from_str("NSScreenNumber"))?;
    let id = number.downcast::<NSNumber>().ok()?.unsignedIntValue();
    // SAFETY: plain C call on a display id.
    let mm = unsafe { CGDisplayScreenSize(id) };
    let d = (mm.width * mm.width + mm.height * mm.height).sqrt() / 25.4;
    (d > 1.).then(|| ((d * 10.).round() / 10.) as f32)
}

/// The device as seen from `window` (its current screen).
pub(crate) fn current(window: &NSWindow) -> DeviceInfo {
    let screen = window
        .screen()
        .or_else(|| NSScreen::mainScreen(window.mtm()));
    let mut info = DeviceInfo {
        kind: DeviceKind::Desktop,
        os: Os {
            name: OsName::Macos,
            version: prism_native::apple::os_version(),
        },
        model: prism_native::apple::model_id(),
        window_mode: if window.styleMask().contains(NSWindowStyleMask::FullScreen) {
            WindowMode::Fullscreen
        } else {
            WindowMode::Windowed
        },
        ..Default::default()
    };
    if let Some(screen) = screen {
        let size = screen.frame().size;
        let (w, h) = (size.width as f32, size.height as f32);
        let scale = screen.backingScaleFactor() as f32;
        let diagonal = diagonal(&screen);
        let pixels = ((w * w + h * h).sqrt()) * scale;
        info.orientation = Orientation::of(w, h);
        info.screen = ScreenInfo {
            width: w,
            height: h,
            scale,
            diagonal,
            ppi: diagonal.map(|d| (pixels / d).round()),
            refresh_rate: Some(screen.maximumFramesPerSecond() as f32),
            // The MacBook notch.
            cutout: screen.safeAreaInsets().top > 0.,
        };
    }
    info
}
