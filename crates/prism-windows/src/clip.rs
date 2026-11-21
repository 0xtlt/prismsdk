//! Clips whose corners differ (`rounded_corners` with opposite corners, or
//! three): a Composition path geometry. Composition takes a path as an
//! `IGeometrySource2D` that hands out a Direct2D geometry
//! (`IGeometrySource2DInterop`, what Win2D's `CanvasGeometry` does); this
//! one draws the rounded rectangle again with whichever Direct2D factory
//! asks.

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

use crate::winui::bindings::Microsoft::UI::Composition::{
    CompositionGeometry, CompositionPath, Compositor,
};

thread_local! {
    static FACTORY: std::cell::OnceCell<Option<ID2D1Factory>> = const { std::cell::OnceCell::new() };
}

/// A `w`×`h` rectangle whose corners (top left, top right, bottom right,
/// bottom left) have these radii, as a Composition geometry.
pub fn rounded_path(
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
