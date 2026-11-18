//! A rich input's editing hooks (#217) in GTK, where a rich input is a
//! plain `GtkTextView`: the selection's rectangle, the app's copy, cut and
//! paste through the display's clipboard, and Ctrl- / Alt-clicks on its
//! characters.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::prelude::*;
use gtk::{gdk, glib};
use prism_core::edit_events::{COPY, CUT, PASTE, SELECTION_RECT, SPAN_CLICK};
use prism_core::{ClipboardContent, NodeId, PasteEvent, Rect, UiEvent};

use crate::controls::TextArea;

thread_local! {
    /// The editing events each field reports.
    static MASKS: RefCell<HashMap<NodeId, u8>> = RefCell::new(HashMap::new());
    /// The rectangle last reported for each field, and those scheduled.
    static REPORTED: RefCell<HashMap<NodeId, Reported>> = RefCell::new(HashMap::new());
    static SCHEDULED: RefCell<HashMap<NodeId, gtk::TextView>> = RefCell::new(HashMap::new());
}

/// A reported rectangle: the selection, then the box in half points.
type Reported = (usize, usize, [i32; 4]);

fn mask(id: NodeId) -> u8 {
    MASKS.with(|m| m.borrow().get(&id).copied().unwrap_or(0))
}

/// Which editing events field `id` reports (`Widgets::set_edit_events`).
pub(crate) fn set_mask(id: NodeId, t: &TextArea, mask: u8) {
    MASKS.with(|m| {
        if mask == 0 {
            m.borrow_mut().remove(&id);
        } else {
            m.borrow_mut().insert(id, mask);
        }
    });
    if mask & SELECTION_RECT == 0 {
        REPORTED.with(|r| r.borrow_mut().remove(&id));
    } else {
        rect_changed(id, &t.view);
    }
}

/// Field `id` is gone.
pub(crate) fn forget(id: NodeId) {
    MASKS.with(|m| m.borrow_mut().remove(&id));
    REPORTED.with(|r| r.borrow_mut().remove(&id));
    SCHEDULED.with(|s| s.borrow_mut().remove(&id));
}

/// Connects text area `view` of node `id` to the hooks: its clipboard
/// signals and a Ctrl- / Alt-click gesture, each a no-op until the app
/// asks for it.
pub(crate) fn connect(id: NodeId, view: &gtk::TextView) {
    for cut in [false, true] {
        let signal = if cut {
            "cut-clipboard"
        } else {
            "copy-clipboard"
        };
        view.connect_local(signal, false, move |args| {
            let view: gtk::TextView = args[0].get().ok()?;
            if mask(id) & if cut { CUT } else { COPY } == 0 {
                return None;
            }
            let b = view.buffer();
            let (start, end) = b.selection_bounds()?;
            view.stop_signal_emission_by_name(signal);
            let text = b.text(&start, &end, false).to_string();
            let (s, e) = (start.offset().max(0) as usize, end.offset().max(0) as usize);
            crate::imp::dispatch(UiEvent::Copy(id, cut, s, e, text));
            if cut {
                b.delete_selection(true, view.is_editable());
            }
            None
        });
    }
    view.connect_local("paste-clipboard", false, move |args| {
        let view: gtk::TextView = args[0].get().ok()?;
        if mask(id) & PASTE == 0 {
            return None;
        }
        view.stop_signal_emission_by_name("paste-clipboard");
        let selection = crate::imp::buffer_selection(&view.buffer());
        let clipboard = view.clipboard();
        let types: Vec<String> = clipboard
            .formats()
            .mime_types()
            .iter()
            .map(|t| t.to_string())
            .collect();
        clipboard.read_text_async(None::<&gtk::gio::Cancellable>, move |text| {
            let text = text
                .ok()
                .flatten()
                .map(|t| t.to_string())
                .unwrap_or_default();
            let paste = PasteEvent::new(selection, text).with_types(types);
            crate::imp::dispatch(UiEvent::Paste(id, paste));
        });
        None
    });
    // Ctrl- or Alt-clicks: the character under the pointer (Super is the
    // desktop's).
    let click = gtk::GestureClick::new();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let v = view.clone();
    click.connect_pressed(move |g, _, x, y| {
        let mods = g.current_event_state();
        if mask(id) & SPAN_CLICK == 0
            || !mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK)
        {
            return;
        }
        g.set_state(gtk::EventSequenceState::Claimed);
        let (bx, by) = v.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let Some(iter) = v.iter_at_location(bx, by) else {
            return;
        };
        crate::imp::dispatch(UiEvent::SpanClick(
            id,
            iter.offset().max(0) as usize,
            crate::imp::modifiers(mods),
        ));
    });
    view.add_controller(click);
}

/// Writes `content` to the display's clipboard, every type as one item.
pub(crate) fn write_clipboard(widget: &impl IsA<gtk::Widget>, content: &ClipboardContent) {
    let providers: Vec<gdk::ContentProvider> = content
        .entries()
        .into_iter()
        .map(|(mime, data)| {
            if mime == prism_core::text_edit::TEXT_PLAIN {
                gdk::ContentProvider::for_value(&data.to_value())
            } else {
                gdk::ContentProvider::for_bytes(mime, &glib::Bytes::from(data.as_bytes()))
            }
        })
        .collect();
    let union = gdk::ContentProvider::new_union(&providers);
    let _ = widget.as_ref().clipboard().set_content(Some(&union));
}

/// Inserts `text` over the selection like a paste, after Prism applied its
/// patches (the buffer then reports the edit).
pub(crate) fn insert_text(view: &gtk::TextView, text: &str) {
    let view = view.clone();
    let text = text.to_string();
    glib::idle_add_local_once(move || {
        let b = view.buffer();
        let editable = view.is_editable();
        b.begin_user_action();
        b.delete_selection(true, editable);
        b.insert_interactive_at_cursor(&text, editable);
        b.end_user_action();
    });
}

/// Field `id`'s selection or text changed: its rectangle is reported on a
/// later turn, once, when it moved.
pub(crate) fn rect_changed(id: NodeId, view: &gtk::TextView) {
    if mask(id) & SELECTION_RECT == 0 {
        return;
    }
    let first = SCHEDULED.with(|s| s.borrow_mut().insert(id, view.clone()).is_none());
    if !first {
        return;
    }
    glib::idle_add_local_full(glib::Priority::LOW, move || {
        let view = SCHEDULED.with(|s| s.borrow_mut().remove(&id));
        if let Some(view) = view
            && mask(id) & SELECTION_RECT != 0
            && let Some(rect) = selection_rect(&view)
        {
            let sel = crate::imp::buffer_selection(&view.buffer());
            let key = (
                sel.start,
                sel.end,
                [rect.x, rect.y, rect.width, rect.height].map(|v| (v * 2.).round() as i32),
            );
            if REPORTED.with(|r| r.borrow_mut().insert(id, key)) != Some(key) {
                crate::imp::dispatch(UiEvent::SelectionRect(id, sel.start, sel.end, rect));
            }
        }
        glib::ControlFlow::Break
    });
}

/// The caret's box (one point wide) or the box around the selection, in
/// the field's coordinates (its overlay, scrolling taken off).
fn selection_rect(view: &gtk::TextView) -> Option<Rect> {
    let b = view.buffer();
    let (start, end) = b.selection_bounds().unwrap_or_else(|| {
        (
            b.iter_at_mark(&b.get_insert()),
            b.iter_at_mark(&b.get_insert()),
        )
    });
    let a = view.iter_location(&start);
    let z = view.iter_location(&end);
    let (x0, y0, x1, y1) = if start == end {
        (a.x(), a.y(), a.x() + 1, a.y() + a.height())
    } else if a.y() == z.y() {
        (a.x(), a.y(), z.x(), a.y() + a.height().max(z.height()))
    } else {
        // Several lines: their whole width.
        let width = view.width();
        (0, a.y(), width, z.y() + z.height())
    };
    let (wx, wy) = view.buffer_to_window_coords(gtk::TextWindowType::Widget, x0, y0);
    let root = view.parent()?.parent()?;
    #[allow(deprecated)]
    let (rx, ry) = view.translate_coordinates(&root, wx as f64, wy as f64)?;
    Some(Rect::new(
        rx as f32,
        ry as f32,
        (x1 - x0).max(1) as f32,
        (y1 - y0) as f32,
    ))
}
