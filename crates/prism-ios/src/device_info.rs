//! `cx.device_info()` on iOS and iPadOS: idiom, OS version, model (inches
//! from a model table), screen, interface orientation, Split View.

use std::sync::OnceLock;

use objc2::MainThreadMarker;
use objc2_ui_kit::{UIDevice, UIInterfaceOrientation, UIUserInterfaceIdiom, UIView};
use prism_core::{
    DeviceInfo, DeviceKind, Orientation, Os, OsName, Rotation, ScreenInfo, WindowMode,
};

/// What never changes while the app runs.
struct Fixed {
    kind: DeviceKind,
    os: Os,
    model: Option<String>,
    model_name: Option<String>,
    diagonal: Option<f32>,
    simulator: bool,
}

fn fixed(mtm: MainThreadMarker) -> &'static Fixed {
    static FIXED: OnceLock<Fixed> = OnceLock::new();
    FIXED.get_or_init(|| {
        let device = UIDevice::currentDevice(mtm);
        let kind = match device.userInterfaceIdiom() {
            UIUserInterfaceIdiom::Phone => DeviceKind::Phone,
            UIUserInterfaceIdiom::Pad => DeviceKind::Tablet,
            UIUserInterfaceIdiom::TV => DeviceKind::Tv,
            UIUserInterfaceIdiom::Mac => DeviceKind::Desktop,
            _ => DeviceKind::Unknown,
        };
        let model = prism_native::apple::model_id();
        let known = model
            .as_deref()
            .and_then(prism_core::device_info::apple_model);
        Fixed {
            kind,
            os: Os {
                name: if kind == DeviceKind::Tablet {
                    OsName::Ipados
                } else {
                    OsName::Ios
                },
                version: prism_native::apple::os_version(),
            },
            model_name: known.map(|(name, _)| name.to_string()),
            diagonal: known.map(|(_, inches)| inches),
            model,
            simulator: prism_native::apple::is_simulator(),
        }
    })
}

/// The device as seen from `root` (the window's root view), or `None`
/// before it is in a window.
pub(crate) fn current(root: &UIView, mtm: MainThreadMarker) -> Option<DeviceInfo> {
    let window = root.window()?;
    let scene = window.windowScene()?;
    let screen = scene.screen();
    let f = fixed(mtm);

    // Points in the current orientation.
    let bounds = screen.bounds().size;
    let (sw, sh) = (bounds.width as f32, bounds.height as f32);
    let native = screen.nativeBounds().size;
    let pixels = ((native.width * native.width + native.height * native.height) as f32).sqrt();
    let rotation = match scene.effectiveGeometry().interfaceOrientation() {
        UIInterfaceOrientation::PortraitUpsideDown => 180,
        // Home indicator on the left: the device turned clockwise.
        UIInterfaceOrientation::LandscapeLeft => 90,
        UIInterfaceOrientation::LandscapeRight => 270,
        _ => 0,
    };

    let win = window.bounds().size;
    let (ww, wh) = (win.width as f32, win.height as f32);
    let window_mode = if f.kind != DeviceKind::Tablet {
        WindowMode::Fullscreen
    } else if wh + 1. < sh {
        // Stage Manager, or the iPadOS 26 windows.
        WindowMode::Windowed
    } else if ww + 1. < sw {
        // Full height but narrower: Split View or Slide Over.
        if ww <= 400. && ww * 2.5 < sw {
            WindowMode::SlideOver
        } else {
            WindowMode::Split
        }
    } else {
        WindowMode::Fullscreen
    };

    // Phones with a notch or Dynamic Island have a tall top (or side) inset.
    let insets = window.safeAreaInsets();
    let cutout = f.kind == DeviceKind::Phone && insets.top.max(insets.left).max(insets.right) > 24.;

    Some(DeviceInfo {
        kind: f.kind,
        os: f.os,
        model: f.model.clone(),
        model_name: f.model_name.clone(),
        simulator: f.simulator,
        orientation: Orientation::of(sw, sh),
        rotation: Rotation(rotation),
        screen: ScreenInfo {
            width: sw,
            height: sh,
            scale: screen.scale() as f32,
            diagonal: f.diagonal,
            ppi: f.diagonal.map(|d| (pixels / d).round()),
            refresh_rate: Some(screen.maximumFramesPerSecond() as f32),
            cutout,
        },
        window_mode,
        touch: true,
        ..Default::default()
    })
}
