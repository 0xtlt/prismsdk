//! 3D scenes: meshes, materials, lights and a camera, described in Rust and
//! drawn by the GPU on every backend (SceneKit on Apple platforms, WebGL 2
//! on the web, OpenGL ES 3 on Android, OpenGL in a `GtkGLArea` on Linux).
//!
//! ```ignore
//! use prism::scene3d::*;
//!
//! scene3d()
//!     .camera(Camera::perspective(45.).position(0., 2., 6.))
//!     .orbit(Orbit::new().auto_rotate(20.))         // drag to turn, pinch or scroll to zoom
//!     .environment(Environment::studio())           // image-based lighting
//!     .light(Light::directional(rgb(0xffffff), 1.).direction(-1., -2., -1.).shadows(true))
//!     .child(cube(1.).color(theme.accent).roughness(0.3).position(0., 0.5, 0.).spin(0., 45., 0.).name("cube"))
//!     .child(robot.node().name("robot").play(robot.clip("Walk").unwrap(), Playback::looping()))
//!     .child(plane(8., 8.).color(rgb(0x2a2a30)))
//!     .on_tap(cx.listener(|this, e: &SceneTapEvent, _| this.picked = e.node.clone()))
//!     .h(px(320.))
//! ```
//!
//! - **Units and axes**: meters if you like, any unit otherwise; right-handed,
//!   +Y up, the default camera looks down −Z. Angles are degrees.
//! - **Rotations** are Euler angles applied Z first, then Y, then X
//!   (`R = Rx · Ry · Rz`, SceneKit's `eulerAngles` and three.js' `XYZ`), or
//!   a quaternion ([`Node3d::quaternion`]), which wins over them.
//! - **Animation**: `spin` turns a node continuously (degrees per second),
//!   and [`Node3d::play`] plays a keyframe [`AnimationClip`], on the
//!   device, without re-rendering the app. Give the scene a
//!   `.transition(…)` and every change of a node's position, rotation,
//!   scale or colors, and of the camera, animates to its new value; nodes are
//!   matched by `name` (else by their place in the tree).
//! - **Models**: [`Model::from_glb`] reads a glTF 2.0 file into nodes,
//!   materials, skins and clips.
//! - **Colors** are sRGB, lighting is computed in linear space, then tone
//!   mapped (ACES, [`Scene3d::exposure`]). Adaptive colors follow the
//!   system appearance.
//! - **Taps**: `on_tap` reports the named node under the finger (the nearest
//!   named ancestor of the mesh hit), the labeled part of it, the instance
//!   and the world point.
//!
//! A scene has no natural size: give it one (`.h(px(300.))`, `.flex_grow(1.)`).
//!
//! The scene reaches the backends as [`SceneUpdate`]s ([`wire`]): the
//! whole scene the first time, then only the nodes that changed, with heavy
//! data (meshes, instances, clips, skins, environments, embedded images) as
//! [`blobs`] fetched once by id. [`player::ScenePlayer`] is the reference
//! implementation of everything that happens on the device (orbit, spin,
//! transitions, clips, skins, culling, shadows, hit testing); [`geometry`]
//! builds the primitives' triangles and [`shaders`] holds the GLSL every
//! OpenGL backend uses, so all of them draw the same picture.

pub mod blobs;
pub mod clip;
pub mod env;
pub mod geometry;
pub mod gltf;
pub mod math;
mod model;
pub mod player;
pub mod shaders;
pub mod wire;

pub use blobs::BlobRef;
pub use clip::{AnimationClip, Interp, Playback};
pub use gltf::ModelError;
pub use model::Model;
pub use wire::{
    FlatNode, FlatScene, InstancesRef, PlayRef, SceneDiff, SceneSettings, SceneUpdate, SkinRef,
};

use serde::{Deserialize, Serialize};

use crate::a11y::{Accessibility, Accessible, Role};
use crate::element::{Element, FluentBuilder, Handler, IntoElement, NodeKind, VNode};
use crate::style::{Color, Style, Styled, Transition};
use math::Aabb;

// --- Wire types -------------------------------------------------------------------------

/// An sRGB color with alpha, and its dark-mode variant: `[r, g, b, a]` or
/// `[r, g, b, a, dr, dg, db, da]` in JSON.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    /// Light (and default) appearance.
    pub light: [f32; 4],
    /// Dark appearance, if the color adapts.
    pub dark: Option<[f32; 4]>,
}

impl Rgba {
    /// The color in the given appearance.
    pub fn resolve(&self, dark: bool) -> [f32; 4] {
        match (dark, self.dark) {
            (true, Some(d)) => d,
            _ => self.light,
        }
    }
}

impl From<Color> for Rgba {
    fn from(c: Color) -> Self {
        Rgba {
            light: [c.r, c.g, c.b, c.a],
            dark: c.dark,
        }
    }
}

impl Serialize for Rgba {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut v: Vec<f32> = self.light.to_vec();
        if let Some(d) = self.dark {
            v.extend(d);
        }
        v.iter().map(|x| round(*x)).collect::<Vec<_>>().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Vec::<f32>::deserialize(d)?;
        let at = |i: usize, default: f32| v.get(i).copied().unwrap_or(default);
        Ok(Rgba {
            light: [at(0, 0.), at(1, 0.), at(2, 0.), at(3, 1.)],
            dark: (v.len() >= 8).then(|| [at(4, 0.), at(5, 0.), at(6, 0.), at(7, 1.)]),
        })
    }
}

/// Four decimals are plenty for colors and keep the JSON short.
fn round(x: f32) -> f32 {
    (x * 10000.).round() / 10000.
}

fn is_false(b: &bool) -> bool {
    !*b
}
fn is_true(b: &bool) -> bool {
    *b
}
fn yes() -> bool {
    true
}
fn is_one(x: &f32) -> bool {
    *x == 1.
}
fn one() -> f32 {
    1.
}
fn is_zero(x: &f32) -> bool {
    *x == 0.
}
fn is_zero3(v: &[f32; 3]) -> bool {
    *v == [0.; 3]
}
fn is_one3(v: &[f32; 3]) -> bool {
    *v == [1.; 3]
}
fn one3() -> [f32; 3] {
    [1.; 3]
}

/// Everything that describes a scene, as the app built it. It reaches the
/// backends flattened ([`SceneData::flatten`], [`FlatScene`]).
#[derive(Clone, Debug, PartialEq)]
pub struct SceneData {
    /// Clear color; `None`: transparent (the element's own background shows).
    pub background: Option<Rgba>,
    /// The point of view.
    pub camera: Camera,
    /// How the user can move the camera; `None`: it stays where it is.
    pub orbit: Option<Orbit>,
    /// Lights (a default rig when the app gives none and no environment).
    pub lights: Vec<Light>,
    /// The top-level nodes.
    pub nodes: Vec<Node3d>,
    /// How changes animate; `None`: they apply at once.
    pub transition: Option<SceneTransition>,
    /// Taps are reported (`on_tap` is set).
    pub tap: bool,
    /// Image-based lighting (and optionally the background).
    pub environment: Option<Environment>,
    /// Tone-mapping exposure (1: neutral).
    pub exposure: f32,
}

impl Default for SceneData {
    fn default() -> Self {
        SceneData {
            background: None,
            camera: Camera::default(),
            orbit: None,
            lights: Vec::new(),
            nodes: Vec::new(),
            transition: None,
            tap: false,
            environment: None,
            exposure: 1.,
        }
    }
}

/// The point of view: a perspective (or orthographic) camera at `position`
/// looking at `target`, +Y up.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    /// Where the camera is.
    #[serde(rename = "pos")]
    pub position: [f32; 3],
    /// The point it looks at (and orbits around).
    pub target: [f32; 3],
    /// Vertical field of view, degrees.
    pub fov: f32,
    /// Near clipping distance.
    pub near: f32,
    /// Far clipping distance.
    pub far: f32,
    /// Orthographic: the visible height in scene units (`fov` is then unused).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ortho: Option<f32>,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            position: [0., 2., 6.],
            target: [0.; 3],
            fov: 45.,
            near: 0.1,
            far: 1000.,
            ortho: None,
        }
    }
}

impl Camera {
    /// A perspective camera with this vertical field of view (degrees), at
    /// `(0, 2, 6)` looking at the origin.
    pub fn perspective(fov: f32) -> Self {
        Camera {
            fov,
            ..Default::default()
        }
    }
    /// An orthographic camera showing `height` units vertically (no
    /// perspective: parallel lines stay parallel).
    pub fn orthographic(height: f32) -> Self {
        Camera {
            ortho: Some(height),
            ..Default::default()
        }
    }
    /// Where the camera is.
    pub fn position(mut self, x: f32, y: f32, z: f32) -> Self {
        self.position = [x, y, z];
        self
    }
    /// The point it looks at (and orbits around).
    pub fn target(mut self, x: f32, y: f32, z: f32) -> Self {
        self.target = [x, y, z];
        self
    }
    /// Near and far clipping distances (default 0.1 and 1000).
    pub fn clip(mut self, near: f32, far: f32) -> Self {
        self.near = near;
        self.far = far;
        self
    }
}

/// Lets the user turn the camera around its target (drag), zoom (pinch,
/// scroll wheel, trackpad) and optionally pan (two fingers, right drag).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Orbit {
    /// Dragging turns the camera around the target.
    pub rotate: bool,
    /// Pinching or scrolling moves it closer or farther.
    pub zoom: bool,
    /// Two-finger drag (right or Shift drag with a mouse) moves the target.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pan: bool,
    /// Turns by itself around the vertical axis, degrees per second
    /// (paused while the user drags).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub auto: f32,
    /// Closest distance to the target (0: a quarter of the initial one).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub min: f32,
    /// Farthest distance (0: four times the initial one).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub max: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        Orbit {
            rotate: true,
            zoom: true,
            pan: false,
            auto: 0.,
            min: 0.,
            max: 0.,
        }
    }
}

impl Orbit {
    /// Drag to turn, pinch or scroll to zoom.
    pub fn new() -> Self {
        Self::default()
    }
    /// Turns by itself, `degrees_per_second` around the vertical axis.
    pub fn auto_rotate(mut self, degrees_per_second: f32) -> Self {
        self.auto = degrees_per_second;
        self
    }
    /// Turns by itself only: the user cannot move the camera.
    pub fn auto_only(degrees_per_second: f32) -> Self {
        Orbit {
            rotate: false,
            zoom: false,
            auto: degrees_per_second,
            ..Default::default()
        }
    }
    /// Whether pinching and scrolling zoom (default: yes).
    pub fn zoom(mut self, zoom: bool) -> Self {
        self.zoom = zoom;
        self
    }
    /// Whether two-finger drags move the target (default: no).
    pub fn pan(mut self, pan: bool) -> Self {
        self.pan = pan;
        self
    }
    /// Closest and farthest distances to the target.
    pub fn distance(mut self, min: f32, max: f32) -> Self {
        self.min = min;
        self.max = max;
        self
    }
}

/// What kind of light a [`Light`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightKind {
    /// Lights every surface evenly, from everywhere.
    Ambient,
    /// Parallel rays along `direction` (the sun).
    Directional,
    /// From `position`, in every direction (a bulb).
    Point,
    /// From `position` along `direction`, in a cone of `angle` degrees.
    Spot,
}

/// A light. Intensity 1 lights a white surface facing it fully white.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Light {
    /// Ambient, directional, point or spot.
    #[serde(rename = "type")]
    pub kind: LightKind,
    /// Its color.
    pub color: Rgba,
    /// Its strength (1: full).
    pub intensity: f32,
    /// Where it is (point and spot lights).
    #[serde(rename = "pos", default, skip_serializing_if = "is_zero3")]
    pub position: [f32; 3],
    /// Where it points (directional and spot lights).
    #[serde(rename = "dir", default, skip_serializing_if = "is_zero3")]
    pub direction: [f32; 3],
    /// Full cone angle of a spot light, degrees (its edge softens over the
    /// outer quarter).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub angle: f32,
    /// Distance at which a point or spot light fades out (0: it does not
    /// fade with distance).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub range: f32,
    /// Casts shadows (the first directional light that does; others
    /// ignore it).
    #[serde(default, skip_serializing_if = "is_false")]
    pub shadows: bool,
}

impl Light {
    fn new(kind: LightKind, color: Color, intensity: f32) -> Self {
        Light {
            kind,
            color: color.into(),
            intensity,
            position: [0.; 3],
            direction: [0.; 3],
            angle: 0.,
            range: 0.,
            shadows: false,
        }
    }
    /// Even light from everywhere (fills the shadows).
    pub fn ambient(color: Color, intensity: f32) -> Self {
        Self::new(LightKind::Ambient, color, intensity)
    }
    /// Parallel light, pointing down by default (`.direction(…)`).
    pub fn directional(color: Color, intensity: f32) -> Self {
        Self::new(LightKind::Directional, color, intensity).direction(0., -1., 0.)
    }
    /// A light at a point (`.position(…)`), shining every way.
    pub fn point(color: Color, intensity: f32) -> Self {
        Self::new(LightKind::Point, color, intensity).position(0., 3., 0.)
    }
    /// A cone of light (`.position(…)`, `.direction(…)`, `.angle(…)`),
    /// 45° wide by default.
    pub fn spot(color: Color, intensity: f32) -> Self {
        Light {
            angle: 45.,
            ..Self::new(LightKind::Spot, color, intensity)
                .position(0., 3., 0.)
                .direction(0., -1., 0.)
        }
    }
    /// Where a point or spot light is.
    pub fn position(mut self, x: f32, y: f32, z: f32) -> Self {
        self.position = [x, y, z];
        self
    }
    /// Where a directional or spot light points.
    pub fn direction(mut self, x: f32, y: f32, z: f32) -> Self {
        self.direction = [x, y, z];
        self
    }
    /// Full cone angle of a spot light, degrees.
    pub fn angle(mut self, degrees: f32) -> Self {
        self.angle = degrees;
        self
    }
    /// Distance at which a point or spot light has faded out.
    pub fn range(mut self, range: f32) -> Self {
        self.range = range;
        self
    }
    /// Whether a directional light casts shadows (the first one that does
    /// in the scene; default: no). Nodes opt out with
    /// [`Node3d::cast_shadows`] and [`Node3d::receive_shadows`].
    pub fn shadows(mut self, on: bool) -> Self {
        self.shadows = on;
        self
    }
}

/// A node's shape. Primitives are centered on the node's origin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Mesh {
    /// A box: width (X), height (Y), depth (Z).
    Box {
        /// `[width, height, depth]`.
        size: [f32; 3],
    },
    /// A sphere.
    Sphere {
        /// Its radius.
        radius: f32,
    },
    /// A flat rectangle in the XZ plane, facing up (+Y): width (X) and depth (Z).
    Plane {
        /// `[width, depth]`.
        size: [f32; 2],
    },
    /// A capped cylinder along Y.
    Cylinder {
        /// Its radius.
        radius: f32,
        /// Its height.
        height: f32,
    },
    /// A cone along Y, its point up, with a base.
    Cone {
        /// Radius of the base.
        radius: f32,
        /// Height.
        height: f32,
    },
    /// A ring in the XZ plane.
    Torus {
        /// From the center to the middle of the tube.
        radius: f32,
        /// Radius of the tube.
        tube: f32,
    },
    /// Your own triangles, or a model's ([`Mesh::custom`], [`Model`]): a
    /// mesh blob (`M`, see [`blobs`]).
    Blob {
        /// The blob.
        id: BlobRef,
        /// Its positions' box: `[min x, min y, min z, max x, max y, max z]`.
        bounds: Aabb,
    },
}

impl Mesh {
    /// Triangles from vertex positions (`[x, y, z]`) and indices (three per
    /// triangle, counter-clockwise seen from the front; `None`: every three
    /// vertices are a triangle). Normals are averaged from the faces
    /// (smooth), unless given with [`Mesh::with_normals`].
    pub fn custom(positions: &[[f32; 3]], indices: Option<&[u32]>) -> Mesh {
        let indices: Vec<u32> = match indices {
            Some(i) => i.to_vec(),
            None => (0..positions.len() as u32).collect(),
        };
        let flat: Vec<f32> = positions.iter().flatten().copied().collect();
        let normals = geometry::smooth_normals(&flat, &indices);
        Mesh::from_data(&blobs::MeshBlob {
            positions: positions.to_vec(),
            normals: normals.as_chunks::<3>().0.to_vec(),
            indices,
            ..Default::default()
        })
    }

    /// A mesh of these vertices (any attributes: uvs, vertex colors, skin
    /// weights). Indices out of range drop their triangle.
    pub fn from_data(data: &blobs::MeshBlob) -> Mesh {
        let n = data.positions.len() as u32;
        let mut data = std::borrow::Cow::Borrowed(data);
        if data.indices.iter().any(|i| *i >= n) || data.indices.len() % 3 != 0 {
            let indices = data
                .indices
                .as_chunks::<3>()
                .0
                .iter()
                .filter(|t| t.iter().all(|i| *i < n))
                .flatten()
                .copied()
                .collect();
            data.to_mut().indices = indices;
        }
        let bytes = blobs::encode_mesh(&data);
        let bounds = math::aabb_of(data.positions.iter().copied());
        Mesh::Blob {
            id: blobs::put(blobs::BlobKind::Mesh, blobs::BINARY_MIME, bytes),
            bounds,
        }
    }

    /// A blob mesh's vertices (`None` for primitives, or if its blob is
    /// gone).
    pub fn data(&self) -> Option<blobs::MeshBlob> {
        match self {
            Mesh::Blob { id, .. } => blobs::decode_mesh(&id.bytes()?),
            _ => None,
        }
    }

    /// Replaces a custom mesh's normals (`[x, y, z]` per vertex).
    pub fn with_normals(self, normals: &[[f32; 3]]) -> Mesh {
        match self.data() {
            Some(mut d) => {
                d.normals = normals.to_vec();
                Mesh::from_data(&d)
            }
            None => self,
        }
    }

    /// Gives a custom mesh texture coordinates (`[u, v]` per vertex,
    /// `(0, 0)`: the image's top-left corner).
    pub fn with_uvs(self, uvs: &[[f32; 2]]) -> Mesh {
        match self.data() {
            Some(mut d) => {
                d.uvs = uvs.to_vec();
                Mesh::from_data(&d)
            }
            None => self,
        }
    }

    /// Gives a custom mesh a color per vertex (linear RGBA, multiplied with
    /// the material's).
    pub fn with_colors(self, colors: &[[f32; 4]]) -> Mesh {
        match self.data() {
            Some(mut d) => {
                d.colors = colors.to_vec();
                Mesh::from_data(&d)
            }
            None => self,
        }
    }

    /// Its box in its own space: `[min x, min y, min z, max x, max y, max z]`
    /// (primitives are centered on the origin).
    pub fn bounds(&self) -> Aabb {
        let b = |x: f32, y: f32, z: f32| [-x, -y, -z, x, y, z];
        match self {
            Mesh::Box { size } => b(size[0] / 2., size[1] / 2., size[2] / 2.),
            Mesh::Sphere { radius } => b(*radius, *radius, *radius),
            Mesh::Plane { size } => b(size[0] / 2., 0., size[1] / 2.),
            Mesh::Cylinder { radius, height } | Mesh::Cone { radius, height } => {
                b(*radius, height / 2., *radius)
            }
            Mesh::Torus { radius, tube } => b(radius + tube, *tube, radius + tube),
            Mesh::Blob { bounds, .. } => *bounds,
        }
    }

    /// Identifies the triangles: equal keys, equal triangles (a blob's id,
    /// or the primitive and its sizes, e.g. `box:1,2,3`). Backends cache GPU
    /// buffers by it.
    pub fn key(&self) -> String {
        match self {
            Mesh::Box { size: [x, y, z] } => format!("box:{x},{y},{z}"),
            Mesh::Sphere { radius } => format!("sphere:{radius}"),
            Mesh::Plane { size: [x, z] } => format!("plane:{x},{z}"),
            Mesh::Cylinder { radius, height } => format!("cylinder:{radius},{height}"),
            Mesh::Cone { radius, height } => format!("cone:{radius},{height}"),
            Mesh::Torus { radius, tube } => format!("torus:{radius},{tube}"),
            Mesh::Blob { id, .. } => id.id().to_string(),
        }
    }
}

/// How a material uses its alpha (the color's and the texture's).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaMode {
    /// Blended when below 1: drawn after the opaque meshes, back to front.
    #[default]
    Blend,
    /// Fragments below the material's `cutoff` are not drawn, the others
    /// are opaque (leaves, fences).
    Mask,
    /// Alpha is ignored.
    Opaque,
}

impl AlphaMode {
    fn is_blend(&self) -> bool {
        *self == AlphaMode::Blend
    }
}

/// How a surface looks: a physically based material (base color,
/// metalness, roughness) with optional textures and glow.
///
/// Texture sources are `picture` sources (an app asset, a URL, a `data:`
/// URL) or `p3d:<blob id>` for images embedded in a model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Material {
    /// Base color (its alpha makes the surface see-through).
    pub color: Rgba,
    /// 0: plastic, wood, stone; 1: metal.
    #[serde(rename = "metal", default, skip_serializing_if = "is_zero")]
    pub metalness: f32,
    /// 0: mirror-smooth; 1: matte.
    #[serde(rename = "rough")]
    pub roughness: f32,
    /// Light it gives off by itself (unaffected by the lights).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emissive: Option<Rgba>,
    /// Ignores the lights: drawn in its flat color.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unlit: bool,
    /// Both faces drawn (planes seen from below, open meshes).
    #[serde(rename = "double", default, skip_serializing_if = "is_false")]
    pub double_sided: bool,
    /// Base-color image (sRGB), multiplied with `color`.
    #[serde(rename = "map", default, skip_serializing_if = "Option::is_none")]
    pub texture: Option<String>,
    /// Metalness and roughness image (glTF's: G roughness, B metalness),
    /// multiplied with `metalness` and `roughness`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mr_map: Option<String>,
    /// Tangent-space normal map (+Y up in the image, like glTF's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normal_map: Option<String>,
    /// How much the normal map bends the surface (1: as drawn).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub normal_scale: f32,
    /// Emission image (sRGB), multiplied with `emissive`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emissive_map: Option<String>,
    /// Ambient occlusion image (R channel): darkens ambient and environment
    /// light in creases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ao_map: Option<String>,
    /// How alpha is used (default: blended when below 1).
    #[serde(default, skip_serializing_if = "AlphaMode::is_blend")]
    pub alpha: AlphaMode,
    /// The alpha below which a masked surface is not drawn.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cutoff: f32,
    /// The mesh's vertex colors multiply the color (set by models).
    #[serde(default, skip_serializing_if = "is_false")]
    pub vcolor: bool,
    /// Embedded images the `p3d:` sources point to, kept alive with the
    /// material (filled by [`Model`]).
    #[serde(skip)]
    pub embedded: Vec<BlobRef>,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            color: Rgba {
                light: [0.8, 0.8, 0.8, 1.],
                dark: None,
            },
            metalness: 0.,
            roughness: 0.5,
            emissive: None,
            unlit: false,
            double_sided: false,
            texture: None,
            mr_map: None,
            normal_map: None,
            normal_scale: 1.,
            emissive_map: None,
            ao_map: None,
            alpha: AlphaMode::Blend,
            cutoff: 0.,
            vcolor: false,
            embedded: Vec::new(),
        }
    }
}

impl Material {
    /// Whether it is the default material (left out of the wire format).
    pub fn is_default(&self) -> bool {
        *self == Material::default()
    }
}

/// A skinned mesh's skin, as a node holds it ([`Model`] sets it): the `K`
/// blob, and how many levels up the skin's root node is (the model's
/// root; its joint paths start there).
#[derive(Clone, Debug, PartialEq)]
pub struct Skin {
    /// The `K` blob.
    pub blob: BlobRef,
    /// Levels from this node up to the skin's root node.
    pub up: u32,
}

/// One object of a scene: a mesh (or none: a group), its material, where
/// it is, and its children (placed relative to it).
#[derive(Clone, Debug, PartialEq)]
pub struct Node3d {
    /// Matches the node across updates and names it in taps.
    pub name: Option<String>,
    /// A description that is not a key (glTF node names): taps report the
    /// nearest labeled node as their `part`.
    pub label: Option<String>,
    /// Its shape; `None`: a group (only its children are drawn).
    pub mesh: Option<Mesh>,
    /// How its surface looks.
    pub material: Material,
    /// Position relative to its parent.
    pub position: [f32; 3],
    /// Euler angles, degrees (Z, then Y, then X).
    pub rotation: [f32; 3],
    /// A rotation as a unit quaternion `[x, y, z, w]`, instead of `rotation`.
    pub quaternion: Option<[f32; 4]>,
    /// Scale along each axis.
    pub scale: [f32; 3],
    /// Turns continuously, degrees per second around each axis.
    pub spin: [f32; 3],
    /// Not drawn (nor its children), still matched across updates.
    pub hidden: bool,
    /// Many copies of its mesh in one draw ([`Node3d::instances`]).
    pub instances: Option<InstancesRef>,
    /// The clip it plays ([`Node3d::play`]).
    pub play: Option<PlayRef>,
    /// Its skin (skinned meshes of a [`Model`]).
    pub skin: Option<Skin>,
    /// Its mesh shadows the others (default: yes).
    pub cast_shadows: bool,
    /// Shadows fall on its mesh (default: yes).
    pub receive_shadows: bool,
    /// Children, placed relative to this node.
    pub children: Vec<Node3d>,
}

impl Node3d {
    /// A node drawing `mesh`.
    pub fn new(mesh: Mesh) -> Self {
        Node3d {
            mesh: Some(mesh),
            ..group()
        }
    }
    /// Matches the node across updates (transitions) and names it in taps.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
    /// A description that is not a key: two nodes may share it. Taps report
    /// the nearest labeled node at or below the named one as `part`.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    /// Position relative to the parent.
    pub fn position(mut self, x: f32, y: f32, z: f32) -> Self {
        self.position = [x, y, z];
        self
    }
    /// Euler angles in degrees, applied Z first, then Y, then X.
    pub fn rotation(mut self, x: f32, y: f32, z: f32) -> Self {
        self.rotation = [x, y, z];
        self
    }
    /// A rotation as a quaternion (normalized); it replaces `rotation`.
    /// Transitions between quaternions take the shorter way around.
    pub fn quaternion(mut self, x: f32, y: f32, z: f32, w: f32) -> Self {
        self.quaternion = Some(math::quat_normalize([x, y, z, w]));
        self
    }
    /// Uniform scale.
    pub fn scale(mut self, s: f32) -> Self {
        self.scale = [s; 3];
        self
    }
    /// Scale along each axis.
    pub fn scale_xyz(mut self, x: f32, y: f32, z: f32) -> Self {
        self.scale = [x, y, z];
        self
    }
    /// Turns continuously, degrees per second around each axis
    /// (`spin(0., 90., 0.)`: a quarter turn per second around Y). Runs on the
    /// device: the app does not re-render. With a quaternion, the spin
    /// turns after it, around the node's own axes.
    pub fn spin(mut self, x: f32, y: f32, z: f32) -> Self {
        self.spin = [x, y, z];
        self
    }
    /// Not drawn (nor its children).
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }
    /// Draws its mesh once per instance, in one draw call (crowds, forests,
    /// particles). Set the mesh first: the set's bounds are computed with
    /// it. Taps report the `instance` hit.
    pub fn instances(mut self, instances: &Instances) -> Self {
        let mesh = self.mesh.as_ref().map(Mesh::bounds);
        self.instances = Some(instances.encoded(mesh));
        self
    }
    /// Plays `clip` on the device (its targets are relative to this node).
    /// Changing the clip (or `restart`) starts it over, crossfading from the
    /// previous one over `playback.fade` seconds.
    pub fn play(mut self, clip: AnimationClip, playback: Playback) -> Self {
        self.play = Some(PlayRef {
            clip: clip.blob(),
            speed: playback.speed,
            looping: playback.looping,
            fade: playback.fade,
            run: playback.restart,
        });
        self
    }
    /// Plays no clip (the nodes it moved go back to their own pose).
    pub fn stop(mut self) -> Self {
        self.play = None;
        self
    }
    /// Whether its mesh shadows others (default: yes).
    pub fn cast_shadows(mut self, on: bool) -> Self {
        self.cast_shadows = on;
        self
    }
    /// Whether shadows fall on its mesh (default: yes).
    pub fn receive_shadows(mut self, on: bool) -> Self {
        self.receive_shadows = on;
        self
    }
    /// Replaces the material.
    pub fn material(mut self, material: Material) -> Self {
        self.material = material;
        self
    }
    /// Base color (alpha below 1: see-through).
    pub fn color(mut self, color: Color) -> Self {
        self.material.color = color.into();
        self
    }
    /// 0: plastic, wood, stone (default); 1: metal.
    pub fn metalness(mut self, m: f32) -> Self {
        self.material.metalness = m.clamp(0., 1.);
        self
    }
    /// 0: mirror-smooth; 1: matte (default 0.5).
    pub fn roughness(mut self, r: f32) -> Self {
        self.material.roughness = r.clamp(0., 1.);
        self
    }
    /// Glows in this color, whatever the lights.
    pub fn emissive(mut self, color: Color) -> Self {
        self.material.emissive = Some(color.into());
        self
    }
    /// Drawn in its flat color, ignoring the lights.
    pub fn unlit(mut self) -> Self {
        self.material.unlit = true;
        self
    }
    /// Both faces drawn.
    pub fn double_sided(mut self) -> Self {
        self.material.double_sided = true;
        self
    }
    /// A base-color image (app asset, URL or `data:` URL), multiplied with
    /// the color (white by default once a texture is set).
    pub fn texture(mut self, source: impl Into<String>) -> Self {
        if self.material.texture.is_none() && self.material.color == Material::default().color {
            self.material.color = Color::WHITE.into();
        }
        self.material.texture = Some(source.into());
        self
    }
    /// A tangent-space normal map (+Y up in the image, like glTF's), its
    /// bumps scaled by `scale` (1: as drawn).
    pub fn normal_map(mut self, source: impl Into<String>, scale: f32) -> Self {
        self.material.normal_map = Some(source.into());
        self.material.normal_scale = scale;
        self
    }
    /// A metalness (B) and roughness (G) image, multiplied with
    /// `metalness` and `roughness`.
    pub fn metal_rough_map(mut self, source: impl Into<String>) -> Self {
        self.material.mr_map = Some(source.into());
        self
    }
    /// An ambient occlusion image (R): creases get less ambient and
    /// environment light.
    pub fn occlusion_map(mut self, source: impl Into<String>) -> Self {
        self.material.ao_map = Some(source.into());
        self
    }
    /// An emission image, multiplied with the emissive color (white if
    /// none was set).
    pub fn emissive_map(mut self, source: impl Into<String>) -> Self {
        if self.material.emissive.is_none() {
            self.material.emissive = Some(Color::WHITE.into());
        }
        self.material.emissive_map = Some(source.into());
        self
    }
    /// Cut-out transparency: where the alpha (color × texture) is below
    /// `cutoff`, nothing is drawn; elsewhere the surface is opaque.
    pub fn alpha_mask(mut self, cutoff: f32) -> Self {
        self.material.alpha = AlphaMode::Mask;
        self.material.cutoff = cutoff;
        self
    }
    /// Adds a child, placed relative to this node.
    pub fn child(mut self, child: Node3d) -> Self {
        self.children.push(child);
        self
    }
    /// Adds children.
    pub fn children(mut self, children: impl IntoIterator<Item = Node3d>) -> Self {
        self.children.extend(children);
        self
    }
}

impl FluentBuilder for Node3d {}

/// An empty node: moves, turns and scales its children together.
pub fn group() -> Node3d {
    Node3d {
        name: None,
        label: None,
        mesh: None,
        material: Material::default(),
        position: [0.; 3],
        rotation: [0.; 3],
        quaternion: None,
        scale: [1.; 3],
        spin: [0.; 3],
        hidden: false,
        instances: None,
        play: None,
        skin: None,
        cast_shadows: true,
        receive_shadows: true,
        children: Vec::new(),
    }
}

/// A cube with sides of `size`.
pub fn cube(size: f32) -> Node3d {
    box3d(size, size, size)
}

/// A box: width (X), height (Y), depth (Z).
pub fn box3d(width: f32, height: f32, depth: f32) -> Node3d {
    Node3d::new(Mesh::Box {
        size: [width, height, depth],
    })
}

/// A sphere.
pub fn sphere(radius: f32) -> Node3d {
    Node3d::new(Mesh::Sphere { radius })
}

/// A flat rectangle facing up (a floor): width (X), depth (Z).
pub fn plane(width: f32, depth: f32) -> Node3d {
    Node3d::new(Mesh::Plane {
        size: [width, depth],
    })
}

/// A capped cylinder standing along Y.
pub fn cylinder(radius: f32, height: f32) -> Node3d {
    Node3d::new(Mesh::Cylinder { radius, height })
}

/// A cone standing along Y, its point up.
pub fn cone(radius: f32, height: f32) -> Node3d {
    Node3d::new(Mesh::Cone { radius, height })
}

/// A ring lying in the XZ plane.
pub fn torus(radius: f32, tube: f32) -> Node3d {
    Node3d::new(Mesh::Torus { radius, tube })
}

/// A node drawing your own triangles ([`Mesh::custom`]).
pub fn mesh(mesh: Mesh) -> Node3d {
    Node3d::new(mesh)
}

/// How scene changes animate (from the scene's `.transition(…)`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneTransition {
    /// Milliseconds.
    pub ms: f32,
    /// Milliseconds before it starts.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub delay: f32,
    /// The timing curve as cubic Bézier control points `x1, y1, x2, y2`
    /// (like CSS `cubic-bezier`; springs overshoot a little).
    pub bezier: [f32; 4],
}

impl From<Transition> for SceneTransition {
    fn from(t: Transition) -> Self {
        let (x1, y1, x2, y2) = t.bezier();
        SceneTransition {
            ms: t.duration * 1000.,
            delay: t.delay * 1000.,
            bezier: [x1, y1, x2, y2],
        }
    }
}

// --- Instances ----------------------------------------------------------------------------

/// Copies of a node's mesh drawn in one call ([`Node3d::instances`]): each
/// with its position, rotation and scale (relative to the node) and an
/// optional color multiplying the material's.
///
/// ```ignore
/// let mut trees = Instances::new();
/// for (x, z) in spots {
///     trees = trees.push([x, 0., z], [0., x * 40., 0.], 1.);
/// }
/// cone(0.4, 1.).color(rgb(0x2f7d4f)).instances(&trees)
/// ```
///
/// Building the set encodes it once per mesh it is used with; keep it in
/// your view's state when it is big and does not change.
#[derive(Clone, Default)]
pub struct Instances {
    transforms: Vec<blobs::InstanceTransform>,
    /// sRGB, per instance (`None`: white).
    colors: Vec<Option<[f32; 4]>>,
    /// The last encoding and the mesh bounds it was made for.
    encoded: std::sync::Arc<std::sync::Mutex<Option<Encoded>>>,
}

/// An instance set's `I` blob, and the mesh bounds it was computed with.
type Encoded = (Option<Aabb>, InstancesRef);

impl PartialEq for Instances {
    fn eq(&self, other: &Self) -> bool {
        self.transforms == other.transforms && self.colors == other.colors
    }
}

impl std::fmt::Debug for Instances {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Instances({})", self.transforms.len())
    }
}

impl Instances {
    /// No instances.
    pub fn new() -> Self {
        Self::default()
    }

    /// `counts` instances along X, Y and Z, `spacing` apart, centered on
    /// the node.
    pub fn grid(counts: [u32; 3], spacing: [f32; 3]) -> Self {
        let mut set = Instances::new();
        let at = |i: u32, n: u32, s: f32| (i as f32 - (n.max(1) - 1) as f32 / 2.) * s;
        for x in 0..counts[0] {
            for y in 0..counts[1] {
                for z in 0..counts[2] {
                    set = set.push(
                        [
                            at(x, counts[0], spacing[0]),
                            at(y, counts[1], spacing[1]),
                            at(z, counts[2], spacing[2]),
                        ],
                        [0.; 3],
                        1.,
                    );
                }
            }
        }
        set
    }

    /// Adds an instance: position, Euler angles in degrees (Z, then Y, then
    /// X) and uniform scale.
    pub fn push(self, position: [f32; 3], rotation_deg: [f32; 3], scale: f32) -> Self {
        self.push_quat(position, math::quat_from_euler(rotation_deg), [scale; 3])
    }

    /// Adds an instance: position, rotation quaternion `[x, y, z, w]` and
    /// scale along each axis.
    pub fn push_quat(mut self, position: [f32; 3], rotation: [f32; 4], scale: [f32; 3]) -> Self {
        self.transforms.push(blobs::InstanceTransform {
            position,
            rotation: math::quat_normalize(rotation),
            scale,
        });
        self.colors.push(None);
        self.encoded = Default::default();
        self
    }

    /// The color of the last instance added (multiplies the material's;
    /// its light-appearance value).
    pub fn color(mut self, color: Color) -> Self {
        if let Some(c) = self.colors.last_mut() {
            *c = Some([color.r, color.g, color.b, color.a]);
            self.encoded = Default::default();
        }
        self
    }

    /// How many instances.
    pub fn len(&self) -> usize {
        self.transforms.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.transforms.is_empty()
    }

    /// The `I` blob for a node whose mesh has these bounds (cached).
    fn encoded(&self, mesh: Option<Aabb>) -> InstancesRef {
        let mut cache = self.encoded.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((m, r)) = &*cache
            && *m == mesh
        {
            return r.clone();
        }
        let bounds = {
            let each = self.transforms.iter().map(|t| match &mesh {
                Some(b) => math::transform_aabb(&t.matrix(), b),
                None => {
                    let p = t.position;
                    [p[0], p[1], p[2], p[0], p[1], p[2]]
                }
            });
            each.reduce(|a, b| math::aabb_union(&a, &b))
                .unwrap_or([0.; 6])
        };
        let colors = if self.colors.iter().any(Option::is_some) {
            self.colors
                .iter()
                .map(|c| {
                    let [r, g, b, a] = c.unwrap_or([1.; 4]);
                    let lin = math::srgb_to_linear;
                    [lin(r), lin(g), lin(b), a]
                })
                .collect()
        } else {
            Vec::new()
        };
        let blob = blobs::encode_instances(&blobs::InstancesBlob {
            transforms: self.transforms.clone(),
            colors,
            bounds,
        });
        let r = InstancesRef {
            id: blobs::put(blobs::BlobKind::Instances, blobs::BINARY_MIME, blob),
            count: self.transforms.len() as u32,
            bounds,
        };
        *cache = Some((mesh, r.clone()));
        r
    }
}

// --- Environments --------------------------------------------------------------------------

/// Image-based lighting: the light of a surrounding image reflected by
/// every surface (metals mirror it, rough surfaces blur it), optionally
/// shown as the background.
#[derive(Clone, Debug, PartialEq)]
pub struct Environment {
    /// The `E` blob.
    pub blob: BlobRef,
    /// Its strength (1: as is).
    pub intensity: f32,
    /// Drawn behind the scene (instead of the background color).
    pub background: bool,
}

impl Environment {
    /// Soft boxes over a gray gradient: a neutral product-shot light.
    pub fn studio() -> Self {
        Self::builtin("studio", env::studio)
    }
    /// An even gray room.
    pub fn neutral() -> Self {
        Self::builtin("neutral", env::neutral)
    }
    /// A warm sky with a low sun.
    pub fn sunset() -> Self {
        Self::builtin("sunset", env::sunset)
    }
    /// A Radiance `.hdr` image (equirectangular); `None` if unreadable.
    pub fn from_hdr(bytes: &[u8]) -> Option<Self> {
        env::from_hdr(bytes).map(|m| Self::from_map(&m))
    }
    /// A prefiltered environment ([`env`]).
    pub fn from_map(map: &env::EnvMap) -> Self {
        Environment {
            blob: blobs::put(
                blobs::BlobKind::Environment,
                blobs::BINARY_MIME,
                map.encode(),
            ),
            intensity: 1.,
            background: false,
        }
    }
    /// The built-in environments are made once per process.
    fn builtin(name: &'static str, make: fn() -> env::EnvMap) -> Self {
        static MADE: std::sync::Mutex<Vec<(&str, BlobRef)>> = std::sync::Mutex::new(Vec::new());
        let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, b)) = made.iter().find(|(n, _)| *n == name) {
            return Environment {
                blob: b.clone(),
                intensity: 1.,
                background: false,
            };
        }
        let e = Self::from_map(&make());
        made.push((name, e.blob.clone()));
        e
    }
    /// Its strength (default 1).
    pub fn intensity(mut self, intensity: f32) -> Self {
        self.intensity = intensity.max(0.);
        self
    }
    /// Shows it behind the scene (default: no; the background color or the
    /// element's own background shows).
    pub fn background(mut self, on: bool) -> Self {
        self.background = on;
        self
    }
}

// --- Taps -------------------------------------------------------------------------------

/// A tap or click on a scene ([`Scene3d::on_tap`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneTapEvent {
    /// The name of the node hit (the nearest named ancestor of the mesh
    /// under the finger, itself included); `None`: nothing named was hit.
    #[serde(default)]
    pub node: Option<String>,
    /// The label of the nearest labeled node from the mesh hit up to the
    /// named one (a model's part: `"LeftArm"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    /// The instance hit, when the mesh is instanced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<u32>,
    /// Where the ray hit, in scene coordinates; `None`: it hit nothing.
    #[serde(default)]
    pub point: Option<[f32; 3]>,
}

impl SceneTapEvent {
    /// Whether the node named `name` was tapped.
    pub fn is(&self, name: &str) -> bool {
        self.node.as_deref() == Some(name)
    }

    /// Reads the JSON a backend reports (`{"node", "part", "instance",
    /// "point"}`).
    pub fn decode(json: &str) -> SceneTapEvent {
        serde_json::from_str(json).unwrap_or_default()
    }
}

// --- The element ------------------------------------------------------------------------

/// A 3D scene element. See [`scene3d`].
pub struct Scene3d {
    node: VNode,
    data: SceneData,
}

/// A 3D scene, drawn by the GPU (see [`scene3d`](mod@crate::scene3d)). It has
/// no natural size: give it one (`.h(px(300.))`, `.flex_grow(1.)`); it does
/// not shrink below it (`flex_shrink(0)`), since nothing inside holds it
/// open. Decorative unless given [`Scene3d::label`].
pub fn scene3d() -> Scene3d {
    let mut node = VNode::new(NodeKind::Scene3d);
    node.a11y.hidden = true;
    node.style.flex_shrink = 0.;
    Scene3d {
        node,
        data: SceneData::default(),
    }
}

impl Scene3d {
    /// Adds a top-level node.
    pub fn child(mut self, node: Node3d) -> Self {
        self.data.nodes.push(node);
        self
    }
    /// Adds top-level nodes.
    pub fn children(mut self, nodes: impl IntoIterator<Item = Node3d>) -> Self {
        self.data.nodes.extend(nodes);
        self
    }
    /// The point of view (default: 45°, at `(0, 2, 6)`, looking at the origin).
    pub fn camera(mut self, camera: Camera) -> Self {
        self.data.camera = camera;
        self
    }
    /// Lets the user turn and zoom the camera (`Orbit::new()`), or turns it
    /// by itself (`Orbit::auto_only(20.)`).
    pub fn orbit(mut self, orbit: Orbit) -> Self {
        self.data.orbit = Some(orbit);
        self
    }
    /// Adds a light. Without any, a soft default rig lights the scene.
    pub fn light(mut self, light: Light) -> Self {
        self.data.lights.push(light);
        self
    }
    /// Fills the scene's frame behind the objects (default: transparent, so
    /// the element's `.bg(…)` shows).
    pub fn background(mut self, color: Color) -> Self {
        self.data.background = Some(color.into());
        self
    }
    /// Lights the scene with an environment (reflections and soft ambient
    /// light; [`Environment::background`] also shows it behind).
    pub fn environment(mut self, environment: Environment) -> Self {
        self.data.environment = Some(environment);
        self
    }
    /// Tone-mapping exposure: above 1 brighter, below darker (default 1).
    pub fn exposure(mut self, exposure: f32) -> Self {
        self.data.exposure = exposure.max(0.);
        self
    }
    /// Called when the scene is tapped or clicked (not after a drag), with
    /// the named node hit (and its labeled part, the instance, the point).
    pub fn on_tap(mut self, handler: Handler<SceneTapEvent>) -> Self {
        self.node.on_scene_tap = Some(handler);
        self
    }
    /// Announces the scene to screen readers ("A red cube on a table").
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.node.a11y.hidden = false;
        self.node.a11y.role = Some(Role::Image);
        self.node.a11y.label = Some(label.into());
        self
    }
    /// Stable identity among siblings.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.node.key = Some(key.into());
        self
    }
    /// An attribute of the element on the web (`data-*` and the safe names
    /// of [`SAFE_ATTRIBUTES`](crate::attributes::SAFE_ATTRIBUTES)), like
    /// `Div::attr`: `.attr("data-testid", "viewer")`. Native backends ignore it.
    pub fn attr(mut self, name: impl AsRef<str>, value: impl Into<String>) -> Self {
        let name = name.as_ref().to_ascii_lowercase();
        let allowed = crate::attributes::is_app_attribute(&name);
        debug_assert!(
            allowed,
            "`{name}` is not an attribute an app may set (see prism_core::attributes)"
        );
        if allowed {
            let value = value.into();
            match self.node.attributes.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => slot.1 = value,
                None => self.node.attributes.push((name, value)),
            }
        }
        self
    }
    /// The scene as it will be sent (for tests).
    pub fn data(&self) -> &SceneData {
        &self.data
    }
}

impl Styled for Scene3d {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.node.style
    }
}

impl Accessible for Scene3d {
    fn a11y_mut(&mut self) -> &mut Accessibility {
        &mut self.node.a11y
    }
}

impl FluentBuilder for Scene3d {}

impl IntoElement for Scene3d {
    fn into_element(self) -> Element {
        let Scene3d { mut node, mut data } = self;
        // The element's transition animates the scene's contents too.
        data.transition = node.style.transition.map(SceneTransition::from);
        data.tap = node.on_scene_tap.is_some();
        node.scene = Some(Box::new(data));
        Element::Node(Box::new(node))
    }
}

#[cfg(test)]
mod tests;
