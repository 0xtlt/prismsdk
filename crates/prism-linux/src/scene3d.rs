//! 3D scenes (`scene3d()`): a `GtkGLArea` drawing the frames of the core's
//! [`ScenePlayer`] with prism-native's OpenGL renderer
//! ([`prism_native::gl3d`]), on OpenGL 3.3 core or OpenGL ES 3 (whichever
//! GTK created). The player moves the camera, spins, plays clips, poses
//! skins, culls and sorts; this module makes the area's context current,
//! forwards input, decodes textures and redraws. GTK's GL area has no
//! multisampling of its own: the renderer draws into a multisampled
//! framebuffer and resolves it into the area's. GTK 4 makes the GL
//! contexts of a display share their objects: scenes of a share group
//! share their meshes and textures.
//!
//! Drawn on change only: a tick callback redraws every frame while the
//! player says something moves, and is gone when it stops (idle: no CPU).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use prism_core::scene3d::player::{ScenePlayer, TAP_SLOP};
use prism_core::scene3d::{Orbit, SceneUpdate, blobs};
use prism_core::{NodeId, UiEvent};
use prism_native::gl3d::{self, Gpu, Image, Shared, Slot, Wanted, lookup, publish};

/// Scroll-wheel notch in the unit of [`ScenePlayer::zoom`]'s
/// `exp(-delta_y * 0.002)`: browsers report about 100 pixels per notch, GTK
/// one line, so a notch zooms the same everywhere. Touchpads already report
/// pixels.
const PIXELS_PER_NOTCH: f64 = 100.;

/// A scene element: its GL area and what draws in it.
pub struct SceneView {
    pub area: gtk::GLArea,
    state: Rc<RefCell<State>>,
}

#[derive(Default)]
struct State {
    player: Option<ScenePlayer>,
    /// GL objects, while the area is realized.
    gpu: Option<Gpu>,
    /// The scene's texture images, by source.
    images: HashMap<String, Slot>,
    /// What the scene's nodes use (kept on the GPU even while culled).
    wanted: Wanted,
    /// `wanted` changed: free what it no longer has at the next render.
    prune: bool,
    /// The last frame moved something: draw the next one.
    animating: bool,
    /// A tick callback is redrawing every frame.
    ticking: bool,
    /// Where app-asset textures are read from.
    assets: Option<PathBuf>,
    /// App assets compiled into the binary, searched first.
    embedded: prism_core::EmbeddedAssets,
}

impl SceneView {
    /// A scene element reporting taps as node `id`; textures that are app
    /// assets are read from `embedded`, then `assets`.
    pub fn new(
        id: NodeId,
        assets: Option<PathBuf>,
        embedded: prism_core::EmbeddedAssets,
    ) -> Rc<SceneView> {
        // Always with alpha in GTK 4: transparent where the scene has no
        // background.
        let area = gtk::GLArea::new();
        // For drawing straight into the area (no multisampling).
        area.set_has_depth_buffer(true);
        // Only when the scene changes or moves (`queue_render`), not on
        // every redraw of the window.
        area.set_auto_render(false);
        let state = Rc::new(RefCell::new(State {
            assets,
            embedded,
            ..State::default()
        }));
        let s = state.clone();
        area.connect_realize(move |a| realize(a, &s));
        let s = state.clone();
        area.connect_unrealize(move |a| unrealize(a, &s));
        let s = state.clone();
        area.connect_render(move |a, _| {
            render(a, &s);
            glib::Propagation::Stop
        });
        attach_input(id, &area, &state);
        Rc::new(SceneView { area, state })
    }

    /// Applies a scene update (animating to it if the scene has a
    /// transition) and starts loading its new textures.
    pub fn set_scene(&self, update: &SceneUpdate) {
        let now = glib::monotonic_time() as f64 / 1e6;
        let mut st = self.state.borrow_mut();
        let player = st.player.get_or_insert_with(|| ScenePlayer::new(now));
        player.apply(update, now);
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
        drop(st);
        for src in load {
            request_image(&self.area, &self.state, src);
        }
        self.area.queue_render();
    }
}

/// Seconds, on GLib's monotonic clock (the frame clock's base).
fn frame_time(area: &gtk::GLArea) -> f64 {
    area.frame_clock()
        .map_or_else(glib::monotonic_time, |c| c.frame_time()) as f64
        / 1e6
}

// --- GL context -------------------------------------------------------------------------

/// The address of GL function `name`. GTK links libepoxy and draws through
/// it: its `epoxy_glFoo` symbols are pointers to the function for the
/// current context (resolved on first call), so no other loader is needed.
fn epoxy(name: &CStr) -> *const c_void {
    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }
    let Ok(symbol) = CString::new([b"epoxy_", name.to_bytes()].concat()) else {
        return std::ptr::null();
    };
    // SAFETY: a null handle is RTLD_DEFAULT (glibc, musl): the libraries
    // already loaded. An `epoxy_*` symbol is a variable holding a function
    // pointer.
    unsafe {
        let slot = dlsym(std::ptr::null_mut(), symbol.as_ptr()) as *const *const c_void;
        if slot.is_null() {
            std::ptr::null()
        } else {
            *slot
        }
    }
}

fn realize(area: &gtk::GLArea, state: &Rc<RefCell<State>>) {
    area.make_current();
    if let Some(e) = area.error() {
        eprintln!("prism-linux: 3D scene: no OpenGL context: {e}");
        return;
    }
    let Some(context) = area.context() else {
        return;
    };
    let (major, minor) = context.version();
    let es = context.uses_es();
    if let Err(e) = gl3d::check_version(major, minor, es, context.is_legacy()) {
        eprintln!("prism-linux: 3D scene: {e}");
        return;
    }
    // SAFETY: the area's context is current (`make_current` above).
    let gpu = match unsafe {
        let gl = glow::Context::from_loader_function_cstr(epoxy);
        Gpu::new(gl, es, share_group(&context))
    } {
        Ok(gpu) => gpu,
        Err(e) => {
            eprintln!("prism-linux: 3D scene: {e}");
            return;
        }
    };
    // Images uploaded by an earlier context: again, unless the share group
    // still has them.
    let mut st = state.borrow_mut();
    let mut load = Vec::new();
    for (src, slot) in &mut st.images {
        if matches!(slot, Slot::Uploaded) && !gpu.has_texture(src) {
            *slot = match decoded(src) {
                Some(image) => Slot::Ready(image),
                None => {
                    load.push(src.clone());
                    Slot::Loading
                }
            };
        }
    }
    st.gpu = Some(gpu);
    drop(st);
    for src in load {
        request_image(area, state, src);
    }
}

fn unrealize(area: &gtk::GLArea, state: &RefCell<State>) {
    let Some(gpu) = state.borrow_mut().gpu.take() else {
        return;
    };
    area.make_current();
    if area.error().is_none() {
        // SAFETY: the context that created them (or one sharing them) is
        // current.
        unsafe { gpu.delete() };
    }
}

// --- Drawing ----------------------------------------------------------------------------

fn render(area: &gtk::GLArea, state: &Rc<RefCell<State>>) {
    let mut st = state.borrow_mut();
    let State {
        player,
        gpu,
        images,
        wanted,
        prune,
        animating,
        ticking,
        ..
    } = &mut *st;
    let Some(gpu) = gpu else {
        return;
    };
    let scale = area.scale_factor();
    let (w, h) = (area.width(), area.height());
    let size = (w * scale, h * scale);
    if std::mem::take(prune) {
        // SAFETY (here and below): GTK made the area's context current for
        // `render`.
        unsafe { gpu.prune(wanted) };
    }
    let Some(player) = player else {
        unsafe { gpu.clear_only(size) };
        return;
    };
    player.advance(frame_time(area));
    let frame = player.frame(w as f32, h as f32, crate::imp::dark());
    let mut lost = Vec::new();
    unsafe { gpu.draw(size, &frame, player, images, &mut lost) };
    *animating = frame.animating;
    let tick = *animating && !*ticking;
    *ticking |= tick;
    drop(frame);
    drop(st);
    // Textures another scene freed between our update and this frame.
    for src in lost {
        request_image(area, state, src);
    }
    if tick {
        let s = state.clone();
        // Removes itself (`Break`) once a frame did not move.
        let _ = area.add_tick_callback(move |a, _| {
            let Ok(mut st) = s.try_borrow_mut() else {
                return glib::ControlFlow::Continue;
            };
            if st.animating {
                a.queue_render();
                glib::ControlFlow::Continue
            } else {
                st.ticking = false;
                glib::ControlFlow::Break
            }
        });
    }
}
// --- Objects shared between scenes --------------------------------------------------------

thread_local! {
    /// The share groups: a context of each, and its objects.
    static GROUPS: RefCell<Vec<(gdk::GLContext, Weak<RefCell<Shared>>)>> =
        const { RefCell::new(Vec::new()) };
}

/// The objects of `context`'s share group (GTK 4 makes the GL contexts of
/// a display share their objects).
fn share_group(context: &gdk::GLContext) -> Rc<RefCell<Shared>> {
    GROUPS.with_borrow_mut(|groups| {
        groups.retain(|(_, s)| s.strong_count() > 0);
        let found = groups
            .iter()
            .find(|(c, _)| c == context || c.is_shared(context))
            .and_then(|(_, s)| s.upgrade());
        found.unwrap_or_else(|| {
            let s = Rc::default();
            groups.push((context.clone(), Rc::downgrade(&s)));
            s
        })
    })
}

// --- Textures ---------------------------------------------------------------------------

/// A GDK texture's pixels as an [`Image`]: straight (not premultiplied)
/// RGBA rows, top first.
fn image_of(texture: &gdk::Texture) -> Image {
    let (width, height) = (texture.width(), texture.height());
    let stride = width as usize * 4;
    let mut rgba = vec![0; stride * height as usize];
    // Cairo's ARGB32: native-endian words, premultiplied.
    texture.download(&mut rgba, stride);
    for px in rgba.as_chunks_mut::<4>().0 {
        let v = u32::from_ne_bytes(*px);
        let a = v >> 24;
        let straight = |c: u32| match a {
            0 => 0,
            _ => ((c * 255 + a / 2) / a).min(255) as u8,
        };
        *px = [
            straight((v >> 16) & 0xff),
            straight((v >> 8) & 0xff),
            straight(v & 0xff),
            a as u8,
        ];
    }
    Image {
        width,
        height,
        rgba,
    }
}

thread_local! {
    /// Images by source: decoded while a scene holds them, and the scenes
    /// waiting for the ones being loaded (each loads once).
    static IMAGES: RefCell<Images> = RefCell::default();
}

#[derive(Default)]
struct Images {
    decoded: HashMap<String, Weak<Image>>,
    waiting: HashMap<String, Vec<Waiter>>,
}

/// A scene waiting for an image.
type Waiter = (glib::WeakRef<gtk::GLArea>, Weak<RefCell<State>>);

/// Image `src` if a scene holds it decoded.
fn decoded(src: &str) -> Option<Rc<Image>> {
    IMAGES.with_borrow(|i| lookup(&i.decoded, src))
}

/// Loads a texture like a `picture` (app asset, file, URL, `data:` URL;
/// `p3d:` images embedded in a model are blobs of this process) and
/// decodes it, off the main thread; then the scene's slot for it gets the
/// image and the scene is redrawn.
fn request_image(area: &gtk::GLArea, state: &Rc<RefCell<State>>, src: String) {
    let first = IMAGES.with_borrow_mut(|i| {
        let list = i.waiting.entry(src.clone()).or_default();
        list.push((area.downgrade(), Rc::downgrade(state)));
        list.len() == 1
    });
    if !first {
        return;
    }
    let sources = prism_native::images::ImageSources {
        embedded: state.borrow().embedded.clone(),
        asset_dirs: state.borrow().assets.iter().cloned().collect(),
        cache_dir: None,
    };
    glib::spawn_future_local(async move {
        let s = src.clone();
        // GdkTexture decodes on any thread.
        let image = gio::spawn_blocking(move || {
            let bytes = match s.strip_prefix("p3d:") {
                Some(id) => blobs::get(id).map(|(b, _)| b.to_vec()),
                None => prism_native::images::load(&s, &sources),
            }?;
            let texture = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok()?;
            Some(image_of(&texture))
        })
        .await
        .ok()
        .flatten()
        .map(Rc::new);
        let waiting = IMAGES.with_borrow_mut(|i| {
            if let Some(image) = &image {
                publish(&mut i.decoded, &src, image);
            }
            i.waiting.remove(&src).unwrap_or_default()
        });
        if image.is_none() {
            eprintln!(
                "prism-linux: could not load texture {:?}",
                prism_core::picture_source(&src).0
            );
        }
        for (area, state) in waiting {
            let (Some(area), Some(state)) = (area.upgrade(), state.upgrade()) else {
                continue;
            };
            // Unless the scene stopped using it meanwhile.
            if let Some(slot) = state.borrow_mut().images.get_mut(&src)
                && matches!(slot, Slot::Loading)
            {
                *slot = match &image {
                    Some(image) => Slot::Ready(image.clone()),
                    None => Slot::Failed,
                };
                area.queue_render();
            }
        }
    });
}

// --- Input ------------------------------------------------------------------------------

/// Drag to turn (right or Shift drag: pan), pinch or scroll to zoom, click
/// to tap. Controllers do nothing (and let events through, e.g. to a parent
/// scroll view) when the scene's orbit does not allow them.
fn attach_input(id: NodeId, area: &gtk::GLArea, state: &Rc<RefCell<State>>) {
    fn orbit(state: &RefCell<State>) -> Option<Orbit> {
        state
            .borrow()
            .player
            .as_ref()
            .and_then(|p| p.settings().orbit)
    }

    // Turn and pan. Offsets are from the press: `last` turns them into
    // moves since the previous update. `pan`: `Some` while dragging, true
    // for a right or Shift drag.
    let drag = gtk::GestureDrag::new();
    drag.set_button(0);
    let pan: Rc<Cell<Option<bool>>> = Rc::default();
    let last = Rc::new(Cell::new((0., 0.)));
    let (s, p, l) = (state.clone(), pan.clone(), last.clone());
    drag.connect_drag_begin(move |g, _, _| {
        if !orbit(&s).is_some_and(|o| o.rotate || o.pan) {
            g.set_state(gtk::EventSequenceState::Denied);
            return;
        }
        let shift = g
            .current_event_state()
            .contains(gdk::ModifierType::SHIFT_MASK);
        p.set(Some(g.current_button() == gdk::BUTTON_SECONDARY || shift));
        l.set((0., 0.));
        if let Some(p) = &mut s.borrow_mut().player {
            p.pointer_down();
        }
    });
    let (s, p) = (state.clone(), pan.clone());
    drag.connect_drag_update(move |g, dx, dy| {
        let Some(pan) = p.get() else {
            return;
        };
        // A drag, not a tap: the click (and a parent's gestures) stop.
        if dx.hypot(dy) >= TAP_SLOP as f64 {
            g.set_state(gtk::EventSequenceState::Claimed);
        }
        let (lx, ly) = last.replace((dx, dy));
        let (mx, my) = ((dx - lx) as f32, (dy - ly) as f32);
        let height = g.widget().map_or(0, |w| w.height()) as f32;
        if let Some(p) = &mut s.borrow_mut().player {
            if pan {
                p.pan(mx, my, height);
            } else {
                p.drag(mx, my);
            }
        }
        redraw(g);
    });
    let up = {
        let s = state.clone();
        move |g: &gtk::GestureDrag| {
            if pan.take().is_some() {
                if let Some(p) = &mut s.borrow_mut().player {
                    p.pointer_up();
                }
                // Auto-rotation resumes.
                redraw(g);
            }
        }
    };
    let up2 = up.clone();
    drag.connect_drag_end(move |g, _, _| up(g));
    drag.connect_cancel(move |g, _| up2(g));
    area.add_controller(drag);

    // Pinch (touchscreens, touchpads): the scale since the gesture began.
    let zoom = gtk::GestureZoom::new();
    let scale = Rc::new(Cell::new(1.));
    let (s, sc) = (state.clone(), scale.clone());
    zoom.connect_begin(move |g, _| {
        if orbit(&s).is_some_and(|o| o.zoom) {
            sc.set(1.);
            g.set_state(gtk::EventSequenceState::Claimed);
        } else {
            g.set_state(gtk::EventSequenceState::Denied);
        }
    });
    let s = state.clone();
    zoom.connect_scale_changed(move |g, now| {
        let ratio = now / scale.replace(now);
        if let Some(p) = &mut s.borrow_mut().player {
            p.zoom(ratio as f32);
        }
        redraw(g);
    });
    area.add_controller(zoom);

    // Scroll wheels and two-finger scrolls.
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let s = state.clone();
    scroll.connect_scroll(move |c, _, dy| {
        if !orbit(&s).is_some_and(|o| o.zoom) {
            return glib::Propagation::Proceed;
        }
        let notch = match c.unit() {
            gdk::ScrollUnit::Wheel => PIXELS_PER_NOTCH,
            _ => 1.,
        };
        if let Some(p) = &mut s.borrow_mut().player {
            p.zoom((-dy * notch * 0.002).exp() as f32);
        }
        redraw(c);
        glib::Propagation::Stop
    });
    area.add_controller(scroll);

    // Taps: a primary click that moved less than the slop.
    let click = gtk::GestureClick::new();
    let pressed = Rc::new(Cell::new((0., 0.)));
    let p = pressed.clone();
    click.connect_pressed(move |_, _, x, y| p.set((x, y)));
    let s = state.clone();
    click.connect_released(move |g, _, x, y| {
        let (px, py) = pressed.get();
        if (x - px).hypot(y - py) >= TAP_SLOP as f64 {
            return;
        }
        let Some(area) = g.widget() else {
            return;
        };
        let (w, h) = (area.width() as f32, area.height() as f32);
        let event = match &s.borrow().player {
            Some(p) if p.settings().tap => p.hit(x as f32, y as f32, w, h, crate::imp::dark()),
            _ => return,
        };
        // Not borrowed anymore: claiming cancels the drag gesture, whose
        // handler borrows the state.
        g.set_state(gtk::EventSequenceState::Claimed);
        crate::imp::dispatch(UiEvent::SceneTap(id, event));
    });
    area.add_controller(click);
}

/// Draws the scene a controller is attached to again.
fn redraw(c: &impl IsA<gtk::EventController>) {
    if let Some(area) = c.widget().and_downcast::<gtk::GLArea>() {
        area.queue_render();
    }
}
