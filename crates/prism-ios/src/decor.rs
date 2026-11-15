//! Box decorations and interaction states on UIKit (iOS, iPadOS, tvOS,
//! visionOS).
//!
//! Decorations are Core Animation layers, kept in sync with the view's size
//! (`layout`, called from `set_frame`):
//!
//! - drop shadows: a companion view just below the view in its superview,
//!   one layer per shadow (`shadowPath`, interior cut out like CSS), so they
//!   survive `clip` (`clipsToBounds` clips a layer's own shadow);
//! - inner shadows: a layer whose shadow falls inside the box, masked to it;
//! - per-side, dashed and dotted borders: `CAShapeLayer`s (the uniform solid
//!   border stays the layer's own border);
//! - gradients: a `CAGradientLayer` (linear along the CSS line, or radial);
//! - backdrop blur: a `UIVisualEffectView` (system materials by radius; the
//!   background tints its content view);
//! - clip: `clipsToBounds` with the layer's per-corner radius.
//!
//! Decoration layers sit under the children (negative `zPosition`), over the
//! background. Interaction states: presses (touches of `PrismContainer`s,
//! button control events), pointer hover (`UIHoverGestureRecognizer`: iPad
//! pointer, visionOS look), keyboard focus (text fields and areas always
//! report it, for focus-within; on tvOS the focus engine's focused view also
//! hovers). Clickable views get the iPad pointer's hover effect
//! (`UIPointerInteraction`; `Cursor::Text` is a beam) and visionOS's hover
//! highlight (`hoverStyle`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_core_foundation::{CFRetained, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGMutablePath};
use objc2_foundation::{NSArray, NSNumber, NSObject, NSObjectProtocol};
use objc2_quartz_core::{CAGradientLayer, CALayer, CAShapeLayer, CATransaction};
use objc2_ui_kit::{UIColor, UITraitEnvironment, UIView};
#[cfg(not(target_os = "tvos"))]
use objc2_ui_kit::{UIInteraction, UIPointerInteractionDelegate};
use prism_core::{
    BorderSide, BorderStyle, Color, Cursor, GradientKind, Interaction, Interactions, NodeId,
    Segment, Shadow, Style,
};

/// Under the children: effect view, gradient, inner shadows, borders.
const Z_EFFECT: CGFloat = -10.;
const Z_GRADIENT: CGFloat = -3.;
const Z_INSET: CGFloat = -2.;
const Z_BORDER: CGFloat = -1.;

#[derive(Default)]
struct Decor {
    view: Option<Retained<UIView>>,
    style: Style,
    /// Drop shadows, below the view in its superview.
    companion: Option<Retained<UIView>>,
    gradient: Option<Retained<CAGradientLayer>>,
    insets: Vec<Retained<CALayer>>,
    borders: Vec<Retained<CAShapeLayer>>,
    effect: Option<Retained<objc2_ui_kit::UIVisualEffectView>>,
    /// `clipsToBounds` was turned on by `clip`.
    clipped: bool,
}

thread_local! {
    static DECOR: RefCell<HashMap<NodeId, Decor>> = RefCell::new(HashMap::new());
    /// The states each node reports (`Widgets::set_interactions`).
    static STATES: RefCell<HashMap<NodeId, Interactions>> = RefCell::new(HashMap::new());
    /// Pointer shapes, and clickable nodes (pointer hover effect).
    static CURSORS: RefCell<HashMap<NodeId, Cursor>> = RefCell::new(HashMap::new());
    static CLICKABLE: RefCell<HashSet<NodeId>> = RefCell::new(HashSet::new());
    static TARGET: RefCell<Option<Retained<InteractionTarget>>> = const { RefCell::new(None) };
    /// iPad pointer interactions by node.
    #[cfg(target_os = "ios")]
    static POINTERS: RefCell<HashMap<NodeId, Retained<objc2_ui_kit::UIPointerInteraction>>> =
        RefCell::new(HashMap::new());
}

fn uses(style: &Style) -> bool {
    !style.shadows.is_empty()
        || custom_border(style)
        || style.clip
        || style.gradient.is_some()
        || style.backdrop_blur > 0.
}

/// Borders the layer's own (uniform, solid) border cannot draw.
pub fn custom_border(style: &Style) -> bool {
    let sides = style.borders();
    let uniform = sides.iter().all(|s| *s == sides[0]);
    !uniform
        || (sides[0].width > 0.
            && (style.border_style != BorderStyle::Solid || !style.border_sides.is_empty()))
}

fn cg_color(c: Color, view: &UIView) -> CFRetained<CGColor> {
    let ui = crate::imp::ui_color(c);
    let resolved = ui.resolvedColorWithTraitCollection(&traits_of(view));
    unsafe { resolved.CGColor() }.into()
}

// --- Adaptive layer colors (#36) ---

thread_local! {
    /// The root view: its appearance is the app's, for views not in a
    /// window yet (a view's own traits are the defaults until then).
    static ROOT: RefCell<Option<Retained<UIView>>> = const { RefCell::new(None) };
    /// The app's traits as the root controller last reported them: views
    /// update theirs lazily, after it.
    static TRAITS: RefCell<Option<Retained<objc2_ui_kit::UITraitCollection>>> =
        const { RefCell::new(None) };
    /// Adaptive uniform borders drawn by a view's own layer (`borderColor`
    /// is a plain `CGColor`), by view: resolved again with the appearance.
    static BORDERS: RefCell<HashMap<usize, (Retained<UIView>, Color)>> =
        RefCell::new(HashMap::new());
}

/// Remembers the root view (see [`traits_of`]).
pub fn set_root(view: &UIView) {
    ROOT.with(|r| *r.borrow_mut() = Some(view.retain()));
}

/// The appearance `view` draws in: the app's (the root controller's last
/// traits, else the root's), as every view shares it; its own before any.
fn traits_of(view: &UIView) -> Retained<objc2_ui_kit::UITraitCollection> {
    if let Some(t) = TRAITS.with(|t| t.borrow().clone()) {
        return t;
    }
    if view.window().is_none()
        && let Some(root) = ROOT.with(|r| r.borrow().clone())
    {
        return root.traitCollection();
    }
    view.traitCollection()
}

/// Sets the border color of `view`'s own layer, resolved for the current
/// appearance, and again whenever it changes (an adaptive color).
pub fn set_layer_border(view: &UIView, color: Option<Color>) {
    let key = view as *const UIView as usize;
    view.layer()
        .setBorderColor(color.map(|c| cg_color(c, view)).as_deref());
    BORDERS.with(|b| {
        let mut b = b.borrow_mut();
        match color {
            Some(c) if c.dark.is_some() => b.insert(key, (view.retain(), c)),
            _ => b.remove(&key),
        };
    });
}

fn add_segments(path: &CGMutablePath, segments: &[Segment]) {
    let p = Some(path);
    let n = std::ptr::null();
    for s in segments {
        unsafe {
            match *s {
                Segment::Move(x, y) => CGMutablePath::move_to_point(p, n, x as _, y as _),
                Segment::Line(x, y) => CGMutablePath::add_line_to_point(p, n, x as _, y as _),
                Segment::Cubic(a, b, c, d, e, f) => CGMutablePath::add_curve_to_point(
                    p, n, a as _, b as _, c as _, d as _, e as _, f as _,
                ),
                Segment::Close => CGMutablePath::close_subpath(p),
                Segment::Arc { .. } => {}
            }
        }
    }
}

fn box_segments(style: &Style, x: f32, y: f32, w: f32, h: f32, grow: f32) -> Vec<Segment> {
    let r = if style.corner_radius > 0. {
        (style.corner_radius + grow).max(0.)
    } else {
        0.
    };
    prism_native::box_path(
        x - grow,
        y - grow,
        w + 2. * grow,
        h + 2. * grow,
        r,
        style.rounded_corners,
    )
}

fn path(segments: &[Segment]) -> CFRetained<CGMutablePath> {
    let p = CGMutablePath::new();
    add_segments(&p, segments);
    p
}

/// A rectangle drawn counter-clockwise (a hole under the non-zero rule when
/// combined with a clockwise box).
fn add_reverse_rect(path: &CGMutablePath, r: CGRect) {
    let (x0, y0) = (r.origin.x, r.origin.y);
    let (x1, y1) = (x0 + r.size.width, y0 + r.size.height);
    let p = Some(path);
    let n = std::ptr::null();
    unsafe {
        CGMutablePath::move_to_point(p, n, x0, y0);
        CGMutablePath::add_line_to_point(p, n, x0, y1);
        CGMutablePath::add_line_to_point(p, n, x1, y1);
        CGMutablePath::add_line_to_point(p, n, x1, y0);
    }
    CGMutablePath::close_subpath(p);
}

fn shape_mask(
    segments: &[Segment],
    bounds: CGRect,
    outside: Option<CGRect>,
) -> Retained<CAShapeLayer> {
    let mask = CAShapeLayer::new();
    mask.setFrame(bounds);
    let p = CGMutablePath::new();
    if let Some(r) = outside {
        unsafe { CGMutablePath::add_rect(Some(&p), std::ptr::null(), r) };
        mask.setFillRule(unsafe { objc2_quartz_core::kCAFillRuleEvenOdd });
    }
    add_segments(&p, segments);
    mask.setPath(Some(&p));
    mask
}

fn shadow_layer(sh: &Shadow, view: &UIView, bounds: CGRect) -> Retained<CALayer> {
    let l = CALayer::new();
    l.setFrame(bounds);
    l.setShadowColor(Some(&cg_color(sh.color.with_alpha(1.), view)));
    l.setShadowOpacity(sh.color.a.clamp(0., 1.));
    // CSS blur is twice the Gaussian's deviation; so is Core Animation's radius.
    l.setShadowRadius((sh.blur / 2.) as CGFloat);
    l.setShadowOffset(CGSize::new(sh.x as _, sh.y as _));
    l
}

/// Applies the decorations of `style` to `view` (after the base style).
pub fn apply(view: &UIView, style: &Style) {
    let id = view.tag() as NodeId;
    let cursor_changed = CURSORS.with(|c| {
        let mut c = c.borrow_mut();
        let old = c.get(&id).copied();
        match style.cursor {
            Some(cur) => c.insert(id, cur),
            None => c.remove(&id),
        };
        old != style.cursor
    });
    if cursor_changed {
        update_pointer(view);
    }
    let has = DECOR.with(|d| d.borrow().contains_key(&id));
    if !has && !uses(style) {
        return;
    }
    DECOR.with(|d| {
        let mut d = d.borrow_mut();
        let e = d.entry(id).or_default();
        e.view = Some(view.retain());
        e.style = style.clone();
        if style.clip {
            view.setClipsToBounds(true);
            e.clipped = true;
        } else if e.clipped {
            view.setClipsToBounds(false);
            e.clipped = false;
        }
        if custom_border(style) {
            view.layer().setBorderWidth(0.);
        }
        set_effect(e, view, style);
        layout_entry(e);
        if !uses(style) {
            d.remove(&id);
        }
    });
}

/// Frosted background: a visual effect view under everything, tinted by
/// the background color.
fn set_effect(e: &mut Decor, view: &UIView, style: &Style) {
    use objc2_ui_kit::{UIBlurEffect, UIBlurEffectStyle as B, UIVisualEffectView};
    if style.backdrop_blur <= 0. {
        if let Some(old) = e.effect.take() {
            old.removeFromSuperview();
        }
        return;
    }
    let r = style.backdrop_blur;
    // System materials by thickness (tvOS has the older styles only:
    // UIBlurEffect.h, API_UNAVAILABLE(tvos) on the materials).
    let blur = if cfg!(target_os = "tvos") {
        if r <= 20. { B::Regular } else { B::Prominent }
    } else if r <= 6. {
        B::SystemUltraThinMaterial
    } else if r <= 16. {
        B::SystemThinMaterial
    } else if r <= 28. {
        B::SystemMaterial
    } else {
        B::SystemThickMaterial
    };
    let mtm = view.mtm();
    let effect = UIBlurEffect::effectWithStyle(blur, mtm);
    let ev = e.effect.get_or_insert_with(|| {
        let ev = UIVisualEffectView::initWithEffect(UIVisualEffectView::alloc(mtm), None);
        ev.setUserInteractionEnabled(false);
        ev.setAutoresizingMask(
            objc2_ui_kit::UIViewAutoresizing::FlexibleWidth
                | objc2_ui_kit::UIViewAutoresizing::FlexibleHeight,
        );
        ev.layer().setZPosition(Z_EFFECT);
        ev.setClipsToBounds(true);
        ev
    });
    ev.setEffect(Some(&effect));
    if ev.superview().as_deref() != Some(view) {
        view.insertSubview_atIndex(ev, 0);
    }
    ev.setFrame(view.bounds());
    // The tint goes over the blur, not under it.
    ev.contentView()
        .setBackgroundColor(style.background.map(crate::imp::ui_color).as_deref());
    view.setBackgroundColor(Some(&UIColor::clearColor()));
}

/// Lays the decorations out at the view's current size.
pub fn layout(view: &UIView) {
    let id = view.tag() as NodeId;
    DECOR.with(|d| {
        if let Some(e) = d.borrow_mut().get_mut(&id) {
            layout_entry(e);
        }
    });
}

fn layout_entry(e: &mut Decor) {
    let Some(view) = e.view.clone() else { return };
    let style = e.style.clone();
    let bounds = view.bounds();
    let (w, h) = (bounds.size.width as f32, bounds.size.height as f32);
    let layer = view.layer();
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    let radius = (style.corner_radius as f64)
        .min(w.min(h) as f64 / 2.)
        .max(0.);
    let mask = prism_native::apple::corner_mask(style.rounded_corners, true);

    // Gradient.
    match &style.gradient {
        Some(g) if w > 0. && h > 0. => {
            let gl = e.gradient.get_or_insert_with(|| {
                let gl = CAGradientLayer::new();
                gl.setZPosition(Z_GRADIENT);
                layer.addSublayer(&gl);
                gl
            });
            gl.setFrame(bounds);
            gl.setCornerRadius(radius);
            gl.setMaskedCorners(mask);
            let colors: Vec<CFRetained<CGColor>> =
                g.stops.iter().map(|s| cg_color(s.color, &view)).collect();
            let objects: Vec<&AnyObject> = colors
                .iter()
                .map(|c| unsafe { &*(&**c as *const CGColor as *const AnyObject) })
                .collect();
            unsafe { gl.setColors(Some(&NSArray::from_slice(&objects))) };
            let at: Vec<Retained<NSNumber>> = g
                .stops
                .iter()
                .map(|s| NSNumber::new_f64(s.at as f64))
                .collect();
            gl.setLocations(Some(&NSArray::from_retained_slice(&at)));
            let [a, b, c, d] = g.geometry(w, h);
            match g.kind {
                GradientKind::Linear { .. } => {
                    gl.setType(unsafe { objc2_quartz_core::kCAGradientLayerAxial });
                    gl.setStartPoint(CGPoint::new((a / w) as _, (b / h) as _));
                    gl.setEndPoint(CGPoint::new((c / w) as _, (d / h) as _));
                }
                GradientKind::Radial => {
                    gl.setType(unsafe { objc2_quartz_core::kCAGradientLayerRadial });
                    gl.setStartPoint(CGPoint::new(0.5, 0.5));
                    gl.setEndPoint(CGPoint::new((0.5 + c / w) as _, (0.5 + d / h) as _));
                }
            }
        }
        _ => {
            if let Some(gl) = e.gradient.take() {
                gl.removeFromSuperlayer();
            }
        }
    }

    // Inner shadows: the shadow of a ring around the box, cut to the box.
    for l in e.insets.drain(..) {
        l.removeFromSuperlayer();
    }
    let box_ = box_segments(&style, 0., 0., w, h, 0.);
    for sh in style.shadows.iter().rev().filter(|s| s.inset) {
        let l = shadow_layer(sh, &view, bounds);
        let ring = CGMutablePath::new();
        let far = (sh.blur + sh.x.abs() + sh.y.abs() + sh.spread.abs()) as f64 + 40.;
        add_reverse_rect(
            &ring,
            CGRect::new(
                CGPoint::new(-far, -far),
                CGSize::new(w as f64 + 2. * far, h as f64 + 2. * far),
            ),
        );
        add_segments(&ring, &box_segments(&style, 0., 0., w, h, -sh.spread));
        l.setShadowPath(Some(&ring));
        let m = shape_mask(&box_, bounds, None);
        unsafe { l.setMask(Some(&m)) };
        l.setZPosition(Z_INSET);
        layer.addSublayer(&l);
        e.insets.push(l);
    }

    // Borders drawn as strokes: dashed, dotted, or different per side.
    for b in e.borders.drain(..) {
        b.removeFromSuperlayer();
    }
    if custom_border(&style) {
        let sides = style.borders();
        let dots = style.border_style == BorderStyle::Dotted;
        let stroke = |b: BorderSide, segments: &[Segment]| -> Option<Retained<CAShapeLayer>> {
            let color = b.color?;
            if b.width <= 0. {
                return None;
            }
            let s = CAShapeLayer::new();
            s.setFrame(bounds);
            s.setPath(Some(&path(segments)));
            s.setFillColor(None);
            s.setStrokeColor(Some(&cg_color(color, &view)));
            s.setLineWidth(b.width as _);
            if let Some((on, off)) = style.border_style.dash(b.width) {
                let pattern = NSArray::from_retained_slice(&[
                    NSNumber::new_f64(on as f64),
                    NSNumber::new_f64(off as f64),
                ]);
                s.setLineDashPattern(Some(&pattern));
            }
            if dots {
                s.setLineCap(unsafe { objc2_quartz_core::kCALineCapRound });
            }
            s.setZPosition(Z_BORDER);
            Some(s)
        };
        if sides.iter().all(|s| *s == sides[0]) {
            let i = sides[0].width / 2.;
            let outline = box_segments(&style, i, i, w - 2. * i, h - 2. * i, -i);
            if let Some(s) = stroke(sides[0], &outline) {
                e.borders.push(s);
            }
        } else {
            let [t, r, b, l] = sides;
            let lines = [
                (t, [0., t.width / 2., w, t.width / 2.]),
                (r, [w - r.width / 2., 0., w - r.width / 2., h]),
                (b, [0., h - b.width / 2., w, h - b.width / 2.]),
                (l, [l.width / 2., 0., l.width / 2., h]),
            ];
            for (side, [x0, y0, x1, y1]) in lines {
                if let Some(s) = stroke(side, &[Segment::Move(x0, y0), Segment::Line(x1, y1)]) {
                    // Kept inside rounded corners.
                    if radius > 0. {
                        let m = shape_mask(&box_, bounds, None);
                        unsafe { s.setMask(Some(&m)) };
                    }
                    e.borders.push(s);
                }
            }
        }
        for s in &e.borders {
            layer.addSublayer(s);
        }
    }

    // Backdrop blur: rounded like the box.
    if let Some(ev) = &e.effect {
        ev.setFrame(bounds);
        let l = ev.layer();
        l.setCornerRadius(radius);
        l.setMaskedCorners(mask);
    }

    // Drop shadows, in a companion below the view.
    let outer: Vec<&Shadow> = style.shadows.iter().filter(|s| !s.inset).collect();
    if outer.is_empty() {
        if let Some(c) = e.companion.take() {
            c.removeFromSuperview();
        }
    } else {
        let c = e
            .companion
            .get_or_insert_with(|| {
                let c = UIView::new(view.mtm());
                c.setUserInteractionEnabled(false);
                c
            })
            .clone();
        // A copy: removing mutates the live array.
        let old: Vec<Retained<CALayer>> = unsafe { c.layer().sublayers() }
            .map(|a| a.to_vec())
            .unwrap_or_default();
        for l in old {
            l.removeFromSuperlayer();
        }
        for sh in outer.iter().rev() {
            let l = shadow_layer(sh, &view, bounds);
            l.setShadowPath(Some(&path(&box_segments(&style, 0., 0., w, h, sh.spread))));
            // Like CSS: no shadow under a translucent box.
            let far = (sh.blur * 2. + sh.x.abs() + sh.y.abs() + sh.spread.abs()) as f64 + 40.;
            let outside = CGRect::new(
                CGPoint::new(-far, -far),
                CGSize::new(w as f64 + 2. * far, h as f64 + 2. * far),
            );
            let m = shape_mask(&box_, bounds, Some(outside));
            unsafe { l.setMask(Some(&m)) };
            c.layer().addSublayer(&l);
        }
        sync_companion(&view, &c);
    }
    CATransaction::commit();
}

/// The companion follows the view: same place, transform, opacity,
/// visibility, just below it.
fn sync_companion(view: &UIView, c: &UIView) {
    c.setBounds(view.bounds());
    c.setCenter(view.center());
    c.setTransform(view.transform());
    c.setAlpha(view.alpha());
    c.setHidden(view.isHidden());
    match view.superview() {
        Some(sup) => {
            if c.superview().as_deref() != Some(&*sup) {
                sup.insertSubview_belowSubview(c, view);
            }
        }
        None => c.removeFromSuperview(),
    }
}

/// The drop-shadow view of node `id`, if any (animations move it along).
pub fn companion(id: NodeId) -> Option<Retained<UIView>> {
    DECOR.with(|d| d.borrow().get(&id).and_then(|e| e.companion.clone()))
}

/// After `parent`'s subviews were set: companions just below their views,
/// effect views at the back.
pub fn after_children(parent: &UIView) {
    DECOR.with(|d| {
        let d = d.borrow();
        for e in d.values() {
            let (Some(view), Some(c)) = (&e.view, &e.companion) else {
                continue;
            };
            let here = view.superview().is_some_and(|s| *s == *parent);
            if here {
                parent.insertSubview_belowSubview(c, view);
                sync_companion(view, c);
            } else if c.superview().is_some_and(|s| *s == *parent) {
                c.removeFromSuperview();
            }
        }
        if let Some(e) = d.get(&(parent.tag() as NodeId))
            && let Some(ev) = &e.effect
        {
            parent.sendSubviewToBack(ev);
        }
    });
}

pub fn set_hidden(view: &UIView, hidden: bool) {
    if let Some(c) = companion(view.tag() as NodeId) {
        c.setHidden(hidden);
    }
}

pub fn destroy(id: NodeId) {
    if let Some(e) = DECOR.with(|d| d.borrow_mut().remove(&id))
        && let Some(c) = e.companion
    {
        c.removeFromSuperview();
    }
    BORDERS.with(|b| b.borrow_mut().retain(|_, (v, _)| v.tag() as NodeId != id));
    STATES.with(|s| s.borrow_mut().remove(&id));
    CURSORS.with(|s| s.borrow_mut().remove(&id));
    CLICKABLE.with(|s| s.borrow_mut().remove(&id));
    #[cfg(target_os = "ios")]
    POINTERS.with(|p| p.borrow_mut().remove(&id));
}

/// Colors resolved again for the new appearance (layers keep plain
/// `CGColor`s): shadows, stroked borders, gradients, uniform borders.
pub fn appearance_changed(traits: &objc2_ui_kit::UITraitCollection) {
    TRAITS.with(|t| *t.borrow_mut() = Some(traits.retain()));
    DECOR.with(|d| {
        for e in d.borrow_mut().values_mut() {
            layout_entry(e);
        }
    });
    let borders: Vec<(Retained<UIView>, Color)> =
        BORDERS.with(|b| b.borrow().values().cloned().collect());
    for (view, color) in borders {
        view.layer().setBorderColor(Some(&cg_color(color, &view)));
    }
}

// ---------------------------------------------------------------------------
// Interaction states
// ---------------------------------------------------------------------------

fn report(id: NodeId, state: Interaction, on: bool) {
    crate::imp::set_interaction(id, state, on);
}

fn wants(id: NodeId, state: Interaction) -> bool {
    STATES.with(|s| s.borrow().get(&id).is_some_and(|i| i.contains(state)))
}

/// Keyboard focus moved into or out of a text field or area (always
/// reported: ancestors may refine focus-within).
pub fn focus(id: NodeId, on: bool) {
    report(id, Interaction::Focus, on);
}

/// The tvOS focus engine moved: focused views are focused and hovered.
#[cfg(target_os = "tvos")]
pub fn tv_focus(id: NodeId, on: bool) {
    report(id, Interaction::Focus, on);
    if wants(id, Interaction::Hover) {
        report(id, Interaction::Hover, on);
    }
}

/// A container was touched (`true`) or the touch ended or was cancelled:
/// reports presses like `UIControl` highlighting (delayed in scroll views,
/// cancelled when they scroll). Called by `interaction::ContainerView`.
#[cfg_attr(target_os = "tvos", allow(dead_code))]
pub fn press(id: NodeId, on: bool) {
    if wants(id, Interaction::Active) {
        report(id, Interaction::Active, on);
    }
}

define_class!(
    /// Hover recognizers, button presses and the iPad pointer's styles.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismInteractionTarget"]
    pub struct InteractionTarget;

    unsafe impl NSObjectProtocol for InteractionTarget {}

    impl InteractionTarget {
        #[unsafe(method(onHover:))]
        fn on_hover(&self, g: &objc2_ui_kit::UIGestureRecognizer) {
            use objc2_ui_kit::UIGestureRecognizerState as S;
            let Some(view) = g.view() else { return };
            let on = match g.state() {
                S::Began => true,
                S::Ended | S::Cancelled | S::Failed => false,
                _ => return,
            };
            report(view.tag() as NodeId, Interaction::Hover, on);
        }

        #[unsafe(method(onPressDown:))]
        fn on_press_down(&self, sender: &UIView) {
            let id = sender.tag() as NodeId;
            if wants(id, Interaction::Active) {
                report(id, Interaction::Active, true);
            }
        }

        #[unsafe(method(onPressUp:))]
        fn on_press_up(&self, sender: &UIView) {
            let id = sender.tag() as NodeId;
            if wants(id, Interaction::Active) {
                report(id, Interaction::Active, false);
            }
        }
    }

    #[cfg(not(target_os = "tvos"))]
    unsafe impl UIPointerInteractionDelegate for InteractionTarget {
        #[unsafe(method_id(pointerInteraction:styleForRegion:))]
        fn style_for_region(
            &self,
            interaction: &objc2_ui_kit::UIPointerInteraction,
            _region: &objc2_ui_kit::UIPointerRegion,
        ) -> Option<Retained<objc2_ui_kit::UIPointerStyle>> {
            pointer_style(self.mtm(), interaction)
        }
    }
);

fn target(mtm: MainThreadMarker) -> Retained<InteractionTarget> {
    TARGET.with(|t| {
        t.borrow_mut()
            .get_or_insert_with(|| unsafe { msg_send![InteractionTarget::alloc(mtm), init] })
            .clone()
    })
}

/// The iPad pointer over a view: the system's hover effect on clickable
/// views (and `Cursor::Pointer`), a beam over `Cursor::Text`, the arrow
/// otherwise (UIKit has no crosshair, grab or resize pointers).
#[cfg(not(target_os = "tvos"))]
fn pointer_style(
    mtm: MainThreadMarker,
    interaction: &objc2_ui_kit::UIPointerInteraction,
) -> Option<Retained<objc2_ui_kit::UIPointerStyle>> {
    use objc2_ui_kit::{
        UIAxis, UIPointerEffect, UIPointerShape, UIPointerStyle, UITargetedPreview,
    };
    let view = interaction.view()?;
    let id = view.tag() as NodeId;
    let cursor = CURSORS.with(|c| c.borrow().get(&id).copied());
    let clickable = CLICKABLE.with(|c| c.borrow().contains(&id));
    match cursor {
        Some(Cursor::Text) => {
            let beam = UIPointerShape::beamWithPreferredLength_axis(20., UIAxis::Vertical, mtm);
            Some(UIPointerStyle::styleWithShape_constrainedAxes(
                &beam,
                UIAxis::empty(),
            ))
        }
        Some(Cursor::Pointer) | None if clickable || cursor.is_some() => {
            view.window()?;
            let preview = UITargetedPreview::initWithView(UITargetedPreview::alloc(mtm), &view);
            let effect = UIPointerEffect::effectWithPreview(&preview);
            Some(unsafe { UIPointerStyle::styleWithEffect_shape(&effect, None) })
        }
        _ => Some(UIPointerStyle::systemPointerStyle(mtm)),
    }
}

/// Pointer (iPad) and look (visionOS) feedback for clickable views and
/// views with a cursor.
fn update_pointer(view: &UIView) {
    let id = view.tag() as NodeId;
    let cursor = CURSORS.with(|c| c.borrow().get(&id).copied());
    let clickable = CLICKABLE.with(|c| c.borrow().contains(&id));
    #[cfg(target_os = "ios")]
    {
        let wanted = clickable || cursor.is_some();
        use objc2::runtime::ProtocolObject;
        let existing = POINTERS.with(|p| p.borrow().get(&id).cloned());
        match (wanted, existing) {
            (true, None) => {
                let mtm = view.mtm();
                let t = target(mtm);
                let pi = objc2_ui_kit::UIPointerInteraction::initWithDelegate(
                    objc2_ui_kit::UIPointerInteraction::alloc(mtm),
                    Some(ProtocolObject::from_ref(&*t)),
                );
                view.addInteraction(ProtocolObject::from_ref(&*pi));
                POINTERS.with(|p| p.borrow_mut().insert(id, pi));
            }
            (false, Some(pi)) => {
                view.removeInteraction(ProtocolObject::from_ref(&*pi));
                POINTERS.with(|p| p.borrow_mut().remove(&id));
            }
            // The style may have changed: asked again.
            (true, Some(pi)) => pi.invalidate(),
            _ => {}
        }
    }
    #[cfg(target_os = "visionos")]
    {
        let highlight = match cursor {
            Some(Cursor::Pointer) => true,
            None => clickable,
            _ => false,
        };
        let style = highlight.then(|| objc2_ui_kit::UIHoverStyle::automaticStyle(view.mtm()));
        view.setHoverStyle(style.as_deref());
    }
    #[cfg(target_os = "tvos")]
    let _ = (cursor, clickable);
}

/// A container became clickable (or not): pointer feedback.
pub fn set_clickable(view: &UIView, clickable: bool) {
    let id = view.tag() as NodeId;
    let changed = CLICKABLE.with(|c| {
        let mut c = c.borrow_mut();
        if clickable {
            c.insert(id)
        } else {
            c.remove(&id)
        }
    });
    if changed {
        update_pointer(view);
    }
}

/// The states `view` reports: hover recognizer, button press events.
pub fn set_interactions(view: &UIView, states: Interactions) {
    let id = view.tag() as NodeId;
    STATES.with(|s| s.borrow_mut().insert(id, states));
    let mtm = view.mtm();
    #[cfg(not(target_os = "tvos"))]
    {
        let hover = <objc2_ui_kit::UIHoverGestureRecognizer as objc2::ClassType>::class();
        let existing = view
            .gestureRecognizers()
            .and_then(|gs| gs.iter().find(|g| g.isKindOfClass(hover)));
        match (states.contains(Interaction::Hover), existing) {
            (true, None) => {
                let t = target(mtm);
                let target: &AnyObject = &t;
                let g = unsafe {
                    objc2_ui_kit::UIHoverGestureRecognizer::initWithTarget_action(
                        objc2_ui_kit::UIHoverGestureRecognizer::alloc(mtm),
                        Some(target),
                        Some(sel!(onHover:)),
                    )
                };
                view.addGestureRecognizer(&g);
            }
            (false, Some(g)) => view.removeGestureRecognizer(&g),
            _ => {}
        }
    }
    // Native buttons: their own control events.
    if states.contains(Interaction::Active)
        && let Some(button) =
            AsRef::<AnyObject>::as_ref(view).downcast_ref::<objc2_ui_kit::UIControl>()
    {
        use objc2_ui_kit::UIControlEvents as E;
        let t = target(mtm);
        let target: &AnyObject = &t;
        unsafe {
            button.removeTarget_action_forControlEvents(Some(target), None, E::AllEvents);
            button.addTarget_action_forControlEvents(
                Some(target),
                sel!(onPressDown:),
                E::TouchDown,
            );
            button.addTarget_action_forControlEvents(
                Some(target),
                sel!(onPressUp:),
                E::TouchUpInside | E::TouchUpOutside | E::TouchCancel,
            );
        }
    }
}
