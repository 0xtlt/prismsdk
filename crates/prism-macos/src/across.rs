//! One selection across several text fields (#231) on macOS: where a mouse
//! drag is over the fields of a selection group (`crate::edit` tracks it,
//! #264), and a one-line input's highlighted part.
//!
//! The keys go through the key-down monitor (`Runtime::consumes_key_event`);
//! text areas and rich inputs draw their part with a temporary attribute of
//! their layout manager (`crate::interact::highlight_text`).

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAppearanceCustomization, NSColor, NSTextField, NSTextView, NSUserInterfaceItemIdentification,
    NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use prism_core::NodeId;

use crate::carets::FieldLine;
use crate::imp::GroupField;

/// The identifier of the view drawing a one-line input's part.
const HIGHLIGHT: &str = "prism-across";

/// The character of `tv` nearest to window point `p` (clamped to its text).
pub(crate) fn offset_in(tv: &NSTextView, p: NSPoint) -> usize {
    let i = tv.characterIndexForInsertionAtPoint(tv.convertPoint_fromView(p, None));
    prism_native::fields::utf16_to_chars(&tv.string().to_string(), i)
}

/// Whether window point `p` is in `view`.
fn contains(view: &NSView, p: NSPoint) -> bool {
    if view.isHiddenOrHasHiddenAncestor() {
        return false;
    }
    let l = view.convertPoint_fromView(p, None);
    let b = view.bounds();
    l.x >= b.origin.x
        && l.x < b.origin.x + b.size.width
        && l.y >= b.origin.y
        && l.y < b.origin.y + b.size.height
}

/// The field of the group under window point `p` (its whole box: padding
/// and a one-line input included), and the character there.
pub(crate) fn field_at(fields: &[(NodeId, GroupField)], p: NSPoint) -> Option<(NodeId, usize)> {
    fields.iter().find_map(|(id, f)| match f {
        GroupField::Area(tv, frame) => contains(frame, p).then(|| (*id, offset_in(tv, p))),
        GroupField::Input(field, frame) => contains(frame, p).then(|| {
            let editor = field
                .currentEditor()
                .and_then(|e| e.downcast::<NSTextView>().ok());
            let to = match editor {
                Some(tv) => offset_in(&tv, p),
                None => FieldLine::of(field).map_or(0, |line| {
                    line.offset_at(field.convertPoint_fromView(p, None).x)
                }),
            };
            (*id, to)
        }),
    })
}

/// Draws characters `start..end` of a one-line input like a selection
/// while another field has the focus (#231): a tinted box over its text,
/// measured with its font. `start == end` removes it.
pub(crate) fn highlight_field(field: &NSTextField, start: usize, end: usize) {
    for sub in field.subviews().to_vec() {
        if sub.identifier().is_some_and(|i| i.to_string() == HIGHLIGHT) {
            sub.removeFromSuperview();
        }
    }
    let Some(line) = (start < end).then(|| FieldLine::of(field)).flatten() else {
        return;
    };
    let (a, b) = (line.width(start), line.width(end));
    let frame = NSRect::new(
        NSPoint::new(line.x0 + a, line.y),
        NSSize::new((b - a).max(1.), line.h),
    );
    let v: Retained<NSView> = NSView::initWithFrame(NSView::alloc(field.mtm()), frame);
    v.setIdentifier(Some(&NSString::from_str(HIGHLIGHT)));
    v.setWantsLayer(true);
    // Over the text, blended so it reads as the selection colour behind
    // it: multiplied on a light field, screened on a dark one.
    let dark = field
        .effectiveAppearance()
        .bestMatchFromAppearancesWithNames(&objc2_foundation::NSArray::from_slice(&[
            unsafe { objc2_app_kit::NSAppearanceNameAqua },
            unsafe { objc2_app_kit::NSAppearanceNameDarkAqua },
        ]))
        .is_some_and(|n| n.to_string().contains("Dark"));
    let color = NSColor::selectedTextBackgroundColor();
    if let Some(layer) = v.layer() {
        layer.setBackgroundColor(Some(&color.CGColor()));
        let filter = NSString::from_str(if dark {
            "screenBlendMode"
        } else {
            "multiplyBlendMode"
        });
        unsafe { layer.setCompositingFilter(Some(&filter)) };
    }
    field.addSubview(&v);
}
