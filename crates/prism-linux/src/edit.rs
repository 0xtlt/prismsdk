//! A rich input's editing hooks (#217) in GTK: the selection's rectangle,
//! the app's copy, cut and paste through the display's clipboard (a paste
//! reads the plain text, the HTML and the apps' own text types), and
//! Ctrl- / Alt-clicks on its characters.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::prelude::*;
use gtk::{gdk, glib};
use prism_core::edit_events::{CLICK, COPY, CUT, PASTE, SELECTION_RECT, SPAN_CLICK};
use prism_core::text_edit::TEXT_HTML;
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
                return plain_copy(id, &view, signal, cut);
            }
            let b = view.buffer();
            let (start, end) = b.selection_bounds()?;
            view.stop_signal_emission_by_name(signal);
            // A chip is its U+FFFC, like the offsets (#115).
            let text = b.slice(&start, &end, false).to_string();
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
            return plain_paste(id, &view);
        }
        view.stop_signal_emission_by_name("paste-clipboard");
        let selection = crate::imp::buffer_selection(&view.buffer());
        let clipboard = view.clipboard();
        glib::MainContext::default().spawn_local(async move {
            let paste = paste_event(&clipboard, selection).await;
            crate::imp::dispatch(UiEvent::Paste(id, paste));
        });
        None
    });
    // Ctrl- or Alt-clicks: the character under the pointer (Super is the
    // desktop's).
    let click = gtk::GestureClick::new();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let v = view.clone();
    click.connect_pressed(move |g, n, x, y| {
        let mods = g.current_event_state();
        let (bx, by) = v.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let at = v
            .iter_at_location(bx, by)
            .map(|i| i.offset().max(0) as usize);
        // Every click, with the character under it (#266); the view still
        // places its caret.
        if mask(id) & CLICK != 0
            && let Some(at) = at
        {
            crate::imp::dispatch(UiEvent::FieldClick(
                id,
                prism_core::ClickEvent {
                    position: Some(prism_core::Point::new(x as f32, y as f32)),
                    modifiers: crate::imp::modifiers(mods),
                    count: n.max(1) as u32,
                    button: prism_core::PointerButton::Primary,
                    text_offset: Some(at),
                },
            ));
        }
        if mask(id) & SPAN_CLICK == 0
            || !mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK)
        {
            return;
        }
        let Some(at) = at else {
            return;
        };
        g.set_state(gtk::EventSequenceState::Claimed);
        crate::imp::dispatch(UiEvent::SpanClick(id, at, crate::imp::modifiers(mods)));
    });
    view.add_controller(click);
}

/// A rich input's copy or cut the app does not take (#115): plain text,
/// each chip as its label, instead of GTK's own buffer (whose tags and
/// anchors are the view's). Other text areas: GTK's.
fn plain_copy(id: NodeId, view: &gtk::TextView, signal: &str, cut: bool) -> Option<glib::Value> {
    if !crate::rich::is_rich(id) {
        return None;
    }
    view.stop_signal_emission_by_name(signal);
    let b = view.buffer();
    let (start, end) = b.selection_bounds()?;
    let text = crate::rich::labelled(id, &b, start.offset(), end.offset());
    view.clipboard().set_text(&text);
    if cut {
        b.delete_selection(true, view.is_editable());
    }
    None
}

/// A rich input's paste the app does not take (#115): the clipboard's
/// plain text, without object characters (chips come from the app).
fn plain_paste(id: NodeId, view: &gtk::TextView) -> Option<glib::Value> {
    if !crate::rich::is_rich(id) {
        return None;
    }
    view.stop_signal_emission_by_name("paste-clipboard");
    let view = view.clone();
    glib::MainContext::default().spawn_local(async move {
        let text = view.clipboard().read_text_future().await.ok().flatten();
        let Some(text) = text else {
            return;
        };
        let text = text.replace(prism_core::OBJECT_CHAR, "");
        let b = view.buffer();
        let editable = view.is_editable();
        b.begin_user_action();
        b.delete_selection(true, editable);
        b.insert_interactive_at_cursor(&text, editable);
        b.end_user_action();
        view.scroll_mark_onscreen(&b.get_insert());
    });
    None
}

/// What `clipboard` holds, as a paste over `selection`: its plain text,
/// its HTML, its types, and the text of Markdown and the apps' own types
/// (`ClipboardContent::custom`), like AppKit's and the web's pastes.
async fn paste_event(
    clipboard: &gdk::Clipboard,
    selection: prism_core::SelectionEvent,
) -> PasteEvent {
    let types: Vec<String> = clipboard
        .formats()
        .mime_types()
        .iter()
        .map(|t| t.to_string())
        .collect();
    let text = clipboard
        .read_text_future()
        .await
        .ok()
        .flatten()
        .map(|t| t.to_string())
        .unwrap_or_default();
    let html = if types.iter().any(|t| t == TEXT_HTML) {
        read_type(clipboard, TEXT_HTML).await
    } else {
        None
    };
    let mut data = Vec::new();
    for ty in types.iter().filter(|t| is_text_data(t)) {
        if let Some(s) = read_type(clipboard, ty).await {
            data.push((ty.clone(), s));
        }
    }
    PasteEvent::new(selection, text)
        .with_html(html)
        .with_types(types)
        .with_data(data)
}

/// The clipboard's data of MIME type `ty` as text (`None`: not there, or
/// not text).
async fn read_type(clipboard: &gdk::Clipboard, ty: &str) -> Option<String> {
    use gtk::gio;
    let (stream, _) = clipboard
        .read_future(&[ty], glib::Priority::DEFAULT)
        .await
        .ok()?;
    let out = gio::MemoryOutputStream::new_resizable();
    out.splice_future(
        &stream,
        gio::OutputStreamSpliceFlags::CLOSE_SOURCE | gio::OutputStreamSpliceFlags::CLOSE_TARGET,
        glib::Priority::DEFAULT,
    )
    .await
    .ok()?;
    decode_text(&out.steal_as_bytes())
}

/// Whether clipboard type `ty` is text an app may want besides the plain
/// text and the HTML: Markdown and the apps' own MIME types. Not the plain
/// text's other names, images, files or the toolkits' own types.
fn is_text_data(ty: &str) -> bool {
    let ty = ty.to_ascii_lowercase();
    let Some((top, _)) = ty.split_once('/') else {
        return false;
    };
    let toolkit = ty.starts_with("text/plain")
        || ty == TEXT_HTML
        || ty == "text/uri-list"
        || ty.starts_with("application/x-gtk")
        || ty.starts_with("x-special/")
        || ty.starts_with("chromium/");
    !toolkit && !matches!(top, "image" | "audio" | "video" | "font" | "model")
}

/// Clipboard bytes as text: UTF-8, or UTF-16 with its byte order mark
/// (Firefox's HTML); `None` for anything else (binary data).
fn decode_text(bytes: &[u8]) -> Option<String> {
    let utf16 = |le: bool| {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16(&units).ok()
    };
    match bytes {
        [0xff, 0xfe, ..] => utf16(true),
        [0xfe, 0xff, ..] => utf16(false),
        [0xef, 0xbb, 0xbf, rest @ ..] => String::from_utf8(rest.to_vec()).ok(),
        _ => String::from_utf8(bytes.to_vec()).ok(),
    }
    .map(|s| s.trim_end_matches('\0').to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paste_reads_markdown_and_the_apps_own_types() {
        for ty in ["text/markdown", "application/x-blocks", "application/json"] {
            assert!(is_text_data(ty), "{ty}");
        }
        for ty in [
            "text/plain",
            "text/plain;charset=utf-8",
            "text/html",
            "text/uri-list",
            "image/png",
            "application/x-gtk-text-buffer-rich-text",
            "UTF8_STRING",
            "GtkTextBuffer",
        ] {
            assert!(!is_text_data(ty), "{ty}");
        }
    }

    #[test]
    fn clipboard_text_is_utf8_or_utf16_with_its_mark() {
        assert_eq!(decode_text(b"<b>a</b>").as_deref(), Some("<b>a</b>"));
        let le: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain("é<i>".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(decode_text(&le).as_deref(), Some("é<i>"));
        assert_eq!(decode_text(b"a\0").as_deref(), Some("a"));
        assert_eq!(decode_text(&[0x80, 0x81]), None, "binary");
    }
}
