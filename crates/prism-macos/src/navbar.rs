//! A screen's bar buttons in the stack's header bar (#30): `NSButton`s
//! (an SF Symbol image when the button has an icon, its title otherwise),
//! after Back on the leading side and at the trailing edge. macOS has one
//! title size: `large_title` changes nothing. The tint is the buttons'
//! `contentTintColor` and the tab bar's selected segment.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSAccessibility, NSButton, NSColor, NSImage, NSSegmentedControl, NSView};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use prism_core::navbar::ScreenConfig;
use prism_core::{NodeId, UiEvent};

/// Bar buttons per screen: a button's tag is `screen * 256 + index`.
const PER_SCREEN: isize = 256;

/// A header's bar buttons: each with whether it is on the trailing side.
pub type Bar = RefCell<Vec<(Retained<NSButton>, bool)>>;

thread_local! {
    static CONFIGS: RefCell<HashMap<NodeId, ScreenConfig>> = RefCell::default();
    /// Tints of stacks (their bar buttons take it).
    static TINTS: RefCell<HashMap<NodeId, Retained<NSColor>>> = RefCell::default();
    static TARGET: Retained<BarTarget> = {
        let mtm = MainThreadMarker::new().expect("main thread");
        let this = BarTarget::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    };
}

define_class!(
    /// The target of every bar button: reports the click to the screen.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismBarTarget"]
    struct BarTarget;

    unsafe impl NSObjectProtocol for BarTarget {}

    impl BarTarget {
        #[unsafe(method(onBarButton:))]
        fn on_bar_button(&self, sender: &NSButton) {
            let tag = sender.tag();
            let (screen, index) = (tag / PER_SCREEN, tag % PER_SCREEN);
            crate::imp::dispatch(UiEvent::Change(screen as NodeId, index as f64));
        }
    }
);

/// Records screen `id`'s config. Returns whether it changed.
pub(crate) fn set_config(id: NodeId, config: &str) -> bool {
    let c = ScreenConfig::decode(config);
    CONFIGS.with(|m| m.borrow_mut().insert(id, c.clone()) != Some(c))
}

/// Forgets a destroyed screen or stack.
pub(crate) fn forget(id: NodeId) {
    CONFIGS.with(|m| m.borrow_mut().remove(&id));
    TINTS.with(|m| m.borrow_mut().remove(&id));
}

/// The tint of stack `id`'s buttons (Back, bar buttons).
pub(crate) fn set_stack_tint(id: NodeId, back: &NSButton, tint: Option<Retained<NSColor>>) {
    back.setContentTintColor(tint.as_deref());
    TINTS.with(|m| match tint {
        Some(t) => m.borrow_mut().insert(id, t),
        None => m.borrow_mut().remove(&id),
    });
}

/// The selected segment of a tab bar: its bezel. Its symbol and title are
/// drawn over it as AppKit draws a selected segment's (white on the tint,
/// gray in a window that is not key); a symbol drawn in the tint itself
/// vanished into the bezel.
pub(crate) fn tint_tabs(segments: &NSSegmentedControl, tint: Option<Retained<NSColor>>) {
    segments.setSelectedSegmentBezelColor(tint.as_deref());
}

/// Puts the buttons of screen `top` in `header` (replacing the old ones).
pub(crate) fn refresh(
    mtm: MainThreadMarker,
    stack: NodeId,
    header: &NSView,
    bar: &Bar,
    top: Option<NodeId>,
) {
    for (b, _) in bar.borrow_mut().drain(..) {
        b.removeFromSuperview();
    }
    let Some(top) = top else { return };
    let config = CONFIGS
        .with(|m| m.borrow().get(&top).cloned())
        .unwrap_or_default();
    let tint = TINTS.with(|m| m.borrow().get(&stack).cloned());
    let mut buttons = Vec::new();
    for (i, item) in config.items.iter().enumerate() {
        let image = (!item.icon.is_empty())
            .then(|| {
                NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(&item.icon),
                    Some(&NSString::from_str(item.spoken())),
                )
            })
            .flatten();
        let title = NSString::from_str(if image.is_some() { "" } else { &item.title });
        let button = TARGET.with(|t| {
            let target: &AnyObject = t;
            unsafe {
                NSButton::buttonWithTitle_target_action(
                    &title,
                    Some(target),
                    Some(sel!(onBarButton:)),
                    mtm,
                )
            }
        });
        if let Some(image) = &image {
            button.setImage(Some(image));
        }
        // Borderless, like toolbar items: the tint colors the symbol or
        // the title (a bordered button keeps the system's colors).
        button.setBordered(false);
        button.setTag(top as isize * PER_SCREEN + i as isize);
        button.setAccessibilityLabel(Some(&NSString::from_str(item.spoken())));
        button.setContentTintColor(tint.as_deref());
        header.addSubview(&button);
        buttons.push((button, item.placement == prism_core::BarPlacement::Trailing));
    }
    *bar.borrow_mut() = buttons;
}

/// Places the bar buttons in a header `w` wide and `h` tall: leading ones
/// after Back (when it shows), trailing ones from the trailing edge in.
pub(crate) fn layout(bar: &Bar, back: &NSButton, w: f64, h: f64, rtl: bool) {
    const GAP: f64 = 8.;
    let mut lead = GAP;
    if !back.isHidden() {
        lead += back.fittingSize().width + GAP;
    }
    let bar = bar.borrow();
    // Trailing buttons in the order added, ending at the trailing edge.
    let trailing: f64 = bar
        .iter()
        .filter(|(_, t)| *t)
        .map(|(b, _)| b.fittingSize().width + GAP)
        .sum();
    let mut trail = w - trailing;
    for (b, is_trailing) in bar.iter() {
        let size = b.fittingSize();
        let x = if *is_trailing { &mut trail } else { &mut lead };
        // Right to left, sides swap.
        let left = if rtl { w - *x - size.width } else { *x };
        b.setFrame(NSRect::new(
            NSPoint::new(left, ((h - size.height) / 2.).round()),
            NSSize::new(size.width, size.height),
        ));
        *x += size.width + GAP;
    }
}
