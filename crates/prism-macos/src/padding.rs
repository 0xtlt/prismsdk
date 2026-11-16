//! Padding on leaves (#14). `prism-native` lays a padded leaf out with its
//! padding around the measured content and hands the padding over in the
//! style (leading = left): texts draw their box and inset their text,
//! inputs and text areas inset their text in place of the field's own
//! inset, buttons get a bezel as tall as their frame, timers sit inside it.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2_app_kit::{NSBezierPath, NSButton, NSColor, NSTextField, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use prism_core::{Edges, NodeId, Segment, Style};

use crate::imp::ns_color;

thread_local! {
    /// Each text, button, input and text area's style (what `measure` and
    /// `set_frame` need: padding, border).
    static STYLES: RefCell<HashMap<NodeId, Style>> = RefCell::new(HashMap::new());
}

/// Remembers the style of leaf `id`.
pub fn remember(id: NodeId, style: &Style) {
    STYLES.with(|p| p.borrow_mut().insert(id, style.clone()));
}

/// Forgets leaf `id`.
pub fn forget(id: NodeId) {
    STYLES.with(|p| p.borrow_mut().remove(&id));
}

/// The style of leaf `id` (the default one when unknown).
pub fn style_of(id: NodeId) -> Style {
    STYLES.with(|p| p.borrow().get(&id).cloned().unwrap_or_default())
}

/// The padding of leaf `id` (zero when it has none).
pub fn of(id: NodeId) -> Edges {
    STYLES.with(|p| p.borrow().get(&id).map(|s| s.padding).unwrap_or_default())
}

/// `rect` (in `view`'s coordinates) minus `e`, whether the view is flipped
/// or not.
pub fn inset_rect(view: &NSView, rect: NSRect, e: Edges) -> NSRect {
    let (l, r, t, b) = (e.left as f64, e.right as f64, e.top as f64, e.bottom as f64);
    let y = if view.isFlipped() { t } else { b };
    NSRect::new(
        NSPoint::new(rect.origin.x + l, rect.origin.y + y),
        NSSize::new(
            (rect.size.width - l - r).max(0.),
            (rect.size.height - t - b).max(0.),
        ),
    )
}

/// The node's box path in `view` (rounding only its rounded corners),
/// inset by `inset` on every side.
fn box_path(view: &NSView, style: &Style, inset: f64) -> objc2::rc::Retained<NSBezierPath> {
    let b = view.bounds().size;
    let (w, h) = (b.width as f32, b.height as f32);
    let i = inset as f32;
    let segs = prism_native::box_path(
        i,
        i,
        (w - 2. * i).max(0.),
        (h - 2. * i).max(0.),
        (style.corner_radius - i).max(0.),
        style.rounded_corners,
    );
    let flipped = view.isFlipped();
    let pt = |x: f32, y: f32| NSPoint::new(x as f64, if flipped { y } else { h - y } as f64);
    let path = NSBezierPath::bezierPath();
    for s in segs {
        match s {
            Segment::Move(x, y) => path.moveToPoint(pt(x, y)),
            Segment::Line(x, y) => path.lineToPoint(pt(x, y)),
            Segment::Cubic(x1, y1, x2, y2, x, y) => {
                path.curveToPoint_controlPoint1_controlPoint2(pt(x, y), pt(x1, y1), pt(x2, y2))
            }
            Segment::Close => path.closePath(),
            _ => {}
        }
    }
    path
}

/// Fills and strokes a leaf's box (background, border, rounding) under its
/// content, for views that are not Prism containers (texts).
pub fn draw_box(view: &NSView, style: &Style) {
    if let Some(bg) = style.background {
        ns_color(bg).setFill();
        box_path(view, style, 0.).fill();
    }
    if style.border_width > 0. {
        let bw = style.border_width as f64;
        style
            .border_color
            .map_or_else(NSColor::separatorColor, ns_color)
            .setStroke();
        let path = box_path(view, style, bw / 2.);
        path.setLineWidth(bw);
        path.stroke();
    }
}

/// A padded button gets a bezel as tall as its frame (a push button's is
/// fixed); its title stays centered in it.
pub fn style_button(b: &NSButton, style: &Style) {
    use objc2_app_kit::NSBezelStyle;
    let bezel = if !style.has_padding() {
        NSBezelStyle::Push
    } else {
        NSBezelStyle::FlexiblePush
    };
    if b.bezelStyle() != bezel {
        b.setBezelStyle(bezel);
    }
}

/// A padded button's content: its title alone.
pub fn button_content(b: &NSButton) -> NSSize {
    use objc2_app_kit::NSAttributedStringNSStringDrawing;
    let s = b.attributedTitle().size();
    NSSize::new(s.width.ceil(), s.height.ceil())
}

/// Whether a text input draws Prism's box instead of the bezeled field: it
/// has padding (an explicit zero included, #69), a background, a border,
/// or `.border(0.)`.
pub fn custom_field(style: &Style) -> bool {
    style.has_padding()
        || style.background.is_some()
        || style.border_width > 0.
        || style.border_none
}

/// Where a custom field's text sits inside its box: its padding (or the
/// default 6 × 8 inset, like the web, when it sets none: `.p(0.)` is no
/// inset at all, #69) inside its border.
pub fn field_inset(style: &Style) -> Edges {
    if !custom_field(style) {
        return Edges::default();
    }
    let p = if !style.has_padding() {
        Edges {
            top: 6.,
            right: 8.,
            bottom: 6.,
            left: 8.,
        }
    } else {
        style.padding
    };
    let b = field_border(style).map_or(0., |(w, _)| w);
    Edges {
        top: p.top + b,
        right: p.right + b,
        bottom: p.bottom + b,
        left: p.left + b,
    }
}

/// A custom field's border: its own, or the default one (1 point, gray,
/// rounded 6) unless `.border(0.)`.
pub fn field_border(style: &Style) -> Option<(f32, Option<prism_core::Color>)> {
    if style.border_width > 0. {
        Some((style.border_width, style.border_color))
    } else if style.border_none {
        None
    } else {
        Some((1., Some(prism_core::rgba(0x808080, 0.45))))
    }
}

/// The field itself: bezeled when unstyled, bare inside Prism's box
/// otherwise.
pub fn style_field(f: &NSTextField, style: &Style) {
    let bezeled = !custom_field(style);
    if f.isBezeled() != bezeled {
        f.setBezeled(bezeled);
        f.setDrawsBackground(bezeled);
        f.setBordered(false);
    }
}

/// The style a custom field's host view draws (its box, with the default
/// border and rounding when it sets none); `None` for a bezeled field.
pub fn field_box(style: &Style) -> Option<Style> {
    if !custom_field(style) {
        return None;
    }
    let mut s = style.clone();
    if style.border_width <= 0.
        && let Some((w, c)) = field_border(style)
    {
        s.border_width = w;
        s.border_color = c;
        if s.corner_radius <= 0. {
            s.corner_radius = 6.;
        }
    }
    Some(s)
}

/// A custom field's content size (without its padding: the layout adds
/// it) from the bare field's fitting size: the default inset and the
/// border, at least 120 points wide in all.
pub fn field_content(style: &Style, fitting: NSSize) -> NSSize {
    let inset = field_inset(style);
    let p = style.padding;
    let extra_x = (inset.left + inset.right - p.left - p.right) as f64;
    let extra_y = (inset.top + inset.bottom - p.top - p.bottom) as f64;
    let min = (120. - (p.left + p.right) as f64).max(0.);
    NSSize::new((fitting.width + extra_x).max(min), fitting.height + extra_y)
}
