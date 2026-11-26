//! Box decorations of a `PrismView` (#11) as Core Animation layers:
//!
//! - outer shadows: a companion layer *behind* the view in its superview's
//!   layer, one sublayer per shadow (`shadowPath` of the rounded box grown
//!   by the spread), so they survive `clip` (`masksToBounds` clips a layer's
//!   own shadow);
//! - a "decor" sublayer under the children, clipped to the rounded box:
//!   the gradient (`CAGradientLayer`), inner shadows (an even-odd ring whose
//!   shadow falls inside) and dashed or per-side borders (`CAShapeLayer`s);
//! - backdrop blur: an `NSVisualEffectView` (within-window blending) as the
//!   first subview, with the background drawn over it as a tint.
//!
//! Interaction states (#12): hover through `NSTrackingArea`s owned by the
//! target, presses in `PrismView`'s mouse down/up, focus by observing the
//! window's first responder, cursors through cursor rects.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSColor, NSCursor, NSView, NSVisualEffectView};
use objc2_core_graphics::CGMutablePath;
use objc2_foundation::{NSArray, NSNumber, NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CALayer, CAShapeLayer, CATransaction};
use prism_core::{BorderSide, BorderStyle, Color, Corners, Cursor, Gradient, Segment, Shadow};

/// The name of the per-corner background layer (the decorations go above it).
pub(crate) const BOX_LAYER: &str = "prism-box";

/// What a view's style asks for, beyond background, radius and border.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct Decor {
    pub shadows: Vec<Shadow>,
    /// `[top, right, bottom, left]` when the sides differ.
    pub sides: Option<[BorderSide; 4]>,
    /// The uniform border (also the sides' fallback).
    pub border: BorderSide,
    pub border_style: BorderStyle,
    pub clip: bool,
    pub gradient: Option<Gradient>,
    pub blur: f32,
    pub cursor: Option<Cursor>,
}

impl Decor {
    pub fn from_style(s: &prism_core::Style) -> Decor {
        Decor {
            shadows: s.shadows.clone(),
            sides: s.has_side_borders().then(|| s.borders()),
            border: BorderSide {
                width: s.border_width,
                color: s.border_color,
            },
            border_style: s.border_style,
            clip: s.clip,
            gradient: s.gradient.clone(),
            blur: s.backdrop_blur,
            cursor: s.cursor,
        }
    }

    /// Borders drawn by shape layers rather than the layer's own border.
    pub fn custom_border(&self) -> bool {
        self.sides.is_some() || (self.border_style != BorderStyle::Solid && self.border.width > 0.)
    }
}

/// A view's decoration state: what to draw and the layers drawing it.
#[derive(Default)]
pub(crate) struct DecorState {
    pub decor: Decor,
    layer: Option<Retained<CALayer>>,
    shadow_host: Option<Retained<CALayer>>,
    /// The blur and the tint drawn over it.
    blur: Option<(Retained<NSVisualEffectView>, Retained<NSView>)>,
}

impl DecorState {
    /// The blur view to keep first among the subviews.
    pub fn blur_view(&self) -> Option<Retained<NSView>> {
        self.blur
            .as_ref()
            .map(|(b, _)| Retained::into_super(b.clone()))
    }

    /// Whether the background is drawn by the blur's tint.
    pub fn blurs(&self) -> bool {
        self.blur.is_some()
    }
}

fn no_actions<R>(f: impl FnOnce() -> R) -> R {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    let r = f();
    CATransaction::commit();
    r
}

/// Whether `layer`'s coordinates have y going down.
pub(crate) fn is_flipped(layer: &CALayer) -> bool {
    let mut flipped = false;
    let mut l = Some(objc2::Message::retain(layer));
    while let Some(cur) = l {
        flipped ^= cur.isGeometryFlipped();
        l = cur.superlayer();
    }
    flipped
}

/// Segments (top-left origin) added to `path` in a layer `h` tall.
fn add_segments(path: &CGMutablePath, segs: &[Segment], h: f64, flipped: bool) {
    let y = |v: f32| if flipped { v as f64 } else { h - v as f64 };
    let p = Some(path);
    let n = std::ptr::null();
    for s in segs {
        unsafe {
            match *s {
                Segment::Move(x, py) => CGMutablePath::move_to_point(p, n, x as f64, y(py)),
                Segment::Line(x, py) => CGMutablePath::add_line_to_point(p, n, x as f64, y(py)),
                Segment::Cubic(a, b, c, d, e, f) => CGMutablePath::add_curve_to_point(
                    p,
                    n,
                    a as f64,
                    y(b),
                    c as f64,
                    y(d),
                    e as f64,
                    y(f),
                ),
                Segment::Close => CGMutablePath::close_subpath(p),
                Segment::Arc { .. } => {}
            }
        }
    }
}

fn cg(c: Color) -> impl std::ops::Deref<Target = objc2_core_graphics::CGColor> {
    super::imp::ns_color(c).CGColor()
}

/// `maskedCorners` for Prism corners (leading = left) in a layer.
fn corner_mask(corners: Corners, flipped: bool) -> objc2_quartz_core::CACornerMask {
    use objc2_quartz_core::CACornerMask as M;
    let (top_l, top_r, bot_l, bot_r) = if flipped {
        (
            M::LayerMinXMinYCorner,
            M::LayerMaxXMinYCorner,
            M::LayerMinXMaxYCorner,
            M::LayerMaxXMaxYCorner,
        )
    } else {
        (
            M::LayerMinXMaxYCorner,
            M::LayerMaxXMaxYCorner,
            M::LayerMinXMinYCorner,
            M::LayerMaxXMinYCorner,
        )
    };
    let mut m = M::empty();
    for (c, bit) in [
        (Corners::TOP_LEADING, top_l),
        (Corners::TOP_TRAILING, top_r),
        (Corners::BOTTOM_LEADING, bot_l),
        (Corners::BOTTOM_TRAILING, bot_r),
    ] {
        if corners.contains(c) {
            m |= bit;
        }
    }
    m
}

/// Rounds `layer` like the box (and clips its content to it when `clip`).
pub(crate) fn round(layer: &CALayer, radius: f64, corners: Corners, flipped: bool) {
    layer.setCornerRadius(radius);
    layer.setMaskedCorners(corner_mask(corners, flipped));
}

/// Draws the decorations of a view whose layer is `layer` (called from
/// `updateLayer`, inside the view's appearance: colors resolve to light or
/// dark here). `radius` is already clamped.
pub(crate) fn update(
    view: &NSView,
    layer: &CALayer,
    st: &mut DecorState,
    radius: f64,
    corners: Corners,
    background: Option<&NSColor>,
) {
    no_actions(|| {
        let flipped = is_flipped(layer);
        let b = view.bounds();
        let (w, h) = (b.size.width as f32, b.size.height as f32);
        let d = st.decor.clone();
        let r = radius as f32;

        // The blur's tint: the background, over the blur.
        if let Some((blur, tint)) = &st.blur {
            if let Some(l) = blur.layer() {
                round(&l, radius, corners, is_flipped(&l));
                l.setMasksToBounds(true);
            }
            if let Some(l) = tint.layer() {
                l.setBackgroundColor(background.map(|c| c.CGColor()).as_deref());
            }
        }

        let needs_layer =
            d.gradient.is_some() || d.shadows.iter().any(|s| s.inset) || d.custom_border();
        if !needs_layer {
            if let Some(l) = st.layer.take() {
                l.removeFromSuperlayer();
            }
        } else {
            let decor = st.layer.get_or_insert_with(|| {
                let l = CALayer::new();
                // Over the background (and the per-corner box layer at
                // index 0), under the children.
                let below_children = unsafe { layer.sublayers() }
                    .and_then(|s| s.firstObject())
                    .filter(|f| f.name().is_some_and(|n| n.to_string() == BOX_LAYER));
                match below_children {
                    Some(first) => layer.insertSublayer_above(&l, Some(&first)),
                    None => layer.insertSublayer_atIndex(&l, 0),
                }
                l
            });
            decor.setFrame(b);
            round(decor, radius, corners, flipped);
            decor.setMasksToBounds(true);
            clear_sublayers(decor);
            if let Some(g) = &d.gradient {
                decor.addSublayer(&gradient_layer(g, b.size, flipped));
            }
            // Inner shadows: the first on top.
            for s in d.shadows.iter().filter(|s| s.inset).rev() {
                decor.addSublayer(&inset_shadow(s, w, h, r, corners, flipped));
            }
            if d.custom_border() {
                for l in border_layers(&d, w, h, r, corners, flipped) {
                    decor.addSublayer(&l);
                }
            }
        }
        rebuild_shadow_host(layer, st, w, h, r, corners);
        sync_shadow_host(view, st);
    });
}

/// Removes every sublayer of `layer` in one call. `sublayers` can hand back
/// the layer's live array, so removing its elements one by one while
/// enumerating it raises « mutation detected during enumeration » (#66).
fn clear_sublayers(layer: &CALayer) {
    unsafe { layer.setSublayers(None) };
}

fn gradient_layer(g: &Gradient, size: NSSize, flipped: bool) -> Retained<CALayer> {
    use objc2_quartz_core::{
        CAGradientLayer, kCAGradientLayerAxial, kCAGradientLayerConic, kCAGradientLayerRadial,
    };
    let l = CAGradientLayer::new();
    l.setFrame(NSRect::new(NSPoint::ZERO, size));
    // A conic gradient turns towards positive angles of the layer's own
    // coordinates: clockwise on screen when they are flipped (y down),
    // counter-clockwise otherwise. CSS turns clockwise: unflipped, the stops
    // are mirrored (`1 - at`, in reverse) around the same starting ray.
    let reversed = matches!(g.kind, prism_core::GradientKind::Conic { .. }) && !flipped;
    let stops: Vec<prism_core::GradientStop> = if reversed {
        g.stops
            .iter()
            .rev()
            .map(|s| prism_core::GradientStop {
                color: s.color,
                at: 1. - s.at,
            })
            .collect()
    } else {
        g.stops.clone()
    };
    let g = &Gradient {
        kind: g.kind,
        stops,
    };
    let colors: Vec<_> = g.stops.iter().map(|s| cg(s.color)).collect();
    let objs: Vec<&AnyObject> = colors
        .iter()
        // SAFETY: CGColor is a CF type, toll-free an Objective-C object.
        .map(|c| unsafe { &*(&**c as *const objc2_core_graphics::CGColor as *const AnyObject) })
        .collect();
    let colors = NSArray::from_slice(&objs);
    unsafe { l.setColors(Some(&colors)) };
    let at: Vec<Retained<NSNumber>> = g
        .stops
        .iter()
        .map(|s| NSNumber::new_f64(s.at as f64))
        .collect();
    l.setLocations(Some(&NSArray::from_retained_slice(&at)));
    let (w, h) = (size.width as f32, size.height as f32);
    let unit = |x: f32, y: f32| {
        let (ux, uy) = (
            if w > 0. { x / w } else { 0.5 },
            if h > 0. { y / h } else { 0.5 },
        );
        NSPoint::new(ux as f64, if flipped { uy } else { 1. - uy } as f64)
    };
    let [a, b, c, e] = g.geometry(w, h);
    match g.kind {
        prism_core::GradientKind::Linear { .. } => {
            // Core Animation spaces colors along the line in unit
            // coordinates, then stretches to the box: the unit line whose
            // stretched isobars are perpendicular to CSS's line.
            let [sx, sy, ex, ey] = g.unit_points(w, h);
            let point =
                |x: f32, y: f32| NSPoint::new(x as f64, if flipped { y } else { 1. - y } as f64);
            unsafe { l.setType(kCAGradientLayerAxial) };
            l.setStartPoint(point(sx, sy));
            l.setEndPoint(point(ex, ey));
        }
        prism_core::GradientKind::Radial => {
            // Center to (center + radii): an ellipse reaching the corners.
            unsafe { l.setType(kCAGradientLayerRadial) };
            // Radii are `endPoint - startPoint` (CAGradientLayer.h): kept
            // positive whatever the orientation.
            let center = unit(a, b);
            let (rx, ry) = (
                if w > 0. { c / w } else { 0.5 },
                if h > 0. { e / h } else { 0.5 },
            );
            l.setStartPoint(center);
            l.setEndPoint(NSPoint::new(center.x + rx as f64, center.y + ry as f64));
        }
        prism_core::GradientKind::Conic { .. } => {
            // Around `startPoint`, the first color towards `endPoint`;
            // angles are measured in points, so any point on the starting
            // ray will do.
            unsafe { l.setType(kCAGradientLayerConic) };
            l.setStartPoint(unit(a, b));
            l.setEndPoint(unit(c, e));
        }
    }
    Retained::into_super(l)
}

/// A ring around the box (everything outside it, the spread inside)
/// whose shadow falls inside; the decor layer clips the ring away.
fn inset_shadow(
    s: &Shadow,
    w: f32,
    h: f32,
    r: f32,
    corners: Corners,
    flipped: bool,
) -> Retained<CALayer> {
    let l = CAShapeLayer::new();
    l.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(w as f64, h as f64)));
    let path = CGMutablePath::new();
    let far = 4. * (w + h + s.blur + s.spread.abs()) + 100.;
    add_segments(
        &path,
        &prism_native::box_path(-far, -far, w + 2. * far, h + 2. * far, 0., Corners::NONE),
        h as f64,
        flipped,
    );
    let sp = s.spread.max(0.);
    add_segments(
        &path,
        &prism_native::box_path(
            sp,
            sp,
            (w - 2. * sp).max(0.),
            (h - 2. * sp).max(0.),
            (r - sp).max(0.),
            corners,
        ),
        h as f64,
        flipped,
    );
    l.setPath(Some(&path));
    unsafe { l.setFillRule(objc2_quartz_core::kCAFillRuleEvenOdd) };
    l.setFillColor(Some(&cg(s.color)));
    // Content alpha × shadow color: the color at full alpha.
    l.setShadowColor(Some(&cg(s.color.with_alpha(1.))));
    l.setShadowOpacity(1.);
    l.setShadowRadius((s.blur / 2.) as f64);
    l.setShadowOffset(shadow_offset(s.x, s.y));
    Retained::into_super(l)
}

/// Dashed outlines, or one line per side (straight between the corners).
fn border_layers(
    d: &Decor,
    w: f32,
    h: f32,
    r: f32,
    corners: Corners,
    flipped: bool,
) -> Vec<Retained<CALayer>> {
    let label = || Color::adaptive(Color::BLACK, Color::WHITE);
    let shape = |segs: &[Segment], side: BorderSide| {
        let l = CAShapeLayer::new();
        l.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(w as f64, h as f64)));
        let path = CGMutablePath::new();
        add_segments(&path, segs, h as f64, flipped);
        l.setPath(Some(&path));
        l.setFillColor(None);
        l.setStrokeColor(Some(&cg(side.color.unwrap_or_else(label))));
        l.setLineWidth(side.width as f64);
        if let Some((on, off)) = d.border_style.dash(side.width) {
            let pattern = NSArray::from_retained_slice(&[
                NSNumber::new_f64(on as f64),
                NSNumber::new_f64(off as f64),
            ]);
            l.setLineDashPattern(Some(&pattern));
            if d.border_style == BorderStyle::Dotted {
                unsafe { l.setLineCap(objc2_quartz_core::kCALineCapRound) };
            }
        }
        Retained::into_super(l)
    };
    let Some([top, right, bottom, left]) = d.sides else {
        // One dashed or dotted outline, inside the box.
        let i = d.border.width / 2.;
        let segs = prism_native::box_path(
            i,
            i,
            (w - 2. * i).max(0.),
            (h - 2. * i).max(0.),
            (r - i).max(0.),
            corners,
        );
        return vec![shape(&segs, d.border)];
    };
    let rad = |c: Corners| if corners.contains(c) { r } else { 0. };
    let mut out = Vec::new();
    let mut side = |s: BorderSide, from: (f32, f32), to: (f32, f32)| {
        if s.width > 0. {
            out.push(shape(
                &[Segment::Move(from.0, from.1), Segment::Line(to.0, to.1)],
                s,
            ));
        }
    };
    let (t, rr, b, l) = (
        top.width / 2.,
        right.width / 2.,
        bottom.width / 2.,
        left.width / 2.,
    );
    side(
        top,
        (rad(Corners::TOP_LEADING), t),
        (w - rad(Corners::TOP_TRAILING), t),
    );
    side(
        right,
        (w - rr, rad(Corners::TOP_TRAILING)),
        (w - rr, h - rad(Corners::BOTTOM_TRAILING)),
    );
    side(
        bottom,
        (rad(Corners::BOTTOM_LEADING), h - b),
        (w - rad(Corners::BOTTOM_TRAILING), h - b),
    );
    side(
        left,
        (l, rad(Corners::TOP_LEADING)),
        (l, h - rad(Corners::BOTTOM_LEADING)),
    );
    out
}

/// One layer per outer shadow (the first on top), in a host layer that
/// [`sync_shadow_host`] keeps behind the view.
fn rebuild_shadow_host(
    layer: &CALayer,
    st: &mut DecorState,
    w: f32,
    h: f32,
    r: f32,
    corners: Corners,
) {
    let outer: Vec<Shadow> = st
        .decor
        .shadows
        .iter()
        .filter(|s| !s.inset)
        .copied()
        .collect();
    if outer.is_empty() {
        if let Some(host) = st.shadow_host.take() {
            host.removeFromSuperlayer();
        }
        return;
    }
    let host = st.shadow_host.get_or_insert_with(CALayer::new);
    clear_sublayers(host);
    // The host sits next to the view's layer: same orientation.
    let flipped = is_flipped(layer);
    // Outer shadows only show outside the box (CSS clips them to it).
    let mask = CAShapeLayer::new();
    let hole = CGMutablePath::new();
    let far = 4. * (w + h) + 1000.;
    add_segments(
        &hole,
        &prism_native::box_path(-far, -far, w + 2. * far, h + 2. * far, 0., Corners::NONE),
        h as f64,
        flipped,
    );
    add_segments(
        &hole,
        &prism_native::box_path(0., 0., w, h, r, corners),
        h as f64,
        flipped,
    );
    mask.setPath(Some(&hole));
    unsafe { mask.setFillRule(objc2_quartz_core::kCAFillRuleEvenOdd) };
    // SAFETY: the mask is a fresh layer with no superlayer.
    unsafe { host.setMask(Some(&mask)) };
    for s in outer.iter().rev() {
        // The shape is drawn far to the left and its shadow offset back in
        // place: only the shadow shows, never the shape (CSS draws outer
        // shadows outside the box only), and it is a content shadow, which
        // snapshots (`renderInContext`) draw too.
        let l = CAShapeLayer::new();
        l.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(w as f64, h as f64)));
        let sp = s.spread;
        let far = FAR;
        let path = CGMutablePath::new();
        add_segments(
            &path,
            &prism_native::box_path(
                -sp - far,
                -sp,
                (w + 2. * sp).max(0.),
                (h + 2. * sp).max(0.),
                if r > 0. { (r + sp).max(0.) } else { 0. },
                corners,
            ),
            h as f64,
            flipped,
        );
        l.setPath(Some(&path));
        l.setFillColor(Some(&cg(s.color.with_alpha(1.))));
        l.setShadowColor(Some(&cg(s.color.with_alpha(1.))));
        l.setShadowOpacity(s.color.a.clamp(0., 1.));
        l.setShadowRadius((s.blur / 2.) as f64);
        l.setShadowOffset(shadow_offset(s.x + far, s.y));
        host.addSublayer(&l);
    }
}

/// How far outer shadow shapes are drawn from their shadow.
const FAR: f32 = 100_000.;

/// `shadowOffset` for an offset in points (y down): Core Animation offsets
/// shadows with y up, whatever the layer's geometry.
fn shadow_offset(x: f32, y: f32) -> NSSize {
    NSSize::new(x as f64, -y as f64)
}

/// Keeps the outer shadows right behind the view: same superlayer, frame,
/// transform, opacity and visibility.
pub(crate) fn sync_shadow_host(view: &NSView, st: &DecorState) {
    let Some(host) = &st.shadow_host else {
        return;
    };
    let (Some(layer), Some(sup)) = (view.layer(), unsafe { view.superview() }) else {
        host.removeFromSuperlayer();
        return;
    };
    let Some(superlayer) = sup.layer().or_else(|| layer.superlayer()) else {
        host.removeFromSuperlayer();
        return;
    };
    no_actions(|| {
        // Re-inserted every time: AppKit reorders sublayers with subviews.
        host.removeFromSuperlayer();
        superlayer.insertSublayer_below(host, Some(&layer));
        host.setAnchorPoint(layer.anchorPoint());
        host.setBounds(layer.bounds());
        host.setPosition(layer.position());
        host.setTransform(layer.transform());
        host.setOpacity(view.alphaValue() as f32);
        host.setHidden(view.isHidden());
    });
}

/// Removes what lives outside the view (its shadows).
pub(crate) fn detach(st: &DecorState) {
    if let Some(host) = &st.shadow_host {
        host.removeFromSuperlayer();
    }
}

/// Adds, updates or removes the backdrop blur of `view`; returns whether
/// the subviews must be set again (a blur view appeared).
pub(crate) fn set_blur(view: &NSView, st: &mut DecorState, mtm: MainThreadMarker) -> bool {
    use objc2_app_kit::{
        NSAutoresizingMaskOptions as A, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
        NSVisualEffectState, NSWindowOrderingMode,
    };
    let radius = st.decor.blur;
    if radius <= 0. {
        if let Some((blur, _)) = st.blur.take() {
            blur.removeFromSuperview();
        }
        return false;
    }
    // AppKit blurs with its own radius: the radius picks how strong a
    // material (thin bars → header, larger → popover, then HUD-like).
    let material = if radius <= 12. {
        NSVisualEffectMaterial::HeaderView
    } else if radius <= 30. {
        NSVisualEffectMaterial::Popover
    } else {
        NSVisualEffectMaterial::FullScreenUI
    };
    let created = st.blur.is_none();
    let (blur, _) = st.blur.get_or_insert_with(|| {
        let blur = NSVisualEffectView::new(mtm);
        blur.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
        blur.setState(NSVisualEffectState::Active);
        blur.setWantsLayer(true);
        blur.setAutoresizingMask(A::ViewWidthSizable | A::ViewHeightSizable);
        let tint = NSView::new(mtm);
        tint.setWantsLayer(true);
        tint.setAutoresizingMask(A::ViewWidthSizable | A::ViewHeightSizable);
        blur.addSubview(&tint);
        (blur, tint)
    });
    blur.setMaterial(material);
    let b = view.bounds();
    blur.setFrame(b);
    if let Some((_, tint)) = &st.blur {
        tint.setFrame(NSRect::new(NSPoint::ZERO, b.size));
    }
    if created {
        let blur = st.blur.as_ref().map(|(b, _)| b.clone()).expect("blur");
        view.addSubview_positioned_relativeTo(&blur, NSWindowOrderingMode::Below, None);
    }
    created
}

/// The `NSCursor` for a Prism cursor (macOS has no "move", "help" or
/// "progress" cursor: open hand, arrow, arrow).
pub(crate) fn ns_cursor(c: Cursor) -> Retained<NSCursor> {
    #[allow(deprecated)]
    match c {
        Cursor::Default | Cursor::Help | Cursor::Progress => NSCursor::arrowCursor(),
        Cursor::Pointer => NSCursor::pointingHandCursor(),
        Cursor::Text => NSCursor::IBeamCursor(),
        Cursor::Crosshair => NSCursor::crosshairCursor(),
        Cursor::Move | Cursor::Grab => NSCursor::openHandCursor(),
        Cursor::Grabbing => NSCursor::closedHandCursor(),
        Cursor::NotAllowed => NSCursor::operationNotAllowedCursor(),
        Cursor::ResizeHorizontal => NSCursor::resizeLeftRightCursor(),
        Cursor::ResizeVertical => NSCursor::resizeUpDownCursor(),
        // Edges, corners and zoom (#223): macOS 15's frame-resize and zoom
        // cursors; before, the nearest ones.
        c => edge_cursor(c),
    }
}

/// A frame-resize or zoom cursor (#223), when the system has them (macOS
/// 15): diagonal arrows at corners. Earlier systems get the two-way arrows
/// of the edge, a crosshair at corners, the arrow for zoom.
#[allow(deprecated)]
fn edge_cursor(c: Cursor) -> Retained<NSCursor> {
    use objc2::ClassType;
    use objc2_app_kit::{NSCursorFrameResizeDirections as D, NSCursorFrameResizePosition as P};
    let modern: bool = unsafe {
        objc2::msg_send![
            NSCursor::class(),
            respondsToSelector: objc2::sel!(frameResizeCursorFromPosition:inDirections:)
        ]
    };
    let position = match c {
        Cursor::ResizeN => Some(P::Top),
        Cursor::ResizeS => Some(P::Bottom),
        Cursor::ResizeE => Some(P::Right),
        Cursor::ResizeW => Some(P::Left),
        Cursor::ResizeNe | Cursor::ResizeNeSw => Some(P::TopRight),
        Cursor::ResizeNw | Cursor::ResizeNwSe => Some(P::TopLeft),
        Cursor::ResizeSe => Some(P::BottomRight),
        Cursor::ResizeSw => Some(P::BottomLeft),
        _ => None,
    };
    match (modern, position, c) {
        (true, Some(p), _) => NSCursor::frameResizeCursorFromPosition_inDirections(p, D::All),
        (true, None, Cursor::ZoomIn) => NSCursor::zoomInCursor(),
        (true, None, Cursor::ZoomOut) => NSCursor::zoomOutCursor(),
        (false, _, Cursor::ResizeN | Cursor::ResizeS) => NSCursor::resizeUpDownCursor(),
        (false, _, Cursor::ResizeE | Cursor::ResizeW) => NSCursor::resizeLeftRightCursor(),
        (false, Some(_), _) => NSCursor::crosshairCursor(),
        _ => NSCursor::arrowCursor(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(layer: &CALayer) -> usize {
        unsafe { layer.sublayers() }.map_or(0, |s| s.count())
    }

    #[test]
    fn clearing_sublayers_twice_does_not_mutate_while_enumerating() {
        let host = CALayer::new();
        for _ in 0..2 {
            let subs: Vec<_> = (0..4).map(|_| CALayer::new()).collect();
            for l in &subs {
                host.addSublayer(l);
            }
            assert_eq!(count(&host), 4);
            clear_sublayers(&host);
            assert_eq!(count(&host), 0);
            assert!(subs.iter().all(|l| l.superlayer().is_none()));
        }
        // On a layer without sublayers, too.
        clear_sublayers(&host);
        assert_eq!(count(&host), 0);
    }
}
