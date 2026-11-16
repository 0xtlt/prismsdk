//! Pointer, keys, focus, overlays, layers and choice controls on AppKit
//! (#17, #18, #19, #25): key names, hit testing (`hit_test(false)`,
//! `inert`), sheets / dialogs / popovers presented without a route, the
//! runtime's layers in a transparent child window above sheets, radio
//! groups, and the text field cell that makes room for a leading icon.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use objc2::ClassType;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::NSAccessibility;
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSCell, NSColor, NSControlStateValueOff, NSControlStateValueOn,
    NSEvent, NSEventModifierFlags, NSFont, NSImage, NSImageView, NSPanel, NSPopover,
    NSPopoverBehavior, NSTextFieldCell, NSUserInterfaceLayoutDirection, NSView, NSViewController,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow,
    NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSRectEdge, NSSize, NSString,
};
use prism_core::controls::{OverlayConfig, RadioConfig};
use prism_core::{KeyEvent, Modifiers, NodeId, OverlayKind};

use crate::imp::PrismView;

// --- Keys ------------------------------------------------------------------------

/// The web's `KeyboardEvent.key` name of a key: named keys by virtual key
/// code (Events.h, `kVK_*`), others by the characters typed without
/// modifiers but Shift.
pub(crate) fn key_name(code: u16, chars: &str) -> Option<String> {
    let named = match code {
        53 => "Escape",
        36 | 76 => "Enter",
        48 => "Tab",
        51 => "Backspace",
        117 => "Delete",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        115 => "Home",
        119 => "End",
        116 => "PageUp",
        121 => "PageDown",
        49 => " ",
        122 => "F1",
        120 => "F2",
        99 => "F3",
        118 => "F4",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        100 => "F8",
        101 => "F9",
        109 => "F10",
        103 => "F11",
        111 => "F12",
        110 => "ContextMenu",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_string());
    }
    // Function-key characters (private use area) without a name: skip.
    let c = chars.chars().next()?;
    if ('\u{F700}'..='\u{F8FF}').contains(&c) || c.is_control() {
        return None;
    }
    Some(chars.to_string())
}

/// Shift, Control, Option, Command.
pub(crate) fn modifiers(flags: NSEventModifierFlags) -> Modifiers {
    Modifiers {
        shift: flags.contains(NSEventModifierFlags::Shift),
        control: flags.contains(NSEventModifierFlags::Control),
        alt: flags.contains(NSEventModifierFlags::Option),
        meta: flags.contains(NSEventModifierFlags::Command),
    }
}

/// A key-down event as Prism reports it.
pub(crate) fn key_event(event: &NSEvent) -> Option<KeyEvent> {
    let chars = event
        .charactersIgnoringModifiers()
        .map(|s| s.to_string())
        .unwrap_or_default();
    Some(KeyEvent {
        key: key_name(event.keyCode(), &chars)?,
        modifiers: modifiers(event.modifierFlags()),
        repeat: event.isARepeat(),
        selection: None,
    })
}

/// The Prism node ids from `view` up to the window's content (innermost
/// first): what a key press in that view bubbles through.
pub(crate) fn node_chain(view: &NSView) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut next: Option<Retained<NSView>> = Some(objc2::Message::retain(view));
    while let Some(v) = next {
        if let Some(p) = v.downcast_ref::<PrismView>() {
            let id = p.ivars().id.get();
            if id != 0 && out.last() != Some(&id) {
                out.push(id);
            }
        }
        next = unsafe { v.superview() };
    }
    out
}

// --- Hit testing -----------------------------------------------------------------

thread_local! {
    /// Views (by address) that take no pointer input, with their subtree.
    static NO_HIT: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    /// Views that are inert (no input, no focus, hidden from VoiceOver).
    static INERT: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
}

fn key(v: &NSView) -> usize {
    v as *const NSView as usize
}

/// `hit_test(false)` / `inert`: the view and its subtree let clicks through.
pub(crate) fn set_no_hit(v: &NSView, on: bool) {
    NO_HIT.with(|s| {
        let mut s = s.borrow_mut();
        if on {
            s.insert(key(v))
        } else {
            s.remove(&key(v))
        };
    });
}

pub(crate) fn is_no_hit(v: &NSView) -> bool {
    NO_HIT.with(|s| s.borrow().contains(&key(v)))
}

/// Marks `v` inert; returns whether that changed.
pub(crate) fn set_inert(v: &NSView, on: bool) -> bool {
    INERT.with(|s| {
        let mut s = s.borrow_mut();
        if on {
            s.insert(key(v))
        } else {
            s.remove(&key(v))
        }
    })
}

/// Whether `v` or an ancestor is inert.
pub(crate) fn in_inert(v: &NSView) -> bool {
    let mut next: Option<Retained<NSView>> = Some(objc2::Message::retain(v));
    while let Some(view) = next {
        if INERT.with(|s| s.borrow().contains(&key(&view))) {
            return true;
        }
        next = unsafe { view.superview() };
    }
    false
}

/// Forgets a destroyed view.
pub(crate) fn forget(v: &NSView) {
    NO_HIT.with(|s| s.borrow_mut().remove(&key(v)));
    INERT.with(|s| s.borrow_mut().remove(&key(v)));
}

/// Controls under an inert view refuse the keyboard focus.
pub(crate) fn refuse_focus(v: &NSView, on: bool) {
    if let Some(c) = v.downcast_ref::<objc2_app_kit::NSControl>() {
        c.setRefusesFirstResponder(on);
    }
    for sub in v.subviews().iter() {
        refuse_focus(&sub, on);
    }
}

/// `NSView.hitTest:` skipping views that take no pointer input. A pass-
/// through view (a layer's root) is never the hit itself, only its children.
pub(crate) fn hit(view: &PrismView, point: NSPoint) -> *mut NSView {
    if view.isHidden() || is_no_hit(view) {
        return std::ptr::null_mut();
    }
    let superview = unsafe { view.superview() };
    let p = view.convertPoint_fromView(point, superview.as_deref());
    let b = view.bounds();
    let inside = p.x >= b.origin.x
        && p.y >= b.origin.y
        && p.x < b.origin.x + b.size.width
        && p.y < b.origin.y + b.size.height;
    if !inside {
        return std::ptr::null_mut();
    }
    let subs: Vec<Retained<NSView>> = view.subviews().iter().collect();
    for sub in subs.into_iter().rev() {
        if is_no_hit(&sub) {
            continue;
        }
        if let Some(found) = sub.hitTest(p) {
            return Retained::autorelease_return(found);
        }
    }
    if view.ivars().pass_through.get() {
        return std::ptr::null_mut();
    }
    let me: &NSView = view;
    Retained::autorelease_return(objc2::Message::retain(me))
}

// --- Radio groups ----------------------------------------------------------------

/// Native radio buttons in a Prism view: buttons with the same action in the
/// same superview form one group (arrow keys move between them).
pub struct MacRadios {
    pub(crate) id: NodeId,
    pub(crate) container: Retained<PrismView>,
    buttons: RefCell<Vec<Retained<NSButton>>>,
    horizontal: Cell<bool>,
    selected: Cell<isize>,
    font: RefCell<Option<Retained<NSFont>>>,
}

const RADIO_GAP_ROW: f64 = 16.;
const RADIO_GAP_COLUMN: f64 = 6.;

impl MacRadios {
    pub(crate) fn new(mtm: MainThreadMarker, id: NodeId) -> Self {
        MacRadios {
            id,
            container: PrismView::new(mtm, id),
            buttons: RefCell::new(Vec::new()),
            horizontal: Cell::new(false),
            selected: Cell::new(-1),
            font: RefCell::new(None),
        }
    }

    /// One radio button per option (tag = index), reporting `onRadio:`.
    pub(crate) fn set_config(&self, config: &str, target: &AnyObject) {
        let c = RadioConfig::decode(config);
        self.horizontal.set(c.horizontal);
        let mtm = self.container.mtm();
        let buttons: Vec<Retained<NSButton>> = c
            .options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let b = unsafe {
                    NSButton::radioButtonWithTitle_target_action(
                        &NSString::from_str(o),
                        Some(target),
                        Some(objc2::sel!(onRadio:)),
                        mtm,
                    )
                };
                b.setTag(i as isize);
                if let Some(f) = self.font.borrow().as_deref() {
                    b.setFont(Some(f));
                }
                b
            })
            .collect();
        let views: Vec<&NSView> = buttons.iter().map(|b| -> &NSView { b }).collect();
        self.container.setSubviews(&NSArray::from_slice(&views));
        *self.buttons.borrow_mut() = buttons;
        self.sync();
        self.layout();
    }

    pub(crate) fn set_selected(&self, index: isize) {
        self.selected.set(index);
        self.sync();
    }

    fn sync(&self) {
        for (i, b) in self.buttons.borrow().iter().enumerate() {
            b.setState(if i as isize == self.selected.get() {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
    }

    pub(crate) fn set_font(&self, font: Retained<NSFont>) {
        for b in self.buttons.borrow().iter() {
            b.setFont(Some(&font));
        }
        *self.font.borrow_mut() = Some(font);
    }

    pub(crate) fn size(&self) -> NSSize {
        let sizes: Vec<NSSize> = self
            .buttons
            .borrow()
            .iter()
            .map(|b| b.fittingSize())
            .collect();
        let n = sizes.len().saturating_sub(1) as f64;
        if self.horizontal.get() {
            NSSize::new(
                sizes.iter().map(|s| s.width).sum::<f64>() + n * RADIO_GAP_ROW,
                sizes.iter().map(|s| s.height).fold(0., f64::max),
            )
        } else {
            NSSize::new(
                sizes.iter().map(|s| s.width).fold(0., f64::max),
                sizes.iter().map(|s| s.height).sum::<f64>() + n * RADIO_GAP_COLUMN,
            )
        }
    }

    /// Places the buttons in the container (leading edge first).
    pub(crate) fn layout(&self) {
        let width = self.container.bounds().size.width;
        let rtl = self.container.userInterfaceLayoutDirection()
            == objc2_app_kit::NSUserInterfaceLayoutDirection::RightToLeft;
        let (mut x, mut y) = (0., 0.);
        for b in self.buttons.borrow().iter() {
            let s = b.fittingSize();
            let left = if rtl { width - x - s.width } else { x };
            b.setFrame(NSRect::new(NSPoint::new(left, y), s));
            if self.horizontal.get() {
                x += s.width + RADIO_GAP_ROW;
            } else {
                y += s.height + RADIO_GAP_COLUMN;
            }
        }
    }
}

// --- Leading icon ----------------------------------------------------------------

#[derive(Default)]
pub struct FieldCellIvars {
    /// Room kept free before the text (the icon).
    leading: Cell<f64>,
}

define_class!(
    /// A text field cell whose text starts after a leading icon.
    #[unsafe(super(NSTextFieldCell, objc2_app_kit::NSActionCell, NSCell, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismFieldCell"]
    #[ivars = FieldCellIvars]
    pub struct PrismFieldCell;

    impl PrismFieldCell {
        /// Text (and the field editor) start after the icon.
        #[unsafe(method(drawingRectForBounds:))]
        fn drawing_rect(&self, bounds: NSRect) -> NSRect {
            let r: NSRect = unsafe { msg_send![super(self), drawingRectForBounds: bounds] };
            let l = self.ivars().leading.get();
            NSRect::new(
                NSPoint::new(r.origin.x + l, r.origin.y),
                NSSize::new((r.size.width - l).max(0.), r.size.height),
            )
        }
    }
);

/// Gives `field` a cell leaving `leading` points before its text (copying
/// the look of its current cell), and an SF Symbol image view in that room.
pub(crate) fn set_leading_icon(
    field: &objc2_app_kit::NSTextField,
    icon: &str,
    existing: Option<Retained<NSImageView>>,
) -> Option<Retained<NSImageView>> {
    let mtm = field.mtm();
    let image = (!icon.is_empty())
        .then(|| {
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(icon),
                None,
            )
        })
        .flatten();
    let Some(image) = image else {
        if let Some(v) = existing {
            v.removeFromSuperview();
        }
        if let Some(cell) = field
            .cell()
            .and_then(|c| c.downcast::<PrismFieldCell>().ok())
        {
            cell.ivars().leading.set(0.);
            NSView::setNeedsDisplay(field, true);
        }
        return None;
    };
    if field.isKindOfClass(objc2_app_kit::NSSecureTextField::class()) {
        // A secure field keeps its own cell (bullets): no icon.
        return None;
    }
    let cell = match field
        .cell()
        .and_then(|c| c.downcast::<PrismFieldCell>().ok())
    {
        Some(c) => c,
        None => {
            let old = field.cell();
            let this = PrismFieldCell::alloc(mtm).set_ivars(FieldCellIvars::default());
            let cell: Retained<PrismFieldCell> =
                unsafe { msg_send![super(this), initTextCell: &*NSString::new()] };
            if let Some(old) = old.and_then(|c| c.downcast::<NSTextFieldCell>().ok()) {
                cell.setBezeled(old.isBezeled());
                cell.setBezelStyle(old.bezelStyle());
                cell.setDrawsBackground(old.drawsBackground());
                cell.setEditable(old.isEditable());
                cell.setSelectable(old.isSelectable());
                cell.setScrollable(old.isScrollable());
                cell.setWraps(old.wraps());
                cell.setUsesSingleLineMode(old.usesSingleLineMode());
                cell.setLineBreakMode(old.lineBreakMode());
                cell.setPlaceholderString(old.placeholderString().as_deref());
                cell.setFont(old.font().as_deref());
                cell.setTextColor(old.textColor().as_deref());
            } else {
                cell.setBezeled(true);
                cell.setEditable(true);
                cell.setSelectable(true);
                cell.setScrollable(true);
            }
            cell.setSendsActionOnEndEditing(false);
            let value = field.stringValue();
            field.setCell(Some(&cell));
            field.setStringValue(&value);
            cell
        }
    };
    cell.ivars().leading.set(22.);
    let view = existing.unwrap_or_else(|| {
        let v = NSImageView::new(mtm);
        v.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        v.setAccessibilityElement(false);
        field.addSubview(&v);
        v
    });
    view.setImage(Some(&image));
    place_icon(field, &view);
    NSView::setNeedsDisplay(field, true);
    Some(view)
}

/// The icon at the field's leading edge, centered vertically.
pub(crate) fn place_icon(field: &objc2_app_kit::NSTextField, icon: &NSImageView) {
    let b = field.bounds().size;
    let rtl = field.userInterfaceLayoutDirection()
        == objc2_app_kit::NSUserInterfaceLayoutDirection::RightToLeft;
    let s = 15.;
    let x = if rtl { b.width - 7. - s } else { 7. };
    icon.setFrame(NSRect::new(
        NSPoint::new(x, ((b.height - s) / 2.).round()),
        NSSize::new(s, s),
    ));
}

/// Completions for a text input with suggestions: those starting with the
/// whole text typed so far, cut to the part after `from` (where the word
/// AppKit completes starts).
pub(crate) fn completions(text: &str, from: usize, suggestions: &[String]) -> Vec<String> {
    let typed = text.to_lowercase();
    if typed.trim().is_empty() {
        return Vec::new();
    }
    let prefix: String = text.chars().take(from).collect();
    suggestions
        .iter()
        .filter(|s| s.to_lowercase().starts_with(&typed) && s.to_lowercase() != typed)
        .filter(|s| s.starts_with(&prefix) || prefix.is_empty())
        .map(|s| s.chars().skip(from).collect())
        .collect()
}

// --- Overlays --------------------------------------------------------------------

/// Who hears that a popover closed (the backend's target).
pub(crate) type Delegate = objc2::runtime::ProtocolObject<dyn objc2_app_kit::NSPopoverDelegate>;

/// A sheet, dialog or popover: an empty anchor view in the parent (the
/// popover points at the parent) and the content's host, presented in a
/// sheet window or an `NSPopover` while open. A popover that leaves the
/// focus to its anchor (`keep_focus`, #70) is a borderless child panel
/// instead: it never becomes key, so the anchor's field keeps the keys,
/// and its views still take clicks (`acceptsFirstMouse`).
pub struct MacOverlay {
    pub(crate) id: NodeId,
    pub(crate) anchor: Retained<PrismView>,
    /// Holds the content (the presented window's content view, or the
    /// popover's view).
    pub(crate) host: Retained<PrismView>,
    pub(crate) config: Cell<OverlayConfig>,
    /// The app wants it shown.
    pub(crate) open: Cell<bool>,
    pub(crate) presented: Cell<bool>,
    /// The content's laid out size.
    pub(crate) size: Cell<NSSize>,
    window: RefCell<Option<Retained<NSWindow>>>,
    pub(crate) popover: RefCell<Option<Retained<NSPopover>>>,
    /// The non-activating panel of a `keep_focus` popover, and its
    /// background (a menu material with rounded corners).
    panel: RefCell<Option<(Retained<NSPanel>, Retained<NSVisualEffectView>)>>,
    /// Whether the panel is the window it shows in.
    in_panel: Cell<bool>,
}

impl MacOverlay {
    pub(crate) fn new(mtm: MainThreadMarker, id: NodeId) -> Self {
        let host = PrismView::new(mtm, id);
        host.ivars().sheet_root.set(true);
        MacOverlay {
            id,
            anchor: PrismView::new(mtm, 0),
            host,
            config: Cell::new(OverlayConfig::default()),
            open: Cell::new(false),
            presented: Cell::new(false),
            size: Cell::new(NSSize::ZERO),
            window: RefCell::new(None),
            popover: RefCell::new(None),
            panel: RefCell::new(None),
            in_panel: Cell::new(false),
        }
    }

    /// A popover shown in a non-activating panel (see [`MacOverlay`]).
    fn panel_kind(c: OverlayConfig) -> bool {
        c.kind == OverlayKind::Popover && c.keep_focus
    }

    pub(crate) fn set_config(&self, config: &str) {
        let c = OverlayConfig::decode(config);
        self.host.ivars().overlay_fixed.set(c.fixed);
        // Shown the other way: presented again by the next layout.
        if self.presented.get()
            && (Self::panel_kind(c) != self.in_panel.get() || c.kind != self.config.get().kind)
        {
            self.dismiss(None);
        }
        self.config.set(c);
        if let Some(p) = self.popover.borrow().as_deref() {
            p.setBehavior(behavior(c));
        }
    }

    /// The window a sheet or dialog shows in, if presented.
    pub(crate) fn window(&self) -> Option<Retained<NSWindow>> {
        if !self.presented.get() {
            return None;
        }
        if self.in_panel.get() {
            return self
                .panel
                .borrow()
                .as_ref()
                .map(|(p, _)| Retained::into_super(p.clone()));
        }
        self.window.borrow().clone().or_else(|| {
            self.popover
                .borrow()
                .as_ref()
                .and_then(|_| self.host.window())
        })
    }

    /// Shows it if open and laid out, or follows its new size.
    pub(crate) fn update(&self, main: Option<&NSWindow>, delegate: &Delegate) {
        let size = self.size.get();
        if !self.open.get() {
            self.dismiss(main);
            return;
        }
        if size.width <= 0. || size.height <= 0. {
            return;
        }
        if self.presented.get() {
            if self.in_panel.get() {
                self.place_panel(size);
                return;
            }
            if let Some(w) = self.window.borrow().as_deref()
                && w.contentRectForFrameRect(w.frame()).size != size
            {
                w.setContentSize(size);
            }
            if let Some(p) = self.popover.borrow().as_deref()
                && p.contentSize() != size
            {
                p.setContentSize(size);
            }
            return;
        }
        let Some(main) = main else { return };
        match self.config.get().kind {
            OverlayKind::Popover if Self::panel_kind(self.config.get()) => self.show_panel(size),
            OverlayKind::Popover => self.show_popover(size, delegate),
            OverlayKind::Sheet | OverlayKind::Dialog => {
                let mtm = main.mtm();
                let window = self.window.borrow_mut().take().unwrap_or_else(|| {
                    let w = unsafe {
                        NSWindow::initWithContentRect_styleMask_backing_defer(
                            NSWindow::alloc(mtm),
                            NSRect::new(NSPoint::ZERO, size),
                            NSWindowStyleMask::Titled,
                            NSBackingStoreType::Buffered,
                            false,
                        )
                    };
                    unsafe { w.setReleasedWhenClosed(false) };
                    w
                });
                window.setContentSize(size);
                window.setContentView(Some(&self.host));
                self.presented.set(true);
                main.beginSheet_completionHandler(&window, None);
                // Escape reaches the host (`cancelOperation:`) through the
                // responder chain.
                window.makeFirstResponder(Some(&self.host));
                *self.window.borrow_mut() = Some(window);
            }
        }
    }

    fn show_popover(&self, size: NSSize, delegate: &Delegate) {
        let Some(parent) = (unsafe { self.anchor.superview() }) else {
            return;
        };
        if parent.window().is_none() {
            return;
        }
        let mtm = self.host.mtm();
        let popover = NSPopover::new(mtm);
        let vc = NSViewController::new(mtm);
        vc.setView(&self.host);
        popover.setContentViewController(Some(&vc));
        popover.setContentSize(size);
        popover.setBehavior(behavior(self.config.get()));
        popover.setAnimates(!std::env::var_os("PRISM_SNAPSHOT").is_some());
        popover.setDelegate(Some(delegate));
        self.presented.set(true);
        // AppKit shows no popover for an anchor scrolled out of view: bring
        // it, and the room the popover takes below it, into view.
        let b = parent.bounds();
        parent.scrollRectToVisible(NSRect::new(
            b.origin,
            NSSize::new(b.size.width, b.size.height + size.height + 16.),
        ));
        // Below the anchor (Prism views are flipped: max Y is the bottom).
        popover.showRelativeToRect_ofView_preferredEdge(parent.bounds(), &parent, NSRectEdge::MaxY);
        *self.popover.borrow_mut() = Some(popover);
    }

    /// Shows a `keep_focus` popover: a borderless panel, a child of the
    /// anchor's window (it moves with it), that never becomes key.
    fn show_panel(&self, size: NSSize) {
        let Some(parent) = (unsafe { self.anchor.superview() }) else {
            return;
        };
        let Some(window) = parent.window() else {
            return;
        };
        let mtm = self.host.mtm();
        let (panel, background) = self.panel.borrow_mut().take().unwrap_or_else(|| {
            let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
            let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
                NSPanel::alloc(mtm),
                NSRect::new(NSPoint::ZERO, size),
                style,
                NSBackingStoreType::Buffered,
                false,
            );
            unsafe { panel.setReleasedWhenClosed(false) };
            // A borderless window cannot become key: clicks inside leave
            // the focus where it is.
            panel.setBecomesKeyOnlyIfNeeded(true);
            panel.setHidesOnDeactivate(false);
            panel.setOpaque(false);
            panel.setBackgroundColor(Some(&NSColor::clearColor()));
            panel.setHasShadow(true);
            let background = NSVisualEffectView::new(mtm);
            background.setMaterial(NSVisualEffectMaterial::Popover);
            background.setState(NSVisualEffectState::Active);
            background.setWantsLayer(true);
            if let Some(l) = background.layer() {
                l.setCornerRadius(10.);
                l.setMasksToBounds(true);
            }
            panel.setContentView(Some(&background));
            (panel, background)
        });
        if unsafe { self.host.superview() }.as_deref() != Some(&**background) {
            self.host.removeFromSuperview();
            background.addSubview(&self.host);
        }
        self.presented.set(true);
        self.in_panel.set(true);
        *self.panel.borrow_mut() = Some((panel.clone(), background));
        self.place_panel(size);
        // SAFETY: both windows are alive; `dismiss` detaches the panel.
        unsafe { window.addChildWindow_ordered(&panel, NSWindowOrderingMode::Above) };
        panel.orderFront(None);
    }

    /// Below the anchor (above it without room below), at the content's size.
    fn place_panel(&self, size: NSSize) {
        let Some((panel, background)) = self.panel.borrow().clone() else {
            return;
        };
        let Some(parent) = (unsafe { self.anchor.superview() }) else {
            return;
        };
        let Some(window) = parent.window() else {
            return;
        };
        let anchor = window.convertRectToScreen(parent.convertRect_toView(parent.bounds(), None));
        let frame = panel_frame(
            anchor,
            size,
            window.screen().map(|s| s.visibleFrame()),
            parent.userInterfaceLayoutDirection() == NSUserInterfaceLayoutDirection::RightToLeft,
        );
        if panel.frame() != frame {
            panel.setFrame_display(frame, true);
        }
        let local = NSRect::new(NSPoint::ZERO, size);
        background.setFrame(local);
        self.host.setFrame(local);
    }

    /// Closes it (the app asked, or it was destroyed).
    pub(crate) fn dismiss(&self, main: Option<&NSWindow>) {
        if !self.presented.replace(false) {
            return;
        }
        if self.in_panel.replace(false) {
            if let Some((p, _)) = self.panel.borrow().as_ref() {
                if let Some(parent) = p.parentWindow() {
                    parent.removeChildWindow(p);
                }
                p.orderOut(None);
            }
            return;
        }
        if let Some(w) = self.window.borrow().as_deref() {
            if let Some(main) = main {
                main.endSheet(w);
            }
            w.orderOut(None);
        }
        if let Some(p) = self.popover.borrow_mut().take() {
            p.close();
        }
    }

    /// The user closed the popover (Escape, a click outside).
    pub(crate) fn popover_closed(&self) -> bool {
        self.popover.borrow_mut().take();
        self.presented.replace(false)
    }
}

/// Where a `keep_focus` popover's panel goes, in screen coordinates
/// (y up): under `anchor` with a 6 point gap, above it when the screen has
/// no room below, leading-aligned, kept inside `screen`.
pub(crate) fn panel_frame(
    anchor: NSRect,
    size: NSSize,
    screen: Option<NSRect>,
    rtl: bool,
) -> NSRect {
    let gap = 6.;
    let mut x = if rtl {
        anchor.origin.x + anchor.size.width - size.width
    } else {
        anchor.origin.x
    };
    let mut y = anchor.origin.y - gap - size.height;
    if let Some(s) = screen {
        if y < s.origin.y
            && anchor.origin.y + anchor.size.height + gap + size.height
                <= s.origin.y + s.size.height
        {
            y = anchor.origin.y + anchor.size.height + gap;
        }
        x = x
            .min(s.origin.x + s.size.width - size.width)
            .max(s.origin.x);
    }
    NSRect::new(NSPoint::new(x, y), size)
}

fn behavior(c: OverlayConfig) -> NSPopoverBehavior {
    // Dev snapshots run in an inactive app, which closes transient popovers.
    if c.fixed || c.manual || std::env::var_os("PRISM_SNAPSHOT").is_some() {
        NSPopoverBehavior::ApplicationDefined
    } else {
        NSPopoverBehavior::Transient
    }
}

/// The area an overlay's content may use: a sheet up to 560 wide and the
/// window's height (half with `medium`), a dialog up to 480, a popover up to
/// 420, fitting their content.
pub(crate) fn overlay_area(config: OverlayConfig, window: NSSize) -> (f32, f32) {
    let (w, h) = (window.width as f32, window.height as f32);
    let (max_w, height) = match config.kind {
        OverlayKind::Sheet if config.medium => (560., (h / 2.).max(240.)),
        OverlayKind::Sheet => (560., h - 48.),
        OverlayKind::Dialog => (480., h - 80.),
        OverlayKind::Popover => (420., h - 40.),
    };
    ((w - 40.).clamp(200., max_w), height.max(120.))
}

// --- Layers ----------------------------------------------------------------------

/// Everything #17–#25 keep per window.
#[derive(Default)]
pub struct Interaction {
    /// Nodes reporting keys while focused / anywhere.
    pub(crate) key_local: HashSet<NodeId>,
    pub(crate) key_global: HashSet<NodeId>,
    /// Text input suggestions, by node.
    pub(crate) suggestions: HashMap<NodeId, Vec<String>>,
    /// Open overlays.
    pub(crate) overlays: Vec<Rc<MacOverlay>>,
    /// The runtime's layers, in the layer window.
    pub(crate) layers: Vec<Retained<NSView>>,
    pub(crate) layer_window: Option<Retained<NSWindow>>,
    pub(crate) layer_host: Option<Retained<PrismView>>,
    /// Text fields reporting their selection (`on_select`, #86).
    pub(crate) select_events: HashSet<NodeId>,
    /// Selections set on single-line inputs not being edited: applied
    /// when the app focuses them (`cx.focus`), dropped when the user does
    /// (their click places the caret). Characters.
    pub(crate) pending_selection: HashMap<NodeId, (usize, usize)>,
    /// The input the app asked to focus last.
    pub(crate) app_focus: Option<NodeId>,
}

impl Interaction {
    /// Mounts `layers` in a transparent, borderless child window over
    /// `main`'s content, which stays above sheets and popovers.
    pub(crate) fn set_layers(&mut self, main: Option<&NSWindow>, layers: Vec<Retained<NSView>>) {
        for old in &self.layers {
            old.removeFromSuperview();
        }
        self.layers = layers;
        if self.layers.is_empty() {
            if let Some(w) = &self.layer_window {
                w.orderOut(None);
            }
            return;
        }
        let Some(main) = main else { return };
        if self.layer_window.is_none() {
            let mtm = main.mtm();
            let rect = main.contentRectForFrameRect(main.frame());
            let w = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    rect,
                    NSWindowStyleMask::Borderless,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe { w.setReleasedWhenClosed(false) };
            w.setOpaque(false);
            w.setBackgroundColor(Some(&NSColor::clearColor()));
            w.setHasShadow(false);
            let host = PrismView::new(mtm, 0);
            host.ivars().pass_through.set(true);
            w.setContentView(Some(&host));
            unsafe { main.addChildWindow_ordered(&w, NSWindowOrderingMode::Above) };
            self.layer_window = Some(w);
            self.layer_host = Some(host);
        }
        if let Some(host) = &self.layer_host {
            for l in &self.layers {
                if let Some(p) = l.downcast_ref::<PrismView>() {
                    p.ivars().pass_through.set(true);
                }
                host.addSubview(l);
            }
        }
        self.follow(main);
    }

    /// Keeps the layer window over `main`'s content and above its sheets
    /// and popovers; it only takes clicks while a layer shows something.
    pub(crate) fn follow(&self, main: &NSWindow) {
        let Some(w) = &self.layer_window else { return };
        if self.layers.is_empty() {
            return;
        }
        let rect = main.contentRectForFrameRect(main.frame());
        if w.frame() != rect {
            w.setFrame_display(rect, true);
        }
        let empty = self.layers.iter().all(|l| l.subviews().is_empty());
        w.setIgnoresMouseEvents(empty);
        // Above the topmost sheet or popover.
        let top = self
            .overlays
            .iter()
            .filter_map(|o| o.window())
            .chain(main.attachedSheet())
            .last();
        match top {
            Some(t) => w.orderWindow_relativeTo(NSWindowOrderingMode::Above, t.windowNumber()),
            None if main.isVisible() => w.orderFront(None),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_follow_the_web() {
        assert_eq!(key_name(53, "\u{1b}").as_deref(), Some("Escape"));
        assert_eq!(key_name(126, "\u{F700}").as_deref(), Some("ArrowUp"));
        assert_eq!(key_name(36, "\r").as_deref(), Some("Enter"));
        assert_eq!(key_name(49, " ").as_deref(), Some(" "));
        assert_eq!(key_name(0, "a").as_deref(), Some("a"));
        assert_eq!(key_name(0, "A").as_deref(), Some("A"));
        assert_eq!(key_name(122, "\u{F704}").as_deref(), Some("F1"));
        assert_eq!(key_name(999, "\u{F72C}"), None);
        assert_eq!(key_name(999, ""), None);
    }

    #[test]
    fn completions_continue_the_typed_text() {
        let s: Vec<String> = ["Paris", "Pau", "New York", "Lyon"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(completions("pa", 0, &s), ["Paris", "Pau"]);
        assert_eq!(completions("New Y", 4, &s), ["York"]);
        assert!(completions("", 0, &s).is_empty());
        assert!(completions("Lyon", 0, &s).is_empty());
    }

    #[test]
    fn inline_popover_panels_sit_under_their_anchor() {
        let screen = NSRect::new(NSPoint::ZERO, NSSize::new(1000., 800.));
        let size = NSSize::new(200., 100.);
        // Screen coordinates go up: under the anchor is a smaller y.
        let anchor = NSRect::new(NSPoint::new(100., 500.), NSSize::new(300., 24.));
        let f = panel_frame(anchor, size, Some(screen), false);
        assert_eq!((f.origin.x, f.origin.y), (100., 394.));
        let rtl = panel_frame(anchor, size, Some(screen), true);
        assert_eq!(rtl.origin.x, 200.);
        // No room below: above the anchor.
        let low = NSRect::new(NSPoint::new(900., 40.), NSSize::new(300., 24.));
        let f = panel_frame(low, size, Some(screen), false);
        assert_eq!((f.origin.x, f.origin.y), (800., 70.));
    }

    #[test]
    fn overlay_areas_fit_the_window() {
        let window = NSSize::new(800., 600.);
        let sheet = OverlayConfig::default();
        assert_eq!(overlay_area(sheet, window), (560., 552.));
        let half = OverlayConfig {
            medium: true,
            ..sheet
        };
        assert_eq!(overlay_area(half, window), (560., 300.));
        let pop = OverlayConfig {
            kind: OverlayKind::Popover,
            ..sheet
        };
        assert_eq!(overlay_area(pop, NSSize::new(300., 400.)), (260., 360.));
    }
}

// ---------------------------------------------------------------------------
// Text field selections (#86)
// ---------------------------------------------------------------------------

/// The selection of `tv` (a text area, or the field editor of an input) in
/// characters.
pub(crate) fn text_selection(tv: &objc2_app_kit::NSTextView) -> prism_core::SelectionEvent {
    let text = tv.string().to_string();
    let r = tv.selectedRange();
    let at = |u: usize| prism_native::fields::utf16_to_chars(&text, u);
    prism_core::SelectionEvent::new(at(r.location), at(r.location + r.length))
}

/// Selects `start..end` (characters) in `tv`, clamped to its text.
pub(crate) fn select_text(tv: &objc2_app_kit::NSTextView, start: usize, end: usize) {
    let text = tv.string().to_string();
    let at = |c: usize| prism_native::fields::chars_to_utf16(&text, c);
    let (s, e) = (at(start), at(end.max(start)));
    tv.setSelectedRange(objc2_foundation::NSRange::new(s, e - s));
}

/// The text field node `tv` edits: a text area, or the field editor of a
/// text input (its delegate).
pub(crate) fn text_node(tv: &objc2_app_kit::NSTextView) -> Option<NodeId> {
    if let Some(t) = tv.downcast_ref::<crate::imp::PrismTextView>() {
        return Some(t.ivars().id.get());
    }
    if !tv.isFieldEditor() {
        return None;
    }
    let delegate = tv.delegate()?;
    let obj: &AnyObject = delegate.as_ref();
    let field = obj.downcast_ref::<objc2_app_kit::NSTextField>()?;
    (field.tag() > 0).then(|| field.tag() as NodeId)
}

thread_local! {
    /// The selection last reported for each field, and the fields whose
    /// report is scheduled.
    static REPORTED: RefCell<HashMap<NodeId, prism_core::SelectionEvent>> = RefCell::new(HashMap::new());
    static SCHEDULED: RefCell<HashSet<NodeId>> = RefCell::new(HashSet::new());
}

/// `tv`'s selection changed (`NSTextViewDidChangeSelectionNotification`):
/// reported on the next turn, once AppKit finished the edit (so after its
/// `UiEvent::Input`), at most once per turn and only when it moved.
pub(crate) fn selection_changed(tv: &objc2_app_kit::NSTextView, id: NodeId) {
    if !SCHEDULED.with(|s| s.borrow_mut().insert(id)) {
        return;
    }
    let tv: Retained<objc2_app_kit::NSTextView> = objc2::Message::retain(tv);
    crate::imp::on_main_after(std::time::Duration::ZERO, move || {
        SCHEDULED.with(|s| s.borrow_mut().remove(&id));
        // The field editor moved on to another field meanwhile.
        if text_node(&tv) != Some(id) {
            return;
        }
        let sel = text_selection(&tv);
        let old = REPORTED.with(|r| r.borrow_mut().insert(id, sel));
        if old != Some(sel) {
            crate::imp::dispatch(prism_core::UiEvent::Select(id, sel.start, sel.end));
        }
    });
}

/// Field `id` stopped reporting (or is gone).
pub(crate) fn forget_selection(id: NodeId) {
    REPORTED.with(|r| r.borrow_mut().remove(&id));
}
