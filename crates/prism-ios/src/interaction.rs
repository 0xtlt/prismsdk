//! Pointer, drag and key events, programmatic focus, runtime layers and
//! text input extras (#17, #18, #25) on UIKit.
//!
//! - Containers are `PrismContainerView`s (iOS, visionOS): they can become
//!   first responder when they listen to keys, let touches through when
//!   they are a layer's root, and report VoiceOver activation as a plain
//!   click (a tap reports its position). tvOS containers are
//!   `PrismFocusView`s (see `tv.rs`).
//! - Hover: `UIHoverGestureRecognizer` (iPadOS pointer, Apple Vision Pro
//!   gaze). Moves while touching: a zero-duration long press that never
//!   blocks other gestures. Drags: `UIPanGestureRecognizer`, which a scroll
//!   view's pan waits for. Pinches: `UIPinchGestureRecognizer` (fingers and
//!   an iPad trackpad), alongside a drag. A drag after a long press
//!   (#243): a `UILongPressGestureRecognizer` for fingers and pens (the pan
//!   keeps the iPad pointer), which lifts with an impact haptic and cancels
//!   the scroll views around it. Wheel: a scroll-only
//!   `UIPanGestureRecognizer` (an iPad trackpad or mouse, #172). tvOS has no
//!   pointer on views: none of these.
//! - Keys: `pressesBegan:` with `UIKey` (hardware keyboards) and the remote's
//!   press types (tvOS), on the view that listens and on the root view
//!   controller for global listeners; their releases (`pressesEnded:`,
//!   `pressesCancelled:`, #159) go where the press went.
//! - Clicks: a tap's modifier keys and `UITouch.tapCount` (#159).
//! - Layers: a pass-through `UIWindow` above the app's (above its sheets).

#[cfg_attr(target_os = "tvos", allow(unused_imports))]
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
#[cfg_attr(target_os = "tvos", allow(unused_imports))]
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_core_foundation::{CGPoint, CGRect};
#[cfg(not(target_os = "tvos"))]
use objc2_foundation::{NSArray, NSRange};
use objc2_foundation::{NSObject, NSObjectProtocol, NSSet, NSString};
#[cfg_attr(target_os = "tvos", allow(unused_imports))]
use objc2_ui_kit::{
    UIEvent, UIGestureRecognizer, UIGestureRecognizerState, UIPress, UIPressType, UIPressesEvent,
    UIResponder, UIView, UIViewController, UIWindow, UIWindowScene,
};
use prism_core::{KeyEvent, Modifiers, NodeId, UiEvent};

thread_local! {
    /// Nodes listening to keys while the focus is in them.
    static LOCAL_KEYS: RefCell<HashSet<NodeId>> = RefCell::new(HashSet::new());
    /// Nodes listening to every key of the window.
    static GLOBAL_KEYS: RefCell<HashSet<NodeId>> = RefCell::new(HashSet::new());
    /// `max_length` of text fields, by node.
    static MAX_LENGTH: RefCell<HashMap<NodeId, usize>> = RefCell::new(HashMap::new());
    /// Suggestions of text fields, by node.
    static SUGGESTIONS: RefCell<HashMap<NodeId, Vec<String>>> = RefCell::new(HashMap::new());
    /// Date pickers editing a time of day (shown in UTC).
    static TIME_OF_DAY: RefCell<HashSet<NodeId>> = RefCell::new(HashSet::new());
    /// A view waiting for the focus until it is on screen, and the tries left.
    static PENDING_FOCUS: RefCell<Option<(Retained<UIView>, u32)>> = const { RefCell::new(None) };
    /// tvOS: where the focus engine should go next (`preferredFocusEnvironments`).
    static FOCUS_TARGET: RefCell<Option<Retained<UIView>>> = const { RefCell::new(None) };
}

// ---------------------------------------------------------------------------
// Bookkeeping
// ---------------------------------------------------------------------------

/// Records which nodes listen to keys (`Patch::SetKeyEvents`).
pub fn set_keys(id: NodeId, local: bool, global: bool) {
    LOCAL_KEYS.with(|s| {
        let mut s = s.borrow_mut();
        if local { s.insert(id) } else { s.remove(&id) }
    });
    GLOBAL_KEYS.with(|s| {
        let mut s = s.borrow_mut();
        if global { s.insert(id) } else { s.remove(&id) }
    });
}

/// Forgets a destroyed node.
pub fn forget(id: NodeId) {
    set_keys(id, false, false);
    MAX_LENGTH.with(|m| m.borrow_mut().remove(&id));
    SUGGESTIONS.with(|m| m.borrow_mut().remove(&id));
    TIME_OF_DAY.with(|m| m.borrow_mut().remove(&id));
}

pub fn has_global_keys() -> bool {
    GLOBAL_KEYS.with(|s| !s.borrow().is_empty())
}

fn listens_locally(id: NodeId) -> bool {
    LOCAL_KEYS.with(|s| s.borrow().contains(&id))
}

pub fn set_max_length(id: NodeId, max: Option<u32>) {
    MAX_LENGTH.with(|m| match max {
        Some(n) => m.borrow_mut().insert(id, n as usize),
        None => m.borrow_mut().remove(&id),
    });
}

/// Whether an edit leaving `new_len` UTF-16 units (from `old_len`) fits the
/// field's `max_length` (like the web's `maxlength`; deleting always fits).
pub fn edit_fits(id: NodeId, old_len: usize, new_len: usize) -> bool {
    MAX_LENGTH.with(|m| {
        m.borrow()
            .get(&id)
            .is_none_or(|max| new_len <= *max || new_len <= old_len)
    })
}

pub fn set_suggestions(id: NodeId, list: Vec<String>) {
    SUGGESTIONS.with(|m| {
        if list.is_empty() {
            m.borrow_mut().remove(&id);
        } else {
            m.borrow_mut().insert(id, list);
        }
    });
}

#[cfg(not(target_os = "tvos"))]
pub fn set_time_of_day(id: NodeId, on: bool) {
    TIME_OF_DAY.with(|s| {
        let mut s = s.borrow_mut();
        if on { s.insert(id) } else { s.remove(&id) }
    });
}

#[cfg(not(target_os = "tvos"))]
pub fn is_time_of_day(id: NodeId) -> bool {
    TIME_OF_DAY.with(|s| s.borrow().contains(&id))
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

/// `UIKeyboardHIDUsage` codes with a name (UIKeyConstants.h).
fn named_key(code: isize) -> Option<String> {
    Some(
        match code {
            0x29 => "Escape",
            0x28 | 0x58 => "Enter",
            0x2B => "Tab",
            0x2A => "Backspace",
            0x4C => "Delete",
            0x4F => "ArrowRight",
            0x50 => "ArrowLeft",
            0x51 => "ArrowDown",
            0x52 => "ArrowUp",
            0x4A => "Home",
            0x4D => "End",
            0x4B => "PageUp",
            0x4E => "PageDown",
            0x65 => "ContextMenu",
            0x3A..=0x45 => return Some(format!("F{}", code - 0x3A + 1)),
            _ => return None,
        }
        .to_string(),
    )
}

/// UIKit's modifier flags as Prism's.
fn modifiers(flags: objc2_ui_kit::UIKeyModifierFlags) -> Modifiers {
    use objc2_ui_kit::UIKeyModifierFlags as F;
    Modifiers {
        shift: flags.contains(F::Shift),
        control: flags.contains(F::Control),
        alt: flags.contains(F::Alternate),
        meta: flags.contains(F::Command),
    }
}

/// What a press means as a Prism key: a hardware key, or the TV remote's
/// arrows, select ("Enter"), play/pause and Menu ("Escape").
pub fn key_event(press: &UIPress, mtm: MainThreadMarker) -> Option<KeyEvent> {
    if let Some(key) = press.key(mtm) {
        let modifiers = modifiers(key.modifierFlags());
        let name = match named_key(key.keyCode().0) {
            Some(n) => n,
            None => {
                let chars = if modifiers.control || modifiers.meta || modifiers.alt {
                    key.charactersIgnoringModifiers()
                } else {
                    key.characters()
                };
                let s = chars.to_string();
                if s.is_empty() || s.chars().any(char::is_control) {
                    return None;
                }
                s
            }
        };
        return Some(KeyEvent {
            key: name,
            modifiers,
            repeat: false,
            selection: None,
            edges: None,
        });
    }
    let name = match press.r#type() {
        UIPressType::UpArrow => "ArrowUp",
        UIPressType::DownArrow => "ArrowDown",
        UIPressType::LeftArrow => "ArrowLeft",
        UIPressType::RightArrow => "ArrowRight",
        UIPressType::Select => "Enter",
        UIPressType::PlayPause => "MediaPlayPause",
        UIPressType::Menu => "Escape",
        UIPressType::PageUp => "PageUp",
        UIPressType::PageDown => "PageDown",
        _ => return None,
    };
    Some(KeyEvent::new(name))
}

/// The node of the first responder (or the tvOS focused view), if Prism made it.
fn focused_node(window: Option<&UIWindow>) -> Option<(NodeId, bool)> {
    let window = window?;
    let responder = find_first_responder(window)?;
    let editing = responder.isKindOfClass(objc2_ui_kit::UITextField::class())
        || responder.isKindOfClass(objc2_ui_kit::UITextView::class());
    let mut v = Some(responder);
    while let Some(view) = v {
        if view.tag() > 0 {
            return Some((view.tag() as NodeId, editing));
        }
        v = view.superview();
    }
    None
}

fn find_first_responder(v: &UIView) -> Option<Retained<UIView>> {
    #[cfg(target_os = "tvos")]
    {
        if v.isFocused() {
            return Some(v.retain());
        }
    }
    #[cfg(not(target_os = "tvos"))]
    if v.isFirstResponder() {
        return Some(v.retain());
    }
    v.subviews().iter().find_map(|c| find_first_responder(&c))
}

/// Whether some node in `id`'s view chain listens to keys locally.
fn chain_listens(view: Option<Retained<UIView>>) -> bool {
    let mut v = view;
    while let Some(view) = v {
        if view.tag() > 0 && listens_locally(view.tag() as NodeId) {
            return true;
        }
        v = view.superview();
    }
    false
}

/// The press Prism handles in `presses`, if any: for a view listening
/// locally (`local`) or for global listeners (`None`).
fn wanted(
    presses: &NSSet<UIPress>,
    window: Option<&UIWindow>,
    local: Option<NodeId>,
    mtm: MainThreadMarker,
) -> Option<(Option<NodeId>, KeyEvent)> {
    let focused = focused_node(window);
    // In a text field: its selection before the key (#86).
    let field = focused
        .filter(|f| f.1)
        .and_then(|_| window.and_then(|w| find_first_responder(w)));
    let selection = field.as_deref().and_then(crate::forms::current_selection);
    // And where its caret is (#211).
    let edges = field.as_deref().and_then(crate::forms::current_edges);
    for press in presses.iter() {
        let Some(key) = key_event(&press, mtm) else {
            continue;
        };
        let key = KeyEvent {
            selection,
            edges,
            ..key
        };
        let remote_menu = press.key(mtm).is_none() && press.r#type() == UIPressType::Menu;
        match local {
            // The remote's Menu stays the system's Back for local listeners.
            Some(_) if remote_menu => continue,
            Some(id) => return Some((Some(focused.map_or(id, |f| f.0)), key)),
            None => {
                let target = focused.map(|f| f.0);
                let in_chain = focused.is_some()
                    && chain_listens(window.and_then(|w| find_first_responder(w)));
                if !has_global_keys() && !in_chain {
                    return None;
                }
                // Typing in a field is not a shortcut.
                let typing = focused.is_some_and(|f| f.1)
                    && key.is_character()
                    && !key.modifiers.control
                    && !key.modifiers.meta;
                if typing && !in_chain {
                    continue;
                }
                // Menu is the system's Back unless the app listens for it.
                if remote_menu && !has_global_keys() {
                    continue;
                }
                return Some((target, key));
            }
        }
    }
    None
}

/// Handles `presses` for a view (`local`: its node) or the root controller.
/// Returns whether they were reported (the caller then does not call super).
pub fn handle_presses(
    presses: &NSSet<UIPress>,
    responder: &UIResponder,
    local: Option<NodeId>,
    began: bool,
) -> bool {
    let mtm = responder.mtm();
    let window = window_of(responder);
    if !began {
        // Releases (#159) go to where their press went, wherever the focus
        // is now.
        let mut released = false;
        for press in presses.iter() {
            let Some(key) = key_event(&press, mtm) else {
                continue;
            };
            if let Some(target) = release_key(&key.key) {
                released = true;
                let key = KeyEvent {
                    repeat: false,
                    selection: None,
                    edges: None,
                    ..key
                };
                crate::imp::dispatch(UiEvent::KeyUp(target, key));
            }
        }
        return released || wanted(presses, window.as_deref(), local, mtm).is_some();
    }
    match wanted(presses, window.as_deref(), local, mtm) {
        Some((target, key)) => {
            HELD_KEYS.with(|h| {
                let mut h = h.borrow_mut();
                if !h.iter().any(|(k, _)| *k == key.key) {
                    h.push((key.key.clone(), target));
                }
            });
            crate::imp::dispatch(UiEvent::Key(target, key));
            true
        }
        None => false,
    }
}

thread_local! {
    /// Keys whose press was reported, and the node it went to (#159).
    static HELD_KEYS: RefCell<Vec<(String, Option<NodeId>)>> = const { RefCell::new(Vec::new()) };
}

/// The node a held key's press went to, or `None` if it was not reported.
fn release_key(key: &str) -> Option<Option<NodeId>> {
    HELD_KEYS.with(|h| {
        let mut h = h.borrow_mut();
        let i = h.iter().position(|(k, _)| k == key)?;
        Some(h.remove(i).1)
    })
}

/// The app stopped being active: UIKit may send no release for the keys
/// still held, so they are released now (#159).
pub fn release_all_keys() {
    let held = HELD_KEYS.with(|h| std::mem::take(&mut *h.borrow_mut()));
    for (key, target) in held {
        crate::imp::dispatch(UiEvent::KeyUp(target, KeyEvent::new(key)));
    }
}

fn window_of(responder: &UIResponder) -> Option<Retained<UIWindow>> {
    if let Some(v) = responder.downcast_ref::<UIView>() {
        return v.window();
    }
    if let Some(vc) = responder.downcast_ref::<UIViewController>() {
        return vc.viewIfLoaded().and_then(|v| v.window());
    }
    None
}

// ---------------------------------------------------------------------------
// Containers (iOS, visionOS)
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "tvos"))]
#[derive(Default)]
pub struct ContainerIvars {
    keys: Cell<bool>,
    pass_through: Cell<bool>,
    clickable: Cell<bool>,
    /// A table or one of its cells (#284).
    table: crate::table::TableIvars,
}

#[cfg(not(target_os = "tvos"))]
define_class!(
    /// A Prism container (`div`).
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismContainerView"]
    #[ivars = ContainerIvars]
    pub struct ContainerView;

    unsafe impl NSObjectProtocol for ContainerView {}

    impl ContainerView {
        #[unsafe(method(canBecomeFirstResponder))]
        fn can_become_first_responder(&self) -> bool {
            self.ivars().keys.get()
        }

        #[unsafe(method(pressesBegan:withEvent:))]
        fn presses_began(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            let local = self.ivars().keys.get().then_some(self.tag() as NodeId);
            if local.is_none() || !handle_presses(presses, self, local, true) {
                let _: () = unsafe { msg_send![super(self), pressesBegan: presses, withEvent: event] };
            }
        }

        #[unsafe(method(pressesEnded:withEvent:))]
        fn presses_ended(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            let local = self.ivars().keys.get().then_some(self.tag() as NodeId);
            if local.is_none() || !handle_presses(presses, self, local, false) {
                let _: () = unsafe { msg_send![super(self), pressesEnded: presses, withEvent: event] };
            }
        }

        #[unsafe(method(pressesCancelled:withEvent:))]
        fn presses_cancelled(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            let local = self.ivars().keys.get().then_some(self.tag() as NodeId);
            if local.is_none() || !handle_presses(presses, self, local, false) {
                let _: () =
                    unsafe { msg_send![super(self), pressesCancelled: presses, withEvent: event] };
            }
        }

        /// A layer's root takes no touch itself: only its children do.
        /// Children drawn outside a view that doesn't clip are hit there too
        /// (#213), like on the web.
        #[unsafe(method_id(hitTest:withEvent:))]
        fn hit_test(&self, point: CGPoint, event: Option<&UIEvent>) -> Option<Retained<UIView>> {
            let mut hit: Option<Retained<UIView>> =
                unsafe { msg_send![super(self), hitTest: point, withEvent: event] };
            if hit.is_none() {
                hit = overflow_hit(self, point, event);
            }
            let me: &UIView = self;
            if self.ivars().pass_through.get() && hit.as_deref().is_some_and(|h| std::ptr::eq(h, me)) {
                None
            } else {
                hit
            }
        }

        // Presses for `.active(|s| …)` (see `decor::press`).
        #[unsafe(method(touchesBegan:withEvent:))]
        fn touches_began(&self, touches: &AnyObject, event: Option<&AnyObject>) {
            let _: () = unsafe { msg_send![super(self), touchesBegan: touches, withEvent: event] };
            crate::decor::press(self.tag() as NodeId, true);
        }

        #[unsafe(method(touchesEnded:withEvent:))]
        fn touches_ended(&self, touches: &AnyObject, event: Option<&AnyObject>) {
            let _: () = unsafe { msg_send![super(self), touchesEnded: touches, withEvent: event] };
            crate::decor::press(self.tag() as NodeId, false);
        }

        #[unsafe(method(touchesCancelled:withEvent:))]
        fn touches_cancelled(&self, touches: &AnyObject, event: Option<&AnyObject>) {
            let _: () =
                unsafe { msg_send![super(self), touchesCancelled: touches, withEvent: event] };
            crate::decor::press(self.tag() as NodeId, false);
        }

        /// VoiceOver's double tap is a click without a position. A table's
        /// cell clicks its row.
        #[unsafe(method(accessibilityActivate))]
        fn accessibility_activate(&self) -> bool {
            if self.ivars().clickable.get() {
                crate::imp::dispatch(UiEvent::Click(self.tag() as NodeId));
                true
            } else if let Some(row) = crate::table::row_of(&self.ivars().table)
                .filter(|r| ivars(r).is_some_and(|i| i.clickable.get()))
            {
                crate::imp::dispatch(UiEvent::Click(row.tag() as NodeId));
                true
            } else {
                false
            }
        }

        // Tables (#284): `UIAccessibilityContainerDataTable` and its cells,
        // see `table.rs`.
        #[unsafe(method(accessibilityRowCount))]
        fn accessibility_row_count(&self) -> isize {
            crate::table::row_count(&self.ivars().table)
        }

        #[unsafe(method(accessibilityColumnCount))]
        fn accessibility_column_count(&self) -> isize {
            crate::table::column_count(&self.ivars().table)
        }

        #[unsafe(method_id(accessibilityDataTableCellElementForRow:column:))]
        fn accessibility_cell(&self, row: usize, column: usize) -> Option<Retained<UIView>> {
            crate::table::cell(&self.ivars().table, row, column)
        }

        #[unsafe(method_id(accessibilityHeaderElementsForColumn:))]
        fn accessibility_headers(&self, column: usize) -> Option<Retained<NSArray<UIView>>> {
            crate::table::headers(&self.ivars().table, column)
        }

        #[unsafe(method(accessibilityRowRange))]
        fn accessibility_row_range(&self) -> NSRange {
            crate::table::row_range(&self.ivars().table)
        }

        #[unsafe(method(accessibilityColumnRange))]
        fn accessibility_column_range(&self) -> NSRange {
            crate::table::column_range(&self.ivars().table)
        }
    }
);

#[cfg(not(target_os = "tvos"))]
pub fn container(mtm: MainThreadMarker) -> Retained<UIView> {
    let this = ContainerView::alloc(mtm).set_ivars(ContainerIvars::default());
    let v: Retained<ContainerView> = unsafe { msg_send![super(this), init] };
    Retained::into_super(v)
}

/// The container ivars of `view`, if it is a Prism container.
#[cfg(not(target_os = "tvos"))]
fn ivars(view: &UIView) -> Option<&ContainerIvars> {
    view.downcast_ref::<ContainerView>().map(|c| c.ivars())
}

/// The table ivars of `view`, if it is a Prism container (#284).
#[cfg(not(target_os = "tvos"))]
pub(crate) fn table_ivars(view: &UIView) -> Option<&crate::table::TableIvars> {
    ivars(view).map(|i| &i.table)
}

/// The view can become first responder (or take the TV focus) and reports keys.
pub fn set_view_keys(view: &UIView, on: bool) {
    #[cfg(not(target_os = "tvos"))]
    if let Some(i) = ivars(view) {
        i.keys.set(on);
    }
    #[cfg(target_os = "tvos")]
    crate::tv::set_keys(view, on);
}

/// A layer root: touches go through where it has no child.
pub fn set_pass_through(view: &UIView, on: bool) {
    #[cfg(not(target_os = "tvos"))]
    if let Some(i) = ivars(view) {
        i.pass_through.set(on);
    }
    #[cfg(target_os = "tvos")]
    let _ = (view, on);
}

/// VoiceOver activation reports a click.
pub fn set_activatable(view: &UIView, on: bool) {
    #[cfg(not(target_os = "tvos"))]
    if let Some(i) = ivars(view) {
        i.clickable.set(on);
    }
    #[cfg(target_os = "tvos")]
    let _ = (view, on);
}

// ---------------------------------------------------------------------------
// Pointer and drag (not tvOS)
// ---------------------------------------------------------------------------

const HOVER: &str = "prism.hover";
const MOVE: &str = "prism.move";
const DRAG: &str = "prism.drag";
const MAGNIFY: &str = "prism.magnify";
const WHEEL: &str = "prism.wheel";
/// A drag after a long press (#243), for fingers and pens.
const HOLD: &str = "prism.hold";

fn has_name(g: &UIGestureRecognizer, name: &str) -> bool {
    g.name().is_some_and(|n| n.to_string() == name)
}

/// Adds or removes the hover, touch-move, pan, pinch and wheel recognizers
/// of `view`.
#[cfg(not(target_os = "tvos"))]
pub fn set_pointer_events(view: &UIView, mask: u8, target: &AnyObject) {
    use objc2_ui_kit::{
        UIHoverGestureRecognizer, UILongPressGestureRecognizer, UIPanGestureRecognizer,
        UIPinchGestureRecognizer, UIScrollTypeMask,
    };
    use prism_core::events::pointer_mask;
    let mtm = view.mtm();
    MASKS.with(|m| m.borrow_mut().insert(view.tag() as NodeId, mask));
    let existing: Vec<Retained<UIGestureRecognizer>> = view
        .gestureRecognizers()
        .map(|a| a.to_vec())
        .unwrap_or_default();
    for (bit, name) in [
        (pointer_mask::HOVER | pointer_mask::MOVE, HOVER),
        (pointer_mask::MOVE, MOVE),
        (pointer_mask::DRAG_ANY, DRAG),
        (pointer_mask::MAGNIFY, MAGNIFY),
        (pointer_mask::WHEEL, WHEEL),
    ] {
        let present = existing.iter().find(|g| has_name(g, name));
        let wanted = mask & bit != 0;
        match (present, wanted) {
            (Some(g), false) => view.removeGestureRecognizer(g),
            (None, true) => {
                let g: Retained<UIGestureRecognizer> = match name {
                    HOVER => Retained::into_super(unsafe {
                        UIHoverGestureRecognizer::initWithTarget_action(
                            UIHoverGestureRecognizer::alloc(mtm),
                            Some(target),
                            Some(sel!(onHover:)),
                        )
                    }),
                    MOVE => {
                        let g = unsafe {
                            UILongPressGestureRecognizer::initWithTarget_action(
                                UILongPressGestureRecognizer::alloc(mtm),
                                Some(target),
                                Some(sel!(onTouchMove:)),
                            )
                        };
                        g.setMinimumPressDuration(0.);
                        g.setAllowableMovement(f64::MAX);
                        Retained::into_super(g)
                    }
                    MAGNIFY => Retained::into_super(unsafe {
                        UIPinchGestureRecognizer::initWithTarget_action(
                            UIPinchGestureRecognizer::alloc(mtm),
                            Some(target),
                            Some(sel!(onPinch:)),
                        )
                    }),
                    WHEEL => {
                        let g = unsafe {
                            UIPanGestureRecognizer::initWithTarget_action(
                                UIPanGestureRecognizer::alloc(mtm),
                                Some(target),
                                Some(sel!(onWheel:)),
                            )
                        };
                        // Scrolling only (a trackpad's two fingers, a
                        // mouse wheel): no touch starts it.
                        g.setAllowedScrollTypesMask(UIScrollTypeMask::All);
                        g.setMaximumNumberOfTouches(0);
                        Retained::into_super(g)
                    }
                    _ => Retained::into_super(unsafe {
                        UIPanGestureRecognizer::initWithTarget_action(
                            UIPanGestureRecognizer::alloc(mtm),
                            Some(target),
                            Some(sel!(onPan:)),
                        )
                    }),
                };
                g.setName(Some(&NSString::from_str(name)));
                // Taps, scrolling and buttons keep working alongside.
                g.setCancelsTouchesInView(name == DRAG || name == MAGNIFY);
                g.setDelaysTouchesEnded(false);
                let delegate: &AnyObject = target;
                let _: () = unsafe { msg_send![&*g, setDelegate: delegate] };
                view.addGestureRecognizer(&g);
            }
            _ => {}
        }
    }
    apply_hold(view, target);
}

/// How long a finger or a pen must be held still before the view's drag
/// starts (`drag_after_long_press`, #243), in milliseconds; 0: on movement.
#[cfg(not(target_os = "tvos"))]
pub fn set_drag_delay(view: &UIView, ms: u32, target: &AnyObject) {
    DELAYS.with(|d| {
        let mut d = d.borrow_mut();
        if ms == 0 {
            d.remove(&(view.tag() as NodeId))
        } else {
            d.insert(view.tag() as NodeId, ms)
        }
    });
    apply_hold(view, target);
}

/// With a hold, the pan only follows an iPad's pointer and a long press
/// takes fingers and pens; without one, the pan takes them all again.
#[cfg(not(target_os = "tvos"))]
fn apply_hold(view: &UIView, target: &AnyObject) {
    use objc2_foundation::{NSArray, NSNumber};
    use objc2_ui_kit::{UILongPressGestureRecognizer, UITouchType};
    let ms = DELAYS.with(|d| d.borrow().get(&(view.tag() as NodeId)).copied());
    let recognizers: Vec<Retained<UIGestureRecognizer>> = view
        .gestureRecognizers()
        .map(|a| a.to_vec())
        .unwrap_or_default();
    let pan = recognizers.iter().find(|g| has_name(g, DRAG));
    let hold = recognizers.iter().find(|g| has_name(g, HOLD));
    let types = |list: &[UITouchType]| {
        let numbers: Vec<Retained<NSNumber>> =
            list.iter().map(|t| NSNumber::new_isize(t.0)).collect();
        NSArray::from_retained_slice(&numbers)
    };
    match (pan, ms) {
        (Some(pan), Some(ms)) => {
            pan.setAllowedTouchTypes(&types(&[UITouchType::IndirectPointer]));
            let hold: Retained<UIGestureRecognizer> = match hold {
                Some(h) => h.clone(),
                None => {
                    let g = unsafe {
                        UILongPressGestureRecognizer::initWithTarget_action(
                            UILongPressGestureRecognizer::alloc(view.mtm()),
                            Some(target),
                            Some(sel!(onHoldDrag:)),
                        )
                    };
                    g.setName(Some(&NSString::from_str(HOLD)));
                    // Once lifted, the drag is not also a tap.
                    g.setCancelsTouchesInView(true);
                    g.setDelaysTouchesEnded(false);
                    g.setAllowedTouchTypes(&types(&[UITouchType::Direct, UITouchType::Pencil]));
                    let delegate: &AnyObject = target;
                    let _: () = unsafe { msg_send![&*g, setDelegate: delegate] };
                    view.addGestureRecognizer(&g);
                    Retained::into_super(g)
                }
            };
            if let Ok(lp) = hold.downcast::<UILongPressGestureRecognizer>() {
                lp.setMinimumPressDuration(ms as f64 / 1000.);
            }
        }
        (pan, _) => {
            if let Some(hold) = hold {
                view.removeGestureRecognizer(hold);
            }
            if let Some(pan) = pan {
                pan.setAllowedTouchTypes(&types(&[
                    UITouchType::Direct,
                    UITouchType::Indirect,
                    UITouchType::Pencil,
                    UITouchType::IndirectPointer,
                ]));
            }
        }
    }
}

/// A long press that drags (#243): it lifts (an impact haptic, the scroll
/// views around it stop), then reports like a pan.
#[cfg(not(target_os = "tvos"))]
pub fn on_hold(g: &UIGestureRecognizer) {
    use prism_core::{DragEvent, DragPhase, Point, PointerButton};
    let Some(view) = g.view() else { return };
    let id = view.tag() as NodeId;
    let phase = match g.state() {
        UIGestureRecognizerState::Began => DragPhase::Start,
        UIGestureRecognizerState::Changed => DragPhase::Move,
        UIGestureRecognizerState::Ended => DragPhase::End,
        UIGestureRecognizerState::Cancelled | UIGestureRecognizerState::Failed => DragPhase::Cancel,
        _ => return,
    };
    // In the window: the view may move while it is dragged (reordering).
    let window = g.locationInView(None);
    if phase == DragPhase::Start {
        let at = point(g.locationInView(Some(&view)));
        let mods = modifiers(g.modifierFlags());
        START.with(|s| {
            s.borrow_mut()
                .insert(id, (at, PointerButton::Primary, mods))
        });
        HOLD_AT.with(|h| h.borrow_mut().insert(id, window));
        lift(&view);
    }
    let (start, button, mods) = START.with(|s| s.borrow().get(&id).copied().unwrap_or_default());
    let from = HOLD_AT
        .with(|h| h.borrow().get(&id).copied())
        .unwrap_or(window);
    if matches!(phase, DragPhase::End | DragPhase::Cancel) {
        START.with(|s| s.borrow_mut().remove(&id));
        HOLD_AT.with(|h| h.borrow_mut().remove(&id));
    }
    crate::imp::dispatch(UiEvent::Drag(
        id,
        DragEvent {
            phase,
            start,
            translation: Point::new((window.x - from.x) as f32, (window.y - from.y) as f32),
            button,
            modifiers: mods,
        },
    ));
}

/// The pick-up (#243): the platform's impact haptic (none on visionOS), and
/// the scroll views around the view let go of the touch.
#[cfg(not(target_os = "tvos"))]
fn lift(view: &UIView) {
    if let Some(class) = objc2::runtime::AnyClass::get(c"UIImpactFeedbackGenerator") {
        unsafe {
            let generator: objc2::rc::Allocated<AnyObject> = msg_send![class, alloc];
            // UIImpactFeedbackStyleMedium.
            let generator: Option<Retained<AnyObject>> =
                msg_send![generator, initWithStyle: 1isize];
            if let Some(generator) = generator {
                let _: () = msg_send![&*generator, impactOccurred];
            }
        }
    }
    let mut next = view.superview();
    while let Some(v) = next {
        if let Ok(scroll) = v.clone().downcast::<objc2_ui_kit::UIScrollView>() {
            // Disabling a recognizer cancels it; the touch stays the drag's.
            let pan = scroll.panGestureRecognizer();
            pan.setEnabled(false);
            pan.setEnabled(true);
        }
        next = v.superview();
    }
}

/// A hover recognizer fired: enter, move or leave.
#[cfg(not(target_os = "tvos"))]
pub fn on_hover(g: &UIGestureRecognizer) {
    use prism_core::{PointerEvent, PointerPhase};
    let Some(view) = g.view() else { return };
    let phase = match g.state() {
        UIGestureRecognizerState::Began => PointerPhase::Enter,
        UIGestureRecognizerState::Changed => PointerPhase::Move,
        _ => PointerPhase::Leave,
    };
    let p = g.locationInView(Some(&view));
    crate::imp::dispatch(UiEvent::Pointer(
        view.tag() as NodeId,
        PointerEvent {
            phase,
            position: point(p),
        },
    ));
}

/// A finger moves over a view listening to pointer moves.
#[cfg(not(target_os = "tvos"))]
pub fn on_touch_move(g: &UIGestureRecognizer) {
    use prism_core::{PointerEvent, PointerPhase};
    let Some(view) = g.view() else { return };
    if !matches!(
        g.state(),
        UIGestureRecognizerState::Began | UIGestureRecognizerState::Changed
    ) {
        return;
    }
    let p = g.locationInView(Some(&view));
    crate::imp::dispatch(UiEvent::Pointer(
        view.tag() as NodeId,
        PointerEvent {
            phase: PointerPhase::Move,
            position: point(p),
        },
    ));
}

/// A pan: start (where the finger went down), moves, end or cancel.
#[cfg(not(target_os = "tvos"))]
pub fn on_pan(g: &objc2_ui_kit::UIPanGestureRecognizer) {
    use prism_core::{DragEvent, DragPhase, Point};
    let Some(view) = g.view() else { return };
    let phase = match g.state() {
        UIGestureRecognizerState::Began => DragPhase::Start,
        UIGestureRecognizerState::Changed => DragPhase::Move,
        UIGestureRecognizerState::Ended => DragPhase::End,
        UIGestureRecognizerState::Cancelled | UIGestureRecognizerState::Failed => DragPhase::Cancel,
        _ => return,
    };
    let t = g.translationInView(Some(&view));
    let at = g.locationInView(Some(&view));
    // Moving the view while it is dragged does not shift the start; the
    // button is the one that started it.
    let (start, button, mods) = START.with(|s| {
        let mut s = s.borrow_mut();
        if phase == DragPhase::Start {
            let p = Point::new((at.x - t.x) as f32, (at.y - t.y) as f32);
            let mods = modifiers(g.modifierFlags());
            s.insert(view.tag() as NodeId, (p, drag_button(g), mods));
        }
        s.get(&(view.tag() as NodeId)).copied().unwrap_or_default()
    });
    if matches!(phase, DragPhase::End | DragPhase::Cancel) {
        START.with(|s| s.borrow_mut().remove(&(view.tag() as NodeId)));
    }
    crate::imp::dispatch(UiEvent::Drag(
        view.tag() as NodeId,
        DragEvent {
            phase,
            start,
            translation: Point::new(t.x as f32, t.y as f32),
            // A finger or a pen is the primary button; an iPad's pointer
            // says which (#136).
            button,
            // The keys held when it started.
            modifiers: mods,
        },
    ));
}

/// The pointer button a pan runs with (fingers and pens: primary).
#[cfg(not(target_os = "tvos"))]
fn drag_button(g: &UIGestureRecognizer) -> prism_core::PointerButton {
    use objc2_ui_kit::UIEventButtonMask;
    let mask = g.buttonMask();
    if mask.contains(UIEventButtonMask::for_button_number(3)) {
        prism_core::PointerButton::Middle
    } else if mask.contains(UIEventButtonMask::Secondary) {
        prism_core::PointerButton::Secondary
    } else {
        prism_core::PointerButton::Primary
    }
}

/// Gesture delegate: a drag starts only with a button the element drags
/// with (`drag_buttons`, #136); other recognizers start as usual.
pub fn should_begin(g: &UIGestureRecognizer) -> bool {
    if cfg!(target_os = "tvos") || !(has_name(g, DRAG) || has_name(g, HOLD)) {
        return true;
    }
    let Some(view) = g.view() else { return true };
    let mask = MASKS.with(|m| {
        m.borrow()
            .get(&(view.tag() as NodeId))
            .copied()
            .unwrap_or(0)
    });
    #[cfg(not(target_os = "tvos"))]
    return mask & drag_button(g).drag_bit() != 0;
    #[cfg(target_os = "tvos")]
    {
        let _ = mask;
        true
    }
}

/// A pinch (#172): the scale since the previous report, around the
/// fingers' center.
#[cfg(not(target_os = "tvos"))]
pub fn on_pinch(g: &objc2_ui_kit::UIPinchGestureRecognizer) {
    use prism_core::{GesturePhase, MagnifyEvent};
    let Some(view) = g.view() else { return };
    let Some(phase) = gesture_phase(g.state()) else {
        return;
    };
    let scale = if phase == GesturePhase::End {
        1.
    } else {
        g.scale().max(0.01) as f32
    };
    // Relative: the next report starts from 1.
    g.setScale(1.);
    let at = g.locationInView(Some(&view));
    crate::imp::dispatch(UiEvent::Magnify(
        view.tag() as NodeId,
        MagnifyEvent {
            scale,
            position: point(at),
            phase,
        },
    ));
}

/// A trackpad or mouse wheel scrolled over the view (#172): the distance
/// since the previous report.
#[cfg(not(target_os = "tvos"))]
pub fn on_wheel(g: &objc2_ui_kit::UIPanGestureRecognizer) {
    use prism_core::{Point, WheelEvent};
    let Some(view) = g.view() else { return };
    let Some(phase) = gesture_phase(g.state()) else {
        return;
    };
    let t = g.translationInView(Some(&view));
    g.setTranslation_inView(CGPoint::new(0., 0.), Some(&view));
    let at = g.locationInView(Some(&view));
    crate::imp::dispatch(UiEvent::Wheel(
        view.tag() as NodeId,
        WheelEvent {
            // UIKit's translation moves the content; Prism's delta scrolls
            // it (positive y: toward the end), like the web and macOS.
            delta: Point::new(-t.x as f32, -t.y as f32),
            position: point(at),
            modifiers: modifiers(g.modifierFlags()),
            phase,
            precise: true,
        },
    ));
}

/// A continuous recognizer's state as a gesture phase (`None`: nothing to
/// report).
#[cfg(not(target_os = "tvos"))]
fn gesture_phase(state: UIGestureRecognizerState) -> Option<prism_core::GesturePhase> {
    use prism_core::GesturePhase;
    match state {
        UIGestureRecognizerState::Began => Some(GesturePhase::Start),
        UIGestureRecognizerState::Changed => Some(GesturePhase::Move),
        UIGestureRecognizerState::Ended | UIGestureRecognizerState::Cancelled => {
            Some(GesturePhase::End)
        }
        _ => None,
    }
}

thread_local! {
    /// Where each drag started, with which button and keys, by node.
    static START: RefCell<HashMap<NodeId, DragStart>> = RefCell::new(HashMap::new());
    /// The pointer events each node listens to.
    static MASKS: RefCell<HashMap<NodeId, u8>> = RefCell::new(HashMap::new());
    /// The touch hold before each node's drag, in ms (#243).
    static DELAYS: RefCell<HashMap<NodeId, u32>> = RefCell::new(HashMap::new());
    /// Where a long-press drag lifted, in the window (#243).
    #[cfg_attr(target_os = "tvos", allow(dead_code))]
    static HOLD_AT: RefCell<HashMap<NodeId, CGPoint>> = RefCell::new(HashMap::new());
}

/// Where a drag started, with which button and keys.
#[cfg_attr(target_os = "tvos", allow(dead_code))]
type DragStart = (prism_core::Point, prism_core::PointerButton, Modifiers);

fn point(p: CGPoint) -> prism_core::Point {
    prism_core::Point::new(p.x as f32, p.y as f32)
}

/// Gesture delegate: hover and touch-move recognizers, and a selection
/// group's pointer recognizer (#231), never block others; a pinch and a
/// drag of the same view both run (one finger drags, two pinch while the
/// drag follows their center).
pub fn simultaneous(g: &UIGestureRecognizer, other: &UIGestureRecognizer) -> bool {
    #[cfg(not(target_os = "tvos"))]
    let across = |r: &UIGestureRecognizer| crate::across::is_across(r);
    #[cfg(target_os = "tvos")]
    let across = |_: &UIGestureRecognizer| false;
    let pair = |a: &UIGestureRecognizer, b: &UIGestureRecognizer| {
        has_name(a, MAGNIFY) && (has_name(b, DRAG) || has_name(b, HOLD) || has_name(b, WHEEL))
    };
    [g, other]
        .iter()
        .any(|r| has_name(r, HOVER) || has_name(r, MOVE) || across(r))
        || pair(g, other)
        || pair(other, g)
}

/// Gesture delegate: the wheel recognizer never tracks a touch (a finger
/// keeps scrolling what is behind it). A tap notes the touch's number in a
/// quick series, as the system counts it (#159).
pub fn receives_touch(g: &UIGestureRecognizer, touch: &objc2_ui_kit::UITouch) -> bool {
    if g.isKindOfClass(objc2_ui_kit::UITapGestureRecognizer::class())
        && let Some(view) = g.view()
    {
        TAP_COUNTS.with(|t| {
            t.borrow_mut()
                .insert(view.tag() as NodeId, touch.tapCount().max(1) as u32)
        });
    }
    !has_name(g, WHEEL)
}

thread_local! {
    /// The number of the last touch a view's tap saw, in a quick series.
    static TAP_COUNTS: RefCell<HashMap<NodeId, u32>> = RefCell::new(HashMap::new());
}

/// A tap as Prism's click: with its position, keys and count (#159).
pub fn tap_event(g: &UIGestureRecognizer) -> Option<UiEvent> {
    let view = g.view()?;
    let id = view.tag() as NodeId;
    let position = tap_position(g);
    let count = TAP_COUNTS.with(|t| t.borrow().get(&id).copied().unwrap_or(1));
    #[cfg(not(target_os = "tvos"))]
    let modifiers = modifiers(g.modifierFlags());
    #[cfg(target_os = "tvos")]
    let modifiers = Modifiers::default();
    Some(if count > 1 || modifiers != Modifiers::default() {
        UiEvent::ClickWith(
            id,
            prism_core::ClickEvent {
                position,
                modifiers,
                count,
                button: prism_core::PointerButton::Primary,
                text_offset: None,
            },
        )
    } else {
        match position {
            Some(p) => UiEvent::ClickAt(id, p),
            None => UiEvent::Click(id),
        }
    })
}

/// Gesture delegate: a scroll view's pan waits for a drag (or a view's
/// wheel) to fail, so dragging a view inside a scroll view drags it, and
/// scrolling over a view with `on_wheel` does not scroll what is behind.
pub fn required_to_fail_by(g: &UIGestureRecognizer, other: &UIGestureRecognizer) -> bool {
    (has_name(g, DRAG) || has_name(g, WHEEL))
        && other
            .view()
            .is_some_and(|v| v.isKindOfClass(objc2_ui_kit::UIScrollView::class()))
}

/// Where a tap landed in its view.
pub fn tap_position(g: &UIGestureRecognizer) -> Option<prism_core::Point> {
    if cfg!(target_os = "tvos") {
        return None;
    }
    let view = g.view()?;
    Some(point(g.locationInView(Some(&view))))
}

// ---------------------------------------------------------------------------
// Focus
// ---------------------------------------------------------------------------

/// Gives `view` the keyboard focus (tvOS: the focus engine's), waiting for
/// it to be on screen (a sheet being presented).
pub fn focus(view: Retained<UIView>) {
    PENDING_FOCUS.with(|p| *p.borrow_mut() = Some((view, 12)));
    try_focus();
}

fn try_focus() {
    let Some((view, left)) = PENDING_FOCUS.with(|p| p.borrow_mut().take()) else {
        return;
    };
    if view.window().is_some() {
        #[cfg(not(target_os = "tvos"))]
        {
            let _ = view.becomeFirstResponder();
        }
        #[cfg(target_os = "tvos")]
        {
            FOCUS_TARGET.with(|t| *t.borrow_mut() = Some(view.clone()));
            if let Some(w) = view.window() {
                let env: &AnyObject = &w;
                let _: () = unsafe { msg_send![env, setNeedsFocusUpdate] };
                let _: () = unsafe { msg_send![env, updateFocusIfNeeded] };
                if let Some(vc) = w.rootViewController() {
                    use objc2_ui_kit::UIFocusEnvironment;
                    let top = crate::menus::top_controller(&vc);
                    top.setNeedsFocusUpdate();
                    top.updateFocusIfNeeded();
                }
            }
        }
        return;
    }
    if left == 0 {
        return;
    }
    PENDING_FOCUS.with(|p| *p.borrow_mut() = Some((view, left - 1)));
    let when = dispatch2::DispatchTime::try_from(std::time::Duration::from_millis(100))
        .unwrap_or(dispatch2::DispatchTime::NOW);
    let _ = dispatch2::DispatchQueue::main().after(when, try_focus);
}

/// Global key listeners need a first responder for presses to arrive: the
/// root controller takes that role while nothing else has it (iOS,
/// visionOS; the TV focus engine always has a focused item).
pub fn claim_keys(controller: &UIViewController) {
    #[cfg(not(target_os = "tvos"))]
    {
        if !has_global_keys() {
            return;
        }
        let Some(window) = controller.viewIfLoaded().and_then(|v| v.window()) else {
            return;
        };
        if controller.presentedViewController().is_none()
            && find_first_responder(&window).is_none()
            && !controller.isFirstResponder()
        {
            let _ = controller.becomeFirstResponder();
        }
    }
    #[cfg(target_os = "tvos")]
    let _ = controller;
}

/// tvOS: the view `focus` asked for, once (`preferredFocusEnvironments`).
pub fn take_focus_target() -> Option<Retained<UIView>> {
    FOCUS_TARGET.with(|t| t.borrow_mut().take())
}

// ---------------------------------------------------------------------------
// Layers: a pass-through window above the app's
// ---------------------------------------------------------------------------

define_class!(
    /// Shows the runtime's layers (the toaster) above everything the app's
    /// window presents; touches anywhere else reach the app.
    #[unsafe(super(UIWindow, UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismLayerWindow"]
    pub struct LayerWindow;

    unsafe impl NSObjectProtocol for LayerWindow {}

    impl LayerWindow {
        #[unsafe(method_id(hitTest:withEvent:))]
        fn hit_test(&self, point: CGPoint, event: Option<&UIEvent>) -> Option<Retained<UIView>> {
            let hit: Option<Retained<UIView>> =
                unsafe { msg_send![super(self), hitTest: point, withEvent: event] };
            let me: &UIView = self;
            let root = self.rootViewController().and_then(|vc| vc.viewIfLoaded());
            match hit {
                Some(h) if std::ptr::eq(&*h, me) || root.as_deref().is_some_and(|r| *r == *h) => None,
                other => other,
            }
        }
    }
);

/// The layer window of `scene`, with `host` (its root view) holding the layers.
pub fn layer_window(
    mtm: MainThreadMarker,
    scene: &UIWindowScene,
) -> (Retained<UIWindow>, Retained<UIView>) {
    let this = LayerWindow::alloc(mtm).set_ivars(());
    let window: Retained<LayerWindow> =
        unsafe { msg_send![super(this), initWithWindowScene: scene] };
    let window: Retained<UIWindow> = Retained::into_super(window);
    // Above the app's window and its sheets, below system alerts.
    window.setWindowLevel(unsafe { objc2_ui_kit::UIWindowLevelNormal } + 1.);
    let vc = UIViewController::new(mtm);
    #[cfg(not(target_os = "tvos"))]
    let host = {
        let host = container(mtm);
        set_pass_through(&host, true);
        host
    };
    #[cfg(target_os = "tvos")]
    let host = UIView::new(mtm);
    host.setBackgroundColor(Some(&objc2_ui_kit::UIColor::clearColor()));
    vc.setView(Some(&host));
    window.setRootViewController(Some(&vc));
    window.setBackgroundColor(Some(&objc2_ui_kit::UIColor::clearColor()));
    window.setHidden(false);
    (window, host)
}

/// Frames the layer host like the app's root (so layer frames match).
pub fn frame_layer_host(host: &UIView, size: CGRect) {
    if host.bounds().size.width != size.size.width || host.bounds().size.height != size.size.height
    {
        host.setFrame(size);
    }
}

// ---------------------------------------------------------------------------
// Suggestions: a bar above the keyboard (iOS; there is no input accessory
// view on visionOS nor a public completion list on UITextField)
// ---------------------------------------------------------------------------

#[cfg(target_os = "ios")]
pub fn refresh_suggestions(field: &objc2_ui_kit::UITextField, target: &AnyObject) {
    use objc2_core_foundation::CGSize;
    use objc2_ui_kit::{UIButton, UIButtonConfiguration, UIControlEvents, UIScrollView};
    let id = field.tag() as NodeId;
    let list = SUGGESTIONS
        .with(|m| m.borrow().get(&id).cloned())
        .unwrap_or_default();
    let typed = field.text().map(|t| t.to_string()).unwrap_or_default();
    let needle = typed.to_lowercase();
    let matches: Vec<String> = list
        .into_iter()
        .filter(|s| {
            let l = s.to_lowercase();
            l != needle && (needle.is_empty() || l.contains(&needle))
        })
        .take(12)
        .collect();
    if matches.is_empty() {
        // Back to the field's keyboard bar (#242), if it has one.
        let bar = keyboard_bar(id);
        if field.inputAccessoryView() != bar {
            field.setInputAccessoryView(bar.as_deref());
            field.reloadInputViews();
        }
        return;
    }
    let mtm = field.mtm();
    let bar = UIScrollView::new(mtm);
    bar.setFrame(CGRect::new(CGPoint::ZERO, CGSize::new(320., 46.)));
    bar.setShowsHorizontalScrollIndicator(false);
    bar.setBackgroundColor(Some(
        &objc2_ui_kit::UIColor::secondarySystemBackgroundColor(),
    ));
    bar.setAutoresizingMask(objc2_ui_kit::UIViewAutoresizing::FlexibleWidth);
    let mut x = 8.;
    for (i, s) in matches.iter().enumerate() {
        let config = UIButtonConfiguration::grayButtonConfiguration(mtm);
        config.setTitle(Some(&NSString::from_str(s)));
        config.setCornerStyle(objc2_ui_kit::UIButtonConfigurationCornerStyle::Capsule);
        let b = UIButton::buttonWithConfiguration_primaryAction(&config, None);
        // Negative tags: not Prism nodes. The field's id rides along.
        b.setTag(-(i as isize) - 1);
        unsafe {
            b.addTarget_action_forControlEvents(
                Some(target),
                sel!(onSuggestion:),
                UIControlEvents::TouchUpInside,
            )
        };
        let size = b.sizeThatFits(CGSize::new(f64::MAX / 4., 34.));
        b.setFrame(CGRect::new(
            CGPoint::new(x, 6.),
            CGSize::new(size.width, 34.),
        ));
        x += size.width + 8.;
        bar.addSubview(&b);
    }
    bar.setContentSize(CGSize::new(x, 46.));
    SUGGEST_FIELD.with(|f| *f.borrow_mut() = Some(field.retain()));
    field.setInputAccessoryView(Some(&bar));
    field.reloadInputViews();
}

#[cfg(target_os = "ios")]
thread_local! {
    /// The field the suggestion bar belongs to.
    static SUGGEST_FIELD: RefCell<Option<Retained<objc2_ui_kit::UITextField>>> =
        const { RefCell::new(None) };
}

/// A suggestion was chosen: it becomes the field's text.
#[cfg(target_os = "ios")]
pub fn suggestion_chosen(button: &objc2_ui_kit::UIButton, target: &AnyObject) {
    let Some(field) = SUGGEST_FIELD.with(|f| f.borrow().clone()) else {
        return;
    };
    let title = button
        .configuration()
        .and_then(|c| c.title())
        .map(|t| t.to_string())
        .unwrap_or_default();
    field.setText(Some(&NSString::from_str(&title)));
    crate::imp::dispatch(UiEvent::Input(field.tag() as NodeId, title));
    refresh_suggestions(&field, target);
}

// ---------------------------------------------------------------------------
// Keyboard bars (#242): a keyboard overlay's input view, the
// `inputAccessoryView` of the fields it serves (iOS, iPadOS)
// ---------------------------------------------------------------------------

/// A field (weakly held) and its keyboard bar's view.
#[cfg(target_os = "ios")]
type KeyboardBar = (objc2::rc::Weak<UIView>, Retained<UIView>);

#[cfg(target_os = "ios")]
thread_local! {
    /// Each field with a keyboard bar, by node id.
    static KEYBOARD_BARS: RefCell<HashMap<NodeId, KeyboardBar>> = RefCell::new(HashMap::new());
}

/// The keyboard bar of field `id`, if it has one.
#[cfg(target_os = "ios")]
fn keyboard_bar(id: NodeId) -> Option<Retained<UIView>> {
    KEYBOARD_BARS.with(|m| m.borrow().get(&id).map(|(_, bar)| bar.clone()))
}

/// Gives text field `field` (its `UITextField` or `UITextView`) the
/// keyboard bar `bar`, or takes it away; a focused field shows the change
/// at once. A suggestion bar showing keeps its place until it goes.
#[cfg(target_os = "ios")]
pub fn set_keyboard_bar(id: NodeId, field: &UIView, bar: Option<&UIView>) {
    let previous = KEYBOARD_BARS.with(|m| {
        let mut m = m.borrow_mut();
        let previous = m.remove(&id).map(|(_, b)| b);
        if let Some(bar) = bar {
            m.insert(id, (objc2::rc::Weak::from(field), bar.retain()));
        }
        previous
    });
    let current: Option<Retained<UIView>> = unsafe { msg_send![field, inputAccessoryView] };
    if current.is_some() && current != previous {
        return;
    }
    let _: () = unsafe { msg_send![field, setInputAccessoryView: bar] };
    if field.isFirstResponder() {
        field.reloadInputViews();
    }
}

/// A keyboard bar changed size: the focused field it serves shows it again
/// (UIKit reads an accessory's height when it shows it).
#[cfg(target_os = "ios")]
pub fn keyboard_bar_resized(bar: &UIView) {
    let focused = KEYBOARD_BARS.with(|m| {
        m.borrow()
            .values()
            .filter(|(_, b)| &**b == bar)
            .filter_map(|(f, _)| f.load())
            .find(|f| f.isFirstResponder())
    });
    if let Some(f) = focused {
        f.reloadInputViews();
    }
}

#[cfg(not(target_os = "ios"))]
pub fn keyboard_bar_resized(_bar: &UIView) {}

#[cfg(not(target_os = "ios"))]
pub fn refresh_suggestions(_field: &objc2_ui_kit::UITextField, _target: &AnyObject) {}

/// A leading SF Symbol inside a text field (`leftView`).
pub fn set_leading_icon(field: &objc2_ui_kit::UITextField, icon: &str) {
    use objc2_core_foundation::CGSize;
    let mtm = field.mtm();
    if icon.is_empty() {
        field.setLeftView(None);
        return;
    }
    let image = objc2_ui_kit::UIImage::systemImageNamed(&NSString::from_str(icon));
    let iv = objc2_ui_kit::UIImageView::new(mtm);
    iv.setImage(image.as_deref());
    iv.setContentMode(objc2_ui_kit::UIViewContentMode::Center);
    // Decorative: the field reads its own label.
    {
        use objc2_ui_kit::NSObjectUIAccessibility;
        iv.setIsAccessibilityElement(false, mtm);
    }
    unsafe { iv.setTintColor(Some(&objc2_ui_kit::UIColor::secondaryLabelColor())) };
    // Room before the text, like the system's search field.
    iv.setFrame(CGRect::new(CGPoint::ZERO, CGSize::new(32., 20.)));
    field.setLeftView(Some(&iv));
    field.setLeftViewMode(objc2_ui_kit::UITextFieldViewMode::Always);
}

/// A child of `view` drawn outside it, under `point` (#213): UIKit only
/// looks inside a view's bounds, so overflowing children of a view that
/// doesn't clip are visited here, topmost first.
#[cfg(not(target_os = "tvos"))]
fn overflow_hit(
    view: &UIView,
    point: CGPoint,
    event: Option<&UIEvent>,
) -> Option<Retained<UIView>> {
    if view.isHidden()
        || !view.isUserInteractionEnabled()
        || view.alpha() < 0.01
        || view.clipsToBounds()
    {
        return None;
    }
    let subs: Vec<Retained<UIView>> = view.subviews().iter().collect();
    for sub in subs.into_iter().rev() {
        let p = view.convertPoint_toView(point, Some(&sub));
        if let Some(hit) = sub.hitTest_withEvent(p, event) {
            return Some(hit);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Consumed keys (#181): taken before a focused text field acts on them
// ---------------------------------------------------------------------------

/// The keys `consume_keys` may list, with the input of a key command
/// (UIKit matches them before the focused field: ↑ does not move its caret).
#[cfg(not(target_os = "tvos"))]
fn command_inputs() -> [(&'static str, Retained<NSString>); 12] {
    use objc2_ui_kit::*;
    let s = |x: &NSString| x.retain();
    unsafe {
        [
            ("ArrowUp", s(UIKeyInputUpArrow)),
            ("ArrowDown", s(UIKeyInputDownArrow)),
            ("ArrowLeft", s(UIKeyInputLeftArrow)),
            ("ArrowRight", s(UIKeyInputRightArrow)),
            ("Escape", s(UIKeyInputEscape)),
            ("Home", s(UIKeyInputHome)),
            ("End", s(UIKeyInputEnd)),
            ("PageUp", s(UIKeyInputPageUp)),
            ("PageDown", s(UIKeyInputPageDown)),
            ("Enter", NSString::from_str("\r")),
            ("Tab", NSString::from_str("\t")),
            (" ", NSString::from_str(" ")),
        ]
    }
}

/// Key commands for the keys the focused node takes (`consume_keys`):
/// the root controller's, with priority over the text field's own use.
#[cfg(not(target_os = "tvos"))]
pub fn consumed_key_commands(responder: &UIResponder) -> Vec<Retained<objc2_ui_kit::UIKeyCommand>> {
    let window = window_of(responder);
    let Some((focused, _)) = focused_node(window.as_deref()) else {
        return Vec::new();
    };
    let mtm = responder.mtm();
    // Asked as a key is pressed: entries of `consume_key_at` (#211) follow
    // the caret now.
    let edges = focused_edges(window.as_deref());
    let command = |input: &NSString, flags: objc2_ui_kit::UIKeyModifierFlags| {
        let c = unsafe {
            objc2_ui_kit::UIKeyCommand::keyCommandWithInput_modifierFlags_action(
                input,
                flags,
                sel!(onConsumedKey:),
                mtm,
            )
        };
        c.setWantsPriorityOverSystemBehavior(true);
        c
    };
    let mut out: Vec<_> = command_inputs()
        .into_iter()
        .filter(|(name, _)| crate::imp::consumes_key(Some(focused), name, edges))
        .map(|(_, input)| command(&input, objc2_ui_kit::UIKeyModifierFlags::empty()))
        .collect();
    // A selection group's keys (#231): ⇧-arrows at the field's edge, and
    // arrows, Escape, Return, Backspace, ⌘C, ⌘X and ⌘V while its selection
    // spans several fields.
    let selection = window
        .as_deref()
        .and_then(|w| find_first_responder(w))
        .and_then(|f| crate::forms::current_selection(&f));
    for (name, input, flags) in across_inputs() {
        let mut key = KeyEvent::new(name);
        key.modifiers = modifiers(flags);
        key.selection = selection;
        key.edges = edges;
        let taken = !flags.is_empty() || !crate::imp::consumes_key(Some(focused), name, edges);
        if taken && crate::imp::consumes_key_event(Some(focused), &key) {
            out.push(command(&input, flags));
        }
    }
    out
}

/// The keys a selection group may take (#231): (name, key command input,
/// modifiers).
#[cfg(not(target_os = "tvos"))]
fn across_inputs() -> Vec<(
    &'static str,
    Retained<NSString>,
    objc2_ui_kit::UIKeyModifierFlags,
)> {
    use objc2_ui_kit::UIKeyModifierFlags as F;
    let mut out = Vec::new();
    for (name, input) in command_inputs().into_iter().take(5) {
        out.push((name, input.clone(), F::Shift));
        out.push((name, input, F::empty()));
    }
    out.push(("Enter", NSString::from_str("\r"), F::empty()));
    out.push(("Backspace", NSString::from_str("\u{8}"), F::empty()));
    for letter in ["c", "x", "v"] {
        out.push((letter, NSString::from_str(letter), F::Command));
    }
    out
}

/// Where the focused text field's caret is (#211), if one has the focus.
#[cfg(not(target_os = "tvos"))]
fn focused_edges(window: Option<&UIWindow>) -> Option<prism_core::CaretEdges> {
    let field = find_first_responder(window?)?;
    crate::forms::current_edges(&field)
}

/// A consumed key's command fired: reported like a press.
#[cfg(not(target_os = "tvos"))]
pub fn consumed_key(responder: &UIResponder, command: &objc2_ui_kit::UIKeyCommand) {
    let Some(input) = command.input() else { return };
    let flags = command.modifierFlags();
    let Some(name) = command_inputs()
        .into_iter()
        .chain(across_inputs().into_iter().map(|(n, i, _)| (n, i)))
        .find(|(_, i)| i.isEqualToString(&input))
        .map(|(n, _)| n)
    else {
        return;
    };
    let window = window_of(responder);
    let focused = focused_node(window.as_deref()).map(|f| f.0);
    let mut key = KeyEvent::new(name);
    key.modifiers = modifiers(flags);
    if let Some(field) = window
        .as_deref()
        .and_then(|w| find_first_responder(w))
        .filter(|r| {
            r.isKindOfClass(objc2_ui_kit::UITextField::class())
                || r.isKindOfClass(objc2_ui_kit::UITextView::class())
        })
    {
        key.selection = crate::forms::current_selection(&field);
        key.edges = crate::forms::current_edges(&field);
    }
    crate::imp::dispatch(UiEvent::Key(focused, key));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hid_usages_have_web_names() {
        assert_eq!(named_key(0x29).as_deref(), Some("Escape"));
        assert_eq!(named_key(0x52).as_deref(), Some("ArrowUp"));
        assert_eq!(named_key(0x3A).as_deref(), Some("F1"));
        assert_eq!(named_key(0x45).as_deref(), Some("F12"));
        assert_eq!(named_key(0x04), None);
    }

    #[test]
    fn max_length_lets_deletions_through() {
        set_max_length(9, Some(3));
        assert!(edit_fits(9, 2, 3));
        assert!(!edit_fits(9, 3, 4));
        assert!(edit_fits(9, 5, 4));
        forget(9);
        assert!(edit_fits(9, 3, 40));
    }
}
