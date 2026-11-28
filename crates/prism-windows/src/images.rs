//! Pictures: bytes from prism-native's loader (assets, files, `data:`
//! URLs, the network off the UI thread), decoded by WIC — PNG, JPEG, GIF
//! (first frame), BMP, TIFF, ICO, and WebP / HEIF where Windows has the
//! codecs — into device-independent bitmaps the painter uploads.

use std::sync::Mutex;

use prism_core::NodeId;
use prism_native::images::ImageSources;
use prism_native::picture_cache::{PictureCache, Show};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Imaging::{
    GUID_WICPixelFormat32bppPBGRA, IWICBitmap, IWICImagingFactory, WICBitmapCacheOnLoad,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::Result;

use crate::win::{WM_PRISM_IMAGE, log};

/// A decoded picture, or `None` when it could not be loaded or decoded
/// (kept, so a drawing does not decode it again at every paint).
type Entry = Option<Decoded>;

#[derive(Clone)]
struct Decoded {
    bitmap: IWICBitmap,
    size: (f32, f32),
    generation: u64,
}

/// Decoded pictures by source, kept while a node shows them, then in a
/// small pool bounded by bytes (#343, `prism_native::picture_cache`).
pub struct Images {
    pub sources: ImageSources,
    cache: PictureCache<Entry>,
    generation: u64,
}

/// Bytes loaded off the UI thread, waiting for it (`WM_PRISM_IMAGE`).
static LOADED: Mutex<Vec<(String, Option<Vec<u8>>)>> = Mutex::new(Vec::new());

impl Images {
    pub fn new(sources: ImageSources) -> Images {
        Images {
            sources,
            cache: PictureCache::default(),
            generation: 0,
        }
    }

    /// Starts loading `src` for a drawing or a poster unless it is known.
    /// Local sources decode now; remote ones on another thread, then
    /// `WM_PRISM_IMAGE` to `hwnd`.
    pub fn request(&mut self, wic: &IWICImagingFactory, src: &str, hwnd: HWND) {
        if src.is_empty() || !self.cache.start_load(src) {
            return;
        }
        self.load(wic, src, hwnd);
    }

    /// Picture `id` shows `src` now (its previous source is released).
    /// Starts loading it when it is not decoded: `Win::show_picture` keeps
    /// the previous image meanwhile.
    pub fn show(&mut self, wic: &IWICImagingFactory, id: NodeId, src: &str, hwnd: HWND) {
        if matches!(self.cache.show(id, src), Show::Load) {
            self.load(wic, src, hwnd);
        }
    }

    /// Picture or shape `id` is gone: its sources are released.
    pub fn forget(&mut self, id: NodeId) {
        self.cache.forget(id);
    }

    /// Shape `id` draws `items` now: their images stay decoded while it
    /// does, however large (#343), instead of being evicted, then decoded
    /// (or downloaded) again, by another picture or the next paint.
    pub fn hold(&mut self, id: NodeId, items: &[prism_core::shape::Item]) {
        let mut sources = Vec::new();
        crate::vector::sources(items, &mut sources);
        self.cache.hold(id, sources);
    }

    /// Whether `src` is loading off the UI thread.
    pub fn is_loading(&self, src: &str) -> bool {
        self.cache.is_loading(src)
    }

    fn load(&mut self, wic: &IWICImagingFactory, src: &str, hwnd: HWND) {
        let url = prism_core::picture_source(src).0;
        if prism_native::images::is_remote(url) {
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
        self.store(wic, src, bytes);
    }

    /// Decodes what the loader threads finished. Returns the sources now
    /// ready (their nodes are measured and drawn again).
    pub fn take_loaded(&mut self, wic: &IWICImagingFactory) -> Vec<String> {
        let done = std::mem::take(&mut *LOADED.lock().unwrap_or_else(|e| e.into_inner()));
        let mut out = Vec::new();
        for (src, bytes) in done {
            self.store(wic, &src, bytes);
            out.push(src);
        }
        out
    }

    fn store(&mut self, wic: &IWICImagingFactory, src: &str, bytes: Option<Vec<u8>>) {
        let entry = match bytes.map(|b| decode(wic, &b)) {
            Some(Ok((bitmap, size))) => {
                self.generation += 1;
                Some(Decoded {
                    bitmap,
                    size,
                    generation: self.generation,
                })
            }
            Some(Err(e)) => {
                log!(
                    "could not decode image {:?}: {e}",
                    prism_core::picture_source(src).0
                );
                None
            }
            None => {
                log!(
                    "could not load image {:?}",
                    prism_core::picture_source(src).0
                );
                None
            }
        };
        let bytes = entry.as_ref().map_or(0, |d| {
            prism_native::picture_cache::decoded_bytes(d.size.0 as f64, d.size.1 as f64)
        });
        self.cache.loaded(src, Some((entry, bytes)));
    }

    fn decoded(&self, src: &str) -> Option<&Decoded> {
        self.cache.get(src)?.as_ref()
    }

    /// The decoded bitmap of `src` and its generation (bumped per decode),
    /// made the most recently used: an image drawn now goes last.
    pub fn bitmap(&mut self, src: &str) -> Option<(u64, IWICBitmap)> {
        let d = self.cache.touch(src)?.as_ref()?;
        Some((d.generation, d.bitmap.clone()))
    }

    /// Natural size in view units (one pixel per DIP, like CSS).
    pub fn size_of(&self, src: &str) -> Option<(f32, f32)> {
        self.decoded(src).map(|d| d.size)
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
