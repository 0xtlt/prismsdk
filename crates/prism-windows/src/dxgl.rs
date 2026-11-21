//! 3D frames from OpenGL straight to the screen: a XAML `SwapChainPanel`
//! shows a DXGI swap chain, and the GL context draws into Direct3D 11
//! textures through `WGL_NV_DX_interop2` (AMD, NVIDIA and Intel drivers
//! have it). No copy through the CPU: the frame stays on the GPU.
//!
//! Each frame, prism-native's renderer draws into its framebuffer as
//! before; a GL blit then copies it, flipped (GL's rows go up, Direct3D's
//! down), into a texture Direct3D shares, and Direct3D copies that into
//! the swap chain's back buffer and presents it. The swap chain is the
//! panel's size in pixels, scaled back to DIPs for XAML.
//!
//! Without the extension (or Direct3D 11), `scene3d.rs` reads the frame
//! back into a bitmap instead.

use std::ffi::{CStr, c_void};

use glow::HasContext;
use windows::Win32::Foundation::{HANDLE, HMODULE};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11CreateDevice, ID3D11Device,
    ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_MATRIX_3X2_F, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    IDXGIDevice, IDXGIFactory2, IDXGISwapChain1, IDXGISwapChain2,
};
use windows::Win32::Graphics::Gdi::HDC;
use windows_core::{Interface, Result};

use crate::winui::xaml::Controls::SwapChainPanel;

/// `ISwapChainPanelNative` (`microsoft.ui.xaml.media.dxinterop.h`): not in
/// the metadata.
#[allow(non_snake_case)]
mod native {
    use std::ffi::c_void;
    use windows_core::{IUnknown, IUnknown_Vtbl};

    #[windows_core::interface("63aad0b8-7c24-40ff-85a8-640d944cc325")]
    pub unsafe trait ISwapChainPanelNative: IUnknown {
        fn SetSwapChain(&self, swap_chain: *mut c_void) -> windows_core::HRESULT;
    }

    /// The panel shows `swap_chain` (an `IDXGISwapChain1`).
    pub fn set_swap_chain(
        panel: &ISwapChainPanelNative,
        swap_chain: *mut c_void,
    ) -> windows_core::Result<()> {
        unsafe { panel.SetSwapChain(swap_chain).ok() }
    }
}
use native::{ISwapChainPanelNative, set_swap_chain};

const WGL_ACCESS_READ_WRITE_NV: u32 = 0x0001;
const D3D11_RESOURCE_MISC_SHARED: u32 = 0x2;

/// How this driver shares a texture: as a GL texture or renderbuffer, in
/// which format, shared or not (drivers differ: probed once).
#[derive(Clone, Copy, Debug)]
struct Kind {
    texture: bool,
    format: DXGI_FORMAT,
    misc: u32,
}

const KINDS: [Kind; 6] = [
    Kind {
        texture: true,
        format: DXGI_FORMAT_R8G8B8A8_UNORM,
        misc: 0,
    },
    Kind {
        texture: true,
        format: DXGI_FORMAT_B8G8R8A8_UNORM,
        misc: 0,
    },
    Kind {
        texture: true,
        format: DXGI_FORMAT_R8G8B8A8_UNORM,
        misc: D3D11_RESOURCE_MISC_SHARED,
    },
    Kind {
        texture: true,
        format: DXGI_FORMAT_B8G8R8A8_UNORM,
        misc: D3D11_RESOURCE_MISC_SHARED,
    },
    Kind {
        texture: false,
        format: DXGI_FORMAT_R8G8B8A8_UNORM,
        misc: 0,
    },
    Kind {
        texture: false,
        format: DXGI_FORMAT_B8G8R8A8_UNORM,
        misc: 0,
    },
];

/// GL's name for a shared texture.
#[derive(Clone, Copy)]
enum Name {
    Texture(glow::Texture),
    Renderbuffer(glow::Renderbuffer),
}

type OpenDevice = unsafe extern "system" fn(*mut c_void) -> HANDLE;
type CloseDevice = unsafe extern "system" fn(HANDLE) -> i32;
type RegisterObject = unsafe extern "system" fn(HANDLE, *mut c_void, u32, u32, u32) -> HANDLE;
type UnregisterObject = unsafe extern "system" fn(HANDLE, HANDLE) -> i32;
type LockObjects = unsafe extern "system" fn(HANDLE, i32, *mut HANDLE) -> i32;
type ExtensionsString = unsafe extern "system" fn(HDC) -> *const i8;

/// The Direct3D 11 device GL shares textures with.
pub struct Interop {
    kind: Kind,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    factory: IDXGIFactory2,
    handle: HANDLE,
    close: CloseDevice,
    register: RegisterObject,
    unregister: UnregisterObject,
    lock: LockObjects,
    unlock: LockObjects,
}

impl Interop {
    /// The device, if the GL context current on `hdc` can share with it
    /// (`address` finds WGL functions).
    ///
    /// # Safety
    /// The GL context must be current on this thread.
    pub unsafe fn new(
        hdc: HDC,
        address: impl Fn(&CStr) -> *const c_void,
        gl: &glow::Context,
    ) -> std::result::Result<Interop, String> {
        unsafe {
            let ext = address(c"wglGetExtensionsStringARB");
            if ext.is_null() {
                return Err("no WGL_ARB_extensions_string".into());
            }
            let ext: ExtensionsString = std::mem::transmute(ext);
            let list = ext(hdc);
            let list = if list.is_null() {
                String::new()
            } else {
                CStr::from_ptr(list).to_string_lossy().into_owned()
            };
            if !list.split(' ').any(|e| e == "WGL_NV_DX_interop2") {
                return Err("the driver has no WGL_NV_DX_interop2".into());
            }
            let f = |name: &CStr| {
                let p = address(name);
                (!p.is_null()).then_some(p)
            };
            let (
                Some(open),
                Some(close),
                Some(register),
                Some(unregister),
                Some(lock),
                Some(unlock),
            ) = (
                f(c"wglDXOpenDeviceNV"),
                f(c"wglDXCloseDeviceNV"),
                f(c"wglDXRegisterObjectNV"),
                f(c"wglDXUnregisterObjectNV"),
                f(c"wglDXLockObjectsNV"),
                f(c"wglDXUnlockObjectsNV"),
            )
            else {
                return Err("WGL_NV_DX_interop2 without its functions".into());
            };
            let (mut device, mut context) = (None, None);
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|e| format!("Direct3D 11: {e}"))?;
            let (Some(device), Some(context)) = (device, context) else {
                return Err("Direct3D 11: no device".into());
            };
            let factory: IDXGIFactory2 = device
                .cast::<IDXGIDevice>()
                .and_then(|d| d.GetAdapter())
                .and_then(|a| a.GetParent())
                .map_err(|e| format!("DXGI: {e}"))?;
            let open: OpenDevice = std::mem::transmute(open);
            let handle = open(device.as_raw());
            if handle.is_invalid() || handle.0.is_null() {
                return Err("wglDXOpenDeviceNV failed".into());
            }
            let mut interop = Interop {
                kind: KINDS[0],
                device,
                context,
                factory,
                handle,
                close: std::mem::transmute::<*const c_void, CloseDevice>(close),
                register: std::mem::transmute::<*const c_void, RegisterObject>(register),
                unregister: std::mem::transmute::<*const c_void, UnregisterObject>(unregister),
                lock: std::mem::transmute::<*const c_void, LockObjects>(lock),
                unlock: std::mem::transmute::<*const c_void, LockObjects>(unlock),
            };
            // What this driver can share: a small texture of each kind,
            // registered, locked and let go.
            let mut why = String::new();
            for kind in KINDS {
                interop.kind = kind;
                match share(&interop, gl, (16, 16)) {
                    Ok(mut sh) => {
                        let ok = sh.lock(&interop);
                        if ok {
                            sh.unlock(&interop);
                        }
                        sh.delete(&interop, gl);
                        if ok {
                            return Ok(interop);
                        }
                        why = format!("{kind:?}: lock failed");
                    }
                    Err(e) => why = format!("{kind:?}: {e}"),
                }
            }
            Err(format!("no texture to share ({why})"))
        }
    }

    /// How textures are shared (for the log).
    pub fn describe(&self) -> String {
        format!(
            "{} {}",
            if self.kind.texture {
                "texture"
            } else {
                "renderbuffer"
            },
            if self.kind.format == DXGI_FORMAT_B8G8R8A8_UNORM {
                "BGRA"
            } else {
                "RGBA"
            }
        )
    }
}

impl Drop for Interop {
    fn drop(&mut self) {
        unsafe {
            (self.close)(self.handle);
        }
    }
}

/// A panel's swap chain and the texture GL draws into for it.
pub struct Surface {
    swap: IDXGISwapChain1,
    shared: Option<Shared>,
    /// Pixels, and pixels per DIP.
    size: (i32, i32),
    scale: f32,
}

/// The texture both APIs see, and GL's framebuffer on it.
struct Shared {
    texture: ID3D11Texture2D,
    object: HANDLE,
    name: Name,
    fb: glow::Framebuffer,
    /// The texture is on the framebuffer (done while locked).
    attached: bool,
}

impl Shared {
    unsafe fn lock(&mut self, interop: &Interop) -> bool {
        unsafe { (interop.lock)(interop.handle, 1, &mut self.object) != 0 }
    }

    unsafe fn unlock(&mut self, interop: &Interop) {
        unsafe {
            (interop.unlock)(interop.handle, 1, &mut self.object);
        }
    }

    unsafe fn delete(self, interop: &Interop, gl: &glow::Context) {
        unsafe {
            (interop.unregister)(interop.handle, self.object);
            gl.delete_framebuffer(self.fb);
            match self.name {
                Name::Texture(t) => gl.delete_texture(t),
                Name::Renderbuffer(r) => gl.delete_renderbuffer(r),
            }
        }
    }
}

impl Surface {
    /// A swap chain of `size` pixels (`scale` per DIP) shown by `panel`.
    pub fn new(
        interop: &Interop,
        panel: &SwapChainPanel,
        size: (i32, i32),
        scale: f32,
    ) -> Result<Surface> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: size.0.max(1) as u32,
            Height: size.1.max(1) as u32,
            Format: interop.kind.format,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        let swap = unsafe {
            interop
                .factory
                .CreateSwapChainForComposition(&interop.device, &desc, None)?
        };
        let native: ISwapChainPanelNative = panel.cast()?;
        set_swap_chain(&native, swap.as_raw())?;
        let s = Surface {
            swap,
            shared: None,
            size,
            scale,
        };
        s.transform()?;
        Ok(s)
    }

    /// The swap chain's pixels shown at the panel's size in DIPs.
    fn transform(&self) -> Result<()> {
        let k = 1. / self.scale.max(0.01);
        let m = DXGI_MATRIX_3X2_F {
            _11: k,
            _22: k,
            ..Default::default()
        };
        unsafe { self.swap.cast::<IDXGISwapChain2>()?.SetMatrixTransform(&m) }
    }

    /// Copies GL framebuffer `from` (`size` pixels, bottom row first) to
    /// the screen.
    ///
    /// # Safety
    /// The GL context that shares `interop` must be current.
    pub unsafe fn present(
        &mut self,
        interop: &Interop,
        gl: &glow::Context,
        from: glow::Framebuffer,
        size: (i32, i32),
        scale: f32,
    ) -> Result<()> {
        unsafe {
            if size != self.size || scale != self.scale {
                self.release(interop, gl);
                self.swap.ResizeBuffers(
                    0,
                    size.0.max(1) as u32,
                    size.1.max(1) as u32,
                    interop.kind.format,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )?;
                self.size = size;
                self.scale = scale;
                self.transform()?;
            }
            if self.shared.is_none() {
                self.shared = Some(share(interop, gl, size)?);
            }
            let Some(sh) = &mut self.shared else {
                return Ok(());
            };
            if !sh.lock(interop) {
                return Err(windows_core::Error::from_thread());
            }
            if !std::mem::replace(&mut sh.attached, true) {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(sh.fb));
                match sh.name {
                    Name::Texture(t) => gl.framebuffer_texture_2d(
                        glow::FRAMEBUFFER,
                        glow::COLOR_ATTACHMENT0,
                        glow::TEXTURE_2D,
                        Some(t),
                        0,
                    ),
                    Name::Renderbuffer(r) => gl.framebuffer_renderbuffer(
                        glow::FRAMEBUFFER,
                        glow::COLOR_ATTACHMENT0,
                        glow::RENDERBUFFER,
                        Some(r),
                    ),
                }
            }
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(from));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(sh.fb));
            let (w, h) = size;
            gl.blit_framebuffer(
                0,
                0,
                w,
                h,
                0,
                h,
                w,
                0,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            sh.unlock(interop);
            let back: ID3D11Texture2D = self.swap.GetBuffer(0)?;
            interop.context.CopyResource(&back, &sh.texture);
            self.swap.Present(0, DXGI_PRESENT(0)).ok()
        }
    }

    unsafe fn release(&mut self, interop: &Interop, gl: &glow::Context) {
        if let Some(sh) = self.shared.take() {
            unsafe { sh.delete(interop, gl) };
        }
    }

    /// Frees the GL side (the context is current).
    ///
    /// # Safety
    /// The GL context that shares `interop` must be current.
    pub unsafe fn delete(mut self, interop: &Interop, gl: &glow::Context) {
        unsafe { self.release(interop, gl) };
    }
}

/// A texture of `size` pixels Direct3D and GL share, as a GL framebuffer.
unsafe fn share(interop: &Interop, gl: &glow::Context, size: (i32, i32)) -> Result<Shared> {
    unsafe {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: size.0.max(1) as u32,
            Height: size.1.max(1) as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: interop.kind.format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: interop.kind.misc,
        };
        let mut texture = None;
        interop
            .device
            .CreateTexture2D(&desc, None, Some(&mut texture))?;
        let texture = texture.ok_or_else(windows_core::Error::empty)?;
        let Ok(fb) = gl.create_framebuffer() else {
            return Err(windows_core::Error::empty());
        };
        let (name, raw, target) = if interop.kind.texture {
            let Ok(t) = gl.create_texture() else {
                gl.delete_framebuffer(fb);
                return Err(windows_core::Error::empty());
            };
            (Name::Texture(t), t.0.get(), glow::TEXTURE_2D)
        } else {
            let Ok(r) = gl.create_renderbuffer() else {
                gl.delete_framebuffer(fb);
                return Err(windows_core::Error::empty());
            };
            (Name::Renderbuffer(r), r.0.get(), glow::RENDERBUFFER)
        };
        let object = (interop.register)(
            interop.handle,
            texture.as_raw(),
            raw,
            target,
            WGL_ACCESS_READ_WRITE_NV,
        );
        if object.is_invalid() || object.0.is_null() {
            let e = windows_core::Error::from_thread();
            gl.delete_framebuffer(fb);
            match name {
                Name::Texture(t) => gl.delete_texture(t),
                Name::Renderbuffer(r) => gl.delete_renderbuffer(r),
            }
            return Err(e);
        }
        Ok(Shared {
            texture,
            object,
            name,
            fb,
            attached: false,
        })
    }
}
