//! 3D scenes (`scene3d()`): the frames of the core's [`ScenePlayer`] drawn
//! by prism-native's OpenGL renderer ([`prism_native::gl3d`], the GTK
//! backend's), in a WGL context of the app's (OpenGL 3.3 core, the GPU
//! driver's), and shown by a XAML `SwapChainPanel`.
//!
//! WinUI composes with DirectX, so OpenGL cannot draw into the island
//! itself: each scene draws into a framebuffer of its own at its size in
//! pixels, which goes to the panel's DXGI swap chain on the GPU
//! (`dxgl.rs`, `WGL_NV_DX_interop2`); a driver without that extension
//! reads it back into a `WriteableBitmap` shown by an `Image` in the
//! panel. Every scene shares the one context (a hidden window's), so
//! meshes, textures and environments are uploaded once for all of them;
//! it starts after the window's first frame (loading the GPU driver takes
//! a while).
//!
//! Drawn on change only: while the player says something moves, a frame
//! every 16 ms; none when it stops (idle: no CPU). Input as on the other
//! backends: drag to turn (right or Shift drag: pan), the wheel zooms, a
//! click that did not move taps (`UiEvent::SceneTap`). Textures load and
//! decode (WIC) off the UI thread.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, c_void};
use std::rc::{Rc, Weak};

use glow::HasContext;
use prism_core::scene3d::player::{ScenePlayer, TAP_SLOP};
use prism_core::scene3d::{Orbit, SceneUpdate, blobs};
use prism_core::{NodeId, UiEvent};
use prism_native::gl3d::{self, Gpu, Image as Pixels, Shared, Slot, Wanted, lookup, publish};
use prism_native::images::ImageSources;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Gdi::{GetDC, HDC};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::Graphics::OpenGL::{
    ChoosePixelFormat, HGLRC, PFD_DOUBLEBUFFER, PFD_DRAW_TO_WINDOW, PFD_MAIN_PLANE,
    PFD_SUPPORT_OPENGL, PFD_TYPE_RGBA, PIXELFORMATDESCRIPTOR, SetPixelFormat, wglCreateContext,
    wglDeleteContext, wglGetProcAddress, wglMakeCurrent,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_OWNDC, CreateWindowExW, DefWindowProcW, KillTimer, RegisterClassW, SetTimer, WNDCLASSW,
    WS_OVERLAPPEDWINDOW,
};
use windows::core::{PCSTR, w};
use windows_core::{Interface, Result};

use crate::imp::{dispatch, with_state};
use crate::win::log;
use crate::winui::bindings::Microsoft::UI::Dispatching::{DispatcherQueue, DispatcherQueueHandler};
use crate::winui::bindings::Microsoft::UI::Input::{PointerDeviceType, PointerUpdateKind};
use crate::winui::xaml::Controls::{Image, SwapChainPanel};
use crate::winui::xaml::Input::{PointerEventHandler, PointerRoutedEventArgs};
use crate::winui::xaml::Media::Imaging::WriteableBitmap;
use crate::winui::xaml::Media::Stretch;
use crate::winui::xaml::UIElement;

/// A wheel notch scrolls 100 points (browsers' unit for the player's
/// `exp(-delta_y * 0.002)` zoom), so a notch zooms the same everywhere.
const POINTS_PER_NOTCH: f32 = 100.;
/// While the scene moves: a frame this often (ms).
const FRAME_MS: u32 = 16;

// --- The OpenGL context -------------------------------------------------------------------

/// The app's GL context (a hidden window's), current while scenes draw.
struct Context {
    hdc: HDC,
    hglrc: HGLRC,
    shared: Rc<RefCell<Shared>>,
    /// Frames to the swap chains on the GPU (else read back).
    interop: Option<crate::dxgl::Interop>,
    /// The framebuffer scenes draw into, and its size.
    target: RefCell<Option<Target>>,
}

struct Target {
    fb: glow::Framebuffer,
    color: glow::Renderbuffer,
    depth: glow::Renderbuffer,
    size: (i32, i32),
}

thread_local! {
    /// Created on the first scene; `Err` if this PC has no OpenGL 3.3.
    static CONTEXT: RefCell<Option<std::result::Result<Rc<Context>, String>>> =
        const { RefCell::new(None) };
    /// The renderer's GL function pointers (one context: one loader).
    static GL: RefCell<Option<Rc<glow::Context>>> = const { RefCell::new(None) };
}

impl Context {
    fn current(&self) -> bool {
        unsafe { wglMakeCurrent(self.hdc, self.hglrc).is_ok() }
    }
}

/// The context, made current (created the first time).
fn context() -> std::result::Result<Rc<Context>, String> {
    let ctx = CONTEXT.with(|c| {
        c.borrow_mut()
            .get_or_insert_with(|| unsafe { create() }.map(Rc::new))
            .clone()
    })?;
    if !ctx.current() {
        return Err("wglMakeCurrent failed".into());
    }
    Ok(ctx)
}

/// `name`'s address: the driver's (`wglGetProcAddress`), else
/// `opengl32.dll`'s for OpenGL 1.1 functions.
fn proc_address(name: &CStr) -> *const c_void {
    unsafe {
        let p = wglGetProcAddress(PCSTR(name.as_ptr().cast()));
        if let Some(f) = p {
            let v = f as usize;
            // Some drivers answer 1, 2, 3 or -1 for "not mine".
            if !matches!(v, 1 | 2 | 3 | usize::MAX) {
                return v as *const c_void;
            }
        }
        let gl = GetModuleHandleW(w!("opengl32.dll"))
            .or_else(|_| LoadLibraryW(w!("opengl32.dll")))
            .unwrap_or(HMODULE::default());
        GetProcAddress(gl, PCSTR(name.as_ptr().cast()))
            .map_or(std::ptr::null(), |f| f as *const c_void)
    }
}

/// The hidden window's procedure: Windows' default.
unsafe extern "system" fn window_proc(
    h: HWND,
    msg: u32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    unsafe { DefWindowProcW(h, msg, wp, lp) }
}

/// A hidden window with an OpenGL pixel format, and a 3.3 core context on
/// it (`WGL_ARB_create_context`).
unsafe fn create() -> std::result::Result<Context, String> {
    let started = std::time::Instant::now();
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let class = w!("PrismGL");
        let wc = WNDCLASSW {
            style: CS_OWNDC,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            Default::default(),
            class,
            w!(""),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            16,
            16,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| format!("window: {e}"))?;
        let hdc = GetDC(Some(hwnd));
        let pfd = PIXELFORMATDESCRIPTOR {
            nSize: size_of::<PIXELFORMATDESCRIPTOR>() as u16,
            nVersion: 1,
            dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
            iPixelType: PFD_TYPE_RGBA,
            cColorBits: 32,
            cAlphaBits: 8,
            cDepthBits: 24,
            cStencilBits: 8,
            iLayerType: PFD_MAIN_PLANE.0 as u8,
            ..Default::default()
        };
        let format = ChoosePixelFormat(hdc, &pfd);
        if format == 0 {
            return Err("no OpenGL pixel format".into());
        }
        SetPixelFormat(hdc, format, &pfd).map_err(|e| format!("pixel format: {e}"))?;
        // A legacy context first: it gives `wglCreateContextAttribsARB`.
        let legacy = wglCreateContext(hdc).map_err(|e| format!("OpenGL: {e}"))?;
        wglMakeCurrent(hdc, legacy).map_err(|e| format!("OpenGL: {e}"))?;
        let create_attribs = proc_address(c"wglCreateContextAttribsARB");
        if create_attribs.is_null() {
            let _ = wglMakeCurrent(HDC::default(), HGLRC::default());
            let _ = wglDeleteContext(legacy);
            return Err("the driver has no WGL_ARB_create_context (OpenGL 3.3)".into());
        }
        type CreateAttribs = unsafe extern "system" fn(HDC, HGLRC, *const i32) -> HGLRC;
        let create_attribs: CreateAttribs = std::mem::transmute(create_attribs);
        // WGL_CONTEXT_MAJOR_VERSION_ARB, MINOR, PROFILE_MASK = CORE.
        let attribs = [0x2091, 3, 0x2092, 3, 0x9126, 1, 0];
        let hglrc = create_attribs(hdc, HGLRC::default(), attribs.as_ptr());
        let _ = wglMakeCurrent(HDC::default(), HGLRC::default());
        let _ = wglDeleteContext(legacy);
        if hglrc.is_invalid() {
            return Err("the driver gave no OpenGL 3.3 core context".into());
        }
        wglMakeCurrent(hdc, hglrc).map_err(|e| format!("OpenGL: {e}"))?;
        let gl = glow::Context::from_loader_function_cstr(proc_address);
        let (major, minor) = (
            gl.get_parameter_i32(glow::MAJOR_VERSION),
            gl.get_parameter_i32(glow::MINOR_VERSION),
        );
        gl3d::check_version(major, minor, false, false)?;
        log!(
            "3D scenes: {} {} (OpenGL started in {:.0} ms)",
            gl.get_parameter_string(glow::RENDERER),
            gl.get_parameter_string(glow::VERSION),
            started.elapsed().as_secs_f64() * 1000.
        );
        GL.with(|g| *g.borrow_mut() = Some(Rc::new(gl)));
        let interop = if crate::win::env("PRISM_3D_READBACK").is_some() {
            None
        } else {
            match crate::dxgl::Interop::new(hdc, proc_address) {
                Ok(i) => Some(i),
                Err(e) => {
                    log!("3D scenes: frames read back through the CPU ({e})");
                    None
                }
            }
        };
        Ok(Context {
            hdc,
            hglrc,
            shared: Rc::default(),
            interop,
            target: RefCell::new(None),
        })
    }
}

/// The framebuffer to draw `size` pixels into, bound (made or resized).
unsafe fn bind_target(ctx: &Context, gl: &glow::Context, size: (i32, i32)) -> bool {
    unsafe {
        let mut target = ctx.target.borrow_mut();
        if target.as_ref().is_some_and(|t| t.size != size)
            && let Some(t) = target.take()
        {
            gl.delete_framebuffer(t.fb);
            gl.delete_renderbuffer(t.color);
            gl.delete_renderbuffer(t.depth);
        }
        if target.is_none() {
            let (Ok(fb), Ok(color), Ok(depth)) = (
                gl.create_framebuffer(),
                gl.create_renderbuffer(),
                gl.create_renderbuffer(),
            ) else {
                return false;
            };
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(color));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::RGBA8, size.0, size.1);
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH24_STENCIL8, size.0, size.1);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fb));
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::RENDERBUFFER,
                Some(color),
            );
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::DEPTH_STENCIL_ATTACHMENT,
                glow::RENDERBUFFER,
                Some(depth),
            );
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                log!("3D scene: framebuffer incomplete");
            }
            *target = Some(Target {
                fb,
                color,
                depth,
                size,
            });
        }
        let fb = target.as_ref().map(|t| t.fb);
        gl.bind_framebuffer(glow::FRAMEBUFFER, fb);
        true
    }
}

// --- Scenes ---------------------------------------------------------------------------------

/// A scene node's panel and what draws in it.
pub struct SceneView {
    pub element: SwapChainPanel,
    state: Rc<RefCell<State>>,
}

struct State {
    id: NodeId,
    player: Option<ScenePlayer>,
    gpu: Option<Gpu>,
    images: HashMap<String, Slot>,
    wanted: Wanted,
    prune: bool,
    animating: bool,
    panel: SwapChainPanel,
    /// The panel's swap chain (frames on the GPU).
    surface: Option<crate::dxgl::Surface>,
    /// Else an image in the panel, and its bitmap.
    image: Option<Image>,
    bitmap: Option<(WriteableBitmap, (i32, i32))>,
    /// Frames drawn and the time they took (`PRISM_TRACE`).
    frames: (u32, f64),
    sources: ImageSources,
    /// A drag in progress: the pointer, where it was, whether it pans, how
    /// far it went in all.
    drag: Option<(u32, (f32, f32), bool, f32)>,
}

thread_local! {
    /// The scenes, by node (to draw them from the frame timer).
    static SCENES: RefCell<HashMap<NodeId, Weak<RefCell<State>>>> = RefCell::new(HashMap::new());
    /// Scenes to draw at the next frame, and whether the timer runs.
    static DIRTY: RefCell<HashSet<NodeId>> = RefCell::new(HashSet::new());
    static TIMER: Cell<usize> = const { Cell::new(0) };
    static START: std::time::Instant = std::time::Instant::now();
}

fn now() -> f64 {
    START.with(|s| s.elapsed().as_secs_f64())
}

impl SceneView {
    pub fn new(id: NodeId) -> Result<SceneView> {
        let element = SwapChainPanel::new()?;
        // Hit everywhere, for the input below.
        element
            .cast::<crate::winui::xaml::Controls::Panel>()?
            .SetBackground(&crate::winui::transparent()?)?;
        let state = Rc::new(RefCell::new(State {
            id,
            player: None,
            gpu: None,
            images: HashMap::new(),
            wanted: Wanted::default(),
            prune: false,
            animating: false,
            panel: element.clone(),
            surface: None,
            image: None,
            bitmap: None,
            frames: (0, 0.),
            sources: ImageSources::default(),
            drag: None,
        }));
        SCENES.with(|s| s.borrow_mut().insert(id, Rc::downgrade(&state)));
        attach_input(&element.cast()?, id)?;
        Ok(SceneView { element, state })
    }

    /// Applies a scene update (animating to it if the scene has a
    /// transition) and starts loading its new textures from `sources`.
    pub fn set_scene(&self, update: &SceneUpdate, sources: &ImageSources) {
        let mut st = self.state.borrow_mut();
        st.sources = sources.clone();
        let t = now();
        let player = st.player.get_or_insert_with(|| ScenePlayer::new(t));
        player.apply(update, t);
        let wanted = Wanted::of(player);
        st.images.retain(|src, _| wanted.sources.contains(src));
        let mut load = Vec::new();
        for src in &wanted.sources {
            if st.images.contains_key(src) {
                continue;
            }
            let on_gpu = st.gpu.as_ref().is_some_and(|g| g.has_texture(src));
            let slot = if on_gpu {
                Slot::Uploaded
            } else if let Some(image) = decoded(src) {
                Slot::Ready(image)
            } else {
                load.push(src.clone());
                Slot::Loading
            };
            st.images.insert(src.clone(), slot);
        }
        st.wanted = wanted;
        st.prune = true;
        let (id, sources) = (st.id, st.sources.clone());
        drop(st);
        for src in load {
            request_image(id, &sources, src);
        }
        queue(id);
    }
}

impl Drop for SceneView {
    fn drop(&mut self) {
        let mut st = self.state.borrow_mut();
        SCENES.with(|s| s.borrow_mut().remove(&st.id));
        let gpu = st.gpu.take();
        let surface = st.surface.take();
        if (gpu.is_some() || surface.is_some())
            && let Ok(ctx) = context()
        {
            // SAFETY: the context that made its objects is current.
            unsafe {
                if let Some(gpu) = gpu {
                    gpu.delete();
                }
                if let (Some(s), Some(i), Some(gl)) =
                    (surface, &ctx.interop, GL.with(|g| g.borrow().clone()))
                {
                    s.delete(i, &gl);
                }
            }
        }
    }
}

/// Draws scene `id` at the next frame: after the window's first one
/// (starting OpenGL loads the GPU driver, which takes a while).
pub fn queue(id: NodeId) {
    DIRTY.with(|d| d.borrow_mut().insert(id));
    if TIMER.get() == 0 && crate::imp::framed() {
        let t = unsafe { SetTimer(None::<HWND>, 0, 1, Some(tick)) };
        TIMER.set(t);
    }
}

/// The window showed its first frame: the scenes waiting for it draw.
pub fn first_frame() {
    if TIMER.get() == 0 && DIRTY.with(|d| !d.borrow().is_empty()) {
        let t = unsafe { SetTimer(None::<HWND>, 0, 1, Some(tick)) };
        TIMER.set(t);
    }
}

/// The frame timer: draws the scenes that changed or move.
unsafe extern "system" fn tick(_: HWND, _: u32, timer: usize, _: u32) {
    unsafe {
        let _ = KillTimer(None::<HWND>, timer);
    }
    TIMER.set(0);
    let ids: Vec<NodeId> = DIRTY.with(|d| d.borrow_mut().drain().collect());
    // Where each scene is and the theme (the window's state is free here:
    // timers run from the message loop).
    let info = with_state(|s| {
        let w = &s.renderer.widgets;
        let dark = w.dark();
        let k = w.scale();
        ids.iter()
            .filter_map(|id| {
                let n = w.scene.get(*id)?;
                Some((*id, (n.frame.w, n.frame.h), k, dark))
            })
            .collect::<Vec<_>>()
    });
    let Some(info) = info else {
        // The window is busy (a modal loop): next frame.
        DIRTY.with(|d| d.borrow_mut().extend(ids));
        let t = unsafe { SetTimer(None::<HWND>, 0, FRAME_MS, Some(tick)) };
        TIMER.set(t);
        return;
    };
    let mut again = Vec::new();
    for (id, size, k, dark) in info {
        let Some(state) = SCENES.with(|s| s.borrow().get(&id).and_then(Weak::upgrade)) else {
            continue;
        };
        if render(&state, size, k, dark) {
            again.push(id);
        }
    }
    if !again.is_empty() {
        DIRTY.with(|d| d.borrow_mut().extend(again));
        if TIMER.get() == 0 {
            let t = unsafe { SetTimer(None::<HWND>, 0, FRAME_MS, Some(tick)) };
            TIMER.set(t);
        }
    }
}

/// Draws a scene `size` DIPs big (`k` pixels per DIP) into its bitmap.
/// Returns whether it moves (draw again next frame).
fn render(state: &Rc<RefCell<State>>, size: (f32, f32), k: f32, dark: bool) -> bool {
    let (w, h) = (
        (size.0 * k).round().max(1.) as i32,
        (size.1 * k).round().max(1.) as i32,
    );
    if size.0 < 1. || size.1 < 1. {
        return false;
    }
    let started = std::time::Instant::now();
    let ctx = match context() {
        Ok(c) => c,
        Err(e) => {
            let id = state.borrow().id;
            log!("3D scene {id}: no OpenGL ({e}): it stays empty");
            return false;
        }
    };
    let Some(gl) = GL.with(|g| g.borrow().clone()) else {
        return false;
    };
    let mut st = state.borrow_mut();
    let id = st.id;
    if st.gpu.is_none() {
        // SAFETY: the context is current (`context()`); this renderer
        // only ever draws with it.
        let gl = unsafe { glow::Context::from_loader_function_cstr(proc_address) };
        match unsafe { Gpu::new(gl, false, ctx.shared.clone()) } {
            Ok(g) => st.gpu = Some(g),
            Err(e) => {
                log!("3D scene {id}: {e}");
                return false;
            }
        }
    }
    let State {
        player,
        gpu,
        images,
        wanted,
        prune,
        animating,
        panel,
        surface,
        image,
        bitmap,
        frames,
        ..
    } = &mut *st;
    let Some(gpu) = gpu else { return false };
    let mut lost = Vec::new();
    // SAFETY (here and below): the context is current.
    unsafe {
        if !bind_target(&ctx, &gl, (w, h)) {
            return false;
        }
        if std::mem::take(prune) {
            gpu.prune(wanted);
        }
        match player {
            Some(player) => {
                player.advance(now());
                let frame = player.frame(size.0, size.1, dark);
                gpu.draw((w, h), &frame, player, images, &mut lost);
                *animating = frame.animating;
            }
            None => {
                gpu.clear_only((w, h));
                *animating = false;
            }
        }
    }
    let fb = ctx.target.borrow().as_ref().map(|t| t.fb);
    // On the GPU: into the panel's swap chain.
    let mut shown = false;
    if let (Some(interop), Some(fb)) = (&ctx.interop, fb) {
        if surface.is_none() {
            match crate::dxgl::Surface::new(interop, panel, (w, h), k) {
                Ok(s) => *surface = Some(s),
                Err(e) => log!("3D scene {id}: swap chain: {e}"),
            }
        }
        if let Some(s) = surface {
            // SAFETY: the context is current.
            match unsafe { s.present(interop, &gl, fb, (w, h), k) } {
                Ok(()) => shown = true,
                Err(e) => log!("3D scene {id}: present: {e}"),
            }
        }
    }
    // Else through the CPU: read back into a bitmap.
    if !shown {
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        // SAFETY: the context is current.
        unsafe {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, fb);
            gl.read_pixels(
                0,
                0,
                w,
                h,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
        }
        if let Err(e) = show(bitmap, &pixels, (w, h)) {
            log!("3D scene {id}: {e}");
        }
        if let Some((b, _)) = bitmap.as_ref()
            && let Err(e) = set_source(panel, image, b)
        {
            log!("3D scene {id}: {e}");
        }
    }
    let moving = *animating;
    if crate::perf::on() {
        frames.0 += 1;
        frames.1 += crate::perf::ms(started);
        let (n, total) = *frames;
        if n == 1 || n % 60 == 0 {
            log!(
                "3D scene {id}: {n} frames, {:.2} ms each ({}, {w}x{h} pixels)",
                total / n as f64,
                if shown { "GPU" } else { "read back" }
            );
        }
    }
    let sources = st.sources.clone();
    drop(st);
    // Textures another scene freed between our update and this frame.
    for src in lost {
        request_image(id, &sources, src);
    }
    moving
}

/// Copies the frame (premultiplied RGBA, bottom row first) into the
/// bitmap (premultiplied BGRA, top row first), made at that size.
fn show(
    bitmap: &mut Option<(WriteableBitmap, (i32, i32))>,
    rgba: &[u8],
    (w, h): (i32, i32),
) -> Result<()> {
    use windows::Win32::System::WinRT::IBufferByteAccess;
    if bitmap.as_ref().is_none_or(|(_, s)| *s != (w, h)) {
        *bitmap = Some((WriteableBitmap::CreateInstanceWithDimensions(w, h)?, (w, h)));
    }
    let Some((wb, _)) = bitmap else {
        return Ok(());
    };
    let buffer = wb.PixelBuffer()?;
    let access: IBufferByteAccess = buffer.cast()?;
    let len = buffer.Capacity()? as usize;
    let row = (w * 4) as usize;
    unsafe {
        let dst = std::slice::from_raw_parts_mut(access.Buffer()?, len);
        for y in 0..h as usize {
            let src = &rgba[(h as usize - 1 - y) * row..][..row];
            let out = &mut dst[y * row..][..row];
            for (o, i) in out
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(src.as_chunks::<4>().0)
            {
                *o = [i[2], i[1], i[0], i[3]];
            }
        }
    }
    buffer.SetLength(len as u32)?;
    wb.Invalidate()
}

/// The panel's image shows `bitmap` (made the first time; the bitmap
/// stays the same until the size changes).
fn set_source(
    panel: &SwapChainPanel,
    image: &mut Option<Image>,
    bitmap: &WriteableBitmap,
) -> Result<()> {
    if image.is_none() {
        let i = Image::new()?;
        i.SetStretch(Stretch::Fill)?;
        panel
            .cast::<crate::winui::xaml::Controls::Panel>()?
            .Children()?
            .Append(&i)?;
        *image = Some(i);
    }
    let Some(i) = image else { return Ok(()) };
    let same = i
        .Source()
        .ok()
        .and_then(|src| src.cast::<WriteableBitmap>().ok())
        .is_some_and(|b| b == *bitmap);
    if !same {
        i.SetSource(bitmap)?;
    }
    Ok(())
}

// --- Textures -------------------------------------------------------------------------------

thread_local! {
    /// Images decoded while a scene holds them, and the scenes waiting for
    /// those being loaded (each loads once).
    static DECODED: RefCell<HashMap<String, Weak<Pixels>>> = RefCell::new(HashMap::new());
    static WAITING: RefCell<HashMap<String, Vec<NodeId>>> = RefCell::new(HashMap::new());
}

fn decoded(src: &str) -> Option<Rc<Pixels>> {
    DECODED.with(|d| lookup(&d.borrow(), src))
}

/// Loads a texture like a `picture` (app asset, file, URL, `data:` URL;
/// `p3d:` images embedded in a model are blobs of this process) and
/// decodes it, off the UI thread; then the scenes waiting get it.
fn request_image(id: NodeId, sources: &ImageSources, src: String) {
    let first = WAITING.with(|w| {
        let mut w = w.borrow_mut();
        let list = w.entry(src.clone()).or_default();
        list.push(id);
        list.len() == 1
    });
    if !first {
        return;
    }
    let Ok(queue) = DispatcherQueue::GetForCurrentThread() else {
        return;
    };
    let sources = sources.clone();
    std::thread::spawn(move || {
        let bytes = match src.strip_prefix("p3d:") {
            Some(blob) => blobs::get(blob).map(|(b, _)| b.to_vec()),
            None => prism_native::images::load(&src, &sources),
        };
        let image = bytes.and_then(|b| decode(&b).ok());
        let _ = queue.TryEnqueue(&DispatcherQueueHandler::new(move || {
            arrived(&src, image.as_ref());
            Ok(())
        }));
    });
}

/// Image `src` is decoded (or failed): the scenes waiting for it draw.
fn arrived(src: &str, image: Option<&(i32, i32, Vec<u8>)>) {
    let image = image.map(|(width, height, rgba)| {
        Rc::new(Pixels {
            width: *width,
            height: *height,
            rgba: rgba.clone(),
        })
    });
    if let Some(i) = &image {
        DECODED.with(|d| publish(&mut d.borrow_mut(), src, i));
    } else {
        log!(
            "3D scene: could not load texture {:?}",
            prism_core::picture_source(src).0
        );
    }
    let waiting = WAITING.with(|w| w.borrow_mut().remove(src).unwrap_or_default());
    for id in waiting {
        let Some(state) = SCENES.with(|s| s.borrow().get(&id).and_then(Weak::upgrade)) else {
            continue;
        };
        // Unless the scene stopped using it meanwhile.
        if let Some(slot) = state.borrow_mut().images.get_mut(src)
            && matches!(slot, Slot::Loading)
        {
            *slot = match &image {
                Some(i) => Slot::Ready(i.clone()),
                None => Slot::Failed,
            };
        }
        queue(id);
    }
}

/// The first frame of an encoded image as straight RGBA rows (WIC, on
/// the loading thread).
fn decode(bytes: &[u8]) -> Result<(i32, i32, Vec<u8>)> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = wic.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let decoder =
            wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let converter = wic.CreateFormatConverter()?;
        converter.Initialize(
            &frame,
            &GUID_WICPixelFormat32bppRGBA,
            WICBitmapDitherTypeNone,
            None,
            0.,
            WICBitmapPaletteTypeMedianCut,
        )?;
        let (mut w, mut h) = (0u32, 0u32);
        converter.GetSize(&mut w, &mut h)?;
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        converter.CopyPixels(std::ptr::null(), w * 4, &mut rgba)?;
        Ok((w as i32, h as i32, rgba))
    }
}

// --- Input -----------------------------------------------------------------------------------

fn state_of(id: NodeId) -> Option<Rc<RefCell<State>>> {
    SCENES.with(|s| s.borrow().get(&id).and_then(Weak::upgrade))
}

fn orbit(id: NodeId) -> Option<Orbit> {
    state_of(id)?
        .borrow()
        .player
        .as_ref()
        .and_then(|p| p.settings().orbit)
}

/// Drag to turn (right or Shift drag: pan), the wheel zooms, a click that
/// did not move taps.
fn attach_input(element: &UIElement, id: NodeId) -> Result<()> {
    let handler = |f: fn(&UIElement, NodeId, &PointerRoutedEventArgs) -> Result<()>| {
        PointerEventHandler::new(move |s, a| {
            let (Some(s), Some(a)) = (s.as_ref(), a.as_ref()) else {
                return Ok(());
            };
            f(&s.cast()?, id, a)
        })
    };
    element.PointerPressed(&handler(pressed))?;
    element.PointerMoved(&handler(moved))?;
    element.PointerReleased(&handler(released))?;
    element.PointerCanceled(&handler(lost))?;
    element.PointerCaptureLost(&handler(lost))?;
    element.PointerWheelChanged(&handler(wheel))?;
    Ok(())
}

fn pressed(e: &UIElement, id: NodeId, a: &PointerRoutedEventArgs) -> Result<()> {
    let p = a.GetCurrentPoint(e)?;
    let right = p
        .Properties()
        .and_then(|p| p.PointerUpdateKind())
        .is_ok_and(|k| k == PointerUpdateKind::RightButtonPressed);
    let shift = a
        .KeyModifiers()
        .is_ok_and(|m| m.contains(windows::System::VirtualKeyModifiers::Shift));
    let o = orbit(id);
    let Some(state) = state_of(id) else {
        return Ok(());
    };
    let at = p.Position().map(|p| (p.X, p.Y))?;
    let turns = o.is_some_and(|o| o.rotate || o.pan);
    let pan = (right || shift) && o.is_some_and(|o| o.pan);
    {
        let mut st = state.borrow_mut();
        st.drag = Some((p.PointerId()?, at, pan, 0.));
        if turns && let Some(pl) = &mut st.player {
            pl.pointer_down();
        }
    }
    if turns {
        // The page around does not scroll while the scene turns.
        if p.PointerDeviceType()? == PointerDeviceType::Touch {
            let _ = e.CancelDirectManipulations();
        }
        e.CapturePointer(&a.Pointer()?)?;
        a.SetHandled(true)?;
    }
    Ok(())
}

fn moved(e: &UIElement, id: NodeId, a: &PointerRoutedEventArgs) -> Result<()> {
    let p = a.GetCurrentPoint(e)?;
    let pointer = p.PointerId()?;
    let Some(state) = state_of(id) else {
        return Ok(());
    };
    let (x, y) = p.Position().map(|p| (p.X, p.Y))?;
    let height = e
        .cast::<crate::winui::xaml::FrameworkElement>()?
        .ActualHeight()? as f32;
    let o = orbit(id);
    {
        let mut st = state.borrow_mut();
        let Some((ptr, last, pan, far)) = st.drag else {
            return Ok(());
        };
        if ptr != pointer || !p.IsInContact()? {
            return Ok(());
        }
        let (mx, my) = (x - last.0, y - last.1);
        st.drag = Some((ptr, (x, y), pan, far + mx.hypot(my)));
        let Some(pl) = &mut st.player else {
            return Ok(());
        };
        if pan {
            pl.pan(mx, my, height);
        } else if o.is_some_and(|o| o.rotate) {
            pl.drag(mx, my);
        } else {
            return Ok(());
        }
    }
    queue(id);
    Ok(())
}

fn released(e: &UIElement, id: NodeId, a: &PointerRoutedEventArgs) -> Result<()> {
    let p = a.GetCurrentPoint(e)?;
    let Some(state) = state_of(id) else {
        return Ok(());
    };
    let (x, y) = p.Position().map(|p| (p.X, p.Y))?;
    let fe = e.cast::<crate::winui::xaml::FrameworkElement>()?;
    let (w, h) = (fe.ActualWidth()? as f32, fe.ActualHeight()? as f32);
    let dark = with_state(|s| s.renderer.widgets.dark()).unwrap_or(false);
    let tap = {
        let mut st = state.borrow_mut();
        let Some((ptr, _, _, far)) = st.drag.take() else {
            return Ok(());
        };
        if ptr != p.PointerId()? {
            return Ok(());
        }
        let Some(pl) = &mut st.player else {
            return Ok(());
        };
        pl.pointer_up();
        (far < TAP_SLOP && pl.settings().tap).then(|| pl.hit(x, y, w, h, dark))
    };
    let _ = e.ReleasePointerCaptures();
    if let Some(event) = tap {
        let e = UiEvent::SceneTap(id, event);
        if crate::win::env("PRISM_TRACE_INPUT").is_some() {
            log!("input: {e:?}");
        }
        dispatch(e);
    }
    // Auto-rotation resumes.
    queue(id);
    Ok(())
}

fn lost(_: &UIElement, id: NodeId, _: &PointerRoutedEventArgs) -> Result<()> {
    if let Some(state) = state_of(id) {
        let mut st = state.borrow_mut();
        if st.drag.take().is_some()
            && let Some(pl) = &mut st.player
        {
            pl.pointer_up();
        }
    }
    queue(id);
    Ok(())
}

fn wheel(e: &UIElement, id: NodeId, a: &PointerRoutedEventArgs) -> Result<()> {
    if !orbit(id).is_some_and(|o| o.zoom) {
        return Ok(());
    }
    let props = a.GetCurrentPoint(e)?.Properties()?;
    if props.IsHorizontalMouseWheel()? {
        return Ok(());
    }
    // Windows: positive turns the wheel away (scrolls up): zooms in.
    let dy = -(props.MouseWheelDelta()? as f32) / 120. * POINTS_PER_NOTCH;
    if let Some(state) = state_of(id)
        && let Some(pl) = &mut state.borrow_mut().player
    {
        pl.zoom((-dy * 0.002).exp());
    }
    a.SetHandled(true)?;
    queue(id);
    Ok(())
}
