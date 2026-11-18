//! What happens to a scene on the device, between two renders of the app:
//! the orbiting camera, spinning nodes, transitions, animation clips and
//! skins, culling, shadows, and hit testing.
//!
//! [`ScenePlayer`] is the reference implementation: the Linux backend uses
//! it as is; the web (JavaScript), Android (Java) and Apple (Swift) backends
//! port it, so every platform moves, animates and picks the same way.
//!
//! ## Protocol
//!
//! A backend keeps one player per scene element and:
//!
//! 1. `ScenePlayer::new(now)` when the element appears, then
//!    [`apply`](ScenePlayer::apply)`(update, now)` on every
//!    [`SceneUpdate`] (the first one is full). Blobs an update refers to
//!    (meshes, instances, clips, skins, the environment, `p3d:` textures)
//!    are fetched by id (see [`blobs`]); a node draws once its
//!    mesh has arrived, and the frame after a blob arrives is redrawn;
//! 2. forwards gestures: [`pointer_down`](ScenePlayer::pointer_down),
//!    [`drag`](ScenePlayer::drag) (points moved since the last call),
//!    [`pointer_up`](ScenePlayer::pointer_up), [`zoom`](ScenePlayer::zoom)
//!    (pinch scale; scroll wheels: `exp(-delta_y * 0.002)`),
//!    [`pan`](ScenePlayer::pan) (two fingers, right or Shift drag);
//! 3. each frame: [`advance`](ScenePlayer::advance)`(now)`, then
//!    [`frame`](ScenePlayer::frame)`(width, height, dark)`: renders the
//!    shadow map if [`Frame::shadow`] (its casters, depth only), the
//!    environment behind if [`FrameEnv::background`], then the
//!    [`Draw`]s in order; it schedules another frame while
//!    [`Frame::animating`] is true (and after a gesture or an update);
//! 4. on a tap (a press that moved less than 6 points), when the scene's
//!    `tap` is set: [`hit`](ScenePlayer::hit) and reports the event.
//!
//! Times are seconds (any origin, increasing); sizes are points.
//!
//! ## Inside
//!
//! The player keeps the nodes in a flat pre-order table (parents before
//! children, so a node's subtree is the range `i..end`). Each row has its
//! target pose (the update's), a running transition, the clip pose that
//! overrides it, and its cached local and world matrices. Each frame only
//! the rows whose local pose moves (spin, transition, clip) are recomputed,
//! with their subtrees' worlds. Then:
//!
//! - **Poses**: a transition lerps position, scale, colors, metalness and
//!   roughness; rotations lerp Euler degrees when both ends have no
//!   quaternion (v1: a 0→360 turn goes all the way round), else slerp.
//!   Local matrix: `T · R · S`, with `R = Rx·Ry·Rz(rot + spin·t)`, or
//!   `R(quat) · Rx·Ry·Rz(spin·t)` with a quaternion.
//! - **Clips** ([`ClipChannel::sample`](super::clip::ClipChannel::sample)):
//!   a node's clip starts when its `play` changes clip or `run`; with
//!   `fade`, the previous clip keeps playing and blends out over `fade`
//!   seconds (lerp, slerp for rotations; a property only one clip animates
//!   blends with the node's own pose). Clip time: `(now − start) · speed`,
//!   wrapped when looping, else clamped (the last pose holds). Sampled
//!   values replace the target's position, rotation (as a quaternion) and
//!   scale.
//! - **Skins**: joint matrix `i = inverse(world(mesh node)) · world(joint
//!   i) · inverse_bind[i]`, recomputed when a joint or the node moved. The
//!   joints place a skinned mesh's vertices, not its node (glTF): a vertex
//!   is `world · Σ w_k · J[j_k] · v`.
//! - **Culling**: each draw's world box against the view frustum: the
//!   mesh's box under its world matrix; instanced: the set's box under it;
//!   skinned: the union of the mesh's box under `world · J_i` for every
//!   joint `i` (it holds every skinned vertex: each is a weighted mean of
//!   those), recomputed with the joints.
//! - **Order**: opaque draws by (shader variant, base texture, mesh key),
//!   then transparent ones back to front (by their box's center).
//! - **Shadows**: the first directional light with `shadows`, its map
//!   fitted to the box of the visible draws (casters: the visible draws
//!   with `cast`; a simplification: casters out of view cast nothing).
//! - **Hits**: the ray against each draw's box, then its triangles,
//!   instance by instance; a skinned mesh's triangles are skinned on the
//!   CPU in the current pose (`world · Σ w_k · J[j_k] · v`; taps are rare).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::blobs::{self, InstancesBlob, SkinBlob};
use super::clip::{ClipData, ClipProperty};
use super::geometry::{MeshData, generate, ray_triangle};
use super::math::{self, Aabb, Mat4, Quat, Vec3};
use super::shaders::{MAX_LIGHTS, Variant};
use super::wire::{FlatNode, SceneSettings, SceneUpdate};
use super::{AlphaMode, Camera, LightKind, Mesh, Rgba, SceneTapEvent, SceneTransition};

/// Degrees the camera turns per point dragged.
pub const DEGREES_PER_POINT: f32 = 0.5;
/// The camera stays this many degrees away from straight up or down.
pub const MAX_PITCH: f32 = 89.;
/// A press that moves less than this (points) is a tap.
pub const TAP_SLOP: f32 = 6.;
/// Width and height of the shadow map, texels.
pub const SHADOW_MAP_SIZE: u32 = 2048;

/// The eased progress at `p` (0 to 1) of a cubic Bézier timing curve
/// `[x1, y1, x2, y2]` (CSS `cubic-bezier`): x is solved by bisection.
pub fn ease(bezier: [f32; 4], p: f32) -> f32 {
    let p = p.clamp(0., 1.);
    if p == 0. || p == 1. {
        return p;
    }
    let [x1, y1, x2, y2] = bezier;
    let at = |a: f32, b: f32, t: f32| {
        let u = 1. - t;
        3. * u * u * t * a + 3. * u * t * t * b + t * t * t
    };
    let (mut lo, mut hi) = (0f32, 1f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.;
        if at(x1, x2, mid) < p {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    at(y1, y2, (lo + hi) / 2.)
}

// --- Poses -------------------------------------------------------------------------------

/// The animatable part of a node.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    position: Vec3,
    rotation: Vec3,
    quat: Option<Quat>,
    scale: Vec3,
    color: Rgba,
    emissive: Rgba,
    metalness: f32,
    roughness: f32,
}

impl Pose {
    fn of(n: &FlatNode) -> Pose {
        Pose {
            position: n.position,
            rotation: n.rotation,
            quat: n.quaternion,
            scale: n.scale,
            color: n.material.color,
            emissive: n.material.emissive.unwrap_or(Rgba {
                light: [0., 0., 0., 1.],
                dark: None,
            }),
            metalness: n.material.metalness,
            roughness: n.material.roughness,
        }
    }

    fn lerp(&self, to: &Pose, t: f32) -> Pose {
        let (rotation, quat) = match (self.quat, to.quat) {
            // Degrees, like v1 and 2D `rotate`: every degree in between.
            (None, None) => (math::lerp3(self.rotation, to.rotation, t), None),
            (a, b) => {
                let a = a.unwrap_or_else(|| math::quat_from_euler(self.rotation));
                let b = b.unwrap_or_else(|| math::quat_from_euler(to.rotation));
                (to.rotation, Some(math::slerp(a, b, t)))
            }
        };
        Pose {
            position: math::lerp3(self.position, to.position, t),
            rotation,
            quat,
            scale: math::lerp3(self.scale, to.scale, t),
            color: lerp_rgba(&self.color, &to.color, t),
            emissive: lerp_rgba(&self.emissive, &to.emissive, t),
            metalness: math::lerp(self.metalness, to.metalness, t),
            roughness: math::lerp(self.roughness, to.roughness, t),
        }
    }
}

fn lerp_rgba(a: &Rgba, b: &Rgba, t: f32) -> Rgba {
    let mix = |x: [f32; 4], y: [f32; 4]| std::array::from_fn(|i| math::lerp(x[i], y[i], t));
    Rgba {
        light: mix(a.light, b.light),
        dark: (a.dark.is_some() || b.dark.is_some()).then(|| mix(a.resolve(true), b.resolve(true))),
    }
}

/// What clips set on a node (each part replaces the node's own).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ClipPose {
    translation: Option<Vec3>,
    rotation: Option<Quat>,
    scale: Option<Vec3>,
}

// --- Clips -------------------------------------------------------------------------------

/// A clip playing on a node.
#[derive(Clone, Debug)]
struct Track {
    clip: Rc<ClipData>,
    /// Clip time at `anchor`.
    base: f32,
    anchor: f64,
    speed: f32,
    looping: bool,
    /// The node each channel animates (resolved after every update).
    targets: Vec<Option<usize>>,
}

impl Track {
    fn new(clip: Rc<ClipData>, now: f64, speed: f32, looping: bool) -> Track {
        Track {
            base: if speed < 0. { clip.duration } else { 0. },
            clip,
            anchor: now,
            speed,
            looping,
            targets: Vec::new(),
        }
    }

    /// Unbounded clip time at `now`.
    fn raw_time(&self, now: f64) -> f32 {
        self.base + ((now - self.anchor) as f32) * self.speed
    }

    /// The time to sample at `now`.
    fn time(&self, now: f64) -> f32 {
        let d = self.clip.duration;
        let t = self.raw_time(now);
        if d <= 0. {
            0.
        } else if self.looping {
            t.rem_euclid(d)
        } else {
            t.clamp(0., d)
        }
    }

    /// Still moving at `now` (looping, or not past its end).
    fn running(&self, now: f64) -> bool {
        let t = self.raw_time(now);
        self.clip.duration > 0.
            && self.speed != 0.
            && (self.looping
                || (self.speed > 0. && t < self.clip.duration)
                || (self.speed < 0. && t > 0.))
    }

    /// Changes the rate without a jump in clip time.
    fn set_speed(&mut self, speed: f32, now: f64) {
        if speed != self.speed {
            self.base = self.raw_time(now);
            self.anchor = now;
            self.speed = speed;
        }
    }

    /// Adds this clip's values at `now` to `out` (by target node).
    fn sample(&self, now: f64, out: &mut HashMap<usize, ClipPose>) {
        let t = self.time(now);
        for (c, target) in self.clip.channels.iter().zip(&self.targets) {
            let Some(target) = target else { continue };
            let v = c.sample(t);
            let pose = out.entry(*target).or_default();
            match c.property {
                ClipProperty::Translation => pose.translation = Some([v[0], v[1], v[2]]),
                ClipProperty::Rotation => pose.rotation = Some(v),
                ClipProperty::Scale => pose.scale = Some([v[0], v[1], v[2]]),
            }
        }
    }
}

/// The clips of a node with `play`.
#[derive(Clone, Debug)]
struct Playing {
    /// The clip playing (`None` while its blob has not arrived).
    current: Option<Track>,
    /// The clip blending out: (track, fade start, fade seconds).
    previous: Option<(Track, f64, f32)>,
    /// It moved at the last refresh: sample once more after it stops, so
    /// the last pose is exact.
    live: bool,
}

// --- Frame -------------------------------------------------------------------------------

/// The camera as an orbit around its target.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Orbit {
    /// Degrees around Y; 0: on +Z of the target.
    yaw: f32,
    /// Degrees above the horizon.
    pitch: f32,
    distance: f32,
    target: Vec3,
    fov: f32,
    /// Visible height of an orthographic camera.
    ortho: Option<f32>,
}

impl Orbit {
    fn of(c: &Camera) -> Orbit {
        let o = math::sub(c.position, c.target);
        let d = math::length(o).max(1e-4);
        Orbit {
            yaw: o[0].atan2(o[2]).to_degrees(),
            pitch: (o[1] / d).clamp(-1., 1.).asin().to_degrees(),
            distance: d,
            target: c.target,
            fov: c.fov,
            ortho: c.ortho,
        }
    }

    fn eye(&self) -> Vec3 {
        let (y, p) = (self.yaw.to_radians(), self.pitch.to_radians());
        math::add(
            self.target,
            [
                self.distance * p.cos() * y.sin(),
                self.distance * p.sin(),
                self.distance * p.cos() * y.cos(),
            ],
        )
    }

    fn lerp(&self, to: &Orbit, t: f32) -> Orbit {
        // The shorter way around.
        let dyaw = (to.yaw - self.yaw + 540.).rem_euclid(360.) - 180.;
        Orbit {
            yaw: self.yaw + dyaw * t,
            pitch: math::lerp(self.pitch, to.pitch, t),
            distance: math::lerp(self.distance, to.distance, t),
            target: math::lerp3(self.target, to.target, t),
            fov: math::lerp(self.fov, to.fov, t),
            ortho: match (self.ortho, to.ortho) {
                (Some(a), Some(b)) => Some(math::lerp(a, b, t)),
                _ => to.ortho,
            },
        }
    }
}

/// One light of a frame, ready for the shader's uniforms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameLight {
    /// 1 directional, 2 point, 3 spot (`u_light_kind`).
    pub kind: i32,
    /// Linear color × intensity.
    pub color: Vec3,
    /// World position (point, spot).
    pub position: Vec3,
    /// Unit direction the light travels (directional, spot).
    pub direction: Vec3,
    /// Range (0: none), cos of the outer half-angle, cos of the inner one, 0.
    pub params: [f32; 4],
}

/// A material resolved for one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawMaterial<'a> {
    /// Linear rgb and alpha (alpha 1 for [`AlphaMode::Opaque`]).
    pub color: [f32; 4],
    /// Linear.
    pub emissive: Vec3,
    /// 0 to 1.
    pub metalness: f32,
    /// 0 to 1.
    pub roughness: f32,
    /// Flat color, no lighting.
    pub unlit: bool,
    /// No back-face culling.
    pub double_sided: bool,
    /// Base-color image source (sRGB).
    pub texture: Option<&'a str>,
    /// Metalness (B) and roughness (G) image source.
    pub mr_map: Option<&'a str>,
    /// Normal map source.
    pub normal_map: Option<&'a str>,
    /// Normal map strength.
    pub normal_scale: f32,
    /// Emission image source (sRGB).
    pub emissive_map: Option<&'a str>,
    /// Ambient occlusion image source (R).
    pub ao_map: Option<&'a str>,
    /// How alpha is used (`u_alpha_mode`: 0 blend, 1 mask, 2 opaque).
    pub alpha: AlphaMode,
    /// Masked: the alpha below which nothing is drawn.
    pub alpha_cutoff: f32,
}

/// An instanced draw's instance set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InstanceRef<'a> {
    /// The `I` blob (upload its transforms as the `a_instance` attribute).
    pub id: &'a str,
    /// How many to draw.
    pub count: u32,
}

/// One mesh to draw.
#[derive(Clone, Debug)]
pub struct Draw<'a> {
    /// The node's row in the player ([`ScenePlayer::node`]).
    pub node: usize,
    /// The node's key (see [`wire`](super::wire)).
    pub key: &'a str,
    /// The nearest named ancestor's name (itself included).
    pub name: Option<&'a str>,
    /// What to draw.
    pub mesh: &'a Mesh,
    /// [`Mesh::key`]: cache GPU buffers by it.
    pub mesh_key: &'a str,
    /// Model matrix (scene coordinates).
    pub world: Mat4,
    /// Normal matrix (column-major 3×3).
    pub normal: [f32; 9],
    /// How it looks.
    pub material: DrawMaterial<'a>,
    /// Alpha below 1 (blend mode): drawn after the opaque meshes, blended,
    /// without writing depth.
    pub transparent: bool,
    /// Drawn once per instance.
    pub instances: Option<InstanceRef<'a>>,
    /// Skinned: one matrix per joint (`u_bones`).
    pub joints: Option<&'a [Mat4]>,
    /// Shadows others (when the frame has a shadow pass).
    pub cast_shadows: bool,
    /// Gets shadows (`u_receive_shadows`).
    pub receive_shadows: bool,
    /// Its box in scene coordinates.
    pub bounds: Aabb,
}

impl Draw<'_> {
    /// The shader variant it needs (without `depth_only`).
    pub fn variant(&self) -> Variant {
        Variant {
            instanced: self.instances.is_some(),
            skinned: self.joints.is_some(),
            depth_only: false,
        }
    }
}

/// The shadow map of a frame: render the casters, depth only, from the
/// light, before the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowPass {
    /// The light casting it: an index into [`Frame::lights`]
    /// (`u_shadow_light`).
    pub light: usize,
    /// The light's `projection · view` (orthographic): the depth pass's
    /// `u_view_proj` and the main pass's `u_shadow_view_proj`.
    pub view_proj: Mat4,
    /// Width and height of the depth texture.
    pub map_size: u32,
    /// Subtracted from the depth before comparing (`u_shadow_bias`, in
    /// the map's 0–1 depth).
    pub bias: f32,
    /// World distance the shaded point moves along its normal before the
    /// lookup (`u_shadow_normal_bias`).
    pub normal_bias: f32,
    /// The draws to render into the map: indices into [`Frame::draws`].
    pub casters: Vec<usize>,
}

/// The environment of a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameEnv<'a> {
    /// The `E` blob.
    pub id: &'a str,
    /// Multiplies its light (`u_env_intensity`).
    pub intensity: f32,
    /// Drawn behind the scene (the sky program), after clearing.
    pub background: bool,
}

/// Everything one frame draws.
#[derive(Clone, Debug)]
pub struct Frame<'a> {
    /// Viewport, points.
    pub width: f32,
    /// Viewport, points.
    pub height: f32,
    /// The camera's view matrix.
    pub view: Mat4,
    /// Its projection.
    pub projection: Mat4,
    /// `projection · view`.
    pub view_proj: Mat4,
    /// Its inverse (the sky program's `u_inverse_view_proj`).
    pub inverse_view_proj: Mat4,
    /// Where the camera is.
    pub camera_position: Vec3,
    /// Clear color, premultiplied (`[0; 4]`: transparent).
    pub clear: [f32; 4],
    /// The ambient lights, summed (linear × intensity).
    pub ambient: Vec3,
    /// The other lights (at most [`MAX_LIGHTS`]).
    pub lights: Vec<FrameLight>,
    /// Opaque meshes, then transparent ones back to front; culled ones left out.
    pub draws: Vec<Draw<'a>>,
    /// The shadow map to render first, if a light casts shadows.
    pub shadow: Option<ShadowPass>,
    /// Image-based lighting.
    pub env: Option<FrameEnv<'a>>,
    /// Tone-mapping exposure (`u_exposure`).
    pub exposure: f32,
    /// Something moves: draw another frame.
    pub animating: bool,
}

// --- The player --------------------------------------------------------------------------

/// A row of the node table.
#[derive(Clone, Debug)]
struct Node {
    flat: FlatNode,
    parent: Option<usize>,
    /// Its subtree is `index..end`.
    end: usize,
    children: Vec<usize>,
    /// The nearest named node, itself included.
    named: Option<usize>,
    /// The nearest labeled node up to the named one.
    part: Option<usize>,
    /// Neither it nor an ancestor is hidden.
    visible: bool,
    mesh_key: String,
    /// When it appeared (its spin starts there).
    appeared: f64,
    /// Its pose in the latest update.
    target: Pose,
    /// A running transition: from this pose, starting at this time.
    from: Option<(Pose, f64)>,
    /// What clips set on it this frame.
    clip_pose: Option<ClipPose>,
    playing: Option<Playing>,
    /// The joint nodes of its skin (resolved after every update).
    joint_nodes: Vec<Option<usize>>,
    /// Its joints' matrices.
    joints: Vec<Mat4>,
    local: Mat4,
    world: Mat4,
    normal: [f32; 9],
    /// Its draw's box in scene coordinates.
    bounds: Aabb,
    /// The tick its world last changed.
    moved: u64,
    /// Its local matrix needs recomputing.
    dirty: bool,
}

impl Node {
    fn new(flat: FlatNode, now: f64) -> Node {
        Node {
            mesh_key: flat.mesh.as_ref().map(Mesh::key).unwrap_or_default(),
            target: Pose::of(&flat),
            flat,
            parent: None,
            end: 0,
            children: Vec::new(),
            named: None,
            part: None,
            visible: true,
            appeared: now,
            from: None,
            clip_pose: None,
            playing: None,
            joint_nodes: Vec::new(),
            joints: Vec::new(),
            local: math::IDENTITY,
            world: math::IDENTITY,
            normal: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
            bounds: [0.; 6],
            moved: 0,
            dirty: true,
        }
    }
}

/// The device side of one scene element (see the module docs).
#[derive(Debug)]
pub struct ScenePlayer {
    settings: SceneSettings,
    /// No update applied yet: the first one does not animate.
    fresh: bool,
    nodes: Vec<Node>,
    index: HashMap<String, usize>,
    /// Visible nodes with a mesh.
    draw_nodes: Vec<usize>,
    /// Nodes with a running transition.
    transitioning: Vec<usize>,
    /// Visible nodes that spin.
    spinning: Vec<usize>,
    /// Nodes playing clips.
    players: Vec<usize>,
    /// Visible nodes with a resolved skin.
    skinned: Vec<usize>,
    orbit: Orbit,
    camera_from: Option<(Orbit, f64)>,
    min_distance: f32,
    max_distance: f32,
    now: f64,
    last: Option<f64>,
    dragging: bool,
    /// Something but the camera moves (spin, transition, clip).
    moving: bool,
    tick: u64,
    clips: HashMap<String, Rc<ClipData>>,
    skins: HashMap<String, Rc<SkinBlob>>,
    instances: HashMap<String, Rc<Instanced>>,
    geometry: RefCell<HashMap<String, Rc<MeshData>>>,
}

/// A decoded instance set and its matrices.
#[derive(Debug)]
struct Instanced {
    matrices: Vec<Mat4>,
}

impl ScenePlayer {
    /// A player showing nothing until its first [`apply`](Self::apply).
    pub fn new(now: f64) -> Self {
        let settings = SceneSettings::default();
        ScenePlayer {
            orbit: Orbit::of(&settings.camera),
            settings,
            fresh: true,
            nodes: Vec::new(),
            index: HashMap::new(),
            draw_nodes: Vec::new(),
            transitioning: Vec::new(),
            spinning: Vec::new(),
            players: Vec::new(),
            skinned: Vec::new(),
            camera_from: None,
            min_distance: 0.,
            max_distance: 0.,
            now,
            last: None,
            dragging: false,
            moving: false,
            tick: 0,
            clips: HashMap::new(),
            skins: HashMap::new(),
            instances: HashMap::new(),
            geometry: RefCell::new(HashMap::new()),
        }
    }

    /// The scene's settings.
    pub fn settings(&self) -> &SceneSettings {
        &self.settings
    }

    /// The nodes, in pre-order.
    pub fn nodes(&self) -> impl Iterator<Item = &FlatNode> {
        self.nodes.iter().map(|n| &n.flat)
    }

    /// Row `i` of the table (a [`Draw::node`]).
    pub fn node(&self, i: usize) -> Option<&FlatNode> {
        self.nodes.get(i).map(|n| &n.flat)
    }

    /// Shows an update at `now`: changed nodes and camera animate from where
    /// they are to their new values if the scene has a transition; a changed
    /// camera replaces where the user moved it. Clips whose `play` changed
    /// start (or crossfade) here.
    pub fn apply(&mut self, update: &SceneUpdate, now: f64) {
        self.now = now;
        if let Some(s) = update.settings() {
            self.set_settings(s.clone(), now);
        }
        let t = if self.fresh {
            None
        } else {
            self.settings.transition
        };
        self.fresh = false;

        let old_order: Vec<String> = self.nodes.iter().map(|n| n.flat.key.clone()).collect();
        let mut rows: HashMap<String, Node> = std::mem::take(&mut self.nodes)
            .into_iter()
            .map(|n| (n.flat.key.clone(), n))
            .collect();
        // Keys in their new order (removed ones simply find no row).
        let order: Vec<String> = match update {
            SceneUpdate::Full(scene) => scene.nodes.iter().map(|n| n.key.clone()).collect(),
            SceneUpdate::Diff(d) => {
                for k in &d.remove {
                    rows.remove(k);
                }
                d.order.clone().unwrap_or_else(|| {
                    old_order
                        .into_iter()
                        .chain(d.upsert.iter().map(|n| n.key.clone()))
                        .collect()
                })
            }
        };
        for flat in update.nodes() {
            match rows.get_mut(&flat.key) {
                Some(row) => self.update_row(row, flat, now, t),
                None => {
                    let mut row = Node::new(flat.clone(), now);
                    self.start_play(&mut row, None, now);
                    rows.insert(flat.key.clone(), row);
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        self.nodes = order
            .iter()
            .filter(|k| seen.insert(k.as_str()))
            .filter_map(|k| rows.remove(k))
            .collect();
        self.relink();
        self.refresh(now, true);
    }

    fn set_settings(&mut self, s: SceneSettings, now: f64) {
        if self.fresh {
            self.orbit = Orbit::of(&s.camera);
            self.set_limits(&s);
        } else if s.camera != self.settings.camera {
            let to = Orbit::of(&s.camera);
            let from = self.current_orbit(now);
            self.orbit = to;
            self.camera_from = s
                .transition
                .filter(|t| t.ms > 0.)
                .map(|t| (from, now + t.delay as f64 / 1000.));
            self.set_limits(&s);
        } else if s.orbit != self.settings.orbit {
            self.set_limits(&s);
        }
        self.settings = s;
    }

    fn set_limits(&mut self, s: &SceneSettings) {
        let d = Orbit::of(&s.camera).distance;
        let o = s.orbit.unwrap_or_default();
        self.min_distance = if o.min > 0. { o.min } else { d * 0.25 };
        self.max_distance = if o.max > 0. { o.max } else { d * 4. };
    }

    /// A node of the update replaces its row's props.
    fn update_row(&self, row: &mut Node, flat: &FlatNode, now: f64, t: Option<SceneTransition>) {
        let pose = Pose::of(flat);
        if pose != row.target {
            let current = self.pose_at(row, now);
            row.from = t
                .filter(|t| t.ms > 0. && current != pose)
                .map(|t| (current, now + t.delay as f64 / 1000.));
            row.target = pose;
        }
        if flat.mesh != row.flat.mesh {
            row.mesh_key = flat.mesh.as_ref().map(Mesh::key).unwrap_or_default();
        }
        let old = std::mem::replace(&mut row.flat, flat.clone());
        self.start_play(row, old.play.as_ref(), now);
    }

    /// Starts, restarts, crossfades, retimes or stops a row's clip after
    /// its `play` changed from `old`.
    fn start_play(&self, row: &mut Node, old: Option<&super::wire::PlayRef>, now: f64) {
        let Some(p) = &row.flat.play else {
            row.playing = None;
            return;
        };
        let clip = self.clip(p.clip.id(), &p.clip);
        let same = old.is_some_and(|o| o.clip == p.clip && o.run == p.run);
        match &mut row.playing {
            Some(playing) if same => {
                if let Some(track) = &mut playing.current {
                    track.set_speed(p.speed, now);
                    track.looping = p.looping;
                }
            }
            _ => {
                let previous = match row.playing.take() {
                    Some(old) if p.fade > 0. => old.current.map(|t| (t, now, p.fade)),
                    _ => None,
                };
                row.playing = Some(Playing {
                    current: clip.map(|c| Track::new(c, now, p.speed, p.looping)),
                    previous,
                    live: true,
                });
            }
        }
    }

    /// The decoded clip `id` (from the cache, or its blob).
    fn clip(&self, id: &str, blob: &blobs::BlobRef) -> Option<Rc<ClipData>> {
        if let Some(c) = self.clips.get(id) {
            return Some(c.clone());
        }
        blobs::decode_clip(&blob.bytes()?).map(Rc::new)
    }

    /// Rebuilds the table's links after an update: parents, subtrees,
    /// named and labeled ancestors, visibility, the lists of moving nodes,
    /// clip targets and skin joints. O(nodes).
    fn relink(&mut self) {
        // Pre-order from the parent keys, whatever order the rows came in
        // (rows whose parent is missing go to the top level).
        let keys: HashMap<&str, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.flat.key.as_str(), i))
            .collect();
        let mut kids: Vec<Vec<usize>> = vec![Vec::new(); self.nodes.len()];
        let mut roots = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            match n.flat.parent.as_deref().and_then(|p| keys.get(p)) {
                Some(&p) if p != i => kids[p].push(i),
                _ => roots.push(i),
            }
        }
        let mut order = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<usize> = roots.into_iter().rev().collect();
        let mut placed = vec![false; self.nodes.len()];
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut placed[i], true) {
                continue;
            }
            order.push(i);
            stack.extend(kids[i].iter().rev());
        }
        drop(keys);
        if order.len() != self.nodes.len() || order.iter().enumerate().any(|(a, b)| a != *b) {
            let mut rows: Vec<Option<Node>> = std::mem::take(&mut self.nodes)
                .into_iter()
                .map(Some)
                .collect();
            self.nodes = order.iter().filter_map(|i| rows[*i].take()).collect();
        }

        self.index = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.flat.key.clone(), i))
            .collect();
        let n = self.nodes.len();
        for i in 0..n {
            let parent = self.nodes[i]
                .flat
                .parent
                .as_deref()
                .and_then(|p| self.index.get(p).copied())
                .filter(|p| *p < i);
            let (named, part, visible) = match parent {
                Some(p) => {
                    let pn = &self.nodes[p];
                    (pn.named, pn.part, pn.visible)
                }
                None => (None, None, true),
            };
            let row = &mut self.nodes[i];
            row.parent = parent;
            row.end = i + 1;
            row.children.clear();
            row.named = if row.flat.name.is_some() {
                Some(i)
            } else {
                named
            };
            row.part = if row.flat.label.is_some() {
                Some(i)
            } else if row.flat.name.is_some() {
                None
            } else {
                part
            };
            row.visible = visible && !row.flat.hidden;
            row.clip_pose = None;
            row.dirty = true;
        }
        for i in (0..n).rev() {
            if let Some(p) = self.nodes[i].parent {
                let end = self.nodes[i].end;
                self.nodes[p].end = self.nodes[p].end.max(end);
            }
        }
        for i in 0..n {
            if let Some(p) = self.nodes[i].parent {
                self.nodes[p].children.push(i);
            }
        }

        // Blobs in use, decoded once.
        let mut clips = HashMap::new();
        let mut skins = HashMap::new();
        let mut instances = HashMap::new();
        for row in &self.nodes {
            if let Some(p) = &row.flat.play
                && let Some(c) = self.clip(p.clip.id(), &p.clip)
            {
                clips.insert(p.clip.id().to_string(), c);
            }
            if let Some(s) = &row.flat.skin {
                let id = s.id.id();
                let skin = self
                    .skins
                    .get(id)
                    .cloned()
                    .or_else(|| blobs::decode_skin(&s.id.bytes()?).map(Rc::new));
                if let Some(skin) = skin {
                    skins.insert(id.to_string(), skin);
                }
            }
            if let Some(inst) = &row.flat.instances {
                let id = inst.id.id();
                let set = self.instances.get(id).cloned().or_else(|| {
                    let b: InstancesBlob = blobs::decode_instances(&inst.id.bytes()?)?;
                    Some(Rc::new(Instanced {
                        matrices: b.transforms.iter().map(|t| t.matrix()).collect(),
                    }))
                });
                if let Some(set) = set {
                    instances.insert(id.to_string(), set);
                }
            }
        }
        self.clips = clips;
        self.skins = skins;
        self.instances = instances;
        let keys: std::collections::HashSet<&str> =
            self.nodes.iter().map(|n| n.mesh_key.as_str()).collect();
        self.geometry
            .borrow_mut()
            .retain(|k, _| keys.contains(k.as_str()));

        // Clip targets and skin joints.
        for i in 0..n {
            if let Some(playing) = self.nodes[i].playing.take() {
                let mut playing = playing;
                for track in playing
                    .current
                    .iter_mut()
                    .chain(playing.previous.as_mut().map(|p| &mut p.0))
                {
                    track.targets = track
                        .clip
                        .channels
                        .iter()
                        .map(|c| self.resolve(i, &c.target))
                        .collect();
                }
                self.nodes[i].playing = Some(playing);
            }
            let joints = match &self.nodes[i].flat.skin {
                Some(s) => match (self.skins.get(s.id.id()), self.index.get(&s.root)) {
                    (Some(skin), Some(&root)) => {
                        skin.joints.iter().map(|p| self.resolve(root, p)).collect()
                    }
                    _ => Vec::new(),
                },
                None => Vec::new(),
            };
            self.nodes[i].joint_nodes = joints;
        }

        self.draw_nodes = (0..n)
            .filter(|i| self.nodes[*i].visible && self.nodes[*i].flat.mesh.is_some())
            .collect();
        self.spinning = (0..n)
            .filter(|i| self.nodes[*i].visible && self.nodes[*i].flat.spin != [0.; 3])
            .collect();
        self.transitioning = (0..n).filter(|i| self.nodes[*i].from.is_some()).collect();
        self.players = (0..n)
            .filter(|i| self.nodes[*i].playing.is_some())
            .collect();
        self.skinned = (0..n)
            .filter(|i| self.nodes[*i].visible && !self.nodes[*i].joint_nodes.is_empty())
            .collect();
    }

    /// The node a clip target or joint path names, from node `from`: `""`
    /// itself, `"0/2"` a child-index path, `"#name"` a named node.
    fn resolve(&self, from: usize, target: &str) -> Option<usize> {
        if target.starts_with('#') {
            return self.index.get(target).copied();
        }
        let mut at = from;
        for step in target.split('/').filter(|s| !s.is_empty()) {
            at = *self.nodes[at].children.get(step.parse::<usize>().ok()?)?;
        }
        Some(at)
    }

    fn transition_progress(&self, start: f64, now: f64) -> Option<f32> {
        let t = self.settings.transition?;
        if t.ms <= 0. {
            return None;
        }
        let p = ((now - start) * 1000. / t.ms as f64) as f32;
        (p < 1.).then(|| ease(t.bezier, p.max(0.)))
    }

    fn pose_at(&self, n: &Node, now: f64) -> Pose {
        match n.from {
            Some((from, start)) => match self.transition_progress(start, now) {
                Some(p) => from.lerp(&n.target, p),
                None => n.target,
            },
            None => n.target,
        }
    }

    fn current_orbit(&self, now: f64) -> Orbit {
        match self.camera_from {
            Some((from, start)) => match self.transition_progress(start, now) {
                Some(p) => from.lerp(&self.orbit, p),
                None => self.orbit,
            },
            None => self.orbit,
        }
    }

    /// The finger or the mouse button went down (auto-rotation pauses).
    pub fn pointer_down(&mut self) {
        self.dragging = true;
    }

    /// The pointer moved by `dx`, `dy` points while down: turns the camera
    /// (if the orbit allows it). Dragging right turns the scene to the
    /// right; dragging down shows more of its top.
    pub fn drag(&mut self, dx: f32, dy: f32) {
        if !self.settings.orbit.is_some_and(|o| o.rotate) {
            return;
        }
        self.settle_camera();
        self.orbit.yaw -= dx * DEGREES_PER_POINT;
        self.orbit.pitch = (self.orbit.pitch + dy * DEGREES_PER_POINT).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// The pointer went up.
    pub fn pointer_up(&mut self) {
        self.dragging = false;
    }

    /// Pinch or scroll: `factor` above 1 moves closer.
    pub fn zoom(&mut self, factor: f32) {
        if !self.settings.orbit.is_some_and(|o| o.zoom) || factor.is_nan() || factor <= 0. {
            return;
        }
        self.settle_camera();
        match &mut self.orbit.ortho {
            Some(h) => *h /= factor,
            None => {
                self.orbit.distance =
                    (self.orbit.distance / factor).clamp(self.min_distance, self.max_distance)
            }
        }
    }

    /// Two-finger drag: moves the target with the content (`height`: the
    /// element's height in points).
    pub fn pan(&mut self, dx: f32, dy: f32, height: f32) {
        if !self.settings.orbit.is_some_and(|o| o.pan) || height <= 0. {
            return;
        }
        self.settle_camera();
        let eye = self.orbit.eye();
        let f = math::normalize(math::sub(self.orbit.target, eye));
        let right = math::normalize(math::cross(f, [0., 1., 0.]));
        let up = math::cross(right, f);
        let visible = match self.orbit.ortho {
            Some(h) => h,
            None => 2. * self.orbit.distance * (self.orbit.fov.to_radians() / 2.).tan(),
        };
        let k = visible / height;
        let moved = math::add(math::scale(right, -dx * k), math::scale(up, dy * k));
        self.orbit.target = math::add(self.orbit.target, moved);
    }

    /// A gesture takes over from a running camera transition.
    fn settle_camera(&mut self) {
        if self.camera_from.is_some() {
            self.orbit = self.current_orbit(self.now);
            self.camera_from = None;
        }
    }

    /// Moves time to `now`: auto-rotation turns by the time since the last
    /// frame; spins, transitions and clips move the nodes.
    pub fn advance(&mut self, now: f64) {
        let dt = self.last.map_or(0., |l| (now - l).clamp(0., 0.25)) as f32;
        self.last = Some(now);
        self.now = now;
        if let Some(o) = self.settings.orbit
            && o.auto != 0.
            && !self.dragging
        {
            self.orbit.yaw += o.auto * dt;
            if let Some((from, _)) = &mut self.camera_from {
                from.yaw += o.auto * dt;
            }
        }
        if let Some((_, start)) = self.camera_from
            && now >= start
            && self.transition_progress(start, now).is_none()
        {
            self.camera_from = None;
        }
        self.refresh(now, false);
    }

    /// Recomputes what moved at `now`: clip poses, then the local matrices
    /// of moving nodes and the worlds of their subtrees, then skins. `all`:
    /// every node (after an update).
    fn refresh(&mut self, now: f64, all: bool) {
        self.tick += 1;
        let tick = self.tick;
        let mut moving = false;
        // Rows whose local pose changes now (each frame: the moving ones only).
        let mut touched: Vec<usize> = Vec::new();

        // Fades that are over: the nodes only the old clip moved go back
        // to their own pose.
        for &p in &self.players {
            let Some(playing) = &mut self.nodes[p].playing else {
                continue;
            };
            let over = playing
                .previous
                .as_ref()
                .is_some_and(|(_, start, fade)| now - start >= *fade as f64);
            if over && let Some((track, ..)) = playing.previous.take() {
                playing.live = true;
                for t in track.targets.iter().flatten() {
                    self.nodes[*t].clip_pose = None;
                    self.nodes[*t].dirty = true;
                    touched.push(*t);
                }
            }
        }
        // Clips: sample every running track (and, after an update, the
        // finished ones too: their last pose holds).
        let mut poses: HashMap<usize, ClipPose> = HashMap::new();
        let mut live = Vec::new();
        for &p in &self.players {
            let Some(playing) = &self.nodes[p].playing else {
                continue;
            };
            let running = playing.previous.is_some()
                || playing.current.as_ref().is_some_and(|t| t.running(now));
            live.push((p, running));
            if !running && !playing.live && !all {
                continue;
            }
            moving |= running;
            let mut new = HashMap::new();
            if let Some(t) = &playing.current {
                t.sample(now, &mut new);
            }
            match &playing.previous {
                Some((old_track, start, fade)) => {
                    let w = (((now - start) / *fade as f64) as f32).clamp(0., 1.);
                    let mut old = HashMap::new();
                    old_track.sample(now, &mut old);
                    for (target, a) in old {
                        let b = new.remove(&target).unwrap_or_default();
                        poses.insert(target, self.blend(target, a, b, w, now));
                    }
                    for (target, b) in new {
                        poses.insert(target, self.blend(target, ClipPose::default(), b, w, now));
                    }
                }
                None => poses.extend(new),
            }
        }
        for (p, running) in live {
            if let Some(playing) = &mut self.nodes[p].playing {
                playing.live = running;
            }
        }
        for (target, pose) in poses {
            let row = &mut self.nodes[target];
            if row.clip_pose != Some(pose) {
                row.clip_pose = Some(pose);
                row.dirty = true;
                touched.push(target);
            }
        }

        // Spins and transitions.
        for &i in &self.spinning {
            self.nodes[i].dirty = true;
            touched.push(i);
            moving = true;
        }
        let mut done = Vec::new();
        for &i in &self.transitioning {
            self.nodes[i].dirty = true;
            touched.push(i);
            match self.nodes[i].from {
                Some((_, start))
                    if now < start || self.transition_progress(start, now).is_some() =>
                {
                    moving = true
                }
                _ => done.push(i),
            }
        }
        if !done.is_empty() {
            for i in &done {
                self.nodes[*i].from = None;
            }
            self.transitioning.retain(|i| !done.contains(i));
        }

        // Locals, then the worlds of the subtrees of the touched rows (in
        // pre-order, a subtree is a range: one pass covers nested ones).
        if all {
            touched = (0..self.nodes.len()).collect();
        } else {
            touched.sort_unstable();
            touched.dedup();
        }
        let mut covered = 0;
        for i in touched {
            if i < covered {
                continue;
            }
            covered = self.nodes[i].end;
            for j in i..covered {
                if all || self.nodes[j].dirty {
                    self.nodes[j].local = self.local(j, now);
                    self.nodes[j].dirty = false;
                }
                self.update_world(j, tick);
            }
        }

        // Skins whose joints or node moved.
        for &s in &self.skinned {
            let row = &self.nodes[s];
            let moved = all
                || row.moved == tick
                || row
                    .joint_nodes
                    .iter()
                    .flatten()
                    .any(|j| self.nodes[*j].moved == tick);
            if !moved {
                continue;
            }
            let Some(skin) = row
                .flat
                .skin
                .as_ref()
                .and_then(|k| self.skins.get(k.id.id()))
            else {
                continue;
            };
            // Where each joint puts the mesh: world · J_i = world(joint i) ·
            // inverse_bind[i] (the node's own transform places nothing).
            let placed: Vec<Mat4> = row
                .joint_nodes
                .iter()
                .enumerate()
                .map(|(k, j)| match j {
                    Some(j) => {
                        let bind = skin.inverse_bind.get(k).unwrap_or(&math::IDENTITY);
                        math::mul(&self.nodes[*j].world, bind)
                    }
                    None => row.world,
                })
                .collect();
            let inverse = math::invert(&row.world);
            let joints: Vec<Mat4> = placed.iter().map(|p| math::mul(&inverse, p)).collect();
            // Every skinned vertex is a weighted mean of `world · J_i · v`, so
            // the union of the mesh box under each `world · J_i` holds them.
            let bounds = match (&row.flat.mesh, &row.flat.instances) {
                (Some(mesh), None) => {
                    let b = mesh.bounds();
                    placed
                        .iter()
                        .map(|p| math::transform_aabb(p, &b))
                        .reduce(|a, b| math::aabb_union(&a, &b))
                }
                _ => None,
            };
            let row = &mut self.nodes[s];
            row.joints = joints;
            if let Some(b) = bounds {
                row.bounds = b;
            }
        }
        self.moving = moving;
    }

    /// Blends two clips' poses of node `target` (`w`: the new one's
    /// weight); a part only one clip sets blends with the node's own pose.
    fn blend(&self, target: usize, a: ClipPose, b: ClipPose, w: f32, now: f64) -> ClipPose {
        let own = self.pose_at(&self.nodes[target], now);
        let own_q = own
            .quat
            .unwrap_or_else(|| math::quat_from_euler(own.rotation));
        let mix3 = |x: Option<Vec3>, y: Option<Vec3>, base: Vec3| {
            (x.is_some() || y.is_some())
                .then(|| math::lerp3(x.unwrap_or(base), y.unwrap_or(base), w))
        };
        ClipPose {
            translation: mix3(a.translation, b.translation, own.position),
            rotation: (a.rotation.is_some() || b.rotation.is_some())
                .then(|| math::slerp(a.rotation.unwrap_or(own_q), b.rotation.unwrap_or(own_q), w)),
            scale: mix3(a.scale, b.scale, own.scale),
        }
    }

    /// Row `i`'s local matrix at `now`.
    fn local(&self, i: usize, now: f64) -> Mat4 {
        let row = &self.nodes[i];
        let pose = self.pose_at(row, now);
        let clip = row.clip_pose.unwrap_or_default();
        let position = clip.translation.unwrap_or(pose.position);
        let scale = clip.scale.unwrap_or(pose.scale);
        let spin = row.flat.spin;
        let turned = if spin != [0.; 3] && row.visible {
            math::scale(spin, (now - row.appeared) as f32)
        } else {
            [0.; 3]
        };
        match clip.rotation.or(pose.quat) {
            Some(q) => {
                let q = if turned != [0.; 3] {
                    math::quat_mul(q, math::quat_from_euler(turned))
                } else {
                    q
                };
                math::compose_quat(position, q, scale)
            }
            None => math::compose(position, math::add(pose.rotation, turned), scale),
        }
    }

    /// Row `i`'s world matrix from its parent's, and what follows from it.
    fn update_world(&mut self, i: usize, tick: u64) {
        let world = match self.nodes[i].parent {
            Some(p) => math::mul(&self.nodes[p].world, &self.nodes[i].local),
            None => self.nodes[i].local,
        };
        let row = &mut self.nodes[i];
        if row.world != world || row.moved == 0 {
            row.world = world;
            row.moved = tick;
            row.normal = math::normal_matrix(&world);
        }
        // A skinned mesh's box follows its joints (set with them, after the
        // worlds); until they resolve, its node places it.
        let skinned = !row.joint_nodes.is_empty() && row.flat.instances.is_none();
        if let Some(mesh) = &row.flat.mesh
            && !skinned
        {
            let local = match &row.flat.instances {
                Some(inst) => inst.bounds,
                None => mesh.bounds(),
            };
            row.bounds = math::transform_aabb(&world, &local);
        }
    }

    /// The frame at the time of the last [`advance`](Self::advance), for a
    /// viewport of `width` × `height` points, in the dark appearance or not.
    pub fn frame(&self, width: f32, height: f32, dark: bool) -> Frame<'_> {
        let now = self.now;
        let cam = self.current_orbit(now);
        let eye = cam.eye();
        let aspect = if height > 0. { width / height } else { 1. };
        let c = &self.settings.camera;
        let projection = match cam.ortho {
            Some(h) => math::orthographic(h, aspect, c.near, c.far),
            None => math::perspective(cam.fov, aspect, c.near, c.far),
        };
        let view = math::look_at(eye, cam.target, [0., 1., 0.]);
        let view_proj = math::mul(&projection, &view);

        let clear = match self.settings.background {
            Some(bg) => {
                let [r, g, b, a] = bg.resolve(dark);
                [r * a, g * a, b * a, a]
            }
            None => [0.; 4],
        };
        let mut ambient = [0f32; 3];
        let mut lights = Vec::new();
        let mut shadow_light = None;
        for l in self.settings.effective_lights() {
            let [r, g, b, _] = l.color.resolve(dark);
            let color = [r, g, b].map(|c| math::srgb_to_linear(c) * l.intensity);
            let half = (l.angle.max(1.) / 2.).to_radians();
            let (kind, params) = match l.kind {
                LightKind::Ambient => {
                    ambient = math::add(ambient, color);
                    continue;
                }
                LightKind::Directional => (1, [0.; 4]),
                LightKind::Point => (2, [l.range, -1., -1., 0.]),
                LightKind::Spot => (3, [l.range, half.cos(), (half * 0.75).cos(), 0.]),
            };
            if lights.len() < MAX_LIGHTS {
                if kind == 1 && l.shadows && shadow_light.is_none() {
                    shadow_light = Some(lights.len());
                }
                let d = math::normalize(l.direction);
                lights.push(FrameLight {
                    kind,
                    color,
                    position: l.position,
                    direction: if d == [0.; 3] { [0., -1., 0.] } else { d },
                    params,
                });
            }
        }

        let planes = math::frustum_planes(&view_proj);
        let mut opaque = Vec::new();
        let mut transparent = Vec::new();
        for &i in &self.draw_nodes {
            let Some(draw) = self.draw(i, dark) else {
                continue;
            };
            if !math::aabb_in_frustum(&planes, &draw.bounds) {
                continue;
            }
            if draw.transparent {
                transparent.push(draw);
            } else {
                opaque.push(draw);
            }
        }
        // Fewer program, texture and buffer switches.
        opaque.sort_by(|a, b| {
            (a.variant(), a.material.texture, a.mesh_key).cmp(&(
                b.variant(),
                b.material.texture,
                b.mesh_key,
            ))
        });
        // Back to front: farthest from the camera first.
        let depth = |d: &Draw| {
            let b = &d.bounds;
            let center = [(b[0] + b[3]) / 2., (b[1] + b[4]) / 2., (b[2] + b[5]) / 2.];
            math::length(math::sub(center, eye))
        };
        transparent.sort_by(|a, b| depth(b).total_cmp(&depth(a)));
        opaque.extend(transparent);
        let draws = opaque;

        let shadow = shadow_light.and_then(|light| shadow_pass(light, &lights[light], &draws));
        let env = self.settings.env.as_ref().map(|e| FrameEnv {
            id: e.id.id(),
            intensity: e.intensity,
            background: e.background,
        });
        let animating = self.moving
            || self.camera_from.is_some()
            || self
                .settings
                .orbit
                .is_some_and(|o| o.auto != 0. && !self.dragging);
        Frame {
            width,
            height,
            view,
            projection,
            view_proj,
            inverse_view_proj: math::invert(&view_proj),
            camera_position: eye,
            clear,
            ambient,
            lights,
            draws,
            shadow,
            env,
            exposure: self.settings.exposure,
            animating,
        }
    }

    /// Row `i`'s draw (`None`: invisible, alpha 0).
    fn draw(&self, i: usize, dark: bool) -> Option<Draw<'_>> {
        let row = &self.nodes[i];
        let mesh = row.flat.mesh.as_ref()?;
        let m = &row.flat.material;
        let pose = self.pose_at(row, self.now);
        let [r, g, b, a] = pose.color.resolve(dark);
        let [er, eg, eb, _] = pose.emissive.resolve(dark);
        let a = if m.alpha == AlphaMode::Opaque { 1. } else { a };
        if a <= 0. {
            // Invisible: neither drawn nor hit.
            return None;
        }
        let lin = math::srgb_to_linear;
        Some(Draw {
            node: i,
            key: &row.flat.key,
            name: row.named.and_then(|n| self.nodes[n].flat.name.as_deref()),
            mesh,
            mesh_key: &row.mesh_key,
            world: row.world,
            normal: row.normal,
            material: DrawMaterial {
                color: [lin(r), lin(g), lin(b), a],
                emissive: [er, eg, eb].map(lin),
                metalness: pose.metalness,
                roughness: pose.roughness,
                unlit: m.unlit,
                double_sided: m.double_sided,
                texture: m.texture.as_deref(),
                mr_map: m.mr_map.as_deref(),
                normal_map: m.normal_map.as_deref(),
                normal_scale: m.normal_scale,
                emissive_map: m.emissive_map.as_deref(),
                ao_map: m.ao_map.as_deref(),
                alpha: m.alpha,
                alpha_cutoff: m.cutoff,
            },
            transparent: m.alpha == AlphaMode::Blend && a < 1.,
            instances: row.flat.instances.as_ref().map(|s| InstanceRef {
                id: s.id.id(),
                count: s.count,
            }),
            joints: (!row.joint_nodes.is_empty() && row.joints.len() == row.joint_nodes.len())
                .then_some(row.joints.as_slice()),
            cast_shadows: row.flat.cast_shadows,
            receive_shadows: row.flat.receive_shadows,
            bounds: row.bounds,
        })
    }

    /// The triangles of `draw` as stored (unskinned; cached by mesh key; empty while a
    /// blob mesh has not arrived).
    pub fn geometry(&self, draw: &Draw) -> Rc<MeshData> {
        let mut cache = self.geometry.borrow_mut();
        if let Some(g) = cache.get(draw.mesh_key) {
            return g.clone();
        }
        let data = Rc::new(generate(draw.mesh));
        // A blob that has not arrived is not cached: it is retried.
        if data.vertex_count() > 0 || !matches!(draw.mesh, Mesh::Blob { .. }) {
            cache.insert(draw.mesh_key.to_string(), data.clone());
        }
        data
    }

    /// What a tap at `(x, y)` (points from the top-left of the element)
    /// hits in the current frame.
    pub fn hit(&self, x: f32, y: f32, width: f32, height: f32, dark: bool) -> SceneTapEvent {
        if width <= 0. || height <= 0. {
            return SceneTapEvent::default();
        }
        let frame = self.frame(width, height, dark);
        let (nx, ny) = (2. * x / width - 1., 1. - 2. * y / height);
        let near = math::transform_point(&frame.inverse_view_proj, [nx, ny, -1.]);
        let far = math::transform_point(&frame.inverse_view_proj, [nx, ny, 1.]);
        let dir = math::sub(far, near);
        // Nearest so far: (ray parameter, draw, instance).
        let mut best: Option<(f32, usize, Option<u32>)> = None;
        let closer =
            |t: f32, best: &Option<(f32, usize, Option<u32>)>| best.is_none_or(|b| t < b.0);
        for d in &frame.draws {
            match math::ray_aabb(near, dir, &d.bounds) {
                Some(t) if closer(t, &best) => {}
                _ => continue,
            }
            let g = self.geometry(d);
            let local_box = d.mesh.bounds();
            let skin = d.joints.filter(|_| g.joints.len() == g.vertex_count() * 4);
            // `m`: where the mesh is (its world, times an instance's matrix).
            let test =
                |m: &Mat4, instance: Option<u32>, best: &mut Option<(f32, usize, Option<u32>)>| {
                    if let Some(joints) = skin {
                        // Skinned: its triangles in the current pose, in the world.
                        let placed: Vec<Mat4> = joints.iter().map(|j| math::mul(m, j)).collect();
                        let p: Vec<Vec3> = (0..g.vertex_count())
                            .map(|i| skinned_position(&g, i, &placed, m))
                            .collect();
                        for tri in g.indices.as_chunks::<3>().0 {
                            let [a, b, c] = tri.map(|i| p[i as usize]);
                            if let Some(t) = ray_triangle(near, dir, a, b, c)
                                && closer(t, best)
                            {
                                *best = Some((t, d.node, instance));
                            }
                        }
                        return;
                    }
                    let inv = math::invert(m);
                    let o = math::transform_point(&inv, near);
                    let v = math::transform_dir(&inv, dir);
                    if !math::ray_aabb(o, v, &local_box).is_some_and(|t| closer(t, best)) {
                        return;
                    }
                    for tri in g.indices.as_chunks::<3>().0 {
                        let hit = ray_triangle(
                            o,
                            v,
                            g.position(tri[0]),
                            g.position(tri[1]),
                            g.position(tri[2]),
                        );
                        if let Some(t) = hit
                            && closer(t, best)
                        {
                            *best = Some((t, d.node, instance));
                        }
                    }
                };
            match d.instances.and_then(|s| self.instances.get(s.id)) {
                Some(set) => {
                    for (k, m) in set.matrices.iter().enumerate() {
                        test(&math::mul(&d.world, m), Some(k as u32), &mut best);
                    }
                }
                None => test(&d.world, None, &mut best),
            }
        }
        match best {
            Some((t, node, instance)) => {
                let row = &self.nodes[node];
                SceneTapEvent {
                    node: row.named.and_then(|n| self.nodes[n].flat.name.clone()),
                    part: row.part.and_then(|n| self.nodes[n].flat.label.clone()),
                    instance,
                    point: Some(math::add(near, math::scale(dir, t))),
                }
            }
            None => SceneTapEvent::default(),
        }
    }
}

/// Vertex `i` of a skinned mesh in the current pose: `Σ w_k · placed[j_k] ·
/// v` (`placed[j] = world · J[j]`), like the `SKINNED` vertex shader; by
/// `world` alone if its weights are all zero.
fn skinned_position(g: &MeshData, i: usize, placed: &[Mat4], world: &Mat4) -> Vec3 {
    let v = g.position(i as u32);
    let mut out = [0f32; 3];
    let mut total = 0.;
    for k in 0..4 {
        let w = g.weights.get(i * 4 + k).copied().unwrap_or(0.);
        let Some(m) = placed.get(g.joints[i * 4 + k] as usize) else {
            continue;
        };
        if w != 0. {
            out = math::add(out, math::scale(math::transform_point(m, v), w));
            total += w;
        }
    }
    if total == 0. {
        math::transform_point(world, v)
    } else {
        out
    }
}

/// The shadow map fitted to the visible draws, seen along `light`.
fn shadow_pass(index: usize, light: &FrameLight, draws: &[Draw]) -> Option<ShadowPass> {
    let union = draws
        .iter()
        .map(|d| d.bounds)
        .reduce(|a, b| math::aabb_union(&a, &b))?;
    let casters: Vec<usize> = (0..draws.len())
        .filter(|i| draws[*i].cast_shadows)
        .collect();
    if casters.is_empty() {
        return None;
    }
    let center = [
        (union[0] + union[3]) / 2.,
        (union[1] + union[4]) / 2.,
        (union[2] + union[5]) / 2.,
    ];
    let radius = math::length([
        union[3] - union[0],
        union[4] - union[1],
        union[5] - union[2],
    ]) / 2.;
    // The radius in steps of 1/16 of its power of two, and the center on
    // whole texels: the map does not shimmer while the scene stays put.
    let step = 2f32.powf(radius.max(1e-3).log2().ceil() - 4.);
    let radius = (radius.max(1e-3) / step).ceil() * step;
    let size = SHADOW_MAP_SIZE as f32;
    let texel = 2. * radius / size;
    let dir = light.direction;
    let up = if dir[1].abs() > 0.99 {
        [0., 0., 1.]
    } else {
        [0., 1., 0.]
    };
    let rotate = math::look_at([0.; 3], dir, up);
    let c = math::transform_point(&rotate, center);
    let snap = |v: f32| (v / texel).floor() * texel;
    let view = math::mul(&math::translation([-snap(c[0]), -snap(c[1]), 0.]), &rotate);
    // One texel of margin for the snap; depth around the center (view −Z).
    let half = radius + texel;
    let projection = math::orthographic(2. * half, 1., -c[2] - radius, -c[2] + radius);
    Some(ShadowPass {
        light: index,
        view_proj: math::mul(&projection, &view),
        map_size: SHADOW_MAP_SIZE,
        bias: 1.5 / size,
        normal_bias: 1.5 * texel,
        casters,
    })
}
