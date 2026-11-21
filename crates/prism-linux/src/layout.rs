//! A GTK layout manager that allocates each child exactly the frame Prism's
//! layout gave it (`set_frame`), instead of GTK's own sizing.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use prism_core::{Transform, Transition};

type ResizeHandler = Box<dyn Fn(i32, i32)>;

thread_local! {
    /// Child widget (pointer) → x, y, width, height.
    static FRAMES: RefCell<HashMap<usize, (i32, i32, i32, i32)>> = RefCell::new(HashMap::new());
    /// Called when the root container gets a new size.
    static ON_ROOT_RESIZE: RefCell<Option<ResizeHandler>> = const { RefCell::new(None) };
    static ROOT: RefCell<Option<(usize, i32, i32)>> = const { RefCell::new(None) };
    /// Child widget → its visual transform (offset, scale, rotation).
    static TRANSFORMS: RefCell<HashMap<usize, Transform>> = RefCell::new(HashMap::new());
    /// Running animations.
    static TICKS: RefCell<HashMap<(usize, Channel), gtk::TickCallbackId>> = RefCell::new(HashMap::new());
    /// Sticky children → how far they are pushed down (their scroll view scrolled).
    static STICKY: RefCell<HashMap<usize, i32>> = RefCell::new(HashMap::new());
}

fn key(w: &impl IsA<gtk::Widget>) -> usize {
    w.as_ref().as_ptr() as usize
}

pub fn set_frame(w: &impl IsA<gtk::Widget>, frame: (i32, i32, i32, i32)) {
    FRAMES.with(|f| f.borrow_mut().insert(key(w), frame));
    if let Some(parent) = w.as_ref().parent() {
        parent.queue_allocate();
    }
}

pub fn forget(w: &impl IsA<gtk::Widget>) {
    FRAMES.with(|f| f.borrow_mut().remove(&key(w)));
    STICKY.with(|s| s.borrow_mut().remove(&key(w)));
    TRANSFORMS.with(|f| f.borrow_mut().remove(&key(w)));
    RESTARTS.with(|r| r.borrow_mut().remove(&key(w)));
    for channel in [Channel::Frame, Channel::Visual, Channel::Loop] {
        stop(w, channel);
    }
}

/// A sticky child is drawn `offset` points below its frame (`None`: it is
/// not sticky anymore), above its siblings.
pub fn set_sticky_offset(w: &impl IsA<gtk::Widget>, offset: Option<f32>) {
    let w = w.as_ref();
    let changed = STICKY.with(|s| {
        let mut s = s.borrow_mut();
        let old = s.get(&key(w)).copied();
        let new = offset.map(|o| o.round() as i32);
        match new {
            Some(o) => s.insert(key(w), o),
            None => s.remove(&key(w)),
        };
        old != new
    });
    if offset.is_some() {
        raise(w);
    }
    if changed && let Some(parent) = w.parent() {
        parent.queue_allocate();
    }
}

/// Whether `w` is a sticky child.
pub fn is_sticky(w: &impl IsA<gtk::Widget>) -> bool {
    STICKY.with(|s| s.borrow().contains_key(&key(w)))
}

/// Moves `w` after its siblings: GTK draws children in order.
pub fn raise(w: &impl IsA<gtk::Widget>) {
    let w = w.as_ref();
    if let Some(parent) = w.parent()
        && parent.last_child().as_ref() != Some(w)
    {
        w.insert_before(&parent, None::<&gtk::Widget>);
    }
}

pub fn frame_of(w: &impl IsA<gtk::Widget>) -> Option<(i32, i32, i32, i32)> {
    FRAMES.with(|f| f.borrow().get(&key(w)).copied())
}

// --- Motion: frame-by-frame animations on GTK's frame clock -----------------

/// What a widget shows of its style: opacity and transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Visual {
    pub opacity: f32,
    pub transform: Transform,
}

impl Visual {
    fn lerp(a: Visual, b: Visual, t: f32) -> Visual {
        let l = |x: f32, y: f32| x + (y - x) * t;
        Visual {
            opacity: l(a.opacity, b.opacity).clamp(0., 1.),
            transform: a.transform.lerp(b.transform, t),
        }
    }
}

/// A widget's animations: one per channel at a time.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    Frame,
    Visual,
    Loop,
}

/// What a widget shows now.
pub fn visual(w: &impl IsA<gtk::Widget>) -> Visual {
    Visual {
        opacity: w.as_ref().opacity() as f32,
        transform: TRANSFORMS.with(|t| t.borrow().get(&key(w)).copied().unwrap_or_default()),
    }
}

/// Shows `v` at once.
pub fn set_visual(w: &impl IsA<gtk::Widget>, v: Visual) {
    let w = w.as_ref();
    w.set_opacity(v.opacity as f64);
    let changed = TRANSFORMS.with(|t| {
        let mut t = t.borrow_mut();
        let old = t.get(&key(w)).copied().unwrap_or_default();
        if v.transform.is_identity() {
            t.remove(&key(w));
        } else {
            t.insert(key(w), v.transform);
        }
        old != v.transform
    });
    if changed && let Some(parent) = w.parent() {
        parent.queue_allocate();
    }
}

pub fn stop(w: &impl IsA<gtk::Widget>, channel: Channel) {
    if let Some(id) = TICKS.with(|t| t.borrow_mut().remove(&(key(w), channel))) {
        id.remove();
    }
}

/// Calls `step` with eased progress on every frame for `t` (then 1, then
/// `done`); `forever` swings back and forth until stopped.
fn run(
    w: &gtk::Widget,
    channel: Channel,
    t: Transition,
    forever: bool,
    step: impl Fn(&gtk::Widget, f32) + 'static,
    done: impl Fn() + 'static,
) {
    stop(w, channel);
    let start = std::cell::Cell::new(None::<i64>);
    let (delay, duration) = ((t.delay * 1e6) as i64, (t.duration.max(0.001) * 1e6) as i64);
    let k = (key(w), channel);
    let id = w.add_tick_callback(move |w, clock| {
        let now = clock.frame_time();
        // The first frame starts the clock (`Cell::get` copies: store it back).
        let first = start.get().unwrap_or(now);
        start.set(Some(first));
        let begin = first + delay;
        if now < begin {
            return glib::ControlFlow::Continue;
        }
        let elapsed = now - begin;
        if forever && RESTARTS.with(|r| r.borrow().contains(&k.0)) {
            // Starts over at the end of every cycle (spinners).
            step(w, t.ease_at((elapsed % duration) as f32 / duration as f32));
            return glib::ControlFlow::Continue;
        }
        if forever {
            let cycle = elapsed % (2 * duration);
            let p = if cycle < duration {
                cycle
            } else {
                2 * duration - cycle
            };
            step(w, t.ease_at(p as f32 / duration as f32));
            return glib::ControlFlow::Continue;
        }
        if elapsed >= duration {
            step(w, 1.);
            TICKS.with(|t| t.borrow_mut().remove(&k));
            done();
            return glib::ControlFlow::Break;
        }
        step(w, t.ease_at(elapsed as f32 / duration as f32));
        glib::ControlFlow::Continue
    });
    TICKS.with(|t| t.borrow_mut().insert(k, id));
}

/// Animates from what is shown to `to`.
pub fn animate_visual(
    w: &impl IsA<gtk::Widget>,
    to: Visual,
    t: Transition,
    done: impl Fn() + 'static,
) {
    let from = visual(w);
    run(
        w.as_ref(),
        Channel::Visual,
        t,
        false,
        move |w, p| set_visual(w, Visual::lerp(from, to, p)),
        done,
    );
}

/// Moves and resizes from the current frame to `to`.
pub fn animate_frame(w: &impl IsA<gtk::Widget>, to: (i32, i32, i32, i32), t: Transition) {
    let Some(from) = frame_of(w) else {
        set_frame(w, to);
        return;
    };
    let l = |a: i32, b: i32, p: f32| a + ((b - a) as f32 * p).round() as i32;
    run(
        w.as_ref(),
        Channel::Frame,
        t,
        false,
        move |w, p| {
            set_frame(
                w,
                (
                    l(from.0, to.0, p),
                    l(from.1, to.1, p),
                    l(from.2, to.2, p),
                    l(from.3, to.3, p),
                ),
            )
        },
        || {},
    );
}

thread_local! {
    /// Widgets whose loop starts over instead of swinging back.
    static RESTARTS: RefCell<std::collections::HashSet<usize>> = RefCell::default();
}

/// From `a` to `b` and starting over, until stopped (a spinner: rotation
/// 0° to 360°, every degree in between).
pub fn repeat_from_start(w: &impl IsA<gtk::Widget>, a: Visual, b: Visual, t: Transition) {
    let t = match t.curve {
        prism_core::Curve::Spring => Transition {
            curve: prism_core::Curve::EaseInOut,
            ..t
        },
        _ => t,
    };
    let k = key(w);
    RESTARTS.with(|r| r.borrow_mut().insert(k));
    run(
        w.as_ref(),
        Channel::Loop,
        t,
        true,
        move |w, p| set_visual(w, Visual::lerp(a, b, p)),
        || {},
    );
}

/// Back and forth between `a` and `b` until stopped.
pub fn repeat(w: &impl IsA<gtk::Widget>, a: Visual, b: Visual, t: Transition) {
    RESTARTS.with(|r| r.borrow_mut().remove(&key(w)));
    // Loops ease in and out (a spring would overshoot every swing).
    let t = Transition {
        curve: prism_core::Curve::EaseInOut,
        ..t
    };
    run(
        w.as_ref(),
        Channel::Loop,
        t,
        true,
        move |w, p| set_visual(w, Visual::lerp(a, b, p)),
        || {},
    );
}

/// The allocation transform of a `w` × `h` child at `x`, `y`.
fn child_transform(x: i32, y: i32, w: i32, h: i32, t: Transform) -> gtk::gsk::Transform {
    use gtk::graphene::Point;
    let (cx, cy) = (w as f32 / 2., h as f32 / 2.);
    gtk::gsk::Transform::new()
        .translate(&Point::new(x as f32 + cx + t.x, y as f32 + cy + t.y))
        .rotate(t.rotate)
        .scale(t.sx(), t.sy())
        .translate(&Point::new(-cx, -cy))
}

/// `root` reports its size to `f` whenever GTK allocates it.
pub fn watch_root(root: &impl IsA<gtk::Widget>, f: impl Fn(i32, i32) + 'static) {
    ROOT.with(|r| *r.borrow_mut() = Some((key(root), 0, 0)));
    ON_ROOT_RESIZE.with(|cb| *cb.borrow_mut() = Some(Box::new(f)));
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PrismLayout;

    #[glib::object_subclass]
    impl ObjectSubclass for PrismLayout {
        const NAME: &'static str = "PrismLayout";
        type Type = super::PrismLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for PrismLayout {}

    impl LayoutManagerImpl for PrismLayout {
        /// Containers ask for nothing: their parent gives them a frame.
        fn measure(
            &self,
            _widget: &gtk::Widget,
            _orientation: gtk::Orientation,
            _for_size: i32,
        ) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            let mut child = widget.first_child();
            while let Some(c) = child {
                // Popovers (context menus) place themselves.
                if let Some(p) = c.downcast_ref::<gtk::Popover>() {
                    p.present();
                    child = c.next_sibling();
                    continue;
                }
                let (x, y, w, h) = FRAMES
                    .with(|f| f.borrow().get(&key(&c)).copied())
                    .unwrap_or_default();
                // Sticky: pushed down as its scroll view scrolls.
                let y = y + STICKY.with(|s| s.borrow().get(&key(&c)).copied().unwrap_or(0));
                // GTK never allocates less than a widget's minimum.
                let (min_w, _, _, _) = c.measure(gtk::Orientation::Horizontal, -1);
                let (min_h, _, _, _) = c.measure(gtk::Orientation::Vertical, w.max(min_w));
                let (w, h) = (w.max(min_w), h.max(min_h));
                match TRANSFORMS.with(|t| t.borrow().get(&key(&c)).copied()) {
                    Some(t) => c.allocate(w, h, -1, Some(child_transform(x, y, w, h, t))),
                    None => c.size_allocate(&gtk::Allocation::new(x, y, w, h), -1),
                }
                child = c.next_sibling();
            }
            let changed = ROOT.with(|r| {
                let mut r = r.borrow_mut();
                match r.as_mut() {
                    Some((k, rw, rh)) if *k == key(widget) && (*rw, *rh) != (width, height) => {
                        (*rw, *rh) = (width, height);
                        true
                    }
                    _ => false,
                }
            });
            if changed {
                // Relayout outside of GTK's allocation pass.
                glib::idle_add_local_once(move || {
                    ON_ROOT_RESIZE.with(|cb| {
                        if let Some(cb) = cb.borrow().as_ref() {
                            cb(width, height)
                        }
                    })
                });
            }
        }
    }
}

glib::wrapper! {
    pub struct PrismLayout(ObjectSubclass<imp::PrismLayout>) @extends gtk::LayoutManager;
}

impl PrismLayout {
    pub fn new() -> Self {
        glib::Object::new()
    }
}

/// A container whose children sit at their Prism frames.
pub fn container() -> gtk::Box {
    let b: PrismBox = glib::Object::builder()
        .property("orientation", gtk::Orientation::Vertical)
        .build();
    b.set_layout_manager(Some(PrismLayout::new()));
    b.upcast()
}

/// CSS class of containers that let pointer input through wherever they
/// have no child (runtime layers such as the toaster).
pub const PASS_THROUGH: &str = "prism-layer";

mod pass_through {
    use super::*;

    /// A `GtkBox` that, given [`PASS_THROUGH`](super::PASS_THROUGH), is
    /// never picked itself: GTK's picking then reaches the widgets under it,
    /// while its children are still picked normally.
    #[derive(Default)]
    pub struct PrismBox;

    #[glib::object_subclass]
    impl ObjectSubclass for PrismBox {
        const NAME: &'static str = "PrismBox";
        type Type = super::PrismBox;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for PrismBox {}

    impl WidgetImpl for PrismBox {
        fn contains(&self, x: f64, y: f64) -> bool {
            !self.obj().has_css_class(super::PASS_THROUGH) && self.parent_contains(x, y)
        }

        /// Tab reaches a focusable box (a drawn button, a view with
        /// `on_key_down`), then its children; Shift+Tab its children, then
        /// the box. GtkBox only moves the focus among its children
        /// (gtkbox.c, `widget_class->focus = gtk_widget_focus_child`), and
        /// GtkWidget's own (`gtk_widget_real_focus`) never reaches them.
        fn focus(&self, direction: gtk::DirectionType) -> bool {
            let w = self.obj();
            if !w.is_focusable() {
                return self.parent_focus(direction);
            }
            let forward = matches!(
                direction,
                gtk::DirectionType::TabForward
                    | gtk::DirectionType::Right
                    | gtk::DirectionType::Down
            );
            if w.is_focus() {
                return forward && self.parent_focus(direction);
            }
            // Coming from before the box, forward: the box first.
            let inside = w
                .root()
                .and_then(|r| r.focus())
                .is_some_and(|f| f.is_ancestor(&*w));
            if forward && !inside && w.grab_focus() {
                return true;
            }
            // Its children; backward, the box after them.
            self.parent_focus(direction) || (!forward && w.grab_focus())
        }
    }

    impl BoxImpl for PrismBox {}
}

/// CSS class of a widget Prism makes take the keyboard focus (a drawn
/// control, a view with `on_key_down`, a clickable text), which shows it;
/// a label also needs [`focusable_labels`].
pub const FOCUSABLE: &str = "prism-keys";

type GrabFocus = unsafe extern "C" fn(*mut gtk::ffi::GtkWidget) -> glib::ffi::gboolean;
type Focus = unsafe extern "C" fn(
    *mut gtk::ffi::GtkWidget,
    gtk::ffi::GtkDirectionType,
) -> glib::ffi::gboolean;

static LABEL_FOCUS: std::sync::OnceLock<(GrabFocus, Focus)> = std::sync::OnceLock::new();

/// Lets a focusable label with [`FOCUSABLE`] take the focus, from a
/// click or with Tab: GTK's only does with links or a selection
/// (gtklabel.c, `gtk_label_grab_focus` and `gtk_label_focus`), and GtkLabel
/// can't be subclassed (its class is private). GtkLabel's two focus
/// methods are wrapped once; other labels behave as before.
pub fn focusable_labels() {
    use gtk::glib::translate::IntoGlib;
    LABEL_FOCUS.get_or_init(|| {
        // SAFETY: GtkLabel's class lives as long as the process (referenced
        // here, never released); it is patched once, on the main thread
        // (GTK's), and every label, made before or after, uses the
        // wrappers. GtkWidgetClass sets both methods.
        unsafe {
            let class = glib::gobject_ffi::g_type_class_ref(gtk::Label::static_type().into_glib())
                as *mut gtk::ffi::GtkWidgetClass;
            let original = (
                (*class).grab_focus.expect("GtkWidget sets grab_focus"),
                (*class).focus.expect("GtkWidget sets focus"),
            );
            (*class).grab_focus = Some(label_grab_focus);
            (*class).focus = Some(label_focus);
            original
        }
    });
}

unsafe extern "C" fn label_grab_focus(w: *mut gtk::ffi::GtkWidget) -> glib::ffi::gboolean {
    use gtk::glib::translate::from_glib_borrow;
    let (grab, _) = LABEL_FOCUS.get().expect("installed with the wrapper");
    // SAFETY: GTK calls it with a live GtkLabel.
    if unsafe { grab(w) } != 0 {
        return 1;
    }
    let w: glib::translate::Borrowed<gtk::Widget> = unsafe { from_glib_borrow(w) };
    // gtkwidget.c, `gtk_widget_grab_focus_self`.
    match w.root() {
        Some(root) if w.is_focusable() && w.has_css_class(FOCUSABLE) => {
            root.set_focus(Some(&*w));
            1
        }
        _ => 0,
    }
}

unsafe extern "C" fn label_focus(
    w: *mut gtk::ffi::GtkWidget,
    direction: gtk::ffi::GtkDirectionType,
) -> glib::ffi::gboolean {
    use gtk::glib::translate::from_glib_borrow;
    let (_, focus) = LABEL_FOCUS.get().expect("installed with the wrapper");
    let widget: glib::translate::Borrowed<gtk::Widget> = unsafe { from_glib_borrow(w) };
    let had = widget.is_focus();
    // SAFETY: GTK calls it with a live GtkLabel.
    if unsafe { focus(w, direction) } != 0 {
        return 1;
    }
    // Without links GTK's takes the focus (above) but answers that it
    // didn't, so Tab went on to the next widget.
    (widget.has_css_class(FOCUSABLE) && !had && widget.is_focus()).into()
}

glib::wrapper! {
    /// See [`container`].
    pub struct PrismBox(ObjectSubclass<pass_through::PrismBox>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}
