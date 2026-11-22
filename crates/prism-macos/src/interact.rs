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
    NSEvent, NSEventMask, NSEventModifierFlags, NSFont, NSImage, NSImageView, NSPanel, NSPopover,
    NSPopoverBehavior, NSTextFieldCell, NSUserInterfaceLayoutDirection, NSView, NSViewController,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow,
    NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSRectEdge, NSSize, NSString,
};
use prism_core::controls::{OverlayConfig, RadioConfig};
use prism_core::{KeyEvent, Modifiers, NodeId, OverlayKind, PopoverAlign, PopoverSide};

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
        edges: None,
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
/// Children drawn outside a view that doesn't clip are hit there too (#213),
/// like on the web: a handle sticking out of a grid takes its clicks.
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
    if !inside && clips(view) {
        return std::ptr::null_mut();
    }
    let subs: Vec<Retained<NSView>> = view.subviews().iter().collect();
    for sub in subs.into_iter().rev() {
        if is_no_hit(&sub) {
            continue;
        }
        if let Some(found) = sub.hitTest(p) {
            // Text in a region that moves the window moves it too (#67):
            // the region takes the press, not the label.
            if view.moves_window() && passive_text(&found, view) {
                break;
            }
            return Retained::autorelease_return(found);
        }
    }
    // Outside its bounds: only its overflowing children, never itself.
    if !inside || view.ivars().pass_through.get() {
        return std::ptr::null_mut();
    }
    let me: &NSView = view;
    Retained::autorelease_return(objc2::Message::retain(me))
}

/// Whether `found` is text that takes no press (not editable, selectable
/// or clickable, no link) with nothing that takes presses between it and
/// `region`: a press on it belongs to `region`.
fn passive_text(found: &NSView, region: &PrismView) -> bool {
    let Some(field) = found.downcast_ref::<objc2_app_kit::NSTextField>() else {
        return false;
    };
    if field.isEditable() || field.isSelectable() {
        return false;
    }
    if let Some(label) = found.downcast_ref::<crate::label::PrismLabel>()
        && label.interactive()
    {
        return false;
    }
    let mut up = unsafe { found.superview() };
    while let Some(v) = up {
        if &*v == region as &NSView {
            return true;
        }
        if v.downcast_ref::<PrismView>()
            .is_some_and(|p| p.takes_presses())
        {
            return false;
        }
        up = unsafe { v.superview() };
    }
    false
}

/// Whether `view` crops its children (`clip`, a picture): then nothing of
/// them is drawn, or hit, outside it.
fn clips(view: &NSView) -> bool {
    view.layer().is_some_and(|l| l.masksToBounds())
}

/// The part of `view` the pointer can reach (#213): its bounds, cut by the
/// ancestors that crop it (scroll views, clipping Prism views, other AppKit
/// views), but not by Prism views it overflows without clipping.
fn reachable_rect(view: &NSView) -> NSRect {
    let mut r = view.bounds();
    let mut child: Retained<NSView> = objc2::Message::retain(view);
    while let Some(sup) = unsafe { child.superview() } {
        let open = sup.isKindOfClass(PrismView::class()) && !clips(&sup);
        if !open {
            let b = view.convertRect_fromView(sup.bounds(), Some(&sup));
            r = intersect(r, b);
        }
        child = sup;
    }
    r
}

fn intersect(a: NSRect, b: NSRect) -> NSRect {
    let x0 = a.origin.x.max(b.origin.x);
    let y0 = a.origin.y.max(b.origin.y);
    let x1 = (a.origin.x + a.size.width).min(b.origin.x + b.size.width);
    let y1 = (a.origin.y + a.size.height).min(b.origin.y + b.size.height);
    NSRect::new(
        NSPoint::new(x0, y0),
        objc2_foundation::NSSize::new((x1 - x0).max(0.), (y1 - y0).max(0.)),
    )
}

/// Prism's hover tracking areas on `view` follow what the pointer can
/// reach (#213): AppKit's visible rect when it covers that
/// (`InVisibleRect`, kept up to date by AppKit), an explicit rect covering
/// the overflowing part otherwise (recomputed in
/// `updateTrackingAreas`, which AppKit calls when the view moves or
/// scrolls). Returns the area replacing `own` (the view's pointer area).
pub(crate) fn refresh_tracking(
    view: &NSView,
    own: Option<&objc2_app_kit::NSTrackingArea>,
) -> Option<Retained<objc2_app_kit::NSTrackingArea>> {
    use objc2_app_kit::{NSTrackingArea, NSTrackingAreaOptions as O};
    let reach = reachable_rect(view);
    // What AppKit tracks with `InVisibleRect`: since macOS 14 views don't
    // clip by default and it already covers the overflow; before, it stops
    // at the parent's bounds.
    let visible = intersect(view.bounds(), view.visibleRect());
    let same = |a: NSRect, b: NSRect| {
        (a.origin.x - b.origin.x).abs() < 0.5
            && (a.origin.y - b.origin.y).abs() < 0.5
            && (a.size.width - b.size.width).abs() < 0.5
            && (a.size.height - b.size.height).abs() < 0.5
    };
    let overflows = !same(reach, visible);
    let mut replaced = None;
    for area in view.trackingAreas().iter() {
        let options = area.options();
        // Only Prism's: hover (and moves) for the view or for its states.
        if !options.contains(O::MouseEnteredAndExited) {
            continue;
        }
        let in_visible = options.contains(O::InVisibleRect);
        let (rect, options) = match (overflows, in_visible) {
            (false, true) => continue,
            (true, false) if same(area.rect(), reach) => continue,
            (true, _) => (reach, options & !O::InVisibleRect),
            (false, false) => (NSRect::ZERO, options | O::InVisibleRect),
        };
        let owner = area.owner();
        let info = area.userInfo();
        let new = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                <NSTrackingArea as objc2::AllocAnyThread>::alloc(),
                rect,
                options,
                owner.as_deref(),
                info.as_deref(),
            )
        };
        view.removeTrackingArea(&area);
        view.addTrackingArea(&new);
        if own.is_some_and(|o| std::ptr::eq(o, &*area)) {
            replaced = Some(new);
        }
    }
    replaced
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

define_class!(
    /// A bare dialog's window (#166): borderless and transparent, yet it
    /// becomes key (a plain borderless window cannot), so its fields take
    /// the keys and Escape reaches its content.
    #[unsafe(super(NSPanel, NSWindow, objc2_app_kit::NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismDialogPanel"]
    pub struct PrismDialogPanel;

    impl PrismDialogPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
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
/// and its views still take clicks (`acceptsFirstMouse`). A bare dialog
/// (#166) is a borderless, transparent child panel too, one that becomes
/// key: no sheet or popover box around the app's panel.
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
    /// background (a menu material with rounded corners), or a bare
    /// dialog's panel (a [`PrismDialogPanel`]) and its plain content view.
    panel: RefCell<Option<(Retained<NSPanel>, Retained<NSView>)>>,
    /// Whether the panel is the window it shows in.
    in_panel: Cell<bool>,
    /// A placed dialog's event monitor (#79): presses outside close it and
    /// go no further, so the dialog stays modal.
    outside: RefCell<Option<Retained<AnyObject>>>,
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
            outside: RefCell::new(None),
        }
    }

    /// While a dialog shown as a popover is open, a press on the window
    /// behind it (`page`) closes it and reaches nothing else: AppKit's
    /// transient popovers close on an outside click but may hand it to what
    /// was clicked. Presses in the dialog, and in menus or popovers opened
    /// from it, go on as usual.
    fn hold_outside_presses(&self, popover: &Retained<NSPopover>, page: Retained<NSWindow>) {
        let popover = popover.clone();
        self.hold_presses_behind(page, true, move || {
            if !popover.isShown() {
                return false;
            }
            // SAFETY: the sender may be nil.
            unsafe { popover.performClose(None) };
            true
        });
    }

    /// While open, presses on the window behind (`page`) go to `outside`,
    /// and, when it says it handled them, no further if `hold` (a modal
    /// dialog) or on to what they landed on otherwise (an inline popover,
    /// #182).
    fn hold_presses_behind(
        &self,
        page: Retained<NSWindow>,
        hold: bool,
        outside: impl Fn() -> bool + 'static,
    ) {
        self.release_outside_presses();
        let block =
            block2::RcBlock::new(move |event: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
                let e = unsafe { event.as_ref() };
                let mtm = MainThreadMarker::new().expect("main thread");
                let behind = e.window(mtm).is_some_and(|w| *w == *page);
                if !behind || !outside() || !hold {
                    return event.as_ptr();
                }
                std::ptr::null_mut()
            });
        let mask =
            NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown;
        let monitor =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
        *self.outside.borrow_mut() = monitor;
    }

    fn release_outside_presses(&self) {
        if let Some(m) = self.outside.borrow_mut().take() {
            unsafe { NSEvent::removeMonitor(&m) };
        }
    }

    /// Shown in a panel (see [`MacOverlay`]): a popover that leaves the
    /// focus to its anchor, and a bare dialog (#166).
    fn panel_kind(c: OverlayConfig) -> bool {
        (c.kind == OverlayKind::Popover && c.keep_focus)
            || (c.kind == OverlayKind::Dialog && c.bare)
    }

    /// Shown in an `NSPopover`: popovers, and dialogs placed beside their
    /// anchor (#79), on the platform's own anchored panel.
    fn popover_kind(c: OverlayConfig) -> bool {
        c.anchored_side().is_some()
    }

    pub(crate) fn set_config(&self, config: &str) {
        let c = OverlayConfig::decode(config);
        self.host.ivars().overlay_fixed.set(c.fixed);
        // Shown the other way: presented again by the next layout.
        let old = self.config.get();
        let moved = !self.in_panel.get() && (c.side, c.align) != (old.side, old.align);
        if self.presented.get()
            && (Self::panel_kind(c) != self.in_panel.get()
                || c.kind != old.kind
                || Self::popover_kind(c) != Self::popover_kind(old)
                || moved)
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
            _ if Self::panel_kind(self.config.get()) => self.show_panel(size),
            _ if Self::popover_kind(self.config.get()) => self.show_popover(size, delegate),
            // No on-screen keyboard: never in the tree (#242).
            OverlayKind::Keyboard => {}
            OverlayKind::Sheet | OverlayKind::Dialog | OverlayKind::Popover => {
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
        let c = self.config.get();
        let flipped = parent.isFlipped();
        let rtl =
            parent.userInterfaceLayoutDirection() == NSUserInterfaceLayoutDirection::RightToLeft;
        // AppKit shows no popover for an anchor scrolled out of view: bring
        // it, and the room the popover takes below or above it, into view.
        let b = parent.bounds();
        let room = match c.side {
            PopoverSide::Bottom | PopoverSide::Top => size.height + 16.,
            PopoverSide::Trailing | PopoverSide::Leading => 0.,
        };
        let down = (c.side == PopoverSide::Bottom) == flipped;
        parent.scrollRectToVisible(NSRect::new(
            NSPoint::new(
                b.origin.x,
                if down { b.origin.y } else { b.origin.y - room },
            ),
            NSSize::new(b.size.width, b.size.height + room),
        ));
        // On the asked side (AppKit flips it without room there), its
        // arrow on the part of the anchor it lines up with (#113).
        popover.showRelativeToRect_ofView_preferredEdge(
            popover_rect(b, size, c.side, c.align, rtl, flipped),
            &parent,
            popover_edge(c.side, rtl, flipped),
        );
        // A dialog stays modal beside its anchor (#79).
        if c.kind == OverlayKind::Dialog
            && behavior(c) == NSPopoverBehavior::Transient
            && let Some(page) = parent.window()
        {
            self.hold_outside_presses(&popover, page);
        }
        *self.popover.borrow_mut() = Some(popover);
    }

    /// Shows a `keep_focus` popover: a borderless panel, a child of the
    /// anchor's window (it moves with it), that never becomes key. A bare
    /// dialog's panel becomes key instead, and holds the presses behind it.
    fn show_panel(&self, size: NSSize) {
        let Some(parent) = (unsafe { self.anchor.superview() }) else {
            return;
        };
        let Some(window) = parent.window() else {
            return;
        };
        let mtm = self.host.mtm();
        let dialog = self.config.get().kind == OverlayKind::Dialog;
        let cached = self
            .panel
            .borrow_mut()
            .take()
            .filter(|(p, _)| p.isKindOfClass(PrismDialogPanel::class()) == dialog);
        let (panel, background) = cached.unwrap_or_else(|| {
            if dialog {
                let panel: Retained<PrismDialogPanel> = unsafe {
                    msg_send![
                        PrismDialogPanel::alloc(mtm),
                        initWithContentRect: NSRect::new(NSPoint::ZERO, size),
                        styleMask: NSWindowStyleMask::Borderless,
                        backing: NSBackingStoreType::Buffered,
                        defer: false,
                    ]
                };
                let panel = Retained::into_super(panel);
                unsafe { panel.setReleasedWhenClosed(false) };
                panel.setHidesOnDeactivate(false);
                // No box: the app's content is the whole panel.
                panel.setOpaque(false);
                panel.setBackgroundColor(Some(&NSColor::clearColor()));
                panel.setHasShadow(false);
                let background = NSView::new(mtm);
                panel.setContentView(Some(&background));
                return (panel, background);
            }
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
            (panel, Retained::into_super(background))
        });
        if unsafe { self.host.superview() }.as_deref() != Some(&*background) {
            self.host.removeFromSuperview();
            background.addSubview(&self.host);
        }
        self.presented.set(true);
        self.in_panel.set(true);
        *self.panel.borrow_mut() = Some((panel.clone(), background));
        self.place_panel(size);
        // SAFETY: both windows are alive; `dismiss` detaches the panel.
        unsafe { window.addChildWindow_ordered(&panel, NSWindowOrderingMode::Above) };
        if !dialog {
            panel.orderFront(None);
            // A press anywhere else in the window closes it and still
            // reaches what it landed on (the anchor's field, #182).
            if self.config.get().light_dismiss_beside_focus() {
                let shown = panel.clone();
                let id = self.id;
                self.hold_presses_behind(window, false, move || {
                    if !shown.isVisible() {
                        return false;
                    }
                    crate::imp::dispatch(prism_core::UiEvent::Dismiss(
                        id,
                        prism_core::DismissReason::Outside,
                    ));
                    true
                });
            }
            return;
        }
        // Modal like any dialog: the keys go to it (Escape reaches the
        // host, `cancelOperation:`), presses behind close it and go no
        // further.
        panel.makeKeyAndOrderFront(None);
        panel.makeFirstResponder(Some(&self.host));
        let (host, shown) = (self.host.clone(), panel.clone());
        let id = self.id;
        self.hold_presses_behind(window, true, move || {
            if !shown.isVisible() {
                return false;
            }
            if !host.ivars().overlay_fixed.get() {
                crate::imp::dispatch(prism_core::UiEvent::Dismiss(
                    id,
                    prism_core::DismissReason::Outside,
                ));
            }
            true
        });
    }

    /// On its side of the anchor (the opposite one without room there), at
    /// the content's size.
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
        let c = self.config.get();
        let rtl =
            parent.userInterfaceLayoutDirection() == NSUserInterfaceLayoutDirection::RightToLeft;
        let frame = if c.kind == OverlayKind::Dialog {
            let content = window.contentRectForFrameRect(window.frame());
            dialog_panel_frame(anchor, size, content, rtl, c)
        } else {
            panel_frame(
                anchor,
                size,
                window.screen().map(|s| s.visibleFrame()),
                rtl,
                c.side,
                c.align,
            )
        };
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
            self.release_outside_presses();
            if let Some((p, _)) = self.panel.borrow().as_ref() {
                let key = p.isKeyWindow();
                if let Some(parent) = p.parentWindow() {
                    parent.removeChildWindow(p);
                    // A bare dialog gives the keys back to the page.
                    if key {
                        parent.makeKeyWindow();
                    }
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
        self.release_outside_presses();
        if let Some(p) = self.popover.borrow_mut().take() {
            p.close();
        }
    }

    /// The user closed the popover (Escape, a click outside).
    pub(crate) fn popover_closed(&self) -> bool {
        self.release_outside_presses();
        self.popover.borrow_mut().take();
        self.presented.replace(false)
    }
}

/// A side of the anchor on screen, once leading and trailing are resolved.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Edge {
    Below,
    Above,
    Left,
    Right,
}

impl Edge {
    fn of(side: PopoverSide, rtl: bool) -> Self {
        match side {
            PopoverSide::Bottom => Edge::Below,
            PopoverSide::Top => Edge::Above,
            PopoverSide::Trailing if rtl => Edge::Left,
            PopoverSide::Trailing => Edge::Right,
            PopoverSide::Leading if rtl => Edge::Right,
            PopoverSide::Leading => Edge::Left,
        }
    }

    fn opposite(self) -> Self {
        match self {
            Edge::Below => Edge::Above,
            Edge::Above => Edge::Below,
            Edge::Left => Edge::Right,
            Edge::Right => Edge::Left,
        }
    }
}

/// Where a `keep_focus` popover's panel goes, in screen coordinates
/// (y up): on `side` of `anchor` with a 6 point gap (the opposite side
/// when the screen has no room there), lined up as `align` says (leading
/// or top edges for `Start`), kept inside `screen`.
pub(crate) fn panel_frame(
    anchor: NSRect,
    size: NSSize,
    screen: Option<NSRect>,
    rtl: bool,
    side: PopoverSide,
    align: PopoverAlign,
) -> NSRect {
    let gap = 6.;
    let (x0, x1) = (anchor.origin.x, anchor.origin.x + anchor.size.width);
    let (y0, y1) = (anchor.origin.y, anchor.origin.y + anchor.size.height);
    let fits = |e: Edge| {
        let Some(s) = screen else { return true };
        match e {
            Edge::Below => y0 - gap - size.height >= s.origin.y,
            Edge::Above => y1 + gap + size.height <= s.origin.y + s.size.height,
            Edge::Left => x0 - gap - size.width >= s.origin.x,
            Edge::Right => x1 + gap + size.width <= s.origin.x + s.size.width,
        }
    };
    let mut edge = Edge::of(side, rtl);
    if !fits(edge) && fits(edge.opposite()) {
        edge = edge.opposite();
    }
    let (mut x, mut y) = match edge {
        Edge::Below | Edge::Above => {
            let (start, end) = if rtl {
                (x1 - size.width, x0)
            } else {
                (x0, x1 - size.width)
            };
            let x = match align {
                PopoverAlign::Start => start,
                PopoverAlign::Center => x0 + (anchor.size.width - size.width) / 2.,
                PopoverAlign::End => end,
            };
            let y = if edge == Edge::Below {
                y0 - gap - size.height
            } else {
                y1 + gap
            };
            (x, y)
        }
        Edge::Left | Edge::Right => {
            let x = if edge == Edge::Right {
                x1 + gap
            } else {
                x0 - gap - size.width
            };
            // y goes up: tops lined up is the anchor's max y.
            let y = match align {
                PopoverAlign::Start => y1 - size.height,
                PopoverAlign::Center => y0 + (anchor.size.height - size.height) / 2.,
                PopoverAlign::End => y0,
            };
            (x, y)
        }
    };
    if let Some(s) = screen {
        x = x
            .min(s.origin.x + s.size.width - size.width)
            .max(s.origin.x);
        y = y
            .min(s.origin.y + s.size.height - size.height)
            .max(s.origin.y);
    }
    NSRect::new(NSPoint::new(x, y), size)
}

/// Where a bare dialog's panel goes (#166), in screen coordinates (y up):
/// beside its anchor as [`place_beside`](prism_core::controls::place_beside)
/// says when it is placed, inside `window` (its content area), 8 points
/// away; centered on the window otherwise, or when neither side has room.
pub(crate) fn dialog_panel_frame(
    anchor: NSRect,
    size: NSSize,
    window: NSRect,
    rtl: bool,
    c: OverlayConfig,
) -> NSRect {
    let (wx, wy) = (window.origin.x, window.origin.y);
    let (ww, wh) = (window.size.width, window.size.height);
    // Screen y up to the window's y down, and back (the same flip).
    let flip = |y: f64, h: f64| wy + wh - y - h;
    let beside = c
        .placed
        .then(|| {
            prism_core::controls::place_beside(
                (
                    (anchor.origin.x - wx) as f32,
                    flip(anchor.origin.y, anchor.size.height) as f32,
                    anchor.size.width as f32,
                    anchor.size.height as f32,
                ),
                (size.width as f32, size.height as f32),
                (0., 0., ww as f32, wh as f32),
                c.side,
                c.align,
                rtl,
                8.,
            )
        })
        .flatten();
    let origin = match beside {
        Some((x, y)) => NSPoint::new(wx + x as f64, flip(y as f64, size.height)),
        None => NSPoint::new(
            wx + ((ww - size.width) / 2.).round(),
            wy + ((wh - size.height) / 2.).round(),
        ),
    };
    NSRect::new(origin, size)
}

/// The edge of the anchor an `NSPopover` opens on (AppKit takes the
/// opposite one when the screen has no room there).
pub(crate) fn popover_edge(side: PopoverSide, rtl: bool, flipped: bool) -> NSRectEdge {
    match Edge::of(side, rtl) {
        Edge::Below if flipped => NSRectEdge::MaxY,
        Edge::Below => NSRectEdge::MinY,
        Edge::Above if flipped => NSRectEdge::MinY,
        Edge::Above => NSRectEdge::MaxY,
        Edge::Left => NSRectEdge::MinX,
        Edge::Right => NSRectEdge::MaxX,
    }
}

/// The part of the anchor's `bounds` an `NSPopover` points at: AppKit
/// centers the popover on it, so a part as long as the popover at the
/// anchor's start, center or end lines their edges up. A popover longer
/// than the anchor is centered on it (its arrow stays on the anchor).
pub(crate) fn popover_rect(
    bounds: NSRect,
    size: NSSize,
    side: PopoverSide,
    align: PopoverAlign,
    rtl: bool,
    flipped: bool,
) -> NSRect {
    let o = bounds.origin;
    let (w, h) = (bounds.size.width, bounds.size.height);
    // Along the edge: x below or above the anchor, y beside it.
    let (along, len, from_min) = if side.beside() {
        // Start is the top: min y in a flipped view.
        (h, size.height, flipped)
    } else {
        (w, size.width, !rtl)
    };
    let (offset, part) = align.arrow_span(along, len, from_min);
    if side.beside() {
        NSRect::new(NSPoint::new(o.x, o.y + offset), NSSize::new(w, part))
    } else {
        NSRect::new(NSPoint::new(o.x + offset, o.y), NSSize::new(part, h))
    }
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
        // Left out of macOS trees (#242).
        OverlayKind::Keyboard => (w, 0.),
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
        let (b, s) = (PopoverSide::Bottom, PopoverAlign::Start);
        let f = panel_frame(anchor, size, Some(screen), false, b, s);
        assert_eq!((f.origin.x, f.origin.y), (100., 394.));
        let rtl = panel_frame(anchor, size, Some(screen), true, b, s);
        assert_eq!(rtl.origin.x, 200.);
        // No room below: above the anchor.
        let low = NSRect::new(NSPoint::new(900., 40.), NSSize::new(300., 24.));
        let f = panel_frame(low, size, Some(screen), false, b, s);
        assert_eq!((f.origin.x, f.origin.y), (800., 70.));
    }

    #[test]
    fn placed_panels_follow_their_side_and_alignment() {
        let screen = NSRect::new(NSPoint::ZERO, NSSize::new(1000., 800.));
        let size = NSSize::new(200., 100.);
        let at = |anchor: NSRect, rtl, side, align| {
            let f = panel_frame(anchor, size, Some(screen), rtl, side, align);
            (f.origin.x, f.origin.y)
        };
        use PopoverAlign as A;
        use PopoverSide as S;
        // A menu item 300 × 24 whose top is at y 524.
        let item = NSRect::new(NSPoint::new(100., 500.), NSSize::new(300., 24.));
        // A submenu: beside it, tops lined up (y up: 524 - 100).
        assert_eq!(at(item, false, S::Trailing, A::Start), (406., 424.));
        assert_eq!(at(item, false, S::Trailing, A::End), (406., 500.));
        assert_eq!(at(item, false, S::Trailing, A::Center), (406., 462.));
        // No room on the left: it flips to the right.
        assert_eq!(at(item, false, S::Leading, A::Start), (406., 424.));
        // Right to left: trailing is the left side.
        let far = NSRect::new(NSPoint::new(500., 500.), NSSize::new(300., 24.));
        assert_eq!(at(far, true, S::Trailing, A::Start), (294., 424.));
        // Near the right edge, a trailing submenu flips to the left.
        assert_eq!(at(far, false, S::Trailing, A::Start), (294., 424.));
        // Below, trailing edges lined up.
        assert_eq!(at(item, false, S::Bottom, A::End), (200., 394.));
        assert_eq!(at(item, false, S::Bottom, A::Center), (150., 394.));
        assert_eq!(at(item, true, S::Bottom, A::End), (100., 394.));
        // Above.
        assert_eq!(at(item, false, S::Top, A::Start), (100., 530.));
    }

    #[test]
    fn bare_dialogs_go_beside_their_anchor_or_in_the_middle() {
        // A window content area 1000 × 800 at (100, 50) on screen, y up.
        let window = NSRect::new(NSPoint::new(100., 50.), NSSize::new(1000., 800.));
        let size = NSSize::new(320., 200.);
        // A 200 × 40 anchor whose top left is (150, 100) in the window
        // (y down): on screen its bottom is at 50 + 800 - 140.
        let anchor = NSRect::new(NSPoint::new(250., 710.), NSSize::new(200., 40.));
        let placed = OverlayConfig {
            kind: OverlayKind::Dialog,
            side: PopoverSide::Trailing,
            placed: true,
            bare: true,
            ..Default::default()
        };
        let f = dialog_panel_frame(anchor, size, window, false, placed);
        // 8 points after the anchor, tops lined up.
        assert_eq!((f.origin.x, f.origin.y), (458., 550.));
        assert_eq!(f.size, size);
        // Right to left: trailing is the left, with no room: the right.
        let f = dialog_panel_frame(anchor, size, window, true, placed);
        assert_eq!(f.origin.x, 458.);
        // Not placed, or no room on either side: centered on the window.
        let centered = OverlayConfig {
            placed: false,
            ..placed
        };
        let f = dialog_panel_frame(anchor, size, window, false, centered);
        assert_eq!((f.origin.x, f.origin.y), (440., 350.));
        let wide = NSSize::new(900., 200.);
        let f = dialog_panel_frame(anchor, wide, window, false, placed);
        assert_eq!((f.origin.x, f.origin.y), (150., 350.));
    }

    #[test]
    fn popovers_point_at_the_part_of_the_anchor_they_line_up_with() {
        // A flipped 300 × 24 anchor (y down), a 200 × 100 popover.
        let bounds = NSRect::new(NSPoint::ZERO, NSSize::new(300., 24.));
        let size = NSSize::new(200., 100.);
        let r = |side, align, rtl| {
            let r = popover_rect(bounds, size, side, align, rtl, true);
            (r.origin.x, r.origin.y, r.size.width, r.size.height)
        };
        use PopoverAlign as A;
        use PopoverSide as S;
        assert_eq!(r(S::Bottom, A::Start, false), (0., 0., 200., 24.));
        assert_eq!(r(S::Bottom, A::End, false), (100., 0., 200., 24.));
        assert_eq!(r(S::Bottom, A::Center, false), (50., 0., 200., 24.));
        assert_eq!(r(S::Bottom, A::Start, true), (100., 0., 200., 24.));
        // Taller than the anchor beside it: centered on it.
        assert_eq!(r(S::Trailing, A::Start, false), (0., 0., 300., 24.));
        let tall = NSRect::new(NSPoint::ZERO, NSSize::new(40., 300.));
        let e = popover_rect(tall, size, S::Trailing, A::End, false, true);
        assert_eq!((e.origin.y, e.size.height), (200., 100.));
        assert_eq!(popover_edge(S::Bottom, false, true), NSRectEdge::MaxY);
        assert_eq!(popover_edge(S::Top, false, true), NSRectEdge::MinY);
        assert_eq!(popover_edge(S::Trailing, false, true), NSRectEdge::MaxX);
        assert_eq!(popover_edge(S::Trailing, true, true), NSRectEdge::MinX);
        assert_eq!(popover_edge(S::Leading, false, false), NSRectEdge::MinX);
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

/// Where `tv`'s caret is (#211): at the start or end of its text, on its
/// first or last visual line (wrapping included), by comparing the caret's
/// line box with the text's first and last ones.
pub(crate) fn caret_edges(tv: &objc2_app_kit::NSTextView) -> prism_core::CaretEdges {
    use objc2_foundation::{NSRange, NSRect};
    let r = tv.selectedRange();
    if r.length > 0 {
        return prism_core::CaretEdges::default();
    }
    let len = tv.string().length();
    // SAFETY: a null `actualRange` is allowed.
    let rect = |at: usize| unsafe {
        objc2_app_kit::NSTextInputClient::firstRectForCharacterRange_actualRange(
            tv,
            NSRange::new(at.min(len), 0),
            std::ptr::null_mut(),
        )
    };
    let same_line = |a: NSRect, b: NSRect| {
        let h = a.size.height.max(b.size.height).max(1.);
        let mid = |r: NSRect| r.origin.y + r.size.height / 2.;
        (mid(a) - mid(b)).abs() < h / 2.
    };
    let caret = rect(r.location);
    prism_core::CaretEdges {
        start: r.location == 0,
        end: r.location >= len,
        first_line: same_line(caret, rect(0)),
        last_line: same_line(caret, rect(len)),
    }
}

/// Selects `start..end` (characters) in `tv`, clamped to its text.
pub(crate) fn select_text(tv: &objc2_app_kit::NSTextView, start: usize, end: usize) {
    let text = tv.string().to_string();
    let at = |c: usize| prism_native::fields::chars_to_utf16(&text, c);
    let (s, e) = (at(start), at(end.max(start)));
    tv.setSelectedRange(objc2_foundation::NSRange::new(s, e - s));
}

/// Draws `start..end` (characters) of `tv` like a selection without
/// selecting it (#231), `start == end` removing it: a temporary attribute
/// of its layout manager, so the text and its own selection are left alone.
pub(crate) fn highlight_text(tv: &objc2_app_kit::NSTextView, start: usize, end: usize) {
    use objc2_foundation::NSRange;
    let Some(lm) = (unsafe { tv.layoutManager() }) else {
        return;
    };
    let text = tv.string().to_string();
    let at = |c: usize| prism_native::fields::chars_to_utf16(&text, c);
    let key = unsafe { objc2_app_kit::NSBackgroundColorAttributeName };
    lm.removeTemporaryAttribute_forCharacterRange(key, NSRange::new(0, at(usize::MAX)));
    if start < end {
        let (s, e) = (at(start), at(end));
        let color = objc2_app_kit::NSColor::selectedTextBackgroundColor();
        unsafe {
            lm.addTemporaryAttribute_value_forCharacterRange(key, &color, NSRange::new(s, e - s))
        };
    }
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
