//! A rich input's chips (#115) as real inline objects: each is a picture
//! RichEdit embeds in the text (`ITextRange::InsertImage`), one character
//! (U+FFFC, Prism's `OBJECT_CHAR`) the caret jumps over and Backspace
//! deletes whole, like the `NSTextAttachment` of macOS. The picture is a
//! rounded box in the chip's background (a light tint of the text color
//! by default), its symbol (Segoe Fluent Icons) and its label, in the
//! run's font, drawn with Direct2D and DirectWrite into a WIC bitmap at the
//! window's scale and sitting on the baseline like the text around it.

use prism_core::Color;
use windows::Storage::Streams::IRandomAccessStream;
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
    D2D1CreateFactory, ID2D1Factory,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_LINE_METRICS, DWRITE_TEXT_METRICS, DWriteCreateFactory,
    IDWriteFactory, IDWriteTextLayout,
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
    pub label: String,
    /// Its symbol's glyph (Segoe Fluent Icons).
    pub icon: Option<char>,
    /// The run's font: its family (one face), size (DIPs) and weight.
    pub family: String,
    pub size: f32,
    pub weight: u16,
    pub color: Color,
    pub background: Color,
    /// The window's scale (pixels per DIP).
    pub scale: f32,
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
) -> Result<IDWriteTextLayout> {
    unsafe {
        let format = dw.CreateTextFormat(
            &HSTRING::from(family),
            None,
            DWRITE_FONT_WEIGHT(weight as i32),
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            &HSTRING::from(""),
        )?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        dw.CreateTextLayout(&wide, &format, 10_000., 10_000.)
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

/// Draws `look` as a PNG in a WinRT stream.
fn draw(look: &Look) -> Result<Picture> {
    let (d2d, dw) = factories()?;
    let text = layout(&dw, &look.label, &look.family, look.size, look.weight)?;
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
    let pad = (em * 0.35).round();
    let icon_w = if look.icon.is_some() { em } else { 0. };
    let gap = if look.icon.is_some() {
        (em * 0.25).round()
    } else {
        0.
    };
    let width = (pad * 2. + icon_w + gap + metrics.widthIncludingTrailingWhitespace).ceil();
    let height = line[0].height.ceil().max(1.);
    let ascent = line[0].baseline;
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
        rt.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: 0.,
                    top: 0.,
                    right: width,
                    bottom: height,
                },
                radiusX: 4.,
                radiusY: 4.,
            },
            &bg,
        );
        let mut x = pad;
        if let Some(glyph) = look.icon {
            let icon = layout(&dw, &glyph.to_string(), "Segoe Fluent Icons", em * 0.9, 400)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            icon.GetMetrics(&mut m)?;
            let origin = windows_numerics::Vector2 {
                X: x + (icon_w - m.width) / 2.,
                Y: (height - m.height) / 2.,
            };
            rt.DrawTextLayout(origin, &icon, &ink, D2D1_DRAW_TEXT_OPTIONS_NONE);
            x += icon_w + gap;
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
