//! The box styles XAML's `Border` cannot draw, beside it in the node's
//! outer canvas:
//!
//! - inset shadows: per shadow, a Composition `DropShadow` cast by a frame
//!   around the box's inside (a rounded rectangle stroked outward), clipped
//!   to the padding box;
//! - borders whose sides differ in color: one `Path` per side, cut at the
//!   corners like CSS (`paint::border_sides`);
//! - conic gradients: their pixels (`paint::conic`) in a `WriteableBitmap`
//!   shown by an `ImageBrush` (the `Border` keeps its corner radii);
//! - children clipped to rounded corners (`overflow: hidden`): a
//!   Composition geometric clip on the children's canvas.

use std::cell::RefCell;

use prism_core::decoration::{Gradient, GradientKind, Shadow};
use prism_core::{Color, Corners, Style};
use windows::Foundation::Point;
use windows_core::{Interface, Result};
use windows_numerics::{Vector2, Vector3};

use crate::fluent::Palette;
use crate::paint::{self, Stop};
use crate::scene::{Node, Rect};
use crate::style::corners;
use crate::winui::bindings::Microsoft::UI::Composition::{
    CompositionRoundedRectangleGeometry, CompositionSpriteShape, CompositionVisualSurface,
    Compositor, ContainerVisual, DropShadow, ShapeVisual, SpriteVisual, Visual,
};
use crate::winui::xaml::Controls::Canvas;
use crate::winui::xaml::Hosting::ElementCompositionPreview;
use crate::winui::xaml::Media::Imaging::WriteableBitmap;
use crate::winui::xaml::Media::{
    Brush, ImageBrush, LineSegment, PathFigure, PathGeometry, RectangleGeometry, Stretch,
};
use crate::winui::xaml::Shapes::Path;
use crate::winui::xaml::UIElement;
use crate::winui::{brush, win_color};

/// A node's extra box layers (`View::decor`).
#[derive(Default)]
pub struct Decor {
    inset: Option<Inset>,
    sides: Option<Canvas>,
    /// The children have a rounded (Composition) clip.
    rounded_clip: bool,
}

impl Decor {
    /// Inset shadows and per-side borders for `n` (an `r`-sized box);
    /// `under` is the element they go above (the box, else the shadows).
    pub fn apply(
        &mut self,
        outer: &Canvas,
        under: Option<UIElement>,
        n: &Node,
        pal: &Palette,
        text: Color,
    ) -> Result<()> {
        let s = &n.style;
        let r = Rect::new(0., 0., n.frame.w, n.frame.h);
        // Inset shadows, over the background.
        let wants_inset = s.shadows.iter().any(|sh| sh.inset && sh.color.a > 0.);
        if wants_inset && self.inset.is_none() {
            let i = Inset::new()?;
            insert_after(outer, under.as_ref(), &i.host.cast()?)?;
            self.inset = Some(i);
        }
        if let Some(i) = &mut self.inset {
            i.apply(s, pal, r)?;
        }
        // Sides of different colors, over the inset shadows.
        let split = split_sides(s);
        if split && self.sides.is_none() {
            let c = Canvas::new()?;
            c.SetIsHitTestVisible(false)?;
            let above = match &self.inset {
                Some(i) => Some(i.host.cast()?),
                None => under,
            };
            insert_after(outer, above.as_ref(), &c.cast()?)?;
            self.sides = Some(c);
        }
        if let Some(c) = &self.sides {
            let coll = c.Children()?;
            coll.Clear()?;
            if split {
                let widths = s.borders().map(|b| b.width.max(0.));
                let (a, b, cc, d) = corners(s, r.w, r.h);
                let outlines = paint::border_sides(r.w, r.h, widths, [a, b, cc, d]);
                for (side, pts) in s.borders().iter().zip(&outlines) {
                    if pts.len() < 3 {
                        continue;
                    }
                    let color = pal.edge(side.color.unwrap_or(text));
                    coll.Append(&polygon(pts, color)?)?;
                }
            }
        }
        Ok(())
    }
}

/// Whether the border's sides differ in color (one `Path` each): the
/// `Border` then draws none.
pub fn split_sides(s: &Style) -> bool {
    if s.border_none || s.border_style != prism_core::decoration::BorderStyle::Solid {
        return false;
    }
    let sides = s.borders();
    let mut drawn = sides.iter().filter(|b| b.width > 0.);
    let Some(first) = drawn.next() else {
        return false;
    };
    drawn.any(|b| b.color != first.color)
}

/// Puts `e` in `outer` just above `under` (at the bottom without one).
fn insert_after(outer: &Canvas, under: Option<&UIElement>, e: &UIElement) -> Result<()> {
    let coll = outer.Children()?;
    let mut at = 0u32;
    let found = match under {
        Some(u) => coll.IndexOf(u, &mut at)?,
        None => false,
    };
    coll.InsertAt(if found { at + 1 } else { 0 }, e)
}

fn polygon(pts: &[(f32, f32)], color: Color) -> Result<Path> {
    let pt = |(x, y): (f32, f32)| Point { X: x, Y: y };
    let f = PathFigure::new()?;
    f.SetStartPoint(pt(pts[0]))?;
    f.SetIsClosed(true)?;
    f.SetIsFilled(true)?;
    let segs = f.Segments()?;
    for p in &pts[1..] {
        let l = LineSegment::new()?;
        l.SetPoint(pt(*p))?;
        segs.Append(&l)?;
    }
    let g = PathGeometry::new()?;
    g.Figures()?.Append(&f)?;
    let path = Path::new()?;
    path.SetData(&g)?;
    path.SetFill(&brush(color)?)?;
    Ok(path)
}

// --- Inset shadows -----------------------------------------------------------

/// The inset shadows of a box: a container clipped to its inside, holding
/// per shadow an invisible sprite whose `DropShadow` is cast by a frame
/// around the shadow's hole.
struct Inset {
    host: Canvas,
    container: Option<ContainerVisual>,
    clip: Option<CompositionRoundedRectangleGeometry>,
    layers: Vec<InsetLayer>,
}

struct InsetLayer {
    sprite: SpriteVisual,
    shadow: DropShadow,
    frame: CompositionRoundedRectangleGeometry,
    shape: CompositionSpriteShape,
    visual: ShapeVisual,
    surface: CompositionVisualSurface,
}

impl Inset {
    fn new() -> Result<Inset> {
        let host = Canvas::new()?;
        host.SetIsHitTestVisible(false)?;
        Ok(Inset {
            host,
            container: None,
            clip: None,
            layers: Vec::new(),
        })
    }

    fn apply(&mut self, s: &Style, pal: &Palette, r: Rect) -> Result<()> {
        let shadows: Vec<&Shadow> = s
            .shadows
            .iter()
            .filter(|sh| sh.inset && sh.color.a > 0.)
            .collect();
        let host: UIElement = self.host.cast()?;
        if shadows.is_empty() {
            if self.container.take().is_some() {
                ElementCompositionPreview::SetElementChildVisual(&host, None::<&Visual>)?;
                self.layers.clear();
                self.clip = None;
            }
            return Ok(());
        }
        let c: Compositor = ElementCompositionPreview::GetElementVisual(&host)?.Compositor()?;
        if self.container.is_none() || self.layers.len() != shadows.len() {
            let container = c.CreateContainerVisual()?;
            let clip = c.CreateRoundedRectangleGeometry()?;
            container.SetClip(&c.CreateGeometricClipWithGeometry(&clip)?)?;
            self.layers.clear();
            for _ in &shadows {
                let frame = c.CreateRoundedRectangleGeometry()?;
                let shape: CompositionSpriteShape = c.CreateSpriteShapeWithGeometry(&frame)?;
                shape.SetStrokeBrush(&c.CreateColorBrushWithColor(win_color(Color::BLACK))?)?;
                let visual = c.CreateShapeVisual()?;
                visual.Shapes()?.Append(&shape)?;
                let surface = c.CreateVisualSurface()?;
                surface.SetSourceVisual(&visual)?;
                let shadow: DropShadow = c.CreateDropShadow()?;
                shadow.SetMask(&c.CreateSurfaceBrushWithSurface(&surface)?)?;
                let sprite = c.CreateSpriteVisual()?;
                sprite.SetShadow(&shadow)?;
                // The first shadow is on top (CSS).
                container.Children()?.InsertAtBottom(&sprite)?;
                self.layers.push(InsetLayer {
                    sprite,
                    shadow,
                    frame,
                    shape,
                    visual,
                    surface,
                });
            }
            ElementCompositionPreview::SetElementChildVisual(&host, &container)?;
            self.container = Some(container);
            self.clip = Some(clip);
        }
        // The padding box: inside the border.
        let b = if s.border_none {
            [0.; 4]
        } else {
            s.borders().map(|b| b.width.max(0.))
        };
        let (x0, y0) = (b[3], b[0]);
        let (pw, ph) = ((r.w - b[1] - b[3]).max(0.), (r.h - b[0] - b[2]).max(0.));
        let radius = (corners(s, r.w, r.h).0 - b[0].max(b[3])).max(0.);
        if let Some(container) = &self.container {
            container.SetOffset(Vector3 {
                X: x0,
                Y: y0,
                Z: 0.,
            })?;
            container.SetSize(Vector2 { X: pw, Y: ph })?;
        }
        if let Some(clip) = &self.clip {
            clip.SetSize(Vector2 { X: pw, Y: ph })?;
            clip.SetCornerRadius(Vector2 {
                X: radius,
                Y: radius,
            })?;
        }
        for (l, sh) in self.layers.iter().zip(&shadows) {
            // The frame is thick enough to cover the inside wherever the
            // hole is not, blur included.
            let thick = sh.x.abs() + sh.y.abs() + sh.spread.abs() + 2. * sh.blur + 4.;
            let margin = thick + sh.x.abs() + sh.y.abs() + 2.;
            let size = Vector2 {
                X: pw + 2. * margin,
                Y: ph + 2. * margin,
            };
            let (hw, hh) = ((pw - 2. * sh.spread).max(0.), (ph - 2. * sh.spread).max(0.));
            let hole_r = (radius - sh.spread).max(0.);
            // The stroke runs outward from the hole's edge.
            l.frame.SetOffset(Vector2 {
                X: margin + sh.spread + sh.x - thick / 2.,
                Y: margin + sh.spread + sh.y - thick / 2.,
            })?;
            l.frame.SetSize(Vector2 {
                X: hw + thick,
                Y: hh + thick,
            })?;
            l.frame.SetCornerRadius(Vector2 {
                X: hole_r + thick / 2.,
                Y: hole_r + thick / 2.,
            })?;
            l.shape.SetStrokeThickness(thick)?;
            l.visual.SetSize(size)?;
            l.surface.SetSourceSize(size)?;
            l.sprite.SetSize(size)?;
            l.sprite.SetOffset(Vector3 {
                X: -margin,
                Y: -margin,
                Z: 0.,
            })?;
            l.shadow.SetBlurRadius(sh.blur)?;
            l.shadow.SetColor(win_color(pal.shadow(sh.color)))?;
        }
        Ok(())
    }
}

// --- Conic gradients ---------------------------------------------------------

thread_local! {
    /// Conic gradients drawn lately, by colors, angle and size.
    static CONICS: RefCell<Vec<(String, ImageBrush)>> = const { RefCell::new(Vec::new()) };
}

/// A CSS conic gradient for a `w` × `h` box: its pixels (twice the box's
/// size, for high DPI) in a bitmap, stretched over the box.
pub fn conic(g: &Gradient, pal: &Palette, w: f32, h: f32) -> Result<Brush> {
    let GradientKind::Conic { from } = g.kind else {
        return Err(windows_core::Error::empty());
    };
    let (pw, ph) = (
        ((w * 2.).ceil() as u32).clamp(1, 1024),
        ((h * 2.).ceil() as u32).clamp(1, 1024),
    );
    let stops: Vec<Stop> = g
        .stops
        .iter()
        .map(|s| {
            let c = pal.fill(s.color);
            Stop {
                at: s.at.clamp(0., 1.),
                rgba: [c.r, c.g, c.b, c.a],
            }
        })
        .collect();
    let key = format!("{from} {pw}x{ph} {stops:?}");
    if let Some(b) = CONICS.with(|c| {
        c.borrow()
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, b)| b.clone())
    }) {
        return b.cast();
    }
    let px = paint::conic(pw, ph, from, &stops);
    let bitmap = WriteableBitmap::CreateInstanceWithDimensions(pw as i32, ph as i32)?;
    let buffer = bitmap.PixelBuffer()?;
    unsafe {
        let access: windows::Win32::System::WinRT::IBufferByteAccess = buffer.cast()?;
        let len = (buffer.Capacity()? as usize).min(px.len());
        std::ptr::copy_nonoverlapping(px.as_ptr(), access.Buffer()?, len);
    }
    buffer.SetLength(px.len() as u32)?;
    bitmap.Invalidate()?;
    let b = ImageBrush::new()?;
    b.SetImageSource(&bitmap)?;
    b.SetStretch(Stretch::Fill)?;
    CONICS.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= 32 {
            c.remove(0);
        }
        c.push((key, b.clone()));
    });
    b.cast()
}

// --- Rounded clipping --------------------------------------------------------

/// Clips a node's children to its box (`overflow: hidden`, stacks, tabs):
/// a rectangle (XAML's clip), or the box's rounded corners (a Composition
/// clip on the children's canvas).
impl Decor {
    pub fn clip_children(&mut self, kids: &Canvas, n: &Node, on: bool) -> Result<()> {
        let rounded = clip(kids, n, on, self.rounded_clip)?;
        self.rounded_clip = rounded;
        Ok(())
    }
}

/// Returns whether the clip is rounded (a Composition clip, set on the
/// canvas's visual only then: a handed out visual costs).
fn clip(kids: &Canvas, n: &Node, on: bool, was_rounded: bool) -> Result<bool> {
    let (w, h) = (n.frame.w.max(0.), n.frame.h.max(0.));
    let s = &n.style;
    let r = s.corner_radius.min(w / 2.).min(h / 2.).max(0.);
    let rounded = on && r > 0. && s.rounded_corners != Corners::NONE;
    let kids_e: UIElement = kids.cast()?;
    if !rounded {
        if was_rounded {
            ElementCompositionPreview::GetElementVisual(&kids_e)?.SetClip(
                None::<&crate::winui::bindings::Microsoft::UI::Composition::CompositionClip>,
            )?;
        }
        if on {
            let g = RectangleGeometry::new()?;
            g.SetRect(windows::Foundation::Rect {
                X: 0.,
                Y: 0.,
                Width: w,
                Height: h,
            })?;
            kids.SetClip(&g)?;
        } else if kids.Clip().is_ok() {
            kids.SetClip(None::<&RectangleGeometry>)?;
        }
        return Ok(false);
    }
    if kids.Clip().is_ok() {
        kids.SetClip(None::<&RectangleGeometry>)?;
    }
    let visual = ElementCompositionPreview::GetElementVisual(&kids_e)?;
    let comp = visual.Compositor()?;
    // One radius for the rounded corners: square ones are pushed out of
    // the box, exact when they are the corners of one side or all but one
    // (top, bottom, left and right each tell whether a rounded corner
    // touches them).
    let c = s.rounded_corners;
    let (top, bottom) = (
        c.contains(Corners::TOP_LEADING) || c.contains(Corners::TOP_TRAILING),
        c.contains(Corners::BOTTOM_LEADING) || c.contains(Corners::BOTTOM_TRAILING),
    );
    let (left, right) = (
        c.contains(Corners::TOP_LEADING) || c.contains(Corners::BOTTOM_LEADING),
        c.contains(Corners::TOP_TRAILING) || c.contains(Corners::BOTTOM_TRAILING),
    );
    let implied = [
        (top && left, Corners::TOP_LEADING),
        (top && right, Corners::TOP_TRAILING),
        (bottom && right, Corners::BOTTOM_TRAILING),
        (bottom && left, Corners::BOTTOM_LEADING),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .fold(Corners::NONE, |acc, (_, k)| acc | *k);
    if implied != c {
        // Opposite corners, or three: a path (Direct2D) with each corner
        // its own.
        match crate::clip::rounded_path(&comp, corners(s, w, h), w, h) {
            Ok(g) => {
                visual.SetClip(&comp.CreateGeometricClipWithGeometry(&g)?)?;
                return Ok(true);
            }
            Err(e) => crate::win::log!("rounded clip: {e}"),
        }
    }
    let x0 = if left { 0. } else { -r };
    let y0 = if top { 0. } else { -r };
    let x1 = if right { w } else { w + r };
    let y1 = if bottom { h } else { h + r };
    let g = comp.CreateRoundedRectangleGeometry()?;
    g.SetOffset(Vector2 { X: x0, Y: y0 })?;
    g.SetSize(Vector2 {
        X: x1 - x0,
        Y: y1 - y0,
    })?;
    g.SetCornerRadius(Vector2 { X: r, Y: r })?;
    visual.SetClip(&comp.CreateGeometricClipWithGeometry(&g)?)?;
    Ok(true)
}
