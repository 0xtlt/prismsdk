//! 3D scenes (`scene3d()`): a SceneKit view from the Swift bridge
//! (`apple/Bridge/PrismScene.swift`, a port of prism-core's scene player).
//! Rust hands it every new scene as JSON and loads its textures like
//! pictures; taps come back as [`UiEvent::SceneTap`].

use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::{CStr, CString, c_char, c_void};

use objc2::rc::Retained;
use objc2_ui_kit::UIView;
use prism_core::scene3d::SceneUpdate;
use prism_core::{NodeId, SceneTapEvent, UiEvent};
use prism_native::images::ImageSources;

unsafe extern "C" {
    /// A new scene view (+1 retained) reporting taps to `tapped`.
    fn prism_scene_create(id: u64, tapped: extern "C" fn(u64, *const c_char)) -> *mut c_void;
    /// Applies a scene update: `SceneUpdate` JSON, `len` bytes.
    fn prism_scene_set(view: *mut c_void, json: *const u8, len: usize);
    /// A texture's image bytes (null: it could not be loaded).
    fn prism_scene_texture(src: *const c_char, bytes: *const u8, len: usize);
}

thread_local! {
    /// Texture sources loading or loaded (Swift keeps the images).
    static TEXTURES: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// The view of scene node `id`.
pub(crate) fn create(id: NodeId) -> Retained<UIView> {
    let view = unsafe { prism_scene_create(id, tapped) };
    unsafe { Retained::from_raw(view.cast()) }.expect("prism_scene_create returns a view")
}

/// Applies `update` (`json`) to a view made by [`create`].
pub(crate) fn set(view: &UIView, update: &SceneUpdate, json: &str, sources: &ImageSources) {
    let view = view as *const UIView as *mut c_void;
    unsafe { prism_scene_set(view, json.as_ptr(), json.len()) };
    load_textures(update, sources);
}

/// Loads the update's new textures on a background thread, then hands them
/// to every scene (each draws untextured until then). Embedded images
/// (`p3d:` blobs) are the Swift side's to fetch (`prism_scene_blob`).
fn load_textures(update: &SceneUpdate, sources: &ImageSources) {
    let mut new = Vec::new();
    for n in update.nodes() {
        let m = &n.material;
        for src in [
            &m.texture,
            &m.mr_map,
            &m.normal_map,
            &m.emissive_map,
            &m.ao_map,
        ]
        .into_iter()
        .flatten()
        {
            if !src.starts_with("p3d:") && TEXTURES.with(|t| t.borrow_mut().insert(src.clone())) {
                new.push(src.clone());
            }
        }
    }
    for src in new {
        let sources = sources.clone();
        std::thread::spawn(move || {
            let bytes = prism_native::images::load(&src, &sources);
            dispatch2::DispatchQueue::main().exec_async(move || {
                if bytes.is_none() {
                    eprintln!(
                        "prism-ios: could not load texture {:?}",
                        prism_core::picture_source(&src).0
                    );
                }
                let (ptr, len) = bytes
                    .as_deref()
                    .map_or((std::ptr::null(), 0), |b| (b.as_ptr(), b.len()));
                let src = CString::new(src.replace('\0', "")).unwrap_or_default();
                unsafe { prism_scene_texture(src.as_ptr(), ptr, len) };
            });
        });
    }
}

/// A tap on a scene whose `on_tap` is set: `SceneTapEvent` JSON.
extern "C" fn tapped(id: u64, json: *const c_char) {
    let json = unsafe { CStr::from_ptr(json) }.to_string_lossy();
    crate::imp::dispatch(UiEvent::SceneTap(id, SceneTapEvent::decode(&json)));
}
