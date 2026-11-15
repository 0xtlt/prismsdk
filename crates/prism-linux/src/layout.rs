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
    TRANSFORMS.with(|f| f.borrow_mut().remove(&key(w)));
    for channel in [Channel::Frame, Channel::Visual, Channel::Loop] {
        stop(w, channel);
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
            transform: Transform {
                x: l(a.transform.x, b.transform.x),
                y: l(a.transform.y, b.transform.y),
                scale: l(a.transform.scale, b.transform.scale),
                rotate: l(a.transform.rotate, b.transform.rotate),
            },
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

/// Back and forth between `a` and `b` until stopped.
pub fn repeat(w: &impl IsA<gtk::Widget>, a: Visual, b: Visual, t: Transition) {
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
        .scale(t.scale, t.scale)
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
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.set_layout_manager(Some(PrismLayout::new()));
    b
}
