//! Custom window chrome (#67): no title bar, the content from the window's
//! top edge, the traffic lights where the app wants them, and the app's own
//! title bar (`.window_drag()`) moving and zooming the window, as does the
//! empty part of a tab bar or navigation header along the top edge.
//!
//! AppKit lays the title bar out again on resizes, full screen and
//! appearance changes, putting the buttons back in their place: the window
//! delegate calls [`place_traffic_lights`] after each of them.

use std::cell::Cell;

use objc2_app_kit::{NSView, NSWindow, NSWindowButton, NSWindowStyleMask, NSWindowTitleVisibility};
use objc2_foundation::{NSPoint, NSRect, NSString, NSUserDefaults};
use prism_core::{WindowChrome, WindowControls};

thread_local! {
    /// The main window's chrome (`Application::window_chrome`).
    static CHROME: Cell<WindowChrome> = const { Cell::new(WindowChrome::Standard) };
    /// The window is in (or entering) full screen.
    static FULLSCREEN: Cell<bool> = const { Cell::new(false) };
}

const BUTTONS: [NSWindowButton; 3] = [
    NSWindowButton::CloseButton,
    NSWindowButton::MiniaturizeButton,
    NSWindowButton::ZoomButton,
];

/// The main window's chrome.
pub(crate) fn chrome() -> WindowChrome {
    CHROME.with(Cell::get)
}

/// The main window's style: a custom chrome's content view covers the
/// title bar.
pub(crate) fn style_mask(chrome: WindowChrome, style: NSWindowStyleMask) -> NSWindowStyleMask {
    if chrome.is_custom() {
        style | NSWindowStyleMask::FullSizeContentView
    } else {
        style
    }
}

/// Removes the title bar of `window` (created with [`style_mask`]) for a
/// custom chrome: a transparent, untitled title bar (the title still names
/// the window in Mission Control, the Window menu and for VoiceOver).
pub(crate) fn setup(window: &NSWindow, chrome: WindowChrome) {
    CHROME.with(|c| c.set(chrome));
    if !chrome.is_custom() {
        return;
    }
    window.setTitlebarAppearsTransparent(true);
    window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    place_traffic_lights(window);
}

/// The window entered or left full screen (`will` notifications: the app's
/// title bar can make or drop room for the buttons during the animation).
pub(crate) fn set_fullscreen(on: bool) {
    FULLSCREEN.with(|f| f.set(on));
}

pub(crate) fn is_fullscreen() -> bool {
    FULLSCREEN.with(Cell::get)
}

/// Moves the traffic lights to `traffic_lights`: the title bar container
/// grows (or shrinks) to keep them inside it, centered vertically, so they
/// still respond to the pointer.
pub(crate) fn place_traffic_lights(window: &NSWindow) {
    let WindowChrome::Custom {
        traffic_lights: Some(at),
    } = chrome()
    else {
        return;
    };
    if is_fullscreen() {
        return;
    }
    let buttons: Vec<_> = BUTTONS
        .iter()
        .filter_map(|b| window.standardWindowButton(*b))
        .collect();
    let [close, mini, ..] = buttons.as_slice() else {
        return;
    };
    // The buttons sit in the title bar view, inside its container.
    let Some(container) = (unsafe { close.superview() }).and_then(|v| unsafe { v.superview() })
    else {
        return;
    };
    let height = close.frame().size.height;
    let spacing = mini.frame().origin.x - close.frame().origin.x;
    let bar = (at.y as f64) * 2. + height;
    let window_height = window.frame().size.height;
    let mut frame = container.frame();
    frame.size.height = bar;
    frame.origin.y = window_height - bar;
    container.setFrame(frame);
    for (i, button) in buttons.iter().enumerate() {
        // The title bar view is not flipped: `y` from its bottom edge.
        button.setFrameOrigin(NSPoint::new(
            at.x as f64 + spacing * i as f64,
            bar - at.y as f64 - height,
        ));
    }
}

/// Where the traffic lights are, from the window's top-left corner: `None`
/// for a standard chrome or in full screen.
pub(crate) fn controls(window: &NSWindow) -> Option<WindowControls> {
    if !chrome().is_custom() || is_fullscreen() {
        return None;
    }
    let content = window.contentView()?;
    let mut union: Option<NSRect> = None;
    for button in BUTTONS
        .iter()
        .filter_map(|b| window.standardWindowButton(*b))
    {
        if button.isHidden() {
            continue;
        }
        let r = button.convertRect_toView(button.bounds(), None::<&NSView>);
        union = Some(match union {
            None => r,
            Some(u) => {
                let x = u.origin.x.min(r.origin.x);
                let y = u.origin.y.min(r.origin.y);
                let right = (u.origin.x + u.size.width).max(r.origin.x + r.size.width);
                let top = (u.origin.y + u.size.height).max(r.origin.y + r.size.height);
                NSRect::new(
                    NSPoint::new(x, y),
                    objc2_foundation::NSSize::new(right - x, top - y),
                )
            }
        });
    }
    let r = union?;
    // Window coordinates grow upwards; the content view covers the window.
    let top = content.frame().size.height - (r.origin.y + r.size.height);
    Some(WindowControls {
        x: r.origin.x as f32,
        y: top as f32,
        width: r.size.width as f32,
        height: r.size.height as f32,
    })
}

/// Whether `view` (a tab bar's or a navigation header's bar) runs along
/// the top edge of a window with a custom chrome: its empty part then moves
/// the window, as Windows' top bar does (Safari's and Finder's tab bars
/// too). Its segments and buttons keep their clicks.
pub(crate) fn along_top(view: &NSView) -> bool {
    if !chrome().is_custom() || is_fullscreen() {
        return false;
    }
    let Some(window) = view.window() else {
        return false;
    };
    // The main window's (sheets keep their title bar).
    if !window
        .styleMask()
        .contains(NSWindowStyleMask::FullSizeContentView)
    {
        return false;
    }
    let Some(content) = window.contentView() else {
        return false;
    };
    // Window coordinates grow upwards.
    let r = view.convertRect_toView(view.bounds(), None::<&NSView>);
    (r.origin.y + r.size.height - content.frame().size.height).abs() < 0.5
}

/// A double click on a `.window_drag()` region: what System Settings ›
/// Desktop & Dock › « Double-click a window's title bar » says.
pub(crate) fn double_click(window: &NSWindow) {
    let defaults = NSUserDefaults::standardUserDefaults();
    let action = defaults
        .stringForKey(&NSString::from_str("AppleActionOnDoubleClick"))
        .map(|s| s.to_string());
    match action.as_deref() {
        Some("Minimize") => window.miniaturize(None),
        Some("None") => {}
        // "Maximize", "Fill" (no public API: zoom), unset.
        Some(_) => window.zoom(None),
        None => {
            // Before macOS 10.11: a boolean.
            if defaults.boolForKey(&NSString::from_str("AppleMiniaturizeOnDoubleClick")) {
                window.miniaturize(None)
            } else {
                window.zoom(None)
            }
        }
    }
}
