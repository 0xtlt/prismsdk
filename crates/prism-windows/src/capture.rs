//! What DWM shows of the window (`PrintWindow` with
//! `PW_RENDERFULLCONTENT`): the XAML island as composed, Mica, Composition
//! shadows and open flyouts included; and what the screen shows over it,
//! the windows above it too (`screen_area`: WinUI's tooltips are windows
//! of their own). Needs a desktop session.

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, ClientToScreen, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HDC, HGDIOBJ, ReleaseDC,
    SRCCOPY, SelectObject,
};
use windows::Win32::Graphics::Imaging::IWICImagingFactory;
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows_core::Result;

/// Saves the window's client area as DWM composes it to `path` (PNG).
pub fn print_window(h: HWND, path: &str, wic: &IWICImagingFactory) -> Result<()> {
    // PW_CLIENTONLY | PW_RENDERFULLCONTENT
    client_area(h, path, wic, |dc, _, _| unsafe {
        PrintWindow(h, dc, PRINT_WINDOW_FLAGS(1 | 2)).as_bool()
    })
}

/// Saves what the screen shows over the window's client area to `path`
/// (PNG): the window and what lies above it, a tooltip under the pointer.
pub fn screen_area(h: HWND, path: &str, wic: &IWICImagingFactory) -> Result<()> {
    let mut at = POINT::default();
    unsafe {
        let _ = ClientToScreen(h, &mut at);
    }
    client_area(h, path, wic, |dc, w, ht| unsafe {
        let screen = GetDC(None);
        let copied = BitBlt(
            dc,
            0,
            0,
            w,
            ht,
            Some(screen),
            at.x,
            at.y,
            SRCCOPY | CAPTUREBLT,
        );
        ReleaseDC(None, screen);
        copied.is_ok()
    })
}

/// A bitmap the size of `h`'s client area, filled by `draw` (into its DC,
/// at that width and height), saved to `path`.
fn client_area(
    h: HWND,
    path: &str,
    wic: &IWICImagingFactory,
    draw: impl FnOnce(HDC, i32, i32) -> bool,
) -> Result<()> {
    unsafe {
        let mut r = RECT::default();
        GetClientRect(h, &mut r)?;
        let (w, ht) = (r.right.max(1), r.bottom.max(1));
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -ht,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bmp = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let old = SelectObject(dc, HGDIOBJ(bmp.0));
        let ok = draw(dc, w, ht);
        let mut px = std::slice::from_raw_parts(bits as *const u8, (w * ht * 4) as usize).to_vec();
        SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        if !ok {
            return Err(windows_core::Error::from_thread());
        }
        for p in px.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
        crate::wic::save_png(wic, path, w as u32, ht as u32, &px)
    }
}
