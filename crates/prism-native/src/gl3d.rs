//! The OpenGL renderer of 3D scenes (`scene3d()`): the frames of the
//! core's [`ScenePlayer`] drawn with the shared shaders ([`shaders`]), on
//! OpenGL 3.3 core or OpenGL ES 3. The GTK backend draws it in a
//! `GtkGLArea`, the Windows backend in a WGL context of its own; the
//! backend makes its context current and binds the framebuffer to draw
//! into, forwards input to the player, decodes the textures
//! ([`Image`]) and redraws while the frame moves.
//!
//! 1. the shadow map ([`Frame::shadow`]): the casters, depth only, into a
//!    depth texture of the scene's own framebuffer;
//! 2. the frame: clear, the environment behind (the sky program), then the
//!    draws, into a 4× multisampled framebuffer resolved (blitted) into the
//!    bound one. Without multisampled renderbuffers (or if the resolve
//!    fails), straight into the bound framebuffer, aliased.
//!
//! A program per [`Variant`] a frame uses, built on first use with
//! [`shaders::program_source`]; per-frame uniforms are set once per program
//! per frame. GPU buffers and textures are shared by every scene of a
//! [`Shared`] group (contexts that share their objects): meshes by key,
//! instance sets and environments by blob id, textures by source. Vertex
//! arrays, which contexts cannot share, are per scene. A scene keeps the
//! objects of every node it has (culled or not), and frees them when its
//! nodes no longer use them.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;
use std::rc::{Rc, Weak};

use glow::HasContext;
use prism_core::scene3d::env::EnvMap;
use prism_core::scene3d::math::Mat4;
use prism_core::scene3d::player::{Draw, Frame, ScenePlayer, ShadowPass};
use prism_core::scene3d::shaders::{self, Stage, Variant};
use prism_core::scene3d::{AlphaMode, blobs};

/// Samples per pixel of the frame (fewer if the context has fewer).
const MSAA_SAMPLES: i32 = 4;

/// Most anisotropic filtering used on textures (when the context has it).
const MAX_ANISOTROPY: f32 = 8.;

/// Texture units of the samplers (the table in [`shaders`]).
const SAMPLER_UNITS: [(&str, u32); 8] = [
    ("u_texture", 0),
    ("u_mr_map", 1),
    ("u_normal_map", 2),
    ("u_emissive_map", 3),
    ("u_ao_map", 4),
    ("u_env", ENV_UNIT),
    ("u_shadow_map", SHADOW_UNIT),
    ("u_bones", BONES_UNIT),
];
const ENV_UNIT: u32 = 5;
const SHADOW_UNIT: u32 = 6;
const BONES_UNIT: u32 = 7;

/// A texture image of a scene.
pub enum Slot {
    /// Being read and decoded.
    Loading,
    /// Decoded, not on the GPU yet.
    Ready(Rc<Image>),
    /// On the GPU (the decoded image is dropped).
    Uploaded,
    /// Could not be read or decoded.
    Failed,
}

/// The meshes, instance sets, texture sources and environment of a
/// scene's nodes.
#[derive(Default)]
pub struct Wanted {
    pub meshes: HashSet<String>,
    pub instances: HashSet<String>,
    pub sources: HashSet<String>,
    pub env: Option<String>,
}

impl Wanted {
    /// What `player`'s nodes use now.
    pub fn of(player: &ScenePlayer) -> Wanted {
        let mut w = Wanted::default();
        for n in player.nodes() {
            if let Some(mesh) = &n.mesh {
                w.meshes.insert(mesh.key());
            }
            if let Some(set) = &n.instances {
                w.instances.insert(set.id.id().to_string());
            }
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
                w.sources.insert(src.clone());
            }
        }
        w.env = player
            .settings()
            .env
            .as_ref()
            .map(|e| e.id.id().to_string());
        w
    }
}

/// Whether a context of `major.minor` (OpenGL ES when `es`; a legacy or
/// compatibility profile when `legacy`) runs the renderer: OpenGL 3.3
/// core or OpenGL ES 3.0 (`#version 330 core`, `#version 300 es`).
pub fn check_version(major: i32, minor: i32, es: bool, legacy: bool) -> Result<(), String> {
    let supported = if es {
        major >= 3
    } else {
        (major, minor) >= (3, 3) && !legacy
    };
    if supported {
        Ok(())
    } else {
        let api = if es { "OpenGL ES" } else { "OpenGL" };
        Err(format!(
            "needs OpenGL 3.3 or OpenGL ES 3.0, the display gave {api} {major}.{minor}"
        ))
    }
}

// --- Objects shared between scenes --------------------------------------------------------

/// The GL objects of one share group, by key, while a scene uses them
/// (scenes hold the `Rc`s; the last one to let go deletes the object).
#[derive(Default)]
pub struct Shared {
    meshes: HashMap<String, Weak<MeshBuffers>>,
    instances: HashMap<String, Weak<InstanceBuffer>>,
    textures: HashMap<String, Weak<Tex>>,
    envs: HashMap<String, Weak<EnvTexture>>,
}

/// The live object `key` of a shared map.
pub fn lookup<T>(map: &HashMap<String, Weak<T>>, key: &str) -> Option<Rc<T>> {
    map.get(key).and_then(Weak::upgrade)
}

/// Shares `object` as `key` (and forgets the dead ones).
pub fn publish<T>(map: &mut HashMap<String, Weak<T>>, key: &str, object: &Rc<T>) {
    map.retain(|_, w| w.strong_count() > 0);
    map.insert(key.to_string(), Rc::downgrade(object));
}

/// A GL object scenes share.
trait GlObject {
    /// Needs a context of the share group current.
    unsafe fn delete(&self, gl: &glow::Context);
}

/// Lets go of `object`, deleting it if no other scene holds it.
unsafe fn release<T: GlObject>(gl: &glow::Context, object: Rc<T>) {
    if Rc::strong_count(&object) == 1 {
        unsafe { object.delete(gl) };
    }
}

// --- The renderer -------------------------------------------------------------------------

/// A scene's GL objects (its context is current in every method).
pub struct Gpu {
    gl: glow::Context,
    /// `#version` line of the shaders.
    header: &'static str,
    shared: Rc<RefCell<Shared>>,
    /// By variant (`None`: did not build).
    programs: HashMap<Variant, Option<Program>>,
    /// The environment background (`None` inside: did not build).
    sky: Option<Option<Program>>,
    meshes: HashMap<String, Rc<MeshBuffers>>,
    /// By mesh key: one per instance set it is drawn with (or none).
    vaos: HashMap<String, Vec<Vao>>,
    instances: HashMap<String, Rc<InstanceBuffer>>,
    textures: HashMap<String, Rc<Tex>>,
    envs: HashMap<String, Rc<EnvTexture>>,
    /// Joint matrices of the skinned draws, by node key.
    bones: HashMap<String, Bones>,
    /// For the sky's attribute-less triangle (core profiles need one).
    empty_vao: glow::VertexArray,
    /// On the units of absent maps, the environment and the shadow map:
    /// white, with the sampling state of the real ones (samplers of a type
    /// on a unit with another kind of texture are undefined, and drivers
    /// that specialize shaders on the bound textures' state, like Mesa's
    /// llvmpipe, would build a variant per combination).
    blank_map: glow::Texture,
    blank_env: glow::Texture,
    blank_depth: glow::Texture,
    shadow: Option<ShadowTarget>,
    msaa: Option<Msaa>,
    /// Samples of the multisampled frame (0: none, drawn straight).
    samples: i32,
    /// The multisampled frame was resolved once without error.
    resolved_once: bool,
    /// Anisotropic filtering of textures (0: none).
    anisotropy: f32,
    /// Frame number (per-frame uniforms are set once per program a frame).
    tick: u64,
    /// The texture on each unit.
    bound: [Option<glow::Texture>; 8],
    /// Meshes and blobs already reported broken.
    reported: HashSet<String>,
}

/// What a draw needs on the GPU this frame.
struct Resolved {
    vao: glow::VertexArray,
    count: i32,
    /// Instances to draw (instanced draws).
    instances: Option<i32>,
    variant: Variant,
    /// Base color, metal-rough, normal, emissive, occlusion.
    maps: [Option<glow::Texture>; 5],
    bones: Option<glow::Texture>,
}

impl Gpu {
    /// The renderer of a scene drawing with `gl` (OpenGL 3.3 core, or
    /// OpenGL ES 3 when `es`; see [`check_version`]), sharing meshes,
    /// textures and environments with the other scenes of `shared` (the
    /// scenes whose contexts share their objects).
    ///
    /// # Safety
    ///
    /// `gl`'s context is current, here and in every later call.
    pub unsafe fn new(
        gl: glow::Context,
        es: bool,
        shared: Rc<RefCell<Shared>>,
    ) -> Result<Gpu, String> {
        unsafe {
            let header = if es {
                shaders::ES_HEADER
            } else {
                shaders::CORE_HEADER
            };
            let ext = gl.supported_extensions();
            let anisotropy = if ext.contains("GL_EXT_texture_filter_anisotropic")
                || ext.contains("GL_ARB_texture_filter_anisotropic")
            {
                gl.get_parameter_f32(glow::MAX_TEXTURE_MAX_ANISOTROPY)
                    .min(MAX_ANISOTROPY)
            } else {
                0.
            };
            let max_samples = gl.get_parameter_i32(glow::MAX_SAMPLES);
            // Software rasterizers (Mesa's llvmpipe in VMs and CI) shade
            // multisampled triangles an order of magnitude slower: aliased
            // there.
            let renderer = gl.get_parameter_string(glow::RENDERER).to_lowercase();
            let software = ["llvmpipe", "softpipe", "swiftshader", "software"]
                .iter()
                .any(|s| renderer.contains(s));
            let samples = if !software && max_samples >= 2 {
                MSAA_SAMPLES.min(max_samples)
            } else {
                0
            };
            let empty_vao = gl.create_vertex_array()?;
            let white = Image {
                width: 1,
                height: 1,
                rgba: vec![255; 4],
            };
            let blank_map = white.upload(&gl, anisotropy)?;
            let blank_env = EnvTexture::upload(
                &gl,
                &EnvMap {
                    width: 1,
                    height: 1,
                    levels: vec![vec![[0; 4]]],
                    sh: [0.; 27],
                },
            )?
            .texture;
            let blank_depth = depth_texture(&gl, 1)?;
            gl.bind_texture(glow::TEXTURE_2D, None);
            Ok(Gpu {
                gl,
                header,
                shared,
                programs: HashMap::new(),
                sky: None,
                meshes: HashMap::new(),
                vaos: HashMap::new(),
                instances: HashMap::new(),
                textures: HashMap::new(),
                envs: HashMap::new(),
                bones: HashMap::new(),
                empty_vao,
                blank_map,
                blank_env,
                blank_depth,
                shadow: None,
                msaa: None,
                samples,
                resolved_once: false,
                anisotropy,
                tick: 0,
                bound: [None; 8],
                reported: HashSet::new(),
            })
        }
    }

    /// Whether texture `src` is on the GPU (this scene's or its group's).
    pub fn has_texture(&self, src: &str) -> bool {
        self.textures.contains_key(src) || lookup(&self.shared.borrow().textures, src).is_some()
    }

    /// Lets go of what the scene's nodes no longer use.
    ///
    /// # Safety
    ///
    /// The context is current.
    pub unsafe fn prune(&mut self, wanted: &Wanted) {
        let gl = &self.gl;
        unsafe {
            self.vaos.retain(|mesh, list| {
                let keep_mesh = wanted.meshes.contains(mesh);
                list.retain(|v| {
                    let keep = keep_mesh
                        && v.instances
                            .as_ref()
                            .is_none_or(|(id, _)| wanted.instances.contains(id));
                    if !keep {
                        gl.delete_vertex_array(v.vao);
                    }
                    keep
                });
                !list.is_empty()
            });
            drain_unwanted(gl, &mut self.meshes, |k| wanted.meshes.contains(k));
            drain_unwanted(gl, &mut self.instances, |k| wanted.instances.contains(k));
            drain_unwanted(gl, &mut self.textures, |k| wanted.sources.contains(k));
            drain_unwanted(gl, &mut self.envs, |k| wanted.env.as_deref() == Some(k));
        }
    }

    /// Deletes the scene's objects (and the shared ones no other scene
    /// holds).
    ///
    /// # Safety
    ///
    /// The context (or one sharing its objects) is current.
    pub unsafe fn delete(mut self) {
        unsafe {
            self.prune(&Wanted::default());
            let gl = &self.gl;
            for p in self
                .programs
                .values()
                .flatten()
                .chain(self.sky.iter().flatten())
            {
                gl.delete_program(p.program);
            }
            for b in self.bones.values() {
                gl.delete_texture(b.texture);
            }
            if let Some(s) = &self.shadow {
                s.delete(gl);
            }
            if let Some(m) = &self.msaa {
                m.delete(gl);
            }
            gl.delete_vertex_array(self.empty_vao);
            gl.delete_texture(self.blank_map);
            gl.delete_texture(self.blank_env);
            gl.delete_texture(self.blank_depth);
        }
    }

    /// No scene yet: transparent.
    ///
    /// # Safety
    ///
    /// The context is current.
    pub unsafe fn clear_only(&self, (w, h): (i32, i32)) {
        let gl = &self.gl;
        unsafe {
            gl.viewport(0, 0, w, h);
            gl.clear_color(0., 0., 0., 0.);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        }
    }

    /// Draws `frame` into the framebuffer bound now (GTK's area's, or the
    /// backend's own), `size` pixels. Sources whose texture went away are
    /// added to `lost` (to load again).
    ///
    /// # Safety
    ///
    /// The context is current.
    pub unsafe fn draw(
        &mut self,
        size: (i32, i32),
        frame: &Frame,
        player: &ScenePlayer,
        images: &mut HashMap<String, Slot>,
        lost: &mut Vec<String>,
    ) {
        self.tick += 1;
        // GTK binds textures of its own in this context (the area's).
        self.bound = [None; 8];
        // GTK's GL area draws into a framebuffer of its own, bound now.
        let area_fb = unsafe {
            NonZeroU32::new(self.gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) as u32)
                .map(glow::NativeFramebuffer)
        };
        let env = frame
            .env
            .and_then(|e| Some((unsafe { self.env(e.id) }?, e.intensity, e.background)));
        let resolved: Vec<Option<Resolved>> = frame
            .draws
            .iter()
            .map(|d| unsafe { self.resolve(d, player, images, lost) })
            .collect();
        // Uploads bound textures on whichever unit was active.
        self.bound = [None; 8];
        let skinned: HashSet<&str> = frame
            .draws
            .iter()
            .filter(|d| d.joints.is_some())
            .map(|d| d.key)
            .collect();
        unsafe {
            let gl = &self.gl;
            self.bones.retain(|key, b| {
                let keep = skinned.contains(key.as_str());
                if !keep {
                    gl.delete_texture(b.texture);
                }
                keep
            });
            // Constant attributes where a mesh has none: uv (0, 0), white
            // vertex and instance colors.
            gl.vertex_attrib_2_f32(2, 0., 0.);
            gl.vertex_attrib_4_f32(3, 1., 1., 1., 1.);
            gl.vertex_attrib_4_f32(10, 1., 1., 1., 1.);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.front_face(glow::CCW);
            gl.cull_face(glow::BACK);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        }

        let mut globals = Globals::of(frame, env.as_ref(), frame.shadow.as_ref());
        globals.has_shadow = match &frame.shadow {
            Some(pass) => unsafe { self.shadow_pass(pass, frame, &resolved, &globals) },
            None => false,
        };

        // The frame: into the multisampled framebuffer, or the area's.
        let (w, h) = size;
        let msaa = unsafe { self.msaa_target(size) };
        unsafe {
            let Gpu {
                gl,
                bound,
                shadow,
                blank_depth,
                blank_env,
                ..
            } = &mut *self;
            gl.bind_framebuffer(glow::FRAMEBUFFER, msaa.or(area_fb));
            gl.viewport(0, 0, w, h);
            let [r, g, b, a] = frame.clear;
            gl.clear_color(r, g, b, a);
            gl.depth_mask(true);
            gl.clear_depth(1.);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            let shadow_map = match (shadow, globals.has_shadow) {
                (Some(s), true) => s.depth,
                _ => *blank_depth,
            };
            bind(gl, bound, SHADOW_UNIT, shadow_map);
            let env_texture = env.as_ref().map_or(*blank_env, |(e, ..)| e.texture);
            bind(gl, bound, ENV_UNIT, env_texture);
            if let Some((_, intensity, true)) = &env {
                self.sky(frame, *intensity);
            }
        }
        let mut current = None;
        for (d, r) in frame.draws.iter().zip(&resolved) {
            let Some(r) = r else {
                continue;
            };
            let Gpu {
                gl,
                header,
                programs,
                bound,
                tick,
                blank_map,
                ..
            } = &mut *self;
            let Some(p) = (unsafe { program(gl, programs, header, r.variant) }) else {
                continue;
            };
            unsafe {
                if current != Some(r.variant) {
                    gl.use_program(Some(p.program));
                    current = Some(r.variant);
                }
                if p.frame != *tick {
                    p.frame = *tick;
                    globals.set(gl, &p.u, &frame.view_proj);
                }
                let u = &p.u;
                let m = &d.material;
                gl.uniform_matrix_4_f32_slice(u.model.as_ref(), false, &d.world);
                gl.uniform_matrix_3_f32_slice(u.normal_matrix.as_ref(), false, &d.normal);
                gl.uniform_4_f32_slice(u.color.as_ref(), &m.color);
                gl.uniform_3_f32_slice(u.emissive.as_ref(), &m.emissive);
                gl.uniform_1_f32(u.metalness.as_ref(), m.metalness);
                gl.uniform_1_f32(u.roughness.as_ref(), m.roughness);
                gl.uniform_1_i32(u.unlit.as_ref(), m.unlit as i32);
                let mode = match m.alpha {
                    AlphaMode::Blend => 0,
                    AlphaMode::Mask => 1,
                    AlphaMode::Opaque => 2,
                };
                gl.uniform_1_i32(u.alpha_mode.as_ref(), mode);
                gl.uniform_1_f32(u.alpha_cutoff.as_ref(), m.alpha_cutoff);
                gl.uniform_1_i32(u.receive_shadows.as_ref(), d.receive_shadows as i32);
                gl.uniform_1_f32(u.normal_scale.as_ref(), m.normal_scale);
                let flags = [
                    &u.has_texture,
                    &u.has_mr_map,
                    &u.has_normal_map,
                    &u.has_emissive_map,
                    &u.has_ao_map,
                ];
                for (unit, (map, flag)) in r.maps.iter().zip(flags).enumerate() {
                    gl.uniform_1_i32(flag.as_ref(), map.is_some() as i32);
                    bind(gl, bound, unit as u32, map.unwrap_or(*blank_map));
                }
                draw_mesh(gl, bound, r, d.material.double_sided, d.transparent);
            }
        }
        unsafe {
            let gl = &self.gl;
            gl.bind_vertex_array(None);
            gl.depth_mask(true);
            gl.disable(glow::BLEND);
            if msaa.is_some() {
                self.resolve_msaa(area_fb, size);
            }
            let gl = &self.gl;
            gl.bind_framebuffer(glow::FRAMEBUFFER, area_fb);
            gl.active_texture(glow::TEXTURE0);
        }
    }

    /// The environment behind everything (bound on its unit): one
    /// triangle, no depth.
    unsafe fn sky(&mut self, frame: &Frame, intensity: f32) {
        let Gpu {
            gl,
            header,
            sky,
            empty_vao,
            ..
        } = self;
        let sky = sky.get_or_insert_with(|| unsafe {
            build(
                gl,
                header,
                shaders::SKY_VERTEX,
                shaders::SKY_FRAGMENT,
                "sky",
            )
        });
        let Some(p) = sky else {
            return;
        };
        unsafe {
            gl.use_program(Some(p.program));
            let u = &p.u;
            gl.uniform_matrix_4_f32_slice(
                u.inverse_view_proj.as_ref(),
                false,
                &frame.inverse_view_proj,
            );
            gl.uniform_3_f32_slice(u.camera.as_ref(), &frame.camera_position);
            gl.uniform_1_f32(u.env_intensity.as_ref(), intensity);
            gl.uniform_1_f32(u.exposure.as_ref(), frame.exposure);
            gl.disable(glow::DEPTH_TEST);
            gl.depth_mask(false);
            gl.disable(glow::BLEND);
            gl.disable(glow::CULL_FACE);
            gl.bind_vertex_array(Some(*empty_vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_mask(true);
        }
    }

    /// Renders the shadow casters' depth from the light into the shadow
    /// map. False if it could not.
    unsafe fn shadow_pass(
        &mut self,
        pass: &ShadowPass,
        frame: &Frame,
        resolved: &[Option<Resolved>],
        globals: &Globals,
    ) -> bool {
        let size = pass.map_size.clamp(1, 8192) as i32;
        unsafe {
            if self.shadow.as_ref().is_none_or(|s| s.size != size) {
                if let Some(old) = self.shadow.take() {
                    old.delete(&self.gl);
                }
                let made = ShadowTarget::new(&self.gl, size);
                self.bound = [None; 8];
                match made {
                    Ok(s) => self.shadow = Some(s),
                    Err(e) => {
                        if self.reported.insert("shadow".into()) {
                            eprintln!("prism: 3D scene: shadow map: {e}");
                        }
                        return false;
                    }
                }
            }
            let Some(fbo) = self.shadow.as_ref().map(|s| s.fbo) else {
                return false;
            };
            let Gpu {
                gl,
                header,
                programs,
                bound,
                tick,
                blank_depth,
                ..
            } = &mut *self;
            // Not sampled while drawn into.
            bind(gl, bound, SHADOW_UNIT, *blank_depth);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.viewport(0, 0, size, size);
            gl.depth_mask(true);
            gl.clear_depth(1.);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            let mut current = None;
            for &i in &pass.casters {
                let (Some(d), Some(Some(r))) = (frame.draws.get(i), resolved.get(i)) else {
                    continue;
                };
                let variant = Variant {
                    depth_only: true,
                    ..r.variant
                };
                let Some(p) = program(gl, programs, header, variant) else {
                    continue;
                };
                if current != Some(variant) {
                    gl.use_program(Some(p.program));
                    current = Some(variant);
                }
                if p.frame != *tick {
                    p.frame = *tick;
                    globals.set(gl, &p.u, &pass.view_proj);
                }
                gl.uniform_matrix_4_f32_slice(p.u.model.as_ref(), false, &d.world);
                draw_mesh(gl, bound, r, d.material.double_sided, false);
            }
        }
        true
    }

    /// The multisampled framebuffer for a frame of `size` pixels (`None`:
    /// draw straight into the area's).
    unsafe fn msaa_target(&mut self, size: (i32, i32)) -> Option<glow::Framebuffer> {
        if self.samples == 0 {
            return None;
        }
        unsafe {
            if self.msaa.as_ref().is_none_or(|m| m.size != size) {
                if let Some(old) = self.msaa.take() {
                    old.delete(&self.gl);
                }
                match Msaa::new(&self.gl, self.samples, size) {
                    Ok(m) => self.msaa = Some(m),
                    Err(e) => {
                        eprintln!("prism: 3D scene: no multisampling: {e}");
                        self.samples = 0;
                        return None;
                    }
                }
            }
        }
        self.msaa.as_ref().map(|m| m.fbo)
    }

    /// Copies the multisampled frame into the area's framebuffer.
    unsafe fn resolve_msaa(&mut self, area_fb: Option<glow::Framebuffer>, (w, h): (i32, i32)) {
        let Some(m) = &self.msaa else {
            return;
        };
        let gl = &self.gl;
        unsafe {
            if !self.resolved_once {
                while gl.get_error() != glow::NO_ERROR {}
            }
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(m.fbo));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, area_fb);
            gl.blit_framebuffer(
                0,
                0,
                w,
                h,
                0,
                0,
                w,
                h,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            if !self.resolved_once {
                let error = gl.get_error();
                if error == glow::NO_ERROR {
                    self.resolved_once = true;
                } else {
                    // E.g. OpenGL ES, where the formats must match the
                    // area's: draw straight into it from now on.
                    eprintln!("prism: 3D scene: no multisampling (resolve error {error:#x})");
                    self.samples = 0;
                    if let Some(m) = self.msaa.take() {
                        m.delete(gl);
                    }
                }
            }
        }
    }

    /// The GPU side of `d`, uploading what is missing (`None`: it cannot
    /// be drawn yet).
    unsafe fn resolve(
        &mut self,
        d: &Draw,
        player: &ScenePlayer,
        images: &mut HashMap<String, Slot>,
        lost: &mut Vec<String>,
    ) -> Option<Resolved> {
        unsafe {
            let mesh = self.mesh(d, player)?;
            let instances = match d.instances {
                Some(set) => Some((set.id, set.count, self.instance_buffer(set.id)?)),
                None => None,
            };
            let vao = self.vao(
                d.mesh_key,
                &mesh,
                instances.as_ref().map(|(id, _, b)| (*id, b)),
            )?;
            let m = &d.material;
            let maps = [m.texture, m.mr_map, m.normal_map, m.emissive_map, m.ao_map]
                .map(|src| src.and_then(|s| self.texture(s, images, lost)));
            let bones = match d.joints {
                Some(joints) if mesh.skinned => self.bones(d.key, joints),
                _ => None,
            };
            Some(Resolved {
                vao,
                count: mesh.count,
                instances: instances.map(|(_, n, b)| n.min(b.count) as i32),
                variant: Variant {
                    instanced: d.instances.is_some(),
                    skinned: bones.is_some(),
                    depth_only: false,
                },
                maps,
                bones,
            })
        }
    }

    /// The buffers of `d`'s mesh.
    unsafe fn mesh(&mut self, d: &Draw, player: &ScenePlayer) -> Option<Rc<MeshBuffers>> {
        if let Some(m) = self.meshes.get(d.mesh_key) {
            return Some(m.clone());
        }
        let shared = lookup(&self.shared.borrow().meshes, d.mesh_key);
        let mesh = match shared {
            Some(m) => m,
            None => {
                let data = player.geometry(d);
                if data.indices.is_empty() {
                    // A blob that does not decode.
                    if self.reported.insert(d.mesh_key.to_string()) {
                        eprintln!("prism: 3D scene: mesh {} has no triangles", d.mesh_key);
                    }
                    return None;
                }
                let m = match unsafe { MeshBuffers::upload(&self.gl, &data) } {
                    Ok(m) => Rc::new(m),
                    Err(e) => {
                        eprintln!("prism: 3D scene: {e}");
                        return None;
                    }
                };
                publish(&mut self.shared.borrow_mut().meshes, d.mesh_key, &m);
                m
            }
        };
        self.meshes.insert(d.mesh_key.to_string(), mesh.clone());
        Some(mesh)
    }

    /// The per-instance buffer of `I` blob `id`.
    unsafe fn instance_buffer(&mut self, id: &str) -> Option<Rc<InstanceBuffer>> {
        if let Some(b) = self.instances.get(id) {
            return Some(b.clone());
        }
        let shared = lookup(&self.shared.borrow().instances, id);
        let buffer = match shared {
            Some(b) => b,
            None => {
                let Some(set) = blobs::get(id).and_then(|(b, _)| blobs::decode_instances(&b))
                else {
                    if self.reported.insert(id.to_string()) {
                        eprintln!("prism: 3D scene: instance set {id} is missing");
                    }
                    return None;
                };
                let b = Rc::new(unsafe { InstanceBuffer::upload(&self.gl, &set) }.ok()?);
                publish(&mut self.shared.borrow_mut().instances, id, &b);
                b
            }
        };
        self.instances.insert(id.to_string(), buffer.clone());
        Some(buffer)
    }

    /// The vertex array of `mesh` (with instance set `instances`).
    unsafe fn vao(
        &mut self,
        key: &str,
        mesh: &Rc<MeshBuffers>,
        instances: Option<(&str, &Rc<InstanceBuffer>)>,
    ) -> Option<glow::VertexArray> {
        let id = instances.map(|(id, _)| id);
        let list = self.vaos.entry(key.to_string()).or_default();
        if let Some(v) = list
            .iter()
            .find(|v| v.instances.as_ref().map(|(i, _)| i.as_str()) == id)
        {
            return Some(v.vao);
        }
        let vao = unsafe { mesh.vertex_array(&self.gl, instances.map(|(_, b)| &**b)) }.ok()?;
        list.push(Vao {
            vao,
            _mesh: mesh.clone(),
            instances: instances.map(|(id, b)| (id.to_string(), b.clone())),
        });
        Some(vao)
    }

    /// The texture of image `src`, uploading it once decoded.
    unsafe fn texture(
        &mut self,
        src: &str,
        images: &mut HashMap<String, Slot>,
        lost: &mut Vec<String>,
    ) -> Option<glow::Texture> {
        if let Some(t) = self.textures.get(src) {
            return Some(t.0);
        }
        let slot = images.get_mut(src)?;
        let shared = lookup(&self.shared.borrow().textures, src);
        let t = match (shared, &*slot) {
            (Some(t), _) => t,
            (None, Slot::Ready(image)) => {
                let t = Rc::new(Tex(unsafe { image.upload(&self.gl, self.anisotropy) }.ok()?));
                publish(&mut self.shared.borrow_mut().textures, src, &t);
                t
            }
            (None, Slot::Uploaded) => {
                // Uploaded by another scene, which let it go since.
                *slot = Slot::Loading;
                lost.push(src.to_string());
                return None;
            }
            (None, Slot::Loading | Slot::Failed) => return None,
        };
        *slot = Slot::Uploaded;
        self.textures.insert(src.to_string(), t.clone());
        Some(t.0)
    }

    /// The texture and spherical harmonics of `E` blob `id`.
    unsafe fn env(&mut self, id: &str) -> Option<Rc<EnvTexture>> {
        if let Some(e) = self.envs.get(id) {
            return Some(e.clone());
        }
        let shared = lookup(&self.shared.borrow().envs, id);
        let env = match shared {
            Some(e) => e,
            None => {
                let Some(map) = blobs::get(id).and_then(|(b, _)| EnvMap::decode(&b)) else {
                    if self.reported.insert(id.to_string()) {
                        eprintln!("prism: 3D scene: environment {id} is missing");
                    }
                    return None;
                };
                let e = Rc::new(unsafe { EnvTexture::upload(&self.gl, &map) }.ok()?);
                publish(&mut self.shared.borrow_mut().envs, id, &e);
                e
            }
        };
        self.envs.insert(id.to_string(), env.clone());
        Some(env)
    }

    /// The bones texture of skinned node `key` with `joints` (uploaded
    /// when they moved).
    unsafe fn bones(&mut self, key: &str, joints: &[Mat4]) -> Option<glow::Texture> {
        let gl = &self.gl;
        unsafe {
            if let Some(b) = self.bones.get_mut(key) {
                if b.joints.as_slice() != joints {
                    b.upload(gl, joints);
                }
                return Some(b.texture);
            }
            let mut b = Bones {
                texture: gl.create_texture().ok()?,
                joints: Vec::new(),
            };
            b.upload(gl, joints);
            let t = b.texture;
            self.bones.insert(key.to_string(), b);
            Some(t)
        }
    }
}

/// Binds `texture` to texture unit `unit` (unless it is there).
unsafe fn bind(
    gl: &glow::Context,
    bound: &mut [Option<glow::Texture>; 8],
    unit: u32,
    texture: glow::Texture,
) {
    let slot = &mut bound[unit as usize];
    if *slot != Some(texture) {
        *slot = Some(texture);
        unsafe {
            gl.active_texture(glow::TEXTURE0 + unit);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        }
    }
}

/// Draws `r`'s triangles with the current program (its uniforms set):
/// its bones, culling unless `double_sided`, blended without writing depth
/// if `blend`.
unsafe fn draw_mesh(
    gl: &glow::Context,
    bound: &mut [Option<glow::Texture>; 8],
    r: &Resolved,
    double_sided: bool,
    blend: bool,
) {
    unsafe {
        if let Some(b) = r.bones {
            bind(gl, bound, BONES_UNIT, b);
        }
        if double_sided {
            gl.disable(glow::CULL_FACE);
        } else {
            gl.enable(glow::CULL_FACE);
        }
        if blend {
            gl.enable(glow::BLEND);
            gl.depth_mask(false);
        } else {
            gl.disable(glow::BLEND);
            gl.depth_mask(true);
        }
        gl.bind_vertex_array(Some(r.vao));
        match r.instances {
            Some(n) => {
                gl.draw_elements_instanced(glow::TRIANGLES, r.count, glow::UNSIGNED_INT, 0, n)
            }
            None => gl.draw_elements(glow::TRIANGLES, r.count, glow::UNSIGNED_INT, 0),
        }
    }
}

/// Releases the objects of `map` whose key `keep` refuses.
unsafe fn drain_unwanted<T: GlObject>(
    gl: &glow::Context,
    map: &mut HashMap<String, Rc<T>>,
    keep: impl Fn(&str) -> bool,
) {
    let gone: Vec<String> = map.keys().filter(|k| !keep(k)).cloned().collect();
    for k in gone {
        if let Some(o) = map.remove(&k) {
            unsafe { release(gl, o) };
        }
    }
}

// --- Programs -----------------------------------------------------------------------------

/// A linked program, its uniforms, and the frame its per-frame uniforms
/// were last set.
struct Program {
    program: glow::Program,
    u: Uniforms,
    frame: u64,
}

type Location = Option<glow::UniformLocation>;

/// The uniforms of `shaders` (see its table; `None`: not in this variant).
struct Uniforms {
    view_proj: Location,
    camera: Location,
    ambient: Location,
    light_count: Location,
    light_kind: Location,
    light_color: Location,
    light_position: Location,
    light_direction: Location,
    light_params: Location,
    exposure: Location,
    has_env: Location,
    env_levels: Location,
    env_sh: Location,
    env_intensity: Location,
    has_shadow: Location,
    shadow_view_proj: Location,
    shadow_light: Location,
    shadow_bias: Location,
    shadow_normal_bias: Location,
    shadow_texel: Location,
    model: Location,
    normal_matrix: Location,
    color: Location,
    emissive: Location,
    metalness: Location,
    roughness: Location,
    unlit: Location,
    alpha_mode: Location,
    alpha_cutoff: Location,
    receive_shadows: Location,
    has_texture: Location,
    has_mr_map: Location,
    has_normal_map: Location,
    normal_scale: Location,
    has_emissive_map: Location,
    has_ao_map: Location,
    inverse_view_proj: Location,
}

impl Uniforms {
    unsafe fn of(gl: &glow::Context, program: glow::Program) -> Uniforms {
        let at = |name: &str| unsafe { gl.get_uniform_location(program, name) };
        Uniforms {
            view_proj: at("u_view_proj"),
            camera: at("u_camera"),
            ambient: at("u_ambient"),
            light_count: at("u_light_count"),
            light_kind: at("u_light_kind"),
            light_color: at("u_light_color"),
            light_position: at("u_light_position"),
            light_direction: at("u_light_direction"),
            light_params: at("u_light_params"),
            exposure: at("u_exposure"),
            has_env: at("u_has_env"),
            env_levels: at("u_env_levels"),
            env_sh: at("u_env_sh"),
            env_intensity: at("u_env_intensity"),
            has_shadow: at("u_has_shadow"),
            shadow_view_proj: at("u_shadow_view_proj"),
            shadow_light: at("u_shadow_light"),
            shadow_bias: at("u_shadow_bias"),
            shadow_normal_bias: at("u_shadow_normal_bias"),
            shadow_texel: at("u_shadow_texel"),
            model: at("u_model"),
            normal_matrix: at("u_normal_matrix"),
            color: at("u_color"),
            emissive: at("u_emissive"),
            metalness: at("u_metalness"),
            roughness: at("u_roughness"),
            unlit: at("u_unlit"),
            alpha_mode: at("u_alpha_mode"),
            alpha_cutoff: at("u_alpha_cutoff"),
            receive_shadows: at("u_receive_shadows"),
            has_texture: at("u_has_texture"),
            has_mr_map: at("u_has_mr_map"),
            has_normal_map: at("u_has_normal_map"),
            normal_scale: at("u_normal_scale"),
            has_emissive_map: at("u_has_emissive_map"),
            has_ao_map: at("u_has_ao_map"),
            inverse_view_proj: at("u_inverse_view_proj"),
        }
    }
}

/// The program of `variant`, built on first use.
unsafe fn program<'p>(
    gl: &glow::Context,
    programs: &'p mut HashMap<Variant, Option<Program>>,
    header: &str,
    variant: Variant,
) -> Option<&'p mut Program> {
    programs
        .entry(variant)
        .or_insert_with(|| {
            let vertex = shaders::program_source(header, Stage::Vertex, variant);
            let fragment = shaders::program_source(header, Stage::Fragment, variant);
            unsafe { build(gl, "", &vertex, &fragment, &format!("{variant:?}")) }
        })
        .as_mut()
}

/// Links `header` + `vertex` and `header` + `fragment`, gives every
/// sampler its unit, and finds the uniforms (`None`: reported).
unsafe fn build(
    gl: &glow::Context,
    header: &str,
    vertex: &str,
    fragment: &str,
    name: &str,
) -> Option<Program> {
    match unsafe {
        link(
            gl,
            &format!("{header}{vertex}"),
            &format!("{header}{fragment}"),
        )
    } {
        Ok(program) => unsafe {
            gl.use_program(Some(program));
            for (sampler, unit) in SAMPLER_UNITS {
                gl.uniform_1_i32(
                    gl.get_uniform_location(program, sampler).as_ref(),
                    unit as i32,
                );
            }
            Some(Program {
                program,
                u: Uniforms::of(gl, program),
                frame: 0,
            })
        },
        Err(e) => {
            eprintln!("prism: 3D scene: program {name}: {e}");
            None
        }
    }
}

/// Compiles and links a program.
unsafe fn link(gl: &glow::Context, vertex: &str, fragment: &str) -> Result<glow::Program, String> {
    unsafe {
        let program = gl.create_program()?;
        let mut compiled = Vec::new();
        for (kind, source) in [
            (glow::VERTEX_SHADER, vertex),
            (glow::FRAGMENT_SHADER, fragment),
        ] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let log = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                gl.delete_program(program);
                return Err(format!("shader: {log}"));
            }
            gl.attach_shader(program, shader);
            compiled.push(shader);
        }
        gl.link_program(program);
        for shader in compiled {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(format!("program: {log}"));
        }
        Ok(program)
    }
}

/// The per-frame uniforms, flattened once per frame.
#[derive(Clone)]
struct Globals {
    camera: [f32; 3],
    ambient: [f32; 3],
    light_count: i32,
    kinds: Vec<i32>,
    colors: Vec<f32>,
    positions: Vec<f32>,
    directions: Vec<f32>,
    params: Vec<f32>,
    exposure: f32,
    /// Levels, spherical harmonics, intensity.
    env: Option<(f32, [f32; 27], f32)>,
    has_shadow: bool,
    /// View-projection, light, bias, normal bias, texel.
    shadow: Option<(Mat4, i32, f32, f32, f32)>,
}

impl Globals {
    fn of(
        frame: &Frame,
        env: Option<&(Rc<EnvTexture>, f32, bool)>,
        shadow: Option<&ShadowPass>,
    ) -> Globals {
        let lights = &frame.lights;
        let flat = |f: fn(&prism_core::scene3d::player::FrameLight) -> [f32; 3]| {
            lights.iter().flat_map(f).collect::<Vec<f32>>()
        };
        Globals {
            camera: frame.camera_position,
            ambient: frame.ambient,
            light_count: lights.len() as i32,
            kinds: lights.iter().map(|l| l.kind).collect(),
            colors: flat(|l| l.color),
            positions: flat(|l| l.position),
            directions: flat(|l| l.direction),
            params: lights.iter().flat_map(|l| l.params).collect(),
            exposure: frame.exposure,
            env: env.map(|(e, intensity, _)| (e.levels, e.sh, *intensity)),
            has_shadow: shadow.is_some(),
            shadow: shadow.map(|s| {
                (
                    s.view_proj,
                    s.light as i32,
                    s.bias,
                    s.normal_bias,
                    1. / s.map_size.max(1) as f32,
                )
            }),
        }
    }

    /// Sets them on the current program `u` (`view_proj`: the camera's or,
    /// in the depth pass, the light's).
    unsafe fn set(&self, gl: &glow::Context, u: &Uniforms, view_proj: &Mat4) {
        unsafe {
            gl.uniform_matrix_4_f32_slice(u.view_proj.as_ref(), false, view_proj);
            if u.camera.is_none() {
                // Depth only.
                return;
            }
            gl.uniform_3_f32_slice(u.camera.as_ref(), &self.camera);
            gl.uniform_3_f32_slice(u.ambient.as_ref(), &self.ambient);
            gl.uniform_1_i32(u.light_count.as_ref(), self.light_count);
            if self.light_count > 0 {
                gl.uniform_1_i32_slice(u.light_kind.as_ref(), &self.kinds);
                gl.uniform_3_f32_slice(u.light_color.as_ref(), &self.colors);
                gl.uniform_3_f32_slice(u.light_position.as_ref(), &self.positions);
                gl.uniform_3_f32_slice(u.light_direction.as_ref(), &self.directions);
                gl.uniform_4_f32_slice(u.light_params.as_ref(), &self.params);
            }
            gl.uniform_1_f32(u.exposure.as_ref(), self.exposure);
            gl.uniform_1_i32(u.has_env.as_ref(), self.env.is_some() as i32);
            if let Some((levels, sh, intensity)) = &self.env {
                gl.uniform_1_f32(u.env_levels.as_ref(), *levels);
                gl.uniform_3_f32_slice(u.env_sh.as_ref(), sh);
                gl.uniform_1_f32(u.env_intensity.as_ref(), *intensity);
            }
            let shadow = self.shadow.filter(|_| self.has_shadow);
            gl.uniform_1_i32(u.has_shadow.as_ref(), shadow.is_some() as i32);
            // No light index matches: nothing is shadowed.
            gl.uniform_1_i32(u.shadow_light.as_ref(), -1);
            if let Some((matrix, light, bias, normal_bias, texel)) = shadow {
                gl.uniform_matrix_4_f32_slice(u.shadow_view_proj.as_ref(), false, &matrix);
                gl.uniform_1_i32(u.shadow_light.as_ref(), light);
                gl.uniform_1_f32(u.shadow_bias.as_ref(), bias);
                gl.uniform_1_f32(u.shadow_normal_bias.as_ref(), normal_bias);
                gl.uniform_1_f32(u.shadow_texel.as_ref(), texel);
            }
        }
    }
}

// --- Buffers ------------------------------------------------------------------------------

/// The bytes of plain numbers (`f32`, `u16`, `u32` and arrays of them: no
/// padding), for the GPU.
fn bytes<T: Copy>(v: &[T]) -> &[u8] {
    // SAFETY: only called with plain numbers, which are plain bytes.
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast(), std::mem::size_of_val(v)) }
}

/// A mesh's vertex and index buffers. One vertex buffer: positions,
/// normals, uvs, then (if the mesh has them) colors, weights and joints,
/// each attribute in its own section.
struct MeshBuffers {
    vbo: glow::Buffer,
    ibo: glow::Buffer,
    count: i32,
    /// Byte offsets of the sections (`None`: absent; positions at 0).
    normals: usize,
    uv: Option<usize>,
    color: Option<usize>,
    /// Weights, joints.
    skin: Option<(usize, usize)>,
    skinned: bool,
}

impl MeshBuffers {
    unsafe fn upload(
        gl: &glow::Context,
        data: &prism_core::scene3d::geometry::MeshData,
    ) -> Result<MeshBuffers, String> {
        let n = data.vertex_count();
        let mut vertices: Vec<u8> = Vec::with_capacity(n * 64);
        vertices.extend_from_slice(bytes(&data.positions));
        let normals = vertices.len();
        vertices.extend_from_slice(bytes(&data.normals));
        let mut section = |present: bool, b: &[u8]| {
            present.then(|| {
                let at = vertices.len();
                vertices.extend_from_slice(b);
                at
            })
        };
        let uv = section(data.uvs.len() == n * 2, bytes(&data.uvs));
        let color = section(data.colors.len() == n * 4, bytes(&data.colors));
        let skinned = data.joints.len() == n * 4 && data.weights.len() == n * 4 && n > 0;
        let weights = section(skinned, bytes(&data.weights));
        let joints = section(skinned, bytes(&data.joints));
        unsafe {
            let vbo = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &vertices, glow::STATIC_DRAW);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            let ibo = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(ibo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&data.indices), glow::STATIC_DRAW);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            Ok(MeshBuffers {
                vbo,
                ibo,
                count: data.indices.len() as i32,
                normals,
                uv,
                color,
                skin: weights.zip(joints),
                skinned,
            })
        }
    }

    /// A vertex array of these buffers (attributes 0–5) and the instance
    /// set's (6–10, divisor 1).
    unsafe fn vertex_array(
        &self,
        gl: &glow::Context,
        instances: Option<&InstanceBuffer>,
    ) -> Result<glow::VertexArray, String> {
        unsafe {
            let vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            let float = |index: u32, size: i32, offset: usize| {
                gl.enable_vertex_attrib_array(index);
                gl.vertex_attrib_pointer_f32(index, size, glow::FLOAT, false, 0, offset as i32);
            };
            float(0, 3, 0);
            float(1, 3, self.normals);
            if let Some(at) = self.uv {
                float(2, 2, at);
            }
            if let Some(at) = self.color {
                float(3, 4, at);
            }
            if let Some((weights, joints)) = self.skin {
                gl.enable_vertex_attrib_array(4);
                gl.vertex_attrib_pointer_i32(4, 4, glow::UNSIGNED_SHORT, 0, joints as i32);
                float(5, 4, weights);
            }
            if let Some(set) = instances {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(set.buffer));
                for column in 0..4 {
                    let index = 6 + column;
                    gl.enable_vertex_attrib_array(index);
                    gl.vertex_attrib_pointer_f32(
                        index,
                        4,
                        glow::FLOAT,
                        false,
                        64,
                        column as i32 * 16,
                    );
                    gl.vertex_attrib_divisor(index, 1);
                }
                if let Some(at) = set.colors {
                    gl.enable_vertex_attrib_array(10);
                    gl.vertex_attrib_pointer_f32(10, 4, glow::FLOAT, false, 16, at as i32);
                    gl.vertex_attrib_divisor(10, 1);
                }
            }
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.ibo));
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
            Ok(vao)
        }
    }
}

impl GlObject for MeshBuffers {
    unsafe fn delete(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_buffer(self.vbo);
            gl.delete_buffer(self.ibo);
        }
    }
}

/// A vertex array of this scene: a mesh's buffers, with an instance set's.
struct Vao {
    vao: glow::VertexArray,
    _mesh: Rc<MeshBuffers>,
    /// `I` blob id and buffer.
    instances: Option<(String, Rc<InstanceBuffer>)>,
}

/// An instance set: `count` matrices (`T · R(q) · S`, 16 floats each),
/// then the colors if it has them.
struct InstanceBuffer {
    buffer: glow::Buffer,
    count: u32,
    /// Byte offset of the colors.
    colors: Option<usize>,
}

impl InstanceBuffer {
    unsafe fn upload(
        gl: &glow::Context,
        set: &blobs::InstancesBlob,
    ) -> Result<InstanceBuffer, String> {
        let count = set.transforms.len();
        let mut data: Vec<f32> = Vec::with_capacity(count * 20);
        for t in &set.transforms {
            data.extend_from_slice(&t.matrix());
        }
        let colors = (set.colors.len() == count && count > 0).then(|| {
            let at = data.len() * 4;
            data.extend(set.colors.iter().flatten());
            at
        });
        unsafe {
            let buffer = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&data), glow::STATIC_DRAW);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            Ok(InstanceBuffer {
                buffer,
                count: count as u32,
                colors,
            })
        }
    }
}

impl GlObject for InstanceBuffer {
    unsafe fn delete(&self, gl: &glow::Context) {
        unsafe { gl.delete_buffer(self.buffer) };
    }
}

/// An image texture.
struct Tex(glow::Texture);

impl GlObject for Tex {
    unsafe fn delete(&self, gl: &glow::Context) {
        unsafe { gl.delete_texture(self.0) };
    }
}

/// An environment: its levels as the mipmaps of one RGBA16F texture, and
/// its irradiance.
struct EnvTexture {
    texture: glow::Texture,
    levels: f32,
    sh: [f32; 27],
}

impl EnvTexture {
    unsafe fn upload(gl: &glow::Context, map: &EnvMap) -> Result<EnvTexture, String> {
        unsafe {
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            for (i, level) in map.levels.iter().enumerate() {
                let w = (map.width >> i).max(1) as i32;
                let h = (map.height >> i).max(1) as i32;
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    i as i32,
                    glow::RGBA16F as i32,
                    w,
                    h,
                    0,
                    glow::RGBA,
                    glow::HALF_FLOAT,
                    glow::PixelUnpackData::Slice(Some(bytes(level))),
                );
            }
            let last = map.levels.len().saturating_sub(1) as i32;
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_BASE_LEVEL, 0);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, last);
            tex_params(
                gl,
                glow::LINEAR_MIPMAP_LINEAR,
                glow::LINEAR,
                glow::CLAMP_TO_EDGE,
            );
            // Around the horizon: repeat.
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
            Ok(EnvTexture {
                texture,
                levels: map.levels.len() as f32,
                sh: map.sh,
            })
        }
    }
}

impl GlObject for EnvTexture {
    unsafe fn delete(&self, gl: &glow::Context) {
        unsafe { gl.delete_texture(self.texture) };
    }
}

/// A skinned draw's joint matrices: an RGBA32F texture 4 texels wide (a
/// matrix's columns), a row per joint.
struct Bones {
    texture: glow::Texture,
    /// As last uploaded.
    joints: Vec<Mat4>,
}

impl Bones {
    /// Binds it on the bones unit (which the caller then knows has it).
    unsafe fn upload(&mut self, gl: &glow::Context, joints: &[Mat4]) {
        unsafe {
            gl.active_texture(glow::TEXTURE0 + BONES_UNIT);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            let data = glow::PixelUnpackData::Slice(Some(bytes(joints)));
            let rows = joints.len() as i32;
            if self.joints.len() == joints.len() {
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    4,
                    rows,
                    glow::RGBA,
                    glow::FLOAT,
                    data,
                );
            } else {
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA32F as i32,
                    4,
                    rows,
                    0,
                    glow::RGBA,
                    glow::FLOAT,
                    data,
                );
                tex_params(gl, glow::NEAREST, glow::NEAREST, glow::CLAMP_TO_EDGE);
            }
            self.joints.clear();
            self.joints.extend_from_slice(joints);
        }
    }
}

/// Filters and wrap of the bound texture.
unsafe fn tex_params(gl: &glow::Context, min: u32, mag: u32, wrap: u32) {
    let param =
        |name, value: u32| unsafe { gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32) };
    param(glow::TEXTURE_MIN_FILTER, min);
    param(glow::TEXTURE_MAG_FILTER, mag);
    param(glow::TEXTURE_WRAP_S, wrap);
    param(glow::TEXTURE_WRAP_T, wrap);
}

/// A `size` × `size` depth texture for `sampler2DShadow` (left bound).
unsafe fn depth_texture(gl: &glow::Context, size: i32) -> Result<glow::Texture, String> {
    unsafe {
        let t = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(t));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::DEPTH_COMPONENT24 as i32,
            size,
            size,
            0,
            glow::DEPTH_COMPONENT,
            glow::UNSIGNED_INT,
            glow::PixelUnpackData::Slice(None),
        );
        tex_params(gl, glow::LINEAR, glow::LINEAR, glow::CLAMP_TO_EDGE);
        let param = |name, value: u32| gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
        param(glow::TEXTURE_COMPARE_MODE, glow::COMPARE_REF_TO_TEXTURE);
        param(glow::TEXTURE_COMPARE_FUNC, glow::LEQUAL);
        Ok(t)
    }
}

/// The shadow map and the framebuffer drawing into it.
struct ShadowTarget {
    fbo: glow::Framebuffer,
    depth: glow::Texture,
    size: i32,
}

impl ShadowTarget {
    /// Leaves the framebuffer bound.
    unsafe fn new(gl: &glow::Context, size: i32) -> Result<ShadowTarget, String> {
        unsafe {
            let depth = depth_texture(gl, size)?;
            let fbo = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::DEPTH_ATTACHMENT,
                glow::TEXTURE_2D,
                Some(depth),
                0,
            );
            gl.draw_buffers(&[glow::NONE]);
            gl.read_buffer(glow::NONE);
            let target = ShadowTarget { fbo, depth, size };
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                target.delete(gl);
                return Err(format!("framebuffer incomplete ({status:#x})"));
            }
            Ok(target)
        }
    }

    unsafe fn delete(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_framebuffer(self.fbo);
            gl.delete_texture(self.depth);
        }
    }
}

/// The multisampled framebuffer frames are drawn into, then resolved into
/// the area's.
struct Msaa {
    fbo: glow::Framebuffer,
    color: glow::Renderbuffer,
    depth: glow::Renderbuffer,
    size: (i32, i32),
}

impl Msaa {
    /// Leaves the framebuffer bound.
    unsafe fn new(gl: &glow::Context, samples: i32, (w, h): (i32, i32)) -> Result<Msaa, String> {
        unsafe {
            let storage = |format| -> Result<glow::Renderbuffer, String> {
                let rb = gl.create_renderbuffer()?;
                gl.bind_renderbuffer(glow::RENDERBUFFER, Some(rb));
                gl.renderbuffer_storage_multisample(
                    glow::RENDERBUFFER,
                    samples,
                    format,
                    w.max(1),
                    h.max(1),
                );
                Ok(rb)
            };
            let color = storage(glow::RGBA8)?;
            let depth = storage(glow::DEPTH_COMPONENT24)?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);
            let fbo = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::RENDERBUFFER,
                Some(color),
            );
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::DEPTH_ATTACHMENT,
                glow::RENDERBUFFER,
                Some(depth),
            );
            let m = Msaa {
                fbo,
                color,
                depth,
                size: (w, h),
            };
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                m.delete(gl);
                return Err(format!("framebuffer incomplete ({status:#x})"));
            }
            Ok(m)
        }
    }

    unsafe fn delete(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_framebuffer(self.fbo);
            gl.delete_renderbuffer(self.color);
            gl.delete_renderbuffer(self.depth);
        }
    }
}

// --- Textures ---------------------------------------------------------------------------

/// A decoded image: straight (not premultiplied) RGBA rows, top first, so
/// uv `(0, 0)` is its top-left corner.
pub struct Image {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// Bytes as they are (the shader linearizes the sRGB ones), with
    /// mipmaps, repeating, filtered anisotropically if `anisotropy` > 0.
    unsafe fn upload(&self, gl: &glow::Context, anisotropy: f32) -> Result<glow::Texture, String> {
        unsafe {
            let t = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(t));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                self.width,
                self.height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&self.rgba)),
            );
            gl.generate_mipmap(glow::TEXTURE_2D);
            tex_params(gl, glow::LINEAR_MIPMAP_LINEAR, glow::LINEAR, glow::REPEAT);
            if anisotropy > 0. {
                gl.tex_parameter_f32(glow::TEXTURE_2D, glow::TEXTURE_MAX_ANISOTROPY, anisotropy);
            }
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            Ok(t)
        }
    }
}
