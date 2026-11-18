//! A rich input's editing hooks (#217) in AppKit: the selection's
//! rectangle, the app's copy, cut and paste through `NSPasteboard`, and
//! ⌘- / ⌥-clicks on its runs.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use objc2::DefinedClass;
use objc2::rc::Retained;
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSPasteboard, NSTextView, NSView};
use objc2_foundation::{NSCopying, NSPoint, NSRange, NSRect, NSSize, NSString};
use prism_core::edit_events::{CLICK, COPY, CUT, PASTE, SELECTION_RECT, SPAN_CLICK};
use prism_core::{ClipboardContent, NodeId, PasteEvent, Rect, UiEvent};

use crate::imp::PrismTextView;

/// The Markdown pasteboard type (#217).
const MARKDOWN_UTI: &str = "net.daringfireball.markdown";

/// The pasteboard type for MIME type `mime`.
fn pasteboard_type(mime: &str) -> Retained<NSString> {
    match mime {
        // SAFETY: AppKit's constant strings.
        prism_core::text_edit::TEXT_PLAIN => {
            unsafe { objc2_app_kit::NSPasteboardTypeString }.copy()
        }
        prism_core::text_edit::TEXT_HTML => unsafe { objc2_app_kit::NSPasteboardTypeHTML }.copy(),
        prism_core::text_edit::TEXT_MARKDOWN => NSString::from_str(MARKDOWN_UTI),
        other => NSString::from_str(other),
    }
}

/// The MIME type of pasteboard type `ty` (as it is when it has none).
fn mime_type(ty: &str) -> String {
    match ty {
        "public.utf8-plain-text" | "public.plain-text" | "NSStringPboardType" => "text/plain",
        "public.html" | "Apple HTML pasteboard type" => "text/html",
        MARKDOWN_UTI => "text/markdown",
        "public.rtf" => "text/rtf",
        "public.png" => "image/png",
        "public.jpeg" => "image/jpeg",
        "public.tiff" => "image/tiff",
        "com.compuserve.gif" => "image/gif",
        "public.file-url" => "text/uri-list",
        other => other,
    }
    .to_string()
}

/// Writes `content` to the general pasteboard, every type as one item.
pub(crate) fn write_clipboard(content: &ClipboardContent) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    for (mime, data) in content.entries() {
        pb.setString_forType(&NSString::from_str(data), &pasteboard_type(mime));
    }
}

/// What the general pasteboard holds, as a paste over `tv`'s selection.
fn paste_event(tv: &NSTextView) -> PasteEvent {
    let pb = NSPasteboard::generalPasteboard();
    let read = |ty: &NSString| pb.stringForType(ty).map(|s| s.to_string());
    // SAFETY: AppKit's constant strings.
    let text = read(unsafe { objc2_app_kit::NSPasteboardTypeString }).unwrap_or_default();
    let html = read(unsafe { objc2_app_kit::NSPasteboardTypeHTML });
    let mut types: Vec<String> = Vec::new();
    let mut data = Vec::new();
    for ty in pb.types().map(|a| a.to_vec()).unwrap_or_default() {
        let name = ty.to_string();
        let mime = mime_type(&name);
        // Markdown and the apps' own types (MIME-like names) as text.
        if (mime == "text/markdown" || (mime.contains('/') && mime == name))
            && !data.iter().any(|(m, _): &(String, String)| *m == mime)
            && let Some(s) = read(&ty)
        {
            data.push((mime.clone(), s));
        }
        if !types.contains(&mime) {
            types.push(mime);
        }
    }
    PasteEvent::new(crate::interact::text_selection(tv), text)
        .with_html(html)
        .with_types(types)
        .with_data(data)
}

fn mask(tv: &PrismTextView) -> u8 {
    tv.ivars().edit_mask.get()
}

/// `copy:` / `cut:` (#217): reported to the app when the field has the
/// hook and something is selected; `false` lets AppKit copy.
pub(crate) fn copy(tv: &PrismTextView, cut: bool) -> bool {
    if mask(tv) & if cut { CUT } else { COPY } == 0 {
        return false;
    }
    let range = tv.selectedRange();
    let Some(id) = crate::interact::text_node(tv) else {
        return false;
    };
    if range.length == 0 {
        return false;
    }
    let text = tv.string().substringWithRange(range).to_string();
    let sel = crate::interact::text_selection(tv);
    crate::imp::dispatch_or_defer(UiEvent::Copy(id, cut, sel.start, sel.end, text));
    true
}

/// `paste:` (#217): the pasteboard goes to the app, which answers with
/// `insert_text` unless it takes the paste.
pub(crate) fn paste(tv: &PrismTextView) -> bool {
    if mask(tv) & PASTE == 0 {
        return false;
    }
    let Some(id) = crate::interact::text_node(tv) else {
        return false;
    };
    crate::imp::dispatch_or_defer(UiEvent::Paste(id, paste_event(tv)));
    true
}

/// Inserts `text` over the selection like a typed or pasted text, on the
/// next turn: AppKit then reports the edit (`textDidChange:`), which it
/// cannot while Prism applies patches.
pub(crate) fn insert_text(tv: &PrismTextView, text: &str) {
    let tv: Retained<PrismTextView> = objc2::Message::retain(tv);
    let text = NSString::from_str(text);
    crate::imp::on_main_after(std::time::Duration::ZERO, move || {
        let range = tv.selectedRange();
        if tv.shouldChangeTextInRange_replacementString(range, Some(&text)) {
            // SAFETY: an `NSString` is a valid `insertText:` argument.
            unsafe {
                objc2_app_kit::NSTextInputClient::insertText_replacementRange(&**tv, &text, range)
            };
        }
    });
}

/// The character under `event`'s pointer in `tv`.
fn char_at(tv: &PrismTextView, event: &NSEvent) -> Option<usize> {
    let (Some(lm), Some(tc)) = (unsafe { tv.layoutManager() }, unsafe { tv.textContainer() })
    else {
        return None;
    };
    let p = tv.convertPoint_fromView(event.locationInWindow(), None);
    let origin = tv.textContainerOrigin();
    let local = NSPoint::new(p.x - origin.x, p.y - origin.y);
    // SAFETY: a null fraction pointer is allowed.
    let index = unsafe {
        lm.characterIndexForPoint_inTextContainer_fractionOfDistanceBetweenInsertionPoints(
            local,
            &tc,
            std::ptr::null_mut(),
        )
    };
    let text = tv.string().to_string();
    Some(prism_native::fields::utf16_to_chars(&text, index))
}

thread_local! {
    /// The selection groups (#231): containers whose fields share one
    /// selection.
    static GROUPS: RefCell<HashSet<NodeId>> = RefCell::new(HashSet::new());
}

/// Node `id` is (`on`) a selection group or not.
pub(crate) fn set_group(id: NodeId, on: bool) {
    GROUPS.with(|g| {
        let mut g = g.borrow_mut();
        if on {
            g.insert(id);
        } else {
            g.remove(&id);
        }
    });
}

/// The selection group `view` is in, if any: its nearest ancestor marked
/// as one.
fn group_of(view: &NSView) -> Option<NodeId> {
    let mut v = unsafe { view.superview() };
    while let Some(view) = v {
        if let Some(p) = view.downcast_ref::<crate::imp::PrismView>() {
            let id = p.ivars().id.get();
            if GROUPS.with(|g| g.borrow().contains(&id)) {
                return Some(id);
            }
        }
        v = unsafe { view.superview() };
    }
    None
}

/// The text field of `group` under `at` (window coordinates), and the
/// character under it.
fn field_at(
    window: &objc2_app_kit::NSWindow,
    event: &NSEvent,
    group: NodeId,
) -> Option<(Retained<PrismTextView>, NodeId, usize)> {
    let content = window.contentView()?;
    let mut v = content.hitTest(event.locationInWindow());
    while let Some(view) = v {
        if let Ok(tv) = view.clone().downcast::<PrismTextView>() {
            if group_of(&tv) != Some(group) {
                return None;
            }
            let id = crate::interact::text_node(&tv)?;
            let at = char_at(&tv, event)?;
            return Some((tv, id, at));
        }
        v = unsafe { view.superview() };
    }
    None
}

/// A drag from a field of a selection group (#264): the field follows the
/// mouse inside itself; over another field of the group, the selection
/// across them is reported (`UiEvent::SelectionAcross`), which the runtime
/// answers with the selection and the highlights to draw.
fn track_across(tv: &PrismTextView, event: &NSEvent, group: NodeId) -> bool {
    let (Some(window), Some(field), Some(anchor)) = (
        tv.window(),
        crate::interact::text_node(tv),
        char_at(tv, event),
    ) else {
        return false;
    };
    window.makeFirstResponder(Some(tv));
    crate::interact::select_text(tv, anchor, anchor);
    let mut across = false;
    let mask =
        objc2_app_kit::NSEventMask::LeftMouseDragged | objc2_app_kit::NSEventMask::LeftMouseUp;
    while let Some(e) = window.nextEventMatchingMask(mask) {
        if e.r#type() == objc2_app_kit::NSEventType::LeftMouseUp {
            break;
        }
        match field_at(&window, &e, group) {
            Some((_, other, at)) if other != field => {
                crate::imp::dispatch(UiEvent::SelectionAcross(
                    group,
                    Some(((field, anchor), (other, at))),
                ));
                across = true;
            }
            // Inside the field (or outside every field): within it.
            _ => {
                let at = char_at(tv, &e).unwrap_or(anchor);
                if across {
                    crate::imp::dispatch(UiEvent::SelectionAcross(
                        group,
                        Some(((field, anchor), (field, at))),
                    ));
                    across = false;
                }
                crate::interact::select_text(tv, anchor.min(at), anchor.max(at));
            }
        }
    }
    true
}

/// A click in the field. With `on_click` (#266), reported with the
/// character under it, the field still placing its caret. With ⌘ or ⌥
/// (#217): reported as a click on the character under it instead of
/// placing the caret (⌘-click would add a selection); ⌃-click stays the
/// context menu.
pub(crate) fn mouse_down(tv: &PrismTextView, event: &NSEvent) -> bool {
    let flags = event.modifierFlags();
    let Some(id) = crate::interact::text_node(tv) else {
        return false;
    };
    // A plain press in a field of a selection group: a drag may cross into
    // the next fields (#264).
    let plain = !flags.intersects(
        NSEventModifierFlags::Command
            | NSEventModifierFlags::Option
            | NSEventModifierFlags::Control
            | NSEventModifierFlags::Shift,
    );
    let group = (plain && event.clickCount() == 1)
        .then(|| group_of(tv))
        .flatten();
    if mask(tv) & CLICK != 0
        && let Some(at) = char_at(tv, event)
    {
        let p = tv.convertPoint_fromView(event.locationInWindow(), None);
        crate::imp::dispatch_or_defer(UiEvent::FieldClick(
            id,
            prism_core::ClickEvent {
                position: Some(prism_core::Point::new(p.x as f32, p.y as f32)),
                modifiers: crate::interact::modifiers(flags),
                count: event.clickCount().max(1) as u32,
                button: prism_core::PointerButton::Primary,
                text_offset: Some(at),
            },
        ));
    }
    if let Some(group) = group {
        return track_across(tv, event, group);
    }
    if mask(tv) & SPAN_CLICK == 0
        || !flags.intersects(NSEventModifierFlags::Command | NSEventModifierFlags::Option)
    {
        return false;
    }
    let Some(at) = char_at(tv, event) else {
        return false;
    };
    crate::imp::dispatch_or_defer(UiEvent::SpanClick(
        id,
        at,
        crate::interact::modifiers(flags),
    ));
    true
}

/// A reported rectangle: the selection, then the box in half points.
type Reported = (usize, usize, [i32; 4]);

thread_local! {
    /// The rectangle last reported for each field, and the fields whose
    /// report is scheduled.
    static REPORTED: RefCell<HashMap<NodeId, Reported>> = RefCell::new(HashMap::new());
    static SCHEDULED: RefCell<HashSet<NodeId>> = RefCell::new(HashSet::new());
}

/// Whether `tv` reports its selection's rectangle.
pub(crate) fn reports_rect(tv: &NSTextView) -> bool {
    tv.downcast_ref::<PrismTextView>()
        .is_some_and(|t| mask(t) & SELECTION_RECT != 0)
}

/// `tv`'s selection or text changed: its rectangle is reported on the next
/// turn (after the edit and the selection), once, when it moved.
pub(crate) fn rect_changed(tv: &NSTextView, id: NodeId) {
    if !reports_rect(tv) || !SCHEDULED.with(|s| s.borrow_mut().insert(id)) {
        return;
    }
    let tv: Retained<NSTextView> = objc2::Message::retain(tv);
    crate::imp::on_main_after(std::time::Duration::ZERO, move || {
        SCHEDULED.with(|s| s.borrow_mut().remove(&id));
        if crate::interact::text_node(&tv) != Some(id) || !reports_rect(&tv) {
            return;
        }
        let Some(rect) = selection_rect(&tv) else {
            return;
        };
        let sel = crate::interact::text_selection(&tv);
        let key = (
            sel.start,
            sel.end,
            [rect.x, rect.y, rect.width, rect.height].map(|v| (v * 2.).round() as i32),
        );
        if REPORTED.with(|r| r.borrow_mut().insert(id, key)) != Some(key) {
            crate::imp::dispatch(UiEvent::SelectionRect(id, sel.start, sel.end, rect));
        }
    });
}

/// Field `id` stopped reporting (or is gone).
pub(crate) fn forget(id: NodeId) {
    REPORTED.with(|r| r.borrow_mut().remove(&id));
}

/// The caret's or the selection's box in the field's view (the text
/// area's container): the selected glyphs' bounding box, or the caret's
/// line box one point wide.
fn selection_rect(tv: &NSTextView) -> Option<Rect> {
    // SAFETY: the text area's scroll view sits in its container.
    let field: Retained<NSView> = unsafe { tv.enclosingScrollView()?.superview() }?;
    let range = tv.selectedRange();
    let in_view = if range.length > 0 {
        let (lm, tc) = (unsafe { tv.layoutManager() }?, unsafe {
            tv.textContainer()
        }?);
        // SAFETY: a null `actualCharacterRange` is allowed.
        let glyphs = unsafe {
            lm.glyphRangeForCharacterRange_actualCharacterRange(range, std::ptr::null_mut())
        };
        let r = lm.boundingRectForGlyphRange_inTextContainer(glyphs, &tc);
        let o = tv.textContainerOrigin();
        NSRect::new(NSPoint::new(r.origin.x + o.x, r.origin.y + o.y), r.size)
    } else {
        // SAFETY: a null `actualRange` is allowed.
        let screen = unsafe {
            objc2_app_kit::NSTextInputClient::firstRectForCharacterRange_actualRange(
                tv,
                NSRange::new(range.location, 0),
                std::ptr::null_mut(),
            )
        };
        let window = tv.window()?;
        let r = tv.convertRect_fromView(window.convertRectFromScreen(screen), None);
        NSRect::new(r.origin, NSSize::new(r.size.width.max(1.), r.size.height))
    };
    let r = field.convertRect_fromView(in_view, Some(tv));
    // The field's view is flipped (Prism's): y grows downwards.
    let y = if field.isFlipped() {
        r.origin.y
    } else {
        field.bounds().size.height - r.origin.y - r.size.height
    };
    Some(Rect::new(
        r.origin.x as f32,
        y as f32,
        r.size.width as f32,
        r.size.height as f32,
    ))
}
