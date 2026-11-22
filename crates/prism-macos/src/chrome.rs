//! Custom window chrome (#67): no title bar, the content from the window's
//! top edge, the traffic lights where the app wants them, and the app's own
//! title bar (`.window_drag()`) moving and zooming the window, as does the
//! empty part of a tab bar or navigation header along the top edge.
//!
//! AppKit lays the title bar out again on resizes, full screen and
//! appearance changes, putting the buttons back in their place: the window
//! delegate calls [`place_traffic_lights`] after each of them.
//!
//! The title bar strip stays the app's: AppKit gives the presses there to
//! the (invisible) title bar, so the main window, a [`PrismWindow`], sends
//! them to the app's view under them itself ([`route_strip_press`]).

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSControl, NSEvent, NSEventType, NSImage, NSResponder, NSView, NSWindow,
    NSWindowButton, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSString, NSUserDefaults};
use prism_core::{WindowChrome, WindowControls};

thread_local! {
    /// The main window's chrome (`Application::window_chrome`).
    static CHROME: Cell<WindowChrome> = const { Cell::new(WindowChrome::Standard) };
    /// The window is in (or entering) full screen.
    static FULLSCREEN: Cell<bool> = const { Cell::new(false) };
    /// The view a press in the title bar strip went to: the rest of that
    /// press (drags, the release) goes to it too.
    static PRESSED: RefCell<Option<Retained<NSView>>> = const { RefCell::new(None) };
}

define_class!(
    /// The main window: an `NSWindow` whose custom chrome's title bar strip
    /// sends its presses to the app's views ([`route_strip_press`]).
    #[unsafe(super(NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismWindow"]
    pub(crate) struct PrismWindow;

    impl PrismWindow {
        #[unsafe(method(sendEvent:))]
        fn send_event(&self, event: &NSEvent) {
            if !route_strip_press(self, event) {
                unsafe { msg_send![super(self), sendEvent: event] }
            }
        }
    }
);

impl PrismWindow {
    /// A main window (`initWithContentRect:styleMask:backing:defer:`).
    pub(crate) fn create(
        mtm: MainThreadMarker,
        frame: NSRect,
        style: NSWindowStyleMask,
    ) -> Retained<NSWindow> {
        let window: Retained<PrismWindow> = unsafe {
            msg_send![
                PrismWindow::alloc(mtm),
                initWithContentRect: frame,
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: false
            ]
        };
        Retained::into_super(window)
    }
}

/// With a custom chrome, AppKit gives a press in the title bar strip (the
/// top 28 points, over the content) to the invisible title bar, which
/// moves or zooms the window, unless one of its controls is under it: the
/// app's views there (a `.window_drag()` bar's text and empty part,
/// clickable views) never got their presses, and a double click did what
/// the title bar does rather than what the app's bar does. So `sendEvent:`
/// sends them to the view under them, as AppKit does below the strip: it
/// makes the window key (the press goes on only if the view takes the
/// first mouse), gives the view the focus if it takes it, and sends it
/// `mouseDown:`, then that press's drags and release (`rightMouseDown:`
/// for a right press). AppKit's controls (Back, bar buttons, tabs), the
/// traffic lights, and the title bar where no view of the app is keep
/// AppKit's own handling; full screen has no strip. Returns whether the
/// event was handled.
fn route_strip_press(window: &NSWindow, event: &NSEvent) -> bool {
    let kind = event.r#type();
    if kind == NSEventType::LeftMouseDragged || kind == NSEventType::LeftMouseUp {
        let up = kind == NSEventType::LeftMouseUp;
        let Some(view) = PRESSED.with(|p| {
            if up {
                p.borrow_mut().take()
            } else {
                p.borrow().clone()
            }
        }) else {
            return false;
        };
        if up {
            view.mouseUp(event);
        } else {
            view.mouseDragged(event);
        }
        return true;
    }
    let right = kind == NSEventType::RightMouseDown;
    if kind != NSEventType::LeftMouseDown && !right {
        return false;
    }
    PRESSED.with(|p| p.borrow_mut().take());
    let Some(view) = strip_view(window, event.locationInWindow()) else {
        return false;
    };
    if !window.isKeyWindow() {
        window.makeKeyAndOrderFront(None);
        if !view.acceptsFirstMouse(Some(event)) {
            return true;
        }
    }
    if right {
        view.rightMouseDown(event);
        return true;
    }
    if view.acceptsFirstResponder() {
        window.makeFirstResponder(Some(&view));
    }
    PRESSED.with(|p| *p.borrow_mut() = Some(view.clone()));
    view.mouseDown(event);
    true
}

/// The app's view under `at` (window coordinates) when it is in a custom
/// chrome's title bar strip, outside the traffic lights.
fn strip_view(window: &NSWindow, at: NSPoint) -> Option<Retained<NSView>> {
    if !chrome().is_custom() || is_fullscreen() {
        return None;
    }
    // The strip: above the part of the window the title bar leaves free.
    let free = window.contentLayoutRect();
    if at.y < free.origin.y + free.size.height {
        return None;
    }
    let on_button = BUTTONS
        .iter()
        .filter_map(|b| window.standardWindowButton(*b))
        .any(|b| {
            let r = b.convertRect_toView(b.bounds(), None::<&NSView>);
            !b.isHidden()
                && at.x >= r.origin.x
                && at.x <= r.origin.x + r.size.width
                && at.y >= r.origin.y
                && at.y <= r.origin.y + r.size.height
        });
    if on_button {
        return None;
    }
    let content = window.contentView()?;
    // `hitTest:` takes a point in the view's superview.
    let frame = unsafe { content.superview() }?;
    let hit = content.hitTest(frame.convertPoint_fromView(at, None::<&NSView>))?;
    // AppKit gives its controls (buttons, segmented controls, fields)
    // their presses there itself, and tracks them itself. Prism's labels
    // are text fields that it does not.
    let control = hit.isKindOfClass(NSControl::class())
        && hit.downcast_ref::<crate::label::PrismLabel>().is_none();
    (!control).then_some(hit)
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

/// The traffic lights' frame in window coordinates (they grow upwards):
/// `None` for a standard chrome or in full screen, where they are hidden.
fn lights(window: &NSWindow) -> Option<NSRect> {
    if !chrome().is_custom() || is_fullscreen() {
        return None;
    }
    let mut union: Option<NSRect> = None;
    for button in BUTTONS
        .iter()
        .filter_map(|b| window.standardWindowButton(*b))
    {
        if button.isHidden() {
            continue;
        }
        let mut r = button.convertRect_toView(button.bounds(), None::<&NSView>);
        // On the way out of full screen the buttons are still in the
        // full-screen title bar's own window: measure them from their title
        // bar's top edge, which will be this window's.
        if let Some(host) = button.window()
            && *host != *window
            && let Some(bar) = unsafe { button.superview() }
        {
            let b = bar.convertRect_toView(bar.bounds(), None::<&NSView>);
            r.origin.y += window.frame().size.height - (b.origin.y + b.size.height);
        }
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
    union
}

/// Where the traffic lights are, from the window's top-left corner: `None`
/// for a standard chrome or in full screen. `cx.window_controls()` reports
/// them only while no bar is under them ([`over_app`]).
pub(crate) fn controls(window: &NSWindow) -> Option<WindowControls> {
    let r = lights(window)?;
    let content = window.contentView()?;
    // Window coordinates grow upwards; the content view covers the window.
    let top = content.frame().size.height - (r.origin.y + r.size.height);
    Some(WindowControls {
        x: r.origin.x as f32,
        y: top as f32,
        width: r.size.width as f32,
        height: r.size.height as f32,
    })
}

/// A custom chrome's traffic lights as the window draws them (its frame
/// view, around them), and where they are in window coordinates: dev
/// snapshots (`PRISM_SNAPSHOT`) draw them over the content.
pub(crate) fn lights_image(window: &NSWindow) -> Option<(NSRect, Retained<NSImage>)> {
    let r = lights(window)?;
    let frame = unsafe { window.contentView()?.superview() }?;
    let local = frame.convertRect_fromView(r, None::<&NSView>);
    let rep = frame.bitmapImageRepForCachingDisplayInRect(local)?;
    frame.cacheDisplayInRect_toBitmapImageRep(local, &rep);
    let image = NSImage::initWithSize(objc2::AllocAnyThread::alloc(), local.size);
    image.addRepresentation(&rep);
    Some((r, image))
}

/// The traffic lights, when the app's own views are under them: `None`
/// when one of `bars` (tab bars, navigation headers) runs along the top
/// edge under them, since it makes room for them itself ([`bar_room`])
/// and an app's own title bar would be a second one.
pub(crate) fn over_app<'a>(
    window: &NSWindow,
    bars: impl IntoIterator<Item = &'a NSView>,
) -> Option<WindowControls> {
    let controls = controls(window)?;
    let covered = bars.into_iter().any(|bar| {
        let (left, right) = bar_room(bar);
        left > 0. || right > 0.
    });
    (!covered).then_some(controls)
}

/// Room between the lights and what follows them in a bar.
const GAP: f64 = 4.;

/// The room `view` (a tab bar's or a navigation header's bar, along the
/// top edge) keeps at its left and right ends for the traffic lights over
/// it, as Windows' bars keep room for the caption buttons: Back, bar
/// buttons and tabs start after it. `(0, 0)` lower down, in a sheet, in
/// full screen (no traffic lights) or with a standard chrome.
pub(crate) fn bar_room(view: &NSView) -> (f64, f64) {
    if view.isHiddenOrHasHiddenAncestor() || !along_top(view) {
        return (0., 0.);
    }
    let Some(lights) = view.window().and_then(|w| lights(&w)) else {
        return (0., 0.);
    };
    let r = view.convertRect_fromView(lights, None::<&NSView>);
    let b = view.bounds();
    // Flipped or not, the lights must cross the bar's height.
    if r.origin.y >= b.origin.y + b.size.height || r.origin.y + r.size.height <= b.origin.y {
        return (0., 0.);
    }
    room(b.size.width, r.origin.x, r.origin.x + r.size.width)
}

/// The room a bar `width` wide keeps at its left and right ends for
/// buttons spanning `x0`..`x1` of it: up to their far edge (and a gap) at
/// the end they are at; none if they are beside it.
fn room(width: f64, x0: f64, x1: f64) -> (f64, f64) {
    if x1 <= 0. || x0 >= width || width <= 0. {
        return (0., 0.);
    }
    if (x0 + x1) / 2. <= width / 2. {
        ((x1 + GAP).min(width), 0.)
    } else {
        (0., (width - x0 + GAP).min(width))
    }
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

#[cfg(test)]
mod tests {
    use super::room;

    #[test]
    fn bars_keep_room_for_the_traffic_lights_at_their_end() {
        // The lights at 7..59 of an 800-wide bar: Back starts after them.
        assert_eq!(room(800., 7., 59.), (63., 0.));
        // A right-to-left system puts them at the right.
        assert_eq!(room(800., 741., 793.), (0., 63.));
        // A split view's right pane, which they do not reach.
        assert_eq!(room(500., -293., -241.), (0., 0.));
        assert_eq!(room(300., 320., 372.), (0., 0.));
        // A narrow bar: at most all of it.
        assert_eq!(room(100., 7., 59.), (63., 0.));
        assert_eq!(room(60., 0., 59.), (60., 0.));
    }
}
