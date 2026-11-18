//! Other people's carets and selections in text fields (#240), on iOS,
//! iPadOS and visionOS (tvOS edits text in its full-screen keyboard: none).
//!
//! A view that takes no touches, laid over the field as its subview: in a
//! text view (text areas, rich inputs) it scrolls with the text; in a text
//! field its name tags may stick out above it. Its content is laid out
//! with the field's own `UITextInput` geometry (`caretRectForPosition:`,
//! `selectionRectsForRange:`) each time the field changes; a text field
//! that is not being edited measures its text with its font. Bars and
//! tinted boxes are plain views; each caret's tag is a label kept across
//! updates, so it fades out when the caret goes idle. The field's text,
//! selection and marked text are never touched.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, Message};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSDictionary, NSString};
use objc2_ui_kit::{
    NSObjectUIAccessibility, NSStringDrawing, NSTextAlignment, UIColor, UIFont,
    UIFontWeightSemibold, UILabel, UITextField, UITextInput, UITextView, UIView,
};
use prism_core::{NodeId, RemoteCaret};
use prism_native::fields::chars_to_utf16;

use crate::imp::ui_color;

struct Overlay {
    root: Retained<UIView>,
    field: Weak<UIView>,
    carets: Vec<RemoteCaret>,
    /// One tag per caret, kept so an idle one fades.
    tags: Vec<Retained<UILabel>>,
}

thread_local! {
    static OVERLAYS: RefCell<HashMap<NodeId, Overlay>> = RefCell::new(HashMap::new());
}

/// Shows `carets` in text field or text view `field` (none: removes them).
pub(crate) fn set(mtm: MainThreadMarker, id: NodeId, field: &UIView, carets: &[RemoteCaret]) {
    if carets.is_empty() {
        forget(id);
        return;
    }
    OVERLAYS.with(|o| {
        let mut o = o.borrow_mut();
        let overlay = o.entry(id).or_insert_with(|| {
            let root = UIView::new(mtm);
            root.setUserInteractionEnabled(false);
            // VoiceOver reads the field, not the tags drawn over it.
            root.setIsAccessibilityElement(false, mtm);
            root.setAccessibilityElementsHidden(true, mtm);
            Overlay {
                root,
                field: Weak::default(),
                carets: Vec::new(),
                tags: Vec::new(),
            }
        });
        overlay.field = Weak::from(field);
        if overlay.root.superview().as_deref() != Some(field) {
            overlay.root.removeFromSuperview();
            field.addSubview(&overlay.root);
        }
        let font = UIFont::systemFontOfSize_weight(11., unsafe { UIFontWeightSemibold });
        overlay.tags.truncate(carets.len());
        while overlay.tags.len() < carets.len() {
            let tag = UILabel::new(mtm);
            unsafe {
                tag.setFont(Some(&font));
                tag.setTextColor(Some(&UIColor::whiteColor()));
            }
            tag.setTextAlignment(NSTextAlignment::Center);
            tag.layer().setCornerRadius(3.);
            tag.layer().setMasksToBounds(true);
            overlay.tags.push(tag);
        }
        for (i, c) in carets.iter().enumerate() {
            let tag = &overlay.tags[i];
            tag.setText(c.label.as_deref().map(NSString::from_str).as_deref());
            tag.setBackgroundColor(Some(&ui_color(c.color)));
            let was_idle = overlay.carets.get(i).is_some_and(|o| o.idle);
            if c.idle && !was_idle {
                let fading = tag.retain();
                let block = block2::RcBlock::new(move || fading.setAlpha(0.));
                UIView::animateWithDuration_animations(0.3, &block, mtm);
            } else if !c.idle {
                tag.setAlpha(1.);
            }
        }
        overlay.carets = carets.to_vec();
    });
    refresh(id);
}

/// Field `id` changed (its text, selection, frame): its carets follow.
pub(crate) fn refresh(id: NodeId) {
    OVERLAYS.with(|o| {
        if let Some(overlay) = o.borrow().get(&id) {
            layout(overlay);
        }
    });
}

/// Field `id` is gone or shows no carets.
pub(crate) fn forget(id: NodeId) {
    if let Some(o) = OVERLAYS.with(|o| o.borrow_mut().remove(&id)) {
        o.root.removeFromSuperview();
    }
}

/// A caret's bar and its selection's boxes, in the field's coordinates.
struct Shape {
    bar: Option<CGRect>,
    boxes: Vec<CGRect>,
}

/// From the field's own text layout (`UITextInput`), in `field`'s
/// coordinates (a text field's geometry is its field editor's).
fn with_text_input<T: UITextInput + Message>(
    f: &T,
    field: &UIView,
    text: &str,
    c: &RemoteCaret,
) -> Shape {
    let from = f.textInputView();
    let to_field = |r: CGRect| field.convertRect_fromView(r, Some(&from));
    let begin = f.beginningOfDocument();
    let at =
        |chars: usize| f.positionFromPosition_offset(&begin, chars_to_utf16(text, chars) as isize);
    let range = c.range();
    let boxes = match (at(range.start), at(range.end)) {
        (Some(a), Some(b)) if range.end > range.start => f
            .textRangeFromPosition_toPosition(&a, &b)
            .map(|r| {
                f.selectionRectsForRange(&r)
                    .iter()
                    .map(|s| s.rect())
                    .filter(|r| r.size.width > 0. && r.size.height > 0.)
                    .map(to_field)
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let bar = at(c.head)
        .map(|p| f.caretRectForPosition(&p))
        .filter(|r| r.size.height > 0. && r.origin.x.is_finite())
        .map(to_field);
    Shape { bar, boxes }
}

/// A text field not being edited: its text measured with its font.
fn in_field(f: &UITextField, text: &str, c: &RemoteCaret) -> Shape {
    let font = f
        .font()
        .unwrap_or_else(|| UIFont::systemFontOfSize(UIFont::systemFontSize()));
    let attrs = {
        let key: &NSString = unsafe { objc2_ui_kit::NSFontAttributeName };
        let value: &AnyObject = &font;
        NSDictionary::from_slices(&[key], &[value])
    };
    let width = |chars: usize| {
        let prefix: String = text.chars().take(chars).collect();
        unsafe { NSString::from_str(&prefix).sizeWithAttributes(Some(&attrs)) }.width
    };
    let rect = f.textRectForBounds(f.bounds());
    let full = width(usize::MAX);
    let x0 = rect.origin.x
        + match f.textAlignment() {
            NSTextAlignment::Center => ((rect.size.width - full) / 2.).max(0.),
            NSTextAlignment::Right => (rect.size.width - full).max(0.),
            _ => 0.,
        };
    let h = unsafe { font.lineHeight() }.ceil();
    let y = rect.origin.y + ((rect.size.height - h) / 2.).max(0.);
    let range = c.range();
    let (a, b) = (width(range.start), width(range.end));
    Shape {
        bar: Some(CGRect::new(
            CGPoint::new(x0 + width(c.head), y),
            CGSize::new(2., h),
        )),
        boxes: if b > a {
            vec![CGRect::new(CGPoint::new(x0 + a, y), CGSize::new(b - a, h))]
        } else {
            Vec::new()
        },
    }
}

fn intersect(a: CGRect, b: CGRect) -> Option<CGRect> {
    let x = a.origin.x.max(b.origin.x);
    let y = a.origin.y.max(b.origin.y);
    let r = (a.origin.x + a.size.width).min(b.origin.x + b.size.width);
    let t = (a.origin.y + a.size.height).min(b.origin.y + b.size.height);
    (r > x && t > y).then(|| CGRect::new(CGPoint::new(x, y), CGSize::new(r - x, t - y)))
}

fn layout(o: &Overlay) {
    let Some(field) = o.field.load() else { return };
    let mtm = MainThreadMarker::from(&*field);
    // The whole content of a text view (it scrolls with it), the bounds of
    // a field; what shows of the text clips the shapes.
    let (shapes, frame, clip): (Vec<Shape>, CGRect, CGRect) =
        if let Some(tv) = field.downcast_ref::<UITextView>() {
            let text = tv.text().to_string();
            let size = tv.contentSize();
            let b = tv.bounds();
            let frame = CGRect::new(
                CGPoint::new(0., 0.),
                CGSize::new(size.width.max(b.size.width), size.height.max(b.size.height)),
            );
            let shapes = o
                .carets
                .iter()
                .map(|c| with_text_input(tv, &field, &text, c))
                .collect();
            (shapes, frame, frame)
        } else if let Some(f) = field.downcast_ref::<UITextField>() {
            let text = f.text().map(|t| t.to_string()).unwrap_or_default();
            let shapes = o
                .carets
                .iter()
                .map(|c| {
                    if f.isEditing() {
                        with_text_input(f, &field, &text, c)
                    } else {
                        in_field(f, &text, c)
                    }
                })
                .collect();
            (shapes, f.bounds(), f.bounds())
        } else {
            return;
        };
    o.root.setFrame(frame);
    for v in o.root.subviews().iter() {
        let kept = o.tags.iter().any(|t| std::ptr::eq::<UIView>(&***t, &*v));
        if !kept {
            v.removeFromSuperview();
        }
    }
    for ((c, s), tag) in o.carets.iter().zip(shapes).zip(&o.tags) {
        let color = ui_color(c.color);
        let tint = color.colorWithAlphaComponent(0.25);
        for b in s.boxes.into_iter().filter_map(|b| intersect(b, clip)) {
            let v = UIView::initWithFrame(UIView::alloc(mtm), b);
            v.setBackgroundColor(Some(&tint));
            o.root.insertSubview_atIndex(&v, 0);
        }
        let bar = s.bar.filter(|b| {
            b.origin.x >= clip.origin.x - 1.
                && b.origin.x <= clip.origin.x + clip.size.width + 1.
                && b.origin.y + b.size.height > clip.origin.y
                && b.origin.y < clip.origin.y + clip.size.height
        });
        let Some(bar) = bar else {
            tag.removeFromSuperview();
            continue;
        };
        let line = UIView::initWithFrame(
            UIView::alloc(mtm),
            CGRect::new(
                CGPoint::new(bar.origin.x - 1., bar.origin.y),
                CGSize::new(2., bar.size.height),
            ),
        );
        line.setBackgroundColor(Some(&color));
        o.root.addSubview(&line);
        if c.label.is_none() {
            tag.removeFromSuperview();
            continue;
        }
        let size = tag.intrinsicContentSize();
        let (w, h) = ((size.width + 8.).ceil(), (size.height + 2.).ceil());
        // Above the caret's line; inside a text view, under it when the
        // line is at the top (a text view clips what sticks out).
        let above = bar.origin.y - h;
        let y = if field.downcast_ref::<UITextView>().is_some() && above < frame.origin.y {
            bar.origin.y + bar.size.height
        } else {
            above
        };
        tag.setFrame(CGRect::new(
            CGPoint::new(bar.origin.x - 1., y),
            CGSize::new(w, h),
        ));
        o.root.addSubview(tag);
    }
}
