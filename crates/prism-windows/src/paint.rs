//! Draws the scene with Direct2D, the same function for every target
//! (the window's frame, GDI, a WIC bitmap): Prism's `Style` as specified,
//! per-corner radii with anti-aliased clipping of children, solid and
//! dashed borders (per side too), outer and inset shadows (Gaussian blur),
//! linear, radial and conic gradients, group opacity, transforms, backdrop
//! blur where the target can be read back, then each kind's content.

use std::mem::ManuallyDrop;

use prism_core::decoration::{BorderStyle, GradientKind};
use prism_core::{Color, IdMap, NodeId, NodeKind, Style};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_F, D2D1_BEZIER_SEGMENT, D2D1_BORDER_MODE_HARD,
    D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
    D2D1_FILL_MODE_ALTERNATE, D2D1_FILL_MODE_WINDING, D2D1_GRADIENT_STOP,
};
use windows::Win32::Graphics::Direct2D::*;
use windows::core::{Interface, Result};
use windows_numerics::{Matrix3x2, Vector2};

use crate::fluent::Palette;
use crate::gfx::{Gfx, color_f};
use crate::images::Images;
use crate::scene::{Rect, Scene};
use crate::text::TextCache;

/// Device-dependent resources of one target: bitmaps of pictures, blurred
/// shadows.
#[derive(Default)]
pub struct TargetCache {
    pub generation: u64,
    pub bitmaps: IdMap<NodeId, (u64, ID2D1Bitmap1)>,
    shadows: std::collections::HashMap<(NodeId, usize), (ShadowKey, ID2D1Image)>,
}

impl TargetCache {
    pub fn new(generation: u64) -> TargetCache {
        TargetCache {
            generation,
            ..Default::default()
        }
    }

    pub fn forget(&mut self, id: NodeId) {
        self.bitmaps.remove(&id);
        self.shadows.retain(|(n, _), _| *n != id);
    }
}

#[derive(Clone, Copy, PartialEq)]
struct ShadowKey {
    w: u32,
    h: u32,
    radius: u32,
    corners: prism_core::Corners,
    blur: u32,
    spread: i32,
    color: [u32; 4],
    inset: bool,
}

/// A sub-part of a node under the pointer or pressed: a tab, the header's
/// Back or bar button, a scroll bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Part {
    Tab(usize),
    Back,
    BarItem(usize),
    ScrollBar,
}

/// What a paint needs.
pub struct Ctx<'a> {
    pub dc: &'a ID2D1DeviceContext,
    pub gfx: &'a Gfx,
    pub scene: &'a Scene,
    pub pal: &'a Palette,
    pub texts: &'a mut TextCache,
    pub images: &'a Images,
    pub cache: &'a mut TargetCache,
    /// Where each node was drawn (window, view units): repaints of what
    /// moved invalidate it.
    pub painted: &'a mut IdMap<NodeId, Rect>,
    /// What to repaint (window, view units); `None`: everything.
    pub clip: Option<Rect>,
    pub dpi: f32,
    /// The keyboard focus ring is shown (focus moved with the keyboard).
    pub focus_visible: bool,
    pub hover: Option<(NodeId, Part)>,
    pub pressed: Option<(NodeId, Part)>,
    pub brush: ID2D1SolidColorBrush,
    /// Dates in date pickers.
    pub locale: &'a prism_core::Locale,
}

pub fn rectf(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

/// A point through a matrix (row vectors, like Direct2D).
pub fn apply(m: &Matrix3x2, x: f32, y: f32) -> (f32, f32) {
    (x * m.M11 + y * m.M21 + m.M31, x * m.M12 + y * m.M22 + m.M32)
}

/// The window-space bounding box of a local rectangle.
pub fn bounds(m: &Matrix3x2, r: Rect) -> Rect {
    let pts = [
        apply(m, r.x, r.y),
        apply(m, r.right(), r.y),
        apply(m, r.x, r.bottom()),
        apply(m, r.right(), r.bottom()),
    ];
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in pts {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

fn axis_aligned(m: &Matrix3x2) -> bool {
    m.M12.abs() < 1e-5 && m.M21.abs() < 1e-5
}

/// The matrix of a node relative to its parent's content: its frame, its
/// sticky offset, then its transform around its center.
pub fn local_matrix(scene: &Scene, id: NodeId) -> Matrix3x2 {
    let Some(n) = scene.get(id) else {
        return Matrix3x2::identity();
    };
    let f = n.frame;
    let s = &n.style;
    let t = s.transform;
    let mut m = Matrix3x2::identity();
    let center = Vector2 {
        X: f.w / 2.,
        Y: f.h / 2.,
    };
    if t.sx() != 1. || t.sy() != 1. {
        m = m * Matrix3x2::scale_around(t.sx(), t.sy(), center);
    }
    if t.rotate != 0. {
        m = m * Matrix3x2::rotation_around(t.rotate, center);
    }
    let (px, py) = s.translate_percent;
    let dx = t.x + px * f.w / 100.;
    let dy = t.y + py * f.h / 100.;
    m * Matrix3x2::translation(f.x + dx, f.y + dy + scene.sticky_offset(n))
}

/// How far a node's drawing reaches outside its box (shadows, focus rings).
pub fn overhang(s: &Style) -> f32 {
    let mut d: f32 = 3.; // focus ring
    for sh in s.shadows.iter().filter(|sh| !sh.inset) {
        d = d.max(sh.blur * 1.5 + sh.spread.max(0.) + sh.x.abs().max(sh.y.abs()) + 1.);
    }
    d
}

impl Ctx<'_> {
    pub fn resolve(&self, c: Color) -> Color {
        self.pal.resolve(c)
    }

    pub fn solid(&self, c: Color) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&color_f(self.resolve(c))) };
        &self.brush
    }

    /// The default text color of a node.
    pub fn text_color(&self, s: &Style) -> Color {
        s.text_color.map_or(self.pal.text, |c| self.resolve(c))
    }

    /// The outline of a box `w` × `h` with `style`'s corners.
    pub fn box_geometry(
        &self,
        r: Rect,
        radius: f32,
        corners: prism_core::Corners,
    ) -> Result<ID2D1Geometry> {
        unsafe {
            if radius <= 0. {
                return self
                    .gfx
                    .d2d
                    .CreateRectangleGeometry(&rectf(r.x, r.y, r.w, r.h))?
                    .cast();
            }
            if corners == prism_core::Corners::ALL {
                let rr = D2D1_ROUNDED_RECT {
                    rect: rectf(r.x, r.y, r.w, r.h),
                    radiusX: radius.min(r.w / 2.).min(r.h / 2.).max(0.),
                    radiusY: radius.min(r.w / 2.).min(r.h / 2.).max(0.),
                };
                return self.gfx.d2d.CreateRoundedRectangleGeometry(&rr)?.cast();
            }
            let segs = prism_native::box_path(r.x, r.y, r.w, r.h, radius, corners);
            self.path_geometry(&segs, false)
        }
    }

    pub fn path_geometry(
        &self,
        segs: &[prism_core::Segment],
        even_odd: bool,
    ) -> Result<ID2D1Geometry> {
        use prism_core::Segment as S;
        unsafe {
            let g = self.gfx.d2d.CreatePathGeometry()?;
            let sink = g.Open()?;
            sink.SetFillMode(if even_odd {
                D2D1_FILL_MODE_ALTERNATE
            } else {
                D2D1_FILL_MODE_WINDING
            });
            let mut open = false;
            for s in segs {
                match *s {
                    S::Move(x, y) => {
                        if open {
                            sink.EndFigure(D2D1_FIGURE_END_OPEN_OR(false));
                        }
                        sink.BeginFigure(pt(x, y), D2D1_FIGURE_BEGIN_FILLED);
                        open = true;
                    }
                    S::Line(x, y) => {
                        if !open {
                            sink.BeginFigure(pt(x, y), D2D1_FIGURE_BEGIN_FILLED);
                            open = true;
                        } else {
                            sink.AddLine(pt(x, y));
                        }
                    }
                    S::Cubic(x1, y1, x2, y2, x, y) => {
                        if open {
                            sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                                point1: pt(x1, y1),
                                point2: pt(x2, y2),
                                point3: pt(x, y),
                            });
                        }
                    }
                    S::Close if open => {
                        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                        open = false;
                    }
                    // Arcs come as curves (`Widgets::set_drawing`).
                    _ => {}
                }
            }
            if open {
                sink.EndFigure(D2D1_FIGURE_END_OPEN_OR(false));
            }
            sink.Close()?;
            g.cast()
        }
    }

    /// Clips what follows to `geometry` (anti-aliased), with `opacity`.
    pub fn push_layer(&self, mask: Option<&ID2D1Geometry>, opacity: f32) {
        let mut params = D2D1_LAYER_PARAMETERS1 {
            contentBounds: rectf(-1e7, -1e7, 2e7, 2e7),
            geometricMask: ManuallyDrop::new(mask.cloned()),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity,
            opacityBrush: ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS1_NONE,
        };
        unsafe {
            self.dc.PushLayer(&params, None::<&ID2D1Layer>);
            ManuallyDrop::drop(&mut params.geometricMask);
        }
    }

    pub fn pop_layer(&self) {
        unsafe { self.dc.PopLayer() };
    }

    pub fn fill_rect(&self, r: Rect, c: Color) {
        unsafe {
            self.dc
                .FillRectangle(&rectf(r.x, r.y, r.w, r.h), self.solid(c))
        };
    }

    pub fn fill_rounded(&self, r: Rect, radius: f32, c: Color) {
        unsafe {
            let rr = D2D1_ROUNDED_RECT {
                rect: rectf(r.x, r.y, r.w, r.h),
                radiusX: radius,
                radiusY: radius,
            };
            self.dc.FillRoundedRectangle(&rr, self.solid(c));
        }
    }

    pub fn stroke_rounded(&self, r: Rect, radius: f32, width: f32, c: Color) {
        unsafe {
            let i = width / 2.;
            let rr = D2D1_ROUNDED_RECT {
                rect: rectf(r.x + i, r.y + i, r.w - width, r.h - width),
                radiusX: (radius - i).max(0.),
                radiusY: (radius - i).max(0.),
            };
            self.dc
                .DrawRoundedRectangle(&rr, self.solid(c), width, None);
        }
    }

    pub fn fill_ellipse(&self, cx: f32, cy: f32, rx: f32, ry: f32, c: Color) {
        unsafe {
            self.dc.FillEllipse(
                &D2D1_ELLIPSE {
                    point: pt(cx, cy),
                    radiusX: rx,
                    radiusY: ry,
                },
                self.solid(c),
            )
        };
    }

    pub fn stroke_ellipse(&self, cx: f32, cy: f32, r: f32, width: f32, c: Color) {
        unsafe {
            self.dc.DrawEllipse(
                &D2D1_ELLIPSE {
                    point: pt(cx, cy),
                    radiusX: r,
                    radiusY: r,
                },
                self.solid(c),
                width,
                None,
            )
        };
    }

    /// A one-line label in the UI font, `align`ed in `r` and vertically
    /// centered.
    pub fn label(
        &mut self,
        text: &str,
        r: Rect,
        size: f32,
        weight: prism_core::FontWeight,
        align: prism_core::TextAlign,
        color: Color,
    ) -> Result<()> {
        let st = crate::text::plain(text, size, weight, align);
        let Some(laid) = self.texts.plain(self.gfx, &st, Some(r.w.max(1.))) else {
            return Ok(());
        };
        let (_, h) = laid.size();
        let pal = *self.pal;
        laid.draw(
            self.dc,
            r.x,
            r.y + (r.h - h) / 2.,
            r.w,
            color,
            &|c| pal.resolve(c),
            true,
        )
    }

    /// A Segoe Fluent Icons glyph centered in `r`.
    pub fn icon(&mut self, glyph: char, r: Rect, size: f32, color: Color) -> Result<()> {
        let mut st = crate::text::plain(
            &glyph.to_string(),
            size,
            prism_core::FontWeight::Regular,
            prism_core::TextAlign::Center,
        );
        st.runs[0].font.family = prism_core::FontFamily::Named(self.gfx.fonts.icons.clone());
        let Some(laid) = self.texts.plain(self.gfx, &st, Some(r.w.max(1.))) else {
            return Ok(());
        };
        let (_, h) = laid.size();
        let pal = *self.pal;
        laid.draw(
            self.dc,
            r.x,
            r.y + (r.h - h) / 2.,
            r.w,
            color,
            &|c| pal.resolve(c),
            false,
        )
    }

    fn set_transform(&self, m: &Matrix3x2) {
        unsafe { self.dc.SetTransform(m) };
    }
}

/// Draws the whole scene (or the part in `ctx.clip`) on a cleared target.
pub fn draw(ctx: &mut Ctx) -> Result<()> {
    let m = Matrix3x2::identity();
    ctx.set_transform(&m);
    if let Some(c) = ctx.clip {
        unsafe {
            ctx.dc
                .PushAxisAlignedClip(&rectf(c.x, c.y, c.w, c.h), D2D1_ANTIALIAS_MODE_ALIASED)
        };
    }
    unsafe {
        ctx.dc.Clear(Some(&color_f(ctx.pal.window)));
        ctx.dc
            .SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
    }
    let roots: Vec<NodeId> = ctx.scene.roots().collect();
    for r in roots {
        draw_node(ctx, r, &m)?;
    }
    // Open overlays (sheets, dialogs, popovers) above everything.
    for o in ctx.scene.open_overlays() {
        draw_overlay(ctx, o)?;
    }
    ctx.set_transform(&m);
    if ctx.clip.is_some() {
        unsafe { ctx.dc.PopAxisAlignedClip() };
    }
    Ok(())
}

fn draw_overlay(ctx: &mut Ctx, id: NodeId) -> Result<()> {
    let scene = ctx.scene;
    let Some(n) = scene.get(id) else {
        return Ok(());
    };
    let config = prism_core::controls::OverlayConfig::decode(&n.config);
    let (ox, oy) = scene.origin(id);
    let dims = !matches!(config.kind, prism_core::OverlayKind::Popover)
        && !matches!(config.backdrop, Some(prism_core::controls::Backdrop::Clear));
    if dims {
        ctx.set_transform(&Matrix3x2::identity());
        let size = unsafe { ctx.dc.GetSize() };
        ctx.fill_rect(
            Rect::new(0., 0., size.width, size.height),
            Color::BLACK.with_alpha(if ctx.pal.dark { 0.45 } else { 0.3 }),
        );
    }
    let m = Matrix3x2::translation(ox, oy);
    for &c in &n.children {
        draw_node(ctx, c, &m)?;
    }
    Ok(())
}

/// Draws node `id` and its subtree; `parent` maps the parent's content
/// to the window.
pub fn draw_node(ctx: &mut Ctx, id: NodeId, parent: &Matrix3x2) -> Result<()> {
    let scene = ctx.scene;
    let Some(n) = scene.get(id) else {
        return Ok(());
    };
    if n.hidden {
        return Ok(());
    }
    let m = local_matrix(scene, id) * *parent;
    let f = n.frame;
    let s = &n.style;
    let local = Rect::new(0., 0., f.w, f.h);
    let window = bounds(&m, local.inflate(overhang(s)));
    // Outside what is repainted: skip it, and its children when it clips
    // them (else they may reach into the repainted area).
    let outside = ctx.clip.is_some_and(|c| c.intersect(&window).is_none());
    if outside && (n.clips() || n.children.is_empty()) {
        return Ok(());
    }
    ctx.painted.insert(id, window);
    let opacity = s.opacity.clamp(0., 1.);
    if opacity <= 0. {
        return Ok(());
    }
    ctx.set_transform(&m);
    let faded = opacity < 1.;
    if faded {
        ctx.push_layer(None, opacity);
    }
    if !outside {
        draw_box(ctx, id, local, &m)?;
        crate::controls::draw_content(ctx, id, local, &m)?;
    }
    // Children.
    let clips = n.clips();
    let radius = s.corner_radius;
    if clips {
        // CSS clips at the padding box: inside the border.
        let bw = if s.border_color.is_some() && !s.border_none {
            s.border_width
        } else {
            0.
        };
        let inner = Rect::new(bw, bw, (f.w - 2. * bw).max(0.), (f.h - 2. * bw).max(0.));
        if radius > 0. || !axis_aligned(&m) {
            let g = ctx.box_geometry(inner, (radius - bw).max(0.), s.rounded_corners)?;
            ctx.push_layer(Some(&g), 1.);
        } else {
            unsafe {
                ctx.dc.PushAxisAlignedClip(
                    &rectf(inner.x, inner.y, inner.w, inner.h),
                    D2D1_ANTIALIAS_MODE_ALIASED,
                )
            };
        }
    }
    let content = if n.kind == NodeKind::Scroll {
        Matrix3x2::translation(-n.scroll.0, -n.scroll.1) * m
    } else {
        m
    };
    let shown = scene.shown_children(id);
    // A scroll view draws only what is in view (long lists).
    let viewport =
        (n.kind == NodeKind::Scroll).then(|| Rect::new(n.scroll.0, n.scroll.1, f.w, f.h));
    let mut sticky = Vec::new();
    for &c in shown.iter() {
        let Some(cn) = scene.get(c) else { continue };
        if cn.sticky.is_some() {
            sticky.push(c);
            continue;
        }
        if let Some(v) = viewport {
            let cf = cn.frame.inflate(overhang(&cn.style));
            if cf.intersect(&v).is_none() && cn.style.transform.is_identity() {
                continue;
            }
        }
        draw_node(ctx, c, &content)?;
    }
    for c in sticky {
        draw_node(ctx, c, &content)?;
    }
    ctx.set_transform(&m);
    if clips {
        if radius > 0. || !axis_aligned(&m) {
            ctx.pop_layer();
        } else {
            unsafe { ctx.dc.PopAxisAlignedClip() };
        }
    }
    if !outside {
        crate::controls::draw_over(ctx, id, local)?;
    }
    if faded {
        ctx.pop_layer();
    }
    Ok(())
}

/// Shadows, backdrop blur, background, gradient, inset shadows, border.
fn draw_box(ctx: &mut Ctx, id: NodeId, r: Rect, m: &Matrix3x2) -> Result<()> {
    let n = ctx.scene.get(id).expect("node");
    let s = &n.style;
    let radius = s.corner_radius;
    let corners = s.rounded_corners;
    // Outer shadows, the first one on top.
    for (i, sh) in s.shadows.iter().enumerate().rev() {
        if !sh.inset && sh.color.a > 0. {
            draw_shadow(ctx, id, i, r, radius, corners, sh, s)?;
        }
    }
    let has_fill = s.background.is_some() || s.gradient.is_some();
    if s.backdrop_blur > 0. {
        backdrop_blur(ctx, r, radius, corners, s.backdrop_blur, m)?;
    }
    if has_fill {
        let g = ctx.box_geometry(r, radius, corners)?;
        if let Some(bg) = s.background {
            unsafe { ctx.dc.FillGeometry(&g, ctx.solid(bg), None) };
        }
        if let Some(gr) = &s.gradient {
            fill_gradient(ctx, &g, r, gr)?;
        }
    }
    for (i, sh) in s.shadows.iter().enumerate().rev() {
        if sh.inset && sh.color.a > 0. {
            draw_shadow(ctx, id, i, r, radius, corners, sh, s)?;
        }
    }
    draw_border(ctx, s, r)
}

pub fn draw_border(ctx: &Ctx, s: &Style, r: Rect) -> Result<()> {
    if s.border_none {
        return Ok(());
    }
    let label = ctx.text_color(s);
    let radius = s.corner_radius;
    if s.has_side_borders() {
        // Side by side: strips along each edge, clipped to the box.
        let [t, rt, b, l] = s.borders();
        let g = ctx.box_geometry(r, radius, s.rounded_corners)?;
        ctx.push_layer(Some(&g), 1.);
        let strip = |side: prism_core::decoration::BorderSide, rect: Rect| {
            if side.width > 0. {
                ctx.fill_rect(rect, side.color.unwrap_or(label));
            }
        };
        strip(t, Rect::new(r.x, r.y, r.w, t.width));
        strip(b, Rect::new(r.x, r.bottom() - b.width, r.w, b.width));
        strip(l, Rect::new(r.x, r.y, l.width, r.h));
        strip(rt, Rect::new(r.right() - rt.width, r.y, rt.width, r.h));
        ctx.pop_layer();
        return Ok(());
    }
    let w = s.border_width;
    if w <= 0. {
        return Ok(());
    }
    let color = s.border_color.unwrap_or(label);
    let inner = Rect::new(
        r.x + w / 2.,
        r.y + w / 2.,
        (r.w - w).max(0.),
        (r.h - w).max(0.),
    );
    let g = ctx.box_geometry(inner, (radius - w / 2.).max(0.), s.rounded_corners)?;
    unsafe {
        let stroke = match s.border_style.dash(w) {
            Some((on, off)) => {
                let dotted = s.border_style == BorderStyle::Dotted;
                let cap = if dotted {
                    D2D1_CAP_STYLE_ROUND
                } else {
                    D2D1_CAP_STYLE_FLAT
                };
                // Dash lengths are in stroke widths.
                let dashes = [on.max(0.001) / w, off / w];
                let factory: &ID2D1Factory = &ctx.gfx.d2d;
                Some(factory.CreateStrokeStyle(
                    &D2D1_STROKE_STYLE_PROPERTIES {
                        startCap: cap,
                        endCap: cap,
                        dashCap: cap,
                        lineJoin: D2D1_LINE_JOIN_MITER,
                        miterLimit: 10.,
                        dashStyle: D2D1_DASH_STYLE_CUSTOM,
                        dashOffset: 0.,
                    },
                    Some(&dashes),
                )?)
            }
            None => None,
        };
        ctx.dc
            .DrawGeometry(&g, ctx.solid(color), w, stroke.as_ref());
    }
    Ok(())
}

fn fill_gradient(
    ctx: &Ctx,
    g: &ID2D1Geometry,
    r: Rect,
    gr: &prism_core::decoration::Gradient,
) -> Result<()> {
    if gr.stops.is_empty() {
        return Ok(());
    }
    let stops: Vec<D2D1_GRADIENT_STOP> = gr
        .stops
        .iter()
        .map(|s| D2D1_GRADIENT_STOP {
            position: s.at.clamp(0., 1.),
            color: color_f(ctx.resolve(s.color)),
        })
        .collect();
    unsafe {
        let rt: &ID2D1RenderTarget = ctx.dc;
        let coll =
            rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
        let (cx, cy) = (r.x + r.w / 2., r.y + r.h / 2.);
        match gr.kind {
            GradientKind::Linear { angle } => {
                // CSS: 0deg points up, clockwise; the line spans the box's
                // corners in that direction.
                let a = angle.to_radians();
                let (dx, dy) = (a.sin(), -a.cos());
                let half = (r.w * dx.abs() + r.h * dy.abs()) / 2.;
                let brush = rt.CreateLinearGradientBrush(
                    &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                        startPoint: Vector2 {
                            X: cx - dx * half,
                            Y: cy - dy * half,
                        },
                        endPoint: Vector2 {
                            X: cx + dx * half,
                            Y: cy + dy * half,
                        },
                    },
                    None,
                    &coll,
                )?;
                ctx.dc.FillGeometry(g, &brush, None);
            }
            GradientKind::Radial => {
                // An ellipse reaching the corners (CSS farthest-corner).
                let k = std::f32::consts::SQRT_2;
                let brush = rt.CreateRadialGradientBrush(
                    &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                        center: Vector2 { X: cx, Y: cy },
                        gradientOriginOffset: Vector2 { X: 0., Y: 0. },
                        radiusX: r.w / 2. * k,
                        radiusY: r.h / 2. * k,
                    },
                    None,
                    &coll,
                )?;
                ctx.dc.FillGeometry(g, &brush, None);
            }
            GradientKind::Conic { from } => {
                // No conic brush in Direct2D: thin wedges, each its color.
                ctx.push_layer(Some(g), 1.);
                let radius = (r.w * r.w + r.h * r.h).sqrt();
                let steps = 180;
                for i in 0..steps {
                    let t0 = i as f32 / steps as f32;
                    let t1 = (i + 1) as f32 / steps as f32;
                    let c = sample(&gr.stops, (t0 + t1) / 2.);
                    let a0 = (from + t0 * 360.).to_radians();
                    let a1 = (from + t1 * 360.).to_radians() + 0.004;
                    let p = |a: f32| {
                        prism_core::Segment::Line(cx + radius * a.sin(), cy - radius * a.cos())
                    };
                    let wedge = [
                        prism_core::Segment::Move(cx, cy),
                        p(a0),
                        p(a1),
                        prism_core::Segment::Close,
                    ];
                    let geo = ctx.path_geometry(&wedge, false)?;
                    ctx.dc.FillGeometry(&geo, ctx.solid(c), None);
                }
                ctx.pop_layer();
            }
        }
    }
    Ok(())
}

/// The gradient's color at `t` (straight-alpha interpolation).
fn sample(stops: &[prism_core::decoration::GradientStop], t: f32) -> Color {
    let first = stops[0];
    if t <= first.at {
        return first.color;
    }
    for w in stops.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.at {
            let k = if b.at > a.at {
                (t - a.at) / (b.at - a.at)
            } else {
                1.
            };
            let mix = |x: f32, y: f32| x + (y - x) * k;
            let (ca, cb) = (a.color, b.color);
            return Color {
                r: mix(ca.r, cb.r),
                g: mix(ca.g, cb.g),
                b: mix(ca.b, cb.b),
                a: mix(ca.a, cb.a),
                dark: match (ca.dark, cb.dark) {
                    (Some(x), Some(y)) => Some([
                        mix(x[0], y[0]),
                        mix(x[1], y[1]),
                        mix(x[2], y[2]),
                        mix(x[3], y[3]),
                    ]),
                    _ => None,
                },
            };
        }
    }
    stops[stops.len() - 1].color
}

/// A blurred copy of the box's shape (CSS `box-shadow`): outside the box
/// for a drop shadow, inside it (clipped) for an inset one.
#[allow(clippy::too_many_arguments)]
fn draw_shadow(
    ctx: &mut Ctx,
    id: NodeId,
    index: usize,
    r: Rect,
    radius: f32,
    corners: prism_core::Corners,
    sh: &prism_core::decoration::Shadow,
    s: &Style,
) -> Result<()> {
    let color = ctx.resolve(sh.color);
    if sh.blur <= 0.5 && !sh.inset {
        // A hard shadow (rings): the grown shape, filled.
        let grown = r.inflate(sh.spread).offset(sh.x, sh.y);
        let g = ctx.box_geometry(grown, (radius + sh.spread).max(0.), corners)?;
        let opaque_box = s.background.is_some_and(|b| ctx.resolve(b).a >= 1.);
        if opaque_box {
            unsafe { ctx.dc.FillGeometry(&g, ctx.solid(color), None) };
        } else {
            // Not under the box: ring around it only.
            let inner = ctx.box_geometry(r, radius, corners)?;
            let ring = combine(ctx, &g, &inner, D2D1_COMBINE_MODE_EXCLUDE)?;
            unsafe { ctx.dc.FillGeometry(&ring, ctx.solid(color), None) };
        }
        return Ok(());
    }
    let key = ShadowKey {
        w: (r.w * 4.) as u32,
        h: (r.h * 4.) as u32,
        radius: (radius * 4.) as u32,
        corners,
        blur: (sh.blur * 4.) as u32,
        spread: (sh.spread * 4.) as i32,
        color: [color.r, color.g, color.b, color.a].map(f32::to_bits),
        inset: sh.inset,
    };
    let pad = sh.blur * 1.5 + 2.;
    let image = match ctx.cache.shadows.get(&(id, index)) {
        Some((k, img)) if *k == key => img.clone(),
        _ => {
            let img = make_shadow(ctx, r, radius, corners, sh, color, pad)?;
            ctx.cache.shadows.insert((id, index), (key, img.clone()));
            img
        }
    };
    unsafe {
        if sh.inset {
            let g = ctx.box_geometry(r, radius, corners)?;
            ctx.push_layer(Some(&g), 1.);
            ctx.dc.DrawImage(
                &image,
                Some(&Vector2 {
                    X: r.x - pad + sh.x,
                    Y: r.y - pad + sh.y,
                }),
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            ctx.pop_layer();
        } else {
            // Not under a see-through box (CSS clips the shadow there).
            let opaque_box =
                s.background.is_some_and(|b| ctx.resolve(b).a >= 1.) && s.gradient.is_none();
            let clip = if opaque_box {
                None
            } else {
                let big = ctx.box_geometry(
                    r.inflate(pad * 2. + sh.spread.abs() + sh.x.abs() + sh.y.abs()),
                    0.,
                    corners,
                )?;
                let inner = ctx.box_geometry(r, radius, corners)?;
                Some(combine(ctx, &big, &inner, D2D1_COMBINE_MODE_EXCLUDE)?)
            };
            if let Some(c) = &clip {
                ctx.push_layer(Some(c), 1.);
            }
            ctx.dc.DrawImage(
                &image,
                Some(&Vector2 {
                    X: r.x - sh.spread - pad + sh.x,
                    Y: r.y - sh.spread - pad + sh.y,
                }),
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            if clip.is_some() {
                ctx.pop_layer();
            }
        }
    }
    Ok(())
}

fn combine(
    ctx: &Ctx,
    a: &ID2D1Geometry,
    b: &ID2D1Geometry,
    mode: D2D1_COMBINE_MODE,
) -> Result<ID2D1Geometry> {
    unsafe {
        let g = ctx.gfx.d2d.CreatePathGeometry()?;
        let sink = g.Open()?;
        a.CombineWithGeometry(b, mode, None, 0.25, &sink)?;
        sink.Close()?;
        g.cast()
    }
}

/// The shadow's image: the shape (or, inset, the frame around the shape)
/// in black on a transparent bitmap, through Direct2D's shadow effect.
fn make_shadow(
    ctx: &Ctx,
    r: Rect,
    radius: f32,
    corners: prism_core::Corners,
    sh: &prism_core::decoration::Shadow,
    color: Color,
    pad: f32,
) -> Result<ID2D1Image> {
    unsafe {
        let (w, h) = if sh.inset {
            (r.w + 2. * pad, r.h + 2. * pad)
        } else {
            (r.w + 2. * (pad + sh.spread), r.h + 2. * (pad + sh.spread))
        };
        let size = D2D_SIZE_F {
            width: w.max(1.),
            height: h.max(1.),
        };
        let brt = ctx.dc.CreateCompatibleRenderTarget(
            Some(&size),
            None,
            None,
            D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE,
        )?;
        brt.BeginDraw();
        brt.Clear(Some(&color_f(Color::TRANSPARENT)));
        let black = brt.CreateSolidColorBrush(&color_f(Color::BLACK), None)?;
        if sh.inset {
            // The frame around the box, shrunk by the spread.
            let outer = ctx.box_geometry(Rect::new(0., 0., w, h), 0., corners)?;
            let hole = ctx.box_geometry(
                Rect::new(
                    pad + sh.spread,
                    pad + sh.spread,
                    (r.w - 2. * sh.spread).max(0.),
                    (r.h - 2. * sh.spread).max(0.),
                ),
                (radius - sh.spread).max(0.),
                corners,
            )?;
            let frame = combine(ctx, &outer, &hole, D2D1_COMBINE_MODE_EXCLUDE)?;
            brt.FillGeometry(&frame, &black, None);
        } else {
            let shape = ctx.box_geometry(
                Rect::new(pad, pad, r.w + 2. * sh.spread, r.h + 2. * sh.spread),
                (radius + sh.spread).max(0.),
                corners,
            )?;
            brt.FillGeometry(&shape, &black, None);
        }
        brt.EndDraw(None, None)?;
        let bmp = brt.GetBitmap()?;
        let fx = ctx.dc.CreateEffect(&CLSID_D2D1Shadow)?;
        fx.SetInput(0, &bmp.cast::<ID2D1Image>()?, true);
        fx.SetValue(
            D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION.0 as u32,
            D2D1_PROPERTY_TYPE_FLOAT,
            &(sh.blur / 2.).max(0.).to_le_bytes(),
        )?;
        let mut bytes = Vec::with_capacity(16);
        for v in [color.r, color.g, color.b, color.a] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        fx.SetValue(
            D2D1_SHADOW_PROP_COLOR.0 as u32,
            D2D1_PROPERTY_TYPE_VECTOR4,
            &bytes,
        )?;
        fx.GetOutput()
    }
}

/// Blurs what is already drawn under the box (best effort: targets that
/// can be read back, the window's frame).
fn backdrop_blur(
    ctx: &mut Ctx,
    r: Rect,
    radius: f32,
    corners: prism_core::Corners,
    blur: f32,
    m: &Matrix3x2,
) -> Result<()> {
    let scale = ctx.dpi / 96.;
    let wb = bounds(m, r);
    let px = crate::win::px_rect(wb, scale);
    let Some(copy) = crate::gfx::copy_backdrop(ctx.dc, px, ctx.dpi) else {
        return Ok(());
    };
    unsafe {
        let fx = ctx.dc.CreateEffect(&CLSID_D2D1GaussianBlur)?;
        fx.SetInput(0, &copy.cast::<ID2D1Image>()?, true);
        fx.SetValue(
            D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
            D2D1_PROPERTY_TYPE_FLOAT,
            &(blur / 2.).to_le_bytes(),
        )?;
        fx.SetValue(
            D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
            D2D1_PROPERTY_TYPE_ENUM,
            &(D2D1_BORDER_MODE_HARD.0 as u32).to_le_bytes(),
        )?;
        let g = ctx.box_geometry(r, radius, corners)?;
        ctx.push_layer(Some(&g), 1.);
        // The copy is in window pixels: draw it untransformed.
        ctx.set_transform(&Matrix3x2::identity());
        ctx.dc.DrawImage(
            &fx.GetOutput()?,
            Some(&Vector2 {
                X: px.left as f32 / scale,
                Y: px.top as f32 / scale,
            }),
            None,
            D2D1_INTERPOLATION_MODE_LINEAR,
            D2D1_COMPOSITE_MODE_SOURCE_OVER,
        );
        ctx.set_transform(m);
        ctx.pop_layer();
    }
    Ok(())
}

/// `D2D1_FIGURE_END_OPEN` (false) or closed.
#[allow(non_snake_case)]
fn D2D1_FIGURE_END_OPEN_OR(
    closed: bool,
) -> windows::Win32::Graphics::Direct2D::Common::D2D1_FIGURE_END {
    if closed {
        D2D1_FIGURE_END_CLOSED
    } else {
        windows::Win32::Graphics::Direct2D::Common::D2D1_FIGURE_END_OPEN
    }
}
