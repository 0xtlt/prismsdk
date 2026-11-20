//! WIC (Windows Imaging Component): decoding pictures (`images.rs`) and
//! writing snapshots as PNG.

use windows::Win32::Foundation::GENERIC_WRITE;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGRA,
    IWICImagingFactory, WICBitmapEncoderNoCache,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{PCWSTR, Result};

use crate::win::wide;

pub fn factory() -> Result<IWICImagingFactory> {
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
}

/// Writes `w` × `h` straight-alpha BGRA pixels to `path` as a PNG.
pub fn save_png(wic: &IWICImagingFactory, path: &str, w: u32, h: u32, bgra: &[u8]) -> Result<()> {
    unsafe {
        let stream = wic.CreateStream()?;
        let p = wide(path);
        stream.InitializeFromFilename(PCWSTR(p.as_ptr()), GENERIC_WRITE.0)?;
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
        enc.Commit()
    }
}

/// Premultiplied BGRA (what XAML renders) to straight alpha, over an
/// opaque `under` color where the window shows through (`None`: kept
/// transparent).
pub fn unpremultiply(px: &mut [u8], under: Option<[u8; 3]>) {
    for p in px.as_chunks_mut::<4>().0 {
        let a = p[3] as u32;
        match under {
            Some([r, g, b]) => {
                // Premultiplied over opaque: c + under × (1 − a).
                let k = 255 - a;
                p[0] = (p[0] as u32 + (b as u32 * k + 127) / 255).min(255) as u8;
                p[1] = (p[1] as u32 + (g as u32 * k + 127) / 255).min(255) as u8;
                p[2] = (p[2] as u32 + (r as u32 * k + 127) / 255).min(255) as u8;
                p[3] = 255;
            }
            None if a > 0 && a < 255 => {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
            None => {}
        }
    }
}
