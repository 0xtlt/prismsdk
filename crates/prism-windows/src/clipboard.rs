//! The clipboard in all its types (#246), in Windows' formats:
//!
//! | Prism | Windows |
//! |---|---|
//! | `text/plain` | `CF_UNICODETEXT` |
//! | `text/html` | `HTML Format` (CF_HTML: a header of offsets, then the page) |
//! | `text/rtf` | `Rich Text Format` |
//! | images | `PNG` (what browsers and Office put), and `CF_DIBV5` for every other app; read back from either |
//! | `text/uri-list` | `CF_HDROP` (files Explorer can paste) |
//! | other MIME types | a format registered under the type's name |

use prism_core::{ClipboardItem, DeviceError};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::Graphics::Imaging::{
    GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapEncoderNoCache, WICBitmapPaletteTypeMedianCut,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::STREAM_SEEK_CUR;
use windows::Win32::System::Com::StructuredStorage::{CreateStreamOnHGlobal, GetHGlobalFromStream};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardFormatNameW, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::core::PCWSTR;

use crate::win::wide;

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;
const CF_DIBV5: u32 = 17;

fn failed(e: windows::core::Error) -> DeviceError {
    DeviceError::Failed(e.message())
}

fn format(name: &str) -> u32 {
    let n = wide(name);
    unsafe { RegisterClipboardFormatW(PCWSTR(n.as_ptr())) }
}

/// The clipboard, open for the time of `f`.
fn with_clipboard<R>(owner: HWND, f: impl FnOnce() -> R) -> Result<R, DeviceError> {
    // Another app may hold it for a moment.
    let mut tries = 0;
    loop {
        match unsafe { OpenClipboard(Some(owner)) } {
            Ok(()) => break,
            Err(_) if tries < 5 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(failed(e)),
        }
    }
    let r = f();
    unsafe {
        let _ = CloseClipboard();
    }
    Ok(r)
}

fn put(format: u32, bytes: &[u8]) -> windows::core::Result<()> {
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let p = GlobalLock(mem) as *mut u8;
        if p.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(mem);
        SetClipboardData(format, Some(HANDLE(mem.0)))?;
        Ok(())
    }
}

/// A format's bytes (the clipboard is open).
fn get(format: u32) -> Option<Vec<u8>> {
    unsafe {
        IsClipboardFormatAvailable(format).ok()?;
        let h = GetClipboardData(format).ok()?;
        let mem = HGLOBAL(h.0);
        let p = GlobalLock(mem) as *const u8;
        if p.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(p, GlobalSize(mem)).to_vec();
        let _ = GlobalUnlock(mem);
        Some(bytes)
    }
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain(Some(0))
        .flat_map(|c| c.to_le_bytes())
        .collect()
}

fn from_utf16z(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|c| *c != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Puts text on the clipboard.
pub fn write_text(owner: HWND, text: &str) -> Result<(), DeviceError> {
    with_clipboard(owner, || unsafe {
        EmptyClipboard()?;
        put(CF_UNICODETEXT, &utf16z(text))
    })?
    .map_err(failed)
}

/// The clipboard's text.
pub fn read_text(owner: HWND) -> Option<String> {
    with_clipboard(owner, || get(CF_UNICODETEXT).map(|b| from_utf16z(&b)))
        .ok()
        .flatten()
}

/// Every type of `item` on the clipboard.
pub fn write_item(owner: HWND, item: &ClipboardItem) -> Result<(), DeviceError> {
    let wic = crate::wic::factory().ok();
    with_clipboard(owner, || -> windows::core::Result<()> {
        unsafe { EmptyClipboard()? };
        for (ty, data) in item.entries() {
            match ty {
                ClipboardItem::TEXT => {
                    put(CF_UNICODETEXT, &utf16z(&String::from_utf8_lossy(data)))?
                }
                ClipboardItem::HTML => put(
                    format("HTML Format"),
                    &cf_html(&String::from_utf8_lossy(data)),
                )?,
                ClipboardItem::RTF => put(format("Rich Text Format"), data)?,
                ClipboardItem::URI_LIST => {
                    let paths: Vec<String> = String::from_utf8_lossy(data)
                        .lines()
                        .filter_map(file_path)
                        .collect();
                    if !paths.is_empty() {
                        put(CF_HDROP, &drop_files(&paths))?;
                    }
                    put(format(ty), data)?;
                }
                ClipboardItem::PNG => put(format("PNG"), data)?,
                t => put(format(t), data)?,
            }
        }
        // Every app reads a DIB.
        if let Some(image) = item.as_image()
            && let Some(wic) = &wic
            && let Ok(dib) = dib_v5(wic, &image.bytes)
        {
            put(CF_DIBV5, &dib)?;
        }
        Ok(())
    })?
    .map_err(failed)
}

/// The clipboard in all its types.
pub fn read_item(owner: HWND) -> ClipboardItem {
    let wic = crate::wic::factory().ok();
    with_clipboard(owner, || {
        let mut item = ClipboardItem::new();
        if let Some(b) = get(CF_UNICODETEXT) {
            item = item.text(from_utf16z(&b));
        }
        if let Some(b) = get(format("HTML Format"))
            && let Some(html) = html_fragment(&b)
        {
            item = item.html(html);
        }
        if let Some(b) = get(format("Rich Text Format")) {
            let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
            item = item.custom(ClipboardItem::RTF, b[..end].to_vec());
        }
        let png = get(format("PNG")).or_else(|| {
            let wic = wic.as_ref()?;
            get(CF_DIBV5)
                .or_else(|| get(CF_DIB))
                .and_then(|dib| dib_to_png(wic, &dib).ok())
        });
        if let Some(png) = png {
            item = item.image(png, ClipboardItem::PNG);
        }
        if let Some(files) = dropped_files() {
            item = item.custom(ClipboardItem::URI_LIST, files.into_bytes());
        }
        // Formats named like MIME types (another Prism app's, browsers').
        let mut f = 0;
        loop {
            f = unsafe { EnumClipboardFormats(f) };
            if f == 0 {
                break;
            }
            let mut name = [0u16; 128];
            let n = unsafe { GetClipboardFormatNameW(f, &mut name) };
            if n <= 0 {
                continue;
            }
            let name = String::from_utf16_lossy(&name[..n as usize]);
            let known = [
                ClipboardItem::TEXT,
                ClipboardItem::HTML,
                ClipboardItem::RTF,
                ClipboardItem::PNG,
                ClipboardItem::URI_LIST,
            ];
            if name.contains('/')
                && !name.contains(' ')
                && !known.contains(&name.as_str())
                && !item.has(&name)
                && let Some(b) = get(f)
            {
                item = item.custom(name, b);
            }
        }
        item
    })
    .unwrap_or_default()
}

/// `file:///C:/a%20b.txt` → `C:\a b.txt`.
fn file_path(uri: &str) -> Option<String> {
    let rest = uri.trim().strip_prefix("file:///")?;
    let mut out = Vec::new();
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).ok()?, 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(if b[i] == b'/' { b'\\' } else { b[i] });
        i += 1;
    }
    String::from_utf8(out).ok()
}

fn file_uri(path: &str) -> String {
    let mut out = String::from("file:///");
    for b in path.replace('\\', "/").bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~/:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `DROPFILES` and its paths (UTF-16, each NUL-ended, then a NUL).
fn drop_files(paths: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    // pFiles (offset of the list), pt.x, pt.y, fNC, fWide.
    out.extend(20u32.to_le_bytes());
    out.extend(0i32.to_le_bytes());
    out.extend(0i32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(1u32.to_le_bytes());
    for p in paths {
        out.extend(utf16z(p));
    }
    out.extend(0u16.to_le_bytes());
    out
}

/// The files on the clipboard as a URI list (the clipboard is open).
fn dropped_files() -> Option<String> {
    unsafe {
        IsClipboardFormatAvailable(CF_HDROP).ok()?;
        let h = GetClipboardData(CF_HDROP).ok()?;
        let drop = HDROP(h.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let mut uris = Vec::new();
        for i in 0..count {
            let len = DragQueryFileW(drop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(drop, i, Some(&mut buf));
            uris.push(file_uri(&String::from_utf16_lossy(&buf[..len])));
        }
        (!uris.is_empty()).then(|| uris.join("\r\n"))
    }
}

/// The CF_HTML form of an HTML fragment.
pub fn cf_html(fragment: &str) -> Vec<u8> {
    const HEADER: &str = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    let before = "<html><body>\r\n<!--StartFragment-->";
    let after = "<!--EndFragment-->\r\n</body></html>";
    let start_html = HEADER.len();
    let start_fragment = start_html + before.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + after.len();
    let header = format!(
        "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n"
    );
    let mut out = format!("{header}{before}{fragment}{after}").into_bytes();
    out.push(0);
    out
}

/// The fragment of a CF_HTML document (its offsets are bytes of UTF-8).
pub fn html_fragment(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let offset = |key: &str| -> Option<usize> {
        let at = text.find(key)? + key.len();
        text[at..]
            .lines()
            .next()?
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|v| *v >= 0)
            .map(|v| v as usize)
    };
    let (a, b) = (offset("StartFragment:")?, offset("EndFragment:")?);
    let slice = bytes.get(a..b.min(bytes.len()))?;
    Some(String::from_utf8_lossy(slice).into_owned())
}

/// An encoded image as a `CF_DIBV5` (straight alpha, bottom-up rows).
fn dib_v5(wic: &IWICImagingFactory, encoded: &[u8]) -> windows::core::Result<Vec<u8>> {
    let (w, h, px) = decode_bgra(wic, encoded)?;
    let mut out = Vec::with_capacity(124 + px.len());
    out.extend(124u32.to_le_bytes()); // bV5Size
    out.extend((w as i32).to_le_bytes());
    out.extend((h as i32).to_le_bytes()); // bottom-up
    out.extend(1u16.to_le_bytes()); // planes
    out.extend(32u16.to_le_bytes()); // bits
    out.extend(3u32.to_le_bytes()); // BI_BITFIELDS
    out.extend((w * h * 4).to_le_bytes());
    out.extend(2835i32.to_le_bytes()); // 72 dpi
    out.extend(2835i32.to_le_bytes());
    out.extend(0u32.to_le_bytes()); // colors used
    out.extend(0u32.to_le_bytes()); // important
    out.extend(0x00FF_0000u32.to_le_bytes()); // red mask
    out.extend(0x0000_FF00u32.to_le_bytes());
    out.extend(0x0000_00FFu32.to_le_bytes());
    out.extend(0xFF00_0000u32.to_le_bytes()); // alpha mask
    out.extend(0x7352_4742u32.to_le_bytes()); // LCS_sRGB
    out.resize(124, 0); // endpoints, gamma, intent, profile: zero
    out[108..112].copy_from_slice(&4u32.to_le_bytes()); // LCS_GM_IMAGES
    let stride = (w * 4) as usize;
    for row in (0..h as usize).rev() {
        out.extend(&px[row * stride..(row + 1) * stride]);
    }
    Ok(out)
}

/// The first frame of an encoded image: width, height, straight BGRA.
fn decode_bgra(
    wic: &IWICImagingFactory,
    encoded: &[u8],
) -> windows::core::Result<(u32, u32, Vec<u8>)> {
    unsafe {
        let stream = wic.CreateStream()?;
        stream.InitializeFromMemory(encoded)?;
        let decoder =
            wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let converter = wic.CreateFormatConverter()?;
        converter.Initialize(
            &frame,
            &GUID_WICPixelFormat32bppBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.,
            WICBitmapPaletteTypeMedianCut,
        )?;
        let (mut w, mut h) = (0u32, 0u32);
        converter.GetSize(&mut w, &mut h)?;
        let mut px = vec![0u8; (w * h * 4) as usize];
        converter.CopyPixels(std::ptr::null(), w * 4, &mut px)?;
        Ok((w, h, px))
    }
}

/// A `CF_DIB` or `CF_DIBV5` (24 or 32 bits) as a PNG.
fn dib_to_png(wic: &IWICImagingFactory, dib: &[u8]) -> windows::core::Result<Vec<u8>> {
    let bad = || windows::core::Error::empty();
    let u32_at =
        |o: usize| -> Option<u32> { Some(u32::from_le_bytes(dib.get(o..o + 4)?.try_into().ok()?)) };
    let header = u32_at(0).ok_or_else(bad)? as usize;
    let w = u32_at(4).ok_or_else(bad)? as i32;
    let h = u32_at(8).ok_or_else(bad)? as i32;
    let bits = u16::from_le_bytes(
        dib.get(14..16)
            .ok_or_else(bad)?
            .try_into()
            .map_err(|_| bad())?,
    );
    let compression = u32_at(16).ok_or_else(bad)?;
    let colors = u32_at(32).unwrap_or(0) as usize;
    if w <= 0 || h == 0 || !(bits == 24 || bits == 32) {
        return Err(bad());
    }
    // BI_BITFIELDS with a 40-byte header: three masks follow it.
    let masks = if compression == 3 && header == 40 {
        12
    } else {
        0
    };
    let start = header + masks + colors * 4;
    let (w, top_down, h) = (w as usize, h < 0, h.unsigned_abs() as usize);
    let stride = (w * bits as usize / 8).div_ceil(4) * 4;
    let rows = dib.get(start..start + stride * h).ok_or_else(bad)?;
    let mut px = vec![0u8; w * h * 4];
    let mut any_alpha = false;
    for y in 0..h {
        let src = if top_down { y } else { h - 1 - y };
        let row = &rows[src * stride..];
        for x in 0..w {
            let o = (y * w + x) * 4;
            if bits == 32 {
                px[o..o + 4].copy_from_slice(&row[x * 4..x * 4 + 4]);
                any_alpha |= row[x * 4 + 3] != 0;
            } else {
                px[o..o + 3].copy_from_slice(&row[x * 3..x * 3 + 3]);
                px[o + 3] = 255;
            }
        }
    }
    // A 32-bit DIB without alpha (most of them): opaque.
    if bits == 32 && !any_alpha {
        for p in px.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
    }
    encode_png(wic, w as u32, h as u32, &px)
}

fn encode_png(
    wic: &IWICImagingFactory,
    w: u32,
    h: u32,
    bgra: &[u8],
) -> windows::core::Result<Vec<u8>> {
    unsafe {
        let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true)?;
        let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let (mut frame, mut props) = (None, None);
        enc.CreateNewFrame(&mut frame, &mut props)?;
        let frame = frame.ok_or_else(windows::core::Error::empty)?;
        frame.Initialize(props.as_ref())?;
        frame.SetSize(w, h)?;
        let mut fmt = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut fmt)?;
        frame.WritePixels(h, w * 4, bgra)?;
        frame.Commit()?;
        enc.Commit()?;
        let mut len = 0u64;
        stream.Seek(0, STREAM_SEEK_CUR, Some(&mut len))?;
        let mem = GetHGlobalFromStream(&stream)?;
        let p = GlobalLock(mem) as *const u8;
        if p.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        let bytes = std::slice::from_raw_parts(p, len as usize).to_vec();
        let _ = GlobalUnlock(mem);
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cf_html_offsets_find_the_fragment() {
        let doc = cf_html("<b>Bold ✓</b>");
        assert_eq!(html_fragment(&doc).as_deref(), Some("<b>Bold ✓</b>"));
    }

    #[test]
    fn file_uris_round_trip() {
        let p = r"C:\Users\me\a b%.txt";
        assert_eq!(file_path(&file_uri(p)).as_deref(), Some(p));
    }

    /// Text, HTML, a picture and a custom type through the real clipboard,
    /// then the user's text put back.
    #[test]
    fn clipboard_round_trip() {
        // WIC is COM.
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );
        }
        let owner = HWND::default();
        let before = read_text(owner);
        let wic = crate::wic::factory().unwrap();
        let png = encode_png(&wic, 2, 1, &[0, 0, 255, 255, 255, 0, 0, 128]).unwrap();
        let item = ClipboardItem::new()
            .text("hello ✓")
            .html("<i>hi</i>")
            .image(png, ClipboardItem::PNG)
            .custom("application/x-prism-test", b"42".to_vec());
        write_item(owner, &item).unwrap();
        let back = read_item(owner);
        assert_eq!(back.as_text().as_deref(), Some("hello ✓"));
        assert_eq!(back.as_html().as_deref(), Some("<i>hi</i>"));
        assert_eq!(back.get("application/x-prism-test"), Some(&b"42"[..]));
        let image = back.as_image().unwrap();
        let (w, h, px) = decode_bgra(&wic, &image.bytes).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(&px[..4], &[0, 0, 255, 255]);
        // Through the DIB alone, as most apps put pictures.
        let dib = dib_v5(&wic, &image.bytes).unwrap();
        let again = dib_to_png(&wic, &dib).unwrap();
        assert_eq!(decode_bgra(&wic, &again).unwrap().2, px);
        match before {
            Some(t) => write_text(owner, &t).unwrap(),
            None => {
                let _ = with_clipboard(owner, || unsafe { EmptyClipboard() });
            }
        }
    }
}
