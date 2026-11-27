//! A drawing that is more than flat paths (#342: an SVG document's
//! gradients, text, images, and groups with an opacity, a clip, a mask,
//! shadows or a blur) as a picture: drawn with Direct2D and DirectWrite
//! into a WIC bitmap at the window's scale, shown by an `Image` in the
//! shape's canvas. XAML has no brush for a radial gradient with a focal
//! point, clips to rectangles only, and has neither masks nor blurs; a
//! picture has them all, and the same on every Windows version. Flat
//! paths stay XAML `Path`s (`controls::set_paths`).
//!
//! Items are drawn in order into a render target whose units are points.
//! A group with an opacity, a clip or a mask is a Direct2D layer (drawn
//! apart, then composed as one picture: through the clip's geometry, the
//! mask's alpha as an opacity brush, at the opacity). A blurred group, or
//! one with shadows, is first drawn into a bitmap of its own, whose pixels
//! `effects.rs` blurs and casts shadows from. A mask is drawn the same way
//! and its brightness turned into alpha.
//!
//! Every bitmap is the view and a margin around it: what lies outside
//! the view still blurs and casts shadows into it.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::mem::ManuallyDrop;

use prism_core::Color;
use prism_core::shape::{
    Baseline, Clip, Gradient, GradientShape, Group, Image, Item, Mask, Matrix, Path, Segment,
    Spread, Text,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F,
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN,
    D2D1_FILL_MODE_ALTERNATE, D2D1_FILL_MODE_WINDING, D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_BRUSH_PROPERTIES,
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, D2D1_BRUSH_PROPERTIES,
    D2D1_CAP_STYLE_FLAT, D2D1_CAP_STYLE_ROUND, D2D1_DASH_STYLE_CUSTOM, D2D1_DASH_STYLE_SOLID,
    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_DRAW_TEXT_OPTIONS_NO_SNAP,
    D2D1_EXTEND_MODE_CLAMP, D2D1_EXTEND_MODE_MIRROR, D2D1_EXTEND_MODE_WRAP,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_GAMMA_2_2, D2D1_LAYER_OPTIONS_NONE,
    D2D1_LAYER_PARAMETERS, D2D1_LINE_JOIN_MITER, D2D1_LINE_JOIN_ROUND,
    D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_STROKE_STYLE_PROPERTIES,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1Bitmap, ID2D1Brush, ID2D1Factory,
    ID2D1Geometry, ID2D1Layer, ID2D1RenderTarget,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_METRICS, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE,
    DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_LINE_METRICS,
    DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE, DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory,
    IDWriteFactory, IDWriteFactory5, IDWriteFontCollection, IDWriteTextLayout, IDWriteTextLayout1,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    GUID_WICPixelFormat32bppPBGRA, IWICBitmap, IWICImagingFactory, WICBitmapCacheOnLoad,
};
use windows::Win32::System::WinRT::IBufferByteAccess;
use windows_core::{BOOL, HSTRING, Interface, Result};
use windows_numerics::{Matrix3x2, Vector2};

use crate::effects::{self, Paint};
use crate::fluent::Palette;
use crate::images::Images;
use crate::win::log;
use crate::winui::xaml::Controls::{Canvas, Image as XamlImage};
use crate::winui::xaml::Media::Imaging::WriteableBitmap;
use crate::winui::xaml::Media::Stretch;

/// The longest side of a picture, pixels: a larger drawing is drawn at a
/// smaller scale and stretched.
const MAX_SIDE: f32 = 4096.;
/// The widest margin kept around the view, points.
const MAX_MARGIN: f32 = 256.;

/// What a picture is drawn for and with.
pub struct Scene<'a> {
    /// The node's colors (light or dark, a contrast theme).
    pub pal: &'a Palette,
    /// Decoded bitmaps, by source.
    pub images: &'a mut Images,
    pub wic: &'a IWICImagingFactory,
    /// The window bitmaps loaded later report to.
    pub hwnd: HWND,
    /// The window's scale (pixels per point).
    pub scale: f32,
}

/// A drawing's pixels: `w` × `h` premultiplied BGRA rows, `scale` of them
/// per point.
pub struct Picture {
    pub px: Vec<u8>,
    pub w: u32,
    pub h: u32,
    pub scale: f32,
}

thread_local! {
    /// Direct2D's and DirectWrite's factories, made once.
    static FACTORIES: OnceCell<Option<(ID2D1Factory, IDWriteFactory)>> =
        const { OnceCell::new() };
    /// DirectWrite's font for each XAML family list (`text::family_name`).
    static FACES: RefCell<HashMap<String, Face>> = RefCell::new(HashMap::new());
}

fn factories() -> Result<(ID2D1Factory, IDWriteFactory)> {
    FACTORIES
        .with(|f| {
            f.get_or_init(|| unsafe {
                let d2d: ID2D1Factory =
                    D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
                let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
                Some((d2d, dw))
            })
            .clone()
        })
        .ok_or_else(windows_core::Error::empty)
}

/// Draws `items` (points, in a `w` × `h` view). `None` when there is
/// nothing to show (no size, no item).
pub fn draw(items: &[Item], (w, h): (f32, f32), cx: Scene) -> Result<Option<Picture>> {
    if !(w > 0. && h > 0.) || items.is_empty() {
        return Ok(None);
    }
    let (d2d, dw) = factories()?;
    let margin = effects::margin(items).min(MAX_MARGIN);
    let mut k = cx.scale.max(0.25);
    let longest = (w.max(h) + 2. * margin) * k;
    if longest > MAX_SIDE {
        k *= MAX_SIDE / longest;
    }
    // Whole pixels, the margin too: the view's pixels are the screen's.
    let pad = (margin * k).ceil() as u32;
    let (vw, vh) = (
        ((w * k).ceil() as u32).max(1),
        ((h * k).ceil() as u32).max(1),
    );
    let mut painter = Painter {
        d2d,
        dw,
        cx,
        k,
        pad,
        pw: vw + 2 * pad,
        ph: vh + 2 * pad,
        masks: 0,
    };
    let surface = painter.surface()?;
    painter.items(&surface.rt, items);
    let all = surface.pixels(&painter)?;
    // The view alone.
    let px = if pad == 0 {
        all
    } else {
        let stride = painter.pw as usize * 4;
        let mut px = Vec::with_capacity((vw * vh * 4) as usize);
        for row in all.chunks(stride).skip(pad as usize).take(vh as usize) {
            px.extend_from_slice(&row[pad as usize * 4..(pad + vw) as usize * 4]);
        }
        px
    };
    Ok(Some(Picture {
        px,
        w: vw,
        h: vh,
        scale: k,
    }))
}

/// Shows `picture` in a shape's canvas, in place of what it held: an
/// `Image` whose pixels are the screen's.
pub fn show(canvas: &Canvas, picture: Option<&Picture>) -> Result<()> {
    let children = canvas.Children()?;
    children.Clear()?;
    let Some(p) = picture else {
        return Ok(());
    };
    let bitmap = WriteableBitmap::CreateInstanceWithDimensions(p.w as i32, p.h as i32)?;
    let buffer = bitmap.PixelBuffer()?;
    let len = (buffer.Capacity()? as usize).min(p.px.len());
    unsafe {
        let access: IBufferByteAccess = buffer.cast()?;
        std::ptr::copy_nonoverlapping(p.px.as_ptr(), access.Buffer()?, len);
    }
    buffer.SetLength(len as u32)?;
    bitmap.Invalidate()?;
    let image = XamlImage::new()?;
    image.SetIsHitTestVisible(false)?;
    image.SetStretch(Stretch::Fill)?;
    image.SetWidth((p.w as f32 / p.scale) as f64)?;
    image.SetHeight((p.h as f32 / p.scale) as f64)?;
    image.SetSource(&bitmap)?;
    children.Append(&image)
}

/// The image sources `items` draw (a shape is drawn again when one that
/// was loading is there).
pub fn sources<'a>(items: &'a [Item], out: &mut Vec<&'a str>) {
    for i in items {
        match i {
            Item::Image(i) => out.push(&i.src),
            Item::Group(g) => {
                sources(&g.items, out);
                if let Some(m) = &g.mask {
                    sources(&m.items, out);
                }
            }
            _ => {}
        }
    }
}

struct Painter<'a> {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    cx: Scene<'a>,
    /// Pixels per point.
    k: f32,
    /// The margin around the view, pixels.
    pad: u32,
    /// Every bitmap's size, pixels: the view and its margin.
    pw: u32,
    ph: u32,
    /// How many masks deep the items being drawn are: a mask's colors
    /// tell how much shows, whatever the theme.
    masks: u32,
}

/// A bitmap being drawn to.
struct Surface {
    bitmap: IWICBitmap,
    rt: ID2D1RenderTarget,
}

impl Surface {
    /// What was drawn: premultiplied BGRA rows.
    fn pixels(self, p: &Painter) -> Result<Vec<u8>> {
        let mut px = vec![0; (p.pw * p.ph * 4) as usize];
        unsafe {
            self.rt.EndDraw(None, None)?;
            self.bitmap
                .CopyPixels(std::ptr::null(), p.pw * 4, &mut px)?;
        }
        Ok(px)
    }
}

impl Painter<'_> {
    /// A transparent bitmap to draw to, in points from the view's corner.
    fn surface(&self) -> Result<Surface> {
        unsafe {
            let bitmap = self.cx.wic.CreateBitmap(
                self.pw,
                self.ph,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapCacheOnLoad,
            )?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: self.format(),
                dpiX: 96. * self.k,
                dpiY: 96. * self.k,
                ..Default::default()
            };
            let rt = self.d2d.CreateWicBitmapRenderTarget(&bitmap, &props)?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            rt.BeginDraw();
            rt.Clear(Some(&D2D1_COLOR_F::default()));
            rt.SetTransform(&self.base());
            Ok(Surface { bitmap, rt })
        }
    }

    fn format(&self) -> D2D1_PIXEL_FORMAT {
        D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        }
    }

    /// The margin, points.
    fn margin(&self) -> f32 {
        self.pad as f32 / self.k
    }

    /// From the view's points to a bitmap's: past the margin.
    fn base(&self) -> Matrix3x2 {
        Matrix3x2::translation(self.margin(), self.margin())
    }

    /// A color of the drawing for the theme; a mask's as it is.
    fn color(&self, c: Color) -> Color {
        if self.masks > 0 {
            self.cx.pal.resolve(c)
        } else {
            self.cx.pal.graphic(c)
        }
    }

    /// Draws `items` in order. One that cannot be drawn is left out.
    fn items(&mut self, rt: &ID2D1RenderTarget, items: &[Item]) {
        for i in items {
            let drawn = match i {
                Item::Path(p) => self.path(rt, p),
                Item::Text(t) => self.text(rt, t),
                Item::Image(i) => self.image(rt, i),
                Item::Group(g) => self.group(rt, g),
            };
            if let Err(e) = drawn {
                log!("drawing: {e}");
            }
            // The next one starts from the view's own coordinates.
            unsafe { rt.SetTransform(&self.base()) };
        }
    }

    fn path(&self, rt: &ID2D1RenderTarget, p: &Path) -> Result<()> {
        let fill = self.brush(rt, p.fill, p.fill_gradient.as_ref())?;
        let stroke = match p.width > 0. {
            true => self.brush(rt, p.stroke, p.stroke_gradient.as_ref())?,
            false => None,
        };
        if fill.is_none() && stroke.is_none() {
            return Ok(());
        }
        let geometry = self.geometry(&p.segments, p.even_odd)?;
        unsafe {
            if let Some(b) = &fill {
                rt.FillGeometry(&geometry, b, None);
            }
            if let Some(b) = &stroke {
                let (cap, join) = if p.round {
                    (D2D1_CAP_STYLE_ROUND, D2D1_LINE_JOIN_ROUND)
                } else {
                    (D2D1_CAP_STYLE_FLAT, D2D1_LINE_JOIN_MITER)
                };
                // Dashes are counted in stroke widths.
                let dashes = p.dash.map(|(d, g)| [d / p.width, g / p.width]);
                let props = D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: cap,
                    endCap: cap,
                    dashCap: cap,
                    lineJoin: join,
                    miterLimit: 10.,
                    dashStyle: if dashes.is_some() {
                        D2D1_DASH_STYLE_CUSTOM
                    } else {
                        D2D1_DASH_STYLE_SOLID
                    },
                    dashOffset: 0.,
                };
                let style = self
                    .d2d
                    .CreateStrokeStyle(&props, dashes.as_ref().map(|d| &d[..]))?;
                rt.DrawGeometry(&geometry, b, p.width, &style);
            }
        }
        Ok(())
    }

    /// The brush of a fill or a stroke: its gradient, else its color.
    /// `None` when it paints nothing.
    fn brush(
        &self,
        rt: &ID2D1RenderTarget,
        flat: Option<Color>,
        gradient: Option<&Gradient>,
    ) -> Result<Option<ID2D1Brush>> {
        let solid = |c: Color| -> Result<Option<ID2D1Brush>> {
            let c = self.color(c);
            if c.a <= 0. {
                return Ok(None);
            }
            let b = unsafe { rt.CreateSolidColorBrush(&d2d_color(c), None)? };
            Ok(Some(b.into()))
        };
        let Some(g) = gradient else {
            return flat.map_or(Ok(None), solid);
        };
        match effects::paint(g) {
            Paint::None => Ok(None),
            Paint::Flat(c) => solid(c),
            Paint::Gradient => {
                // In order, each no sooner than the one before.
                let mut at = 0f32;
                let stops: Vec<D2D1_GRADIENT_STOP> = g
                    .stops
                    .iter()
                    .map(|s| {
                        at = s.offset.clamp(at, 1.);
                        D2D1_GRADIENT_STOP {
                            position: at,
                            color: d2d_color(self.color(s.color)),
                        }
                    })
                    .collect();
                let extend = match g.spread {
                    Spread::Pad => D2D1_EXTEND_MODE_CLAMP,
                    Spread::Reflect => D2D1_EXTEND_MODE_MIRROR,
                    Spread::Repeat => D2D1_EXTEND_MODE_WRAP,
                };
                // The gradient's own matrix: a skewed one stays skewed.
                let props = D2D1_BRUSH_PROPERTIES {
                    opacity: 1.,
                    transform: matrix(g.transform),
                };
                let at = |x: f32, y: f32| Vector2 { X: x, Y: y };
                unsafe {
                    // Colors are mixed as they are written (sRGB), their
                    // alpha apart, like SVG.
                    let stops = rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, extend)?;
                    Ok(Some(match g.shape {
                        GradientShape::Linear { x1, y1, x2, y2 } => rt
                            .CreateLinearGradientBrush(
                                &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                                    startPoint: at(x1, y1),
                                    endPoint: at(x2, y2),
                                },
                                Some(&props),
                                &stops,
                            )?
                            .into(),
                        GradientShape::Radial { cx, cy, r, fx, fy } => {
                            let (ox, oy) = effects::focal_offset(cx, cy, r, fx, fy);
                            rt.CreateRadialGradientBrush(
                                &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                                    center: at(cx, cy),
                                    gradientOriginOffset: at(ox, oy),
                                    radiusX: r,
                                    radiusY: r,
                                },
                                Some(&props),
                                &stops,
                            )?
                            .into()
                        }
                    }))
                }
            }
        }
    }

    /// An outline (a path's, a clip's) as a Direct2D geometry.
    fn geometry(&self, segments: &[Segment], even_odd: bool) -> Result<ID2D1Geometry> {
        let at = |x: f32, y: f32| Vector2 { X: x, Y: y };
        unsafe {
            let geometry = self.d2d.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.SetFillMode(if even_odd {
                D2D1_FILL_MODE_ALTERNATE
            } else {
                D2D1_FILL_MODE_WINDING
            });
            let mut open = false;
            for s in segments {
                match *s {
                    Segment::Move(x, y) => {
                        if open {
                            sink.EndFigure(D2D1_FIGURE_END_OPEN);
                        }
                        sink.BeginFigure(at(x, y), D2D1_FIGURE_BEGIN_FILLED);
                        open = true;
                    }
                    Segment::Line(x, y) if open => sink.AddLine(at(x, y)),
                    Segment::Cubic(a, b, c, d, e, f) if open => {
                        sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                            point1: at(a, b),
                            point2: at(c, d),
                            point3: at(e, f),
                        })
                    }
                    Segment::Close if open => {
                        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                        open = false;
                    }
                    // Arcs come as curves already (`set_drawing`).
                    _ => {}
                }
            }
            if open {
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
            }
            sink.Close()?;
            Ok(geometry.into())
        }
    }

    /// One line of text: its runs one after the other from its anchor,
    /// each on its baseline.
    fn text(&self, rt: &ID2D1RenderTarget, t: &Text) -> Result<()> {
        // How much its matrix enlarges it: the system font has one design
        // for text and one for titles.
        let m = t.transform;
        let grow = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
        let mut laid = Vec::with_capacity(t.runs.len());
        for r in &t.runs {
            let list = crate::text::family_name(&r.family, r.size * grow, r.weight, r.italic);
            let face = face(&self.dw, &list);
            laid.push(match r.size > 0. && !r.text.is_empty() {
                true => Some(Laid::new(&self.dw, &face, r)?),
                false => None,
            });
        }
        let width: f32 = t
            .runs
            .iter()
            .zip(&laid)
            .map(|(r, l)| r.dx + l.as_ref().map_or(0., |l| l.width))
            .sum();
        // Which line of the first run's font sits on `y`.
        let drop = match (t.baseline, t.runs.first()) {
            (Baseline::Alphabetic, _) | (_, None) => 0.,
            (baseline, Some(r)) => {
                let list = crate::text::family_name(&r.family, r.size * grow, r.weight, r.italic);
                let (ascent, descent, x_height) =
                    font_metrics(&self.dw, &face(&self.dw, &list), r.weight, r.italic, r.size);
                effects::baseline_drop(baseline, ascent, descent, x_height)
            }
        };
        let mut x = effects::line_start(t.anchor, t.x, width);
        unsafe {
            rt.SetTransform(&(matrix(t.transform) * self.base()));
            for (r, l) in t.runs.iter().zip(&laid) {
                x += r.dx;
                let Some(l) = l else { continue };
                let c = self.color(r.fill);
                // Without ink it only takes its room.
                if c.a > 0. {
                    let brush = rt.CreateSolidColorBrush(&d2d_color(c), None)?;
                    rt.DrawTextLayout(
                        Vector2 {
                            X: x,
                            Y: t.y + drop + r.dy - l.baseline,
                        },
                        &l.layout,
                        &brush,
                        // Where it is told, to the fraction of a pixel.
                        D2D1_DRAW_TEXT_OPTIONS_NO_SNAP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                    );
                }
                x += l.width;
            }
        }
        Ok(())
    }

    /// A bitmap in its rectangle: stretched, whole, or covering it.
    fn image(&mut self, rt: &ID2D1RenderTarget, i: &Image) -> Result<()> {
        if i.opacity <= 0. {
            return Ok(());
        }
        // Decoded once per source; one that does not decode draws nothing.
        self.cx.images.request(self.cx.wic, &i.src, self.cx.hwnd);
        let (Some((_, bitmap)), Some(size)) = (
            self.cx.images.bitmap(&i.src),
            self.cx.images.size_of(&i.src),
        ) else {
            return Ok(());
        };
        let Some((dest, part)) = effects::fit(i.fit, i.align, (i.x, i.y, i.w, i.h), size) else {
            return Ok(());
        };
        let rect = |[left, top, right, bottom]: [f32; 4]| D2D_RECT_F {
            left,
            top,
            right,
            bottom,
        };
        unsafe {
            // One pixel per unit: `part` is in pixels.
            let bitmap = rt.CreateBitmapFromWicBitmap(&bitmap, Some(&self.bitmap_props(96.)))?;
            rt.SetTransform(&(matrix(i.transform) * self.base()));
            rt.DrawBitmap(
                &bitmap,
                Some(&rect(dest)),
                i.opacity.min(1.),
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                Some(&rect(part)),
            );
        }
        Ok(())
    }

    fn bitmap_props(&self, dpi: f32) -> D2D1_BITMAP_PROPERTIES {
        D2D1_BITMAP_PROPERTIES {
            pixelFormat: self.format(),
            dpiX: dpi,
            dpiY: dpi,
        }
    }

    /// Items drawn together, then shown as one picture: blurred and over
    /// its shadows, through its clip and its mask, at its opacity.
    fn group(&mut self, rt: &ID2D1RenderTarget, g: &Group) -> Result<()> {
        if g.opacity <= 0. {
            return Ok(());
        }
        let clip = match &g.clip {
            Some(Clip { segments, even_odd }) => Some(self.geometry(segments, *even_odd)?),
            None => None,
        };
        let mask = match &g.mask {
            Some(m) => Some(self.mask(rt, m)?),
            None => None,
        };
        let layer = clip.is_some() || mask.is_some() || g.opacity < 1.;
        if layer {
            let mut params = D2D1_LAYER_PARAMETERS {
                // Everything: the clip and the mask say what shows.
                contentBounds: D2D_RECT_F {
                    left: -f32::MAX,
                    top: -f32::MAX,
                    right: f32::MAX,
                    bottom: f32::MAX,
                },
                geometricMask: ManuallyDrop::new(clip),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2::identity(),
                opacity: g.opacity.min(1.),
                opacityBrush: ManuallyDrop::new(mask),
                layerOptions: D2D1_LAYER_OPTIONS_NONE,
            };
            unsafe {
                rt.PushLayer(&params, None::<&ID2D1Layer>);
                ManuallyDrop::drop(&mut params.geometricMask);
                ManuallyDrop::drop(&mut params.opacityBrush);
            }
        }
        let drawn = if g.blur > 0. || !g.shadows.is_empty() {
            self.filtered(rt, g)
        } else {
            self.items(rt, &g.items);
            Ok(())
        };
        if layer {
            unsafe { rt.PopLayer() };
        }
        drawn
    }

    /// A group with a blur or shadows: its items in a bitmap of their own,
    /// blurred; under it each shadow (the bitmap's alpha, blurred, in the
    /// shadow's color), the first one at the bottom.
    fn filtered(&mut self, rt: &ID2D1RenderTarget, g: &Group) -> Result<()> {
        let surface = self.surface()?;
        self.items(&surface.rt, &g.items);
        let mut px = surface.pixels(self)?;
        effects::gaussian(&mut px, self.pw, self.ph, g.blur * self.k);
        for s in &g.shadows {
            // A contrast theme has no shadows.
            let c = if self.masks > 0 {
                self.cx.pal.resolve(s.color)
            } else {
                self.cx.pal.shadow(s.color)
            };
            if c.a <= 0. {
                continue;
            }
            let cast =
                effects::shadow(&px, self.pw, self.ph, s.blur * self.k, [c.r, c.g, c.b, c.a]);
            self.blit(rt, &cast, s.dx, s.dy)?;
        }
        self.blit(rt, &px, 0., 0.)
    }

    /// Draws a bitmap's pixels over `rt`'s, moved by `(dx, dy)` points.
    fn blit(&self, rt: &ID2D1RenderTarget, px: &[u8], dx: f32, dy: f32) -> Result<()> {
        let bitmap = self.bitmap(rt, px)?;
        let (left, top) = (dx - self.margin(), dy - self.margin());
        unsafe {
            rt.SetTransform(&self.base());
            rt.DrawBitmap(
                &bitmap,
                Some(&D2D_RECT_F {
                    left,
                    top,
                    right: left + self.pw as f32 / self.k,
                    bottom: top + self.ph as f32 / self.k,
                }),
                1.,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
        }
        Ok(())
    }

    /// Pixels of a bitmap of ours, for `rt`: as many per point as it has.
    fn bitmap(&self, rt: &ID2D1RenderTarget, px: &[u8]) -> Result<ID2D1Bitmap> {
        unsafe {
            rt.CreateBitmap(
                D2D_SIZE_U {
                    width: self.pw,
                    height: self.ph,
                },
                Some(px.as_ptr().cast()),
                self.pw * 4,
                &self.bitmap_props(96. * self.k),
            )
        }
    }

    /// A mask as the brush of a layer's opacity: its items drawn in a
    /// bitmap of their own, whose alpha (its brightness times its alpha
    /// for a luminance mask) is how much of the group shows.
    fn mask(&mut self, rt: &ID2D1RenderTarget, m: &Mask) -> Result<ID2D1Brush> {
        let surface = self.surface()?;
        self.masks += 1;
        self.items(&surface.rt, &m.items);
        self.masks -= 1;
        let mut px = surface.pixels(self)?;
        if m.luminance {
            effects::luminance_to_alpha(&mut px);
        }
        let bitmap = self.bitmap(rt, &px)?;
        let brush = unsafe {
            rt.CreateBitmapBrush(
                &bitmap,
                Some(&D2D1_BITMAP_BRUSH_PROPERTIES {
                    extendModeX: D2D1_EXTEND_MODE_CLAMP,
                    extendModeY: D2D1_EXTEND_MODE_CLAMP,
                    interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                }),
                // The bitmap starts a margin before the view.
                Some(&D2D1_BRUSH_PROPERTIES {
                    opacity: 1.,
                    transform: Matrix3x2::translation(-self.margin(), -self.margin()),
                }),
            )?
        };
        Ok(brush.into())
    }
}

/// Prism's matrix (`x' = a·x + c·y + e`, `y' = b·x + d·y + f`) as
/// Direct2D's (row vectors: the same six numbers, row by row).
fn matrix(m: Matrix) -> Matrix3x2 {
    Matrix3x2 {
        M11: m[0],
        M12: m[1],
        M21: m[2],
        M22: m[3],
        M31: m[4],
        M32: m[5],
    }
}

fn d2d_color(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

// --- Text ----------------------------------------------------------------------

/// A font as DirectWrite names it: a family of a collection, and where it
/// is in it (`None`: the system's, as a last resort).
#[derive(Clone)]
struct Face {
    collection: Option<IDWriteFontCollection>,
    index: u32,
    family: String,
}

/// DirectWrite's font for a XAML family list (`text::family_name`): the
/// first family there is, a bundled or run-time font from its file.
fn face(dw: &IDWriteFactory, list: &str) -> Face {
    if let Some(f) = FACES.with(|f| f.borrow().get(list).cloned()) {
        return f;
    }
    let found = list
        .split(',')
        .map(str::trim)
        .find_map(|name| match name.starts_with("ms-appx:") {
            true => file_face(dw, name),
            false => system_family(dw, name).map(|(collection, index)| Face {
                collection: Some(collection),
                index,
                family: name.to_string(),
            }),
        })
        .unwrap_or_else(|| Face {
            collection: None,
            index: 0,
            family: "Segoe UI".into(),
        });
    FACES.with(|f| f.borrow_mut().insert(list.to_string(), found.clone()));
    found
}

/// The system's collection and where `family` is in it.
fn system_family(dw: &IDWriteFactory, family: &str) -> Option<(IDWriteFontCollection, u32)> {
    unsafe {
        let mut collection = None;
        dw.GetSystemFontCollection(&mut collection, false).ok()?;
        let collection = collection?;
        let (mut index, mut exists) = (0u32, BOOL(0));
        collection
            .FindFamilyName(&HSTRING::from(family), &mut index, &mut exists)
            .ok()?;
        exists.as_bool().then_some((collection, index))
    }
}

/// A bundled or run-time font (`fonts.rs`: XAML names its file): a
/// collection of that one file, and its family as DirectWrite reads it.
fn file_face(dw: &IDWriteFactory, source: &str) -> Option<Face> {
    let path = crate::fonts::file(source)?;
    unsafe {
        let dw: IDWriteFactory5 = dw.cast().ok()?;
        let builder = dw.CreateFontSetBuilder().ok()?;
        let file = dw
            .CreateFontFileReference(&HSTRING::from(path.as_os_str()), None)
            .ok()?;
        builder.AddFontFile(&file).ok()?;
        let collection = dw
            .CreateFontCollectionFromFontSet(&builder.CreateFontSet().ok()?)
            .ok()?;
        let names = collection.GetFontFamily(0).ok()?.GetFamilyNames().ok()?;
        let mut name = vec![0u16; names.GetStringLength(0).ok()? as usize + 1];
        names.GetString(0, &mut name).ok()?;
        name.pop();
        Some(Face {
            collection: Some(collection.cast().ok()?),
            index: 0,
            family: String::from_utf16_lossy(&name),
        })
    }
}

fn font_style(italic: bool) -> DWRITE_FONT_STYLE {
    if italic {
        DWRITE_FONT_STYLE_ITALIC
    } else {
        DWRITE_FONT_STYLE_NORMAL
    }
}

/// A font's ascent, descent and x-height at `size`.
fn font_metrics(
    dw: &IDWriteFactory,
    face: &Face,
    weight: u16,
    italic: bool,
    size: f32,
) -> (f32, f32, f32) {
    let read = || unsafe {
        let (collection, index) = match &face.collection {
            Some(c) => (c.clone(), face.index),
            None => system_family(dw, &face.family)?,
        };
        let font = collection
            .GetFontFamily(index)
            .ok()?
            .GetFirstMatchingFont(
                DWRITE_FONT_WEIGHT(weight as i32),
                DWRITE_FONT_STRETCH_NORMAL,
                font_style(italic),
            )
            .ok()?;
        let mut m = DWRITE_FONT_METRICS::default();
        font.GetMetrics(&mut m);
        (m.designUnitsPerEm > 0).then(|| {
            let k = size / m.designUnitsPerEm as f32;
            (
                m.ascent as f32 * k,
                m.descent as f32 * k,
                m.xHeight as f32 * k,
            )
        })
    };
    // A font that cannot be read: the usual proportions.
    read().unwrap_or((0.9 * size, 0.25 * size, 0.5 * size))
}

/// A run laid out: how wide it is and where its baseline is under its
/// top.
struct Laid {
    layout: IDWriteTextLayout,
    width: f32,
    baseline: f32,
}

impl Laid {
    fn new(dw: &IDWriteFactory, face: &Face, r: &prism_core::shape::TextRun) -> Result<Laid> {
        unsafe {
            let format = dw.CreateTextFormat(
                &HSTRING::from(face.family.as_str()),
                face.collection.as_ref(),
                DWRITE_FONT_WEIGHT(r.weight as i32),
                font_style(r.italic),
                DWRITE_FONT_STRETCH_NORMAL,
                r.size,
                &HSTRING::new(),
            )?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            let text: Vec<u16> = r.text.encode_utf16().collect();
            let layout = dw.CreateTextLayout(&text, &format, 100_000., 100_000.)?;
            if r.spacing != 0.
                && let Ok(spaced) = layout.cast::<IDWriteTextLayout1>()
            {
                // After each character.
                spaced.SetCharacterSpacing(
                    0.,
                    r.spacing,
                    0.,
                    DWRITE_TEXT_RANGE {
                        startPosition: 0,
                        length: text.len() as u32,
                    },
                )?;
            }
            let (mut metrics, mut line) = (
                DWRITE_TEXT_METRICS::default(),
                [DWRITE_LINE_METRICS::default()],
            );
            let mut lines = 0;
            layout.GetMetrics(&mut metrics)?;
            // More lines than one (a line break in the run): the first's.
            let _ = layout.GetLineMetrics(Some(&mut line), &mut lines);
            Ok(Laid {
                layout,
                width: metrics.widthIncludingTrailingWhitespace,
                baseline: line[0].baseline,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fluent::Accent;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

    /// A 2 × 1 PNG: a red pixel, then a blue one.
    const RED_BLUE: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAADklEQVR4nGP4z8AAQv8BD/kD/YURmXYAAAAASUVORK5CYII=";

    /// An SVG document drawn at its own size, `scale` pixels per point,
    /// in light colors.
    fn render(markup: &str, scale: f32) -> Picture {
        render_in(markup, scale, false)
    }

    fn render_in(markup: &str, scale: f32, dark: bool) -> Picture {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let svg = prism_core::svg::parse(markup).expect("an <svg> element");
        let items = svg.drawing.resolve_items(svg.width, svg.height, 1.);
        let wic = crate::wic::factory().expect("WIC");
        let mut images = Images::new(Default::default());
        let cx = Scene {
            pal: &Palette::new(dark, Accent::default()),
            images: &mut images,
            wic: &wic,
            hwnd: HWND::default(),
            scale,
        };
        draw(&items, (svg.width, svg.height), cx)
            .expect("drawn")
            .expect("a picture")
    }

    /// The pixel at `(x, y)` points: straight-alpha `[r, g, b, a]`.
    fn at(p: &Picture, x: f32, y: f32) -> [u8; 4] {
        let (x, y) = ((x * p.scale) as usize, (y * p.scale) as usize);
        let i = (y * p.w as usize + x) * 4;
        let [b, g, r, a] = [p.px[i], p.px[i + 1], p.px[i + 2], p.px[i + 3]];
        let un = |c: u8| match a {
            0 => 0,
            a => ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8,
        };
        [un(r), un(g), un(b), a]
    }

    fn near(got: [u8; 4], want: [u8; 4], by: u8) -> bool {
        got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= by)
    }

    #[track_caller]
    fn assert_near(got: [u8; 4], want: [u8; 4], by: u8) {
        assert!(near(got, want, by), "{got:?} is not {want:?} (± {by})");
    }

    /// The columns of row `y` (points) that hold ink, points.
    fn inked_columns(p: &Picture, y: f32) -> Option<(f32, f32)> {
        let row = (y * p.scale) as usize * p.w as usize * 4;
        let alpha = |x: usize| p.px[row + x * 4 + 3];
        let first = (0..p.w as usize).find(|x| alpha(*x) > 64)?;
        let last = (0..p.w as usize).rev().find(|x| alpha(*x) > 64)?;
        Some((first as f32 / p.scale, (last + 1) as f32 / p.scale))
    }

    /// The rows that hold ink, points.
    fn inked_rows(p: &Picture) -> Option<(f32, f32)> {
        let w = p.w as usize;
        let ink = |y: usize| (0..w).any(|x| p.px[(y * w + x) * 4 + 3] > 64);
        let first = (0..p.h as usize).find(|y| ink(*y))?;
        let last = (0..p.h as usize).rev().find(|y| ink(*y))?;
        Some((first as f32 / p.scale, (last + 1) as f32 / p.scale))
    }

    #[test]
    fn a_linear_gradient_goes_from_its_first_stop_to_its_last() {
        let p = render(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="10">
                <linearGradient id="g"><stop offset="0" stop-color="#f00"/>
                <stop offset="1" stop-color="#00f"/></linearGradient>
                <rect width="100" height="10" fill="url(#g)"/></svg>"##,
            1.,
        );
        assert_near(at(&p, 0.5, 5.), [255, 0, 0, 255], 6);
        assert_near(at(&p, 99.5, 5.), [0, 0, 255, 255], 6);
        // Mixed as written (sRGB): half red, half blue.
        assert_near(at(&p, 50., 5.), [128, 0, 128, 255], 8);
    }

    #[test]
    fn a_reflected_gradient_comes_back() {
        let p = render(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="10">
                <linearGradient id="g" gradientUnits="userSpaceOnUse" x1="0" x2="50"
                spreadMethod="reflect"><stop offset="0" stop-color="#f00"/>
                <stop offset="1" stop-color="#00f"/></linearGradient>
                <rect width="100" height="10" fill="url(#g)"/></svg>"##,
            1.,
        );
        assert_near(at(&p, 50., 5.), [0, 0, 255, 255], 8);
        assert_near(at(&p, 99.5, 5.), [255, 0, 0, 255], 8);
    }

    #[test]
    fn a_radial_gradient_starts_at_its_focal_point() {
        let p = render(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <radialGradient id="g" cx="0.5" cy="0.5" r="0.5" fx="0.3" fy="0.3">
                <stop offset="0" stop-color="#fff"/><stop offset="1" stop-color="#000"/>
                </radialGradient><rect width="100" height="100" fill="url(#g)"/></svg>"##,
            1.,
        );
        assert_near(at(&p, 30., 30.), [255, 255, 255, 255], 12);
        // The far side of the circle is the last stop; the center is not.
        assert_near(at(&p, 99., 50.), [0, 0, 0, 255], 12);
        assert!(at(&p, 50., 50.)[0] > 40);
    }

    #[test]
    fn a_group_s_opacity_is_the_whole_group_s() {
        let p = render(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="40">
                <g opacity="0.5"><rect width="40" height="40" fill="#000"/>
                <rect x="20" width="40" height="40" fill="#000"/></g></svg>"##,
            1.,
        );
        let (alone, both) = (at(&p, 10., 20.), at(&p, 30., 20.));
        assert!(alone[3].abs_diff(128) <= 2, "{alone:?}");
        // Not darker where they overlap.
        assert_eq!(alone[3], both[3]);
    }

    #[test]
    fn a_clip_cuts_the_group_to_its_outline() {
        let p = render(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <clipPath id="c"><circle cx="50" cy="50" r="40"/></clipPath>
                <g clip-path="url(#c)"><rect width="100" height="100" fill="#0a0"/></g></svg>"##,
            1.,
        );
        assert_near(at(&p, 50., 50.), [0, 170, 0, 255], 2);
        assert_eq!(at(&p, 3., 3.)[3], 0);
        assert_eq!(at(&p, 50., 95.)[3], 0);
    }

    #[test]
    fn a_mask_shows_the_group_as_one_picture() {
        // A yellow circle over a blue rect, through a mask of 50 % gray:
        // the circle stays yellow (half shown), not mixed with the blue.
        let p = render(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <mask id="m"><rect width="100" height="100" fill="#808080"/></mask>
                <g mask="url(#m)"><rect width="100" height="100" fill="#00f"/>
                <circle cx="50" cy="50" r="20" fill="#ff0"/></g></svg>"##,
            1.,
        );
        let sun = at(&p, 50., 50.);
        assert_near(sun, [255, 255, 0, 128], 6);
        let sky = at(&p, 5., 5.);
        assert_near(sky, [0, 0, 255, 128], 6);
    }

    #[test]
    fn an_alpha_mask_uses_the_mask_s_alpha() {
        let p = render(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="10">
                <mask id="m" style="mask-type: alpha"><rect width="50" height="10" fill="#000"/></mask>
                <g mask="url(#m)"><rect width="100" height="10" fill="#f00"/></g></svg>"##,
            1.,
        );
        assert_near(at(&p, 25., 5.), [255, 0, 0, 255], 2);
        assert_eq!(at(&p, 75., 5.)[3], 0);
    }

    #[test]
    fn a_blur_spreads_an_edge_over_both_sides() {
        let p = render(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="40">
                <filter id="b"><feGaussianBlur stdDeviation="4"/></filter>
                <g filter="url(#b)"><rect x="50" y="-40" width="100" height="120" fill="#000"/></g></svg>"##,
            1.,
        );
        // Half at the edge, little 2 σ out, nearly all 2 σ in.
        assert!(
            at(&p, 50., 20.)[3].abs_diff(128) <= 14,
            "{:?}",
            at(&p, 50., 20.)
        );
        assert!(at(&p, 42., 20.)[3] < 20);
        assert!(at(&p, 58., 20.)[3] > 235);
        // What lies past the view still counts: no dimmer edge at the right.
        assert!(at(&p, 99.5, 20.)[3] > 250);
    }

    #[test]
    fn a_shadow_is_under_the_content_moved() {
        let p = render(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <filter id="s" x="-1" y="-1" width="3" height="3"><feDropShadow dx="20" dy="20" stdDeviation="0.5"
                flood-color="#f00"/></filter>
                <g filter="url(#s)"><rect x="10" y="10" width="40" height="40" fill="#00f"/></g></svg>"##,
            1.,
        );
        assert_near(at(&p, 30., 30.), [0, 0, 255, 255], 2);
        assert_near(at(&p, 60., 60.), [255, 0, 0, 255], 4);
        assert_eq!(at(&p, 80., 80.)[3], 0);
    }

    #[test]
    fn text_sits_on_its_baseline_from_its_anchor() {
        let p = render(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="60">
                <text x="100" y="40" font-size="20" font-family="sans-serif" text-anchor="end"
                fill="#000">HHH</text></svg>"##,
            2.,
        );
        let (top, bottom) = inked_rows(&p).expect("ink");
        // Capitals stand on the baseline, about 0.7 em tall.
        assert!((bottom - 40.).abs() < 1., "bottom {bottom}");
        assert!((40. - top - 14.).abs() < 2.5, "top {top}");
        let (_, right) = inked_columns(&p, 35.).expect("ink");
        // An H's right stem ends a little before its advance.
        assert!(right <= 100. && right > 97., "right {right}");
    }

    #[test]
    fn centered_text_is_centered() {
        let p = render(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="60">
                <text x="100" y="40" font-size="20" text-anchor="middle" fill="#000">HHHH</text>
                </svg>"##,
            2.,
        );
        let (left, right) = inked_columns(&p, 35.).expect("ink");
        assert!(((left + right) / 2. - 100.).abs() < 1., "{left} … {right}");
    }

    #[test]
    fn bold_text_is_bolder_in_every_family() {
        // Cascadia Mono is one variable font: drawn at its default
        // outline, both weights would be the same.
        let ink = |family: &str, weight: u16| {
            let p = render(
                &format!(
                    r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="30">
                    <text x="4" y="22" font-size="16" font-family="{family}"
                    font-weight="{weight}" fill="#000">start end</text></svg>"##
                ),
                2.,
            );
            p.px.iter()
                .skip(3)
                .step_by(4)
                .map(|a| *a as u32)
                .sum::<u32>()
        };
        for family in ["monospace", "sans-serif", "serif"] {
            let (regular, bold) = (ink(family, 400), ink(family, 700));
            assert!(
                bold as f32 > regular as f32 * 1.15,
                "{family}: {regular} → {bold}"
            );
        }
    }

    #[test]
    fn a_sliced_image_covers_its_rectangle_and_no_more() {
        // 2 × 1 pixels in a 20 × 20 square: scaled to 40 × 20, centered,
        // so a quarter of each color is cut off; what shows is the half
        // between the two pixels' centers, mixed smoothly.
        let p = render(
            &format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
                <image x="10" width="20" height="20" preserveAspectRatio="xMidYMid slice"
                href="{RED_BLUE}"/></svg>"##
            ),
            1.,
        );
        assert_near(at(&p, 10., 10.), [249, 0, 6, 255], 4);
        assert_near(at(&p, 20., 10.), [128, 0, 128, 255], 6);
        assert_near(at(&p, 29., 10.), [6, 0, 249, 255], 4);
        assert_eq!(at(&p, 8., 10.)[3], 0);
        assert_eq!(at(&p, 31., 10.)[3], 0);
    }

    #[test]
    fn a_met_image_keeps_its_proportions() {
        let p = render(
            &format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
                <image width="20" height="20" href="{RED_BLUE}"/></svg>"##
            ),
            1.,
        );
        // 20 × 10, centered: rows 5 to 15.
        assert_eq!(at(&p, 10., 2.)[3], 0);
        assert_near(at(&p, 2., 10.), [255, 0, 0, 255], 4);
        assert_near(at(&p, 18., 10.), [0, 0, 255, 255], 4);
        assert_eq!(at(&p, 10., 18.)[3], 0);
    }

    #[test]
    fn half_the_scale_draws_the_same_picture() {
        let doc = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
            <linearGradient id="g"><stop offset="0" stop-color="#f00"/>
            <stop offset="1" stop-color="#00f"/></linearGradient>
            <circle cx="50" cy="50" r="40" fill="url(#g)"/></svg>"##;
        let (big, small) = (render(doc, 2.), render(doc, 1.));
        assert_eq!((big.w, small.w), (200, 100));
        for (x, y) in [(50., 50.), (20., 50.), (80., 50.), (5., 5.)] {
            assert_near(at(&big, x, y), at(&small, x, y), 6);
        }
    }

    /// Draws every `.svg` of `PRISM_SVG_DIR` at 3 pixels per point into
    /// PNGs next to it, in light colors over white and in dark ones over
    /// Windows' dark background: for looking at.
    #[test]
    #[ignore]
    fn dump() {
        let Ok(dir) = std::env::var("PRISM_SVG_DIR") else {
            return;
        };
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "svg") {
                continue;
            }
            let markup = std::fs::read_to_string(&path).unwrap();
            for (dark, under, name) in [
                (false, [255; 3], "svg.win.png"),
                (true, [32; 3], "svg.win-dark.png"),
            ] {
                let mut p = render_in(&markup, 3., dark);
                crate::wic::unpremultiply(&mut p.px, Some(under));
                let out = path.with_extension(name);
                let wic = crate::wic::factory().unwrap();
                crate::wic::save_png(&wic, out.to_str().unwrap(), p.w, p.h, &p.px).unwrap();
            }
        }
    }
}
