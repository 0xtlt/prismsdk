//! A rich input's chips (#115) as real inline objects: each is a picture
//! RichEdit embeds in the text (`ITextRange::InsertImage`), one character
//! (U+FFFC, Prism's `OBJECT_CHAR`) the caret jumps over and Backspace
//! deletes whole, like the `NSTextAttachment` of macOS. The picture is a
//! rounded box in the chip's background (a light tint of the text color
//! by default), its symbol (Segoe Fluent Icons) and its label, in the
//! run's font, drawn with Direct2D and DirectWrite into a WIC bitmap at the
//! window's scale and sitting on the baseline like the text around it.
//!
//! Its options (#329) follow prism-native's `chips` numbers: its label's
//! runs (each its weight, slant and color, DirectWrite ranges), cut with
//! an ellipsis at its largest width (DirectWrite trimming), its ring,
//! radius and height, and a vector icon (a Direct2D path geometry). Its
//! tool tip is `rich.rs`'.

use prism_core::{Color, Path, Segment, TextRun};
use windows::Storage::Streams::IRandomAccessStream;
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F,
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN,
    D2D1_FILL_MODE_ALTERNATE, D2D1_FILL_MODE_WINDING, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_CAP_STYLE_FLAT, D2D1_CAP_STYLE_ROUND, D2D1_DASH_STYLE_CUSTOM, D2D1_DASH_STYLE_SOLID,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_LINE_JOIN_MITER,
    D2D1_LINE_JOIN_ROUND, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
    D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
    D2D1CreateFactory, ID2D1Factory, ID2D1PathGeometry, ID2D1RenderTarget,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_LINE_METRICS, DWRITE_TEXT_METRICS,
    DWRITE_TEXT_RANGE, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    GUID_ContainerFormatPng, GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad,
    WICBitmapEncoderNoCache,
};
use windows::Win32::System::Com::STREAM_SEEK_SET;
use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
use windows::Win32::System::WinRT::{BSOS_DEFAULT, CreateRandomAccessStreamOverStream};
use windows_core::{HSTRING, Result};

use crate::winui::bindings::Microsoft::UI::Text::{ITextRange, VerticalCharacterAlignment};

/// What a chip looks like: what its picture is drawn from.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    /// Its label, plain: the picture's alternative text.
    pub label: String,
    /// Its label's runs: their text, weight, slant and color (resolved).
    pub runs: Vec<LabelRun>,
    /// Its symbol's glyph (Segoe Fluent Icons).
    pub icon: Option<char>,
    /// Its vector icon's paths in its box (`em` × ratio by `em`), colors
    /// resolved for the theme.
    pub vector: Vec<Path>,
    /// The chip's run: its box's options (prism-native `chips`).
    pub run: TextRun,
    /// Its ring: width and color (resolved).
    pub ring: Option<(f32, Color)>,
    /// The run's font: its family (one face), size (DIPs), weight, slant.
    pub family: String,
    pub size: f32,
    pub weight: u16,
    pub italic: bool,
    pub color: Color,
    pub background: Color,
    /// The window's scale (pixels per DIP).
    pub scale: f32,
}

/// A run of a chip's label.
#[derive(Clone, Debug, PartialEq)]
pub struct LabelRun {
    pub text: String,
    pub weight: Option<u16>,
    pub italic: bool,
    pub color: Option<Color>,
}

/// A picture of a chip, ready for RichEdit.
struct Picture {
    stream: IRandomAccessStream,
    /// Its size in DIPs, and how far it rises above the baseline.
    width: f32,
    height: f32,
    ascent: f32,
}

fn factories() -> Result<(ID2D1Factory, IDWriteFactory)> {
    unsafe {
        let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        Ok((d2d, dw))
    }
}

fn layout(
    dw: &IDWriteFactory,
    text: &str,
    family: &str,
    size: f32,
    weight: u16,
    italic: bool,
) -> Result<IDWriteTextLayout> {
    unsafe {
        let format = dw.CreateTextFormat(
            &HSTRING::from(family),
            None,
            DWRITE_FONT_WEIGHT(weight as i32),
            if italic {
                DWRITE_FONT_STYLE_ITALIC
            } else {
                DWRITE_FONT_STYLE_NORMAL
            },
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            &HSTRING::from(""),
        )?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        dw.CreateTextLayout(&wide, &format, 10_000., 10_000.)
    }
}

/// `look`'s label: its runs' weights and slants as ranges of one layout.
fn label_layout(dw: &IDWriteFactory, look: &Look) -> Result<IDWriteTextLayout> {
    let text: String = look.runs.iter().map(|r| r.text.as_str()).collect();
    let l = layout(dw, &text, &look.family, look.size, look.weight, look.italic)?;
    let mut at = 0u32;
    for r in &look.runs {
        let range = DWRITE_TEXT_RANGE {
            startPosition: at,
            length: r.text.encode_utf16().count() as u32,
        };
        at += range.length;
        unsafe {
            if let Some(w) = r.weight {
                l.SetFontWeight(DWRITE_FONT_WEIGHT(w as i32), range)?;
            }
            if r.italic {
                l.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range)?;
            }
        }
    }
    Ok(l)
}

/// `paths` as Direct2D draws them: each its geometry, then its fill and
/// stroke.
fn draw_paths(
    d2d: &ID2D1Factory,
    rt: &ID2D1RenderTarget,
    paths: &[Path],
    dx: f32,
    dy: f32,
) -> Result<()> {
    let at = |x: f32, y: f32| windows_numerics::Vector2 {
        X: x + dx,
        Y: y + dy,
    };
    for p in paths {
        let geometry: ID2D1PathGeometry = unsafe { d2d.CreatePathGeometry()? };
        unsafe {
            let sink = geometry.Open()?;
            sink.SetFillMode(if p.even_odd {
                D2D1_FILL_MODE_ALTERNATE
            } else {
                D2D1_FILL_MODE_WINDING
            });
            let mut open = false;
            for s in &p.segments {
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
                    _ => {}
                }
            }
            if open {
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
            }
            sink.Close()?;
        }
        if let Some(c) = p.fill {
            let brush = unsafe { rt.CreateSolidColorBrush(&d2d_color(c), None)? };
            unsafe { rt.FillGeometry(&geometry, &brush, None) };
        }
        if let Some(c) = p.stroke {
            let brush = unsafe { rt.CreateSolidColorBrush(&d2d_color(c), None)? };
            let (cap, join) = if p.round {
                (D2D1_CAP_STYLE_ROUND, D2D1_LINE_JOIN_ROUND)
            } else {
                (D2D1_CAP_STYLE_FLAT, D2D1_LINE_JOIN_MITER)
            };
            let w = p.width.max(0.01);
            let dashes = p.dash.map(|(d, g)| [d / w, g / w]);
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
            let style = unsafe { d2d.CreateStrokeStyle(&props, dashes.as_ref().map(|d| &d[..]))? };
            unsafe { rt.DrawGeometry(&geometry, &brush, p.width, &style) };
        }
    }
    Ok(())
}

fn d2d_color(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

/// Draws `look` as a PNG in a WinRT stream.
fn draw(look: &Look) -> Result<Picture> {
    let (d2d, dw) = factories()?;
    let text = label_layout(&dw, look)?;
    let (mut metrics, mut line) = (
        DWRITE_TEXT_METRICS::default(),
        [DWRITE_LINE_METRICS::default()],
    );
    let mut lines = 0;
    unsafe {
        text.GetMetrics(&mut metrics)?;
        text.GetLineMetrics(Some(&mut line), &mut lines)?;
    }
    let em = look.size;
    let label_w = if look.label.is_empty() {
        0.
    } else {
        metrics.widthIncludingTrailingWhitespace
    };
    let mut run = look.run.clone();
    if look.icon.is_none() && look.vector.is_empty() {
        // A symbol without a Fluent glyph: no room for it.
        run.chip_icon = None;
        run.chip_drawing = None;
    }
    let text_h = line[0].height.ceil().max(1.);
    let g = prism_native::chips::chip_box(&run, em, text_h, label_w);
    let (width, height) = (g.width, g.height);
    let extra = (height - text_h) / 2.;
    let ascent = line[0].baseline + extra;
    if g.cut {
        // Cut where its room ends, with an ellipsis.
        unsafe {
            text.SetMaxWidth(g.label_w)?;
            let format = dw.CreateTextFormat(
                &HSTRING::from(look.family.as_str()),
                None,
                DWRITE_FONT_WEIGHT(look.weight as i32),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                look.size,
                &HSTRING::from(""),
            )?;
            let sign = dw.CreateEllipsisTrimmingSign(&format)?;
            text.SetTrimming(
                &DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                },
                &sign,
            )?;
        }
    }
    let k = look.scale.max(1.);
    let (pw, ph) = ((width * k).ceil() as u32, (height * k).ceil() as u32);
    let wic = crate::wic::factory()?;
    unsafe {
        let bitmap =
            wic.CreateBitmap(pw, ph, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96. * k,
            dpiY: 96. * k,
            ..Default::default()
        };
        let rt = d2d.CreateWicBitmapRenderTarget(&bitmap, &props)?;
        rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        rt.BeginDraw();
        rt.Clear(Some(&D2D1_COLOR_F::default()));
        let bg = rt.CreateSolidColorBrush(&d2d_color(look.background), None)?;
        let ink = rt.CreateSolidColorBrush(&d2d_color(look.color), None)?;
        let rect = D2D_RECT_F {
            left: 0.,
            top: 0.,
            right: width,
            bottom: height,
        };
        rt.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect,
                radiusX: g.radius,
                radiusY: g.radius,
            },
            &bg,
        );
        if let Some((w, c)) = look.ring {
            let ring = rt.CreateSolidColorBrush(&d2d_color(c), None)?;
            let r = (g.radius - w / 2.).max(0.);
            rt.DrawRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: w / 2.,
                        top: w / 2.,
                        right: width - w / 2.,
                        bottom: height - w / 2.,
                    },
                    radiusX: r,
                    radiusY: r,
                },
                &ring,
                w,
                None,
            );
        }
        // Each run's own color.
        let mut at = 0u32;
        for r in &look.runs {
            let length = r.text.encode_utf16().count() as u32;
            if let Some(c) = r.color {
                let brush = rt.CreateSolidColorBrush(&d2d_color(c), None)?;
                text.SetDrawingEffect(
                    &brush,
                    DWRITE_TEXT_RANGE {
                        startPosition: at,
                        length,
                    },
                )?;
            }
            at += length;
        }
        let mut x = g.pad;
        if !look.vector.is_empty() {
            draw_paths(&d2d, &rt, &look.vector, x, (height - g.icon_h) / 2.)?;
            x += g.icon_w + g.gap;
        } else if let Some(glyph) = look.icon {
            let icon = layout(
                &dw,
                &glyph.to_string(),
                "Segoe Fluent Icons",
                em * 0.9,
                400,
                false,
            )?;
            let mut m = DWRITE_TEXT_METRICS::default();
            icon.GetMetrics(&mut m)?;
            let origin = windows_numerics::Vector2 {
                X: x + (g.icon_w - m.width) / 2.,
                Y: (height - m.height) / 2.,
            };
            rt.DrawTextLayout(origin, &icon, &ink, D2D1_DRAW_TEXT_OPTIONS_NONE);
            x += g.icon_w + g.gap;
        }
        let origin = windows_numerics::Vector2 {
            X: x,
            Y: (height - metrics.height) / 2.,
        };
        rt.DrawTextLayout(origin, &text, &ink, D2D1_DRAW_TEXT_OPTIONS_NONE);
        rt.EndDraw(None, None)?;
        // As a PNG in memory, read through a WinRT stream.
        let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true)?;
        let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let (mut frame, mut bag) = (None, None);
        enc.CreateNewFrame(&mut frame, &mut bag)?;
        let frame = frame.ok_or_else(windows_core::Error::empty)?;
        frame.Initialize(bag.as_ref())?;
        frame.SetSize(pw, ph)?;
        frame.WriteSource(&bitmap, std::ptr::null())?;
        frame.Commit()?;
        enc.Commit()?;
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let stream: IRandomAccessStream =
            CreateRandomAccessStreamOverStream(&stream, BSOS_DEFAULT)?;
        Ok(Picture {
            stream,
            width,
            height,
            ascent,
        })
    }
}

/// Puts chip `look` in place of the character at UTF-16 position `at` of
/// `range`'s document (`range` spans it).
pub fn place(range: &ITextRange, look: &Look) -> Result<()> {
    let p = draw(look)?;
    range.SetText(&HSTRING::new())?;
    range.InsertImage(
        p.width.round() as i32,
        p.height.round() as i32,
        p.ascent.round() as i32,
        VerticalCharacterAlignment::Baseline,
        &HSTRING::from(look.label.as_str()),
        &p.stream,
    )
}

/// The default background of a chip: the text color, lightly.
pub fn tint(color: Color) -> Color {
    color.with_alpha(0.14)
}
