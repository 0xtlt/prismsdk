//! Other people's carets and selections in text fields (#240).
//!
//! `PrismCarets` is a view laid over the field that is never hit, in the
//! field's host view: above the scroll view of a text area or rich input,
//! above the `NSTextField` of a text input, with room over it for the name
//! tags of carets on the first line. It draws, from the field's own layout
//! each time it is displayed (scrolling included), every caret as a thin
//! bar in its color with a name tag and its selection as tinted boxes,
//! clipped to what shows of the text. The field's value, selection and
//! marked text are never touched.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSFont, NSFontWeightSemibold, NSResponder, NSStringDrawing, NSTextField,
    NSTextView, NSView,
};
use objc2_foundation::{NSDictionary, NSObject, NSPoint, NSRange, NSRect, NSSize, NSString};
use prism_core::{NodeId, RemoteCaret};
use prism_native::fields::chars_to_utf16;

use crate::imp::ns_color;

/// Room above a text input for the tags of carets on its line.
const TAG_ROOM: f64 = 16.;

pub struct CaretIvars {
    carets: RefCell<Vec<RemoteCaret>>,
    /// The text view drawn for (a text area, a rich input), or the field.
    text: RefCell<Weak<NSTextView>>,
    field: RefCell<Weak<NSTextField>>,
    /// The view it is laid over (the scroll view, the field), a sibling.
    over: RefCell<Weak<NSView>>,
    /// Each caret's tag opacity: 0 once idle (faded), 1 otherwise.
    tags: RefCell<Vec<f64>>,
    fading: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = CaretIvars]
    pub struct PrismCarets;

    impl PrismCarets {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        /// Never hit: clicks reach the text under it.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }
    }
);

thread_local! {
    static OVERLAYS: RefCell<HashMap<NodeId, Retained<PrismCarets>>> = RefCell::new(HashMap::new());
}

/// Where the carets of one field are drawn from.
pub(crate) enum Host<'a> {
    /// A text area or rich input: its text view, in `scroll`, in `host`.
    Text {
        host: &'a NSView,
        scroll: &'a NSView,
        text: &'a NSTextView,
    },
    /// A text input: `field` in its `host` view.
    Input {
        host: &'a NSView,
        field: &'a NSTextField,
    },
}

/// Shows `carets` in field `id` (none: removes them).
pub(crate) fn set(mtm: MainThreadMarker, id: NodeId, host: Host, carets: &[RemoteCaret]) {
    if carets.is_empty() {
        forget(id);
        return;
    }
    let overlay = OVERLAYS.with(|o| {
        o.borrow_mut()
            .entry(id)
            .or_insert_with(|| new_overlay(mtm))
            .clone()
    });
    {
        let iv = overlay.ivars();
        let mut tags = iv.tags.borrow_mut();
        tags.resize(carets.len(), 1.);
        for (t, c) in tags.iter_mut().zip(carets) {
            // A tag comes back at once; it fades out when idle.
            if !c.idle {
                *t = 1.;
            }
        }
        *iv.carets.borrow_mut() = carets.to_vec();
    }
    let (host, over): (&NSView, &NSView) = match host {
        Host::Text { host, scroll, text } => {
            *overlay.ivars().text.borrow_mut() = Weak::from(text);
            *overlay.ivars().field.borrow_mut() = Weak::default();
            (host, scroll)
        }
        Host::Input { host, field } => {
            *overlay.ivars().field.borrow_mut() = Weak::from(field);
            *overlay.ivars().text.borrow_mut() = Weak::default();
            (host, field)
        }
    };
    *overlay.ivars().over.borrow_mut() = Weak::from(over);
    if unsafe { overlay.superview() }.as_deref() != Some(host) {
        overlay.removeFromSuperview();
        host.addSubview(&overlay);
    }
    place_over(&overlay, over);
    fade(&overlay);
    overlay.setNeedsDisplay(true);
}

/// Field `id` was laid out: the overlay follows the view it covers.
pub(crate) fn laid_out(id: NodeId) {
    let Some(overlay) = OVERLAYS.with(|o| o.borrow().get(&id).cloned()) else {
        return;
    };
    if let Some(over) = overlay.ivars().over.borrow().load() {
        place_over(&overlay, &over);
    }
}

/// Text input `id` swapped its field (`secure`): the overlay covers the new
/// one, on top of it.
pub(crate) fn input_swapped(id: NodeId, host: &NSView, field: &NSTextField) {
    let Some(overlay) = OVERLAYS.with(|o| o.borrow().get(&id).cloned()) else {
        return;
    };
    *overlay.ivars().field.borrow_mut() = Weak::from(field);
    *overlay.ivars().over.borrow_mut() = Weak::from(field as &NSView);
    overlay.removeFromSuperview();
    host.addSubview(&overlay);
    place_over(&overlay, field);
}

/// Field `id` changed (its text, selection, scrolling): redraw its carets.
pub(crate) fn refresh(id: NodeId) {
    OVERLAYS.with(|o| {
        if let Some(v) = o.borrow().get(&id) {
            v.setNeedsDisplay(true);
        }
    });
}

/// A clip view scrolled (`NSViewBoundsDidChangeNotification`): the text
/// area or rich input it shows redraws its carets.
pub(crate) fn scrolled(clip: &NSView) {
    let Some(clip) = clip.downcast_ref::<objc2_app_kit::NSClipView>() else {
        return;
    };
    if let Some(doc) = clip.documentView()
        && let Some(tv) = doc.downcast_ref::<crate::imp::PrismTextView>()
    {
        refresh(tv.ivars().id.get());
    }
}

/// Field `id` is gone or shows no carets.
pub(crate) fn forget(id: NodeId) {
    if let Some(v) = OVERLAYS.with(|o| o.borrow_mut().remove(&id)) {
        v.removeFromSuperview();
    }
}

fn new_overlay(mtm: MainThreadMarker) -> Retained<PrismCarets> {
    let this = PrismCarets::alloc(mtm).set_ivars(CaretIvars {
        carets: RefCell::new(Vec::new()),
        text: RefCell::new(Weak::default()),
        field: RefCell::new(Weak::default()),
        over: RefCell::new(Weak::default()),
        tags: RefCell::new(Vec::new()),
        fading: Cell::new(false),
    });
    unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
}

/// Over `view` (a sibling), with room above it for the tags.
fn place_over(overlay: &PrismCarets, view: &NSView) {
    let f = view.frame();
    overlay.setFrame(NSRect::new(
        NSPoint::new(f.origin.x, f.origin.y - TAG_ROOM),
        NSSize::new(f.size.width, f.size.height + TAG_ROOM),
    ));
}

/// Steps the fading tags of idle carets down to nothing (0.3 s).
fn fade(overlay: &Retained<PrismCarets>) {
    let iv = overlay.ivars();
    let busy = {
        let carets = iv.carets.borrow();
        let mut tags = iv.tags.borrow_mut();
        let mut busy = false;
        for (t, c) in tags.iter_mut().zip(carets.iter()) {
            if c.idle && *t > 0. {
                *t = (*t - 0.15).max(0.);
                busy = true;
            }
        }
        busy
    };
    overlay.setNeedsDisplay(true);
    if busy && !iv.fading.replace(true) {
        let overlay = overlay.clone();
        crate::imp::on_main_after(std::time::Duration::from_millis(40), move || {
            overlay.ivars().fading.set(false);
            fade(&overlay);
        });
    }
}

/// A caret's bar and its selection's boxes, in some view's coordinates.
struct Shape {
    bar: Option<NSRect>,
    boxes: Vec<NSRect>,
}

/// Carets laid out with text view `tv`'s own layout, in its coordinates.
fn in_text_view(tv: &NSTextView, carets: &[RemoteCaret]) -> Vec<Shape> {
    let (Some(lm), Some(tc)) = (unsafe { tv.layoutManager() }, unsafe { tv.textContainer() })
    else {
        return Vec::new();
    };
    lm.ensureLayoutForTextContainer(&tc);
    let text = tv.string().to_string();
    let origin = tv.textContainerOrigin();
    let pad = tc.lineFragmentPadding();
    let glyph = |chars: usize| lm.glyphIndexForCharacterAtIndex(chars_to_utf16(&text, chars));
    let n = lm.numberOfGlyphs();
    let shift = |r: NSRect| {
        NSRect::new(
            NSPoint::new(r.origin.x + origin.x, r.origin.y + origin.y),
            r.size,
        )
    };
    let line_of = |g: usize| {
        let mut eff = NSRange::new(0, 0);
        let line = unsafe { lm.lineFragmentRectForGlyphAtIndex_effectiveRange(g, &mut eff) };
        (line, eff)
    };
    carets
        .iter()
        .map(|c| {
            let range = c.range();
            let (gs, ge) = (glyph(range.start).min(n), glyph(range.end).min(n));
            let mut boxes = Vec::new();
            let mut g = gs;
            while g < ge {
                let (line, eff) = line_of(g);
                if eff.length == 0 {
                    break;
                }
                let hi = ge.min(eff.location + eff.length);
                let r = lm.boundingRectForGlyphRange_inTextContainer(NSRange::new(g, hi - g), &tc);
                boxes.push(shift(NSRect::new(
                    NSPoint::new(r.origin.x, line.origin.y),
                    NSSize::new(r.size.width, line.size.height),
                )));
                g = hi;
            }
            let head = glyph(c.head).min(n);
            let extra = lm.extraLineFragmentRect();
            let bar = if head < n {
                let (line, _) = line_of(head);
                let x = line.origin.x + lm.locationForGlyphAtIndex(head).x;
                Some((x, line.origin.y, line.size.height))
            } else if extra.size.height > 0. && (n == 0 || text.ends_with('\n')) {
                Some((extra.origin.x + pad, extra.origin.y, extra.size.height))
            } else if n > 0 {
                let (line, _) = line_of(n - 1);
                let r = lm.boundingRectForGlyphRange_inTextContainer(NSRange::new(n - 1, 1), &tc);
                Some((r.origin.x + r.size.width, line.origin.y, line.size.height))
            } else {
                None
            };
            Shape {
                bar: bar
                    .map(|(x, y, h)| shift(NSRect::new(NSPoint::new(x, y), NSSize::new(0., h)))),
                boxes,
            }
        })
        .collect()
}

/// Carets of a text input that is not being edited: its text measured
/// with its font, in the field's coordinates (one line).
fn in_field(field: &NSTextField, carets: &[RemoteCaret]) -> Vec<Shape> {
    let Some(cell) = field.cell() else {
        return Vec::new();
    };
    let title = cell.titleRectForBounds(field.bounds());
    let font = field
        .font()
        .unwrap_or_else(|| NSFont::systemFontOfSize(NSFont::systemFontSize()));
    let text = field.stringValue().to_string();
    let attrs = {
        let f: &AnyObject = &font;
        unsafe { NSDictionary::from_slices(&[objc2_app_kit::NSFontAttributeName], &[f]) }
    };
    let width = |chars: usize| {
        let prefix: String = text.chars().take(chars).collect();
        unsafe { NSString::from_str(&prefix).sizeWithAttributes(Some(&attrs)) }.width
    };
    // The field editor's line fragment padding.
    let inner = title.size.width - 4.;
    let full = width(usize::MAX);
    let x0 = title.origin.x
        + 2.
        + match field.alignment() {
            objc2_app_kit::NSTextAlignment::Center => ((inner - full) / 2.).max(0.),
            objc2_app_kit::NSTextAlignment::Right => (inner - full).max(0.),
            _ => 0.,
        };
    let h = (font.ascender() - font.descender() + font.leading())
        .ceil()
        .min(title.size.height.max(1.));
    let y = title.origin.y;
    carets
        .iter()
        .map(|c| {
            let range = c.range();
            let (a, b) = (width(range.start), width(range.end));
            Shape {
                bar: Some(NSRect::new(
                    NSPoint::new(x0 + width(c.head), y),
                    NSSize::new(0., h),
                )),
                boxes: if b > a {
                    vec![NSRect::new(NSPoint::new(x0 + a, y), NSSize::new(b - a, h))]
                } else {
                    Vec::new()
                },
            }
        })
        .collect()
}

fn into_view(tv: Retained<NSTextView>) -> Retained<NSView> {
    Retained::into_super(Retained::into_super(tv))
}

fn intersect(a: NSRect, b: NSRect) -> Option<NSRect> {
    let x = a.origin.x.max(b.origin.x);
    let y = a.origin.y.max(b.origin.y);
    let r = (a.origin.x + a.size.width).min(b.origin.x + b.size.width);
    let t = (a.origin.y + a.size.height).min(b.origin.y + b.size.height);
    (r > x && t > y).then(|| NSRect::new(NSPoint::new(x, y), NSSize::new(r - x, t - y)))
}

impl PrismCarets {
    /// The carets' shapes in this view's coordinates, and the part of it
    /// that shows the field's text.
    fn shapes(&self) -> (Vec<Shape>, NSRect) {
        let iv = self.ivars();
        let carets = iv.carets.borrow();
        let (shapes, from, clip): (Vec<Shape>, Retained<NSView>, NSRect) =
            if let Some(tv) = iv.text.borrow().load() {
                // What the scroll view shows of the text.
                let clip = match tv.enclosingScrollView() {
                    Some(sv) => {
                        let cv = sv.contentView();
                        self.convertRect_fromView(cv.bounds(), Some(&cv))
                    }
                    None => self.convertRect_fromView(tv.visibleRect(), Some(&tv)),
                };
                (in_text_view(&tv, &carets), into_view(tv), clip)
            } else if let Some(field) = iv.field.borrow().load() {
                let clip = self.convertRect_fromView(field.bounds(), Some(&field));
                let editor = field
                    .currentEditor()
                    .and_then(|e| e.downcast::<NSTextView>().ok());
                match editor {
                    // Being edited: the field editor's layout, scrolled.
                    Some(e) => (in_text_view(&e, &carets), into_view(e), clip),
                    None => (
                        in_field(&field, &carets),
                        Retained::into_super(Retained::into_super(field)),
                        clip,
                    ),
                }
            } else {
                return (Vec::new(), NSRect::ZERO);
            };
        let shapes = shapes
            .into_iter()
            .map(|s| Shape {
                bar: s.bar.map(|r| self.convertRect_fromView(r, Some(&from))),
                boxes: s
                    .boxes
                    .into_iter()
                    .map(|r| self.convertRect_fromView(r, Some(&from)))
                    .collect(),
            })
            .collect();
        (shapes, clip)
    }

    fn draw(&self) {
        let (shapes, clip) = self.shapes();
        let iv = self.ivars();
        let carets = iv.carets.borrow();
        let tags = iv.tags.borrow();
        let bounds = self.bounds();
        let tag_font = unsafe { NSFont::systemFontOfSize_weight(10., NSFontWeightSemibold) };
        for (i, (c, s)) in carets.iter().zip(shapes).enumerate() {
            let color = ns_color(c.color);
            color
                .colorWithAlphaComponent(0.25 * color.alphaComponent())
                .setFill();
            for b in s.boxes.into_iter().filter_map(|b| intersect(b, clip)) {
                NSBezierPath::fillRect(b);
            }
            let Some(bar) = s.bar else { continue };
            let x = bar.origin.x;
            if x < clip.origin.x - 1.
                || x > clip.origin.x + clip.size.width + 1.
                || intersect(
                    NSRect::new(
                        NSPoint::new(clip.origin.x, bar.origin.y),
                        NSSize::new(clip.size.width, bar.size.height),
                    ),
                    clip,
                )
                .is_none()
            {
                continue;
            }
            color.setFill();
            NSBezierPath::fillRect(NSRect::new(
                NSPoint::new(x - 1., bar.origin.y),
                NSSize::new(2., bar.size.height),
            ));
            let alpha = tags.get(i).copied().unwrap_or(1.);
            let Some(label) = c.label.as_deref().filter(|_| alpha > 0.) else {
                continue;
            };
            let white = NSColor::whiteColor().colorWithAlphaComponent(alpha);
            let attrs = {
                let (f, w): (&AnyObject, &AnyObject) = (&tag_font, &white);
                unsafe {
                    NSDictionary::from_slices(
                        &[
                            objc2_app_kit::NSFontAttributeName,
                            objc2_app_kit::NSForegroundColorAttributeName,
                        ],
                        &[f, w],
                    )
                }
            };
            let s = NSString::from_str(label);
            let size = unsafe { s.sizeWithAttributes(Some(&attrs)) };
            let (w, h) = ((size.width + 8.).ceil(), (size.height + 2.).ceil());
            // Above the caret's line, or under it at the top of the view.
            let above = bar.origin.y - h;
            let y = if above >= bounds.origin.y {
                above
            } else {
                bar.origin.y + bar.size.height
            };
            let tag = NSRect::new(
                NSPoint::new(
                    (x - 1.)
                        .min(bounds.origin.x + bounds.size.width - w)
                        .max(bounds.origin.x),
                    y,
                ),
                NSSize::new(w, h),
            );
            color
                .colorWithAlphaComponent(alpha * color.alphaComponent())
                .setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(tag, 3., 3.).fill();
            unsafe {
                s.drawAtPoint_withAttributes(
                    NSPoint::new(tag.origin.x + 4., tag.origin.y + 1.),
                    Some(&attrs),
                )
            };
        }
    }
}
