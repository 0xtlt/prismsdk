//! `cx.device_info()` on iOS and iPadOS: idiom, OS version, model, screen
//! (inches from the screen's pixels and the model's ppi), interface
//! orientation, Split View, and foldables (iOS 27.1+): hinge state from
//! `UIHingeInteraction`, hinge position from the view's division reserved
//! regions.

use std::cell::Cell;
use std::sync::OnceLock;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_core_foundation::CGRect;
use objc2_foundation::{NSArray, NSObjectProtocol};
#[cfg(not(target_os = "tvos"))]
use objc2_ui_kit::UIInterfaceOrientation;
use objc2_ui_kit::{UIDevice, UIUserInterfaceIdiom, UIView};
use prism_core::device_info::{AppleModel, apple_model, diagonal_inches};
use prism_core::{
    DeviceInfo, DeviceKind, Fold, Orientation, Os, OsName, Posture, Rotation, ScreenInfo,
    WindowMode,
};

/// What never changes while the app runs.
struct Fixed {
    kind: DeviceKind,
    os: Os,
    model: Option<String>,
    known: Option<AppleModel>,
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
        Fixed {
            kind,
            os: Os {
                name: match kind {
                    DeviceKind::Tablet => OsName::Ipados,
                    DeviceKind::Tv => OsName::Tvos,
                    _ => OsName::Ios,
                },
                version: prism_native::apple::os_version(),
            },
            known: model.as_deref().and_then(apple_model),
            model,
            simulator: prism_native::apple::is_simulator(),
        }
    })
}

// --- Hinge (iOS 27.1+) -------------------------------------------------------------------

/// `UIHingeStatus` (UIHinge.h).
const HINGE_CLOSED: isize = 1;
const HINGE_PARTIALLY_OPEN: isize = 2;
const HINGE_FULLY_OPEN: isize = 3;

thread_local! {
    /// The last hinge update: status and angle in radians.
    static HINGE: Cell<Option<(isize, f64)>> = const { Cell::new(None) };
    static OBSERVING: Cell<bool> = const { Cell::new(false) };
}

/// Observes the hinge of a foldable from `root`; `changed` runs on each
/// update. Does nothing before iOS 27.1.
pub(crate) fn observe_hinge(root: &UIView, changed: fn()) {
    if OBSERVING.get() {
        return;
    }
    let Some(class) = AnyClass::get(c"UIHingeInteraction") else {
        return;
    };
    OBSERVING.set(true);
    let handler = RcBlock::new(
        move |_interaction: *mut AnyObject, update: *mut AnyObject| {
            // SAFETY: UIKit passes a live UIHingeInteractionUpdate; `hinge` is a
            // nullable UIHinge with `status` (NSInteger) and `angle` (CGFloat).
            let state = unsafe {
                let hinge: Option<Retained<AnyObject>> = msg_send![update, hinge];
                hinge.map(|h| {
                    let status: isize = msg_send![&*h, status];
                    let angle: f64 = msg_send![&*h, angle];
                    (status, angle)
                })
            };
            HINGE.set(state);
            changed();
        },
    );
    // SAFETY: documented initializer and UIView's `addInteraction:`.
    unsafe {
        let interaction: Retained<AnyObject> =
            msg_send![msg_send![class, alloc], initWithUpdateHandler: &*handler];
        let _: () = msg_send![root, addInteraction: &*interaction];
    }
}

/// Active division regions of `view` (where content should split in two),
/// in its coordinates.
fn division_regions(view: &UIView) -> Vec<CGRect> {
    let Some(kind_class) = AnyClass::get(c"UIViewReservedRegionKind") else {
        return Vec::new();
    };
    if !view.respondsToSelector(sel!(reservedRegionsOfKind:)) {
        return Vec::new();
    }
    // SAFETY: iOS 27.1 API (UIViewReservedRegion.h, UIView.h), checked above.
    unsafe {
        let kind: Retained<AnyObject> = msg_send![kind_class, divisionRegionKind];
        let regions: Retained<NSArray<AnyObject>> = msg_send![view, reservedRegionsOfKind: &*kind];
        regions
            .iter()
            .filter(|r| {
                let active: bool = msg_send![&**r, isActive];
                active
            })
            .map(|r| {
                let frame: CGRect = msg_send![&*r, frame];
                frame
            })
            .collect()
    }
}

// --- Device info -------------------------------------------------------------------------

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
    let (pw, ph) = (native.width as f32, native.height as f32);
    // Which built-in display shows the app (foldables have two) and its ppi.
    let display = f
        .known
        .and_then(|m| m.screen(pw.round() as u32, ph.round() as u32));
    let ppi = display.map(|(_, ppi)| ppi);
    let diagonal = ppi.and_then(|ppi| diagonal_inches(pw, ph, ppi));

    // UIOrientation.h: interface LandscapeLeft = device LandscapeRight (home
    // button on the left), i.e. the device turned clockwise.
    // (tvOS: no interface orientation, UIWindowScene.h.)
    #[cfg(target_os = "tvos")]
    let rotation = 0;
    #[cfg(not(target_os = "tvos"))]
    let rotation = match scene.effectiveGeometry().interfaceOrientation() {
        UIInterfaceOrientation::PortraitUpsideDown => 180,
        UIInterfaceOrientation::LandscapeLeft => 90,
        UIInterfaceOrientation::LandscapeRight => 270,
        _ => 0,
    };

    let win = window.bounds().size;
    let (ww, wh) = (win.width as f32, win.height as f32);
    let window_mode = if f.kind != DeviceKind::Tablet {
        WindowMode::Fullscreen
    } else if wh + 1. < sh {
        // Shorter than the screen: a movable window.
        WindowMode::Windowed
    } else if ww + 1. < sw {
        // Full height but narrower: next to another app.
        WindowMode::Split
    } else {
        WindowMode::Fullscreen
    };

    // A notch or camera cutout makes the top (or side) safe area taller than
    // a plain status bar.
    let insets = window.safeAreaInsets();
    let cutout = f.kind == DeviceKind::Phone && insets.top.max(insets.left).max(insets.right) > 24.;

    // Foldables: the hinge region in window coordinates, and its state.
    let fold = division_regions(root).first().map(|r| {
        let vertical = r.size.height > r.size.width;
        Fold {
            x: r.origin.x as f32,
            y: r.origin.y as f32,
            width: r.size.width as f32,
            height: r.size.height as f32,
            vertical,
            occluding: false,
        }
    });
    let hinge = HINGE.get();
    let foldable = f.known.is_some_and(|m| m.is_foldable());
    let posture = match hinge.map(|(status, _)| status) {
        Some(HINGE_CLOSED) => Posture::Closed,
        Some(HINGE_PARTIALLY_OPEN) => match fold {
            Some(fold) if !fold.vertical => Posture::Tabletop,
            _ => Posture::Book,
        },
        Some(HINGE_FULLY_OPEN) => Posture::Flat,
        // No hinge report: a foldable showing its smallest display is closed.
        _ if foldable
            && display.is_some_and(|(i, _)| f.known.is_some_and(|m| m.is_smallest(i))) =>
        {
            Posture::Closed
        }
        _ => Posture::Flat,
    };

    Some(DeviceInfo {
        kind: f.kind,
        os: f.os,
        model: f.model.clone(),
        model_name: f.known.map(|m| m.name.to_string()),
        simulator: f.simulator,
        orientation: Orientation::of(sw, sh),
        rotation: Rotation(rotation),
        screen: ScreenInfo {
            width: sw,
            height: sh,
            scale: screen.scale() as f32,
            diagonal,
            ppi,
            refresh_rate: Some(screen.maximumFramesPerSecond() as f32),
            cutout,
        },
        posture,
        hinge_angle: hinge.map(|(_, angle)| angle.to_degrees() as f32),
        fold,
        window_mode,
        touch: f.kind != DeviceKind::Tv,
        ..Default::default()
    })
}
