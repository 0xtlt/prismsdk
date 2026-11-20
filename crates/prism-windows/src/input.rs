//! Pointer, wheel, keys and focus: hit testing in the scene, the Fluent
//! hover / pressed looks, prism-native's interaction states (`.hover()`,
//! `.active()`, `.focus()` refinements), clicks, drags, Tab order and the
//! keyboard activation of controls. Everything that activates something
//! (the mouse, the keyboard, `PRISM_CLICK`, UI Automation's Invoke) goes
//! through [`activate`].

use std::cell::RefCell;

use prism_core::events::{DragEvent, DragPhase, PointerEvent, PointerPhase, pointer_mask};
use prism_core::menu::{MenuItemSpec, MenuSpec};
use prism_core::{Interaction, KeyEvent, Modifiers, NodeId, NodeKind, Point, UiEvent};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN,
    VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, HMENU, MENU_ITEM_FLAGS, MF_CHECKED, MF_GRAYED,
    MF_POPUP, MF_SEPARATOR, MF_STRING, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
};
use windows::core::PCWSTR;

use crate::controls;
use crate::imp::{State, dispatch};
use crate::paint::Part;
use crate::scene::{Node, Target};
use crate::win::{WM_PRISM_MENU, wide};

/// A press in progress.
#[derive(Clone, Copy, Debug)]
enum Press {
    /// A control or clickable view (the innermost pressable one).
    Node(NodeId),
    /// A tab, the header's Back, a bar button.
    Part(NodeId, Part),
    /// Dragging a scroll bar's thumb: the scroll view and where the
    /// pointer went down (offset then).
    Thumb(NodeId, f32),
}

#[derive(Default)]
struct Pointer {
    press: Option<Press>,
    /// Nodes reporting `Hover` that the pointer is over.
    hovered: Vec<NodeId>,
    /// Nodes reporting `Active` that are pressed.
    active: Vec<NodeId>,
    /// Nodes listening to pointer moves the pointer is over.
    over: Vec<NodeId>,
    /// A drag of a node listening to drags: (node, start in window
    /// coordinates, started).
    drag: Option<(NodeId, Point, bool)>,
    last: Point,
}

/// A menu waiting for the message loop: what, where (window, view
/// units), and the picker it chooses for.
type PendingMenu = (MenuSpec, f32, f32, Option<NodeId>);

thread_local! {
    static POINTER: RefCell<Pointer> = RefCell::new(Pointer::default());
    /// A menu to show from the message loop (it runs a modal loop):
    /// (menu, window point, a picker it chooses for).
    static MENU: RefCell<Option<PendingMenu>> = const { RefCell::new(None) };
}

pub fn modifiers() -> Modifiers {
    let down = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
    Modifiers {
        shift: down(VK_SHIFT.0),
        control: down(VK_CONTROL.0),
        alt: down(VK_MENU.0),
        meta: down(VK_LWIN.0) || down(VK_RWIN.0),
    }
}

/// Where `(x, y)` (window, view units) is in node `id`.
fn local(s: &State, id: NodeId, x: f32, y: f32) -> Point {
    let (ox, oy) = s.renderer.widgets.scene.origin(id);
    Point::new(x - ox, y - oy)
}

/// Whether a node takes presses itself.
fn pressable(n: &Node) -> bool {
    !n.a11y.disabled
        && !n.style.inert
        && (n.is_pressable() || matches!(n.kind, NodeKind::Slider | NodeKind::RadioGroup))
}

/// What a press at `(x, y)` would press.
fn press_target(s: &State, x: f32, y: f32) -> Option<Press> {
    let w = &s.renderer.widgets;
    let scene = &w.scene;
    let hit = scene.hit_test(x, y)?;
    for a in scene.chain(hit) {
        let n = scene.get(a)?;
        let p = local(s, a, x, y);
        if matches!(n.kind, NodeKind::Tabs | NodeKind::Stack)
            && let Some(part) = controls::part_at(&w.gfx, &w.texts, scene, a, p.x, p.y)
        {
            return Some(Press::Part(a, part));
        }
        if n.kind == NodeKind::Scroll && controls::scrollbar_hit(n, p.x, p.y) {
            let vertical = n.style.scroll_axis == prism_core::ScrollAxis::Vertical;
            return Some(Press::Thumb(a, if vertical { y } else { x }));
        }
        if pressable(n) {
            return Some(Press::Node(a));
        }
        if n.field.is_some() {
            return None;
        }
    }
    None
}

/// The pointer moved to `(x, y)` (window, view units).
pub fn pointer_move(s: &mut State, x: f32, y: f32) {
    let p = Point::new(x, y);
    // Dragging a slider, a scroll bar, a draggable node.
    let press = POINTER.with(|ptr| {
        let mut ptr = ptr.borrow_mut();
        ptr.last = p;
        ptr.press
    });
    match press {
        Some(Press::Node(id))
            if s.renderer
                .widgets
                .scene
                .get(id)
                .is_some_and(|n| n.kind == NodeKind::Slider) =>
        {
            slide(s, id, x);
        }
        Some(Press::Thumb(id, from)) => {
            let w = &mut s.renderer.widgets;
            if let Some(n) = w.scene.get(id) {
                let vertical = n.style.scroll_axis == prism_core::ScrollAxis::Vertical;
                let (view, content) = if vertical {
                    (n.frame.h, n.content.1)
                } else {
                    (n.frame.w, n.content.0)
                };
                let now = if vertical { y } else { x };
                let k = content / view.max(1.);
                let d = (now - from) * k;
                POINTER.with(|ptr| ptr.borrow_mut().press = Some(Press::Thumb(id, now)));
                if vertical {
                    w.scroll_by(id, 0., d);
                } else {
                    w.scroll_by(id, d, 0.);
                }
                scrolled(s, id);
            }
        }
        _ => {}
    }
    drag_move(s, p);
    update_hover(s, x, y);
}

fn drag_move(s: &mut State, p: Point) {
    let drag = POINTER.with(|ptr| ptr.borrow().drag);
    let Some((id, start, started)) = drag else {
        return;
    };
    let (dx, dy) = (p.x - start.x, p.y - start.y);
    if !started && dx * dx + dy * dy < 16. {
        return;
    }
    POINTER.with(|ptr| ptr.borrow_mut().drag = Some((id, start, true)));
    let origin = local(s, id, start.x, start.y);
    dispatch(UiEvent::Drag(
        id,
        DragEvent {
            phase: if started {
                DragPhase::Move
            } else {
                DragPhase::Start
            },
            start: origin,
            translation: Point::new(dx, dy),
            button: prism_core::PointerButton::Primary,
            modifiers: modifiers(),
        },
    ));
}

/// Hover: the Fluent look of the control under the pointer, the parts of
/// bars, prism-native's `Hover` state, pointer events.
pub fn update_hover(s: &mut State, x: f32, y: f32) {
    let w = &mut s.renderer.widgets;
    let hit = w.scene.hit_test(x, y);
    let chain = hit.map(|h| w.scene.chain(h)).unwrap_or_default();
    // The control drawn hovered, or a part of a bar.
    let mut hover_part = None;
    let mut hover_node = None;
    for &a in &chain {
        let Some(n) = w.scene.get(a) else { continue };
        let (ox, oy) = w.scene.origin(a);
        if matches!(n.kind, NodeKind::Tabs | NodeKind::Stack)
            && let Some(part) = controls::part_at(&w.gfx, &w.texts, &w.scene, a, x - ox, y - oy)
        {
            hover_part = Some((a, part));
            break;
        }
        if n.kind == NodeKind::Scroll && controls::scrollbar_hit(n, x - ox, y - oy) {
            hover_part = Some((a, Part::ScrollBar));
            break;
        }
        if pressable(n) || n.field.is_some() {
            hover_node = Some(a);
            break;
        }
    }
    if w.hover != hover_part {
        if let Some((p, _)) = w.hover {
            w.mark(p);
        }
        if let Some((p, _)) = hover_part {
            w.mark(p);
        }
        w.hover = hover_part;
    }
    let previous = w
        .scene
        .nodes
        .iter()
        .find(|(_, n)| n.hover)
        .map(|(id, _)| *id);
    if previous != hover_node {
        if let Some(p) = previous.and_then(|p| w.scene.get_mut(p)) {
            p.hover = false;
        }
        if let Some(n) = hover_node.and_then(|h| w.scene.get_mut(h)) {
            n.hover = true;
        }
        for id in previous.into_iter().chain(hover_node) {
            w.mark(id);
        }
    }
    // prism-native's hover state, and pointer events.
    let reporting: Vec<NodeId> = chain
        .iter()
        .copied()
        .filter(|&a| {
            w.scene
                .get(a)
                .is_some_and(|n| n.reports(Interaction::Hover))
        })
        .collect();
    let listening: Vec<NodeId> = chain
        .iter()
        .copied()
        .filter(|&a| {
            w.scene
                .get(a)
                .is_some_and(|n| n.pointer_mask & (pointer_mask::HOVER | pointer_mask::MOVE) != 0)
        })
        .collect();
    let (before, over_before) = POINTER.with(|ptr| {
        let mut ptr = ptr.borrow_mut();
        (
            std::mem::replace(&mut ptr.hovered, reporting.clone()),
            std::mem::replace(&mut ptr.over, listening.clone()),
        )
    });
    let mut restyled = false;
    for id in before.iter().filter(|b| !reporting.contains(b)) {
        restyled |= s.renderer.set_interaction(*id, Interaction::Hover, false);
    }
    for id in reporting.iter().filter(|r| !before.contains(r)) {
        restyled |= s.renderer.set_interaction(*id, Interaction::Hover, true);
    }
    for id in over_before.iter().filter(|b| !listening.contains(b)) {
        let p = local(s, *id, x, y);
        dispatch(UiEvent::Pointer(
            *id,
            PointerEvent {
                phase: PointerPhase::Leave,
                position: p,
            },
        ));
    }
    for id in &listening {
        let mask = s
            .renderer
            .widgets
            .scene
            .get(*id)
            .map_or(0, |n| n.pointer_mask);
        let p = local(s, *id, x, y);
        if !over_before.contains(id) {
            dispatch(UiEvent::Pointer(
                *id,
                PointerEvent {
                    phase: PointerPhase::Enter,
                    position: p,
                },
            ));
        } else if mask & pointer_mask::MOVE != 0 {
            dispatch(UiEvent::Pointer(
                *id,
                PointerEvent {
                    phase: PointerPhase::Move,
                    position: p,
                },
            ));
        }
    }
    if restyled {
        crate::imp::relayout(s);
    }
    s.renderer.widgets.flush();
}

/// The pointer left the window.
pub fn pointer_leave(s: &mut State) {
    update_hover(s, -1e6, -1e6);
}

/// A primary button press at `(x, y)`.
pub fn press(s: &mut State, main: HWND, x: f32, y: f32) {
    let target = press_target(s, x, y);
    let w = &mut s.renderer.widgets;
    w.focus_visible = false;
    unsafe {
        SetCapture(main);
    }
    // A draggable node under the pointer.
    let hit = w.scene.hit_test(x, y);
    let draggable = hit.and_then(|h| {
        w.scene
            .find_up(h, |n| n.pointer_mask & pointer_mask::DRAG != 0)
    });
    POINTER.with(|ptr| {
        let mut ptr = ptr.borrow_mut();
        ptr.press = target;
        ptr.drag = draggable.map(|d| (d, Point::new(x, y), false));
    });
    match target {
        Some(Press::Node(id)) => {
            if let Some(n) = w.scene.get_mut(id) {
                n.pressed = true;
            }
            w.mark(id);
            // Controls take the keyboard focus (without its ring).
            set_focus(s, Some(id), false);
            let kind = s.renderer.widgets.scene.get(id).map(|n| n.kind);
            if kind == Some(NodeKind::Slider) {
                slide(s, id, x);
            }
        }
        Some(Press::Part(id, part)) => {
            w.pressed = Some((id, part));
            w.mark(id);
        }
        Some(Press::Thumb(id, _)) => {
            w.pressed = Some((id, Part::ScrollBar));
            w.mark(id);
        }
        None => {
            // A field takes the focus itself; elsewhere the focus goes.
            let field = hit.and_then(|h| w.scene.find_up(h, |n| n.field.is_some()));
            match field.and_then(|f| w.field_of(f)) {
                Some(f) => unsafe {
                    let _ = SetFocus(Some(f));
                },
                None => set_focus(s, None, false),
            }
        }
    }
    // prism-native's `Active` state for the nodes under the pointer.
    let w = &s.renderer.widgets;
    let active: Vec<NodeId> = hit
        .map(|h| w.scene.chain(h))
        .unwrap_or_default()
        .into_iter()
        .filter(|&a| {
            w.scene
                .get(a)
                .is_some_and(|n| n.reports(Interaction::Active))
        })
        .collect();
    let mut restyled = false;
    for &id in &active {
        restyled |= s.renderer.set_interaction(id, Interaction::Active, true);
    }
    POINTER.with(|ptr| ptr.borrow_mut().active = active);
    if restyled {
        crate::imp::relayout(s);
    }
    s.renderer.widgets.flush();
}

/// The primary button went up at `(x, y)`: activates what was pressed if
/// the pointer is still on it.
pub fn release(s: &mut State, x: f32, y: f32, count: u32) {
    unsafe {
        let _ = ReleaseCapture();
    }
    let (press, active, drag) = POINTER.with(|ptr| {
        let mut ptr = ptr.borrow_mut();
        (
            ptr.press.take(),
            std::mem::take(&mut ptr.active),
            ptr.drag.take(),
        )
    });
    let mut restyled = false;
    for id in active {
        restyled |= s.renderer.set_interaction(id, Interaction::Active, false);
    }
    if let Some((id, start, true)) = drag {
        let origin = local(s, id, start.x, start.y);
        dispatch(UiEvent::Drag(
            id,
            DragEvent {
                phase: DragPhase::End,
                start: origin,
                translation: Point::new(x - start.x, y - start.y),
                button: prism_core::PointerButton::Primary,
                modifiers: modifiers(),
            },
        ));
    }
    let w = &mut s.renderer.widgets;
    if let Some((p, _)) = w.pressed.take() {
        w.mark(p);
    }
    match press {
        Some(Press::Node(id)) => {
            if let Some(n) = w.scene.get_mut(id) {
                n.pressed = false;
            }
            w.mark(id);
            let still =
                press_target(s, x, y).is_some_and(|p| matches!(p, Press::Node(i) if i == id));
            let dragged = drag.is_some_and(|(_, _, started)| started);
            let kind = s.renderer.widgets.scene.get(id).map(|n| n.kind);
            if still && !dragged && kind != Some(NodeKind::Slider) {
                let p = local(s, id, x, y);
                click(s, id, Some(p), count);
            }
        }
        Some(Press::Part(id, part)) => {
            let still = press_target(s, x, y)
                .is_some_and(|p| matches!(p, Press::Part(i, q) if i == id && q == part));
            if still {
                activate_part(s, id, part);
            }
        }
        _ => {}
    }
    if restyled {
        crate::imp::relayout(s);
    }
    s.renderer.widgets.flush();
}

/// A click on node `id` (at `p` in it), as its kind takes it.
fn click(s: &mut State, id: NodeId, p: Option<Point>, count: u32) {
    let Some(n) = s.renderer.widgets.scene.get(id) else {
        return;
    };
    match n.kind {
        NodeKind::RadioGroup => {
            let c = prism_core::controls::RadioConfig::decode(&n.config);
            if let Some(p) = p {
                let i = if c.horizontal {
                    // Approximation: equal shares of the width.
                    (p.x / (n.frame.w / c.options.len().max(1) as f32)) as usize
                } else {
                    (p.y / crate::fluent::metrics::CONTROL_HEIGHT) as usize
                };
                if i < c.options.len() {
                    dispatch(UiEvent::Change(id, i as f64));
                }
            }
        }
        _ => {
            let modifiers = modifiers();
            if count > 1 || modifiers != Modifiers::default() {
                dispatch(UiEvent::ClickWith(
                    id,
                    prism_core::ClickEvent {
                        position: p,
                        modifiers,
                        count: count.max(1),
                        button: prism_core::PointerButton::Primary,
                        text_offset: None,
                    },
                ));
                return;
            }
            activate(s, Target::Node(id), p);
        }
    }
}

fn activate_part(s: &mut State, id: NodeId, part: Part) {
    let target = match part {
        Part::Tab(i) => Target::Tab(id, i),
        Part::Back => Target::Back(id),
        Part::BarItem(i) => {
            let screen = s.renderer.widgets.scene.pages(id).last().copied();
            match screen {
                Some(sc) => Target::BarItem(sc, i),
                None => return,
            }
        }
        Part::ScrollBar => return,
    };
    activate(s, target, None);
}

/// Activates what `PRISM_CLICK`, the keyboard, UI Automation or a click
/// pressed.
pub fn activate(s: &mut State, target: Target, at: Option<Point>) {
    let scene = &s.renderer.widgets.scene;
    match target {
        Target::Node(id) => {
            let Some(n) = scene.get(id) else { return };
            match n.kind {
                NodeKind::Toggle | NodeKind::Checkbox => {
                    dispatch(UiEvent::Change(id, if n.value >= 0.5 { 0. } else { 1. }))
                }
                NodeKind::Picker => {
                    let c = prism_core::controls::PickerConfig::decode(&n.config);
                    let menu = MenuSpec {
                        title: String::new(),
                        items: c
                            .options
                            .iter()
                            .enumerate()
                            .map(|(i, o)| MenuItemSpec::Action {
                                id: i as u64,
                                title: o.clone(),
                                icon: None,
                                shortcut: None,
                                shortcut_hint: false,
                                role: Default::default(),
                                checked: Some(n.value as usize == i && n.value >= 0.),
                                enabled: true,
                            })
                            .collect(),
                    };
                    let r = scene.window_rect(id);
                    MENU.with(|m| *m.borrow_mut() = Some((menu, r.x, r.bottom(), Some(id))));
                    post(crate::imp::main_hwnd());
                }
                NodeKind::DatePicker => {
                    crate::win::log!("date pickers have no calendar yet (phase 3)");
                }
                _ => match at {
                    Some(p) => dispatch(UiEvent::ClickAt(id, p)),
                    None => dispatch(UiEvent::Click(id)),
                },
            }
        }
        Target::Tab(tabs, i) => dispatch(UiEvent::Change(tabs, i as f64)),
        Target::Back(stack) => {
            let depth = scene.pages(stack).len();
            dispatch(UiEvent::Back(stack, depth.saturating_sub(1)));
        }
        Target::BarItem(screen, i) => dispatch(UiEvent::Change(screen, i as f64)),
    }
}

/// Sets a slider from the pointer's `x` (window).
fn slide(s: &mut State, id: NodeId, x: f32) {
    let w = &s.renderer.widgets;
    let Some(n) = w.scene.get(id) else { return };
    let (ox, _) = w.scene.origin(id);
    let c = prism_core::controls::SliderConfig::decode(&n.config);
    let thumb = crate::fluent::metrics::SLIDER_THUMB;
    let t = ((x - ox - thumb / 2.) / (n.frame.w - thumb).max(1.)).clamp(0., 1.) as f64;
    let v = c.snap(c.min + t * (c.max - c.min));
    if (v - n.value).abs() > f64::EPSILON {
        dispatch(UiEvent::Change(id, v));
    }
}

/// The wheel turned by `(dx, dy)` points (positive: toward the end) over
/// `(x, y)`: wheel listeners hear it, then the scroll views under it move.
pub fn wheel(s: &mut State, x: f32, y: f32, dx: f32, dy: f32, precise: bool) {
    let w = &mut s.renderer.widgets;
    let Some(hit) = w.scene.hit_test(x, y) else {
        return;
    };
    if let Some(l) = w
        .scene
        .find_up(hit, |n| n.pointer_mask & pointer_mask::WHEEL != 0)
    {
        let (ox, oy) = w.scene.origin(l);
        dispatch(UiEvent::Wheel(
            l,
            prism_core::WheelEvent {
                delta: Point::new(dx, dy),
                position: Point::new(x - ox, y - oy),
                modifiers: modifiers(),
                phase: prism_core::GesturePhase::None,
                precise,
            },
        ));
    }
    let vertical = dy != 0.;
    for sc in w.scene.scrollers(hit, vertical) {
        let moved = if vertical {
            w.scroll_by(sc, 0., dy)
        } else {
            w.scroll_by(sc, dx, 0.)
        };
        if moved {
            scrolled(s, sc);
            break;
        }
    }
    s.renderer.widgets.flush();
    update_hover(s, x, y);
}

/// A scroll view moved: its listeners hear it.
pub fn scrolled(s: &mut State, id: NodeId) {
    if let Some(n) = s.renderer.widgets.scene.get(id)
        && n.scroll_events
    {
        dispatch(UiEvent::Scroll(id, n.scroll.1, n.frame.h));
    }
}

/// Gives node `id` the keyboard focus (`None`: nothing), with its ring
/// when it came from the keyboard. Fields take the Win32 focus.
pub fn set_focus(s: &mut State, id: Option<NodeId>, keyboard: bool) {
    let w = &mut s.renderer.widgets;
    let previous = w.focus;
    w.focus_visible = keyboard;
    if let Some(f) = id.and_then(|i| w.field_of(i)) {
        unsafe {
            let _ = SetFocus(Some(f));
        }
        return;
    }
    unsafe {
        let _ = SetFocus(Some(w.hwnd));
    }
    if previous == id {
        if let Some(p) = previous {
            w.mark(p);
        }
        w.flush();
        return;
    }
    w.focus = id;
    for (n, on) in [(previous, false), (id, true)] {
        let Some(n) = n else { continue };
        if let Some(node) = w.scene.get_mut(n) {
            node.focused = on;
        }
        w.mark(n);
    }
    let mut restyled = false;
    if let Some(p) = previous {
        restyled |= s.renderer.set_interaction(p, Interaction::Focus, false);
    }
    if let Some(n) = id {
        restyled |= s.renderer.set_interaction(n, Interaction::Focus, true);
        if keyboard {
            prism_native::Widgets::scroll_into_view(&mut s.renderer.widgets, &n);
        }
    }
    if restyled {
        crate::imp::relayout(s);
    }
    for e in s.renderer.take_focus_changes() {
        dispatch(e);
    }
    crate::uia::focus_changed(s.renderer.widgets.hwnd, id);
    s.renderer.widgets.flush();
}

/// Tab / Shift+Tab.
pub fn focus_step(s: &mut State, back: bool) {
    let w = &s.renderer.widgets;
    let next = w.scene.next_focus(w.focus, back);
    set_focus(s, next, true);
}

/// A key the window got (no text field has the focus): Tab moves the
/// focus, Enter / Space activate, arrows switch tabs and move sliders,
/// page keys scroll. Returns whether it was used.
pub fn key(s: &mut State, name: &str) -> bool {
    let m = modifiers();
    let w = &s.renderer.widgets;
    let focus = w.focus.filter(|f| w.scene.is_shown(*f));
    let kind = focus.and_then(|f| w.scene.get(f)).map(|n| n.kind);
    match name {
        "Tab" => {
            focus_step(s, m.shift);
            true
        }
        "Enter" | " " => match focus {
            Some(f) if kind != Some(NodeKind::Tabs) => {
                activate(s, Target::Node(f), None);
                true
            }
            _ => false,
        },
        "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown"
            if matches!(
                kind,
                Some(NodeKind::Tabs | NodeKind::Slider | NodeKind::RadioGroup)
            ) =>
        {
            let f = focus.expect("focus");
            let n = w.scene.get(f).expect("node");
            let up = matches!(name, "ArrowRight" | "ArrowDown");
            match n.kind {
                NodeKind::Tabs => {
                    let count = w.scene.pages(f).len();
                    let i = n.selected() as i64 + if up { 1 } else { -1 };
                    if count > 0 {
                        dispatch(UiEvent::Change(f, i.rem_euclid(count as i64) as f64));
                    }
                }
                NodeKind::Slider => {
                    let c = prism_core::controls::SliderConfig::decode(&n.config);
                    let step = if c.step > 0. {
                        c.step
                    } else {
                        (c.max - c.min) / 100.
                    };
                    let v = (n.value + if up { step } else { -step }).clamp(c.min, c.max);
                    dispatch(UiEvent::Change(f, c.snap(v)));
                }
                _ => {
                    let c = prism_core::controls::RadioConfig::decode(&n.config);
                    let count = c.options.len() as i64;
                    if count > 0 {
                        let i = n.value as i64 + if up { 1 } else { -1 };
                        dispatch(UiEvent::Change(f, i.rem_euclid(count) as f64));
                    }
                }
            }
            true
        }
        "ArrowUp" | "ArrowDown" | "PageUp" | "PageDown" | "Home" | "End" => {
            // The scroll view around the focus, else the page's.
            let scene = &w.scene;
            let target = focus
                .and_then(|f| scene.scrollers(f, true).into_iter().next())
                .or_else(|| {
                    scene.shown_in_order().into_iter().find(|&id| {
                        scene
                            .get(id)
                            .is_some_and(|n| n.scrolls(true) && n.max_scroll().1 > 0.)
                    })
                });
            let Some(sc) = target else { return false };
            let n = scene.get(sc).expect("scroll");
            let page = n.frame.h * 0.875;
            let dy = match name {
                "ArrowUp" => -48.,
                "ArrowDown" => 48.,
                "PageUp" => -page,
                "PageDown" => page,
                "Home" => -n.scroll.1,
                _ => n.max_scroll().1 - n.scroll.1,
            };
            if s.renderer.widgets.scroll_by(sc, 0., dy) {
                scrolled(s, sc);
            }
            s.renderer.widgets.flush();
            true
        }
        "Escape" => {
            // Closes the top overlay.
            let top = w.scene.open_overlays().last().copied();
            match top {
                Some(o) => {
                    dispatch(UiEvent::Dismiss(o, prism_core::DismissReason::Escape));
                    true
                }
                None => false,
            }
        }
        "ArrowLeft" if m.alt => {
            // Back, like a browser and WinUI's NavigationView.
            let stack = w.scene.shown_in_order().into_iter().rev().find(|&id| {
                w.scene.get(id).is_some_and(|n| n.kind == NodeKind::Stack) && w.scene.has_back(id)
            });
            match stack {
                Some(st) => {
                    activate(s, Target::Back(st), None);
                    true
                }
                None => false,
            }
        }
        _ => false,
    }
}

/// The context menu of what is under `(x, y)` (right click, the menu key).
pub fn context_menu(s: &mut State, x: f32, y: f32) {
    let w = &s.renderer.widgets;
    let Some(hit) = w.scene.hit_test(x, y) else {
        return;
    };
    if let Some(id) = w.scene.find_up(hit, |n| n.menu.is_some())
        && let Some(menu) = w.scene.get(id).and_then(|n| n.menu.clone())
    {
        MENU.with(|m| *m.borrow_mut() = Some((menu, x, y, None)));
        post(w.hwnd);
    }
}

/// `Widgets::show_menu`: shown from the message loop.
pub fn post_menu(menu: MenuSpec, x: f32, y: f32) {
    MENU.with(|m| *m.borrow_mut() = Some((menu, x, y, None)));
    post(crate::imp::main_hwnd());
}

fn post(h: HWND) {
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            Some(h),
            WM_PRISM_MENU,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(0),
        );
    }
}

/// The menu text of an item: its title, then its shortcut after a tab.
fn item_text(title: &str, shortcut: &Option<String>) -> Vec<u16> {
    match shortcut {
        Some(k) if !k.is_empty() => wide(&format!("{title}\t{k}")),
        _ => wide(title),
    }
}

fn build_menu(spec: &MenuSpec) -> Option<HMENU> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        for item in &spec.items {
            match item {
                MenuItemSpec::Action {
                    id,
                    title,
                    shortcut,
                    checked,
                    enabled,
                    ..
                } => {
                    let mut flags = MF_STRING;
                    if !enabled {
                        flags |= MF_GRAYED;
                    }
                    if *checked == Some(true) {
                        flags |= MF_CHECKED;
                    }
                    let t = item_text(title, shortcut);
                    // Ids from 1: 0 is "nothing chosen".
                    let _ = AppendMenuW(
                        menu,
                        flags,
                        (*id as usize).wrapping_add(1),
                        PCWSTR(t.as_ptr()),
                    );
                }
                MenuItemSpec::System {
                    title, shortcut, ..
                } => {
                    let t = item_text(title, shortcut);
                    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(t.as_ptr()));
                }
                MenuItemSpec::Separator => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
                MenuItemSpec::Submenu {
                    title, menu: sub, ..
                } => {
                    if let Some(h) = build_menu(sub) {
                        let t = wide(title);
                        let _ = AppendMenuW(
                            menu,
                            MF_POPUP | MENU_ITEM_FLAGS(0),
                            h.0 as usize,
                            PCWSTR(t.as_ptr()),
                        );
                    }
                }
            }
        }
        Some(menu)
    }
}

/// Shows the pending menu (`WM_PRISM_MENU`, outside any state borrow: a
/// menu runs its own modal loop) and reports the choice.
pub fn show_pending_menu(main: HWND, scale: f32) {
    let Some((spec, x, y, picker)) = MENU.with(|m| m.borrow_mut().take()) else {
        return;
    };
    let Some(menu) = build_menu(&spec) else {
        return;
    };
    let mut p = POINT {
        x: (x * scale) as i32,
        y: (y * scale) as i32,
    };
    let chosen = unsafe {
        let _ = ClientToScreen(main, &mut p);
        let r = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
            p.x,
            p.y,
            main,
            None,
        );
        let _ = DestroyMenu(menu);
        r.0 as usize
    };
    if chosen == 0 {
        return;
    }
    let id = (chosen - 1) as u64;
    match picker {
        Some(p) => dispatch(UiEvent::Change(p, id as f64)),
        None => dispatch(UiEvent::MenuSelect(id)),
    }
}

/// The node with the keyboard focus: a field's when its editor has the
/// Win32 focus.
pub fn focused_node(s: &State) -> Option<NodeId> {
    let f = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() };
    crate::fields::node_of(f).or(s.renderer.widgets.focus)
}

/// A key press for the app (`on_key_down`), before the focused control
/// acts on it. Returns whether the app took it (`consume_keys`).
pub fn key_event(s: &mut State, vk: u16, typed: Option<char>, repeat: bool, up: bool) -> bool {
    let Some(name) = crate::keys::key_name(vk, typed) else {
        return false;
    };
    let modifiers = modifiers();
    let focus = focused_node(s);
    let scene = &s.renderer.widgets.scene;
    let any_global = scene.nodes.values().any(|n| n.keys_global);
    let target = focus.and_then(|f| scene.find_up(f, |n| n.keys_local));
    let editing = focus
        .and_then(|f| scene.get(f))
        .is_some_and(|n| n.field.is_some());
    let typing =
        editing && crate::keys::types_text(&name, modifiers.control, modifiers.alt, modifiers.meta);
    if target.is_none() && (typing || !any_global) {
        return false;
    }
    let (selection, edges) = match focus.and_then(|f| s.renderer.widgets.field_of(f)) {
        Some(h) => {
            let text = crate::fields::text(h);
            let (a, b) = crate::fields::selection(h);
            let a = prism_native::fields::utf16_to_chars(&text, a);
            let b = prism_native::fields::utf16_to_chars(&text, b);
            (Some(prism_core::SelectionEvent::new(a, b)), None)
        }
        None => (None, None),
    };
    let key = KeyEvent {
        key: name.clone(),
        modifiers,
        repeat,
        selection,
        edges,
    };
    if up {
        dispatch(UiEvent::KeyUp(target, key));
        return false;
    }
    let claimed = target.is_some_and(|t| Some(t) == focus && crate::keys::claims(&name));
    let consumed = s.runtime.consumes_key_event(focus, &key);
    dispatch(UiEvent::Key(target, key));
    claimed || consumed
}
