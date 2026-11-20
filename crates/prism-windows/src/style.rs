//! Prism's box styles on XAML elements: a `Border` for the background
//! (solid, `LinearGradientBrush`, `RadialGradientBrush`, `AcrylicBrush`
//! for a backdrop blur), the border and per-corner radii; a `Rectangle`
//! with a dash array for dashed and dotted borders; Composition drop
//! shadows (`DropShadow` on a `LayerVisual` of a rounded rectangle) for
//! exact offset, blur, spread and color; opacity and a `CompositeTransform`
//! (scale and rotation around the center, then translation).

use prism_core::decoration::{BorderStyle, Gradient, GradientKind, Shadow};
use prism_core::{Color, Corners, Style};
use windows::Foundation::Point;
use windows_core::{Interface, Result};
use windows_numerics::{Vector2, Vector3};

use crate::fluent::Palette;
use crate::scene::Rect;
use crate::winui::bindings::Microsoft::UI::Composition::{
    CompositionRoundedRectangleGeometry, CompositionSpriteShape, CompositionVisualSurface,
    Compositor, DropShadow, ShapeVisual, SpriteVisual,
};
use crate::winui::xaml::Controls::{Border, Canvas};
use crate::winui::xaml::Hosting::ElementCompositionPreview;
use crate::winui::xaml::Media::{
    AcrylicBrush, Brush, BrushMappingMode, DoubleCollection, GradientStop, GradientStopCollection,
    LinearGradientBrush, RadialGradientBrush,
};
use crate::winui::xaml::Shapes::Rectangle;
use crate::winui::xaml::{CornerRadius, UIElement};
use crate::winui::{brush, radius, thickness, win_color};

/// The radius of each corner (top left, top right, bottom right, bottom
/// left), only the rounded ones (`Style::rounded_corners`), at most half
/// the box.
pub fn corners(s: &Style, w: f32, h: f32) -> (f32, f32, f32, f32) {
    let r = s.corner_radius.min(w / 2.).min(h / 2.).max(0.);
    let c = s.rounded_corners;
    let on = |k: Corners| if c.contains(k) { r } else { 0. };
    (
        on(Corners::TOP_LEADING),
        on(Corners::TOP_TRAILING),
        on(Corners::BOTTOM_TRAILING),
        on(Corners::BOTTOM_LEADING),
    )
}

pub fn corner_radius(s: &Style, w: f32, h: f32) -> CornerRadius {
    let (a, b, c, d) = corners(s, w, h);
    radius(a, b, c, d)
}

/// Whether the node draws a box: a background, a gradient, a border, a
/// radius on something, a backdrop blur.
pub fn has_box(s: &Style) -> bool {
    s.background.is_some()
        || s.gradient.is_some()
        || s.backdrop_blur > 0.
        || (!s.border_none && s.borders().iter().any(|b| b.width > 0.))
}

/// The background brush: solid, gradient, or acrylic for a backdrop blur.
pub fn background(s: &Style, pal: &Palette, w: f32, h: f32) -> Result<Option<Brush>> {
    if let Some(g) = &s.gradient
        && !g.stops.is_empty()
    {
        return gradient(g, pal, w, h).map(Some);
    }
    let Some(bg) = s.background.map(|c| pal.fill(c)) else {
        return Ok(None);
    };
    if s.backdrop_blur > 0. {
        let a = AcrylicBrush::new()?;
        a.SetTintColor(win_color(Color { a: 1., ..bg }))?;
        a.SetTintOpacity(bg.a as f64)?;
        a.SetFallbackColor(win_color(bg))?;
        return Ok(Some(a.cast()?));
    }
    Ok(Some(brush(bg)?.cast()?))
}

fn stops(g: &Gradient, pal: &Palette) -> Result<GradientStopCollection> {
    let coll = GradientStopCollection::new()?;
    for s in &g.stops {
        let stop = GradientStop::new()?;
        stop.SetColor(win_color(pal.fill(s.color)))?;
        stop.SetOffset(s.at.clamp(0., 1.) as f64)?;
        coll.Append(&stop)?;
    }
    Ok(coll)
}

/// A CSS gradient for a `w` × `h` box.
pub fn gradient(g: &Gradient, pal: &Palette, w: f32, h: f32) -> Result<Brush> {
    match g.kind {
        GradientKind::Linear { angle } => linear(g, pal, w, h, angle),
        GradientKind::Radial => {
            // CSS `radial-gradient()`: an ellipse the box's shape reaching
            // its corners (farthest-corner).
            let b = RadialGradientBrush::new()?;
            b.SetMappingMode(BrushMappingMode::RelativeToBoundingBox)?;
            b.SetCenter(Point { X: 0.5, Y: 0.5 })?;
            b.SetGradientOrigin(Point { X: 0.5, Y: 0.5 })?;
            b.SetRadiusX(std::f64::consts::FRAC_1_SQRT_2)?;
            b.SetRadiusY(std::f64::consts::FRAC_1_SQRT_2)?;
            let coll = b.GradientStops()?;
            for s in &g.stops {
                let stop = GradientStop::new()?;
                stop.SetColor(win_color(pal.fill(s.color)))?;
                stop.SetOffset(s.at.clamp(0., 1.) as f64)?;
                coll.Append(&stop)?;
            }
            b.cast()
        }
        // XAML has no conic gradient: its pixels, in a bitmap.
        GradientKind::Conic { .. } => crate::decor::conic(g, pal, w, h),
    }
}

fn linear(g: &Gradient, pal: &Palette, w: f32, h: f32, angle: f32) -> Result<Brush> {
    // CSS: 0deg points up, clockwise; the line spans the box's corners in
    // that direction.
    let a = angle.to_radians();
    let (dx, dy) = (a.sin(), -a.cos());
    let half = (w * dx.abs() + h * dy.abs()) / 2.;
    let (cx, cy) = (w / 2., h / 2.);
    let b = LinearGradientBrush::new()?;
    b.SetMappingMode(BrushMappingMode::Absolute)?;
    b.SetStartPoint(Point {
        X: cx - dx * half,
        Y: cy - dy * half,
    })?;
    b.SetEndPoint(Point {
        X: cx + dx * half,
        Y: cy + dy * half,
    })?;
    b.SetGradientStops(&stops(g, pal)?)?;
    b.cast()
}

/// The border's color and widths (top, right, bottom, left); `None`
/// when there is none (sides of different colors are `decor.rs` paths).
pub fn border(s: &Style, pal: &Palette, text: Color) -> Option<(Color, [f32; 4])> {
    if s.border_none {
        return None;
    }
    let sides = s.borders();
    let widths = sides.map(|b| b.width.max(0.));
    let first = sides.iter().find(|b| b.width > 0.)?;
    let color = pal.edge(first.color.unwrap_or(text));
    Some((color, widths))
}

/// Sets `b` up as the node's box.
pub fn apply_box(b: &Border, s: &Style, pal: &Palette, text: Color, r: Rect) -> Result<()> {
    match background(s, pal, r.w, r.h)? {
        Some(bg) => b.SetBackground(&bg)?,
        None => b.SetBackground(None::<&Brush>)?,
    }
    b.SetCornerRadius(corner_radius(s, r.w, r.h))?;
    let solid = s.border_style == BorderStyle::Solid;
    // Sides of different colors are paths (`decor.rs`).
    let one = solid && !crate::decor::split_sides(s);
    match border(s, pal, text).filter(|_| one) {
        Some((color, [t, rt, bt, l])) => {
            b.SetBorderBrush(&brush(color)?)?;
            b.SetBorderThickness(thickness(l, t, rt, bt))?;
        }
        None => {
            b.SetBorderBrush(None::<&Brush>)?;
            b.SetBorderThickness(thickness(0., 0., 0., 0.))?;
        }
    }
    Ok(())
}

/// A dashed or dotted border, as a stroked `Rectangle`.
pub fn apply_dash(rect: &Rectangle, s: &Style, pal: &Palette, text: Color, r: Rect) -> Result<()> {
    let w = s.border_width.max(1.);
    let color = pal.edge(s.border_color.unwrap_or(text));
    rect.SetStroke(&brush(color)?)?;
    rect.SetStrokeThickness(w as f64)?;
    let (on, off) = s.border_style.dash(w).unwrap_or((w * 3., w * 3.));
    let dashes = DoubleCollection::new()?;
    // XAML dash lengths are in stroke widths.
    dashes.Append((on.max(0.001) / w) as f64)?;
    dashes.Append((off / w) as f64)?;
    rect.SetStrokeDashArray(&dashes)?;
    let cap = if s.border_style == BorderStyle::Dotted {
        crate::winui::xaml::Media::PenLineCap::Round
    } else {
        crate::winui::xaml::Media::PenLineCap::Flat
    };
    rect.SetStrokeDashCap(cap)?;
    let rad = (corners(s, r.w, r.h).0 - w / 2.).max(0.) as f64;
    rect.SetRadiusX(rad)?;
    rect.SetRadiusY(rad)?;
    rect.SetWidth(r.w as f64)?;
    rect.SetHeight(r.h as f64)?;
    Ok(())
}

/// Opacity and transform of a node's outer element: the element's
/// Composition facades (`Opacity`, `Translation`, `Rotation`, `Scale`
/// around `CenterPoint`), which `motion.rs` animates.
pub fn apply_transform(outer: &Canvas, s: &Style, r: Rect, sticky: f32) -> Result<()> {
    let v = crate::motion::Visual::of(s, r.w, r.h, sticky);
    crate::motion::set_visual(&outer.cast()?, v, r.w, r.h)
}

/// The drop shadows of a box, drawn under it: per shadow, an invisible
/// `SpriteVisual` casting a `DropShadow` masked by the box's rounded
/// rectangle (a `ShapeVisual` captured by a `CompositionVisualSurface`),
/// on a host element placed under the node's `Border`.
pub struct Shadows {
    pub host: Canvas,
    layers: Vec<ShadowLayer>,
}

struct ShadowLayer {
    sprite: SpriteVisual,
    shadow: DropShadow,
    geometry: CompositionRoundedRectangleGeometry,
    shape: ShapeVisual,
    surface: CompositionVisualSurface,
}

impl Shadows {
    pub fn new() -> Result<Shadows> {
        let host = Canvas::new()?;
        host.SetIsHitTestVisible(false)?;
        Ok(Shadows {
            host,
            layers: Vec::new(),
        })
    }

    /// Shows `shadows` (inset ones excepted: phase 3) for an `r`-sized box.
    pub fn apply(&mut self, s: &Style, pal: &Palette, r: Rect) -> Result<()> {
        let outer: Vec<&Shadow> = s
            .shadows
            .iter()
            .filter(|sh| !sh.inset && sh.color.a > 0.)
            .collect();
        let host: UIElement = self.host.cast()?;
        if outer.is_empty() {
            if !self.layers.is_empty() {
                ElementCompositionPreview::SetElementChildVisual(
                    &host,
                    None::<&crate::winui::bindings::Microsoft::UI::Composition::Visual>,
                )?;
                self.layers.clear();
            }
            return Ok(());
        }
        let compositor: Compositor =
            ElementCompositionPreview::GetElementVisual(&host)?.Compositor()?;
        if self.layers.len() != outer.len() {
            let container = compositor.CreateContainerVisual()?;
            self.layers.clear();
            // The first shadow is on top (CSS).
            for _ in &outer {
                let geometry = compositor.CreateRoundedRectangleGeometry()?;
                let shape_sprite: CompositionSpriteShape =
                    compositor.CreateSpriteShapeWithGeometry(&geometry)?;
                shape_sprite.SetFillBrush(
                    &compositor.CreateColorBrushWithColor(win_color(Color::BLACK))?,
                )?;
                let shape = compositor.CreateShapeVisual()?;
                shape.Shapes()?.Append(&shape_sprite)?;
                let surface = compositor.CreateVisualSurface()?;
                surface.SetSourceVisual(&shape)?;
                let mask = compositor.CreateSurfaceBrushWithSurface(&surface)?;
                let shadow: DropShadow = compositor.CreateDropShadow()?;
                shadow.SetMask(&mask)?;
                let sprite = compositor.CreateSpriteVisual()?;
                sprite.SetShadow(&shadow)?;
                container.Children()?.InsertAtBottom(&sprite)?;
                self.layers.push(ShadowLayer {
                    sprite,
                    shadow,
                    geometry,
                    shape,
                    surface,
                });
            }
            ElementCompositionPreview::SetElementChildVisual(&host, &container)?;
        }
        let (tl, ..) = corners(s, r.w, r.h);
        for (l, sh) in self.layers.iter().zip(&outer) {
            let spread = sh.spread;
            let size = Vector2 {
                X: (r.w + 2. * spread).max(0.),
                Y: (r.h + 2. * spread).max(0.),
            };
            l.geometry.SetSize(size)?;
            let rad = (tl + spread).max(0.);
            l.geometry.SetCornerRadius(Vector2 { X: rad, Y: rad })?;
            l.shape.SetSize(size)?;
            l.surface.SetSourceSize(size)?;
            l.sprite.SetSize(size)?;
            l.sprite.SetOffset(Vector3 {
                X: -spread,
                Y: -spread,
                Z: 0.,
            })?;
            // Composition blurs about twice as wide as CSS for a radius.
            l.shadow.SetBlurRadius(sh.blur)?;
            l.shadow.SetOffset(Vector3 {
                X: sh.x,
                Y: sh.y,
                Z: 0.,
            })?;
            l.shadow.SetColor(win_color(pal.shadow(sh.color)))?;
        }
        self.host.SetWidth(r.w as f64)?;
        self.host.SetHeight(r.h as f64)?;
        Ok(())
    }
}
