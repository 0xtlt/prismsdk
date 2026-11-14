//! GTK widgets for Prism nodes, the window and the event loop.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::OnceLock;

use gtk::prelude::*;
use gtk::{gdk, glib};
use prism_core::{
    Accessibility, AppConfig, AppInit, Appearance, Backend, Color, FontWeight, NodeId, NodeKind,
    Runtime, Style, TextAlign, UiEvent,
};
use prism_native::{Insets, NativeRenderer, Viewport, Widgets};

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
            | View::Sheet(id, _) => *id,
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
        }
    }
}

// ---------------------------------------------------------------------------
// Main-thread state
// ---------------------------------------------------------------------------

struct State {
    runtime: Runtime,
    renderer: NativeRenderer<Gtk>,
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
fn dispatch(event: UiEvent) {
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
    let _ = s.runtime.take_requests();
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
    /// Window content size.
    size: (f32, f32),
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
                View::Input(id, e)
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
            NodeKind::Scroll => {
                let content = layout::container();
                let s = gtk::ScrolledWindow::new();
                s.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
                s.set_child(Some(&content));
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
        if s.opacity < 1. {
            css += &format!("opacity:{};", s.opacity);
        }
        if let Some(c) = s.text_color {
            css += &format!("color:{};", css_color(c, self.dark));
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
        self.rules.insert(id, format!(".p{id}{{{css}}}\n"));
        self.css_dirty = true;
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
            View::Container(..) => view.widget().set_overflow(if s.corner_radius > 0. {
                gtk::Overflow::Hidden
            } else {
                gtk::Overflow::Visible
            }),
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
        if let View::Input(_, e) = view {
            e.set_placeholder_text(Some(text));
        }
    }

    fn set_clickable(&mut self, view: &View, clickable: bool) {
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
            if !keep.contains(&c) {
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

    fn destroy(&mut self, view: &View) {
        let id = view.id();
        self.rules.remove(&id);
        self.css_dirty = true;
        self.clickable.borrow_mut().remove(&id);
        self.timers.remove(&id);
        self.titles.remove(&id);
        self.icons.remove(&id);
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
        let (w, h) = (
            container.widget().width() as f32,
            container.widget().height() as f32,
        );
        let (w, h) = if w > 0. { (w, h) } else { self.size };
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
        if let Some(label) = &a11y.label {
            set_accessible_label(&view.widget(), label);
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
            _ => {}
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
    }
}

/// `GTK_ACCESSIBLE_PROPERTY_LABEL` (gtkenums.h: the fifth value, from 0).
const ACCESSIBLE_PROPERTY_LABEL: i32 = 4;

unsafe extern "C" {
    /// gtkaccessible.h, `GDK_AVAILABLE_IN_ALL` (gtk-rs only binds it from 4.10).
    fn gtk_accessible_update_property_value(
        accessible: *mut std::ffi::c_void,
        n_properties: std::ffi::c_int,
        properties: *const i32,
        values: *const gtk::glib::gobject_ffi::GValue,
    );
}

/// What screen readers say for `w`.
fn set_accessible_label(w: &gtk::Widget, label: &str) {
    use gtk::glib::translate::{ToGlibPtr, ToGlibPtrMut};
    let mut value = label.to_value();
    let widget: *mut gtk::ffi::GtkWidget = w.to_glib_none().0;
    // SAFETY: every GtkWidget implements GtkAccessible; one string property.
    unsafe {
        gtk_accessible_update_property_value(
            widget.cast(),
            1,
            &ACCESSIBLE_PROPERTY_LABEL,
            value.to_glib_none_mut().0,
        );
    }
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

    let data = data_dir(&config.title);
    let runtime = Runtime::with_options(
        init,
        prism_core::RuntimeOptions {
            path: std::env::var("PRISM_PATH").unwrap_or_else(|_| "/".into()),
            storage: Some(Box::new(prism_core::FileStorage::new(data.clone()))),
            wake_ui: Some(std::sync::Arc::new(|| {
                glib::MainContext::default().invoke(poll);
            })),
            environment: prism_core::Environment {
                locale: locale(),
                ..Default::default()
            },
            data_dir: Some(data),
            viewport: Some((width as f32, height as f32)),
            device_info: Some(device_info(&window)),
            ..Default::default()
        },
    );
    let renderer = NativeRenderer::new(Gtk {
        root: root.clone(),
        window: window.clone(),
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
        size: (width as f32, height as f32),
    });
    STATE.with(|s| *s.borrow_mut() = Some(State { runtime, renderer }));
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
    // Timers count down every second.
    glib::timeout_add_seconds_local(1, || {
        with_state(|s| {
            for (l, end) in s.renderer.widgets.timers.values() {
                l.set_text(&prism_core::format_remaining(*end));
            }
        });
        glib::ControlFlow::Continue
    });
    window.present();
    automation(window);
}

// ---------------------------------------------------------------------------
// Development hooks (like the macOS backend's)
// ---------------------------------------------------------------------------

/// `PRISM_CLICK="+1|Save"` presses buttons by label (or clickable views
/// containing that text); `PRISM_SNAPSHOT=out.png` then saves the window and
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
            if !press(&window.clone().upcast(), title) {
                eprintln!("prism-linux: PRISM_CLICK: nothing labelled {title:?}");
            }
        }
        let Some(path) = snapshot else { return };
        glib::timeout_add_local_once(std::time::Duration::from_millis(700), move || {
            save_png(&window, &path);
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

fn save_png(window: &gtk::ApplicationWindow, path: &str) {
    let (w, h) = (window.width() as f64, window.height() as f64);
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, w, h);
    let Some(node) = snapshot.to_node() else {
        eprintln!("prism-linux: snapshot: nothing drawn");
        return;
    };
    let Some(renderer) = window.renderer() else {
        eprintln!("prism-linux: snapshot: no renderer");
        return;
    };
    let texture = renderer.render_texture(&node, None);
    match texture.save_to_png(path) {
        Ok(()) => eprintln!("prism-linux: snapshot written to {path}"),
        Err(e) => eprintln!("prism-linux: snapshot failed: {e}"),
    }
}
