//! One selection across several text fields (#231) on Linux: a mouse drag
//! from a text area or rich input into the other fields of its selection
//! group, and an entry's highlighted part.
//!
//! The keys go through the window's key controller
//! (`Runtime::consumes_key_event`); text areas and rich inputs draw their
//! part with a text tag (`TextArea::highlight`).

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use prism_core::{NodeId, UiEvent};

/// Follows a mouse drag in text area `id`: the text view's own while it
/// stays there; once it is over another field of the selection group, the
/// drag is claimed (the view's selection stops following it) and every
/// move goes to the runtime (`UiEvent::SelectionAcross`, #264), back over the text
/// area too.
pub fn attach_drag(id: NodeId, view: &gtk::TextView) {
    let drag = gtk::GestureDrag::new();
    drag.set_button(gtk::gdk::BUTTON_PRIMARY);
    // Before the view's own drag, which it may then take over.
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    // The group, where it started (characters) and whether it was taken
    // over.
    let state: Rc<Cell<Option<(NodeId, usize, bool)>>> = Rc::default();
    let (st, v) = (state.clone(), view.clone());
    drag.connect_drag_begin(move |_, x, y| {
        let group = crate::imp::selection_group_of(id);
        st.set(group.map(|g| (g, offset_in_view(&v, x, y), false)));
    });
    let (st, v) = (state.clone(), view.clone());
    drag.connect_drag_update(move |g, dx, dy| {
        let Some((group, at, taken)) = st.get() else {
            return;
        };
        let (Some((sx, sy)), Some(root)) = (g.start_point(), v.root()) else {
            return;
        };
        let local = gtk::graphene::Point::new((sx + dx) as f32, (sy + dy) as f32);
        let Some(p) = v.compute_point(&root, &local) else {
            return;
        };
        let Some((over, to)) = crate::imp::group_field_at(id, root.upcast_ref(), &p) else {
            return;
        };
        if over == id && !taken {
            return;
        }
        if !taken {
            g.set_state(gtk::EventSequenceState::Claimed);
            st.set(Some((group, at, true)));
        }
        crate::imp::dispatch(UiEvent::SelectionAcross(
            group,
            Some(((id, at), (over, to))),
        ));
    });
    drag.connect_drag_end(move |_, _, _| state.set(None));
    view.add_controller(drag);
}

/// The character of `view` at `(x, y)` in its own coordinates.
pub fn offset_in_view(view: &gtk::TextView, x: f64, y: f64) -> usize {
    let (bx, by) = view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
    view.iter_at_location(bx, by)
        .map_or_else(|| view.buffer().char_count(), |i| i.offset())
        .max(0) as usize
}

/// The entry's inner text widget, where its text is drawn.
fn text_child(entry: &gtk::Entry) -> Option<gtk::Widget> {
    let mut child = entry.first_child();
    while let Some(c) = child {
        if c.is::<gtk::Text>() {
            return Some(c);
        }
        child = c.next_sibling();
    }
    None
}

/// The character of `entry` nearest to `x` in its own coordinates (its
/// text measured with its font from where it is drawn; an entry not being
/// edited shows its start).
pub fn offset_in_entry(entry: &gtk::Entry, x: f64) -> usize {
    let text = entry.text().to_string();
    let x0 = text_child(entry)
        .and_then(|c| c.compute_bounds(entry))
        .map_or(0., |b| b.x() as f64);
    let layout = entry.create_pango_layout(Some(&text));
    let (_, index, trailing) = layout.xy_to_index(((x - x0) * gtk::pango::SCALE as f64) as i32, 0);
    let index = (index.max(0) as usize).min(text.len());
    text[..index].chars().count() + trailing.max(0) as usize
}

/// Draws characters `start..end` of an entry like a selection while
/// another field has the focus (#231): a background attribute of its text,
/// in the selection colour GTK 4 draws. `start == end` removes it.
pub fn highlight_entry(entry: &gtk::Entry, start: usize, end: usize) {
    let list = gtk::pango::AttrList::new();
    let text = entry.text().to_string();
    let byte = |c: usize| text.char_indices().nth(c).map_or(text.len(), |(b, _)| b);
    if start < end {
        let (s, e) = (byte(start) as u32, byte(end) as u32);
        let mut bg = gtk::pango::AttrColor::new_background(53 * 257, 132 * 257, 228 * 257);
        bg.set_start_index(s);
        bg.set_end_index(e);
        list.insert(bg);
        let mut alpha = gtk::pango::AttrInt::new_background_alpha((0.35 * 65535.) as u16);
        alpha.set_start_index(s);
        alpha.set_end_index(e);
        list.insert(alpha);
    }
    entry.set_attributes(&list);
}
