//! A filled picture's focal point (#218): GTK's `ContentFit::Cover` always
//! crops around the center, so the picture shows a paintable that covers
//! its size around the focal point instead (`ContentFit::Fill`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use gtk::gdk;
use gtk::glib;
use gtk::graphene;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use prism_core::NodeId;

/// A picture's fit (`true`: letterboxed) and focal point.
type Scale = (bool, Option<(f32, f32)>);

thread_local! {
    static PICTURES: RefCell<HashMap<NodeId, Scale>> = RefCell::default();
}

/// Picture `id` is letterboxed (`fit`) or filled.
pub(crate) fn set_fit(id: NodeId, p: &gtk::Picture, fit: bool) {
    PICTURES.with(|m| m.borrow_mut().entry(id).or_default().0 = fit);
    apply(id, p);
}

/// Picture `id`'s focal point (its config, `x y`).
pub(crate) fn set_focus(id: NodeId, p: &gtk::Picture, config: &str) {
    let at = prism_core::picture_position(config);
    PICTURES.with(|m| m.borrow_mut().entry(id).or_default().1 = (at != (0.5, 0.5)).then_some(at));
    apply(id, p);
}

/// Shows picture `id` as it asks; called again when its content changes.
pub(crate) fn apply(id: NodeId, p: &gtk::Picture) {
    let (fit, at) = PICTURES.with(|m| m.borrow().get(&id).copied().unwrap_or_default());
    let current = p.paintable();
    let inner = match current
        .as_ref()
        .and_then(|c| c.downcast_ref::<FocalPaintable>())
    {
        Some(f) => f.imp().inner.borrow().clone(),
        None => current.clone(),
    };
    match (fit, at, inner) {
        (false, Some(at), Some(inner)) => {
            match current
                .as_ref()
                .and_then(|c| c.downcast_ref::<FocalPaintable>())
            {
                Some(f) => f.set_focus(at),
                None => p.set_paintable(Some(&FocalPaintable::new(&inner, at))),
            }
            p.set_content_fit(gtk::ContentFit::Fill);
        }
        (fit, _, inner) => {
            if current.as_ref().is_some_and(|c| c.is::<FocalPaintable>()) {
                p.set_paintable(inner.as_ref());
            }
            p.set_content_fit(if fit {
                gtk::ContentFit::Contain
            } else {
                gtk::ContentFit::Cover
            });
        }
    }
}

/// Drops picture `id`'s settings.
pub(crate) fn forget(id: NodeId) {
    PICTURES.with(|m| m.borrow_mut().remove(&id));
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct FocalPaintable {
        pub inner: RefCell<Option<gdk::Paintable>>,
        pub at: Cell<(f32, f32)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FocalPaintable {
        const NAME: &'static str = "PrismFocalPaintable";
        type Type = super::FocalPaintable;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for FocalPaintable {}

    impl PaintableImpl for FocalPaintable {
        fn intrinsic_width(&self) -> i32 {
            self.inner
                .borrow()
                .as_ref()
                .map_or(0, |p| p.intrinsic_width())
        }

        fn intrinsic_height(&self) -> i32 {
            self.inner
                .borrow()
                .as_ref()
                .map_or(0, |p| p.intrinsic_height())
        }

        fn intrinsic_aspect_ratio(&self) -> f64 {
            self.inner
                .borrow()
                .as_ref()
                .map_or(0., |p| p.intrinsic_aspect_ratio())
        }

        /// The image scaled to cover `width` × `height`, placed so the
        /// focal point stays where it would be, and clipped.
        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            let Some(inner) = self.inner.borrow().clone() else {
                return;
            };
            let (iw, ih) = (
                inner.intrinsic_width() as f64,
                inner.intrinsic_height() as f64,
            );
            let Some(snap) = snapshot.downcast_ref::<gtk::Snapshot>() else {
                return inner.snapshot(snapshot, width, height);
            };
            if iw <= 0. || ih <= 0. {
                return inner.snapshot(snapshot, width, height);
            }
            let s = (width / iw).max(height / ih);
            let (sw, sh) = (iw * s, ih * s);
            let (ax, ay) = self.at.get();
            snap.push_clip(&graphene::Rect::new(0., 0., width as f32, height as f32));
            snap.save();
            snap.translate(&graphene::Point::new(
                ((width - sw) * ax as f64) as f32,
                ((height - sh) * ay as f64) as f32,
            ));
            inner.snapshot(snap, sw, sh);
            snap.restore();
            snap.pop();
        }
    }
}

glib::wrapper! {
    /// A paintable covering its size around a focal point.
    pub struct FocalPaintable(ObjectSubclass<imp::FocalPaintable>) @implements gdk::Paintable;
}

impl FocalPaintable {
    fn new(inner: &gdk::Paintable, at: (f32, f32)) -> Self {
        let p: Self = glib::Object::new();
        *p.imp().inner.borrow_mut() = Some(inner.clone());
        p.imp().at.set(at);
        // An animation or a late size: shown again.
        let weak = p.downgrade();
        inner.connect_invalidate_contents(move |_| {
            if let Some(p) = weak.upgrade() {
                p.invalidate_contents();
            }
        });
        let weak = p.downgrade();
        inner.connect_invalidate_size(move |_| {
            if let Some(p) = weak.upgrade() {
                p.invalidate_size();
            }
        });
        p
    }

    fn set_focus(&self, at: (f32, f32)) {
        if self.imp().at.replace(at) != at {
            self.invalidate_contents();
        }
    }
}
