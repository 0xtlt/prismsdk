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
//!   view's pan waits for. tvOS has no pointer on views: none of these.
//! - Keys: `pressesBegan:` with `UIKey` (hardware keyboards) and the remote's
//!   press types (tvOS), on the view that listens and on the root view
//!   controller for global listeners.
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

/// What a press means as a Prism key: a hardware key, or the TV remote's
/// arrows, select ("Enter"), play/pause and Menu ("Escape").
pub fn key_event(press: &UIPress, mtm: MainThreadMarker) -> Option<KeyEvent> {
    if let Some(key) = press.key(mtm) {
        use objc2_ui_kit::UIKeyModifierFlags as F;
        let flags = key.modifierFlags();
        let modifiers = Modifiers {
            shift: flags.contains(F::Shift),
            control: flags.contains(F::Control),
            alt: flags.contains(F::Alternate),
            meta: flags.contains(F::Command),
        };
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
    let selection = focused
        .filter(|f| f.1)
        .and_then(|_| window.and_then(|w| find_first_responder(w)))
        .and_then(|r| crate::forms::current_selection(&r));
    for press in presses.iter() {
        let Some(key) = key_event(&press, mtm) else {
            continue;
        };
        let key = KeyEvent { selection, ..key };
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
    match wanted(presses, window.as_deref(), local, mtm) {
        Some((target, key)) => {
            if began {
                crate::imp::dispatch(UiEvent::Key(target, key));
            }
            true
        }
        None => false,
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

        /// A layer's root takes no touch itself: only its children do.
        #[unsafe(method_id(hitTest:withEvent:))]
        fn hit_test(&self, point: CGPoint, event: Option<&UIEvent>) -> Option<Retained<UIView>> {
            let hit: Option<Retained<UIView>> =
                unsafe { msg_send![super(self), hitTest: point, withEvent: event] };
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

        /// VoiceOver's double tap is a click without a position.
        #[unsafe(method(accessibilityActivate))]
        fn accessibility_activate(&self) -> bool {
            if self.ivars().clickable.get() {
                crate::imp::dispatch(UiEvent::Click(self.tag() as NodeId));
                true
            } else {
                false
            }
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

fn has_name(g: &UIGestureRecognizer, name: &str) -> bool {
    g.name().is_some_and(|n| n.to_string() == name)
}

/// Adds or removes the hover, touch-move and pan recognizers of `view`.
#[cfg(not(target_os = "tvos"))]
pub fn set_pointer_events(view: &UIView, mask: u8, target: &AnyObject) {
    use objc2_ui_kit::{
        UIHoverGestureRecognizer, UILongPressGestureRecognizer, UIPanGestureRecognizer,
    };
    use prism_core::events::pointer_mask;
    let mtm = view.mtm();
    let existing: Vec<Retained<UIGestureRecognizer>> = view
        .gestureRecognizers()
        .map(|a| a.to_vec())
        .unwrap_or_default();
    for (bit, name) in [
        (pointer_mask::HOVER | pointer_mask::MOVE, HOVER),
        (pointer_mask::MOVE, MOVE),
        (pointer_mask::DRAG, DRAG),
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
                g.setCancelsTouchesInView(name == DRAG);
                g.setDelaysTouchesEnded(false);
                let delegate: &AnyObject = target;
                let _: () = unsafe { msg_send![&*g, setDelegate: delegate] };
                view.addGestureRecognizer(&g);
            }
            _ => {}
        }
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
    // Moving the view while it is dragged does not shift the start.
    let start = START.with(|s| {
        let mut s = s.borrow_mut();
        if phase == DragPhase::Start {
            let p = Point::new((at.x - t.x) as f32, (at.y - t.y) as f32);
            s.insert(view.tag() as NodeId, p);
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
            // A finger, a pen or the pointer's main button.
            button: prism_core::PointerButton::Primary,
            modifiers: Default::default(),
        },
    ));
}

thread_local! {
    /// Where each drag started, by node.
    static START: RefCell<HashMap<NodeId, prism_core::Point>> = RefCell::new(HashMap::new());
}

fn point(p: CGPoint) -> prism_core::Point {
    prism_core::Point::new(p.x as f32, p.y as f32)
}

/// Gesture delegate: hover and touch-move recognizers never block others.
pub fn simultaneous(g: &UIGestureRecognizer) -> bool {
    has_name(g, HOVER) || has_name(g, MOVE)
}

/// Gesture delegate: a scroll view's pan waits for a drag to fail, so
/// dragging a view inside a scroll view drags it.
pub fn required_to_fail_by(g: &UIGestureRecognizer, other: &UIGestureRecognizer) -> bool {
    has_name(g, DRAG)
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
        if field.inputAccessoryView().is_some() {
            field.setInputAccessoryView(None);
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
