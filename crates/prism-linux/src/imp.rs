//! GTK widgets for Prism nodes, the window and the event loop.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::OnceLock;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use prism_core::{
    Accessibility, AppConfig, AppInit, Appearance, Backend, Color, FontWeight, NodeId, NodeKind,
    Runtime, Style, TextAlign, UiEvent,
};
use prism_native::{Insets, NativeRenderer, Viewport, Widgets};

use crate::controls;
use crate::layout;

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
    Slider(
        NodeId,
        gtk::Scale,
        Rc<Cell<prism_core::controls::SliderConfig>>,
    ),
    Picker(NodeId, Rc<controls::Picker>),
    DatePicker(NodeId, Rc<controls::DatePicker>),
    TextArea(NodeId, Rc<controls::TextArea>),
    Spinner(NodeId, gtk::Spinner),
}

thread_local! {
    /// Whether shapes draw the dark variants of their colors.
    static SHAPE_DARK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
            | View::Slider(id, ..)
            | View::Picker(id, _)
            | View::DatePicker(id, _)
            | View::TextArea(id, _)
            | View::Spinner(id, _) => *id,
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
            View::Slider(_, w, _) => w.clone().upcast(),
            View::Picker(_, p) => p.dropdown.clone().upcast(),
            View::DatePicker(_, p) => p.button.clone().upcast(),
            View::TextArea(_, t) => t.root.clone().upcast(),
            View::Spinner(_, w) => w.clone().upcast(),
        }
    }
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
        prism_core::PlatformServices::handle(&mut s.services, request);
    }
    if let Some(at) = s.runtime.next_timer() {
        let delay = at.saturating_duration_since(std::time::Instant::now());
        glib::timeout_add_local_once(delay, poll);
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
    s.renderer.layout(w, h, Insets::default());
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
    dark: bool,
    /// Titles of screens and tabs, icons of tabs.
    titles: HashMap<NodeId, String>,
    icons: HashMap<NodeId, String>,
    stacks: Vec<Rc<NavStack>>,
    tab_bars: Vec<Rc<TabBar>>,
    /// Shapes, redrawn when the appearance changes.
    shapes: HashMap<NodeId, gtk::DrawingArea>,
    /// Window content size.
    size: (f32, f32),
    /// Right-to-left layout and the app's locale (form controls follow them).
    rtl: bool,
    locale: prism_core::Locale,
    /// Form controls, mirrored in right-to-left.
    controls: HashMap<NodeId, gtk::Widget>,
    date_pickers: HashMap<NodeId, Rc<controls::DatePicker>>,
    /// Status regions and what they last announced.
    statuses: HashMap<NodeId, String>,
}

fn css_color(c: Color, dark: bool) -> String {
    let c = if dark {
        c.dark_variant()
    } else {
        c.light_variant()
    };
    let v = |x: f32| (x.clamp(0., 1.) * 255.).round() as u8;
    format!("rgba({},{},{},{})", v(c.r), v(c.g), v(c.b), c.a)
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
    fn flush_css(&mut self) {
        if !self.css_dirty {
            return;
        }
        self.css_dirty = false;
        let sheet: String = self.rules.values().map(String::as_str).collect();
        #[allow(deprecated)]
        self.css.load_from_data(&sheet);
    }

    fn header_back(&self, stack: &Rc<NavStack>) {
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
        let open_right = open.clone();
        right.connect_pressed(move |g, _, x, y| {
            if open_right(x, y) {
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
                gesture.connect_released(move |g, _, _, _| {
                    if clickable.borrow().contains(&id) {
                        g.set_state(gtk::EventSequenceState::Claimed);
                        dispatch(UiEvent::Click(id));
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
                View::Text(id, l)
            }
            NodeKind::Button => {
                let b = gtk::Button::new();
                b.connect_clicked(move |_| dispatch(UiEvent::Click(id)));
                View::Button(id, b)
            }
            NodeKind::TextInput => {
                let e = gtk::Entry::new();
                e.connect_changed(move |e| dispatch(UiEvent::Input(id, e.text().to_string())));
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
            NodeKind::Scroll => {
                let content = layout::container();
                let s = gtk::ScrolledWindow::new();
                s.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
                s.set_child(Some(&content));
                let events = self.scroll_events.clone();
                let adj = s.vadjustment();
                adj.connect_value_changed(move |a| {
                    if events.borrow().contains(&id) {
                        dispatch(UiEvent::Scroll(id, a.value() as f32, a.page_size() as f32));
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
                        dispatch(UiEvent::Back(stack.id(), 0));
                    }
                    glib::Propagation::Stop
                });
                View::Sheet(id, sheet)
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
        ) {
            controls::set_direction_deep(&w, self.rtl);
            self.controls.insert(id, w);
        }
        view
    }

    fn set_style(&mut self, view: &View, s: &Style) {
        let id = view.id();
        let mut css = String::new();
        if let Some(bg) = s.background {
            css += &format!("background-color:{};", css_color(bg, self.dark));
        }
        if s.corner_radius > 0. {
            css += &format!("border-radius:{}px;", s.corner_radius);
        }
        if s.border_width > 0. {
            let c = s
                .border_color
                .map_or("currentColor".into(), |c| css_color(c, self.dark));
            css += &format!("border:{}px solid {c};", s.border_width);
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
            let drawn = s.background.is_some() || s.border_width > 0.;
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
            match s.font_weight {
                FontWeight::Regular => 400,
                FontWeight::Medium => 500,
                FontWeight::Semibold => 600,
                FontWeight::Bold => 700,
            }
        );
        self.rules.insert(id, format!(".p{id}{{{css}}}\n{extra}"));
        self.css_dirty = true;
        let w = view.widget();
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
                view.widget().set_overflow(if s.corner_radius > 0. {
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
            View::Text(_, l) => l.set_text(text),
            View::Button(_, b) => b.set_label(text),
            View::Input(_, e) => {
                if e.text() != text && !e.has_focus() {
                    e.set_text(text);
                }
            }
            View::TextArea(_, t) => t.set_text(text),
            View::Picture(_, p) => {
                let path = if text.starts_with('/') {
                    Some(std::path::PathBuf::from(text))
                } else {
                    self.assets.as_ref().map(|a| a.join(text))
                };
                if let Some(path) = path.filter(|p| p.exists()) {
                    p.set_filename(Some(&path));
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
                c.set_cursor_from_name(Some("pointer"));
            } else {
                self.clickable.borrow_mut().remove(id);
                c.set_cursor_from_name(None);
            }
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
            c.unparent();
        }
        view.widget().set_parent(&self.root);
    }

    fn set_drawing(&mut self, view: &View, paths: &[prism_core::Path]) {
        if let View::Shape(_, area, p) = view {
            *p.borrow_mut() = paths.to_vec();
            area.queue_draw();
        }
    }

    fn destroy(&mut self, view: &View) {
        let id = view.id();
        self.shapes.remove(&id);
        self.controls.remove(&id);
        self.date_pickers.remove(&id);
        self.statuses.remove(&id);
        self.rules.remove(&id);
        self.css_dirty = true;
        self.clickable.borrow_mut().remove(&id);
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
        let w = view.widget();
        layout::forget(&w);
        if w.parent().is_some() {
            w.unparent();
        }
    }

    fn measure(&mut self, view: &View, max_width: Option<f32>) -> (f32, f32) {
        let w = view.widget();
        let (_, nat_w, _, _) = w.measure(gtk::Orientation::Horizontal, -1);
        let width = match (view, max_width) {
            (View::Text(..), Some(m)) => (nat_w as f32).min(m),
            (View::Input(..) | View::Progress(..), Some(m)) => m,
            // GtkScale's natural width is its knob: sliders take 200 px.
            (View::Slider(..), m) => m.map_or(200., |m| m.min(200.)),
            // As wide as offered; as tall as its text, at least 3 lines.
            (View::TextArea(_, t), m) => {
                let w = m.unwrap_or(240.).ceil();
                return (w, t.measure_height(w).ceil());
            }
            _ => nat_w as f32,
        };
        let (_, nat_h, _, _) = w.measure(gtk::Orientation::Vertical, width.ceil() as i32);
        (width.ceil(), nat_h as f32)
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
                layout::repeat(&w, rest, layout::Visual { opacity, transform }, t);
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
            View::Slider(_, s, c) => s.set_value(c.get().snap(value)),
            View::Picker(_, p) => p.select(value),
            View::DatePicker(_, p) => p.set_value(value),
            View::Spinner(_, s) => controls::spinner_size(s, value >= 1.),
            _ => {}
        }
    }

    fn set_config(&mut self, view: &View, _kind: NodeKind, config: &str) {
        match view {
            View::Slider(_, s, c) => {
                let cfg = prism_core::controls::SliderConfig::decode(config);
                c.set(cfg);
                controls::configure_slider(s, cfg);
            }
            View::Picker(_, p) => p.set_options(config),
            View::DatePicker(_, p) => p.set_config(config),
            View::Input(_, e) => controls::configure_entry(e, config),
            View::TextArea(_, t) => controls::configure_text_view(&t.view, config),
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
    }

    fn set_icon(&mut self, view: &View, icon: &str) {
        match view {
            View::Image(_, l) => l.set_text(prism_core::glyphs::glyph(icon)),
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

    fn set_content_size(&mut self, view: &View, width: f32, height: f32) {
        if let View::Scroll(_, _, content) = view {
            content.set_size_request(width.ceil() as i32, height.ceil() as i32);
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
        self.dark = dark;
        SHAPE_DARK.with(|d| d.set(dark));
        for area in self.shapes.values() {
            area.queue_draw();
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
    let _ = APP.set((config, init));
    let app = gtk::Application::builder()
        .application_id("dev.prism.app")
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

    let css = gtk::CssProvider::new();
    let base = gtk::CssProvider::new();
    #[allow(deprecated)]
    base.load_from_data(
        ".prism-header{padding:0 8px;border-bottom:1px solid alpha(currentColor,0.15);}\
         .prism-tabbar{padding:6px;border-top:1px solid alpha(currentColor,0.15);}",
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
    let services = crate::services::LinuxServices::new(app.clone(), window.clone());
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
            wake_ui: Some(std::sync::Arc::new(|| {
                glib::MainContext::default().invoke(poll);
            })),
            capabilities: prism_core::PlatformServices::capabilities(&services),
            environment: environment(),
            data_dir: Some(data),
            viewport: Some((width as f32, height as f32)),
            device_info: Some(device_info(&window)),
            ..Default::default()
        },
    );
    let renderer = NativeRenderer::new(Gtk {
        root: root.clone(),
        window: window.clone(),
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
        dark,
        titles: HashMap::new(),
        icons: HashMap::new(),
        stacks: Vec::new(),
        tab_bars: Vec::new(),
        shapes: HashMap::new(),
        size: (width as f32, height as f32),
        rtl: false,
        locale: locale(),
        controls: HashMap::new(),
        date_pickers: HashMap::new(),
        statuses: HashMap::new(),
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
    let has_label = shows(w, title);
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
