//! Small icons drawn at run time with GDI: a tray icon from a Segoe Fluent
//! Icons glyph, and the taskbar badge (a number on a disc). GDI draws
//! white on black four times larger; the average of each 4 × 4 block is
//! the icon's alpha, so edges are smooth.

use prism_core::Color;
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};
use windows::core::PCWSTR;

use crate::win::wide;

const SS: i32 = 4;

/// 32-bit pixels of a `size` × `size` DIB section (top-down BGRA).
struct Canvas {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    size: i32,
}

impl Canvas {
    fn new(size: i32) -> Option<Canvas> {
        unsafe {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size,
                    biHeight: -size,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let dc = CreateCompatibleDC(None);
            let old = SelectObject(dc, bitmap.into());
            Some(Canvas {
                dc,
                bitmap,
                old,
                bits: bits as *mut u8,
                size,
            })
        }
    }

    fn pixels(&mut self) -> &mut [u8] {
        let _ = unsafe { GdiFlush() };
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.size * self.size * 4) as usize) }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteDC(self.dc);
            let _ = DeleteObject(self.bitmap.into());
        }
    }
}

/// The coverage (0–255) of what `draw` paints white, at `size` pixels.
fn coverage(size: i32, draw: impl FnOnce(HDC, i32)) -> Option<Vec<u8>> {
    let mut big = Canvas::new(size * SS)?;
    unsafe {
        let _ = PatBlt(big.dc, 0, 0, size * SS, size * SS, BLACKNESS);
    }
    draw(big.dc, size * SS);
    let px = big.pixels();
    let stride = (size * SS) as usize;
    let mut out = vec![0u8; (size * size) as usize];
    for y in 0..size as usize {
        for x in 0..size as usize {
            let mut sum = 0u32;
            for dy in 0..SS as usize {
                for dx in 0..SS as usize {
                    let i = ((y * SS as usize + dy) * stride + x * SS as usize + dx) * 4;
                    sum += px[i + 1] as u32;
                }
            }
            out[y * size as usize + x] = (sum / (SS * SS) as u32) as u8;
        }
    }
    Some(out)
}

fn font(height: i32, weight: i32, face: &str) -> HFONT {
    let face = wide(face);
    unsafe {
        CreateFontW(
            -height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            0,
            PCWSTR(face.as_ptr()),
        )
    }
}

fn text(dc: HDC, s: &str, size: i32, height: i32, weight: i32, face: &str) {
    unsafe {
        let f = font(height, weight, face);
        let old = SelectObject(dc, f.into());
        SetTextColor(dc, COLORREF(0xFFFFFF));
        SetBkMode(dc, TRANSPARENT);
        let mut r = RECT {
            left: 0,
            top: 0,
            right: size,
            bottom: size,
        };
        let mut w: Vec<u16> = s.encode_utf16().collect();
        DrawTextW(
            dc,
            &mut w,
            &mut r,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(dc, old);
        let _ = DeleteObject(f.into());
    }
}

fn disc(dc: HDC, size: i32) {
    unsafe {
        let brush = GetStockObject(WHITE_BRUSH);
        let pen = GetStockObject(NULL_PEN);
        let ob = SelectObject(dc, brush);
        let op = SelectObject(dc, pen);
        let _ = Ellipse(dc, 0, 0, size + 1, size + 1);
        SelectObject(dc, ob);
        SelectObject(dc, op);
    }
}

fn channel(v: f32) -> u8 {
    (v.clamp(0., 1.) * 255.).round() as u8
}

/// An icon from layers of (coverage, color), the first at the bottom.
fn compose(size: i32, layers: &[(Vec<u8>, Color)]) -> Option<HICON> {
    let mut canvas = Canvas::new(size)?;
    let px = canvas.pixels();
    for i in 0..(size * size) as usize {
        // Premultiplied "over".
        let (mut r, mut g, mut b, mut a) = (0f32, 0f32, 0f32, 0f32);
        for (cov, c) in layers {
            let alpha = cov[i] as f32 / 255. * c.a;
            r = c.r * alpha + r * (1. - alpha);
            g = c.g * alpha + g * (1. - alpha);
            b = c.b * alpha + b * (1. - alpha);
            a = alpha + a * (1. - alpha);
        }
        px[i * 4] = channel(b);
        px[i * 4 + 1] = channel(g);
        px[i * 4 + 2] = channel(r);
        px[i * 4 + 3] = channel(a);
    }
    unsafe {
        let mask = CreateBitmap(size, size, 1, 1, None);
        let info = ICONINFO {
            fIcon: true.into(),
            hbmMask: mask,
            hbmColor: canvas.bitmap,
            ..Default::default()
        };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(mask.into());
        icon
    }
}

/// A Segoe Fluent Icons glyph in `color`, `size` pixels square.
pub fn glyph(glyph: char, size: i32, color: Color) -> Option<HICON> {
    let cov = coverage(size, |dc, s| {
        text(
            dc,
            &glyph.to_string(),
            s,
            s * 7 / 8,
            400,
            "Segoe Fluent Icons",
        )
    })?;
    compose(size, &[(cov, color)])
}

/// The badge: `label` ("3", "99+") on a disc of `fill`, `size` pixels.
pub fn badge(label: &str, size: i32, fill: Color, ink: Color) -> Option<HICON> {
    let back = coverage(size, disc)?;
    let n = label.chars().count();
    let ratio = match n {
        0 | 1 => 0.72,
        2 => 0.6,
        _ => 0.44,
    };
    let front = coverage(size, |dc, s| {
        text(
            dc,
            label,
            s,
            (s as f32 * ratio) as i32,
            700,
            "Segoe UI Variable Display",
        )
    })?;
    compose(size, &[(back, fill), (front, ink)])
}

/// The badge's text for a count.
pub fn badge_label(n: u32) -> String {
    if n > 99 { "99+".into() } else { n.to_string() }
}
