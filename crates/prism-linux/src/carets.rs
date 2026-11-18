//! Other people's carets and selections in text areas and rich inputs
//! (#240): a drawing area over the text view, in the text area's overlay,
//! that takes no pointer input. It draws, with the view's own layout
//! (`iter_location`, display lines, scrolling), each caret as a thin bar in
//! its color with a name tag, and its selection as tinted boxes, and
//! redraws when the text, the marks or the scroll position change. The
//! buffer, its marks and the input method are never touched. A one-line
//! text input (`GtkEntry`) shows none.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use prism_core::{Color, NodeId, RemoteCaret};

use crate::controls::TextArea;

struct Layer {
    area: gtk::DrawingArea,
    carets: Rc<RefCell<Vec<RemoteCaret>>>,
}

thread_local! {
    static LAYERS: RefCell<HashMap<NodeId, Layer>> = RefCell::new(HashMap::new());
}

/// Shows `carets` over text area `t` (none: removes them).
pub(crate) fn set(id: NodeId, t: &TextArea, carets: &[RemoteCaret]) {
    if carets.is_empty() {
        forget(id);
        return;
    }
    LAYERS.with(|l| {
        let mut l = l.borrow_mut();
        let layer = l.entry(id).or_insert_with(|| new_layer(t));
        *layer.carets.borrow_mut() = carets.to_vec();
        layer.area.queue_draw();
    });
}

/// Text area `id` is gone or shows no carets.
pub(crate) fn forget(id: NodeId) {
    if let Some(layer) = LAYERS.with(|l| l.borrow_mut().remove(&id))
        && let Some(overlay) = layer.area.parent().and_downcast::<gtk::Overlay>()
    {
        overlay.remove_overlay(&layer.area);
    }
}

fn new_layer(t: &TextArea) -> Layer {
    let area = gtk::DrawingArea::new();
    area.set_can_target(false);
    area.set_can_focus(false);
    area.set_hexpand(true);
    area.set_vexpand(true);
    let carets: Rc<RefCell<Vec<RemoteCaret>>> = Rc::default();
    let (view, scroll) = (t.view.downgrade(), t.scroll.downgrade());
    let drawn = carets.clone();
    area.set_draw_func(move |area, cr, _, _| {
        if let (Some(view), Some(scroll)) = (view.upgrade(), scroll.upgrade()) {
            draw(area, cr, &view, &scroll, &drawn.borrow());
        }
    });
    t.root.add_overlay(&area);
    // Redrawn when the text, a mark (typing, the user's caret) or the
    // scroll position change.
    let redraw = {
        let area = area.downgrade();
        move || {
            if let Some(a) = area.upgrade() {
                a.queue_draw();
            }
        }
    };
    let buffer = t.view.buffer();
    let r = redraw.clone();
    buffer.connect_changed(move |_| r());
    let r = redraw.clone();
    buffer.connect_mark_set(move |_, _, _| r());
    let r = redraw.clone();
    t.scroll.vadjustment().connect_value_changed(move |_| r());
    Layer { area, carets }
}

fn source(cr: &gtk::cairo::Context, c: Color, alpha: f64) {
    let c = if crate::imp::dark() {
        c.dark_variant()
    } else {
        c.light_variant()
    };
    cr.set_source_rgba(c.r as f64, c.g as f64, c.b as f64, c.a as f64 * alpha);
}

/// A rectangle (x, y, width, height).
type Rect = (f64, f64, f64, f64);

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x = a.0.max(b.0);
    let y = a.1.max(b.1);
    let r = (a.0 + a.2).min(b.0 + b.2);
    let t = (a.1 + a.3).min(b.1 + b.3);
    (r > x && t > y).then_some((x, y, r - x, t - y))
}

fn draw(
    area: &gtk::DrawingArea,
    cr: &gtk::cairo::Context,
    view: &gtk::TextView,
    scroll: &gtk::ScrolledWindow,
    carets: &[RemoteCaret],
) {
    let buffer = view.buffer();
    let len = buffer.char_count().max(0) as usize;
    let at = |c: usize| buffer.iter_at_offset(c.min(len) as i32);
    // Buffer coordinates → this area's.
    let to_area = |r: gtk::gdk::Rectangle| -> Option<Rect> {
        let (x, y) = view.buffer_to_window_coords(gtk::TextWindowType::Widget, r.x(), r.y());
        let (x, y) = view.translate_coordinates(area, x as f64, y as f64)?;
        Some((x, y, r.width() as f64, r.height() as f64))
    };
    // What shows of the text.
    let Some((cx, cy)) = scroll.translate_coordinates(area, 0., 0.) else {
        return;
    };
    let clip = (cx, cy, scroll.width() as f64, scroll.height() as f64);
    for c in carets {
        let range = c.range();
        // One box per display line of the selection.
        source(cr, c.color, 0.25);
        let (mut s, e) = (at(range.start), at(range.end));
        while s < e {
            let mut line_end = s;
            view.forward_display_line_end(&mut line_end);
            let stop = if line_end < e && line_end > s {
                line_end
            } else {
                e
            };
            let (a, b) = (view.iter_location(&s), view.iter_location(&stop));
            let width = if stop == e {
                (b.x() - a.x()) as f64
            } else {
                // To the line's end: the space it wrapped at too.
                (b.x() + b.width().max(4) - a.x()) as f64
            };
            if let Some((x, y, _, h)) = to_area(a)
                && let Some((x, y, w, h)) = intersect((x, y, width, h), clip)
            {
                cr.rectangle(x, y, w, h);
                let _ = cr.fill();
            }
            let mut next = stop;
            if stop == e || !view.forward_display_line(&mut next) {
                break;
            }
            view.backward_display_line_start(&mut next);
            if next <= s {
                break;
            }
            s = next;
        }
        // The caret.
        let Some((x, y, _, h)) = to_area(view.iter_location(&at(c.head))) else {
            continue;
        };
        if x < clip.0 - 1.
            || x > clip.0 + clip.2 + 1.
            || intersect((x - 1., y, 2., h), clip).is_none()
        {
            continue;
        }
        source(cr, c.color, 1.);
        cr.rectangle(x - 1., y, 2., h);
        let _ = cr.fill();
        // Its name, above the line (under it at the top); gone when idle.
        let Some(label) = c.label.as_deref().filter(|_| !c.idle) else {
            continue;
        };
        cr.select_font_face(
            "Sans",
            gtk::cairo::FontSlant::Normal,
            gtk::cairo::FontWeight::Bold,
        );
        cr.set_font_size(10.);
        let Ok(ext) = cr.text_extents(label) else {
            continue;
        };
        let (w, th) = ((ext.x_advance() + 8.).ceil(), 14.);
        let ty = if y - th >= 0. { y - th } else { y + h };
        let tx = (x - 1.).min(area.width() as f64 - w).max(0.);
        source(cr, c.color, 1.);
        cr.rectangle(tx, ty, w, th);
        let _ = cr.fill();
        // White, or the app's (#267).
        match c.label_text_color {
            Some(t) => source(cr, t, 1.),
            None => cr.set_source_rgba(1., 1., 1., 1.),
        }
        cr.move_to(tx + 4., ty + 10.5);
        let _ = cr.show_text(label);
    }
}
