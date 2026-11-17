//! GTK widgets for Prism nodes, the window and the event loop.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::OnceLock;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use prism_core::{
    Accessibility, AppConfig, AppInit, Appearance, Backend, Color, NodeId, NodeKind, Runtime,
    Style, TextAlign, UiEvent,
};
use prism_native::{Insets, NativeRenderer, Viewport, Widgets};

use crate::controls;
use crate::layout;
use crate::text;

const HEADER: f32 = 44.;
const TAB_BAR: f32 = 52.;

static APP: OnceLock<(AppConfig, AppInit)> = OnceLock::new();

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

pub struct NavStack {
    root: gtk::Box,
    header: gtk::Box,
    back: gtk::Button,
    title: gtk::Label,
    screens: RefCell<Vec<View>>,
    /// Inside a modal: the first screen offers "Close".
    in_sheet: Cell<bool>,
    // --- Hidden header (#8) ---
    /// No header bar at all (`Router::header(false)`).
    no_header: Cell<bool>,
}

pub struct TabBar {
    root: gtk::Box,
    bar: gtk::Box,
    tabs: RefCell<Vec<View>>,
    selected: Cell<usize>,
}

pub struct Sheet {
    window: gtk::Window,
    content: gtk::Box,
    children: RefCell<Vec<View>>,
}

#[derive(Clone)]
pub enum View {
    /// view, screen, tab: a Prism-laid-out container.
    Container(NodeId, NodeKind, gtk::Box),
    Text(NodeId, gtk::Label),
    Button(NodeId, gtk::Button),
    Input(NodeId, gtk::Entry),
    Toggle(NodeId, gtk::Switch),
    Progress(NodeId, gtk::ProgressBar),
    /// SF Symbol names drawn as glyphs (see prism_core::glyphs).
    Image(NodeId, gtk::Label),
    Picture(NodeId, gtk::Picture),
    Timer(NodeId, gtk::Label),
    Scroll(NodeId, gtk::ScrolledWindow, gtk::Box),
    Stack(NodeId, Rc<NavStack>),
    Tabs(NodeId, Rc<TabBar>),
    Sheet(NodeId, Rc<Sheet>),
    /// Vector drawing (cairo), with its paths in points.
    Shape(NodeId, gtk::DrawingArea, Rc<RefCell<Vec<prism_core::Path>>>),
    /// 3D scene (OpenGL).
    Scene(NodeId, Rc<crate::scene3d::SceneView>),
    Slider(
        NodeId,
        gtk::Scale,
        Rc<Cell<prism_core::controls::SliderConfig>>,
    ),
    Picker(NodeId, Rc<controls::Picker>),
    DatePicker(NodeId, Rc<controls::DatePicker>),
    TextArea(NodeId, Rc<controls::TextArea>),
    Spinner(NodeId, gtk::Spinner),
    // Choice controls and overlays (#18, #19).
    Check(NodeId, gtk::CheckButton),
    Radio(NodeId, Rc<controls::RadioGroup>),
    Overlay(NodeId, Rc<OverlayView>),
    /// A video (#207): GTK's player (GStreamer) with its own controls.
    Video(NodeId, gtk::Video),
}

thread_local! {
    /// Whether shapes and 3D scenes draw the dark variants of their colors.
    static SHAPE_DARK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the dark appearance is on (for drawings: shapes, 3D scenes).
pub(crate) fn dark() -> bool {
    SHAPE_DARK.with(Cell::get)
}

/// Fills, then strokes, each path with cairo.
fn draw_paths(cr: &gtk::cairo::Context, paths: &[prism_core::Path]) {
    use prism_core::Segment as S;
    let dark = SHAPE_DARK.with(|d| d.get());
    let source = |c: Color| {
        let c = if dark {
            c.dark_variant()
        } else {
            c.light_variant()
        };
        cr.set_source_rgba(c.r as f64, c.g as f64, c.b as f64, c.a as f64);
    };
    for p in paths {
        cr.new_path();
        for s in &p.segments {
            match *s {
                S::Move(x, y) => cr.move_to(x as f64, y as f64),
                S::Line(x, y) => cr.line_to(x as f64, y as f64),
                S::Cubic(a, b, c, d, e, f) => {
                    cr.curve_to(a as f64, b as f64, c as f64, d as f64, e as f64, f as f64)
                }
                S::Close => cr.close_path(),
                S::Arc { .. } => {}
            }
        }
        if let Some(c) = p.fill {
            source(c);
            cr.set_fill_rule(if p.even_odd {
                gtk::cairo::FillRule::EvenOdd
            } else {
                gtk::cairo::FillRule::Winding
            });
            let _ = cr.fill_preserve();
        }
        if let Some(c) = p.stroke {
            source(c);
            cr.set_line_width(p.width as f64);
            let (cap, join) = if p.round {
                (gtk::cairo::LineCap::Round, gtk::cairo::LineJoin::Round)
            } else {
                (gtk::cairo::LineCap::Butt, gtk::cairo::LineJoin::Miter)
            };
            cr.set_line_cap(cap);
            cr.set_line_join(join);
            match p.dash {
                Some((d, g)) => cr.set_dash(&[d as f64, g as f64], 0.),
                None => cr.set_dash(&[], 0.),
            }
            let _ = cr.stroke_preserve();
        }
        cr.new_path();
    }
}

impl View {
    fn id(&self) -> NodeId {
        match self {
            View::Container(id, ..)
            | View::Text(id, _)
            | View::Button(id, _)
            | View::Input(id, _)
            | View::Toggle(id, _)
            | View::Progress(id, _)
            | View::Image(id, _)
            | View::Picture(id, _)
            | View::Timer(id, _)
            | View::Scroll(id, ..)
            | View::Stack(id, _)
            | View::Tabs(id, _)
            | View::Sheet(id, _)
            | View::Shape(id, ..)
            | View::Scene(id, _)
            | View::Slider(id, ..)
            | View::Picker(id, _)
            | View::DatePicker(id, _)
            | View::TextArea(id, _)
            | View::Spinner(id, _)
            | View::Check(id, _)
            | View::Radio(id, _)
            | View::Overlay(id, _)
            | View::Video(id, _) => *id,
        }
    }

    /// The widget placed in the parent.
    fn widget(&self) -> gtk::Widget {
        match self {
            View::Container(_, _, w) => w.clone().upcast(),
            View::Text(_, w) | View::Image(_, w) | View::Timer(_, w) => w.clone().upcast(),
            View::Button(_, w) => w.clone().upcast(),
            View::Input(_, w) => w.clone().upcast(),
            View::Toggle(_, w) => w.clone().upcast(),
            View::Progress(_, w) => w.clone().upcast(),
            View::Picture(_, w) => w.clone().upcast(),
            View::Scroll(_, w, _) => w.clone().upcast(),
            View::Stack(_, s) => s.root.clone().upcast(),
            View::Tabs(_, t) => t.root.clone().upcast(),
            View::Sheet(_, s) => s.content.clone().upcast(),
            View::Shape(_, w, _) => w.clone().upcast(),
            View::Scene(_, s) => s.area.clone().upcast(),
            View::Slider(_, w, _) => w.clone().upcast(),
            View::Picker(_, p) => p.dropdown.clone().upcast(),
            View::DatePicker(_, p) => p.button.clone().upcast(),
            View::TextArea(_, t) => t.root.clone().upcast(),
            View::Spinner(_, w) => w.clone().upcast(),
            View::Check(_, b) => b.clone().upcast(),
            View::Radio(_, r) => r.root.clone().upcast(),
            View::Overlay(_, o) => o.anchor.clone().upcast(),
            View::Video(_, v) => v.clone().upcast(),
        }
    }
}

// ---------------------------------------------------------------------------
// Overlays (#19): a popover anchored to the overlay's parent, or a modal
// window for sheets and dialogs
// ---------------------------------------------------------------------------

/// Marks a popover a press around closes while it leaves the focus to its
/// anchor (#182).
const LIGHT_BESIDE_FOCUS: &str = "prism-light-beside-focus";

/// Autohide (close on a click outside, which also grabs the keyboard focus)
/// unless the popover is fixed, manual, or leaves the focus to its anchor;
/// one that leaves the focus never takes it, and a press around it closes
/// it through `OverlayView::watch_outside` instead (#182).
fn set_popover_mode(p: &gtk::Popover, config: prism_core::controls::OverlayConfig) {
    p.set_autohide(!(config.fixed || config.manual || config.keep_focus));
    p.set_can_focus(!config.keep_focus);
    if config.light_dismiss_beside_focus() {
        p.add_css_class(LIGHT_BESIDE_FOCUS);
    } else {
        p.remove_css_class(LIGHT_BESIDE_FOCUS);
    }
    p.set_has_arrow(!config.bare);
    set_bare(p, config.bare);
}

/// A bare dialog (#166): no box of its own (the `prism-bare` rules take
/// GTK's background, border, padding and shadow away).
fn set_bare(w: &impl IsA<gtk::Widget>, bare: bool) {
    if bare {
        w.add_css_class("prism-bare");
    } else {
        w.remove_css_class("prism-bare");
    }
}

/// On its side of `parent` (GTK takes the opposite side without room
/// there), pointing at the part of `parent` it lines up with: GTK centers
/// the popover on that part (#113).
fn place_popover(
    p: &gtk::Popover,
    parent: &gtk::Widget,
    config: prism_core::controls::OverlayConfig,
    size: (i32, i32),
) {
    use prism_core::PopoverSide as S;
    let rtl = parent.direction() == gtk::TextDirection::Rtl;
    // GTK's left and right are the screen's.
    p.set_position(match config.side {
        S::Bottom => gtk::PositionType::Bottom,
        S::Top => gtk::PositionType::Top,
        S::Trailing if rtl => gtk::PositionType::Left,
        S::Trailing => gtk::PositionType::Right,
        S::Leading if rtl => gtk::PositionType::Right,
        S::Leading => gtk::PositionType::Left,
    });
    let (w, h) = (parent.width(), parent.height());
    let rect = if config.side.beside() {
        // Start is the top (GTK's y goes down).
        let (o, l) = config.align.arrow_span(h as f64, size.1 as f64, true);
        gdk::Rectangle::new(0, o.round() as i32, w, (l.round() as i32).max(1))
    } else {
        let (o, l) = config.align.arrow_span(w as f64, size.0 as f64, !rtl);
        gdk::Rectangle::new(o.round() as i32, 0, (l.round() as i32).max(1), h)
    };
    p.set_pointing_to(Some(&rect));
}

/// Shown in a `GtkPopover`: popovers, and dialogs placed beside their
/// anchor (#79). An autohiding popover keeps the focus inside and closes
/// on Escape or a click outside, like the dialog.
fn popover_kind(config: prism_core::controls::OverlayConfig) -> bool {
    config.anchored_side().is_some()
}

pub struct OverlayView {
    id: NodeId,
    /// Zero-size placeholder in the parent (the popover's anchor is the
    /// parent).
    anchor: gtk::Box,
    /// Holds the content node, at its Prism frame.
    content: gtk::Box,
    popover: RefCell<Option<gtk::Popover>>,
    window: RefCell<Option<gtk::Window>>,
    config: Cell<prism_core::controls::OverlayConfig>,
    open: Cell<bool>,
    /// Content size from the last layout.
    size: Cell<(i32, i32)>,
    /// The press watcher on the anchor's window of a popover that leaves
    /// the focus to its anchor (#182).
    outside: RefCell<Option<(gtk::Window, gtk::GestureClick)>>,
}

impl OverlayView {
    /// Builds the popover or window `config` asks for.
    fn configure(
        self: &Rc<Self>,
        config: prism_core::controls::OverlayConfig,
        main: &gtk::ApplicationWindow,
        keys: &Rc<KeyState>,
    ) {
        use prism_core::OverlayKind;
        let old = self.config.replace(config);
        let built = self.popover.borrow().is_some() || self.window.borrow().is_some();
        let as_popover = popover_kind(config);
        if built && popover_kind(old) == as_popover {
            if let Some(p) = self.popover.borrow().as_ref() {
                set_popover_mode(p, config);
                if let Some(parent) = p.parent() {
                    place_popover(p, &parent, config, self.size.get());
                }
            }
            if let Some(w) = self.window.borrow().as_ref() {
                w.set_deletable(!config.fixed);
                w.set_decorated(!config.bare);
                set_bare(w, config.bare);
            }
            return;
        }
        self.teardown();
        let id = self.id;
        if as_popover {
            let p = gtk::Popover::new();
            set_popover_mode(&p, config);
            p.set_child(Some(&self.content));
            // An Escape just before it closes is its reason (#156); GTK's
            // autohide does not say.
            let keys = gtk::EventControllerKey::new();
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            keys.connect_key_pressed(|_, key, _, _| {
                if key == gdk::Key::Escape {
                    ESCAPE_AT.set(Some(std::time::Instant::now()));
                }
                glib::Propagation::Proceed
            });
            p.add_controller(keys);
            let weak = Rc::downgrade(self);
            p.connect_closed(move |p| {
                let Some(o) = weak.upgrade() else { return };
                if !o.open.get() {
                    return;
                }
                if o.config.get().fixed {
                    // Escape closed a fixed popover: it stays.
                    let p = p.clone();
                    glib::idle_add_local_once(move || p.popup());
                } else {
                    o.open.set(false);
                    let escape = ESCAPE_AT
                        .take()
                        .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(500));
                    let reason = if escape {
                        prism_core::DismissReason::Escape
                    } else {
                        prism_core::DismissReason::Outside
                    };
                    dispatch(UiEvent::Dismiss(id, reason));
                }
            });
            *self.popover.borrow_mut() = Some(p);
        } else {
            let w = gtk::Window::builder()
                .transient_for(main)
                .modal(true)
                .destroy_with_parent(true)
                .resizable(config.kind == OverlayKind::Sheet)
                .deletable(!config.fixed)
                .decorated(!config.bare)
                .accessible_role(gtk::AccessibleRole::Dialog)
                .child(&self.content)
                .build();
            let weak = Rc::downgrade(self);
            w.connect_close_request(move |_| {
                if let Some(o) = weak.upgrade() {
                    o.user_closed(prism_core::DismissReason::Back);
                }
                glib::Propagation::Stop
            });
            // Escape closes it; the app's key handlers see the dialog's keys.
            let esc = gtk::EventControllerKey::new();
            let weak = Rc::downgrade(self);
            esc.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape
                    && let Some(o) = weak.upgrade()
                    && !o.config.get().fixed
                {
                    o.user_closed(prism_core::DismissReason::Escape);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            w.add_controller(esc);
            attach_keys(&w, keys.clone());
            set_bare(&w, config.bare);
            *self.window.borrow_mut() = Some(w);
        }
        if self.open.get() {
            self.show();
        }
    }

    /// Escape or the close button: closes at once (unless fixed) and tells
    /// the app, which keeps it closed or opens it again.
    fn user_closed(&self, reason: prism_core::DismissReason) {
        if self.config.get().fixed || !self.open.get() {
            return;
        }
        self.open.set(false);
        if let Some(w) = self.window.borrow().as_ref() {
            w.set_visible(false);
        }
        dispatch(UiEvent::Dismiss(self.id, reason));
    }

    fn teardown(&self) {
        self.unwatch_outside();
        if let Some(p) = self.popover.borrow_mut().take() {
            p.set_child(None::<&gtk::Widget>);
            if p.parent().is_some() {
                p.unparent();
            }
        }
        if let Some(w) = self.window.borrow_mut().take() {
            w.set_child(None::<&gtk::Widget>);
            w.destroy();
        }
    }

    fn set_open(&self, open: bool) {
        if self.open.replace(open) == open {
            return;
        }
        if open {
            self.show();
        } else {
            if let Some(p) = self.popover.borrow().as_ref() {
                p.popdown();
            }
            if let Some(w) = self.window.borrow().as_ref() {
                w.set_visible(false);
            }
        }
    }

    fn show(&self) {
        if let Some(p) = self.popover.borrow().as_ref() {
            // Anchored to the element that contains the overlay.
            if let Some(parent) = self.anchor.parent()
                && p.parent().as_ref() != Some(&parent)
            {
                if p.parent().is_some() {
                    p.unparent();
                }
                p.set_parent(&parent);
            }
            if let Some(parent) = p.parent() {
                place_popover(p, &parent, self.config.get(), self.size.get());
                p.popup();
                self.watch_outside(p, &parent);
            }
        }
        if let Some(w) = self.window.borrow().as_ref() {
            let (cw, ch) = self.size.get();
            if cw > 0 && ch > 0 {
                w.set_default_size(cw, ch);
            }
            w.present();
        }
    }

    /// A popover that leaves the focus to its anchor has no autohide (it
    /// would take the focus and swallow the press): a press anywhere in the
    /// anchor's window closes it and goes on to what it landed on (#182).
    /// Presses in the popover are its own surface's, not the window's.
    fn watch_outside(&self, p: &gtk::Popover, parent: &gtk::Widget) {
        let Some(window) = parent.root().and_downcast::<gtk::Window>() else {
            return;
        };
        if self
            .outside
            .borrow()
            .as_ref()
            .is_some_and(|(w, _)| *w == window)
        {
            return;
        }
        self.unwatch_outside();
        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let p = p.clone();
        click.connect_pressed(move |_, _, _, _| {
            // Not claimed: the press goes on to what it landed on.
            if p.has_css_class(LIGHT_BESIDE_FOCUS) && p.is_visible() {
                // `connect_closed` tells the app, with `Outside`.
                p.popdown();
            }
        });
        window.add_controller(click.clone());
        *self.outside.borrow_mut() = Some((window, click));
    }

    fn unwatch_outside(&self) {
        if let Some((w, c)) = self.outside.borrow_mut().take() {
            w.remove_controller(&c);
        }
    }

    /// Shows a popover the app opened before its anchor had a parent
    /// (inserted open with it, #70): called after each layout.
    fn show_if_pending(&self) {
        let pending = self
            .popover
            .borrow()
            .as_ref()
            .is_some_and(|p| !p.is_visible());
        if self.open.get() && pending {
            self.show();
        }
    }

    fn set_size(&self, w: i32, h: i32) {
        if self.size.replace((w, h)) == (w, h) {
            return;
        }
        self.content.set_size_request(w, h);
        // Its edges line up with the anchor's at its new size.
        if let Some(p) = self.popover.borrow().as_ref()
            && let Some(parent) = p.parent()
        {
            place_popover(p, &parent, self.config.get(), (w, h));
        }
        if let Some(win) = self.window.borrow().as_ref() {
            win.set_default_size(w, h);
        }
    }
}

// ---------------------------------------------------------------------------
// Keys (#17): one controller per window, in the capture phase
// ---------------------------------------------------------------------------

/// Nodes that listen to keys, the keys held down (to flag repeats), and
/// those whose press was reported: hardware keycode, name and target, for
/// their release (#159).
#[derive(Default)]
pub struct KeyState {
    local: RefCell<HashSet<NodeId>>,
    global: RefCell<HashSet<NodeId>>,
    down: RefCell<HashSet<u32>>,
    held: RefCell<Vec<(u32, String, Option<NodeId>)>>,
}

/// A click with its count (GTK's `n_press`, the system's double-click
/// time and distance) and keys (#159).
fn click_event(
    id: NodeId,
    position: Option<prism_core::Point>,
    n_press: i32,
    g: &gtk::GestureClick,
) -> UiEvent {
    let count = n_press.max(1) as u32;
    let modifiers = modifiers(g.current_event_state());
    match position {
        _ if count > 1 || modifiers != prism_core::Modifiers::default() => UiEvent::ClickWith(
            id,
            prism_core::ClickEvent {
                position,
                modifiers,
                count,
            },
        ),
        Some(p) => UiEvent::ClickAt(id, p),
        None => UiEvent::Click(id),
    }
}

/// Releases every key whose press was reported (the window lost the focus:
/// GTK sends no release for them, #159).
fn release_keys(keys: &KeyState) {
    keys.down.borrow_mut().clear();
    let held = std::mem::take(&mut *keys.held.borrow_mut());
    for (_, name, target) in held {
        dispatch(UiEvent::KeyUp(target, prism_core::KeyEvent::new(name)));
    }
}

/// The Prism node a widget belongs to (its `p{id}` class), if any.
fn node_of(w: &gtk::Widget) -> Option<NodeId> {
    w.css_classes()
        .iter()
        .find_map(|c| c.strip_prefix('p').and_then(|n| n.parse::<NodeId>().ok()))
}

/// Reports key presses in `window` (`UiEvent::Key`): to the focused node's
/// nearest ancestor with `on_key_down`, and to global listeners. Typing in
/// a text field only reaches local listeners.
fn attach_keys(window: &gtk::Window, keys: Rc<KeyState>) {
    use gtk::glib::translate::IntoGlib;
    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let up = keys.clone();
    controller.connect_key_released(move |_, key, code, mods| {
        up.down.borrow_mut().remove(&key.into_glib());
        // A release goes where its press went (#159), by hardware key: Shift
        // may have changed the key's name meanwhile.
        let pressed = {
            let mut held = up.held.borrow_mut();
            held.iter()
                .position(|(c, ..)| *c == code)
                .map(|i| held.remove(i))
        };
        if let Some((_, name, target)) = pressed {
            dispatch(UiEvent::KeyUp(
                target,
                prism_core::KeyEvent {
                    key: name,
                    modifiers: modifiers(mods),
                    repeat: false,
                    selection: None,
                },
            ));
        }
    });
    let gone = keys.clone();
    window.connect_is_active_notify(move |w| {
        if !w.is_active() {
            release_keys(&gone);
        }
    });
    controller.connect_key_pressed(move |c, key, code, mods| {
        if keys.local.borrow().is_empty() && keys.global.borrow().is_empty() {
            return glib::Propagation::Proceed;
        }
        let name = key.name().map(|n| n.to_string()).unwrap_or_default();
        let Some(name) = crate::input::key_name(&name, key.to_unicode()) else {
            return glib::Propagation::Proceed;
        };
        let repeat = !keys.down.borrow_mut().insert(key.into_glib());
        let modifiers = modifiers(mods);
        let focus = c
            .widget()
            .and_downcast::<gtk::Window>()
            .and_then(|w| GtkWindowExt::focus(&w));
        let editing = focus
            .as_ref()
            .is_some_and(|f| f.is::<gtk::Text>() || f.is::<gtk::TextView>());
        // In a text field: its selection before the key (#86).
        let selection = focus.as_ref().and_then(|f| {
            if let Some(t) = f.downcast_ref::<gtk::Text>() {
                Some(editable_selection(t))
            } else {
                f.downcast_ref::<gtk::TextView>()
                    .map(|v| buffer_selection(&v.buffer()))
            }
        });
        // The nearest focused node with `on_key_down`.
        let mut target = None;
        let mut next = focus.clone();
        while let Some(w) = next {
            if let Some(id) = node_of(&w)
                && keys.local.borrow().contains(&id)
            {
                target = Some((id, w.clone()));
                break;
            }
            next = w.parent();
        }
        let typing = editing
            && crate::input::types_text(&name, modifiers.control, modifiers.alt, modifiers.meta);
        if target.is_none() && (typing || keys.global.borrow().is_empty()) {
            return glib::Propagation::Proceed;
        }
        let claimed = target
            .as_ref()
            .is_some_and(|(_, w)| Some(w) == focus.as_ref() && crate::input::claims(&name));
        let target = target.map(|(id, _)| id);
        {
            let mut held = keys.held.borrow_mut();
            if !held.iter().any(|(c, ..)| *c == code) {
                held.push((code, name.clone(), target));
            }
        }
        // The app's alone (`consume_keys`, #181): the focused field does not
        // also act on it. Asked from the focused node, wherever its handler is.
        let focused_node = {
            let mut next = focus.clone();
            let mut found = None;
            while let Some(w) = next {
                if let Some(id) = node_of(&w) {
                    found = Some(id);
                    break;
                }
                next = w.parent();
            }
            found
        };
        let mut consumed = false;
        with_state(|s| consumed = s.runtime.consumes_key(focused_node, &name));
        let claimed = claimed || consumed;
        dispatch(UiEvent::Key(
            target,
            prism_core::KeyEvent {
                key: name,
                modifiers,
                repeat,
                selection,
            },
        ));
        if claimed {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(controller);
}

// ---------------------------------------------------------------------------
// Pointer (#17): moves are coalesced to one dispatch per main-loop turn
// ---------------------------------------------------------------------------

thread_local! {
    static COALESCED: RefCell<HashMap<(NodeId, u8), UiEvent>> = RefCell::new(HashMap::new());
}

/// Dispatches `event`, replacing a not yet delivered one of the same kind
/// for the same node (pointer and drag moves).
fn dispatch_latest(id: NodeId, channel: u8, event: UiEvent) {
    if UPDATING.get() {
        return;
    }
    let first = COALESCED.with(|c| c.borrow_mut().insert((id, channel), event).is_none());
    if first {
        glib::idle_add_local_once(move || {
            if let Some(e) = COALESCED.with(|c| c.borrow_mut().remove(&(id, channel))) {
                with_state(|s| {
                    let patches = s.runtime.dispatch(e);
                    commit(s, patches);
                });
            }
        });
    }
}

/// Where a drag's pointer went down, with which button and keys, and
/// whether it moved past the slop.
#[derive(Clone, Copy)]
struct DragStart {
    at: (f64, f64),
    button: prism_core::PointerButton,
    mods: prism_core::Modifiers,
    started: bool,
}

/// GTK's button number as Prism's (touches are the primary button).
fn drag_button(button: u32) -> prism_core::PointerButton {
    use prism_core::PointerButton as B;
    match button {
        0 | gdk::BUTTON_PRIMARY => B::Primary,
        gdk::BUTTON_MIDDLE => B::Middle,
        gdk::BUTTON_SECONDARY => B::Secondary,
        n => B::Other(n.min(255) as u8),
    }
}

/// GDK's modifier state as Prism's.
fn modifiers(mods: gdk::ModifierType) -> prism_core::Modifiers {
    prism_core::Modifiers {
        shift: mods.contains(gdk::ModifierType::SHIFT_MASK),
        control: mods.contains(gdk::ModifierType::CONTROL_MASK),
        alt: mods.contains(gdk::ModifierType::ALT_MASK),
        meta: mods.intersects(gdk::ModifierType::SUPER_MASK | gdk::ModifierType::META_MASK),
    }
}

/// A wheel or touchpad scroll of `dx`, `dy` (#133) as Prism's `WheelEvent`:
/// points, positive `y` toward the end (GTK's sign, after the user's
/// natural scrolling), at the pointer in the element.
fn wheel_event(
    id: NodeId,
    c: &gtk::EventControllerScroll,
    dx: f64,
    dy: f64,
    phase: prism_core::GesturePhase,
) -> UiEvent {
    // A wheel counts notches: 16 points a line, three lines a notch, like
    // browsers. A touchpad counts surface pixels.
    let precise = c.unit() == gdk::ScrollUnit::Surface;
    let k = if precise { 1. } else { 48. };
    UiEvent::Wheel(
        id,
        prism_core::WheelEvent {
            delta: point(dx * k, dy * k),
            position: event_point(c),
            modifiers: modifiers(c.current_event_state()),
            phase,
            precise,
        },
    )
}

/// Where the controller's current event is, in its widget.
fn event_point(c: &impl IsA<gtk::EventController>) -> prism_core::Point {
    let at = (|| {
        let w = c.widget()?;
        let (x, y) = c.current_event()?.position()?;
        let native = w.native()?;
        let (nx, ny) = native.surface_transform();
        let p = native.compute_point(
            &w,
            &gtk::graphene::Point::new((x - nx) as f32, (y - ny) as f32),
        )?;
        Some(point(p.x() as f64, p.y() as f64))
    })();
    at.unwrap_or_default()
}

fn point(x: f64, y: f64) -> prism_core::Point {
    prism_core::Point::new(x as f32, y as f32)
}

// ---------------------------------------------------------------------------
// Main-thread state
// ---------------------------------------------------------------------------

struct State {
    runtime: Runtime,
    renderer: NativeRenderer<Gtk>,
    services: crate::services::LinuxServices,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// Set while Prism changes widgets, so their signals are not reported back.
    static UPDATING: Cell<bool> = const { Cell::new(false) };
    /// What each text field sent and the app has not rendered back yet (#73).
    static TYPED: RefCell<HashMap<NodeId, prism_native::fields::Typed>> = RefCell::new(HashMap::new());
}

// ---------------------------------------------------------------------------
// Text field selections (#86): GTK counts characters, like Prism
// ---------------------------------------------------------------------------

thread_local! {
    /// Fields reporting their selection (`on_select`).
    static SELECT_EVENTS: RefCell<HashSet<NodeId>> = RefCell::new(HashSet::new());
    /// The selection last reported, and the one waiting to be, by field.
    static REPORTED: RefCell<HashMap<NodeId, prism_core::SelectionEvent>> = RefCell::new(HashMap::new());
    static PENDING_SELECTION: RefCell<HashMap<NodeId, prism_core::SelectionEvent>> = RefCell::new(HashMap::new());
    /// Entries the app set a selection on: `cx.focus` keeps it (instead of
    /// selecting everything).
    static APP_SELECTED: RefCell<HashSet<NodeId>> = RefCell::new(HashSet::new());
}

/// The selection of an editable (an entry, its inner `GtkText`), in characters.
pub(crate) fn editable_selection(e: &impl IsA<gtk::Editable>) -> prism_core::SelectionEvent {
    match e.selection_bounds() {
        Some((a, b)) => prism_core::SelectionEvent::new(a.max(0) as usize, b.max(0) as usize),
        None => prism_core::SelectionEvent::caret(e.position().max(0) as usize),
    }
}

/// The selection of a text buffer, in characters.
pub(crate) fn buffer_selection(b: &gtk::TextBuffer) -> prism_core::SelectionEvent {
    match b.selection_bounds() {
        Some((s, e)) => {
            prism_core::SelectionEvent::new(s.offset().max(0) as usize, e.offset().max(0) as usize)
        }
        None => prism_core::SelectionEvent::caret(
            b.iter_at_mark(&b.get_insert()).offset().max(0) as usize
        ),
    }
}

/// Field `id`'s selection may have moved to `sel` (typing, the pointer,
/// keys): reported once per main-loop turn, after the `Input` of the same
/// edit (a lower priority than `dispatch`), only when it moved, and never
/// for Prism's own changes.
pub(crate) fn report_selection(id: NodeId, sel: prism_core::SelectionEvent) {
    if UPDATING.get() || !SELECT_EVENTS.with(|s| s.borrow().contains(&id)) {
        return;
    }
    let first = PENDING_SELECTION.with(|p| p.borrow_mut().insert(id, sel).is_none());
    if !first {
        return;
    }
    glib::idle_add_local_full(glib::Priority::LOW, move || {
        let sel = PENDING_SELECTION.with(|p| p.borrow_mut().remove(&id));
        if let Some(sel) = sel
            && SELECT_EVENTS.with(|s| s.borrow().contains(&id))
            && REPORTED.with(|r| r.borrow_mut().insert(id, sel)) != Some(sel)
        {
            with_state(|s| {
                let patches = s.runtime.dispatch(UiEvent::Select(id, sel.start, sel.end));
                commit(s, patches);
            });
        }
        glib::ControlFlow::Break
    });
}

fn forget_selection(id: NodeId) {
    APP_SELECTED.with(|s| s.borrow_mut().remove(&id));
    SELECT_EVENTS.with(|s| s.borrow_mut().remove(&id));
    REPORTED.with(|r| r.borrow_mut().remove(&id));
    PENDING_SELECTION.with(|p| p.borrow_mut().remove(&id));
}

/// The user typed `text` in field `id`: remembered until the app renders it
/// back (events run on a later turn, the user may type further meanwhile),
/// then dispatched.
pub(crate) fn dispatch_typed(id: NodeId, text: String) {
    if UPDATING.get() {
        return;
    }
    TYPED.with(|t| t.borrow_mut().entry(id).or_default().sent(&text));
    dispatch(UiEvent::Input(id, text));
}

/// Whether the app's `text` for field `id`, showing `current`, is written
/// into it: not an echo of the user's typing, and different (#73).
fn field_takes(id: NodeId, text: &str, current: &str) -> bool {
    TYPED.with(|t| {
        t.borrow_mut()
            .entry(id)
            .or_default()
            .should_apply(text, current)
    })
}

fn with_state(f: impl FnOnce(&mut State)) {
    STATE.with(|cell| {
        if let Ok(mut guard) = cell.try_borrow_mut()
            && let Some(state) = guard.as_mut()
        {
            f(state)
        }
    });
}

/// A UI event from GTK, handled on the next turn (GTK may be mid-update).
pub(crate) fn dispatch(event: UiEvent) {
    if UPDATING.get() {
        return;
    }
    glib::idle_add_local_once(move || {
        with_state(|s| {
            let patches = s.runtime.dispatch(event);
            commit(s, patches);
        })
    });
}

fn commit(s: &mut State, patches: Vec<prism_core::Patch>) {
    UPDATING.set(true);
    if !patches.is_empty() {
        s.renderer.apply(&patches);
    }
    s.renderer.widgets.flush_css();
    relayout(s);
    UPDATING.set(false);
    for request in s.runtime.take_requests() {
        if let prism_core::PlatformRequest::LoadFont(face) = request {
            load_font(s, face);
            continue;
        }
        // `cx.set_title` (#154): the window's title, else the app's name.
        // Unsaved changes (#158): closing the window asks first.
        if let prism_core::PlatformRequest::SetUnsavedChanges(on) = &request {
            UNSAVED.set(*on);
        }
        if let prism_core::PlatformRequest::SetTitle(title) = &request {
            let w = &s.renderer.widgets;
            w.window
                .set_title(Some(title.as_deref().unwrap_or(&w.app_title)));
        }
        prism_core::PlatformServices::handle(&mut s.services, request);
    }
    if let Some(at) = s.runtime.next_timer() {
        let delay = at.saturating_duration_since(std::time::Instant::now());
        glib::timeout_add_local_once(delay, poll);
    }
}

/// A video player (#207): reports the end of the playback, and lays out
/// again once the stream knows its size.
fn video_widget(id: NodeId) -> gtk::Video {
    let v = gtk::Video::new();
    v.set_autoplay(false);
    v.connect_media_stream_notify(move |v| {
        let Some(stream) = v.media_stream() else {
            return;
        };
        // SAFETY: set by `set_config` as a bool.
        let muted = unsafe { v.data::<bool>("prism-muted").map(|m| *m.as_ref()) };
        stream.set_muted(muted.unwrap_or(false));
        let looping = v.is_loop();
        stream.connect_ended_notify(move |s| {
            if s.is_ended() && !looping {
                dispatch(UiEvent::Ended(id));
            }
        });
        stream.connect_invalidate_size(|_| {
            glib::idle_add_local_once(|| {
                with_state(|s| {
                    s.renderer.mark_all_dirty();
                    relayout(s);
                })
            });
        });
    });
    v
}

/// Registers a run-time font (`cx.load_font`) with fontconfig (from a file in
/// the user's cache) and draws the texts again. A URL comes back here
/// with its bytes once loaded.
fn load_font(s: &mut State, face: prism_core::FontFace) {
    let w = &s.renderer.widgets;
    let sources = prism_native::images::ImageSources {
        embedded: w.embedded.clone(),
        asset_dirs: w.assets.iter().cloned().collect(),
        ..Default::default()
    };
    let Some(font) = prism_native::fonts::prepare(&mut s.runtime.app, face, &sources) else {
        return;
    };
    // Next to the embedded fonts (fontconfig only reads files).
    let dir = glib::user_cache_dir().join("prism-fonts").join("runtime");
    if let Some(path) = prism_native::fonts::save(&font, &dir)
        && text::add_runtime_font(&path)
    {
        s.renderer.restyle_all();
        s.renderer.widgets.flush_css();
        relayout(s);
    }
}

fn relayout(s: &mut State) {
    let (w, h) = s.renderer.widgets.size;
    if w <= 0. {
        return;
    }
    if (w, h) != s.runtime.app.viewport() {
        let patches = s.runtime.set_viewport(w, h);
        if !patches.is_empty() {
            s.renderer.apply(&patches);
            s.renderer.widgets.flush_css();
        }
    }
    let insets = s.renderer.widgets.chrome_insets();
    s.renderer.layout(w, h, insets);
    // Elements that report their frame and moved (#192).
    let more = prism_native::report_layout(&mut s.runtime, &mut s.renderer);
    if !more.is_empty() {
        commit(s, more);
    }
}

pub(crate) fn device_reply(
    id: u64,
    result: Result<prism_core::DeviceValue, prism_core::DeviceError>,
) {
    with_state(|s| {
        let patches = s.runtime.device_reply(id, result);
        commit(s, patches);
    });
}

pub(crate) fn push_registered(
    result: Result<prism_core::PushSubscription, prism_core::DeviceError>,
) {
    with_state(|s| {
        let patches = s.runtime.push_registered(result);
        commit(s, patches);
    });
}

/// A UnifiedPush message: shown as a notification unless the window is
/// active; silent ones run `on_background_push`.
pub(crate) fn push_received(message: prism_core::PushMessage) {
    with_state(|s| {
        let patches = if message.event == prism_core::PushEvent::Background {
            s.runtime.run_background_push(message);
            s.runtime.poll()
        } else {
            s.runtime.push_received(message, true)
        };
        commit(s, patches);
    });
}

pub(crate) fn device_event(id: u64, event: prism_core::DeviceEvent) {
    with_state(|s| {
        let patches = s.runtime.device_event(id, event);
        commit(s, patches);
    });
}

/// `PRISM_BACKGROUND_INTERVAL=<seconds>` shortens background intervals (development).
pub(crate) fn background_interval(every: std::time::Duration) -> std::time::Duration {
    std::env::var("PRISM_BACKGROUND_INTERVAL")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .map_or(every, std::time::Duration::from_secs_f64)
}

fn poll() {
    with_state(|s| {
        let patches = s.runtime.poll();
        commit(s, patches);
    });
}

// ---------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------

pub struct Gtk {
    root: gtk::Box,
    window: gtk::ApplicationWindow,
    /// The window's own title (the app's name), for `cx.clear_title`.
    app_title: String,
    app: gtk::Application,
    /// Context menus by node (right-click / press and hold).
    menus: Rc<RefCell<HashMap<NodeId, gtk::PopoverMenu>>>,
    swipes: Rc<RefCell<HashMap<NodeId, (bool, bool)>>>,
    scroll_events: Rc<RefCell<HashSet<NodeId>>>,
    /// Refreshable scroll views (F5 / Ctrl+R).
    refreshable: Rc<RefCell<HashSet<NodeId>>>,
    /// Each node's size from the last layout (GTK's own lags behind).
    sizes: HashMap<NodeId, (f32, f32)>,
    css: gtk::CssProvider,
    /// Per-node CSS rules, rebuilt into one stylesheet after each change.
    rules: HashMap<NodeId, String>,
    css_dirty: bool,
    clickable: Rc<RefCell<HashSet<NodeId>>>,
    timers: HashMap<NodeId, (gtk::Label, f64)>,
    assets: Option<std::path::PathBuf>,
    /// Assets compiled into the binary, found before the directory's.
    embedded: prism_core::EmbeddedAssets,
    dark: bool,
    /// Titles of screens and tabs, icons of tabs.
    titles: HashMap<NodeId, String>,
    icons: HashMap<NodeId, String>,
    stacks: Vec<Rc<NavStack>>,
    tab_bars: Vec<Rc<TabBar>>,
    /// Shapes, redrawn when the appearance changes.
    shapes: HashMap<NodeId, gtk::DrawingArea>,
    /// 3D scenes, redrawn when the appearance changes.
    scenes: HashMap<NodeId, gtk::GLArea>,
    /// Window content size.
    size: (f32, f32),
    /// Right-to-left layout and the app's locale (form controls follow them).
    rtl: bool,
    locale: prism_core::Locale,
    /// Form controls, mirrored in right-to-left.
    controls: HashMap<NodeId, gtk::Widget>,
    date_pickers: HashMap<NodeId, Rc<controls::DatePicker>>,
    /// "Clear" in the app's language (optional date pickers, #89).
    clear_label: String,
    /// Status regions and what they last announced.
    statuses: HashMap<NodeId, String>,
    /// Text labels: their text, runs and style (rebuilt when one changes).
    labels: HashMap<NodeId, text::LabelState>,
    // --- Hidden header (#8) ---
    /// Screens that hide their stack's header while on top (`.bare()`).
    bare: HashSet<NodeId>,
    /// Sticky views of each scroll view, moved as it scrolls.
    stickies: Stickies,
    /// Interaction states each widget reports to prism-native (refinements
    /// CSS cannot draw, groups, the focus of text fields), and the widgets
    /// that have the controllers reporting them.
    reports: Rc<RefCell<HashMap<NodeId, prism_core::Interactions>>>,
    watched: HashSet<NodeId>,
    /// Cursors set by `Style::cursor` (they win over the clickable default).
    cursors: HashMap<NodeId, prism_core::Cursor>,
    // Pointer, keys, layers, overlays (#17, #19, #25).
    /// Pointer events each node reports (`pointer_mask` bits).
    pointer: Rc<RefCell<HashMap<NodeId, u8>>>,
    /// Nodes whose motion / drag controllers are attached.
    pointer_attached: HashSet<(NodeId, u8)>,
    /// Drop targets' controllers (#111).
    drop_targets: HashMap<NodeId, gtk::DropTarget>,
    keys: Rc<KeyState>,
    /// Runtime layers, above the root in the window.
    layers: Vec<gtk::Widget>,
    /// Nodes made inert (hidden from assistive technologies).
    inert: HashSet<NodeId>,
    // --- Adaptive colors (#36) ---
    /// Styles with an adaptive color: their CSS rule picks the light or the
    /// dark variant, so it is rebuilt when the appearance changes.
    adaptive: HashMap<NodeId, (View, Style)>,
}

/// Scroll view → its sticky views (node, widget, range).
type Stickies = Rc<RefCell<HashMap<NodeId, HashMap<NodeId, (gtk::Widget, prism_native::Sticky)>>>>;

/// Pushes the sticky views of a scroll view scrolled to `value`.
fn place_sticky(stickies: &Stickies, scroll: NodeId, value: f64) {
    if let Some(views) = stickies.borrow().get(&scroll) {
        for (w, st) in views.values() {
            layout::set_sticky_offset(w, Some(st.offset(value as f32)));
        }
    }
}

fn css_color(c: Color, dark: bool) -> String {
    crate::decor::color(c, dark)
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> (i32, i32, i32, i32) {
    (
        x.round() as i32,
        y.round() as i32,
        w.round().max(0.) as i32,
        h.round().max(0.) as i32,
    )
}

impl Gtk {
    /// What the window's own chrome covers: a root tab bar at the bottom.
    /// Roots that are stacks or tab bars lay their bars out themselves, so
    /// only the layers (toasts) keep clear of it.
    fn chrome_insets(&self) -> Insets {
        let root = self.root.first_child();
        let tabs = self
            .tab_bars
            .iter()
            .any(|t| root.as_ref() == Some(t.root.upcast_ref::<gtk::Widget>()));
        Insets {
            bottom: if tabs { TAB_BAR } else { 0. },
            ..Insets::default()
        }
    }

    /// Puts the layers back on top of the root's children.
    fn raise_layers(&self) {
        for l in &self.layers {
            if l.parent().is_some() {
                l.unparent();
            }
            l.set_parent(&self.root);
        }
    }

    fn flush_css(&mut self) {
        if !self.css_dirty {
            return;
        }
        self.css_dirty = false;
        let sheet: String = self.rules.values().map(String::as_str).collect();
        #[allow(deprecated)]
        self.css.load_from_data(&sheet);
    }

    /// Whether `stack` shows no header bar: none at all, or a bare screen on top.
    fn header_hidden(&self, stack: &NavStack) -> bool {
        stack.no_header.get()
            || stack
                .screens
                .borrow()
                .last()
                .is_some_and(|top| self.bare.contains(&top.id()))
    }

    fn header_back(&self, stack: &Rc<NavStack>) {
        stack.header.set_visible(!self.header_hidden(stack));
        let depth = stack.screens.borrow().len();
        let closable = stack.in_sheet.get() && depth <= 1;
        stack.back.set_visible(depth > 1 || closable);
        stack
            .back
            .set_label(if closable { "Close" } else { "‹ Back" });
        // Only the top screen shows; its title is the header's.
        for (i, s) in stack.screens.borrow().iter().enumerate() {
            s.widget().set_visible(i + 1 == depth);
        }
        let title = stack
            .screens
            .borrow()
            .last()
            .and_then(|s| self.titles.get(&s.id()).cloned())
            .unwrap_or_default();
        stack.title.set_text(&title);
        // The top screen's bar buttons (#30).
        let top = stack.screens.borrow().last().map(View::id);
        crate::navbar::refresh(&stack.header, &stack.back, &stack.title, top);
    }

    fn refresh_tabs(&self, tabs: &Rc<TabBar>) {
        while let Some(c) = tabs.bar.first_child() {
            tabs.bar.remove(&c);
        }
        let selected = tabs.selected.get();
        for (i, tab) in tabs.tabs.borrow().iter().enumerate() {
            tab.widget().set_visible(i == selected);
            let title = self.titles.get(&tab.id()).cloned().unwrap_or_default();
            let label = match self.icons.get(&tab.id()) {
                Some(icon) => format!("{}  {title}", prism_core::glyphs::glyph(icon)),
                None => title,
            };
            let text = gtk::Label::new(Some(&label));
            text.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let button = gtk::ToggleButton::new();
            button.set_child(Some(&text));
            button.set_tooltip_text(Some(&label));
            button.set_active(i == selected);
            button.set_hexpand(true);
            let id = tabs_id(tabs);
            button.connect_clicked(move |_| dispatch(UiEvent::Change(id, i as f64)));
            tabs.bar.append(&button);
        }
    }
}

impl Gtk {
    /// Right-click and press-and-hold open the node's context menu; a
    /// horizontal swipe (touchpad, touch screen) reports `Swipe`.
    fn attach_gestures(&self, id: NodeId, w: &gtk::Widget) {
        let menus = self.menus.clone();
        let open = move |x: f64, y: f64| {
            if let Some(p) = menus.borrow().get(&id) {
                p.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                p.popup();
                return true;
            }
            false
        };
        let right = gtk::GestureClick::new();
        right.set_button(gdk::BUTTON_SECONDARY);
        // An element dragged with the secondary button (#136) opens its menu
        // on a release that did not move (a drag claims the press first).
        let masks = self.pointer.clone();
        let drags_secondary = move || {
            masks
                .borrow()
                .get(&id)
                .is_some_and(|m| m & prism_core::events::pointer_mask::DRAG_SECONDARY != 0)
        };
        let (open_right, drags) = (open.clone(), drags_secondary.clone());
        right.connect_pressed(move |g, _, x, y| {
            if !drags() && open_right(x, y) {
                g.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        let open_right = open.clone();
        right.connect_released(move |g, _, x, y| {
            if drags_secondary() && open_right(x, y) {
                g.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        w.add_controller(right);
        let hold = gtk::GestureLongPress::new();
        hold.connect_pressed(move |g, x, y| {
            if open(x, y) {
                g.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        w.add_controller(hold);
        let swipes = self.swipes.clone();
        let swipe = gtk::GestureSwipe::new();
        swipe.connect_swipe(move |_, vx, vy| {
            let (left, right) = swipes.borrow().get(&id).copied().unwrap_or_default();
            if vx.abs() < 300. || vx.abs() < vy.abs() * 2. {
                return;
            }
            if vx < 0. && left {
                dispatch(UiEvent::Swipe(id, prism_core::SwipeDirection::Left));
            } else if vx > 0. && right {
                dispatch(UiEvent::Swipe(id, prism_core::SwipeDirection::Right));
            }
        });
        w.add_controller(swipe);
    }
}

// Interaction states prism-native resolves (refinements CSS cannot draw:
// sizes, visibility, opacity, transforms; group states; focus-within).
impl Gtk {
    /// Decides which states `id` reports to prism-native and adds the event
    /// controllers reporting them (once): pointer motion (hover), raw
    /// presses (also setting GTK's `:active`, which plain boxes don't get),
    /// and keyboard focus.
    fn watch_states(&mut self, id: NodeId, w: &gtk::Widget, s: &Style, text_field: bool) {
        use prism_core::Interaction as I;
        let used = s.states.used();
        let mut report = prism_core::Interactions::NONE;
        if crate::decor::needs_engine(s) {
            for i in [I::Hover, I::Active, I::Focus] {
                report = report.with(i, used.contains(i));
            }
        }
        if s.group {
            report = report.with(I::Hover, true).with(I::Active, true);
        }
        // Text fields always report their focus: ancestors' focus-within
        // and group refinements follow it.
        if text_field {
            report = report.with(I::Focus, true);
        }
        // GTK only sets `:active` on widgets that handle presses themselves.
        let pressable = used.contains(I::Active) || s.group;
        if report.is_empty() && !pressable {
            self.reports.borrow_mut().remove(&id);
            return;
        }
        self.reports.borrow_mut().insert(id, report);
        if !self.watched.insert(id) {
            return;
        }
        let reports = self.reports.clone();
        let reports_on = move |i: I| reports.borrow().get(&id).is_some_and(|r| r.contains(i));
        let motion = gtk::EventControllerMotion::new();
        let on = reports_on.clone();
        motion.connect_enter(move |_, _, _| {
            if on(I::Hover) {
                report_state(id, I::Hover, true)
            }
        });
        let on = reports_on.clone();
        motion.connect_leave(move |_| {
            if on(I::Hover) {
                report_state(id, I::Hover, false)
            }
        });
        w.add_controller(motion);
        // Raw presses and releases, seen before children (buttons still get
        // them): a click gesture would stop tracking at the double-click
        // timeout while the button is still down.
        let press = gtk::EventControllerLegacy::new();
        press.set_propagation_phase(gtk::PropagationPhase::Capture);
        let on = reports_on.clone();
        press.connect_event(move |c, event| {
            let down = match event.event_type() {
                gdk::EventType::ButtonPress | gdk::EventType::TouchBegin => true,
                gdk::EventType::ButtonRelease
                | gdk::EventType::TouchEnd
                | gdk::EventType::TouchCancel => false,
                _ => return glib::Propagation::Proceed,
            };
            if let Some(w) = c.widget() {
                if down {
                    w.set_state_flags(gtk::StateFlags::ACTIVE, false);
                } else {
                    w.unset_state_flags(gtk::StateFlags::ACTIVE);
                }
            }
            if on(I::Active) {
                report_state(id, I::Active, down)
            }
            glib::Propagation::Proceed
        });
        w.add_controller(press);
        let focus = gtk::EventControllerFocus::new();
        let on = reports_on.clone();
        focus.connect_enter(move |_| {
            if on(I::Focus) {
                report_state(id, I::Focus, true)
            }
        });
        focus.connect_leave(move |_| {
            if reports_on(I::Focus) {
                report_state(id, I::Focus, false)
            }
        });
        w.add_controller(focus);
    }
}

thread_local! {
    /// The app has unsaved changes (`cx.set_unsaved_changes`, #158).
    static UNSAVED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// « Leave without saving? » over `window` (#158): discarding closes it.
fn confirm_leave(window: &gtk::ApplicationWindow) {
    let mut texts = None;
    with_state(|s| {
        texts = Some(
            ["unsaved_title", "unsaved_message", "discard", "cancel"]
                .map(|k| s.runtime.app.chrome_text(k)),
        )
    });
    let [title, message, discard, cancel] = texts.unwrap_or_else(|| {
        [
            "Leave without saving?",
            "Your changes will be lost.",
            "Discard Changes",
            "Cancel",
        ]
        .map(String::from)
    });
    let dialog = gtk::Window::builder()
        .transient_for(window)
        .modal(true)
        .resizable(false)
        .default_width(360)
        .accessible_role(gtk::AccessibleRole::AlertDialog)
        .build();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_top(20);
    body.set_margin_bottom(20);
    body.set_margin_start(20);
    body.set_margin_end(20);
    let heading = gtk::Label::new(Some(&title));
    heading.add_css_class("title-3");
    heading.set_wrap(true);
    body.append(&heading);
    let text = gtk::Label::new(Some(&message));
    text.set_wrap(true);
    body.append(&text);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let keep = gtk::Button::with_label(&cancel);
    let leave = gtk::Button::with_label(&discard);
    leave.add_css_class("destructive-action");
    let d = dialog.clone();
    keep.connect_clicked(move |_| d.close());
    let (d, w) = (dialog.clone(), window.clone());
    leave.connect_clicked(move |_| {
        UNSAVED.set(false);
        d.close();
        w.close();
    });
    buttons.append(&keep);
    buttons.append(&leave);
    body.append(&buttons);
    dialog.set_child(Some(&body));
    dialog.set_default_widget(Some(&keep));
    dialog.present();
}

thread_local! {
    /// When Escape was last pressed in a popover (#156).
    static ESCAPE_AT: std::cell::Cell<Option<std::time::Instant>> =
        const { std::cell::Cell::new(None) };
}

/// A widget entered or left an interaction state: prism-native restyles it
/// (and its group's descendants, its ancestors for focus-within), then the
/// window is laid out again. On the next turn: GTK may be mid-update.
fn report_state(id: NodeId, state: prism_core::Interaction, on: bool) {
    glib::idle_add_local_once(move || {
        with_state(|s| {
            if s.renderer.set_interaction(id, state, on) {
                UPDATING.set(true);
                s.renderer.widgets.flush_css();
                relayout(s);
                UPDATING.set(false);
            }
        })
    });
}

fn tabs_id(tabs: &Rc<TabBar>) -> NodeId {
    tabs.root
        .widget_name()
        .trim_start_matches('n')
        .parse()
        .unwrap_or(0)
}

impl Widgets for Gtk {
    type View = View;

    fn create(&mut self, id: NodeId, kind: NodeKind) -> View {
        let view = match kind {
            NodeKind::View | NodeKind::Screen | NodeKind::Tab => {
                let c = layout::container();
                let gesture = gtk::GestureClick::new();
                let clickable = self.clickable.clone();
                gesture.connect_released(move |g, n, x, y| {
                    if clickable.borrow().contains(&id) {
                        g.set_state(gtk::EventSequenceState::Claimed);
                        dispatch(click_event(id, Some(point(x, y)), n, g));
                    }
                });
                c.add_controller(gesture);
                View::Container(id, kind, c)
            }
            NodeKind::Text => {
                let l = gtk::Label::new(None);
                l.set_wrap(true);
                l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                l.set_xalign(0.);
                l.set_yalign(0.);
                // Inline links (rich text): reported like menu items.
                l.connect_activate_link(|_, uri| match text::link_id(uri) {
                    Some(item) => {
                        dispatch(UiEvent::MenuSelect(item));
                        glib::Propagation::Stop
                    }
                    None => glib::Propagation::Proceed,
                });
                // A clickable text: click, or Enter / Space when focused
                // (not on one of its links, which handle themselves).
                let gesture = gtk::GestureClick::new();
                let clickable = self.clickable.clone();
                gesture.connect_released(move |g, n, _, _| {
                    let on_link = g
                        .widget()
                        .and_then(|w| w.downcast::<gtk::Label>().ok())
                        .is_some_and(|l| l.current_uri().is_some());
                    if clickable.borrow().contains(&id) && !on_link {
                        dispatch(click_event(id, None, n, g));
                    }
                });
                l.add_controller(gesture);
                let keys = gtk::EventControllerKey::new();
                let clickable = self.clickable.clone();
                keys.connect_key_pressed(move |k, key, _, _| {
                    let on_link = k
                        .widget()
                        .and_then(|w| w.downcast::<gtk::Label>().ok())
                        .is_some_and(|l| l.current_uri().is_some());
                    let activate =
                        matches!(key, gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space);
                    if activate && !on_link && clickable.borrow().contains(&id) {
                        dispatch(UiEvent::Click(id));
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                });
                l.add_controller(keys);
                View::Text(id, l)
            }
            NodeKind::Button => {
                let b = gtk::Button::new();
                b.set_cursor_from_name(Some("pointer"));
                b.connect_clicked(move |_| dispatch(UiEvent::Click(id)));
                View::Button(id, b)
            }
            NodeKind::TextInput => {
                let e = gtk::Entry::new();
                e.connect_changed(move |e| dispatch_typed(id, e.text().to_string()));
                // The caret and the selection's other end (#86).
                for prop in ["cursor-position", "selection-bound"] {
                    e.connect_notify_local(Some(prop), move |e, _| {
                        report_selection(id, editable_selection(e))
                    });
                }
                // Enter.
                e.connect_activate(move |_| dispatch(UiEvent::Submit(id)));
                View::Input(id, e)
            }
            NodeKind::Slider => {
                let (scale, config) = controls::slider(id);
                View::Slider(id, scale, config)
            }
            NodeKind::Picker => View::Picker(id, controls::picker(id)),
            NodeKind::DatePicker => {
                let p = controls::date_picker(id, self.locale.clone());
                if !self.clear_label.is_empty() {
                    p.set_clear_label(&self.clear_label);
                }
                self.date_pickers.insert(id, p.clone());
                View::DatePicker(id, p)
            }
            NodeKind::TextArea => View::TextArea(id, controls::text_area(id)),
            NodeKind::Spinner => {
                let s = gtk::Spinner::new();
                s.set_spinning(true);
                controls::spinner_size(&s, false);
                View::Spinner(id, s)
            }
            NodeKind::Toggle => {
                let t = gtk::Switch::new();
                t.set_valign(gtk::Align::Center);
                t.connect_active_notify(move |t| {
                    dispatch(UiEvent::Change(id, if t.is_active() { 1. } else { 0. }))
                });
                View::Toggle(id, t)
            }
            NodeKind::Progress => View::Progress(id, gtk::ProgressBar::new()),
            NodeKind::Image => View::Image(id, gtk::Label::new(None)),
            NodeKind::Picture => {
                let p = gtk::Picture::new();
                p.set_content_fit(gtk::ContentFit::Cover);
                View::Picture(id, p)
            }
            NodeKind::Timer => View::Timer(id, gtk::Label::new(None)),
            NodeKind::Shape => {
                let area = gtk::DrawingArea::new();
                let paths: Rc<RefCell<Vec<prism_core::Path>>> = Rc::default();
                let drawn = paths.clone();
                area.set_draw_func(move |_, cr, _, _| draw_paths(cr, &drawn.borrow()));
                self.shapes.insert(id, area.clone());
                View::Shape(id, area, paths)
            }
            NodeKind::Scene3d => {
                let scene =
                    crate::scene3d::SceneView::new(id, self.assets.clone(), self.embedded.clone());
                self.scenes.insert(id, scene.area.clone());
                View::Scene(id, scene)
            }
            NodeKind::Scroll => {
                let content = layout::container();
                let s = gtk::ScrolledWindow::new();
                s.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
                s.set_child(Some(&content));
                let events = self.scroll_events.clone();
                let adj = s.vadjustment();
                let stickies = self.stickies.clone();
                adj.connect_value_changed(move |a| {
                    place_sticky(&stickies, id, a.value());
                    if events.borrow().contains(&id) {
                        // Also when the app moved it (`scroll_by`, #204)
                        // while Prism applied patches: reported right after.
                        let event = UiEvent::Scroll(id, a.value() as f32, a.page_size() as f32);
                        glib::idle_add_local_once(move || {
                            with_state(|s| {
                                let patches = s.runtime.dispatch(event);
                                commit(s, patches);
                            })
                        });
                    }
                });
                View::Scroll(id, s, content)
            }
            NodeKind::Stack => {
                let root = layout::container();
                let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                header.add_css_class("prism-header");
                let back = gtk::Button::with_label("‹ Back");
                back.set_has_frame(false);
                let title = gtk::Label::new(None);
                title.set_hexpand(true);
                title.add_css_class("heading");
                header.append(&back);
                header.append(&title);
                header.set_parent(&root);
                let stack = Rc::new(NavStack {
                    root,
                    header,
                    back: back.clone(),
                    title,
                    screens: RefCell::new(Vec::new()),
                    in_sheet: Cell::new(false),
                    no_header: Cell::new(false),
                });
                self.stacks.push(stack.clone());
                let weak = Rc::downgrade(&stack);
                back.connect_clicked(move |_| {
                    if let Some(st) = weak.upgrade() {
                        let depth = st.screens.borrow().len();
                        dispatch(UiEvent::Back(id, depth.saturating_sub(1)));
                    }
                });
                View::Stack(id, stack)
            }
            NodeKind::Tabs => {
                let root = layout::container();
                root.set_widget_name(&format!("n{id}"));
                let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                bar.set_homogeneous(true);
                bar.add_css_class("prism-tabbar");
                bar.set_parent(&root);
                let tabs = Rc::new(TabBar {
                    root,
                    bar,
                    tabs: RefCell::new(Vec::new()),
                    selected: Cell::new(0),
                });
                self.tab_bars.push(tabs.clone());
                View::Tabs(id, tabs)
            }
            NodeKind::Modal => {
                let content = layout::container();
                let window = gtk::Window::builder()
                    .transient_for(&self.window)
                    .modal(true)
                    .default_width(520)
                    .default_height(620)
                    .child(&content)
                    .build();
                let sheet = Rc::new(Sheet {
                    window: window.clone(),
                    content,
                    children: RefCell::new(Vec::new()),
                });
                let weak = Rc::downgrade(&sheet);
                // Closing the window dismisses the modal's stack.
                window.connect_close_request(move |_| {
                    if let Some(s) = weak.upgrade()
                        && let Some(stack) = s.children.borrow().first()
                    {
                        dispatch(UiEvent::Dismiss(
                            stack.id(),
                            prism_core::DismissReason::Back,
                        ));
                    }
                    glib::Propagation::Stop
                });
                View::Sheet(id, sheet)
            }
            // No web view on Linux yet (#62): an empty box sized by its
            // style (`capabilities().web_view` is false).
            NodeKind::WebView => View::Container(id, kind, layout::container()),
            NodeKind::Video => View::Video(id, video_widget(id)),
            NodeKind::Checkbox => View::Check(id, controls::checkbox(id)),
            NodeKind::RadioGroup => View::Radio(id, controls::radio_group(id)),
            NodeKind::Overlay => {
                let anchor = gtk::Box::new(gtk::Orientation::Vertical, 0);
                anchor.set_can_target(false);
                View::Overlay(
                    id,
                    Rc::new(OverlayView {
                        id,
                        anchor,
                        content: layout::container(),
                        popover: RefCell::new(None),
                        window: RefCell::new(None),
                        config: Cell::new(Default::default()),
                        open: Cell::new(false),
                        size: Cell::new((0, 0)),
                        outside: RefCell::new(None),
                    }),
                )
            }
        };
        let w = view.widget();
        w.add_css_class(&format!("p{id}"));
        self.attach_gestures(id, &w);
        if matches!(
            view,
            View::Input(..)
                | View::Slider(..)
                | View::Picker(..)
                | View::DatePicker(..)
                | View::TextArea(..)
                | View::Check(..)
                | View::Radio(..)
        ) {
            controls::set_direction_deep(&w, self.rtl);
            self.controls.insert(id, w);
        }
        view
    }

    fn set_style(&mut self, view: &View, s: &Style) {
        let id = view.id();
        if let View::Scroll(_, sw, _) = view {
            // `scroll_x()`: sideways only.
            // Hidden bars (#132): `External` still scrolls, without a bar.
            let on = if s.scrollbar == prism_core::Scrollbar::Hidden {
                gtk::PolicyType::External
            } else {
                gtk::PolicyType::Automatic
            };
            let (h, v) = if s.scroll_axis == prism_core::ScrollAxis::Horizontal {
                (on, gtk::PolicyType::Never)
            } else {
                (gtk::PolicyType::Never, on)
            };
            sw.set_policy(h, v);
            // Overlay bars, whatever the desktop's setting for others.
            sw.set_overlay_scrolling(true);
        }
        if crate::decor::adapts(s) {
            self.adaptive.insert(id, (view.clone(), s.clone()));
        } else {
            self.adaptive.remove(&id);
        }
        // Background, gradient, corners, borders (per side, dashed, dotted)
        // and shadows.
        let mut css = crate::decor::declarations(&crate::decor::paint(s, self.dark, false));
        // The theme paints buttons with an image over their color.
        if s.background.is_some() && s.gradient.is_none() && matches!(view, View::Button(..)) {
            css += "background-image:none;";
        }
        if let Some(t) = s.transition {
            // Colors animate in GTK's CSS; opacity and transforms in `layout`.
            let (x1, y1, x2, y2) = t.bezier();
            css += &format!(
                "transition:all {}s cubic-bezier({x1},{y1},{x2},{y2}) {}s;",
                t.duration, t.delay
            );
        }
        // Tint of form controls: the slider's fill, the picker's and the
        // date picker's text (not their popovers').
        let mut extra = String::new();
        if let Some(c) = s.text_color {
            let c = css_color(c, self.dark);
            match view {
                View::Slider(..) => {
                    extra += &format!(
                        ".p{id} trough highlight{{background-color:{c};border-color:{c};}}\n"
                    )
                }
                View::Picker(..) | View::DatePicker(..) => {
                    extra += &format!(".p{id} > button{{color:{c};}}\n")
                }
                _ => css += &format!("color:{c};"),
            }
        }
        if let View::TextArea(_, t) = view {
            // Prism draws the field: the text view is transparent inside.
            let drawn =
                s.background.is_some() || s.borders().iter().any(|b| b.width > 0.) || s.border_none;
            t.scroll.set_has_frame(!drawn);
            if drawn {
                extra += &format!(
                    ".p{id} textview, .p{id} textview text{{background-color:transparent;}}\n"
                );
            }
        }
        css += &format!(
            "font-size:{}px;font-weight:{};",
            s.font_size,
            s.font_weight.css()
        );
        if let View::Text(..) = view {
            extra += &text::link_css(id);
        }
        // Padding (#14): drawn by GTK in place of the theme's, or the text
        // view's margins; `.border(0.)`: not even the theme's frame.
        crate::padding::remember(id, s);
        match view {
            View::Text(..) | View::Timer(..) | View::Button(..) => css += &crate::padding::css(s),
            View::Input(..) => {
                css += &crate::padding::css(s);
                css += crate::padding::entry_css(s);
            }
            View::TextArea(_, t) => t.set_padding(s.has_padding().then_some(s.padding)),
            _ => {}
        }
        if s.border_none && !s.borders().iter().any(|b| b.width > 0.) {
            match view {
                View::Input(..) | View::Button(..) => css += crate::padding::NO_BORDER,
                View::Picker(..) | View::DatePicker(..) => {
                    extra += &format!(".p{id} > button{{{}}}\n", crate::padding::NO_BORDER)
                }
                _ => {}
            }
        }
        // Hover, press and focus refinements: GTK's own pseudo-classes.
        let text_on_self = !matches!(
            view,
            View::Slider(..) | View::Picker(..) | View::DatePicker(..)
        );
        let inner_focus = matches!(view, View::Input(..) | View::TextArea(..));
        extra += &crate::decor::state_rules(id, s, self.dark, text_on_self, inner_focus);
        // Bar tint (#30).
        if matches!(view, View::Tabs(..) | View::Stack(..)) {
            let tabs = matches!(view, View::Tabs(..));
            extra += &crate::navbar::tint_rules(id, tabs, s.tint, self.dark);
        }
        self.rules.insert(id, format!(".p{id}{{{css}}}\n{extra}"));
        self.css_dirty = true;
        let w = view.widget();
        if s.group {
            w.add_css_class("prism-group");
        } else {
            w.remove_css_class("prism-group");
        }
        self.watch_states(id, &w, s, inner_focus);
        match s.cursor {
            Some(c) => {
                self.cursors.insert(id, c);
                w.set_cursor_from_name(Some(c.css()));
            }
            None => {
                if self.cursors.remove(&id).is_some() {
                    let pointer =
                        self.clickable.borrow().contains(&id) || matches!(view, View::Button(..));
                    w.set_cursor_from_name(pointer.then_some("pointer"));
                }
            }
        }
        // `hit_test(false)`: GTK skips the widget and its children when
        // picking; `inert`: no pointer, no focus, hidden from screen readers.
        w.set_can_target(s.hit_test && !s.inert);
        w.set_can_focus(!s.inert);
        if s.inert != self.inert.contains(&id) {
            if s.inert {
                self.inert.insert(id);
            } else {
                self.inert.remove(&id);
            }
            set_accessible_hidden(&w, s.inert);
        }
        layout::stop(&w, layout::Channel::Visual);
        layout::set_visual(
            &w,
            layout::Visual {
                opacity: s.opacity,
                transform: s.transform,
            },
        );
        let xalign = match s.text_align {
            TextAlign::Left => 0.,
            TextAlign::Center => 0.5,
            TextAlign::Right => 1.,
        };
        if let View::Text(id, l) = view {
            let state = self.labels.entry(*id).or_default();
            state.label = Some(l.clone());
            state.style = s.clone();
            text::apply(l, state, self.dark);
        }
        match view {
            View::Text(_, l) | View::Timer(_, l) => {
                l.set_xalign(xalign);
                l.set_justify(match s.text_align {
                    TextAlign::Left => gtk::Justification::Left,
                    TextAlign::Center => gtk::Justification::Center,
                    TextAlign::Right => gtk::Justification::Right,
                });
            }
            View::Container(..) | View::TextArea(..) => {
                // `clip()`: children clipped to the (rounded) box.
                view.widget()
                    .set_overflow(if s.corner_radius > 0. || s.clip {
                        gtk::Overflow::Hidden
                    } else {
                        gtk::Overflow::Visible
                    })
            }
            _ => {}
        }
    }

    fn set_text(&mut self, view: &View, text: &str) {
        match view {
            View::Text(id, l) => {
                let state = self.labels.entry(*id).or_default();
                state.label = Some(l.clone());
                state.text = text.to_string();
                text::apply(l, state, self.dark);
            }
            View::Button(_, b) => b.set_label(text),
            // A value from the app reaches a focused field too; echoes of
            // the typing leave it (and its caret) alone (#73).
            View::Input(id, e) => {
                let current = e.text();
                if field_takes(*id, text, &current) {
                    let caret = prism_native::fields::caret_from_end(
                        current.chars().count(),
                        e.position().max(0) as usize,
                        text.chars().count(),
                    );
                    e.set_text(text);
                    e.set_position(caret as i32);
                }
            }
            View::TextArea(id, t) => {
                let takes = field_takes(*id, text, &t.text());
                t.set_text(text, takes);
            }
            // Bytes in memory (`picture_data`): decoded by GdkTexture
            // (PNG, JPEG, TIFF; others through gdk-pixbuf's loaders).
            View::Picture(_, p) if text.starts_with("data:") => {
                let texture = prism_native::images::data_url(text).and_then(|b| {
                    gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from_owned(b)).ok()
                });
                match texture {
                    Some(t) => p.set_paintable(Some(&t)),
                    None => eprintln!("prism-linux: could not decode a data: picture"),
                }
            }
            View::Check(_, b) => b.set_label((!text.is_empty()).then_some(text)),
            // A video's source (#207): a URL, a file or an asset. GTK's
            // player sends no headers: the first line is the URL.
            View::Video(_, v) => {
                let sources = prism_native::images::ImageSources {
                    embedded: self.embedded.clone(),
                    asset_dirs: self.assets.iter().cloned().collect(),
                    cache_dir: gtk::glib::user_cache_dir().join("prism").into(),
                };
                if let Some(url) = prism_native::images::media_url(text, &sources) {
                    let url = prism_core::picture_source(&url).0.to_string();
                    v.set_file(Some(&gtk::gio::File::for_uri(&url)));
                }
            }
            // A remote image (with its headers, `Picture::header`): loaded
            // on a background thread.
            View::Picture(_, p) if prism_native::images::is_remote(text) => {
                load_remote_picture(p, text);
            }
            View::Picture(_, p) => {
                // Compiled into the binary first, then the assets directory.
                if let Some(bytes) = self.embedded.get(text) {
                    match gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from(bytes)) {
                        Ok(t) => p.set_paintable(Some(&t)),
                        Err(_) => eprintln!("prism-linux: could not decode the picture {text}"),
                    }
                } else {
                    let path = if text.starts_with('/') {
                        Some(std::path::PathBuf::from(text))
                    } else {
                        self.assets.as_ref().map(|a| a.join(text))
                    };
                    if let Some(path) = path.filter(|p| p.exists()) {
                        p.set_filename(Some(&path));
                    }
                }
            }
            // Screens and tabs: titles (the stack header / tab bar shows them).
            View::Container(id, NodeKind::Screen | NodeKind::Tab, _) => {
                self.titles.insert(*id, text.to_string());
                for st in self.stacks.clone() {
                    if st.screens.borrow().iter().any(|s| s.id() == *id) {
                        self.header_back(&st);
                    }
                }
                for t in self.tab_bars.clone() {
                    if t.tabs.borrow().iter().any(|s| s.id() == *id) {
                        self.refresh_tabs(&t);
                    }
                }
            }
            _ => {}
        }
    }

    fn set_placeholder(&mut self, view: &View, text: &str) {
        match view {
            View::Input(_, e) => {
                e.set_placeholder_text(Some(text));
                // GtkText creates its placeholder label now: mirror it too.
                controls::set_direction_deep(e.upcast_ref(), self.rtl);
            }
            View::TextArea(_, t) => {
                t.set_placeholder(text);
                set_accessible_string(&t.view, a11y_property::PLACEHOLDER, text);
            }
            View::Picker(_, p) => p.set_placeholder(text),
            View::DatePicker(_, p) => p.set_placeholder(text),
            _ => {}
        }
    }

    fn set_clickable(&mut self, view: &View, clickable: bool) {
        // A "clickable" scroll view refreshes (F5 / Ctrl+R on desktops).
        if let View::Scroll(id, ..) = view {
            if clickable {
                self.refreshable.borrow_mut().insert(*id);
            } else {
                self.refreshable.borrow_mut().remove(id);
            }
            return;
        }
        if let View::Container(id, _, c) = view {
            if clickable {
                self.clickable.borrow_mut().insert(*id);
            } else {
                self.clickable.borrow_mut().remove(id);
            }
            // A pointing hand, unless the style chose a cursor.
            if !self.cursors.contains_key(id) {
                c.set_cursor_from_name(clickable.then_some("pointer"));
            }
        }
        if let View::Text(id, l) = view {
            if clickable {
                self.clickable.borrow_mut().insert(*id);
                if !self.cursors.contains_key(id) {
                    l.set_cursor_from_name(Some("pointer"));
                }
                // A button for assistive technologies (the role can only
                // change before the widget is realized).
                if !l.is_realized() {
                    l.set_property("accessible-role", gtk::AccessibleRole::Button);
                }
            } else {
                self.clickable.borrow_mut().remove(id);
                if !self.cursors.contains_key(id) {
                    l.set_cursor_from_name(None);
                }
            }
            self.update_text_focus(*id, l);
        }
    }

    fn set_children(&mut self, parent: &View, children: &[View]) {
        let (sheets, children): (Vec<View>, Vec<View>) = children
            .iter()
            .cloned()
            .partition(|c| matches!(c, View::Sheet(..)));
        for sheet in &sheets {
            if let View::Sheet(_, s) = sheet {
                s.window.present();
            }
        }
        let host: gtk::Widget = match parent {
            View::Container(_, _, c) => c.clone().upcast(),
            View::Scroll(_, _, content) => content.clone().upcast(),
            View::Stack(_, st) => st.root.clone().upcast(),
            View::Tabs(_, t) => t.root.clone().upcast(),
            View::Sheet(_, s) => {
                *s.children.borrow_mut() = children.clone();
                if let Some(View::Stack(_, st)) = children.first() {
                    st.in_sheet.set(true);
                    self.header_back(st);
                }
                s.content.clone().upcast()
            }
            View::Overlay(_, o) => o.content.clone().upcast(),
            _ => return,
        };
        // Remove the old children (keeping a stack's header and a tab bar).
        let keep: Vec<gtk::Widget> = match parent {
            View::Stack(_, st) => vec![st.header.clone().upcast()],
            View::Tabs(_, t) => vec![t.bar.clone().upcast()],
            _ => vec![],
        };
        let mut child = host.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            // Context-menu popovers are children too: they stay, like
            // pictures of removed views animating out.
            if !keep.contains(&c) && !c.is::<gtk::Popover>() && !c.has_css_class("prism-ghost") {
                c.unparent();
            }
        }
        for c in &children {
            let w = c.widget();
            if w.parent().is_some() {
                w.unparent();
            }
            w.set_parent(&host);
        }
        // Sticky views draw over the content that scrolls under them.
        for c in &children {
            let w = c.widget();
            if layout::is_sticky(&w) {
                layout::raise(&w);
            }
        }
        // The header and the tab bar draw over the screens.
        for bar in &keep {
            bar.unparent();
            bar.set_parent(&host);
        }
        match parent {
            View::Stack(_, st) => {
                *st.screens.borrow_mut() = children;
                self.header_back(st);
            }
            View::Tabs(_, t) => {
                *t.tabs.borrow_mut() = children;
                self.refresh_tabs(t);
            }
            _ => {}
        }
    }

    fn set_root(&mut self, view: &View) {
        let mut child = self.root.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            if !self.layers.contains(&c) {
                c.unparent();
            }
        }
        view.widget().set_parent(&self.root);
        self.raise_layers();
    }

    fn set_drawing(&mut self, view: &View, paths: &[prism_core::Path]) {
        if let View::Shape(_, area, p) = view {
            *p.borrow_mut() = paths.to_vec();
            area.queue_draw();
        }
    }

    fn set_scene(&mut self, view: &View, update: &prism_core::scene3d::SceneUpdate, _json: &str) {
        if let View::Scene(_, s) = view {
            s.set_scene(update);
        }
    }

    fn destroy(&mut self, view: &View) {
        let id = view.id();
        TYPED.with(|t| t.borrow_mut().remove(&id));
        forget_selection(id);
        crate::navbar::forget(id);
        self.shapes.remove(&id);
        self.scenes.remove(&id);
        self.controls.remove(&id);
        self.date_pickers.remove(&id);
        self.statuses.remove(&id);
        self.labels.remove(&id);
        self.rules.remove(&id);
        self.adaptive.remove(&id);
        self.css_dirty = true;
        self.clickable.borrow_mut().remove(&id);
        self.reports.borrow_mut().remove(&id);
        self.watched.remove(&id);
        self.cursors.remove(&id);
        self.timers.remove(&id);
        if let Some(p) = self.menus.borrow_mut().remove(&id) {
            p.unparent();
        }
        self.swipes.borrow_mut().remove(&id);
        self.scroll_events.borrow_mut().remove(&id);
        self.refreshable.borrow_mut().remove(&id);
        self.titles.remove(&id);
        self.icons.remove(&id);
        self.sizes.remove(&id);
        {
            let mut stickies = self.stickies.borrow_mut();
            stickies.remove(&id);
            for views in stickies.values_mut() {
                views.remove(&id);
            }
        }
        if let View::Stack(_, st) = view {
            self.stacks.retain(|s| !Rc::ptr_eq(s, st));
        }
        if let View::Tabs(_, t) = view {
            self.tab_bars.retain(|b| !Rc::ptr_eq(b, t));
        }
        if let View::Sheet(_, s) = view {
            s.window.destroy();
            return;
        }
        if let View::Overlay(_, o) = view {
            o.open.set(false);
            o.teardown();
        }
        self.pointer.borrow_mut().remove(&id);
        self.pointer_attached.retain(|(n, _)| *n != id);
        self.drop_targets.remove(&id);
        self.keys.local.borrow_mut().remove(&id);
        self.keys.global.borrow_mut().remove(&id);
        self.inert.remove(&id);
        self.layers.retain(|l| *l != view.widget());
        let w = view.widget();
        layout::forget(&w);
        if w.parent().is_some() {
            w.unparent();
        }
    }

    fn measure(&mut self, view: &View, max_width: Option<f32>) -> (f32, f32) {
        let w = view.widget();
        // GTK measures with the CSS padding: the content is without it
        // (the layout adds it, #14).
        let p = crate::padding::of(view.id());
        let (px, py) = (p.left + p.right, p.top + p.bottom);
        let (_, nat_w, _, _) = w.measure(gtk::Orientation::Horizontal, -1);
        let nat_w = (nat_w as f32 - px).max(0.);
        // A video (#207): the stream's size once known, else 16:9.
        if let View::Video(_, v) = view {
            let natural = v
                .media_stream()
                .map(|m| (m.intrinsic_width(), m.intrinsic_height()));
            return match natural {
                Some((w, h)) if w > 0 && h > 0 => {
                    let (w, h) = (w as f32, h as f32);
                    let shown = max_width.map_or(w, |m| m.min(w));
                    (shown, shown * h / w)
                }
                _ => {
                    let w = max_width.unwrap_or(320.);
                    (w, w * 9. / 16.)
                }
            };
        }
        let width = match (view, max_width) {
            (View::Text(..), Some(m)) => nat_w.min(m),
            (View::Input(..) | View::Progress(..), Some(m)) => m,
            // GtkScale's natural width is its knob: sliders take 200 px.
            (View::Slider(..), m) => m.map_or(200., |m| m.min(200.)),
            // As wide as offered; as tall as its text, at least 3 lines.
            (View::TextArea(_, t), m) => {
                let w = m.unwrap_or(240.).ceil();
                return (w, (t.measure_height(w + px) - py).max(0.).ceil());
            }
            _ => nat_w,
        };
        let (_, nat_h, _, _) = w.measure(gtk::Orientation::Vertical, (width + px).ceil() as i32);
        (width.ceil(), (nat_h as f32 - py).max(0.))
    }

    /// GTK's minimum width of a wrapping label is its longest word.
    fn measure_min_content(&mut self, view: &View) -> f32 {
        let (min_w, nat_w, _, _) = view.widget().measure(gtk::Orientation::Horizontal, -1);
        if matches!(view, View::Text(..)) {
            min_w as f32
        } else {
            nat_w as f32
        }
    }

    fn set_frame(&mut self, view: &View, x: f32, y: f32, w: f32, h: f32) {
        self.sizes.insert(view.id(), (w, h));
        match view {
            View::Sheet(..) => {}
            View::Stack(_, st) => {
                layout::set_frame(&st.root, rect(x, y, w, h));
                layout::set_frame(&st.header, rect(0., 0., w, HEADER));
            }
            View::Tabs(_, t) => {
                layout::set_frame(&t.root, rect(x, y, w, h));
                layout::set_frame(&t.bar, rect(0., h - TAB_BAR, w, TAB_BAR));
            }
            _ => layout::set_frame(&view.widget(), rect(x, y, w, h)),
        }
    }

    fn set_frame_animated(
        &mut self,
        view: &View,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        t: prism_core::Transition,
    ) {
        match view {
            View::Container(..)
            | View::Text(..)
            | View::Image(..)
            | View::Button(..)
            | View::Picture(..)
            | View::Toggle(..)
            | View::Progress(..)
            | View::Timer(..) => {
                self.sizes.insert(view.id(), (w, h));
                layout::animate_frame(&view.widget(), rect(x, y, w, h), t);
            }
            _ => self.set_frame(view, x, y, w, h),
        }
    }

    fn set_style_animated(&mut self, view: &View, style: &Style, t: prism_core::Transition) {
        let w = view.widget();
        let from = layout::visual(&w);
        self.set_style(view, style);
        let to = layout::visual(&w);
        if from != to {
            layout::set_visual(&w, from);
            layout::animate_visual(&w, to, t, || {});
        }
    }

    /// A still picture of the view stays in its place and animates out.
    fn animate_exit(&mut self, parent: &View, view: &View, to: &Style, t: prism_core::Transition) {
        let host: gtk::Widget = match parent {
            View::Container(_, _, c) => c.clone().upcast(),
            View::Scroll(_, _, content) => content.clone().upcast(),
            _ => return,
        };
        let w = view.widget();
        let Some(frame) = layout::frame_of(&w) else {
            return;
        };
        if w.parent().as_ref() != Some(&host) || frame.2 <= 0 || frame.3 <= 0 {
            return;
        }
        let image = gtk::WidgetPaintable::new(Some(&w)).current_image();
        let picture = gtk::Picture::for_paintable(&image);
        picture.set_can_shrink(true);
        picture.set_can_target(false);
        picture.add_css_class("prism-ghost");
        picture.set_parent(&host);
        layout::set_frame(&picture, frame);
        layout::set_visual(&picture, layout::visual(&w));
        let ghost = picture.clone();
        layout::animate_visual(
            &picture,
            layout::Visual {
                opacity: to.opacity,
                transform: to.transform,
            },
            t,
            move || {
                layout::forget(&ghost);
                ghost.unparent();
            },
        );
    }

    fn set_repeat(
        &mut self,
        view: &View,
        style: &Style,
        repeat: Option<(prism_core::Pose, prism_core::Transition)>,
    ) {
        let w = view.widget();
        let rest = layout::Visual {
            opacity: style.opacity,
            transform: style.transform,
        };
        match repeat {
            None => {
                layout::stop(&w, layout::Channel::Loop);
                layout::set_visual(&w, rest);
            }
            Some((pose, t)) => {
                let (opacity, transform) = pose.apply(style.opacity, style.transform);
                let to = layout::Visual { opacity, transform };
                if style.animations.restart {
                    layout::repeat_from_start(&w, rest, to, t);
                } else {
                    layout::repeat(&w, rest, to, t);
                }
            }
        }
    }

    fn screen_viewport(&mut self, container: &View, child: &View) -> Viewport {
        if let View::Sheet(_, s) = child {
            let (w, h) = (s.window.width().max(520), s.window.height().max(620));
            return Viewport {
                x: 0.,
                y: 0.,
                width: w as f32,
                height: h as f32,
                insets: Insets::default(),
            };
        }
        let (w, h) = self
            .sizes
            .get(&container.id())
            .copied()
            .unwrap_or(self.size);
        match container {
            View::Tabs(..) => Viewport {
                x: 0.,
                y: 0.,
                width: w,
                height: (h - TAB_BAR).max(0.),
                insets: Insets::default(),
            },
            // No header bar: the screen gets the whole stack.
            View::Stack(_, st) if self.header_hidden(st) => Viewport {
                x: 0.,
                y: 0.,
                width: w,
                height: h,
                insets: Insets::default(),
            },
            _ => Viewport {
                x: 0.,
                y: HEADER,
                width: w,
                height: (h - HEADER).max(0.),
                insets: Insets::default(),
            },
        }
    }

    fn set_accessibility(&mut self, view: &View, a11y: &Accessibility) {
        let w = view.widget();
        if let Some(label) = &a11y.label {
            let target: gtk::Widget = match view {
                // The focusable part reads the label.
                View::Picker(_, p) => p.dropdown.clone().upcast(),
                View::TextArea(_, t) => t.view.clone().upcast(),
                _ => w.clone(),
            };
            set_accessible_label(&target, label);
        }
        if let Some(hint) = &a11y.hint {
            set_accessible_string(&w, a11y_property::DESCRIPTION, hint);
        }
        // A selected item or the current page of a navigation (GTK has no
        // « current » state).
        set_accessible_selected(&w, a11y.is_selected());
        match view {
            // GtkScale exposes its own range and value (VALUE_MIN/MAX/NOW,
            // gtkrange.c); a formatted value ("40 %") is read instead.
            View::Slider(_, s, _) => {
                if let Some(v) = &a11y.value {
                    set_accessible_string(s, a11y_property::VALUE_TEXT, v);
                }
                s.set_sensitive(!a11y.disabled);
            }
            View::Picker(_, p) => {
                if let Some(v) = &a11y.value {
                    set_accessible_string(&p.dropdown, a11y_property::VALUE_TEXT, v);
                }
                p.dropdown.set_sensitive(!a11y.disabled);
            }
            View::DatePicker(_, p) => p.button.set_sensitive(!a11y.disabled),
            View::Check(_, b) => b.set_sensitive(!a11y.disabled),
            View::Radio(_, r) => r.set_sensitive(!a11y.disabled),
            View::TextArea(_, t) => t.view.set_sensitive(!a11y.disabled),
            View::Container(id, ..) if a11y.role == Some(prism_core::Role::Status) => {
                // A live region: GTK 4.8 has ROLE_STATUS (gtkenums.h); the
                // role can only change before the widget is realized.
                if !w.is_realized()
                    && w.property::<gtk::AccessibleRole>("accessible-role")
                        != gtk::AccessibleRole::Status
                {
                    w.set_property("accessible-role", gtk::AccessibleRole::Status);
                }
                let value = a11y.value.clone().unwrap_or_default();
                if self.statuses.get(id) != Some(&value) {
                    if !value.is_empty() {
                        controls::announce(&w, &value);
                    }
                    self.statuses.insert(*id, value);
                }
            }
            // Drawn choices (#84): option cards that are radios, custom
            // checkboxes and switches, with their checked state.
            View::Container(..) => {
                let role = match a11y.role {
                    Some(prism_core::Role::Radio) => gtk::AccessibleRole::Radio,
                    Some(prism_core::Role::RadioGroup) => gtk::AccessibleRole::RadioGroup,
                    Some(prism_core::Role::Checkbox) => gtk::AccessibleRole::Checkbox,
                    Some(prism_core::Role::Switch) => gtk::AccessibleRole::Switch,
                    _ => return,
                };
                // As above: the role can only change before it is realized.
                if !w.is_realized() && w.property::<gtk::AccessibleRole>("accessible-role") != role
                {
                    w.set_property("accessible-role", role);
                }
                if role != gtk::AccessibleRole::RadioGroup {
                    set_accessible_checked(&w, a11y.checked.unwrap_or(false));
                }
            }
            _ => {}
        }
    }

    fn set_value(&mut self, view: &View, value: f64) {
        match view {
            View::Toggle(_, t) => t.set_active(value != 0.),
            View::Progress(_, p) => p.set_fraction(value.clamp(0., 1.)),
            View::Picture(_, p) => p.set_content_fit(if value == 1. {
                gtk::ContentFit::Contain
            } else {
                gtk::ContentFit::Cover
            }),
            View::Timer(id, l) => {
                l.set_text(&prism_core::format_remaining(value));
                self.timers.insert(*id, (l.clone(), value));
            }
            View::Tabs(_, t) => {
                t.selected.set(value.max(0.) as usize);
                self.refresh_tabs(t);
            }
            View::Stack(_, st) => {
                st.no_header
                    .set((value as u32) & prism_core::STACK_NO_HEADER != 0);
                self.header_back(st);
            }
            View::Container(id, NodeKind::Screen, _) => {
                let changed = if value == 1. {
                    self.bare.insert(*id)
                } else {
                    self.bare.remove(id)
                };
                if changed {
                    for st in self.stacks.clone() {
                        self.header_back(&st);
                    }
                }
            }
            View::Slider(_, s, c) => s.set_value(c.get().snap(value)),
            View::Picker(_, p) => p.select(value),
            View::DatePicker(_, p) => p.set_value(value),
            View::Spinner(_, s) => controls::spinner_size(s, value >= 1.),
            View::Check(_, b) => b.set_active(value != 0.),
            View::Radio(_, r) => r.select(value),
            View::Overlay(_, o) => o.set_open(value != 0.),
            _ => {}
        }
    }

    fn set_config(&mut self, view: &View, _kind: NodeKind, config: &str) {
        match view {
            // How a video plays (#207). GTK's player always offers its
            // controls on hover.
            View::Video(_, v) => {
                let c = prism_core::VideoConfig::decode(config);
                v.set_autoplay(c.autoplay);
                v.set_loop(c.looping);
                if let Some(stream) = v.media_stream() {
                    stream.set_muted(c.muted);
                }
                // SAFETY: a plain bool kept on the widget for new streams.
                unsafe { v.set_data("prism-muted", c.muted) };
            }
            // Bar buttons (#30): the stack showing the screen puts them up.
            View::Container(id, NodeKind::Screen, _) => {
                if crate::navbar::set_config(*id, config) {
                    for st in self.stacks.clone() {
                        self.header_back(&st);
                    }
                }
            }
            View::Slider(_, s, c) => {
                let cfg = prism_core::controls::SliderConfig::decode(config);
                c.set(cfg);
                controls::configure_slider(s, cfg);
            }
            View::Picker(_, p) => p.set_options(config),
            View::DatePicker(_, p) => p.set_config(config),
            View::Input(_, e) => controls::configure_entry(e, config),
            View::TextArea(_, t) => controls::configure_text_view(&t.view, config),
            // A rich text's runs.
            View::Text(id, l) => {
                let state = self.labels.entry(*id).or_default();
                state.label = Some(l.clone());
                state.config = config.to_string();
                text::apply(l, state, self.dark);
                self.update_text_focus(*id, l);
            }
            View::Radio(id, r) => r.set_options(*id, config),
            View::Overlay(_, o) => o.configure(
                prism_core::controls::OverlayConfig::decode(config),
                &self.window,
                &self.keys,
            ),
            _ => {}
        }
    }

    fn set_environment(&mut self, env: &prism_core::RenderEnvironment) {
        let rtl = env.direction == prism_core::Direction::Rtl;
        let locale = prism_core::Locale::new(&env.lang);
        if rtl != self.rtl {
            self.rtl = rtl;
            for w in self.controls.values() {
                controls::set_direction_deep(w, rtl);
            }
        }
        if locale != self.locale {
            self.locale = locale.clone();
            for p in self.date_pickers.values() {
                p.set_locale(locale.clone());
            }
        }
        if env.clear != self.clear_label {
            self.clear_label = env.clear.clone();
            for p in self.date_pickers.values() {
                p.set_clear_label(&env.clear);
            }
        }
    }

    fn set_icon(&mut self, view: &View, icon: &str) {
        match view {
            View::Image(_, l) => l.set_text(prism_core::glyphs::glyph(icon)),
            // A leading icon in the field (a themed icon; unknown symbols
            // show none).
            View::Input(_, e) => e.set_icon_from_icon_name(
                gtk::EntryIconPosition::Primary,
                crate::input::freedesktop_icon(icon),
            ),
            View::Container(id, NodeKind::Tab, _) => {
                self.icons.insert(*id, icon.to_string());
                for t in self.tab_bars.clone() {
                    if t.tabs.borrow().iter().any(|s| s.id() == *id) {
                        self.refresh_tabs(&t);
                    }
                }
            }
            _ => {}
        }
    }

    fn set_menu(&mut self, view: &View, menu: Option<&prism_core::menu::MenuSpec>) {
        let id = view.id();
        if let Some(old) = self.menus.borrow_mut().remove(&id) {
            old.unparent();
        }
        if let Some(spec) = menu {
            let model = crate::services::menu_model(&self.app, spec);
            let popover = gtk::PopoverMenu::from_model(Some(&model));
            popover.set_has_arrow(false);
            popover.set_parent(&view.widget());
            self.menus.borrow_mut().insert(id, popover);
        }
    }

    fn set_swipe(&mut self, view: &View, left: bool, right: bool) {
        self.swipes.borrow_mut().insert(view.id(), (left, right));
    }

    fn set_scroll_events(&mut self, view: &View, on: bool) {
        if on {
            self.scroll_events.borrow_mut().insert(view.id());
        } else {
            self.scroll_events.borrow_mut().remove(&view.id());
        }
    }

    fn set_hidden(&mut self, view: &View, hidden: bool) {
        view.widget().set_visible(!hidden);
    }

    /// Labels, buttons and fields report their first baseline from GTK's
    /// measure at the width they were given.
    fn baseline(&mut self, view: &View, width: f32, _height: f32) -> Option<f32> {
        let w = match view {
            View::Text(..)
            | View::Timer(..)
            | View::Image(..)
            | View::Button(..)
            | View::Input(..)
            | View::Picker(..)
            | View::DatePicker(..) => view.widget(),
            _ => return None,
        };
        let (_, _, _, natural) = w.measure(gtk::Orientation::Vertical, width.ceil() as i32);
        (natural >= 0).then_some(natural as f32)
    }

    fn set_sticky(&mut self, view: &View, sticky: Option<(&View, prism_native::Sticky)>) {
        let id = view.id();
        let w = view.widget();
        let mut stickies = self.stickies.borrow_mut();
        for views in stickies.values_mut() {
            views.remove(&id);
        }
        match sticky {
            Some((View::Scroll(sid, sw, _), st)) => {
                stickies
                    .entry(*sid)
                    .or_default()
                    .insert(id, (w.clone(), st));
                drop(stickies);
                layout::set_sticky_offset(&w, Some(st.offset(sw.vadjustment().value() as f32)));
            }
            _ => {
                drop(stickies);
                layout::set_sticky_offset(&w, None);
            }
        }
    }

    fn set_content_size(&mut self, view: &View, width: f32, height: f32) {
        match view {
            View::Scroll(_, _, content) => {
                content.set_size_request(width.ceil() as i32, height.ceil() as i32)
            }
            View::Overlay(_, o) => {
                o.set_size(width.ceil() as i32, height.ceil() as i32);
                o.show_if_pending();
            }
            _ => {}
        }
    }

    // --- Pointer, keys, focus, layers, overlays (#17, #19, #25) ---------------

    fn set_drop_target(&mut self, view: &View, types: Option<&[String]>) {
        use prism_core::Interaction::DropHover;
        let id = view.id();
        let w = view.widget();
        if let Some(old) = self.drop_targets.remove(&id) {
            w.remove_controller(&old);
        }
        let Some(types) = types else { return };
        let accept = prism_core::PickFiles::new().types(types.to_vec());
        // Files from the file manager or another app (#111); GTK sets
        // `:drop(active)` while they are over it.
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        target.connect_enter(move |_, _, _| {
            report_state(id, DropHover, true);
            gdk::DragAction::COPY
        });
        target.connect_leave(move |_| report_state(id, DropHover, false));
        target.connect_drop(move |_, value, _, _| {
            report_state(id, DropHover, false);
            let Ok(list) = value.get::<gdk::FileList>() else {
                return false;
            };
            let paths: Vec<_> = list.files().iter().filter_map(|f| f.path()).collect();
            let accept = accept.clone();
            // Copying large files takes a while: off the main loop.
            glib::MainContext::default().spawn_local(async move {
                let copied = gio::spawn_blocking(move || {
                    paths
                        .into_iter()
                        .filter_map(crate::services::copy_picked_path)
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default();
                let files: Vec<_> = copied
                    .into_iter()
                    .filter(|f| accept.accepts(&f.name, &f.mime))
                    .collect();
                if !files.is_empty() {
                    dispatch(UiEvent::DropFiles(id, files));
                }
            });
            true
        });
        w.add_controller(target.clone());
        self.drop_targets.insert(id, target);
    }

    fn set_pointer_events(&mut self, view: &View, mask: u8) {
        use prism_core::events::pointer_mask::{DRAG_ANY, HOVER, MAGNIFY, MOVE, WHEEL};
        let id = view.id();
        self.pointer.borrow_mut().insert(id, mask);
        let w = view.widget();
        if mask & (HOVER | MOVE) != 0 && self.pointer_attached.insert((id, HOVER)) {
            let motion = gtk::EventControllerMotion::new();
            let masks = self.pointer.clone();
            let has = move |bits: u8| masks.borrow().get(&id).is_some_and(|m| m & bits != 0);
            let pointer = move |phase, x, y| {
                UiEvent::Pointer(
                    id,
                    prism_core::PointerEvent {
                        phase,
                        position: point(x, y),
                    },
                )
            };
            let h = has.clone();
            motion.connect_enter(move |_, x, y| {
                if h(HOVER | MOVE) {
                    dispatch(pointer(prism_core::PointerPhase::Enter, x, y));
                }
            });
            let h = has.clone();
            motion.connect_motion(move |_, x, y| {
                if h(MOVE) {
                    dispatch_latest(id, 0, pointer(prism_core::PointerPhase::Move, x, y));
                }
            });
            motion.connect_leave(move |_| {
                if has(HOVER | MOVE) {
                    dispatch(pointer(prism_core::PointerPhase::Leave, 0., 0.));
                }
            });
            w.add_controller(motion);
        }
        if mask & DRAG_ANY != 0 && self.pointer_attached.insert((id, DRAG_ANY)) {
            let drag = gtk::GestureDrag::new();
            // Any button; the element's `drag_buttons` pick (#136).
            drag.set_button(0);
            let masks = self.pointer.clone();
            // Where the pointer went down, with which button, and whether
            // the drag started.
            let state: Rc<Cell<Option<DragStart>>> = Rc::default();
            let threshold = gtk::Settings::default()
                .map_or(8, |s| s.gtk_dnd_drag_threshold())
                .max(1) as f64;
            let event = move |phase, d: DragStart, dx: f64, dy: f64| {
                UiEvent::Drag(
                    id,
                    prism_core::DragEvent {
                        phase,
                        start: point(d.at.0, d.at.1),
                        translation: point(dx, dy),
                        button: d.button,
                        modifiers: d.mods,
                    },
                )
            };
            let st = state.clone();
            drag.connect_drag_begin(move |g, x, y| {
                let button = drag_button(g.current_button());
                let on = masks
                    .borrow()
                    .get(&id)
                    .is_some_and(|m| m & button.drag_bit() != 0);
                st.set(on.then_some(DragStart {
                    at: (x, y),
                    button,
                    mods: modifiers(g.current_event_state()),
                    started: false,
                }));
            });
            let st = state.clone();
            drag.connect_drag_update(move |g, dx, dy| {
                let Some(d) = st.get() else {
                    return;
                };
                if !d.started {
                    if dx.hypot(dy) < threshold {
                        return;
                    }
                    // A drag, not a click: the click gestures (and the
                    // middle button's paste, the secondary's menu) stop here.
                    g.set_state(gtk::EventSequenceState::Claimed);
                    let d = DragStart { started: true, ..d };
                    st.set(Some(d));
                    dispatch(event(prism_core::DragPhase::Start, d, dx, dy));
                    return;
                }
                dispatch_latest(id, 1, event(prism_core::DragPhase::Move, d, dx, dy));
            });
            let st = state.clone();
            drag.connect_drag_end(move |_, dx, dy| {
                if let Some(d) = st.take().filter(|d| d.started) {
                    dispatch(event(prism_core::DragPhase::End, d, dx, dy));
                }
            });
            drag.connect_cancel(move |_, _| {
                if let Some(d) = state.take().filter(|d| d.started) {
                    dispatch(event(prism_core::DragPhase::Cancel, d, 0., 0.));
                }
            });
            w.add_controller(drag);
        }
        if mask & WHEEL != 0 && self.pointer_attached.insert((id, WHEEL)) {
            // Wheels and touchpads (#133): the element takes the scrolling.
            let scroll =
                gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
            let masks = self.pointer.clone();
            let on = move || masks.borrow().get(&id).is_some_and(|m| m & WHEEL != 0);
            // A touchpad's gesture: between `scroll-begin` and `scroll-end`.
            let gesture: Rc<Cell<Option<bool>>> = Rc::default();
            let g = gesture.clone();
            scroll.connect_scroll_begin(move |_| g.set(Some(false)));
            let (g, on2) = (gesture.clone(), on.clone());
            scroll.connect_scroll_end(move |c| {
                if g.take() == Some(true) && on2() {
                    dispatch(wheel_event(id, c, 0., 0., prism_core::GesturePhase::End));
                }
            });
            scroll.connect_scroll(move |c, dx, dy| {
                if !on() {
                    return glib::Propagation::Proceed;
                }
                let phase = match gesture.get() {
                    None => prism_core::GesturePhase::None,
                    Some(false) => {
                        gesture.set(Some(true));
                        prism_core::GesturePhase::Start
                    }
                    Some(true) => prism_core::GesturePhase::Move,
                };
                dispatch(wheel_event(id, c, dx, dy, phase));
                glib::Propagation::Stop
            });
            w.add_controller(scroll);
        }
        if mask & MAGNIFY != 0 && self.pointer_attached.insert((id, MAGNIFY)) {
            // Pinches (#133): touchscreens and touchpads. GTK gives the scale
            // since the gesture began; Prism the change since the last event.
            let zoom = gtk::GestureZoom::new();
            let masks = self.pointer.clone();
            let last = Rc::new(Cell::new(1.));
            let event = move |g: &gtk::GestureZoom, scale: f64, phase| {
                let (x, y) = g.bounding_box_center().unwrap_or_default();
                UiEvent::Magnify(
                    id,
                    prism_core::MagnifyEvent {
                        scale: scale.max(0.01) as f32,
                        position: point(x, y),
                        phase,
                    },
                )
            };
            let l = last.clone();
            zoom.connect_begin(move |g, _| {
                if !masks.borrow().get(&id).is_some_and(|m| m & MAGNIFY != 0) {
                    g.set_state(gtk::EventSequenceState::Denied);
                    return;
                }
                l.set(1.);
                dispatch(event(g, 1., prism_core::GesturePhase::Start));
            });
            let l = last.clone();
            zoom.connect_scale_changed(move |g, now| {
                let ratio = now / l.replace(now).max(0.01);
                dispatch(event(g, ratio, prism_core::GesturePhase::Move));
            });
            zoom.connect_end(move |g, _| {
                dispatch(event(g, 1., prism_core::GesturePhase::End));
            });
            w.add_controller(zoom);
        }
    }

    /// Text field selections (#86), in characters like GTK.
    fn set_selection(&mut self, view: &View, start: usize, end: usize) {
        match view {
            View::Input(id, e) => {
                let len = e.text().chars().count();
                e.select_region(start.min(len) as i32, end.min(len) as i32);
                APP_SELECTED.with(|s| s.borrow_mut().insert(*id));
            }
            View::TextArea(_, t) => t.select(start, end),
            _ => {}
        }
    }

    fn set_select_events(&mut self, view: &View, on: bool) {
        let id = view.id();
        if on {
            SELECT_EVENTS.with(|s| s.borrow_mut().insert(id));
        } else {
            forget_selection(id);
        }
    }

    fn set_key_events(&mut self, view: &View, local: bool, global: bool) {
        let id = view.id();
        let set = |s: &RefCell<HashSet<NodeId>>, on: bool| {
            if on {
                s.borrow_mut().insert(id);
            } else {
                s.borrow_mut().remove(&id);
            }
        };
        set(&self.keys.local, local);
        set(&self.keys.global, global);
        // A view listening to keys can take the focus (Tab, a click).
        if let View::Container(_, _, c) = view {
            c.set_focusable(local);
            if local {
                c.add_css_class("prism-keys");
                let focus = gtk::GestureClick::new();
                let weak = c.downgrade();
                focus.connect_pressed(move |_, _, _, _| {
                    if let Some(c) = weak.upgrade()
                        && c.is_focusable()
                    {
                        c.grab_focus();
                    }
                });
                if self.pointer_attached.insert((id, 8)) {
                    c.add_controller(focus);
                }
            } else {
                c.remove_css_class("prism-keys");
            }
        }
    }

    fn focus(&mut self, view: &View) {
        let target: gtk::Widget = match view {
            View::Input(_, e) => e.clone().upcast(),
            View::TextArea(_, t) => t.view.clone().upcast(),
            View::Picker(_, p) => p.dropdown.clone().upcast(),
            View::DatePicker(_, p) => p.button.clone().upcast(),
            View::Radio(_, r) => r
                .root
                .first_child()
                .unwrap_or_else(|| r.root.clone().upcast()),
            other => other.widget(),
        };
        if target.is::<gtk::Box>() && !target.is_focusable() {
            target.set_focusable(true);
        }
        // An entry keeps the selection the app gave it (#86).
        let keep = match view {
            View::Input(id, _) => APP_SELECTED.with(|s| s.borrow().contains(id)),
            _ => false,
        };
        // After the layout pass: a new widget is not mapped yet.
        glib::idle_add_local_once(move || match target.downcast_ref::<gtk::Entry>() {
            Some(e) if keep => {
                e.grab_focus_without_selecting();
            }
            _ => {
                target.grab_focus();
            }
        });
    }

    /// The window keeps the focus but no widget has it (an on-screen
    /// keyboard hides); sheets are their own windows.
    fn dismiss_keyboard(&mut self) {
        let windows: Vec<gtk::Window> = gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Window>().ok())
            .filter(|w| w.is_active())
            .chain(std::iter::once(self.window.clone().upcast()))
            .collect();
        glib::idle_add_local_once(move || {
            for w in windows {
                GtkWindowExt::set_focus(&w, None::<&gtk::Widget>);
            }
        });
    }

    /// Scrolls every scroll view around the widget so its frame is visible.
    /// `cx.scroll_by` / `cx.scroll_to` (#193): the scrolled window's
    /// adjustments, kept within the content.
    fn scroll_to(&mut self, view: &View, x: f32, y: f32, relative: bool, _animated: bool) {
        let View::Scroll(_, sw, _) = view else {
            return;
        };
        for (adj, v) in [(sw.hadjustment(), x as f64), (sw.vadjustment(), y as f64)] {
            let target = if relative { adj.value() + v } else { v };
            let max = (adj.upper() - adj.page_size()).max(adj.lower());
            let target = if target.is_nan() {
                adj.lower()
            } else {
                target.clamp(adj.lower(), max)
            };
            adj.set_value(target);
        }
    }

    fn scroll_into_view(&mut self, view: &View) {
        let mut w = view.widget();
        loop {
            // Offset of `w` inside the scroll view's content, from the
            // frames Prism gave each widget on the way up.
            let (mut y, h) = layout::frame_of(&w).map_or((0, 0), |f| (f.1, f.3));
            let mut up = w.parent();
            let sw = loop {
                let Some(p) = up else { return };
                if let Some(sw) = p.downcast_ref::<gtk::ScrolledWindow>() {
                    break sw.clone();
                }
                if !p.is::<gtk::Viewport>() {
                    y += layout::frame_of(&p).map_or(0, |f| f.1);
                }
                up = p.parent();
            };
            let adj = sw.vadjustment();
            let (top, bottom) = (f64::from(y) - 12., f64::from(y + h) + 12.);
            let (value, page) = (adj.value(), adj.page_size());
            // The adjustment clamps to its range.
            if top < value || bottom - top > page {
                adj.set_value(top);
            } else if bottom > value + page {
                adj.set_value(bottom - page);
            }
            w = sw.upcast();
        }
    }

    /// Layers are the last children of the window's root container (drawn
    /// above it); they let input through where they have no child.
    fn set_layers(&mut self, layers: &[View]) {
        let next: Vec<gtk::Widget> = layers.iter().map(View::widget).collect();
        for old in &self.layers {
            if !next.contains(old)
                && old.parent().as_ref() == Some(self.root.upcast_ref::<gtk::Widget>())
            {
                old.remove_css_class(layout::PASS_THROUGH);
                old.unparent();
            }
        }
        self.layers = next;
        for l in &self.layers {
            l.add_css_class(layout::PASS_THROUGH);
        }
        self.raise_layers();
    }

    /// A popover gets a phone-like width; dialogs and sheets fit the
    /// window (a sheet fills its height, or half of it).
    fn overlay_viewport(&mut self, overlay: &View, _content: &View) -> Viewport {
        use prism_core::OverlayKind;
        let (w, h) = self.size;
        let config = match overlay {
            View::Overlay(_, o) => o.config.get(),
            _ => Default::default(),
        };
        let (width, height) = match config.kind {
            OverlayKind::Popover => ((w - 32.).clamp(120., 360.), (h * 0.8).max(120.)),
            OverlayKind::Dialog => ((w - 48.).clamp(200., 480.), (h - 80.).max(160.)),
            OverlayKind::Sheet if config.medium => (w.min(560.), (h * 0.5).max(200.)),
            OverlayKind::Sheet => (w.min(560.), (h - 48.).max(200.)),
        };
        Viewport {
            x: 0.,
            y: 0.,
            width,
            height,
            insets: Insets::default(),
        }
    }

    fn set_appearance(&mut self, appearance: Appearance) {
        let dark = match appearance {
            Appearance::Dark => true,
            Appearance::Light => false,
            Appearance::System => self.dark,
        };
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_application_prefer_dark_theme(dark);
        }
        let changed = self.dark != dark;
        self.dark = dark;
        SHAPE_DARK.with(|d| d.set(dark));
        // Backgrounds, borders, shadows, gradients: CSS rules of the
        // light or the dark variant.
        if changed {
            let adaptive: Vec<(View, Style)> = self.adaptive.values().cloned().collect();
            for (view, style) in adaptive {
                self.set_style(&view, &style);
            }
        }
        // Run colors are Pango attributes: the light or the dark variant.
        for state in self.labels.values() {
            if let Some(l) = &state.label {
                text::apply(l, state, dark);
            }
        }
        for area in self.shapes.values() {
            area.queue_draw();
        }
        for area in self.scenes.values() {
            area.queue_render();
        }
    }
}

/// `GtkAccessibleProperty` values (gtkenums.h, counted from 0).
pub(crate) mod a11y_property {
    pub const DESCRIPTION: i32 = 1;
    pub const LABEL: i32 = 4;
    pub const PLACEHOLDER: i32 = 10;
    pub const VALUE_TEXT: i32 = 18;
}

unsafe extern "C" {
    /// gtkaccessible.h, `GDK_AVAILABLE_IN_ALL` (gtk-rs only binds it from 4.10).
    fn gtk_accessible_update_property_value(
        accessible: *mut std::ffi::c_void,
        n_properties: std::ffi::c_int,
        properties: *const i32,
        values: *const gtk::glib::gobject_ffi::GValue,
    );
}

/// Sets a string accessible property of `w` (see [`a11y_property`]).
pub(crate) fn set_accessible_string(w: &impl IsA<gtk::Widget>, property: i32, text: &str) {
    use gtk::glib::translate::{ToGlibPtr, ToGlibPtrMut};
    let mut value = text.to_value();
    let widget: *mut gtk::ffi::GtkWidget = w.as_ref().to_glib_none().0;
    // SAFETY: every GtkWidget implements GtkAccessible; one string property.
    unsafe {
        gtk_accessible_update_property_value(
            widget.cast(),
            1,
            &property,
            value.to_glib_none_mut().0,
        );
    }
}

unsafe extern "C" {
    /// gtkaccessible.h, `GDK_AVAILABLE_IN_ALL` (gtk-rs binds it from 4.10).
    fn gtk_accessible_update_state_value(
        accessible: *mut std::ffi::c_void,
        n_states: std::ffi::c_int,
        states: *const i32,
        values: *const gtk::glib::gobject_ffi::GValue,
    );
}

/// `GTK_ACCESSIBLE_STATE_HIDDEN` (gtkenums.h): out of the accessibility
/// tree (inert subtrees).
pub(crate) fn set_accessible_hidden(w: &impl IsA<gtk::Widget>, hidden: bool) {
    use gtk::glib::translate::{ToGlibPtr, ToGlibPtrMut};
    const HIDDEN: i32 = 4;
    let mut value = hidden.to_value();
    let widget: *mut gtk::ffi::GtkWidget = w.as_ref().to_glib_none().0;
    // SAFETY: every GtkWidget implements GtkAccessible; one boolean state.
    unsafe {
        gtk_accessible_update_state_value(widget.cast(), 1, &HIDDEN, value.to_glib_none_mut().0);
    }
}

/// `GTK_ACCESSIBLE_STATE_CHECKED` (gtkenums.h): a radio, checkbox or switch
/// drawn by the app is on (a `GtkAccessibleTristate`).
fn set_accessible_checked(w: &impl IsA<gtk::Widget>, checked: bool) {
    use gtk::glib::translate::{ToGlibPtr, ToGlibPtrMut};
    const CHECKED: i32 = 1;
    let state = if checked {
        gtk::AccessibleTristate::True
    } else {
        gtk::AccessibleTristate::False
    };
    let mut value = state.to_value();
    let widget: *mut gtk::ffi::GtkWidget = w.as_ref().to_glib_none().0;
    // SAFETY: every GtkWidget implements GtkAccessible; one tristate state.
    unsafe {
        gtk_accessible_update_state_value(widget.cast(), 1, &CHECKED, value.to_glib_none_mut().0);
    }
}

/// `GTK_ACCESSIBLE_STATE_SELECTED` (gtkenums.h): a selected item.
pub(crate) fn set_accessible_selected(w: &impl IsA<gtk::Widget>, selected: bool) {
    use gtk::glib::translate::{ToGlibPtr, ToGlibPtrMut};
    const SELECTED: i32 = 7;
    let mut value = selected.to_value();
    let widget: *mut gtk::ffi::GtkWidget = w.as_ref().to_glib_none().0;
    // SAFETY: every GtkWidget implements GtkAccessible; one boolean state.
    unsafe {
        gtk_accessible_update_state_value(widget.cast(), 1, &SELECTED, value.to_glib_none_mut().0);
    }
}

/// What screen readers say for `w`.
fn set_accessible_label(w: &gtk::Widget, label: &str) {
    set_accessible_string(w, a11y_property::LABEL, label);
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn data_dir(title: &str) -> std::path::PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".local/share")))
        .unwrap_or_else(|| ".".into());
    base.join(title.replace('/', "-"))
}

/// Active, inactive (another window focused) or background (minimized).
fn report_lifecycle(window: &gtk::ApplicationWindow) {
    let minimized = window
        .surface()
        .and_downcast::<gdk::Toplevel>()
        .is_some_and(|t| t.state().contains(gdk::ToplevelState::MINIMIZED));
    let phase = if minimized {
        prism_core::Lifecycle::Background
    } else if window.is_active() {
        prism_core::Lifecycle::Active
    } else {
        prism_core::Lifecycle::Inactive
    };
    with_state(|s| {
        let patches = s.runtime.set_lifecycle(phase);
        commit(s, patches);
    });
}

/// Locale, text size (GNOME's text scaling factor, else the font DPI over
/// the 96 default) and reduced motion (animations off).
fn environment() -> prism_core::Environment {
    let settings = gtk::Settings::default();
    let gnome = gio::SettingsSchemaSource::default()
        .and_then(|src| src.lookup("org.gnome.desktop.interface", true))
        .filter(|schema| schema.has_key("text-scaling-factor"))
        .map(|_| gio::Settings::new("org.gnome.desktop.interface"));
    let text_scale = gnome
        .map(|g| g.double("text-scaling-factor"))
        .filter(|f| *f > 0.)
        .or_else(|| {
            settings
                .as_ref()
                .map(|s| s.gtk_xft_dpi())
                .filter(|dpi| *dpi > 0)
                .map(|dpi| dpi as f64 / 1024. / 96.)
        })
        .unwrap_or(1.);
    let text_scale = std::env::var("PRISM_TEXT_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(text_scale as f32);
    prism_core::Environment {
        locale: locale(),
        text_scale,
        reduce_motion: settings.is_some_and(|s| !s.is_gtk_enable_animations()),
        ..Default::default()
    }
}

fn environment_changed() {
    with_state(|s| {
        s.runtime.app.set_environment(environment());
        let patches = s.runtime.poll();
        commit(s, patches);
    });
}

fn locale() -> prism_core::Locale {
    let lang = std::env::var("PRISM_LOCALE")
        .or_else(|_| std::env::var("LC_ALL"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    let tag = lang
        .split(['.', '@'])
        .next()
        .unwrap_or("")
        .replace('_', "-");
    prism_core::Locale::new(if tag.is_empty() || tag == "C" || tag == "POSIX" {
        "en"
    } else {
        &tag
    })
}

/// `VERSION_ID` from /etc/os-release.
fn os_version() -> prism_core::Version {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("VERSION_ID="))
                .map(|v| prism_core::Version::parse(v.trim_matches('"')))
        })
        .unwrap_or_default()
}

fn device_info(window: &gtk::ApplicationWindow) -> prism_core::DeviceInfo {
    let mut info = prism_core::DeviceInfo::guess(prism_core::Platform::Linux, (0., 0.));
    info.os.version = os_version();
    if let Some(display) = gdk::Display::default()
        && let Some(monitor) = window
            .surface()
            .and_then(|s| display.monitor_at_surface(&s))
            .or_else(|| display.monitors().item(0).and_downcast::<gdk::Monitor>())
    {
        let g = monitor.geometry();
        let (w, h) = (g.width() as f32, g.height() as f32);
        let (mm_w, mm_h) = (monitor.width_mm() as f32, monitor.height_mm() as f32);
        let diagonal = ((mm_w * mm_w + mm_h * mm_h).sqrt() / 25.4 * 10.).round() / 10.;
        info.orientation = prism_core::Orientation::of(w, h);
        info.screen = prism_core::ScreenInfo {
            width: w,
            height: h,
            scale: monitor.scale_factor() as f32,
            diagonal: (diagonal > 1.).then_some(diagonal),
            refresh_rate: (monitor.refresh_rate() > 0)
                .then(|| monitor.refresh_rate() as f32 / 1000.),
            ..Default::default()
        };
    }
    info
}

/// Opens a window and runs the GTK main loop.
pub fn run(config: AppConfig, init: AppInit) {
    // App fonts, before GTK creates Pango's font map.
    let assets: Option<std::path::PathBuf> = std::env::var("PRISM_ASSETS_DIR")
        .ok()
        .or_else(|| config.assets.clone())
        .map(Into::into);
    text::register_fonts(assets.as_deref(), &config.embedded_assets);
    let _ = APP.set((config, init));
    // Installed packages set PRISM_APP_ID (the .desktop file's name), so
    // desktops match the window to its launcher and icon.
    let id = std::env::var("PRISM_APP_ID")
        .ok()
        .filter(|id| gtk::gio::Application::id_is_valid(id))
        .unwrap_or_else(|| "dev.prism.app".into());
    let app = gtk::Application::builder()
        .application_id(id)
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(activate);
    app.run_with_args::<&str>(&[]);
}

fn activate(app: &gtk::Application) {
    let (config, init) = APP.get().expect("prism-linux: run() was not called");
    let (width, height) = std::env::var("PRISM_WINDOW")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((config.width as i32, config.height as i32));
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(&config.title)
        .default_width(width)
        .default_height(height)
        .build();
    let root = layout::container();
    window.set_child(Some(&root));
    // With unsaved changes, closing asks first (#158).
    window.connect_close_request(|w| {
        if !UNSAVED.get() {
            return glib::Propagation::Proceed;
        }
        confirm_leave(w);
        glib::Propagation::Stop
    });

    let css = gtk::CssProvider::new();
    let base = gtk::CssProvider::new();
    #[allow(deprecated)]
    base.load_from_data(
        ".prism-header{padding:0 8px;border-bottom:1px solid alpha(currentColor,0.15);}\
         .prism-tabbar{padding:6px;border-top:1px solid alpha(currentColor,0.15);}\
         .prism-keys:focus-visible{outline:2px solid alpha(currentColor,0.55);outline-offset:2px;}\
         popover.prism-bare,popover.prism-bare>contents,window.prism-bare{background:none;border:none;border-radius:0;box-shadow:none;padding:0;}",
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &base,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }
    let dark = gtk::Settings::default().is_some_and(|s| s.is_gtk_application_prefer_dark_theme());
    SHAPE_DARK.with(|d| d.set(dark));

    let data = data_dir(&config.title);
    let services = crate::services::LinuxServices::new(app.clone(), window.clone(), data.clone());
    // Notification clicks and buttons: (id, action, link).
    let respond = gio::SimpleAction::new(
        "prism-notification",
        Some(glib::VariantTy::new("(sss)").expect("type")),
    );
    respond.connect_activate(|_, param| {
        let Some((id, action, link)) = param.and_then(|p| p.get::<(String, String, String)>())
        else {
            return;
        };
        let response = prism_core::NotificationResponse {
            id,
            action: (!action.is_empty()).then_some(action),
            text: None,
        };
        let link = (!link.is_empty()).then_some(link);
        glib::idle_add_local_once(move || {
            with_state(|s| {
                let patches = s.runtime.notification_response(response, link);
                commit(s, patches);
            })
        });
    });
    app.add_action(&respond);
    let runtime = Runtime::with_options(
        init,
        prism_core::RuntimeOptions {
            path: std::env::var("PRISM_PATH").unwrap_or_else(|_| "/".into()),
            storage: Some(Box::new(prism_core::FileStorage::new(data.clone()))),
            // Queued, not `invoke`d: on the UI thread `invoke` would run
            // `poll` right away, nested in the update that woke it (#57).
            wake_ui: Some(std::sync::Arc::new(|| {
                glib::idle_add_once(poll);
            })),
            capabilities: prism_core::PlatformServices::capabilities(&services),
            environment: environment(),
            data_dir: Some(data),
            viewport: Some((width as f32, height as f32)),
            device_info: Some(device_info(&window)),
            ..Default::default()
        },
    );
    let keys = Rc::new(KeyState::default());
    attach_keys(window.upcast_ref(), keys.clone());
    let renderer = NativeRenderer::new(Gtk {
        root: root.clone(),
        window: window.clone(),
        app_title: config.title.clone(),
        app: app.clone(),
        menus: Rc::new(RefCell::new(HashMap::new())),
        swipes: Rc::new(RefCell::new(HashMap::new())),
        scroll_events: Rc::new(RefCell::new(HashSet::new())),
        refreshable: Rc::new(RefCell::new(HashSet::new())),
        sizes: HashMap::new(),
        css,
        rules: HashMap::new(),
        css_dirty: false,
        clickable: Rc::new(RefCell::new(HashSet::new())),
        timers: HashMap::new(),
        assets: std::env::var("PRISM_ASSETS_DIR")
            .ok()
            .or_else(|| config.assets.clone())
            .map(Into::into),
        embedded: config.embedded_assets.clone(),
        dark,
        titles: HashMap::new(),
        icons: HashMap::new(),
        stacks: Vec::new(),
        tab_bars: Vec::new(),
        shapes: HashMap::new(),
        scenes: HashMap::new(),
        size: (width as f32, height as f32),
        rtl: false,
        locale: locale(),
        controls: HashMap::new(),
        date_pickers: HashMap::new(),
        clear_label: String::new(),
        statuses: HashMap::new(),
        bare: HashSet::new(),
        stickies: Rc::new(RefCell::new(HashMap::new())),
        labels: HashMap::new(),
        reports: Rc::default(),
        watched: HashSet::new(),
        cursors: HashMap::new(),
        pointer: Rc::new(RefCell::new(HashMap::new())),
        pointer_attached: HashSet::new(),
        drop_targets: HashMap::new(),
        keys: keys.clone(),
        layers: Vec::new(),
        inert: HashSet::new(),
        adaptive: HashMap::new(),
    });
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            runtime,
            renderer,
            services,
        })
    });
    layout::watch_root(&root, |w, h| {
        with_state(|s| {
            s.renderer.widgets.size = (w as f32, h as f32);
            UPDATING.set(true);
            relayout(s);
            UPDATING.set(false);
        })
    });
    with_state(|s| {
        let first = s.runtime.render();
        commit(s, first);
    });
    // Timers count down every second; background tasks run while the app
    // is open.
    glib::timeout_add_seconds_local(1, || {
        with_state(|s| {
            for (l, end) in s.renderer.widgets.timers.values() {
                l.set_text(&prism_core::format_remaining(*end));
            }
            let due = s.services.due_background();
            for id in &due {
                s.runtime.run_background_task(id);
            }
            if !due.is_empty() {
                let patches = s.runtime.poll();
                commit(s, patches);
            }
        });
        glib::ControlFlow::Continue
    });
    // Lifecycle: focused, unfocused, minimized.
    window.connect_is_active_notify(report_lifecycle);
    window.connect_realize(|w| {
        if let Some(surface) = w.surface() {
            let window = w.clone();
            surface.connect_notify_local(Some("state"), move |_, _| report_lifecycle(&window));
        }
    });
    // Refresh (pull to refresh elsewhere): F5 or Ctrl+R.
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(|_, key, _, modifiers| {
        let refresh = key == gdk::Key::F5
            || (modifiers.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::r);
        if !refresh {
            return glib::Propagation::Proceed;
        }
        let mut ids = Vec::new();
        with_state(|s| {
            let w = &s.renderer.widgets;
            ids = w.refreshable.borrow().iter().copied().collect();
        });
        for id in ids {
            dispatch(UiEvent::Click(id));
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);
    // The system's text size and animation settings.
    if let Some(settings) = gtk::Settings::default() {
        settings.connect_gtk_enable_animations_notify(|_| environment_changed());
        settings.connect_gtk_xft_dpi_notify(|_| environment_changed());
    }
    window.present();
    automation(window);
}

// ---------------------------------------------------------------------------
// Development hooks (like the macOS backend's)
// ---------------------------------------------------------------------------

/// `PRISM_CLICK="+1|Save"` presses buttons by label (or clickable views
/// containing that text, or opens the picker / date picker showing it);
/// `fill:Placeholder=text` types into the text input or text area with that
/// placeholder (or picks option `text` of the picker showing it;
/// `fill:@slider=40` drags the first slider), `submit:Placeholder` presses
/// Enter in it.
/// `PRISM_SCROLL=600` scrolls the visible scroll views.
/// `PRISM_SNAPSHOT=out.png` then saves the window (and open popovers) and
/// quits. `PRISM_DELAY` (seconds, default 1) before.
fn automation(window: gtk::ApplicationWindow) {
    let snapshot = std::env::var("PRISM_SNAPSHOT").ok();
    let clicks = std::env::var("PRISM_CLICK").ok();
    if snapshot.is_none() && clicks.is_none() {
        return;
    }
    let delay: f64 = std::env::var("PRISM_DELAY")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(1.);
    glib::timeout_add_local_once(std::time::Duration::from_secs_f64(delay), move || {
        for title in clicks
            .iter()
            .flat_map(|c| c.split('|'))
            .filter(|t| !t.is_empty())
        {
            // `menu:Title` opens the context menu of the view showing Title.
            let root: gtk::Widget = window.clone().upcast();
            let found = if let Some(t) = title.strip_prefix("menu:") {
                open_menu(&root, t)
            } else if let Some((placeholder, text)) =
                title.strip_prefix("fill:").and_then(|t| t.split_once('='))
            {
                fill(&root, placeholder, Some(&text.replace("\\n", "\n")))
            } else if let Some(placeholder) = title.strip_prefix("submit:") {
                fill(&root, placeholder, None)
            } else {
                press(&root, title)
            };
            if !found {
                eprintln!("prism-linux: PRISM_CLICK: nothing labelled {title:?}");
            }
        }
        // `PRISM_SCROLL=<y>` scrolls the visible vertical scroll views.
        if let Some(y) = std::env::var("PRISM_SCROLL")
            .ok()
            .and_then(|y| y.parse::<f64>().ok())
        {
            fn scroll_all(w: &gtk::Widget, y: f64) {
                if let Some(sw) = w.downcast_ref::<gtk::ScrolledWindow>()
                    && w.is_mapped()
                {
                    let adj = sw.vadjustment();
                    adj.set_value(y.min(adj.upper() - adj.page_size()));
                }
                let mut child = w.first_child();
                while let Some(c) = child {
                    scroll_all(&c, y);
                    child = c.next_sibling();
                }
            }
            scroll_all(&window.clone().upcast(), y);
        }
        let Some(path) = snapshot else { return };
        // `PRISM_AFTER` (seconds, default 0.7): time for async work first.
        let after: f64 = std::env::var("PRISM_AFTER")
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(0.7);
        glib::timeout_add_local_once(std::time::Duration::from_secs_f64(after), move || {
            save_png(&window.clone().upcast(), &path);
            // Dialogs (alerts, modals) and open menus: out-2.png, out-3.png…
            let mut n = 2;
            let mut extra = |w: &gtk::Widget| {
                save_png(w, &path.replace(".png", &format!("-{n}.png")));
                n += 1;
            };
            for w in gtk::Window::list_toplevels() {
                if w != window.clone().upcast::<gtk::Widget>() && w.is_visible() {
                    extra(&w);
                }
            }
            // Context menus, pickers' and date pickers' popovers.
            fn popovers(w: &gtk::Widget, out: &mut Vec<gtk::Widget>) {
                if w.is::<gtk::Popover>() && w.is_visible() {
                    out.push(w.clone());
                }
                let mut child = w.first_child();
                while let Some(c) = child {
                    popovers(&c, out);
                    child = c.next_sibling();
                }
            }
            let mut open = Vec::new();
            popovers(&window.clone().upcast(), &mut open);
            for p in &open {
                extra(p);
            }
            if let Some(app) = window.application() {
                app.quit();
            }
        });
    });
}

impl Gtk {
    /// A text is focusable when it is clickable or has links.
    fn update_text_focus(&self, id: NodeId, l: &gtk::Label) {
        let links = self.labels.get(&id).is_some_and(|s| {
            prism_core::decode_runs(&s.config)
                .iter()
                .any(|r| r.is_link())
        });
        l.set_focusable(links || self.clickable.borrow().contains(&id));
    }
}

fn label_of(w: &gtk::Widget) -> Option<String> {
    w.downcast_ref::<gtk::Label>()
        .map(|l| l.text().to_string())
        .or_else(|| {
            w.downcast_ref::<gtk::Button>()
                .and_then(|b| b.label())
                .map(|l| l.to_string())
        })
}

fn press(w: &gtk::Widget, title: &str) -> bool {
    if !w.is_visible() {
        return false;
    }
    if let Some(b) = w.downcast_ref::<gtk::Button>()
        && b.label().is_some_and(|l| l == title)
    {
        b.emit_clicked();
        return true;
    }
    // A date picker or a picker showing `title`: open it.
    if let Some(m) = w.downcast_ref::<gtk::MenuButton>()
        && shows(w, title)
    {
        m.popup();
        return true;
    }
    if w.is::<gtk::DropDown>() && shows(w, title) {
        w.activate();
        return true;
    }
    // A clickable container showing `title` somewhere inside.
    fn shows(w: &gtk::Widget, title: &str) -> bool {
        let mut child = w.first_child();
        while let Some(c) = child {
            if (label_of(&c).is_some_and(|l| l == title) && !c.is::<gtk::Button>())
                || shows(&c, title)
            {
                return true;
            }
            child = c.next_sibling();
        }
        false
    }
    // An inline link reading `title` (rich text): activated like a click on it.
    if let Some(l) = w.downcast_ref::<gtk::Label>()
        && let Some(id) = w
            .css_classes()
            .iter()
            .find_map(|c| c.strip_prefix('p').and_then(|n| n.parse::<NodeId>().ok()))
        && let Some(item) = STATE.with(|s| {
            s.borrow().as_ref().and_then(|s| {
                let state = s.renderer.widgets.labels.get(&id)?;
                prism_core::decode_runs(&state.config)
                    .into_iter()
                    .find(|r| r.text == title)
                    .and_then(|r| r.link)
            })
        })
    {
        let uri = format!("{}{item}", text::LINK_SCHEME);
        l.emit_by_name::<bool>("activate-link", &[&uri]);
        return true;
    }
    // A clickable container showing `title`, or a clickable text reading it.
    let has_label =
        shows(w, title) || (w.is::<gtk::Label>() && label_of(w).is_some_and(|l| l == title));
    if has_label
        && let Some(id) = w
            .css_classes()
            .iter()
            .find_map(|c| c.strip_prefix('p').and_then(|n| n.parse::<NodeId>().ok()))
        && STATE.with(|s| {
            s.borrow()
                .as_ref()
                .is_some_and(|s| s.renderer.widgets.clickable.borrow().contains(&id))
        })
    {
        dispatch(UiEvent::Click(id));
        return true;
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        if press(&c, title) {
            return true;
        }
        child = c.next_sibling();
    }
    false
}

/// Types `text` into the entry or text area whose placeholder is
/// `placeholder` (`None`: presses Enter in the entry).
fn fill(w: &gtk::Widget, placeholder: &str, text: Option<&str>) -> bool {
    if let Some(e) = w.downcast_ref::<gtk::Entry>()
        && e.placeholder_text().is_some_and(|p| p == placeholder)
    {
        match text {
            Some(t) => e.set_text(t),
            None => e.emit_by_name::<()>("activate", &[]),
        }
        return true;
    }
    // `@slider`: drags the first slider to `text`.
    if placeholder == "@slider"
        && let Some(scale) = w.downcast_ref::<gtk::Scale>()
        && let Some(v) = text.and_then(|t| t.parse::<f64>().ok())
    {
        scale.emit_by_name::<bool>("change-value", &[&gtk::ScrollType::Jump, &v]);
        return true;
    }
    // A picker showing `placeholder` (or an option): selects option `text`.
    if let Some(d) = w.downcast_ref::<gtk::DropDown>()
        && let Some(i) = text.and_then(|t| t.parse::<u32>().ok())
        && label_shown(w, placeholder)
    {
        d.set_selected(i);
        return true;
    }
    // A text area: an overlay with the text view and its placeholder label.
    if let Some(o) = w.downcast_ref::<gtk::Overlay>()
        && let Some(view) = o
            .child()
            .and_then(|s| s.downcast::<gtk::ScrolledWindow>().ok())
            .and_then(|s| s.child())
            .and_then(|v| v.downcast::<gtk::TextView>().ok())
        && o.last_child()
            .and_then(|l| l.downcast::<gtk::Label>().ok())
            .is_some_and(|l| l.text() == placeholder)
    {
        if let Some(t) = text {
            view.buffer().set_text(t);
        }
        return true;
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        if fill(&c, placeholder, text) {
            return true;
        }
        child = c.next_sibling();
    }
    false
}

/// Whether a visible label inside `w` reads `text`.
fn label_shown(w: &gtk::Widget, text: &str) -> bool {
    if !w.is_visible() {
        return false;
    }
    if w.downcast_ref::<gtk::Label>()
        .is_some_and(|l| l.text() == text)
    {
        return true;
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        if label_shown(&c, text) {
            return true;
        }
        child = c.next_sibling();
    }
    false
}

/// Opens the context menu of the view showing `title`.
fn open_menu(w: &gtk::Widget, title: &str) -> bool {
    let mut child = w.first_child();
    while let Some(c) = child {
        if open_menu(&c, title) {
            return true;
        }
        child = c.next_sibling();
    }
    let Some(id) = w
        .css_classes()
        .iter()
        .find_map(|c| c.strip_prefix('p').and_then(|n| n.parse::<NodeId>().ok()))
    else {
        return false;
    };
    fn shows(w: &gtk::Widget, title: &str) -> bool {
        if label_of(w).is_some_and(|l| l == title) {
            return true;
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            if shows(&c, title) {
                return true;
            }
            child = c.next_sibling();
        }
        false
    }
    if !shows(w, title) {
        return false;
    }
    let mut opened = false;
    with_state(|s| {
        if let Some(p) = s.renderer.widgets.menus.borrow().get(&id) {
            p.popup();
            opened = true;
        }
    });
    opened
}

fn save_png(window: &gtk::Widget, path: &str) {
    let (w, h) = (window.width() as f64, window.height() as f64);
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, w, h);
    let Some(node) = snapshot.to_node() else {
        eprintln!("prism-linux: snapshot: nothing drawn");
        return;
    };
    let Some(renderer) = window.native().and_then(|n| n.renderer()) else {
        eprintln!("prism-linux: snapshot: no renderer");
        return;
    };
    let texture = renderer.render_texture(&node, None);
    match texture.save_to_png(path) {
        Ok(()) => eprintln!("prism-linux: snapshot written to {path}"),
        Err(e) => eprintln!("prism-linux: snapshot failed: {e}"),
    }
}

// --- Remote pictures (#32) ----------------------------------------------------------------

/// Downloads `src` (a URL, maybe with header lines) off the main thread and
/// shows it in `p`, unless `p` was given another source meanwhile.
fn load_remote_picture(p: &gtk::Picture, src: &str) {
    p.set_paintable(None::<&gdk::Paintable>);
    // SAFETY: the key only ever holds a `String`.
    unsafe { p.set_data("prism-src", src.to_string()) };
    let (picture, src) = (p.clone(), src.to_string());
    glib::spawn_future_local(async move {
        let source = src.clone();
        let bytes =
            gio::spawn_blocking(move || prism_native::images::load(&source, &Default::default()))
                .await
                .ok()
                .flatten();
        // SAFETY: as above, a `String`.
        let current = unsafe {
            picture
                .data::<String>("prism-src")
                .map(|s| s.as_ref().clone())
        };
        if current.as_deref() != Some(src.as_str()) {
            return;
        }
        let texture =
            bytes.and_then(|b| gdk::Texture::from_bytes(&glib::Bytes::from_owned(b)).ok());
        match texture {
            Some(t) => picture.set_paintable(Some(&t)),
            None => eprintln!(
                "prism-linux: could not load image {:?}",
                prism_core::picture_source(&src).0
            ),
        }
    });
}
