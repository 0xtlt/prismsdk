//! Rounded clips as Composition geometric clips: a node's children
//! (`overflow: hidden`, `decor.rs`) and a picture cropped to its rounded
//! frame, as macOS and GTK crop theirs.
//!
//! One radius for the corners of one side or all but one: a rounded
//! rectangle, the square corners pushed out of the box. Corners that
//! differ otherwise (opposite ones, or three): a path geometry. Composition
//! takes a path as an `IGeometrySource2D` that hands out a Direct2D
//! geometry (`IGeometrySource2DInterop`, what Win2D's `CanvasGeometry`
//! does); this one draws the rounded rectangle again with whichever
//! Direct2D factory asks.

use prism_core::{Corners, Style};
use windows::Graphics::{IGeometrySource2D, IGeometrySource2D_Impl};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_SIZE_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_SMALL, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_SWEEP_DIRECTION_CLOCKWISE, D2D1CreateFactory, ID2D1Factory, ID2D1Geometry,
};
use windows::Win32::System::WinRT::Graphics::Direct2D::{
    IGeometrySource2DInterop, IGeometrySource2DInterop_Impl,
};
use windows_core::{Interface, Ref, Result, implement};
use windows_numerics::Vector2;

use crate::scene::Node;
use crate::style::corners;
use crate::winui::bindings::Microsoft::UI::Composition::{
    CompositionClip, CompositionGeometricClip, CompositionGeometry, CompositionPath, Compositor,
};
use crate::winui::xaml::Hosting::ElementCompositionPreview;
use crate::winui::xaml::UIElement;

thread_local! {
    static FACTORY: std::cell::OnceCell<Option<ID2D1Factory>> = const { std::cell::OnceCell::new() };
}

/// A `w`×`h` box clipped to `s`'s rounded corners; `None` when it has
/// none (a radius of 0, no corner rounded).
pub fn rounded(
    comp: &Compositor,
    s: &Style,
    w: f32,
    h: f32,
) -> Result<Option<CompositionGeometricClip>> {
    let r = s.corner_radius.min(w / 2.).min(h / 2.).max(0.);
    if r <= 0. || s.rounded_corners == Corners::NONE {
        return Ok(None);
    }
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
        match rounded_path(comp, corners(s, w, h), w, h) {
            Ok(g) => return comp.CreateGeometricClipWithGeometry(&g).map(Some),
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
    comp.CreateGeometricClipWithGeometry(&g).map(Some)
}

/// A picture cropped to its node's rounded frame (macOS crops its layer,
/// GTK its widget): a clip on the `Image`'s own visual, which sits at
/// `(x, y)` in the node (inside a border or padding). `was` tells whether
/// it had one; returns whether it has one now (a visual is handed out only
/// for a rounded picture: it costs).
pub fn round_picture(img: &UIElement, n: &Node, (x, y): (f32, f32), was: bool) -> Result<bool> {
    let (w, h) = (n.frame.w.max(0.), n.frame.h.max(0.));
    let s = &n.style;
    let r = s.corner_radius.min(w / 2.).min(h / 2.).max(0.);
    if r <= 0. || s.rounded_corners == Corners::NONE {
        if was {
            ElementCompositionPreview::GetElementVisual(img)?.SetClip(None::<&CompositionClip>)?;
        }
        return Ok(false);
    }
    let visual = ElementCompositionPreview::GetElementVisual(img)?;
    let comp = visual.Compositor()?;
    let Some(clip) = rounded(&comp, s, w, h)? else {
        return Ok(false);
    };
    // The frame's corners, in the picture's coordinates.
    clip.SetOffset(Vector2 { X: -x, Y: -y })?;
    visual.SetClip(&clip)?;
    Ok(true)
}

/// A `w`×`h` rectangle whose corners (top left, top right, bottom right,
/// bottom left) have these radii, as a Composition geometry.
fn rounded_path(
    comp: &Compositor,
    radii: (f32, f32, f32, f32),
    w: f32,
    h: f32,
) -> Result<CompositionGeometry> {
    let source: IGeometrySource2D = RoundedRect { radii, w, h }.into();
    let path = CompositionPath::Create(&source)?;
    comp.CreatePathGeometryWithPath(&path)?.cast()
}

#[implement(IGeometrySource2D, IGeometrySource2DInterop)]
struct RoundedRect {
    radii: (f32, f32, f32, f32),
    w: f32,
    h: f32,
}

impl RoundedRect {
    fn draw(&self, factory: &ID2D1Factory) -> Result<ID2D1Geometry> {
        let (tl, tr, br, bl) = self.radii;
        let (w, h) = (self.w, self.h);
        let p = |x: f32, y: f32| Vector2 { X: x, Y: y };
        unsafe {
            let path = factory.CreatePathGeometry()?;
            let sink = path.Open()?;
            sink.BeginFigure(p(tl, 0.), D2D1_FIGURE_BEGIN_FILLED);
            // Clockwise from the top left corner's end; each rounded
            // corner a quarter circle.
            let arc = |to: Vector2, r: f32| D2D1_ARC_SEGMENT {
                point: to,
                size: D2D_SIZE_F {
                    width: r,
                    height: r,
                },
                rotationAngle: 0.,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: D2D1_ARC_SIZE_SMALL,
            };
            sink.AddLine(p(w - tr, 0.));
            if tr > 0. {
                sink.AddArc(&arc(p(w, tr), tr));
            }
            sink.AddLine(p(w, h - br));
            if br > 0. {
                sink.AddArc(&arc(p(w - br, h), br));
            }
            sink.AddLine(p(bl, h));
            if bl > 0. {
                sink.AddArc(&arc(p(0., h - bl), bl));
            }
            sink.AddLine(p(0., tl));
            if tl > 0. {
                sink.AddArc(&arc(p(tl, 0.), tl));
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            path.cast()
        }
    }
}

impl IGeometrySource2D_Impl for RoundedRect_Impl {}

impl IGeometrySource2DInterop_Impl for RoundedRect_Impl {
    fn GetGeometry(&self) -> Result<ID2D1Geometry> {
        let factory = FACTORY
            .with(|f| {
                f.get_or_init(|| unsafe {
                    D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()
                })
                .clone()
            })
            .ok_or_else(windows_core::Error::empty)?;
        self.draw(&factory)
    }

    fn TryGetGeometryUsingFactory(&self, factory: Ref<ID2D1Factory>) -> Result<ID2D1Geometry> {
        match factory.as_ref() {
            Some(f) => self.draw(f),
            None => self.GetGeometry(),
        }
    }
}
