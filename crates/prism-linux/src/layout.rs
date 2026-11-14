//! A GTK layout manager that allocates each child exactly the frame Prism's
//! layout gave it (`set_frame`), instead of GTK's own sizing.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

type ResizeHandler = Box<dyn Fn(i32, i32)>;

thread_local! {
    /// Child widget (pointer) → x, y, width, height.
    static FRAMES: RefCell<HashMap<usize, (i32, i32, i32, i32)>> = RefCell::new(HashMap::new());
    /// Called when the root container gets a new size.
    static ON_ROOT_RESIZE: RefCell<Option<ResizeHandler>> = const { RefCell::new(None) };
    static ROOT: RefCell<Option<(usize, i32, i32)>> = const { RefCell::new(None) };
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
                let (x, y, w, h) = FRAMES
                    .with(|f| f.borrow().get(&key(&c)).copied())
                    .unwrap_or_default();
                // GTK never allocates less than a widget's minimum.
                let (min_w, _, _, _) = c.measure(gtk::Orientation::Horizontal, -1);
                let (min_h, _, _, _) = c.measure(gtk::Orientation::Vertical, w.max(min_w));
                c.size_allocate(&gtk::Allocation::new(x, y, w.max(min_w), h.max(min_h)), -1);
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
