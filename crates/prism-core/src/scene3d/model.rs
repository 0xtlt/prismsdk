//! glTF models as scene nodes ([`Model`]).

use super::blobs::{self, BINARY_MIME, BlobKind, BlobRef, MeshBlob, SkinBlob};
use super::clip::{AnimationClip, ClipChannel, ClipData, ClipProperty, Interp};
use super::gltf::{
    self, Imported, ImportedAlpha, ImportedInterpolation, ImportedMaterial, ImportedPath,
    ImportedPrimitive, ImportedSkin, ModelError,
};
use super::math::{self, Aabb, Mat4};
use super::{AlphaMode, Material, Mesh, Node3d, Rgba, Skin, group};

/// A glTF 2.0 model (`.glb`, `.gltf`), read once: its default scene as a
/// node tree, its materials and textures, skins and animation clips.
///
/// ```ignore
/// static ROBOT: LazyLock<Model> =
///     LazyLock::new(|| Model::from_glb(include_bytes!("robot.glb")).unwrap());
///
/// scene3d().child(
///     ROBOT.node().name("robot").play(ROBOT.clip("Walk").unwrap(), Playback::looping()),
/// )
/// ```
///
/// [`node`](Model::node) is unnamed: name it (two copies need two names).
/// The file's node names become labels (taps report them as `part`).
/// Each primitive of a mesh is a child node of the file's node, after its
/// own children. Lights and cameras in the file are ignored.
#[derive(Clone, Debug)]
pub struct Model {
    root: Node3d,
    clips: Vec<AnimationClip>,
    bounds: [[f32; 3]; 2],
}

impl Model {
    /// Reads a binary glTF (`.glb`).
    pub fn from_glb(bytes: &[u8]) -> Result<Model, ModelError> {
        Ok(Model::from_imported(&gltf::import_glb(bytes)?))
    }

    /// Reads a JSON glTF; `load` returns the bytes of a relative URI
    /// (buffers, images), e.g. from `include_bytes!` or the app's assets.
    pub fn from_gltf(
        json: &[u8],
        load: impl Fn(&str) -> Option<Vec<u8>>,
    ) -> Result<Model, ModelError> {
        Ok(Model::from_imported(&gltf::import_gltf(json, &load)?))
    }

    /// A group holding the model's node tree (a cheap clone: the heavy data
    /// is shared). Unnamed: name it before adding it to a scene.
    pub fn node(&self) -> Node3d {
        self.root.clone()
    }

    /// Its animations. Their targets are child-index paths from
    /// [`node`](Model::node): play them on it.
    pub fn clips(&self) -> &[AnimationClip] {
        &self.clips
    }

    /// The animation named `name`.
    pub fn clip(&self, name: &str) -> Option<AnimationClip> {
        self.clips.iter().find(|c| c.name() == name).cloned()
    }

    /// `[min, max]` corners of its box, in its own space, at rest (skinned
    /// meshes where their joints put them).
    pub fn bounds(&self) -> [[f32; 3]; 2] {
        self.bounds
    }

    /// Turns imported data into nodes and blobs.
    pub fn from_imported(imp: &Imported) -> Model {
        let images: Vec<BlobRef> = imp
            .images
            .iter()
            .map(|i| blobs::put(BlobKind::Image, &i.mime, i.bytes.clone()))
            .collect();
        // Every primitive's mesh blob and whether it has vertex colors.
        let meshes: Vec<Vec<Option<(Mesh, bool)>>> = imp
            .meshes
            .iter()
            .map(|m| {
                m.primitives
                    .iter()
                    .map(|p| {
                        if p.positions.is_empty() || p.indices.len() < 3 {
                            return None;
                        }
                        let n = p.positions.len();
                        let data = MeshBlob {
                            positions: p.positions.clone(),
                            normals: p.normals.clone(),
                            uvs: p.uvs.clone(),
                            joints: p.joints.clone(),
                            weights: p.weights.clone(),
                            colors: p.colors.clone(),
                            indices: p.indices.clone(),
                            bounds: [0.; 6],
                        };
                        Some((Mesh::from_data(&data), p.colors.len() == n))
                    })
                    .collect()
            })
            .collect();

        // Each node's child-index path from the model's root, and its depth.
        let mut paths: Vec<Option<(String, u32)>> = vec![None; imp.nodes.len()];
        let mut stack: Vec<(usize, String, u32)> = imp
            .roots
            .iter()
            .enumerate()
            .rev()
            .map(|(i, r)| (*r, i.to_string(), 1))
            .collect();
        while let Some((n, path, depth)) = stack.pop() {
            // A node reached twice (a malformed file) keeps its first place.
            let Some(slot) = paths.get_mut(n).filter(|s| s.is_none()) else {
                continue;
            };
            *slot = Some((path.clone(), depth));
            for (j, c) in imp.nodes[n].children.iter().enumerate().rev() {
                stack.push((*c, format!("{path}/{j}"), depth + 1));
            }
        }

        let skins: Vec<BlobRef> = imp
            .skins
            .iter()
            .map(|s| {
                let skin = SkinBlob {
                    inverse_bind: s.inverse_bind.clone(),
                    // A joint outside the scene: a path that resolves to nothing.
                    joints: s
                        .joints
                        .iter()
                        .map(|j| paths.get(*j).cloned().flatten().map_or("-".into(), |p| p.0))
                        .collect(),
                };
                blobs::put(BlobKind::Skin, BINARY_MIME, blobs::encode_skin(&skin))
            })
            .collect();

        let rest = rest_worlds(imp);
        let mut build = Build {
            imp,
            images: &images,
            meshes: &meshes,
            skins: &skins,
            paths: &paths,
            rest: &rest,
            placed: vec![false; imp.nodes.len()],
            bounds: None,
        };
        // A root that cannot be placed still takes its index (paths stay right).
        let children: Vec<Node3d> = imp
            .roots
            .iter()
            .map(|r| build.node(*r, &math::IDENTITY).unwrap_or_else(group))
            .collect();
        let b = build.bounds.unwrap_or([0.; 6]);

        let clips = imp
            .animations
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let channels: Vec<ClipChannel> = a
                    .channels
                    .iter()
                    .filter_map(|c| {
                        let (target, _) = paths.get(c.node)?.clone()?;
                        Some(ClipChannel {
                            target,
                            property: match c.path {
                                ImportedPath::Translation => ClipProperty::Translation,
                                ImportedPath::Rotation => ClipProperty::Rotation,
                                ImportedPath::Scale => ClipProperty::Scale,
                            },
                            interpolation: match c.interpolation {
                                ImportedInterpolation::Linear => Interp::Linear,
                                ImportedInterpolation::Step => Interp::Step,
                                ImportedInterpolation::CubicSpline => Interp::CubicSpline,
                            },
                            times: c.times.clone(),
                            values: c.values.clone(),
                        })
                    })
                    .collect();
                let duration = channels
                    .iter()
                    .filter_map(|c| c.times.last().copied())
                    .fold(0., f32::max);
                let name = a.name.clone().unwrap_or_else(|| format!("animation {i}"));
                AnimationClip::from_data(name, ClipData { duration, channels })
            })
            .collect();

        Model {
            root: group().children(children),
            clips,
            bounds: [[b[0], b[1], b[2]], [b[3], b[4], b[5]]],
        }
    }
}

struct Build<'a> {
    imp: &'a Imported,
    images: &'a [BlobRef],
    meshes: &'a [Vec<Option<(Mesh, bool)>>],
    skins: &'a [BlobRef],
    paths: &'a [Option<(String, u32)>],
    /// Every node's matrix in the model's space, at rest.
    rest: &'a [Option<Mat4>],
    /// Nodes already built (a node reached twice is built once).
    placed: Vec<bool>,
    bounds: Option<Aabb>,
}

impl Build<'_> {
    /// Node `n` and its subtree; `parent`: its parent's matrix in model space.
    fn node(&mut self, n: usize, parent: &Mat4) -> Option<Node3d> {
        if std::mem::replace(self.placed.get_mut(n)?, true) {
            return None;
        }
        let src = &self.imp.nodes[n];
        let q = math::quat_normalize(src.rotation);
        let mut node = group()
            .position(src.translation[0], src.translation[1], src.translation[2])
            .scale_xyz(src.scale[0], src.scale[1], src.scale[2]);
        if q != math::QUAT_IDENTITY {
            node.quaternion = Some(q);
        }
        node.label = src.name.clone();
        let world = math::mul(parent, &math::compose_quat(src.translation, q, src.scale));
        for c in &src.children {
            // Children the file lists but cannot be placed still take
            // their index, so paths stay right.
            node.children
                .push(self.node(*c, &world).unwrap_or_else(group));
        }
        let depth = self.paths.get(n).cloned().flatten().map_or(1, |p| p.1);
        if let Some(prims) = src
            .mesh
            .and_then(|m| self.imp.meshes.get(m).zip(self.meshes.get(m)))
        {
            for (p, built) in prims.0.primitives.iter().zip(prims.1) {
                let Some((mesh, colors)) = built else {
                    continue;
                };
                // A skinned mesh is placed by its joints, not by its node.
                let b = match src.skin.and_then(|s| self.imp.skins.get(s)) {
                    Some(skin) => skinned_bounds(p, skin, self.rest, &world),
                    None => math::transform_aabb(&world, &mesh.bounds()),
                };
                self.bounds = Some(match self.bounds {
                    Some(a) => math::aabb_union(&a, &b),
                    None => b,
                });
                let mut prim = Node3d::new(mesh.clone());
                prim.material =
                    self.material(p.material.and_then(|m| self.imp.materials.get(m)), *colors);
                prim.skin = src.skin.and_then(|s| self.skins.get(s)).map(|blob| Skin {
                    blob: blob.clone(),
                    // The primitive is one level below its node.
                    up: depth + 1,
                });
                node.children.push(prim);
            }
        }
        Some(node)
    }

    /// A glTF material (`None`: glTF's default, white metal of roughness 1).
    fn material(&self, m: Option<&ImportedMaterial>, colors: bool) -> Material {
        let Some(m) = m else {
            return Material {
                color: rgba([1., 1., 1., 1.]),
                metalness: 1.,
                roughness: 1.,
                alpha: AlphaMode::Opaque,
                vcolor: colors,
                ..Material::default()
            };
        };
        let mut embedded = Vec::new();
        let mut source = |i: Option<usize>| {
            let blob = self.images.get(i?)?;
            embedded.push(blob.clone());
            Some(format!("p3d:{}", blob.id()))
        };
        let texture = source(m.base_texture);
        let mr_map = source(m.mr_texture);
        let normal_map = source(m.normal_texture);
        let emissive_map = source(m.emissive_texture);
        let ao_map = source(m.occlusion_texture);
        let [er, eg, eb] = m.emissive;
        let (alpha, cutoff) = match m.alpha {
            ImportedAlpha::Opaque => (AlphaMode::Opaque, 0.),
            ImportedAlpha::Mask(c) => (AlphaMode::Mask, c),
            ImportedAlpha::Blend => (AlphaMode::Blend, 0.),
        };
        embedded.dedup();
        Material {
            color: rgba(m.base_color),
            metalness: m.metallic.clamp(0., 1.),
            roughness: m.roughness.clamp(0., 1.),
            emissive: (m.emissive != [0.; 3] || emissive_map.is_some())
                .then(|| rgba([er, eg, eb, 1.])),
            unlit: m.unlit,
            double_sided: m.double_sided,
            texture,
            mr_map,
            normal_map,
            normal_scale: m.normal_scale,
            emissive_map,
            ao_map,
            alpha,
            cutoff,
            vcolor: colors,
            embedded,
        }
    }
}

/// Each node's matrix in the model's space at rest (`None`: not in the
/// scene; a node reached twice keeps its first place).
fn rest_worlds(imp: &Imported) -> Vec<Option<Mat4>> {
    let mut worlds = vec![None; imp.nodes.len()];
    let mut stack: Vec<(usize, Mat4)> = imp
        .roots
        .iter()
        .rev()
        .map(|r| (*r, math::IDENTITY))
        .collect();
    while let Some((n, parent)) = stack.pop() {
        let Some(slot) = worlds.get_mut(n).filter(|w| w.is_none()) else {
            continue;
        };
        let src = &imp.nodes[n];
        let q = math::quat_normalize(src.rotation);
        let world = math::mul(&parent, &math::compose_quat(src.translation, q, src.scale));
        *slot = Some(world);
        stack.extend(src.children.iter().rev().map(|c| (*c, world)));
    }
    worlds
}

/// The box of a skinned primitive at rest: each vertex `Σ w_k · rest(joint
/// j_k) · inverse_bind[j_k] · v` (glTF: the joints place it, not its node;
/// `node_world` for a vertex without weights).
fn skinned_bounds(
    p: &ImportedPrimitive,
    skin: &ImportedSkin,
    rest: &[Option<Mat4>],
    node_world: &Mat4,
) -> Aabb {
    let joints: Vec<Mat4> = skin
        .joints
        .iter()
        .enumerate()
        .map(|(k, j)| {
            let bind = skin.inverse_bind.get(k).unwrap_or(&math::IDENTITY);
            match rest.get(*j).copied().flatten() {
                Some(w) => math::mul(&w, bind),
                None => *node_world,
            }
        })
        .collect();
    let weighted = p.joints.len() == p.positions.len() && p.weights.len() == p.positions.len();
    math::aabb_of(p.positions.iter().enumerate().map(|(i, v)| {
        let mut out = [0f32; 3];
        let mut total = 0.;
        if weighted {
            for (j, w) in p.joints[i].iter().zip(p.weights[i]) {
                if let (Some(m), true) = (joints.get(*j as usize), w != 0.) {
                    out = math::add(out, math::scale(math::transform_point(m, *v), w));
                    total += w;
                }
            }
        }
        if total == 0. {
            math::transform_point(node_world, *v)
        } else {
            out
        }
    }))
}

/// A glTF color (linear) as a scene color (sRGB; alpha as is).
fn rgba(linear: [f32; 4]) -> Rgba {
    let [r, g, b, a] = linear;
    let s = |c: f32| math::linear_to_srgb(c.max(0.));
    Rgba {
        light: [s(r), s(g), s(b), a],
        dark: None,
    }
}
