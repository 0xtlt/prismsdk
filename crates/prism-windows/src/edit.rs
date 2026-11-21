//! A rich input's editing hooks (#217) on Windows, where a rich input is
//! a `RichEditBox` (`rich.rs`), as `edit.rs` does on macOS and GTK:
//!
//! - copies and cuts (`CopyingToClipboard`, `CuttingToClipboard`) go to
//!   the app, which writes the clipboard itself (`write_clipboard`); a cut
//!   still deletes the selection;
//! - pastes go to the app with what the clipboard holds (text, HTML, its
//!   types, Markdown and other text types written by apps), which answers
//!   with `insert_text` unless it takes the paste;
//! - Ctrl- and Alt-clicks report the character under the pointer
//!   (`SpanClick`), every click too when asked (`FieldClick`): the box
//!   handles its presses, so they are heard with `handledEventsToo`;
//! - the caret's or the selection's rectangle (`SelectionRect`), once a
//!   turn when it moved, in the field's coordinates.
//!
//! RichEdit counts in UTF-16 units of its plain text (a chip's picture is
//! one, U+FFFC); Prism in characters.

use std::cell::RefCell;
use std::collections::HashMap;

use prism_core::edit_events::{CLICK, COPY, CUT, PASTE, SELECTION_RECT, SPAN_CLICK};
use prism_core::{
    ClickEvent, ClipboardContent, ClipboardItem, NodeId, PasteEvent, Point, PointerButton, Rect,
    SelectionEvent, UiEvent,
};
use prism_native::fields::utf16_to_chars;
use windows::Foundation::TypedEventHandler;
use windows_core::{HSTRING, Interface, Result};

use crate::imp::{dispatch, main_hwnd};
use crate::win::{env, log};
use crate::winui::bindings::Microsoft::UI::Dispatching::{DispatcherQueue, DispatcherQueueHandler};
use crate::winui::bindings::Microsoft::UI::Text::{
    ITextRange, PointOptions, RichEditTextDocument, TextGetOptions,
};
use crate::winui::xaml::Controls::{
    RichEditBox, TextControlCopyingToClipboardEventArgs, TextControlCuttingToClipboardEventArgs,
};
use crate::winui::xaml::Input::{PointerEventHandler, PointerRoutedEventArgs};
use crate::winui::xaml::{FrameworkElement, UIElement};

/// A reported rectangle: the selection, then the box in half points.
type Reported = (usize, usize, [i32; 4]);

thread_local! {
    /// The editing events each field reports.
    static MASKS: RefCell<HashMap<NodeId, u8>> = RefCell::new(HashMap::new());
    /// The rectangle last reported for each field (the selection, then
    /// the box in half points), and the fields whose report is due.
    static REPORTED: RefCell<HashMap<NodeId, Reported>> = RefCell::new(HashMap::new());
    static SCHEDULED: RefCell<HashMap<NodeId, RichEditBox>> = RefCell::new(HashMap::new());
}

/// The editing events field `id` reports.
pub fn mask(id: NodeId) -> u8 {
    MASKS.with(|m| m.borrow().get(&id).copied().unwrap_or(0))
}

/// Which editing events field `id` reports (`Widgets::set_edit_events`).
pub fn set_mask(id: NodeId, r: &RichEditBox, mask: u8) {
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
        changed(id, r);
    }
}

/// Field `id` is gone.
pub fn forget(id: NodeId) {
    MASKS.with(|m| m.borrow_mut().remove(&id));
    REPORTED.with(|r| r.borrow_mut().remove(&id));
    SCHEDULED.with(|s| s.borrow_mut().remove(&id));
}

fn trace(e: &UiEvent) {
    if env("PRISM_TRACE_INPUT").is_some() {
        log!("input: {e:?}");
    }
}

/// The document's plain text (`\n` between paragraphs, without the last
/// paragraph mark), as `rich.rs` reports it.
fn text_of(doc: &RichEditTextDocument) -> Result<String> {
    let mut s = HSTRING::new();
    doc.GetText(TextGetOptions::UseLf, &mut s)?;
    Ok(s.to_string().replace('\u{b}', "\n"))
}

/// The selection in characters, and the text.
fn selection(r: &RichEditBox) -> Result<(SelectionEvent, String)> {
    let doc = r.Document()?;
    let text = text_of(&doc)?;
    let sel = doc.Selection()?;
    let (a, b) = (
        sel.StartPosition()?.max(0) as usize,
        sel.EndPosition()?.max(0) as usize,
    );
    Ok((
        SelectionEvent::new(utf16_to_chars(&text, a), utf16_to_chars(&text, b)),
        text,
    ))
}

/// Connects rich input `r` of node `id` to the hooks: each a no-op until
/// the app asks for it.
pub fn connect(id: NodeId, r: &RichEditBox) -> Result<()> {
    r.CopyingToClipboard(&TypedEventHandler::<
        RichEditBox,
        TextControlCopyingToClipboardEventArgs,
    >::new(move |s, a| {
        if let (Some(r), Some(a)) = (s.as_ref(), a.as_ref())
            && copy(id, r, false)?
        {
            a.SetHandled(true)?;
        }
        Ok(())
    }))?;
    r.CuttingToClipboard(&TypedEventHandler::<
        RichEditBox,
        TextControlCuttingToClipboardEventArgs,
    >::new(move |s, a| {
        if let (Some(r), Some(a)) = (s.as_ref(), a.as_ref())
            && copy(id, r, true)?
        {
            a.SetHandled(true)?;
            // The app writes the clipboard; the cut text still goes.
            r.Document()?.Selection()?.SetText(&HSTRING::new())?;
        }
        Ok(())
    }))?;
    // The box takes its presses (marks them handled): heard anyway.
    let e: UIElement = r.cast()?;
    e.AddHandler(
        &UIElement::PointerPressedEvent()?,
        &crate::boxed::boxed(PointerEventHandler::new(move |s, a| {
            if let (Some(s), Some(a)) = (s.as_ref(), a.as_ref())
                && let Ok(r) = s.cast::<RichEditBox>()
            {
                pressed(id, &r, a)?;
            }
            Ok(())
        })),
        true,
    )?;
    Ok(())
}

/// A copy (or cut) in field `id`: reported to the app when it has the
/// hook and something is selected (`false`: the box copies).
fn copy(id: NodeId, r: &RichEditBox, cut: bool) -> Result<bool> {
    if mask(id) & if cut { CUT } else { COPY } == 0 {
        return Ok(false);
    }
    let (sel, text) = selection(r)?;
    if sel.is_caret() {
        return Ok(false);
    }
    let copied: String = text
        .chars()
        .skip(sel.start)
        .take(sel.end - sel.start)
        .collect();
    let e = UiEvent::Copy(id, cut, sel.start, sel.end, copied);
    trace(&e);
    dispatch(e);
    Ok(true)
}

/// A paste in field `id`: the clipboard goes to the app (`true`), which
/// answers with `insert_text` unless it takes it.
pub fn paste(id: NodeId, r: &RichEditBox) -> Result<bool> {
    if mask(id) & PASTE == 0 {
        return Ok(false);
    }
    let (sel, _) = selection(r)?;
    let item = crate::clipboard::read_item(main_hwnd());
    let text = item.string(ClipboardItem::TEXT).unwrap_or_default();
    let html = item.string(ClipboardItem::HTML);
    let types: Vec<String> = item.types().into_iter().map(String::from).collect();
    // Markdown and the apps' own text types (MIME names).
    let data: Vec<(String, String)> = types
        .iter()
        .filter(|t| {
            ![ClipboardItem::TEXT, ClipboardItem::HTML, ClipboardItem::RTF].contains(&t.as_str())
                && !t.starts_with("image/")
        })
        .filter_map(|t| Some((t.clone(), item.string(t)?)))
        .collect();
    let e = UiEvent::Paste(
        id,
        PasteEvent::new(sel, text)
            .with_html(html)
            .with_types(types)
            .with_data(data),
    );
    trace(&e);
    dispatch(e);
    Ok(true)
}

/// Writes `content` to the clipboard, every type as one item.
pub fn write_clipboard(content: &ClipboardContent) {
    let mut item = ClipboardItem::new();
    for (mime, data) in content.entries() {
        item = match mime {
            ClipboardItem::TEXT => item.text(data),
            ClipboardItem::HTML => item.html(data),
            other => item.custom(other, data.as_bytes().to_vec()),
        };
    }
    if let Err(e) = crate::clipboard::write_item(main_hwnd(), &item) {
        log!("clipboard: {e:?}");
    }
}

/// Inserts `text` over the selection like a paste, once Prism's patches
/// are applied (the box then reports the edit).
pub fn insert_text(r: &RichEditBox, text: &str) {
    let r = r.clone();
    let text = HSTRING::from(text);
    crate::motion::after(
        std::time::Duration::ZERO,
        Box::new(move || {
            let typed = (|| -> Result<()> { r.Document()?.Selection()?.TypeText(&text) })();
            if let Err(e) = typed {
                log!("insert text: {e}");
            }
        }),
    );
}

/// Where RichEdit's client coordinates start in the box: its text host
/// (`ContentElement`, inside the border and padding).
fn client_origin(r: &RichEditBox) -> (f32, f32) {
    let Ok(me) = r.cast::<UIElement>() else {
        return (0., 0.);
    };
    crate::carets::host_origin(
        &me,
        r.GetTemplateChild(&HSTRING::from("ContentElement")).ok(),
    )
}

/// The character under `(x, y)` (in the box), in characters.
pub fn offset_at(r: &RichEditBox, x: f32, y: f32) -> Option<usize> {
    let (ox, oy) = client_origin(r);
    let doc = r.Document().ok()?;
    let range = doc
        .GetRangeFromPoint(
            windows::Foundation::Point {
                X: x - ox,
                Y: y - oy,
            },
            PointOptions::ClientCoordinates,
        )
        .ok()?;
    let at = range.StartPosition().ok()?.max(0) as usize;
    let text = text_of(&doc).ok()?;
    Some(utf16_to_chars(&text, at))
}

fn pressed(id: NodeId, r: &RichEditBox, a: &PointerRoutedEventArgs) -> Result<()> {
    let m = mask(id);
    if m & (CLICK | SPAN_CLICK) == 0 {
        return Ok(());
    }
    let e: UIElement = r.cast()?;
    let p = a.GetCurrentPoint(&e)?;
    let primary = p
        .Properties()
        .and_then(|p| p.IsLeftButtonPressed())
        .unwrap_or(true);
    if !primary {
        return Ok(());
    }
    let (x, y) = p.Position().map(|p| (p.X, p.Y))?;
    let Some(at) = offset_at(r, x, y) else {
        return Ok(());
    };
    let modifiers = crate::pointer::modifiers(a);
    if m & CLICK != 0 {
        let e = UiEvent::FieldClick(
            id,
            ClickEvent {
                position: Some(Point { x, y }),
                modifiers,
                count: crate::events::count_press(id, a).unwrap_or(1),
                button: PointerButton::Primary,
                text_offset: Some(at),
            },
        );
        trace(&e);
        dispatch(e);
    }
    // Ctrl- or Alt-clicks (the Windows key is the shell's).
    if m & SPAN_CLICK != 0 && (modifiers.control || modifiers.alt) {
        let e = UiEvent::SpanClick(id, at, modifiers);
        trace(&e);
        dispatch(e);
    }
    Ok(())
}

/// Field `id`'s selection or text changed: its rectangle is reported on a
/// later turn, once, when it moved.
pub fn changed(id: NodeId, r: &RichEditBox) {
    if mask(id) & SELECTION_RECT == 0 {
        return;
    }
    let first = SCHEDULED.with(|s| s.borrow_mut().insert(id, r.clone()).is_none());
    if !first {
        return;
    }
    let Ok(queue) = DispatcherQueue::GetForCurrentThread() else {
        return;
    };
    let _ = queue.TryEnqueue(&DispatcherQueueHandler::new(move || {
        report_rect(id);
        Ok(())
    }));
}

fn report_rect(id: NodeId) {
    let Some(r) = SCHEDULED.with(|s| s.borrow_mut().remove(&id)) else {
        return;
    };
    if mask(id) & SELECTION_RECT == 0 {
        return;
    }
    let Ok((sel, rect)) = selection_rect(&r) else {
        return;
    };
    let key = (
        sel.start,
        sel.end,
        [rect.x, rect.y, rect.width, rect.height].map(|v| (v * 2.).round() as i32),
    );
    if REPORTED.with(|r| r.borrow_mut().insert(id, key)) != Some(key) {
        let e = UiEvent::SelectionRect(id, sel.start, sel.end, rect);
        trace(&e);
        dispatch(e);
    }
}

/// The caret's rectangle at UTF-16 position `at`: its left edge, top and
/// line height, in the box.
fn caret_at(doc: &RichEditTextDocument, at: i32, origin: (f32, f32)) -> Result<(f32, f32, f32)> {
    let range: ITextRange = doc.GetRange(at, at)?.cast()?;
    let mut rect = windows::Foundation::Rect::default();
    let mut hit = 0;
    range.GetRect(PointOptions::ClientCoordinates, &mut rect, &mut hit)?;
    Ok((rect.X + origin.0, rect.Y + origin.1, rect.Height.max(1.)))
}

/// The selection, and the caret's box (one point wide) or the box around
/// the selected text (on several lines, the whole width), in the field.
fn selection_rect(r: &RichEditBox) -> Result<(SelectionEvent, Rect)> {
    let doc = r.Document()?;
    let sel = doc.Selection()?;
    let (a, b) = (sel.StartPosition()?, sel.EndPosition()?);
    let origin = client_origin(r);
    let (x0, y0, h0) = caret_at(&doc, a, origin)?;
    let rect = if a == b {
        Rect::new(x0, y0, 1., h0)
    } else {
        let (x1, y1, h1) = caret_at(&doc, b, origin)?;
        if (y1 - y0).abs() < 0.5 {
            Rect::new(x0, y0, (x1 - x0).max(1.), h0.max(h1))
        } else {
            let fe: FrameworkElement = r.cast()?;
            Rect::new(0., y0, fe.ActualWidth()? as f32, y1 + h1 - y0)
        }
    };
    let (sel, _) = selection(r)?;
    Ok((sel, rect))
}
