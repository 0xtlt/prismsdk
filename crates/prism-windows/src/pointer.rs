//! Pointer input on Prism's views, from XAML's pointer events on a node's
//! outer element: hover and moves (`on_pointer_*`), drags (`on_drag`,
//! `draggable`: any button, past the system's drag distance; a touch on a
//! node that drags after a long press waits for it), horizontal swipes
//! (`on_swipe_*`: a quick flick with any pointer), wheel and touchpad
//! scrolling (`on_wheel`), middle, back and forward clicks
//! (`on_aux_click`), pinches (`on_magnify`: two touches, or Ctrl + wheel,
//! as precision touchpads send their pinches), and the see-through preview
//! of a dragged payload. Files dropped from Explorer are `drops`'.
//!
//! A clickable view's `Button` takes the press (it marks it handled): in
//! such a view the press is heard on its children's canvas, inside the
//! button, before the button sees it; moves, releases and captures bubble
//! up to the outer element. Once a drag (or a swipe) starts, the outer
//! element captures the pointer: the button lets go of it, so the press
//! does not end in a click, and a scroll view around stops panning.
//!
//! Positions are in the element (DIPs, Prism's points); a drag's
//! translation is measured in the window, so it stays right while the
//! element moves under the pointer.

use std::cell::RefCell;
use std::collections::HashMap;

use prism_core::events::pointer_mask::{AUX_CLICK, HOVER, MAGNIFY, MOVE, WHEEL};
use prism_core::{
    ClickEvent, DragEvent, DragPhase, GesturePhase, MagnifyEvent, Modifiers, NodeId, Point,
    PointerButton, PointerEvent, PointerPhase, SwipeDirection, UiEvent, WheelEvent,
};
use windows::System::VirtualKeyModifiers;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, KillTimer, SM_CXDRAG, SetTimer};
use windows_core::{Interface, Result};

use crate::imp::{defer, dispatch};
use crate::widgets::Win;
use crate::win::{env, log};
use crate::winui::bindings::Microsoft::UI::Input::{
    PointerDeviceType, PointerPoint, PointerUpdateKind,
};
use crate::winui::ui;
use crate::winui::xaml::Controls::{Canvas, Image};
use crate::winui::xaml::Input::{PointerEventHandler, PointerRoutedEventArgs};
use crate::winui::xaml::Media::Imaging::RenderTargetBitmap;
use crate::winui::xaml::UIElement;

/// A quick flick: at least this fast (DIPs per second), mostly
/// horizontal, and this far (DIPs), like GTK's swipe gesture.
const SWIPE_SPEED: f32 = 300.;
const SWIPE_DISTANCE: f32 = 32.;
/// A touch moves this far (DIPs) before it drags or swipes (fingers are
/// less precise than the mouse's `SM_CXDRAG`).
const TOUCH_SLOP: f32 = 10.;
/// How far back (µs) the swipe's speed is measured.
const SPEED_WINDOW: u64 = 100_000;

/// What a node listens to, and the press in progress on it.
#[derive(Default)]
struct Listener {
    mask: u8,
    /// Swipes reported: left, right.
    swipe: (bool, bool),
    /// A touch holds this long (ms) before the node drags (#243).
    delay: u32,
    /// The event handlers are on the outer element, the press handler on
    /// the children's canvas (inside a button).
    wired: bool,
    kids_wired: bool,
    press: Option<Press>,
    /// The touches on it, for pinches.
    pinch: crate::pinch::Pinch,
}

#[derive(Clone, Copy)]
struct Press {
    pointer: u32,
    /// When it went down (µs): the same press heard twice.
    stamp: u64,
    /// Where it went down, in the element and in the window.
    at: (f32, f32),
    window_at: (f32, f32),
    button: PointerButton,
    touch: bool,
    modifiers: Modifiers,
    /// The drag started (past the slop, or held long enough).
    started: bool,
    /// A touch waiting for its long press (the timer's id).
    holding: Option<usize>,
    /// The element captured the pointer (a drag or a swipe).
    captured: bool,
    /// A swipe may come of it (the node reports swipes).
    swipe: bool,
    /// Recent positions in the window with their times (µs), for the
    /// swipe's speed.
    samples: [(f32, f32, u64); 8],
    count: usize,
}

impl Press {
    fn sample(&mut self, x: f32, y: f32, t: u64) {
        self.samples[self.count % self.samples.len()] = (x, y, t);
        self.count += 1;
    }

    /// The speed (DIPs per second) over the last [`SPEED_WINDOW`].
    fn velocity(&self) -> (f32, f32) {
        let n = self.count.min(self.samples.len());
        if n < 2 {
            return (0., 0.);
        }
        let last = self.samples[(self.count - 1) % self.samples.len()];
        let mut first = last;
        for i in 2..=n {
            let s = self.samples[(self.count - i) % self.samples.len()];
            first = s;
            if last.2.saturating_sub(s.2) >= SPEED_WINDOW {
                break;
            }
        }
        let dt = last.2.saturating_sub(first.2) as f32 / 1e6;
        if dt <= 0. {
            return (0., 0.);
        }
        ((last.0 - first.0) / dt, (last.1 - first.1) / dt)
    }
}

thread_local! {
    static NODES: RefCell<HashMap<NodeId, Listener>> = RefCell::new(HashMap::new());
    /// The long-press timers waiting (`SetTimer` id → node).
    static HOLDS: RefCell<HashMap<usize, NodeId>> = RefCell::new(HashMap::new());
    /// The dragged payload's picture and the node it shows.
    static PREVIEW: RefCell<Option<(NodeId, Image)>> = const { RefCell::new(None) };
}

/// Reports `e` (`PRISM_TRACE_INPUT=1` logs it: what reached the app).
fn report(e: UiEvent) {
    if env("PRISM_TRACE_INPUT").is_some() {
        log!("input: {e:?}");
    }
    dispatch(e);
}

fn with<R>(id: NodeId, f: impl FnOnce(&mut Listener) -> R) -> R {
    NODES.with(|n| f(n.borrow_mut().entry(id).or_default()))
}

/// Which pointer events node `id` reports (`pointer_mask` bits).
pub fn set_mask(id: NodeId, mask: u8) {
    with(id, |l| l.mask = mask);
}

/// A node reporting pinches keeps its touches (#133): the scroll views
/// around would otherwise take two moving fingers for themselves
/// (DirectManipulation) before the pinch has grown. XAML leaves the
/// touches that start on an element with a manipulation mode of its own
/// to it: on such a node one finger does not scroll the page either, as
/// on a canvas.
pub fn set_touch_mode(e: &UIElement, mask: u8) {
    set_mode(e, mask, false);
}

fn set_mode(e: &UIElement, mask: u8, swipes: bool) {
    use crate::winui::xaml::Input::ManipulationModes;
    let mode = if mask & MAGNIFY != 0 {
        ManipulationModes::Scale
    } else if swipes {
        // Vertical pans stay the scroll views' (DirectManipulation),
        // horizontal ones come to the node as manipulations.
        ManipulationModes::System
            | ManipulationModes::TranslateX
            | ManipulationModes::TranslateRailsX
    } else {
        ManipulationModes::System
    };
    if e.ManipulationMode().ok() != Some(mode) {
        let _ = e.SetManipulationMode(mode);
    }
}

/// Node `id` (element `e`) reports swipes or pinches, or not anymore:
/// the touches it keeps. By touch, the scroll view around takes a finger
/// as it lands (the row gets a press, then nothing): with horizontal
/// translations of its own (on rails), a horizontal flick stays the row's
/// pointer moves, judged like the mouse's, while vertical pans still
/// scroll the list.
pub fn wire_swipes(e: &UIElement, id: NodeId) -> Result<()> {
    let (mask, swipe) = with(id, |l| (l.mask, l.swipe));
    set_mode(e, mask, swipe != (false, false));
    Ok(())
}

/// Which swipes node `id` reports.
pub fn set_swipe(id: NodeId, left: bool, right: bool) {
    with(id, |l| l.swipe = (left, right));
}

/// How long a touch holds before node `id` drags (#243).
pub fn set_delay(id: NodeId, ms: u32) {
    with(id, |l| l.delay = ms);
}

/// Whether node `id` takes pointer input for these events (its element
/// must be hit-testable).
pub fn wants(id: NodeId) -> bool {
    NODES.with(|n| {
        n.borrow()
            .get(&id)
            .is_some_and(|l| l.mask != 0 || l.swipe != (false, false))
    }) || crate::drops::accepts(id)
}

/// The node is gone.
pub fn forget(id: NodeId) {
    NODES.with(|n| n.borrow_mut().remove(&id));
}

type Handler = fn(&UIElement, NodeId, &PointerRoutedEventArgs) -> Result<()>;

fn handler(id: NodeId, f: Handler) -> PointerEventHandler {
    PointerEventHandler::new(move |s, a| {
        let (Some(s), Some(a)) = (s.as_ref(), a.as_ref()) else {
            return Ok(());
        };
        f(&s.cast()?, id, a)
    })
}

/// Puts the handlers on node `id`'s elements, once: `outer`, and `kids`
/// when the node is a clickable view (`button`).
pub fn wire(outer: &Canvas, kids: &Canvas, button: bool, id: NodeId) -> Result<()> {
    if !wants(id) {
        return Ok(());
    }
    if !with(id, |l| std::mem::replace(&mut l.wired, true)) {
        let e: UIElement = outer.cast()?;
        e.PointerPressed(&handler(id, pressed))?;
        e.PointerMoved(&handler(id, moved))?;
        e.PointerReleased(&handler(id, released))?;
        e.PointerCanceled(&handler(id, canceled))?;
        e.PointerCaptureLost(&handler(id, capture_lost))?;
        e.PointerEntered(&handler(id, entered))?;
        e.PointerExited(&handler(id, exited))?;
        e.PointerWheelChanged(&handler(id, wheel))?;
    }
    if button && !with(id, |l| std::mem::replace(&mut l.kids_wired, true)) {
        // Hit everywhere in the button, so every press passes here first.
        kids.SetBackground(&crate::winui::transparent()?)?;
        kids.PointerPressed(&handler(id, pressed))?;
    }
    Ok(())
}

fn point(x: f32, y: f32) -> Point {
    Point { x, y }
}

pub(crate) fn modifiers(a: &PointerRoutedEventArgs) -> Modifiers {
    let m = a.KeyModifiers().unwrap_or(VirtualKeyModifiers::None);
    Modifiers {
        shift: m.contains(VirtualKeyModifiers::Shift),
        control: m.contains(VirtualKeyModifiers::Control),
        alt: m.contains(VirtualKeyModifiers::Menu),
        meta: m.contains(VirtualKeyModifiers::Windows),
    }
}

/// The point in the element and in the window.
fn points(e: &UIElement, a: &PointerRoutedEventArgs) -> Result<(PointerPoint, PointerPoint)> {
    Ok((
        a.GetCurrentPoint(e)?,
        a.GetCurrentPoint(None::<&UIElement>)?,
    ))
}

fn xy(p: &PointerPoint) -> (f32, f32) {
    p.Position().map_or((0., 0.), |p| (p.X, p.Y))
}

/// The button a press or release changed.
fn button_of(p: &PointerPoint) -> Option<PointerButton> {
    let kind = p.Properties().and_then(|p| p.PointerUpdateKind()).ok()?;
    Some(match kind {
        PointerUpdateKind::LeftButtonPressed | PointerUpdateKind::LeftButtonReleased => {
            PointerButton::Primary
        }
        PointerUpdateKind::MiddleButtonPressed | PointerUpdateKind::MiddleButtonReleased => {
            PointerButton::Middle
        }
        PointerUpdateKind::RightButtonPressed | PointerUpdateKind::RightButtonReleased => {
            PointerButton::Secondary
        }
        PointerUpdateKind::XButton1Pressed | PointerUpdateKind::XButton1Released => {
            PointerButton::BACK
        }
        PointerUpdateKind::XButton2Pressed | PointerUpdateKind::XButton2Released => {
            PointerButton::FORWARD
        }
        _ => return None,
    })
}

/// How far a pointer moves before it drags: the system's drag distance
/// for the mouse and pens, more for fingers.
fn slop(touch: bool) -> f32 {
    if touch {
        return TOUCH_SLOP;
    }
    let px = unsafe { GetSystemMetrics(SM_CXDRAG) }.max(1) as f32;
    let scale = crate::imp::with_state(|s| s.renderer.widgets.scale()).unwrap_or(1.);
    (px / scale).max(2.)
}

fn drag_event(id: NodeId, p: &Press, phase: DragPhase, now: (f32, f32)) -> UiEvent {
    UiEvent::Drag(
        id,
        DragEvent {
            phase,
            start: point(p.at.0, p.at.1),
            translation: point(now.0 - p.window_at.0, now.1 - p.window_at.1),
            button: p.button,
            modifiers: p.modifiers,
        },
    )
}

/// Reports a pinch's step on node `id`.
fn magnify(id: NodeId, step: crate::pinch::Step) {
    use crate::pinch::Step;
    let (phase, scale, (x, y)) = match step {
        Step::Start(c) => (GesturePhase::Start, 1., c),
        Step::Move(k, c) => (GesturePhase::Move, k, c),
        Step::End(c) => (GesturePhase::End, 1., c),
    };
    report(UiEvent::Magnify(
        id,
        MagnifyEvent {
            scale,
            position: point(x, y),
            phase,
        },
    ));
}

/// A pinch's step on node `id`, its point from the window to the element
/// (`offset`: the element's origin in the window, negated).
fn magnify_at(id: NodeId, step: crate::pinch::Step, offset: (f32, f32)) {
    use crate::pinch::Step;
    let local = |(x, y): (f32, f32)| (x + offset.0, y + offset.1);
    magnify(
        id,
        match step {
            Step::Start(c) => Step::Start(local(c)),
            Step::Move(k, c) => Step::Move(k, local(c)),
            Step::End(c) => Step::End(local(c)),
        },
    );
}

/// The window → element offset of the event's point.
fn offset(local: &PointerPoint, window: &PointerPoint) -> (f32, f32) {
    let (l, w) = (xy(local), xy(window));
    (l.0 - w.0, l.1 - w.1)
}

/// A touch went down on node `id`, which reports pinches: a second finger
/// starts one (`true`), the fingers then the node's alone. A drag going on
/// follows the fingers' middle from there (#172), without a jump.
fn pinch_down(
    e: &UIElement,
    id: NodeId,
    a: &PointerRoutedEventArgs,
    local: &PointerPoint,
    window: &PointerPoint,
) -> Result<bool> {
    let pointer = local.PointerId()?;
    if let Ok(ptr) = a.Pointer() {
        POINTERS.with(|m| m.borrow_mut().insert(pointer, ptr));
    }
    let Some(step) = with(id, |l| l.pinch.down(pointer, xy(window))) else {
        return Ok(false);
    };
    // The scroll view around stops.
    let _ = e.CancelDirectManipulations();
    let touches: Vec<u32> = POINTERS.with(|m| m.borrow().keys().copied().collect());
    for t in touches {
        if with(id, |l| l.pinch.has(t)) {
            let _ = capture_now(e, t);
        }
    }
    rebase(id);
    magnify_at(id, step, offset(local, window));
    a.SetHandled(true)?;
    Ok(true)
}

/// The fingers of node `id`'s press changed (one came or lifted): its
/// drag now follows their middle (or the finger left), from where it is.
fn rebase(id: NodeId) {
    with(id, |l| {
        let Some(p) = l.press.as_mut() else { return };
        let last = p.samples[(p.count.max(1) - 1) % p.samples.len()];
        let now = match l.pinch.center() {
            Some(c) => c,
            None => match l.pinch.first() {
                Some((finger, at)) => {
                    p.pointer = finger;
                    at
                }
                None => return,
            },
        };
        p.window_at.0 += now.0 - last.0;
        p.window_at.1 += now.1 - last.1;
        p.sample(now.0, now.1, last.2);
    });
}

/// A touch of node `id` lifted or was cancelled: the end of its pinch; a
/// drag goes on with the finger left.
fn pinch_up(e: &UIElement, id: NodeId, pointer: u32, offset: (f32, f32)) {
    if let Some(step) = with(id, |l| l.pinch.up(pointer)) {
        if with(id, |l| l.pinch.first().is_none()) {
            let _ = e.ReleasePointerCaptures();
        }
        rebase(id);
        magnify_at(id, step, offset);
    }
}

/// During a pinch, node `id`'s drag follows the fingers' middle (#172).
fn pinch_drag(id: NodeId, local: &PointerPoint) -> Result<()> {
    let Some(w) = with(id, |l| l.pinch.center()) else {
        return Ok(());
    };
    let mask = with(id, |l| l.mask);
    let Some(mut p) = with(id, |l| l.press) else {
        return Ok(());
    };
    p.sample(w.0, w.1, local.Timestamp()?);
    let far = (w.0 - p.window_at.0).hypot(w.1 - p.window_at.1) >= slop(true);
    let phase = if p.started {
        Some(DragPhase::Move)
    } else if far && mask & p.button.drag_bit() != 0 {
        p.started = true;
        p.captured = true;
        Some(DragPhase::Start)
    } else {
        None
    };
    with(id, |l| l.press = Some(p));
    if let Some(phase) = phase {
        report(drag_event(id, &p, phase, w));
    }
    Ok(())
}

fn pressed(e: &UIElement, id: NodeId, a: &PointerRoutedEventArgs) -> Result<()> {
    let (local, window) = points(e, a)?;
    let touch = local.PointerDeviceType()? == PointerDeviceType::Touch;
    if touch && with(id, |l| l.mask) & MAGNIFY != 0 && pinch_down(e, id, a, &local, &window)? {
        return Ok(());
    }
    let button = if touch {
        PointerButton::Primary
    } else {
        match button_of(&local) {
            Some(b) => b,
            None => return Ok(()),
        }
    };
    let (mask, swipe, delay) = with(id, |l| (l.mask, l.swipe, l.delay));
    if env("PRISM_TRACE_INPUT").is_some() {
        log!("input: press on node {id} ({button:?}, mask {mask})");
    }
    // Heard twice when it bubbles from the children's canvas: once.
    let stamp = local.Timestamp()?;
    let pointer = local.PointerId()?;
    if with(id, |l| {
        l.press
            .is_some_and(|p| p.pointer == pointer && p.stamp == stamp)
    }) {
        return Ok(());
    }
    let drags = mask & button.drag_bit() != 0;
    let swipes = swipe != (false, false) && button == PointerButton::Primary;
    if !drags && !swipes {
        return Ok(());
    }
    // Only the innermost node that drags takes the press (its ancestors
    // see it bubbling up).
    if drags && claimed(a) {
        return Ok(());
    }
    let w = xy(&window);
    let mut press = Press {
        pointer,
        stamp,
        at: xy(&local),
        window_at: w,
        button,
        touch,
        modifiers: modifiers(a),
        started: false,
        holding: None,
        captured: false,
        swipe: swipes,
        samples: [(0., 0., 0); 8],
        count: 0,
    };
    press.sample(w.0, w.1, local.Timestamp()?);
    if drags && touch && delay > 0 {
        // Lifted after the long press, unless it moved first (a scroll).
        let timer = unsafe { SetTimer(None::<HWND>, 0, delay, Some(held)) };
        if timer != 0 {
            HOLDS.with(|hl| hl.borrow_mut().insert(timer, id));
            press.holding = Some(timer);
        }
    }
    if let Ok(ptr) = a.Pointer() {
        POINTERS.with(|m| m.borrow_mut().insert(press.pointer, ptr));
    }
    with(id, |l| l.press = Some(press));
    if drags {
        claim(a);
    }
    Ok(())
}

thread_local! {
    /// The press being dispatched was taken by a node that drags (the
    /// innermost one): its ancestors leave it.
    static CLAIMED: RefCell<Option<(u32, u64)>> = const { RefCell::new(None) };
}

fn press_key(a: &PointerRoutedEventArgs) -> Option<(u32, u64)> {
    let p = a.GetCurrentPoint(None::<&UIElement>).ok()?;
    Some((p.PointerId().ok()?, p.Timestamp().ok()?))
}

fn claim(a: &PointerRoutedEventArgs) {
    CLAIMED.with(|c| *c.borrow_mut() = press_key(a));
}

fn claimed(a: &PointerRoutedEventArgs) -> bool {
    let key = press_key(a);
    CLAIMED.with(|c| key.is_some() && *c.borrow() == key)
}

/// The long press of a touch on node `id` elapsed: it lifts.
unsafe extern "system" fn held(_: HWND, _: u32, timer: usize, _: u32) {
    unsafe {
        let _ = KillTimer(None::<HWND>, timer);
    }
    let Some(id) = HOLDS.with(|h| h.borrow_mut().remove(&timer)) else {
        return;
    };
    let Some(mut p) = with(id, |l| l.press.filter(|p| p.holding == Some(timer))) else {
        return;
    };
    p.holding = None;
    p.started = true;
    let last = p.samples[(p.count - 1) % p.samples.len()];
    with(id, |l| l.press = Some(p));
    // The finger owns the node now: the scroll view around stops.
    capture(id, p.pointer);
    report(drag_event(id, &p, DragPhase::Start, (last.0, last.1)));
}

/// `e` takes the pointer of `a` from whatever inside holds it (a
/// clickable view's button lets go: no click at the end) and from the
/// scroll views around (they stop panning).
fn steal(e: &UIElement, a: &PointerRoutedEventArgs) {
    let Ok(ptr) = a.Pointer() else { return };
    let mut at = a
        .OriginalSource()
        .ok()
        .and_then(|o| o.cast::<crate::winui::xaml::DependencyObject>().ok());
    while let Some(o) = at {
        let Ok(el) = o.cast::<UIElement>() else { break };
        if el == *e {
            break;
        }
        let _ = el.ReleasePointerCapture(&ptr);
        at = crate::winui::xaml::Media::VisualTreeHelper::GetParent(&o).ok();
    }
    let _ = e.CancelDirectManipulations();
    if !e.CapturePointer(&ptr).unwrap_or(false) {
        log!("pointer: could not capture the pointer");
    }
}

/// Node `id`'s element takes pointer `pointer` (its button lets go).
fn capture(id: NodeId, pointer: u32) {
    defer(Box::new(move |s| {
        let Some(v) = s.renderer.widgets.views.get(&id) else {
            return;
        };
        let e = v.outer();
        let _ = e.CancelDirectManipulations();
        let _ = capture_now(&e, pointer);
    }));
}

thread_local! {
    /// The `Pointer` objects of the presses in progress, for capturing.
    static POINTERS: RefCell<HashMap<u32, crate::winui::xaml::Input::Pointer>> =
        RefCell::new(HashMap::new());
}

fn capture_now(e: &UIElement, pointer: u32) -> Result<bool> {
    match POINTERS.with(|p| p.borrow().get(&pointer).cloned()) {
        Some(p) => e.CapturePointer(&p),
        None => Ok(false),
    }
}

fn moved(e: &UIElement, id: NodeId, a: &PointerRoutedEventArgs) -> Result<()> {
    let (local, window) = points(e, a)?;
    let (x, y) = xy(&local);
    let mask = with(id, |l| l.mask);
    if mask & MAGNIFY != 0 {
        let pointer = local.PointerId()?;
        let pinching = with(id, |l| l.pinch.pinching() && l.pinch.has(pointer));
        if let Some(step) = with(id, |l| l.pinch.moved(pointer, xy(&window))) {
            magnify_at(id, step, offset(&local, &window));
        }
        if pinching {
            return pinch_drag(id, &local);
        }
    }
    if mask & MOVE != 0 && !a.Handled().unwrap_or(false) {
        report(UiEvent::Pointer(
            id,
            PointerEvent {
                phase: PointerPhase::Move,
                position: point(x, y),
            },
        ));
    }
    let pointer = local.PointerId()?;
    let Some(mut p) = with(id, |l| l.press.filter(|p| p.pointer == pointer)) else {
        return Ok(());
    };
    if !local.IsInContact()? && !p.started {
        // Released where nobody told us (a button took the release).
        with(id, |l| l.press = None);
        return Ok(());
    }
    let w = xy(&window);
    p.sample(w.0, w.1, local.Timestamp()?);
    p.modifiers = modifiers(a);
    let (dx, dy) = (w.0 - p.window_at.0, w.1 - p.window_at.1);
    let far = dx.hypot(dy) >= slop(p.touch);
    let drags = mask & p.button.drag_bit() != 0;
    if let Some(timer) = p.holding {
        if far {
            // Moved before the long press: a scroll, not a drag.
            unsafe {
                let _ = KillTimer(None::<HWND>, timer);
            }
            HOLDS.with(|h| h.borrow_mut().remove(&timer));
            p.holding = None;
            with(id, |l| {
                l.press = p.swipe.then_some(Press { swipe: true, ..p })
            });
            return Ok(());
        }
        with(id, |l| l.press = Some(p));
        return Ok(());
    }
    if !p.started && far {
        if drags {
            p.started = true;
            p.captured = true;
            with(id, |l| l.press = Some(p));
            steal(e, a);
            report(drag_event(id, &p, DragPhase::Start, w));
            return Ok(());
        }
        if p.swipe && !p.captured && dx.abs() > dy.abs() * 2. {
            // A horizontal flick on a row: the row's button lets go (no
            // click at the end).
            p.captured = true;
            with(id, |l| l.press = Some(p));
            steal(e, a);
        }
    }
    with(id, |l| l.press = Some(p));
    if p.started {
        report(drag_event(id, &p, DragPhase::Move, w));
    }
    Ok(())
}

fn released(e: &UIElement, id: NodeId, a: &PointerRoutedEventArgs) -> Result<()> {
    let (local, window) = points(e, a)?;
    let pointer = local.PointerId()?;
    let mask = with(id, |l| l.mask);
    pinch_up(e, id, pointer, offset(&local, &window));
    // Middle, back and forward clicks (#215).
    if mask & AUX_CLICK != 0
        && local.PointerDeviceType()? == PointerDeviceType::Mouse
        && let Some(b) = button_of(&local)
        && matches!(b, PointerButton::Middle | PointerButton::Other(_))
        && with(id, |l| l.press.is_none_or(|p| !p.started))
    {
        let (x, y) = xy(&local);
        report(UiEvent::AuxClick(
            id,
            ClickEvent {
                position: Some(point(x, y)),
                modifiers: modifiers(a),
                count: 1,
                button: b,
                text_offset: None,
            },
        ));
    }
    let Some(mut p) = with(id, |l| {
        l.press
            .filter(|p| p.pointer == pointer)
            .inspect(|_| l.press = None)
    }) else {
        return Ok(());
    };
    POINTERS.with(|m| m.borrow_mut().remove(&pointer));
    if let Some(timer) = p.holding.take() {
        unsafe {
            let _ = KillTimer(None::<HWND>, timer);
        }
        HOLDS.with(|h| h.borrow_mut().remove(&timer));
    }
    let w = xy(&window);
    p.sample(w.0, w.1, local.Timestamp()?);
    if p.captured {
        let _ = e.ReleasePointerCaptures();
    }
    if p.started {
        report(drag_event(id, &p, DragPhase::End, w));
        return Ok(());
    }
    if p.swipe {
        let (dx, dy) = (w.0 - p.window_at.0, w.1 - p.window_at.1);
        let (vx, _) = p.velocity();
        let (left, right) = with(id, |l| l.swipe);
        if vx.abs() >= SWIPE_SPEED && dx.abs() >= SWIPE_DISTANCE && dx.abs() >= dy.abs() * 2. {
            if dx < 0. && left {
                report(UiEvent::Swipe(id, SwipeDirection::Left));
            } else if dx > 0. && right {
                report(UiEvent::Swipe(id, SwipeDirection::Right));
            }
        }
    }
    Ok(())
}

/// An element lost the pointer's capture: the outer element itself (the
/// system took it), or a button inside when the outer element took it
/// for a drag, which goes on.
fn capture_lost(e: &UIElement, id: NodeId, a: &PointerRoutedEventArgs) -> Result<()> {
    let own = a
        .OriginalSource()
        .ok()
        .and_then(|o| o.cast::<UIElement>().ok())
        .is_some_and(|o| o == *e);
    let captured = with(id, |l| l.press.is_some_and(|p| p.captured));
    if captured && !own {
        return Ok(());
    }
    if !captured && !own {
        // The button let go after a click: the press is over.
        let pointer = a.Pointer().and_then(|p| p.PointerId()).unwrap_or(u32::MAX);
        with(id, |l| {
            if l.press.is_some_and(|p| p.pointer == pointer && !p.started) {
                l.press = None;
            }
        });
        return Ok(());
    }
    canceled(e, id, a)
}

/// The system took the pointer (a scroll view panned, the window lost
/// it): a drag in progress is cancelled.
fn canceled(e: &UIElement, id: NodeId, a: &PointerRoutedEventArgs) -> Result<()> {
    let pointer = a.Pointer().and_then(|p| p.PointerId()).unwrap_or(u32::MAX);
    let off = points(e, a).map_or((0., 0.), |(l, w)| offset(&l, &w));
    pinch_up(e, id, pointer, off);
    let Some(p) = with(id, |l| {
        l.press
            .filter(|p| p.pointer == pointer)
            .inspect(|_| l.press = None)
    }) else {
        return Ok(());
    };
    if let Some(timer) = p.holding {
        unsafe {
            let _ = KillTimer(None::<HWND>, timer);
        }
        HOLDS.with(|h| h.borrow_mut().remove(&timer));
    }
    if p.started {
        let last = p.samples[(p.count.max(1) - 1) % p.samples.len()];
        report(drag_event(id, &p, DragPhase::Cancel, (last.0, last.1)));
    }
    Ok(())
}

fn entered(e: &UIElement, id: NodeId, a: &PointerRoutedEventArgs) -> Result<()> {
    if with(id, |l| l.mask) & (HOVER | MOVE) != 0 {
        let (x, y) = xy(&a.GetCurrentPoint(e)?);
        report(UiEvent::Pointer(
            id,
            PointerEvent {
                phase: PointerPhase::Enter,
                position: point(x, y),
            },
        ));
    }
    Ok(())
}

fn exited(e: &UIElement, id: NodeId, a: &PointerRoutedEventArgs) -> Result<()> {
    if with(id, |l| l.mask) & (HOVER | MOVE) != 0 {
        let (x, y) = xy(&a.GetCurrentPoint(e)?);
        report(UiEvent::Pointer(
            id,
            PointerEvent {
                phase: PointerPhase::Leave,
                position: point(x, y),
            },
        ));
    }
    Ok(())
}

/// Wheel notches are 120 units; a notch scrolls 100 points like
/// browsers. A precision touchpad sends smaller steps, already fine.
const NOTCH: f32 = 120.;
const POINTS_PER_NOTCH: f32 = 100.;

fn wheel(e: &UIElement, id: NodeId, a: &PointerRoutedEventArgs) -> Result<()> {
    let mask = with(id, |l| l.mask);
    if mask & (WHEEL | MAGNIFY) == 0 || a.Handled()? {
        return Ok(());
    }
    let p = a.GetCurrentPoint(e)?;
    let props = p.Properties()?;
    let raw = props.MouseWheelDelta()? as f32;
    let horizontal = props.IsHorizontalMouseWheel()?;
    // Ctrl + wheel zooms, as everywhere on Windows: precision touchpads
    // send their pinches so.
    if mask & MAGNIFY != 0 && !horizontal && modifiers(a).control {
        let (x, y) = xy(&p);
        report(UiEvent::Magnify(
            id,
            MagnifyEvent {
                scale: crate::pinch::wheel_scale(raw),
                position: point(x, y),
                phase: GesturePhase::None,
            },
        ));
        a.SetHandled(true)?;
        return Ok(());
    }
    if mask & WHEEL == 0 {
        return Ok(());
    }
    let precise = raw % NOTCH != 0.;
    let amount = raw / NOTCH * POINTS_PER_NOTCH;
    // Windows: positive turns the wheel away (scrolls up) or tilts it
    // right; Prism: positive scrolls toward the end.
    let delta = if horizontal {
        point(amount, 0.)
    } else {
        point(0., -amount)
    };
    let (x, y) = xy(&p);
    report(UiEvent::Wheel(
        id,
        WheelEvent {
            delta,
            position: point(x, y),
            modifiers: modifiers(a),
            phase: GesturePhase::None,
            precise,
        },
    ));
    // The node takes the scrolling: the scroll view around stays.
    a.SetHandled(true)?;
    Ok(())
}

// --- The dragged payload's preview (#251) -----------------------------------------------

impl Win {
    /// A see-through picture of node `id` at window point `at`, above
    /// everything and letting the pointer through; `None` removes it.
    pub fn drag_preview(&mut self, id: NodeId, at: Option<(f32, f32)>) {
        let r = (|| -> Result<()> {
            let Some((x, y)) = at else {
                if let Some((_, img)) = PREVIEW.with(|p| p.borrow_mut().take()) {
                    crate::view::detach(&ui(&img)?)?;
                }
                return Ok(());
            };
            let shown = PREVIEW.with(|p| p.borrow().as_ref().map(|(n, i)| (*n, i.clone())));
            let img = match shown {
                Some((n, img)) if n == id => img,
                other => {
                    if let Some((_, old)) = other {
                        crate::view::detach(&ui(&old)?)?;
                    }
                    let Some(v) = self.views.get(&id) else {
                        return Ok(());
                    };
                    let img = Image::new()?;
                    img.SetIsHitTestVisible(false)?;
                    img.SetOpacity(0.8)?;
                    let frame = self.scene.get(id).map(|n| n.frame).unwrap_or_default();
                    img.SetWidth(frame.w as f64)?;
                    img.SetHeight(frame.h as f64)?;
                    let bitmap = RenderTargetBitmap::new()?;
                    img.SetSource(&bitmap)?;
                    // The node as it is now; shown once rendered.
                    let _ = bitmap.RenderAsync(&v.outer())?;
                    self.overlays.Children()?.Append(&img)?;
                    PREVIEW.with(|p| *p.borrow_mut() = Some((id, img.clone())));
                    img
                }
            };
            let e = ui(&img)?;
            Canvas::SetLeft(&e, x as f64)?;
            Canvas::SetTop(&e, y as f64)?;
            Ok(())
        })();
        if let Err(e) = r {
            log!("drag preview of node {id}: {e}");
        }
    }
}

/// While a dragged payload stays near a scroll view's edge (#251): a step
/// every frame, the drag reported again over what moved under it.
pub fn autoscroll(s: &mut crate::imp::State) {
    if !s.renderer.autoscroll_due() || AUTOSCROLLING.with(|a| a.replace(true)) {
        return;
    }
    let ms = prism_native::dnd::AUTOSCROLL_INTERVAL.as_millis() as u32;
    unsafe {
        SetTimer(None::<HWND>, 0, ms, Some(autoscroll_step));
    }
}

thread_local! {
    static AUTOSCROLLING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

unsafe extern "system" fn autoscroll_step(_: HWND, _: u32, timer: usize, _: u32) {
    unsafe {
        let _ = KillTimer(None::<HWND>, timer);
    }
    AUTOSCROLLING.with(|a| a.set(false));
    crate::imp::with_state(|s| {
        if let Some((event, restyled)) = s.renderer.autoscroll() {
            if restyled {
                crate::imp::relayout(s);
            }
            let patches = s.runtime.dispatch(event);
            crate::imp::commit(s, patches);
        }
        autoscroll(s);
    });
}
