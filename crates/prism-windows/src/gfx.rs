//! Direct2D, DirectWrite and WIC: the factories, the window's surface (a
//! DXGI flip-model swap chain on a D3D11 device, or GDI when there is no
//! GPU presentation), offscreen rendering into a WIC bitmap and PNG output.
//!
//! Every target is drawn by the same function (`paint::draw`), in view
//! units (DIPs) at the window's DPI.

use std::cell::RefCell;

use windows::Win32::Foundation::{GENERIC_WRITE, HMODULE, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_U, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
    D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_FACTORY_OPTIONS,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1CreateFactory,
    ID2D1Bitmap1, ID2D1DCRenderTarget, ID2D1Device, ID2D1DeviceContext, ID2D1Factory1, ID2D1Image,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWriteCreateFactory, IDWriteFactory2, IDWriteFontCollection,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter, IDXGIDevice,
    IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SRCCOPY, SelectObject,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGRA,
    GUID_WICPixelFormat32bppPBGRA, IWICBitmap, IWICBitmapSource, IWICImagingFactory,
    WICBitmapCacheOnLoad, WICBitmapEncoderNoCache, WICBitmapLockWrite, WICRect,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{BOOL, Interface, PCWSTR, Result};
use windows_numerics::Matrix3x2;

use crate::scene::Rect;
use crate::win::{log, wide};

/// The UI fonts, from what the system has.
#[derive(Clone, Debug)]
pub struct Fonts {
    /// Body text: Segoe UI Variable Text (Windows 11), else Segoe UI.
    pub text: String,
    /// Large text (≥ 20 DIPs): Segoe UI Variable Display, else `text`.
    pub display: String,
    /// Icons: Segoe Fluent Icons (Windows 11), else Segoe MDL2 Assets.
    pub icons: String,
}

pub struct Gfx {
    pub d2d: ID2D1Factory1,
    pub dw: IDWriteFactory2,
    pub wic: IWICImagingFactory,
    pub fonts: Fonts,
    pub system_fonts: IDWriteFontCollection,
}

impl Gfx {
    pub fn new() -> Result<Gfx> {
        unsafe {
            let options = D2D1_FACTORY_OPTIONS::default();
            let d2d: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&options))?;
            let dw: IDWriteFactory2 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let mut fonts = None;
            dw.GetSystemFontCollection(&mut fonts, false)?;
            let system_fonts = fonts.ok_or_else(windows::core::Error::empty)?;
            let has = |name: &str| {
                let (mut index, mut exists) = (0u32, BOOL(0));
                let w = wide(name);
                system_fonts
                    .FindFamilyName(PCWSTR(w.as_ptr()), &mut index, &mut exists)
                    .is_ok()
                    && exists.as_bool()
            };
            let first = |names: &[&str], fallback: &str| {
                names
                    .iter()
                    .find(|n| has(n))
                    .map_or(fallback.to_string(), |n| n.to_string())
            };
            let text = first(&["Segoe UI Variable Text", "Segoe UI Variable"], "Segoe UI");
            let fonts = Fonts {
                display: first(&["Segoe UI Variable Display"], &text),
                icons: first(
                    &["Segoe Fluent Icons", "Segoe MDL2 Assets"],
                    "Segoe UI Symbol",
                ),
                text,
            };
            Ok(Gfx {
                d2d,
                dw,
                wic,
                fonts,
                system_fonts,
            })
        }
    }

    /// Draws into a new WIC bitmap (`w` × `h` pixels at `dpi`), without a
    /// window or a GPU: snapshots, drag previews.
    pub fn offscreen(
        &self,
        w: u32,
        h: u32,
        dpi: f32,
        draw: impl FnOnce(&ID2D1DeviceContext) -> Result<()>,
    ) -> Result<IWICBitmap> {
        unsafe {
            let bmp = self.wic.CreateBitmap(
                w.max(1),
                h.max(1),
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapCacheOnLoad,
            )?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: dpi,
                dpiY: dpi,
                ..Default::default()
            };
            let rt = self.d2d.CreateWicBitmapRenderTarget(&bmp, &props)?;
            let dc: ID2D1DeviceContext = rt.cast()?;
            dc.BeginDraw();
            let drawn = draw(&dc);
            let ended = dc.EndDraw(None, None);
            drawn.and(ended)?;
            Ok(bmp)
        }
    }

    /// Copies 32-bit BGRA pixels (`cw` × `ch`, rows packed) into `bmp` at
    /// (`x`, `y`), clipped to it and to where `keep` (window pixels) says,
    /// made opaque.
    pub fn blit(
        &self,
        bmp: &IWICBitmap,
        (x, y): (i32, i32),
        (cw, ch): (i32, i32),
        px: &[u8],
        keep: &dyn Fn(i32, i32) -> bool,
    ) -> Result<()> {
        unsafe {
            let (mut w, mut h) = (0u32, 0u32);
            bmp.GetSize(&mut w, &mut h)?;
            let lock = bmp.Lock(
                &WICRect {
                    X: 0,
                    Y: 0,
                    Width: w as i32,
                    Height: h as i32,
                },
                WICBitmapLockWrite.0 as u32,
            )?;
            let stride = lock.GetStride()? as usize;
            let (mut size, mut data) = (0u32, std::ptr::null_mut());
            lock.GetDataPointer(&mut size, &mut data)?;
            let dst = std::slice::from_raw_parts_mut(data, size as usize);
            for row in 0..ch {
                let ty = y + row;
                if ty < 0 || ty >= h as i32 {
                    continue;
                }
                for col in 0..cw {
                    let tx = x + col;
                    if tx < 0 || tx >= w as i32 || !keep(tx, ty) {
                        continue;
                    }
                    let s = ((row * cw + col) * 4) as usize;
                    let d = ty as usize * stride + tx as usize * 4;
                    dst[d..d + 3].copy_from_slice(&px[s..s + 3]);
                    dst[d + 3] = 255;
                }
            }
        }
        Ok(())
    }

    /// Writes a bitmap as a PNG file.
    pub fn save_png(&self, path: &str, src: &IWICBitmapSource) -> Result<()> {
        unsafe {
            let (mut w, mut h) = (0u32, 0u32);
            src.GetSize(&mut w, &mut h)?;
            let stream = self.wic.CreateStream()?;
            let p = wide(path);
            stream.InitializeFromFilename(PCWSTR(p.as_ptr()), GENERIC_WRITE.0)?;
            let enc = self
                .wic
                .CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
            enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
            let (mut frame, mut props) = (None, None);
            enc.CreateNewFrame(&mut frame, &mut props)?;
            let frame = frame.ok_or_else(windows::core::Error::empty)?;
            frame.Initialize(props.as_ref())?;
            frame.SetSize(w, h)?;
            let mut fmt = GUID_WICPixelFormat32bppBGRA;
            frame.SetPixelFormat(&mut fmt)?;
            // Premultiplied BGRA from Direct2D → straight BGRA for the PNG.
            let converter = self.wic.CreateFormatConverter()?;
            converter.Initialize(
                src,
                &GUID_WICPixelFormat32bppBGRA,
                windows::Win32::Graphics::Imaging::WICBitmapDitherTypeNone,
                None,
                0.,
                windows::Win32::Graphics::Imaging::WICBitmapPaletteTypeCustom,
            )?;
            frame.WriteSource(&converter, std::ptr::null())?;
            frame.Commit()?;
            enc.Commit()
        }
    }
}

/// A D3D11 device and its Direct2D device: hardware, else WARP (software
/// rasterizer on the GPU API, always there).
struct Device {
    _d3d: ID3D11Device,
    d2d: ID2D1Device,
    dxgi_factory: IDXGIFactory2,
    warp: bool,
}

fn create_device(gfx: &Gfx) -> Result<Device> {
    let mut last = None;
    for (driver, warp) in [
        (D3D_DRIVER_TYPE_HARDWARE, false),
        (D3D_DRIVER_TYPE_WARP, true),
    ] {
        match try_device(gfx, driver, warp) {
            Ok(d) => return Ok(d),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(windows::core::Error::empty))
}

fn try_device(gfx: &Gfx, driver: D3D_DRIVER_TYPE, warp: bool) -> Result<Device> {
    unsafe {
        let mut d3d: Option<ID3D11Device> = None;
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut d3d),
            None,
            None,
        )?;
        let d3d = d3d.ok_or_else(windows::core::Error::empty)?;
        let dxgi: IDXGIDevice = d3d.cast()?;
        let d2d = gfx.d2d.CreateDevice(&dxgi)?;
        let adapter = dxgi.GetAdapter()?;
        let dxgi_factory: IDXGIFactory2 = adapter.GetParent()?;
        Ok(Device {
            _d3d: d3d,
            d2d,
            dxgi_factory,
            warp,
        })
    }
}

struct Swap {
    _device: Device,
    dc: ID2D1DeviceContext,
    swap: IDXGISwapChain1,
    /// What the window shows, kept between frames: a paint redraws the
    /// invalidated part into it, then the whole of it goes to the back
    /// buffer (flip-model back buffers do not keep the previous frame).
    frame: Option<ID2D1Bitmap1>,
    size: (u32, u32),
}

enum Mode {
    /// Not tried yet.
    Unknown,
    Swap(Box<Swap>),
    /// No GPU presentation (no desktop compositor, a remote session…):
    /// Direct2D's software rasterizer into a GDI bitmap, copied to the
    /// window.
    Gdi(Option<GdiFrame>),
}

/// The window's drawing surface.
pub struct Surface {
    hwnd: HWND,
    mode: Mode,
    /// Bumped when the device changes: device-dependent caches (bitmaps,
    /// shadows) are dropped.
    pub generation: u64,
}

impl Surface {
    pub fn new(hwnd: HWND) -> Surface {
        let mode = if crate::win::env("PRISM_WINDOWS_GDI").is_some() {
            Mode::Gdi(None)
        } else {
            Mode::Unknown
        };
        Surface {
            hwnd,
            mode,
            generation: 1,
        }
    }

    /// Whether a frame went to the window yet.
    pub fn presented(&self) -> bool {
        !matches!(self.mode, Mode::Unknown)
    }

    /// What presents the frames, for logs and the smoke test.
    pub fn describe(&self) -> &'static str {
        match &self.mode {
            Mode::Unknown => "none yet",
            Mode::Swap(s) if s._device.warp => "DXGI swap chain (WARP)",
            Mode::Swap(_) => "DXGI swap chain (GPU)",
            Mode::Gdi(_) => "GDI (Direct2D software rendering)",
        }
    }

    fn make_swap(&self, gfx: &Gfx, size: (u32, u32)) -> Result<Swap> {
        unsafe {
            let device = create_device(gfx)?;
            let dc = device
                .d2d
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: size.0.max(1),
                Height: size.1.max(1),
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };
            let swap = device.dxgi_factory.CreateSwapChainForHwnd(
                &device._d3d,
                self.hwnd,
                &desc,
                None,
                None::<&windows::Win32::Graphics::Dxgi::IDXGIOutput>,
            )?;
            Ok(Swap {
                _device: device,
                dc,
                swap,
                frame: None,
                size,
            })
        }
    }

    /// Repaints `dirty` (client pixels; `None`: everything) with `draw`
    /// (given the clip in view units), and presents the frame. `hdc` is
    /// the paint DC (`BeginPaint`), used without GPU presentation.
    pub fn paint(
        &mut self,
        gfx: &Gfx,
        hdc: HDC,
        size: (u32, u32),
        dpi: f32,
        dirty: Option<RECT>,
        draw: &mut dyn FnMut(&ID2D1DeviceContext, Option<Rect>) -> Result<()>,
    ) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        if matches!(self.mode, Mode::Unknown) {
            self.mode = match self.make_swap(gfx, size) {
                Ok(s) => {
                    self.generation += 1;
                    Mode::Swap(Box::new(s))
                }
                Err(e) => {
                    log!("no swap chain ({e}): drawing through GDI");
                    Mode::Gdi(None)
                }
            };
        }
        let scale = dpi / 96.;
        let clip = |r: RECT| {
            Rect::new(
                r.left as f32 / scale,
                r.top as f32 / scale,
                (r.right - r.left) as f32 / scale,
                (r.bottom - r.top) as f32 / scale,
            )
        };
        let result = match &mut self.mode {
            Mode::Swap(s) => present(s, size, dpi, dirty.map(clip), draw),
            Mode::Gdi(frame) => paint_gdi(gfx, frame, hdc, size, dpi, dirty.map(clip), draw),
            Mode::Unknown => Ok(()),
        };
        if let Err(e) = result {
            // A lost device (driver update, GPU reset), a session without
            // presentation: start over, then fall back to GDI.
            let gdi = matches!(self.mode, Mode::Gdi(_));
            log!("paint failed ({e}); recreating the surface");
            self.mode = if gdi { Mode::Gdi(None) } else { Mode::Unknown };
            self.generation += 1;
            if !gdi && e.code() != windows::Win32::Foundation::D2DERR_RECREATE_TARGET {
                self.mode = Mode::Gdi(None);
            }
            crate::win::invalidate(self.hwnd, None, scale);
        }
    }
}

fn present(
    s: &mut Swap,
    size: (u32, u32),
    dpi: f32,
    mut clip: Option<Rect>,
    draw: &mut dyn FnMut(&ID2D1DeviceContext, Option<Rect>) -> Result<()>,
) -> Result<()> {
    unsafe {
        if s.size != size {
            s.dc.SetTarget(None::<&ID2D1Image>);
            s.frame = None;
            s.swap.ResizeBuffers(
                0,
                size.0,
                size.1,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
            s.size = size;
        }
        let props = |options| D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: dpi,
            dpiY: dpi,
            bitmapOptions: options,
            ..Default::default()
        };
        let frame = match &s.frame {
            Some(f) => f.clone(),
            None => {
                let f = s.dc.CreateBitmap(
                    D2D_SIZE_U {
                        width: size.0,
                        height: size.1,
                    },
                    None,
                    0,
                    &props(D2D1_BITMAP_OPTIONS_TARGET),
                )?;
                s.frame = Some(f.clone());
                // A new frame has no content yet: draw all of it.
                clip = None;
                f
            }
        };
        s.dc.SetDpi(dpi, dpi);
        s.dc.SetTarget(&frame);
        s.dc.BeginDraw();
        s.dc.SetTransform(&Matrix3x2::identity());
        let drawn = draw(&s.dc, clip);
        s.dc.EndDraw(None, None)?;
        drawn?;
        // The whole frame to the back buffer, then present.
        let surface: IDXGISurface = s.swap.GetBuffer(0)?;
        let back = s.dc.CreateBitmapFromDxgiSurface(
            &surface,
            Some(&props(
                D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            )),
        )?;
        s.dc.SetTarget(&back);
        s.dc.BeginDraw();
        s.dc.SetTransform(&Matrix3x2::identity());
        s.dc.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
        s.dc.DrawImage(
            &frame,
            None,
            None,
            windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE(
                D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR.0,
            ),
            windows::Win32::Graphics::Direct2D::Common::D2D1_COMPOSITE_MODE_SOURCE_COPY,
        );
        s.dc.EndDraw(None, None)?;
        s.dc.SetTarget(None::<&ID2D1Image>);
        s.swap.Present(1, DXGI_PRESENT(0)).ok()
    }
}

/// The window's content kept in a GDI bitmap, drawn by Direct2D's
/// software rasterizer: what paints when there is no GPU presentation
/// (no desktop compositor). Painted partly, copied to the window.
pub struct GdiFrame {
    rt: ID2D1DCRenderTarget,
    mem: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    size: (u32, u32),
}

impl Drop for GdiFrame {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.mem, self.old);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.mem);
        }
    }
}

fn gdi_frame(gfx: &Gfx, size: (u32, u32), dpi: f32) -> Result<GdiFrame> {
    unsafe {
        let mem = CreateCompatibleDC(None);
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size.0 as i32,
                biHeight: -(size.1 as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = match CreateDIBSection(Some(mem), &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(e) => {
                let _ = DeleteDC(mem);
                return Err(e);
            }
        };
        let old = SelectObject(mem, HGDIOBJ(bitmap.0));
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: dpi,
            dpiY: dpi,
            ..Default::default()
        };
        let frame = GdiFrame {
            rt: gfx.d2d.CreateDCRenderTarget(&props)?,
            mem,
            bitmap,
            old,
            size,
        };
        frame.rt.BindDC(
            mem,
            &RECT {
                left: 0,
                top: 0,
                right: size.0 as i32,
                bottom: size.1 as i32,
            },
        )?;
        Ok(frame)
    }
}

fn paint_gdi(
    gfx: &Gfx,
    frame: &mut Option<GdiFrame>,
    hdc: HDC,
    size: (u32, u32),
    dpi: f32,
    mut clip: Option<Rect>,
    draw: &mut dyn FnMut(&ID2D1DeviceContext, Option<Rect>) -> Result<()>,
) -> Result<()> {
    unsafe {
        if frame.as_ref().is_none_or(|f| f.size != size) {
            *frame = Some(gdi_frame(gfx, size, dpi)?);
            // A new bitmap: all of it.
            clip = None;
        }
        let f = frame.as_ref().expect("frame");
        f.rt.SetDpi(dpi, dpi);
        let dc: ID2D1DeviceContext = f.rt.cast()?;
        dc.BeginDraw();
        dc.SetTransform(&Matrix3x2::identity());
        let drawn = draw(&dc, clip);
        let ended = dc.EndDraw(None, None);
        drawn.and(ended)?;
        if !hdc.is_invalid() {
            // What changed, to the window (nothing shows without a desktop).
            let scale = dpi / 96.;
            let r = clip.map_or(
                RECT {
                    left: 0,
                    top: 0,
                    right: size.0 as i32,
                    bottom: size.1 as i32,
                },
                |c| crate::win::px_rect(c, scale),
            );
            let _ = BitBlt(
                hdc,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                Some(f.mem),
                r.left,
                r.top,
                SRCCOPY,
            );
        }
        Ok(())
    }
}

/// A copy of what is drawn under `r` (pixels) on the current target, for
/// backdrop blur. `None` when the target cannot be read (GDI, WIC).
pub fn copy_backdrop(dc: &ID2D1DeviceContext, r: RECT, dpi: f32) -> Option<ID2D1Bitmap1> {
    unsafe {
        let target: ID2D1Bitmap1 = dc.GetTarget().ok()?.cast().ok()?;
        let size = target.GetPixelSize();
        let x0 = r.left.clamp(0, size.width as i32) as u32;
        let y0 = r.top.clamp(0, size.height as i32) as u32;
        let x1 = r.right.clamp(0, size.width as i32) as u32;
        let y1 = r.bottom.clamp(0, size.height as i32) as u32;
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let copy = dc
            .CreateBitmap(
                D2D_SIZE_U {
                    width: x1 - x0,
                    height: y1 - y0,
                },
                None,
                0,
                &D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: dpi,
                    dpiY: dpi,
                    ..Default::default()
                },
            )
            .ok()?;
        // Pending drawing must reach the target first.
        dc.Flush(None, None).ok()?;
        copy.CopyFromBitmap(
            None,
            &target,
            Some(&D2D_RECT_U {
                left: x0,
                top: y0,
                right: x1,
                bottom: y1,
            }),
        )
        .ok()?;
        Some(copy)
    }
}

pub fn color_f(c: prism_core::Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

thread_local! {
    /// The factories, made once per UI thread.
    static GFX: RefCell<Option<std::rc::Rc<Gfx>>> = const { RefCell::new(None) };
}

/// The thread's factories.
pub fn shared() -> std::rc::Rc<Gfx> {
    GFX.with(|g| {
        g.borrow_mut()
            .get_or_insert_with(|| std::rc::Rc::new(Gfx::new().expect("Direct2D factories")))
            .clone()
    })
}
