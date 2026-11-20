//! Pictures: bytes from prism-native's loader (assets, files, `data:`
//! URLs, the network off the UI thread), decoded by WIC — PNG, JPEG, GIF
//! (first frame), BMP, TIFF, ICO, and WebP / HEIF where Windows has the
//! codecs — into device-independent bitmaps the painter uploads.

use std::collections::HashMap;
use std::sync::Mutex;

use prism_native::images::ImageSources;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Imaging::{
    GUID_WICPixelFormat32bppPBGRA, IWICBitmap, IWICImagingFactory, WICBitmapCacheOnLoad,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::Result;

use crate::win::{WM_PRISM_IMAGE, log};

enum Entry {
    Loading,
    Ready {
        bitmap: IWICBitmap,
        size: (f32, f32),
        generation: u64,
    },
    Failed,
}

/// Decoded pictures by source.
pub struct Images {
    pub sources: ImageSources,
    entries: HashMap<String, Entry>,
    generation: u64,
}

/// Bytes loaded off the UI thread, waiting for it (`WM_PRISM_IMAGE`).
static LOADED: Mutex<Vec<(String, Option<Vec<u8>>)>> = Mutex::new(Vec::new());

impl Images {
    pub fn new(sources: ImageSources) -> Images {
        Images {
            sources,
            entries: HashMap::new(),
            generation: 0,
        }
    }

    /// Starts loading `src` unless it is known. Local sources decode now;
    /// remote ones on another thread, then `WM_PRISM_IMAGE` to `hwnd`.
    pub fn request(&mut self, wic: &IWICImagingFactory, src: &str, hwnd: HWND) {
        if src.is_empty() || self.entries.contains_key(src) {
            return;
        }
        let url = prism_core::picture_source(src).0;
        if prism_native::images::is_remote(url) {
            self.entries.insert(src.to_string(), Entry::Loading);
            let (src, sources) = (src.to_string(), self.sources.clone());
            let target = hwnd.0 as isize;
            std::thread::spawn(move || {
                let bytes = prism_native::images::load(&src, &sources);
                LOADED
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((src, bytes));
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(target as *mut _)),
                        WM_PRISM_IMAGE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            });
            return;
        }
        let bytes = prism_native::images::load(src, &self.sources);
        self.store(wic, src.to_string(), bytes);
    }

    /// Decodes what the loader threads finished. Returns the sources now
    /// ready (their nodes are measured and drawn again).
    pub fn take_loaded(&mut self, wic: &IWICImagingFactory) -> Vec<String> {
        let done = std::mem::take(&mut *LOADED.lock().unwrap_or_else(|e| e.into_inner()));
        let mut out = Vec::new();
        for (src, bytes) in done {
            self.store(wic, src.clone(), bytes);
            out.push(src);
        }
        out
    }

    fn store(&mut self, wic: &IWICImagingFactory, src: String, bytes: Option<Vec<u8>>) {
        let entry = match bytes.map(|b| decode(wic, &b)) {
            Some(Ok((bitmap, size))) => {
                self.generation += 1;
                Entry::Ready {
                    bitmap,
                    size,
                    generation: self.generation,
                }
            }
            Some(Err(e)) => {
                log!(
                    "could not decode image {:?}: {e}",
                    prism_core::picture_source(&src).0
                );
                Entry::Failed
            }
            None => {
                log!(
                    "could not load image {:?}",
                    prism_core::picture_source(&src).0
                );
                Entry::Failed
            }
        };
        self.entries.insert(src, entry);
    }

    /// The decoded bitmap of `src` and its generation (bumped per decode).
    pub fn bitmap(&self, src: &str) -> Option<(u64, IWICBitmap)> {
        match self.entries.get(src)? {
            Entry::Ready {
                bitmap, generation, ..
            } => Some((*generation, bitmap.clone())),
            _ => None,
        }
    }

    /// Natural size in view units (one pixel per DIP, like CSS).
    pub fn size_of(&self, src: &str) -> Option<(f32, f32)> {
        match self.entries.get(src)? {
            Entry::Ready { size, .. } => Some(*size),
            _ => None,
        }
    }
}

/// The first frame of an encoded image, as premultiplied BGRA.
fn decode(wic: &IWICImagingFactory, bytes: &[u8]) -> Result<(IWICBitmap, (f32, f32))> {
    unsafe {
        let stream = wic.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let decoder =
            wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let converter = wic.CreateFormatConverter()?;
        converter.Initialize(
            &frame,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.,
            WICBitmapPaletteTypeMedianCut,
        )?;
        // A copy: the stream borrows `bytes`.
        let bitmap = wic.CreateBitmapFromSource(&converter, WICBitmapCacheOnLoad)?;
        let (mut w, mut h) = (0u32, 0u32);
        bitmap.GetSize(&mut w, &mut h)?;
        Ok((bitmap, (w as f32, h as f32)))
    }
}
