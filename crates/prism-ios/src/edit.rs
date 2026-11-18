//! A rich input's editing hooks (#217) in UIKit: the selection's
//! rectangle, the app's copy, cut and paste through `UIPasteboard`. A rich
//! input is a plain `UITextView` here ([`PrismTextView`]); tvOS has no text
//! area and no pasteboard.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSArray, NSDictionary, NSObjectProtocol, NSString};
use objc2_ui_kit::{UIKeyInput, UIPasteboard, UITextInput, UITextView, UIView};
use prism_core::edit_events::{COPY, CUT, PASTE, SELECTION_RECT};
use prism_core::{ClipboardContent, NodeId, PasteEvent, Rect, UiEvent};

/// A reported rectangle: the selection, then the box in half points.
type Reported = (usize, usize, [i32; 4]);

thread_local! {
    /// The editing events each field reports.
    static MASKS: RefCell<HashMap<NodeId, u8>> = RefCell::new(HashMap::new());
    /// The rectangle last reported for each field, and the fields whose
    /// report is scheduled.
    static REPORTED: RefCell<HashMap<NodeId, Reported>> = RefCell::new(HashMap::new());
    static SCHEDULED: RefCell<HashMap<NodeId, Retained<UITextView>>> = RefCell::new(HashMap::new());
    /// Texts to insert on the next turn, with their field.
    static INSERTS: RefCell<Vec<(Retained<UITextView>, String)>> = const { RefCell::new(Vec::new()) };
}

fn mask(id: NodeId) -> u8 {
    MASKS.with(|m| m.borrow().get(&id).copied().unwrap_or(0))
}

/// Which editing events field `id` reports (`Widgets::set_edit_events`).
pub fn set_mask(view: &UITextView, mask: u8) {
    let id = view.tag() as NodeId;
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
        rect_changed(view);
    }
}

/// Field `id` is gone.
pub fn forget(id: NodeId) {
    MASKS.with(|m| m.borrow_mut().remove(&id));
    REPORTED.with(|r| r.borrow_mut().remove(&id));
}

define_class!(
    /// The text view of a text area: copy, cut and paste go to the app when
    /// it has the hooks (#217).
    #[unsafe(super(UITextView, objc2_ui_kit::UIScrollView, UIView, objc2_ui_kit::UIResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismTextAreaView"]
    pub struct PrismTextView;

    unsafe impl NSObjectProtocol for PrismTextView {}

    impl PrismTextView {
        #[unsafe(method(copy:))]
        fn copy(&self, sender: Option<&AnyObject>) {
            if !report_copy(self, false) {
                let _: () = unsafe { msg_send![super(self), copy: sender] };
            }
        }

        #[unsafe(method(cut:))]
        fn cut(&self, sender: Option<&AnyObject>) {
            if report_copy(self, true) {
                self.deleteBackward();
            } else {
                let _: () = unsafe { msg_send![super(self), cut: sender] };
            }
        }

        #[unsafe(method(paste:))]
        fn paste(&self, sender: Option<&AnyObject>) {
            let id = self.tag() as NodeId;
            if mask(id) & PASTE != 0 {
                crate::imp::dispatch(UiEvent::Paste(id, paste_event(self)));
            } else {
                let _: () = unsafe { msg_send![super(self), paste: sender] };
            }
        }
    }
);

impl PrismTextView {
    /// A new text view.
    pub fn text_view(mtm: MainThreadMarker) -> Retained<UITextView> {
        let this = Self::alloc(mtm).set_ivars(());
        let view: Retained<Self> = unsafe { msg_send![super(this), init] };
        Retained::into_super(view)
    }
}

/// A copy or a cut the app hears: `false` lets UIKit copy.
fn report_copy(tv: &UITextView, cut: bool) -> bool {
    let id = tv.tag() as NodeId;
    if mask(id) & if cut { CUT } else { COPY } == 0 {
        return false;
    }
    let Some(sel) = crate::forms::current_selection(tv) else {
        return false;
    };
    if sel.is_caret() {
        return false;
    }
    let text = tv
        .selectedTextRange()
        .and_then(|r| tv.textInRange(&r))
        .map(|s| s.to_string())
        .unwrap_or_default();
    crate::imp::dispatch(UiEvent::Copy(id, cut, sel.start, sel.end, text));
    true
}

/// The pasteboard type for MIME type `mime`.
fn pasteboard_type(mime: &str) -> &str {
    match mime {
        prism_core::text_edit::TEXT_PLAIN => "public.utf8-plain-text",
        prism_core::text_edit::TEXT_HTML => "public.html",
        prism_core::text_edit::TEXT_MARKDOWN => "net.daringfireball.markdown",
        other => other,
    }
}

/// The MIME type of pasteboard type `ty` (as it is when it has none).
fn mime_type(ty: &str) -> &str {
    match ty {
        "public.utf8-plain-text" | "public.plain-text" | "public.text" => "text/plain",
        "public.html" => "text/html",
        "net.daringfireball.markdown" => "text/markdown",
        "public.rtf" => "text/rtf",
        "public.png" => "image/png",
        "public.jpeg" => "image/jpeg",
        "public.heic" => "image/heic",
        "public.url" => "text/uri-list",
        other => other,
    }
}

/// What the general pasteboard holds, as a paste over `tv`'s selection.
fn paste_event(tv: &UITextView) -> PasteEvent {
    let pb = UIPasteboard::generalPasteboard();
    let read = |ty: &str| -> Option<String> {
        let v = pb.valueForPasteboardType(&NSString::from_str(ty))?;
        if let Some(s) = v.downcast_ref::<NSString>() {
            return Some(s.to_string());
        }
        let data = v.downcast_ref::<objc2_foundation::NSData>()?;
        String::from_utf8(data.to_vec()).ok()
    };
    let text = unsafe { pb.string() }
        .map(|s| s.to_string())
        .unwrap_or_default();
    let html = read("public.html");
    let mut types: Vec<String> = Vec::new();
    let mut data = Vec::new();
    for ty in unsafe { pb.pasteboardTypes() }.to_vec() {
        let name = ty.to_string();
        let mime = mime_type(&name).to_string();
        if (mime == "text/markdown" || (mime.contains('/') && mime == name))
            && !data.iter().any(|(m, _): &(String, String)| *m == mime)
            && let Some(s) = read(&name)
        {
            data.push((mime.clone(), s));
        }
        if !types.contains(&mime) {
            types.push(mime);
        }
    }
    let selection = crate::forms::current_selection(tv).unwrap_or_default();
    PasteEvent::new(selection, text)
        .with_html(html)
        .with_types(types)
        .with_data(data)
}

/// Writes `content` to the general pasteboard, every type as one item.
pub fn write_clipboard(content: &ClipboardContent) {
    let entries = content.entries();
    let keys: Vec<Retained<NSString>> = entries
        .iter()
        .map(|(m, _)| NSString::from_str(pasteboard_type(m)))
        .collect();
    let values: Vec<Retained<NSString>> =
        entries.iter().map(|(_, d)| NSString::from_str(d)).collect();
    let keys: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<&AnyObject> = values.iter().map(|v| v.as_ref()).collect();
    let item = NSDictionary::from_slices(&keys, &values);
    // SAFETY: string values for string types.
    unsafe { UIPasteboard::generalPasteboard().setItems(&NSArray::from_retained_slice(&[item])) };
}

/// Inserts `text` over the selection like typed text, on the next turn
/// (UIKit then reports the edit, which it cannot while Prism applies
/// patches).
pub fn insert_text(tv: &UITextView, text: &str) {
    INSERTS.with(|i| {
        i.borrow_mut()
            .push((objc2::Message::retain(tv), text.to_string()))
    });
    dispatch2::DispatchQueue::main().exec_async(|| {
        for (tv, text) in INSERTS.with(|i| std::mem::take(&mut *i.borrow_mut())) {
            tv.insertText(&NSString::from_str(&text));
        }
    });
}

/// `tv`'s selection or text changed: its rectangle is reported on the next
/// turn, once, when it moved.
pub fn rect_changed(tv: &UITextView) {
    let id = tv.tag() as NodeId;
    if mask(id) & SELECTION_RECT == 0 {
        return;
    }
    let first = SCHEDULED.with(|s| {
        s.borrow_mut()
            .insert(id, objc2::Message::retain(tv))
            .is_none()
    });
    if !first {
        return;
    }
    dispatch2::DispatchQueue::main().exec_async(move || {
        let Some(tv) = SCHEDULED.with(|s| s.borrow_mut().remove(&id)) else {
            return;
        };
        if mask(id) & SELECTION_RECT == 0 {
            return;
        }
        let (Some(sel), Some(rect)) = (crate::forms::current_selection(&tv), selection_rect(&tv))
        else {
            return;
        };
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

/// The caret's box (one point wide) or the union of the selection's line
/// boxes, in the field's visible coordinates (its scrolling taken off).
fn selection_rect(tv: &UITextView) -> Option<Rect> {
    let range = tv.selectedTextRange()?;
    let r = if range.isEmpty() {
        let c = tv.caretRectForPosition(&range.start());
        objc2_core_foundation::CGRect::new(
            c.origin,
            objc2_core_foundation::CGSize::new(c.size.width.max(1.), c.size.height),
        )
    } else {
        let rects = tv.selectionRectsForRange(&range);
        let mut out: Option<objc2_core_foundation::CGRect> = None;
        for s in rects.to_vec() {
            let r = s.rect();
            if r.size.width <= 0. && r.size.height <= 0. {
                continue;
            }
            out = Some(match out {
                None => r,
                Some(o) => {
                    let x0 = o.origin.x.min(r.origin.x);
                    let y0 = o.origin.y.min(r.origin.y);
                    let x1 = (o.origin.x + o.size.width).max(r.origin.x + r.size.width);
                    let y1 = (o.origin.y + o.size.height).max(r.origin.y + r.size.height);
                    objc2_core_foundation::CGRect::new(
                        objc2_core_foundation::CGPoint::new(x0, y0),
                        objc2_core_foundation::CGSize::new(x1 - x0, y1 - y0),
                    )
                }
            });
        }
        out?
    };
    let offset = tv.bounds().origin;
    Some(Rect::new(
        (r.origin.x - offset.x) as f32,
        (r.origin.y - offset.y) as f32,
        r.size.width as f32,
        r.size.height as f32,
    ))
}
