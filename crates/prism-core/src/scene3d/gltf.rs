//! glTF 2.0 import (`.glb`, `.gltf`): the file's default scene as plain data
//! (nodes, meshes, materials, images, skins, animations), which
//! [`Model`](super::Model) turns into nodes and blobs.
//!
//! Built on the typed JSON document of the `gltf` project (`gltf-json`, cargo
//! feature `gltf`, on by default); the GLB container, buffers and accessors are
//! read here, with every offset checked, so a malformed file never panics:
//! an unreadable one is `Err(ModelError::Parse)`, a readable one with bad
//! parts (out-of-range indices, accessors past their buffer, missing external
//! files…) imports without those parts.
//!
//! What is imported:
//! - **Scene**: the default scene (`scene`), else the first one, else every
//!   node without a parent. Every node of the file is in
//!   [`Imported::nodes`]; the node graph is made a forest (a node keeps its
//!   first parent; cycles are cut).
//! - **Transforms**: TRS, or `matrix` decomposed into T/R/S (shear is lost; a
//!   mirroring matrix, negative determinant, gets a negative X scale; a matrix
//!   with a zero-length axis keeps an identity rotation). Rotations are
//!   normalized.
//! - **Meshes**: TRIANGLES primitives (triangle strips and fans are converted;
//!   points and lines are skipped). POSITION, NORMAL, TEXCOORD_0, COLOR_0 (RGB
//!   or RGBA), JOINTS_0, WEIGHTS_0 (normalized to sum 1); every component type,
//!   normalized integers, byte strides and sparse accessors. Missing indices
//!   are generated; missing normals are computed smooth, area-weighted over
//!   shared vertices (flat for unindexed meshes). Triangles with an
//!   out-of-range index are dropped. Morph targets and tangents are skipped.
//! - **Materials**: metallic-roughness factors and textures, normal map and
//!   scale, emissive (times `KHR_materials_emissive_strength`), occlusion,
//!   alpha mode and cutoff, double sided, `KHR_materials_unlit`. Textures index
//!   [`Imported::images`] (deduplicated); a texture on another UV set than 0
//!   is dropped; samplers and `KHR_texture_transform` are ignored (textures
//!   sample untransformed, repeating).
//! - **Images**: from a buffer view, a `data:` URI or the loader (a texture's
//!   `EXT_texture_webp` source is preferred to its core one); the MIME type
//!   is sniffed from the bytes (PNG, JPEG, WebP, GIF, KTX2), else the
//!   file's.
//! - **Skins**: joints and inverse bind matrices (identity when absent).
//! - **Animations**: translation, rotation and scale channels (morph weights
//!   are skipped); CUBICSPLINE keys stay in-tangent/value/out-tangent
//!   triplets; rotation values (not tangents) are normalized.
//! - Cameras and `KHR_lights_punctual` are ignored. A file *requiring* an
//!   extension outside the ones above (Draco, meshopt, Basis…) is
//!   `Err(ModelError::Unsupported)`.
//!
//! UVs keep glTF's convention, `(0, 0)` at the image's top-left.

/// Why a model could not be read.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelError {
    /// Not a readable glTF file.
    Parse(String),
    /// A glTF feature this importer cannot do without (e.g. a required extension).
    Unsupported(String),
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::Parse(m) => write!(f, "unreadable glTF: {m}"),
            ModelError::Unsupported(m) => write!(f, "unsupported glTF: {m}"),
        }
    }
}

impl std::error::Error for ModelError {}

/// A model's default scene, as plain data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Imported {
    /// Indices into `nodes` of the scene's root nodes.
    pub roots: Vec<usize>,
    /// Every node of the file.
    pub nodes: Vec<ImportedNode>,
    /// Meshes, referenced by nodes.
    pub meshes: Vec<ImportedMesh>,
    /// Materials, referenced by primitives.
    pub materials: Vec<ImportedMaterial>,
    /// Embedded images (encoded), referenced by materials.
    pub images: Vec<ImportedImage>,
    /// Skins, referenced by nodes.
    pub skins: Vec<ImportedSkin>,
    /// Animations.
    pub animations: Vec<ImportedAnimation>,
}

/// One node: its local transform, children, mesh and skin.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedNode {
    /// The file's name for it.
    pub name: Option<String>,
    /// Local translation.
    pub translation: [f32; 3],
    /// Local rotation, a unit quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    /// Local scale.
    pub scale: [f32; 3],
    /// Indices into `Imported::nodes`.
    pub children: Vec<usize>,
    /// Index into `Imported::meshes`.
    pub mesh: Option<usize>,
    /// Index into `Imported::skins`.
    pub skin: Option<usize>,
}

/// A mesh: one or more primitives (one material each).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportedMesh {
    /// The file's name for it.
    pub name: Option<String>,
    /// Triangle lists.
    pub primitives: Vec<ImportedPrimitive>,
}

/// A triangle list with its vertex attributes (all of the same length as
/// `positions`, or empty when absent; normals are always present: computed
/// when the file has none).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportedPrimitive {
    /// Positions.
    pub positions: Vec<[f32; 3]>,
    /// Unit normals.
    pub normals: Vec<[f32; 3]>,
    /// TEXCOORD_0, `(0, 0)` top-left.
    pub uvs: Vec<[f32; 2]>,
    /// COLOR_0, linear RGBA.
    pub colors: Vec<[f32; 4]>,
    /// JOINTS_0.
    pub joints: Vec<[u16; 4]>,
    /// WEIGHTS_0 (normalized).
    pub weights: Vec<[f32; 4]>,
    /// Three per triangle, counter-clockwise from the front (generated when
    /// the file has none).
    pub indices: Vec<u32>,
    /// Index into `Imported::materials`.
    pub material: Option<usize>,
}

/// How a material's alpha is used.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ImportedAlpha {
    /// Alpha ignored.
    #[default]
    Opaque,
    /// Fragments below the cutoff are discarded.
    Mask(f32),
    /// Blended.
    Blend,
}

/// A metallic-roughness material. Texture fields index `Imported::images`.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedMaterial {
    /// The file's name for it.
    pub name: Option<String>,
    /// Base color factor, linear RGBA.
    pub base_color: [f32; 4],
    /// Base color texture (sRGB).
    pub base_texture: Option<usize>,
    /// Metallic factor.
    pub metallic: f32,
    /// Roughness factor.
    pub roughness: f32,
    /// Metallic-roughness texture (G roughness, B metalness).
    pub mr_texture: Option<usize>,
    /// Tangent-space normal map.
    pub normal_texture: Option<usize>,
    /// Normal map scale.
    pub normal_scale: f32,
    /// Emissive factor, linear RGB (times KHR_materials_emissive_strength).
    pub emissive: [f32; 3],
    /// Emissive texture (sRGB).
    pub emissive_texture: Option<usize>,
    /// Occlusion texture (R).
    pub occlusion_texture: Option<usize>,
    /// Alpha mode.
    pub alpha: ImportedAlpha,
    /// Both faces drawn.
    pub double_sided: bool,
    /// KHR_materials_unlit.
    pub unlit: bool,
}

/// An encoded image (PNG, JPEG, WebP).
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedImage {
    /// Its MIME type (`image/png`…).
    pub mime: String,
    /// Its bytes.
    pub bytes: Vec<u8>,
}

/// A skin: its joints (node indices) and their inverse bind matrices
/// (column-major).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportedSkin {
    /// Indices into `Imported::nodes`.
    pub joints: Vec<usize>,
    /// One per joint (identity when the file has none).
    pub inverse_bind: Vec<[f32; 16]>,
}

/// What a channel animates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportedPath {
    /// Translation (3 values per key).
    Translation,
    /// Rotation quaternion `[x, y, z, w]` (4 values per key).
    Rotation,
    /// Scale (3 values per key).
    Scale,
}

/// How a channel interpolates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportedInterpolation {
    /// Linear (spherical for rotations).
    Linear,
    /// Holds each key.
    Step,
    /// Cubic Hermite: each key has in-tangent, value, out-tangent.
    CubicSpline,
}

/// One animated property of one node.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedChannel {
    /// Index into `Imported::nodes`.
    pub node: usize,
    /// What it animates.
    pub path: ImportedPath,
    /// How it interpolates.
    pub interpolation: ImportedInterpolation,
    /// Key times, seconds, increasing.
    pub times: Vec<f32>,
    /// Key values, flat (× 3 per key for cubic splines).
    pub values: Vec<f32>,
}

/// An animation: channels played together.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportedAnimation {
    /// The file's name for it.
    pub name: Option<String>,
    /// Its channels (weights channels are skipped).
    pub channels: Vec<ImportedChannel>,
}

/// Reads a binary glTF (`.glb`). Buffers and images must be in the file
/// (BIN chunk or `data:` URIs): external URIs are not loaded.
pub fn import_glb(bytes: &[u8]) -> Result<Imported, ModelError> {
    #[cfg(feature = "gltf")]
    {
        imp::import_glb(bytes)
    }
    #[cfg(not(feature = "gltf"))]
    {
        let _ = bytes;
        Err(disabled())
    }
}

/// Reads a JSON glTF; `load` returns the bytes of a relative URI (buffers,
/// images); `data:` URIs are decoded here.
///
/// `load` gets the URI percent-decoded (`my%20mesh.bin` → `my mesh.bin`). A
/// `.glb` passed here is read as one (its external URIs through `load`).
pub fn import_gltf(
    json: &[u8],
    load: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Result<Imported, ModelError> {
    #[cfg(feature = "gltf")]
    {
        imp::import_gltf(json, load)
    }
    #[cfg(not(feature = "gltf"))]
    {
        let _ = (json, load);
        Err(disabled())
    }
}

#[cfg(not(feature = "gltf"))]
fn disabled() -> ModelError {
    ModelError::Unsupported("prism-core was built without its `gltf` feature".into())
}

#[cfg(feature = "gltf")]
mod imp {
    use super::*;
    use gltf_json as json;
    use json::accessor::{ComponentType, Type};
    use json::animation::{Interpolation, Property};
    use json::material::AlphaMode;
    use json::mesh::{Mode, Semantic};
    use json::validation::Checked;
    use std::borrow::Cow;

    /// Required extensions the importer reads, or can ignore without
    /// misreading the file.
    const KNOWN_REQUIRED: &[&str] = &[
        "KHR_materials_unlit",
        "KHR_materials_emissive_strength",
        "KHR_mesh_quantization",
        "KHR_texture_transform",
        "KHR_lights_punctual",
        "EXT_texture_webp",
    ];

    /// Cap on the components of an accessor without a buffer view (all zeros
    /// plus sparse values), so a bogus `count` cannot exhaust memory.
    const MAX_ZERO_COMPONENTS: usize = 1 << 26;

    fn parse(m: impl Into<String>) -> ModelError {
        ModelError::Parse(m.into())
    }

    pub(super) fn import_glb(bytes: &[u8]) -> Result<Imported, ModelError> {
        let (json, bin) = split_glb(bytes)?;
        import(json, bin, &|_| None)
    }

    pub(super) fn import_gltf(
        bytes: &[u8],
        load: &dyn Fn(&str) -> Option<Vec<u8>>,
    ) -> Result<Imported, ModelError> {
        if bytes.starts_with(b"glTF") {
            let (json, bin) = split_glb(bytes)?;
            return import(json, bin, load);
        }
        import(bytes, None, load)
    }

    fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
        let b = bytes.get(at..at.checked_add(4)?)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// The JSON and BIN chunks of a GLB.
    pub(super) fn split_glb(bytes: &[u8]) -> Result<(&[u8], Option<&[u8]>), ModelError> {
        if bytes.len() < 12 || !bytes.starts_with(b"glTF") {
            return Err(parse("not a GLB file"));
        }
        let version = u32_at(bytes, 4).unwrap_or(0);
        if version != 2 {
            return Err(ModelError::Unsupported(format!("GLB version {version}")));
        }
        let length = u32_at(bytes, 8).unwrap_or(0) as usize;
        if length < 12 || length > bytes.len() {
            return Err(parse("truncated GLB"));
        }
        let data = &bytes[..length];
        let (mut json, mut bin) = (None, None);
        let mut at = 12;
        while at + 8 <= data.len() {
            let len = u32_at(data, at).unwrap_or(0) as usize;
            let kind = &data[at + 4..at + 8];
            let start = at + 8;
            let end = start
                .checked_add(len)
                .filter(|&end| end <= data.len())
                .ok_or_else(|| parse("truncated GLB chunk"))?;
            let chunk = &data[start..end];
            if json.is_none() {
                if kind != b"JSON" {
                    return Err(parse("the GLB's first chunk is not JSON"));
                }
                json = Some(chunk);
            } else if kind == b"BIN\0" && bin.is_none() {
                bin = Some(chunk);
            }
            at = end;
        }
        let json = json.ok_or_else(|| parse("GLB without a JSON chunk"))?;
        Ok((json, bin))
    }

    fn import(
        json_bytes: &[u8],
        bin: Option<&[u8]>,
        load: &dyn Fn(&str) -> Option<Vec<u8>>,
    ) -> Result<Imported, ModelError> {
        let text = trim_json(json_bytes);
        let root: json::Root = match json::deserialize::from_slice(text) {
            Ok(root) => root,
            Err(e) => {
                // glTF 1.0 has another schema: say so rather than "missing field".
                let version = serde_json::from_slice::<serde_json::Value>(text)
                    .ok()
                    .and_then(|v| v.pointer("/asset/version")?.as_str().map(str::to_owned));
                return Err(match version {
                    Some(v) if !v.starts_with("2.") && v != "2" => {
                        ModelError::Unsupported(format!("glTF version {v}"))
                    }
                    _ => parse(e.to_string()),
                });
            }
        };
        let version = root.asset.version.as_str();
        if version.split('.').next() != Some("2") {
            return Err(ModelError::Unsupported(format!("glTF version {version}")));
        }
        if let Some(ext) = root
            .extensions_required
            .iter()
            .find(|e| !KNOWN_REQUIRED.contains(&e.as_str()))
        {
            return Err(ModelError::Unsupported(format!("required extension {ext}")));
        }
        Ok(Reader::new(&root, bin, load).import())
    }

    /// Drops a UTF-8 BOM and trailing padding (spaces, or NULs some
    /// exporters write).
    fn trim_json(mut b: &[u8]) -> &[u8] {
        if let Some(rest) = b.strip_prefix(b"\xEF\xBB\xBF") {
            b = rest;
        }
        while let [rest @ .., last] = b {
            if *last == 0 || last.is_ascii_whitespace() {
                b = rest;
            } else {
                break;
            }
        }
        b
    }

    struct Reader<'a> {
        root: &'a json::Root,
        buffers: Vec<Option<Cow<'a, [u8]>>>,
        load: &'a dyn Fn(&str) -> Option<Vec<u8>>,
    }

    impl<'a> Reader<'a> {
        fn new(
            root: &'a json::Root,
            bin: Option<&'a [u8]>,
            load: &'a dyn Fn(&str) -> Option<Vec<u8>>,
        ) -> Self {
            let buffers = root
                .buffers
                .iter()
                .enumerate()
                .map(|(i, b)| match &b.uri {
                    None if i == 0 => bin.map(Cow::Borrowed),
                    None => None,
                    Some(uri) => match data_uri(uri) {
                        Some((bytes, _)) => Some(Cow::Owned(bytes)),
                        None if uri.starts_with("data:") => None,
                        None => load(&percent_decode(uri)).map(Cow::Owned),
                    },
                })
                .collect();
            Reader {
                root,
                buffers,
                load,
            }
        }

        fn import(&self) -> Imported {
            let root = self.root;
            let count = root.nodes.len();
            let scene = root
                .scene
                .and_then(|s| root.scenes.get(s.value()))
                .or(root.scenes.first());
            let mut scene_roots: Option<Vec<usize>> = scene.map(|s| {
                let mut seen = vec![false; count];
                s.nodes
                    .iter()
                    .map(|n| n.value())
                    .filter(|&n| n < count && !std::mem::replace(&mut seen[n], true))
                    .collect()
            });
            let (children, parentless) = forest(root, scene_roots.as_deref().unwrap_or(&[]));
            let roots = scene_roots.take().unwrap_or(parentless);

            let skins: Vec<Option<ImportedSkin>> =
                root.skins.iter().map(|s| self.skin(s)).collect();
            let nodes = root
                .nodes
                .iter()
                .zip(children)
                .map(|(n, children)| {
                    let (translation, rotation, scale) = node_trs(n);
                    ImportedNode {
                        name: n.name.clone(),
                        translation,
                        rotation,
                        scale,
                        children,
                        mesh: n.mesh.map(|m| m.value()).filter(|&m| m < root.meshes.len()),
                        skin: n
                            .skin
                            .map(|s| s.value())
                            .filter(|&s| skins.get(s).is_some_and(Option::is_some)),
                    }
                })
                .collect();
            let meshes = root
                .meshes
                .iter()
                .map(|m| ImportedMesh {
                    name: m.name.clone(),
                    primitives: m
                        .primitives
                        .iter()
                        .filter_map(|p| self.primitive(p))
                        .collect(),
                })
                .collect();
            let mut images = Images {
                cache: vec![None; root.images.len()],
                out: Vec::new(),
            };
            let materials = root
                .materials
                .iter()
                .map(|m| self.material(m, &mut images))
                .collect();
            let animations = root
                .animations
                .iter()
                .filter_map(|a| self.animation(a))
                .collect();
            Imported {
                roots,
                nodes,
                meshes,
                materials,
                images: images.out,
                skins: skins.into_iter().map(Option::unwrap_or_default).collect(),
                animations,
            }
        }

        // ---- buffers and accessors ----

        /// A buffer view's bytes and its byte stride.
        fn view(&self, index: usize) -> Option<(&[u8], Option<usize>)> {
            let v = self.root.buffer_views.get(index)?;
            let buffer = self.buffers.get(v.buffer.value())?.as_deref()?;
            let offset = usize::try_from(v.byte_offset.map_or(0, |o| o.0)).ok()?;
            let length = usize::try_from(v.byte_length.0).ok()?;
            let bytes = buffer.get(offset..offset.checked_add(length)?)?;
            Some((bytes, v.byte_stride.map(|s| s.0).filter(|&s| s > 0)))
        }

        /// An accessor's elements, flat, decoded by `decode` (component type,
        /// component bytes), and its component count per element. `None` when
        /// it cannot be read.
        fn read<T: Copy>(
            &self,
            index: usize,
            decode: impl Fn(ComponentType, &[u8]) -> T,
        ) -> Option<(Vec<T>, usize)> {
            let acc = self.root.accessors.get(index)?;
            let Checked::Valid(ct) = acc.component_type else {
                return None;
            };
            let ct = ct.0;
            let Checked::Valid(ty) = acc.type_ else {
                return None;
            };
            let n = ty.multiplicity();
            let cs = ct.size();
            let count = usize::try_from(acc.count.0).ok()?;
            // Matrix columns start on 4-byte boundaries.
            let (cols, rows) = match ty {
                Type::Mat2 => (2, 2),
                Type::Mat3 => (3, 3),
                Type::Mat4 => (4, 4),
                _ => (1, n),
            };
            let col_stride = if cols > 1 {
                (rows * cs + 3) & !3
            } else {
                rows * cs
            };
            let elem = cols * col_stride;
            let comp = |k: usize| (k / rows) * col_stride + (k % rows) * cs;
            let total = count.checked_mul(n)?;

            let mut out = match acc.buffer_view {
                Some(view) => {
                    let (bytes, stride) = self.view(view.value())?;
                    let stride = stride.unwrap_or(elem);
                    let base = usize::try_from(acc.byte_offset.map_or(0, |o| o.0)).ok()?;
                    if count > 0 {
                        let end = (count - 1)
                            .checked_mul(stride)?
                            .checked_add(base)?
                            .checked_add(elem)?;
                        if end > bytes.len() {
                            return None;
                        }
                    }
                    let mut out = Vec::with_capacity(total);
                    for i in 0..count {
                        let at = base + i * stride;
                        for k in 0..n {
                            let o = at + comp(k);
                            out.push(decode(ct, &bytes[o..o + cs]));
                        }
                    }
                    out
                }
                None => {
                    if total > MAX_ZERO_COMPONENTS {
                        return None;
                    }
                    vec![decode(ct, &[0; 4][..cs]); total]
                }
            };

            if let Some(sparse) = &acc.sparse {
                let scount = usize::try_from(sparse.count.0).ok()?;
                let Checked::Valid(ict) = sparse.indices.component_type else {
                    return None;
                };
                let ict = ict.0;
                let ics = ict.size();
                let (ib, _) = self.view(sparse.indices.buffer_view.value())?;
                let ioff = usize::try_from(sparse.indices.byte_offset.0).ok()?;
                let ib = ib.get(ioff..)?.get(..scount.checked_mul(ics)?)?;
                let (vb, _) = self.view(sparse.values.buffer_view.value())?;
                let voff = usize::try_from(sparse.values.byte_offset.0).ok()?;
                let vb = vb.get(voff..)?.get(..scount.checked_mul(elem)?)?;
                for s in 0..scount {
                    let target = to_u32(ict, &ib[s * ics..(s + 1) * ics]) as usize;
                    if target >= count {
                        continue;
                    }
                    for k in 0..n {
                        let o = s * elem + comp(k);
                        out[target * n + k] = decode(ct, &vb[o..o + cs]);
                    }
                }
            }
            Some((out, n))
        }

        /// An accessor as floats: normalized integers mapped to [0, 1] or
        /// [-1, 1] when the accessor says so or `normalized` forces it;
        /// non-finite values become 0.
        fn floats(&self, index: usize, normalized: bool) -> Option<(Vec<f32>, usize)> {
            let normalized = normalized || self.root.accessors.get(index)?.normalized;
            self.read(index, |ct, b| to_f32(ct, b, normalized))
        }

        /// An accessor as unsigned integers (`u32::MAX` for a float that is
        /// not one).
        fn ints(&self, index: usize) -> Option<(Vec<u32>, usize)> {
            self.read(index, to_u32)
        }

        // ---- meshes ----

        fn primitive(&self, p: &json::mesh::Primitive) -> Option<ImportedPrimitive> {
            let Checked::Valid(mode) = p.mode else {
                return None;
            };
            if !matches!(
                mode,
                Mode::Triangles | Mode::TriangleStrip | Mode::TriangleFan
            ) {
                return None;
            }
            let attr = |s: Semantic| p.attributes.get(&Checked::Valid(s)).map(|i| i.value());
            // An attribute with one of `widths` components per vertex, one
            // element per position.
            let attribute = |s: Semantic, widths: &[usize], normalized: bool, len: usize| {
                attr(s)
                    .and_then(|i| self.floats(i, normalized))
                    .filter(|(v, n)| widths.contains(n) && v.len() == len * n)
            };

            let (pos, n) = self.floats(attr(Semantic::Positions)?, false)?;
            if n != 3 || pos.is_empty() {
                return None;
            }
            let positions: Vec<[f32; 3]> = pos.as_chunks::<3>().0.to_vec();
            let len = positions.len();
            let len32 = u32::try_from(len).ok()?;

            let raw_indices: Vec<u32> = match p.indices {
                Some(i) => {
                    let (idx, n) = self.ints(i.value())?;
                    if n != 1 {
                        return None;
                    }
                    idx
                }
                None => (0..len32).collect(),
            };
            let indices = triangles(mode, &raw_indices, len32);
            if indices.is_empty() {
                return None;
            }

            let normals = match attribute(Semantic::Normals, &[3], false, len) {
                Some((v, _)) => v
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|&c| normalize3(c))
                    .collect(),
                None => smooth_normals(&positions, &indices),
            };
            let uvs = attribute(Semantic::TexCoords(0), &[2], false, len)
                .map(|(v, _)| v.as_chunks::<2>().0.to_vec())
                .unwrap_or_default();
            let colors = attribute(Semantic::Colors(0), &[3, 4], true, len)
                .map(|(v, n)| {
                    v.chunks_exact(n)
                        .map(|c| [c[0], c[1], c[2], if n == 4 { c[3] } else { 1. }])
                        .collect()
                })
                .unwrap_or_default();
            let joints: Option<Vec<[u16; 4]>> = attr(Semantic::Joints(0))
                .and_then(|i| self.ints(i))
                .filter(|(v, n)| *n == 4 && v.len() == len * 4)
                .map(|(v, _)| {
                    v.as_chunks::<4>()
                        .0
                        .iter()
                        .map(|c| c.map(|j| u16::try_from(j).unwrap_or(0)))
                        .collect()
                });
            let weights: Option<Vec<[f32; 4]>> = attribute(Semantic::Weights(0), &[4], true, len)
                .map(|(v, _)| {
                    v.as_chunks::<4>()
                        .0
                        .iter()
                        .map(|&c| normalize_weights(c))
                        .collect()
                });
            // Skinning needs both.
            let (joints, weights) = match (joints, weights) {
                (Some(j), Some(w)) => (j, w),
                _ => (Vec::new(), Vec::new()),
            };

            Some(ImportedPrimitive {
                positions,
                normals,
                uvs,
                colors,
                joints,
                weights,
                indices,
                material: p
                    .material
                    .map(|m| m.value())
                    .filter(|&m| m < self.root.materials.len()),
            })
        }

        // ---- materials and images ----

        fn material(&self, m: &json::Material, images: &mut Images) -> ImportedMaterial {
            let mut tex = |index: usize, tex_coord: u32| {
                if tex_coord != 0 {
                    return None;
                }
                images.texture(self, index)
            };
            let pbr = &m.pbr_metallic_roughness;
            let base = pbr.base_color_factor.0;
            let strength = m
                .extensions
                .as_ref()
                .and_then(|e| e.emissive_strength.as_ref())
                .map_or(1., |s| s.emissive_strength.0);
            let strength = finite(strength, 1.).max(0.);
            let emissive = m
                .emissive_factor
                .0
                .map(|c| finite(c, 0.).max(0.) * strength);
            ImportedMaterial {
                name: m.name.clone(),
                base_color: base.map(|c| finite(c, 1.).clamp(0., 1.)),
                base_texture: pbr
                    .base_color_texture
                    .as_ref()
                    .and_then(|t| tex(t.index.value(), t.tex_coord)),
                metallic: finite(pbr.metallic_factor.0, 1.).clamp(0., 1.),
                roughness: finite(pbr.roughness_factor.0, 1.).clamp(0., 1.),
                mr_texture: pbr
                    .metallic_roughness_texture
                    .as_ref()
                    .and_then(|t| tex(t.index.value(), t.tex_coord)),
                normal_texture: m
                    .normal_texture
                    .as_ref()
                    .and_then(|t| tex(t.index.value(), t.tex_coord)),
                normal_scale: m
                    .normal_texture
                    .as_ref()
                    .map_or(1., |t| finite(t.scale, 1.)),
                emissive,
                emissive_texture: m
                    .emissive_texture
                    .as_ref()
                    .and_then(|t| tex(t.index.value(), t.tex_coord)),
                occlusion_texture: m
                    .occlusion_texture
                    .as_ref()
                    .and_then(|t| tex(t.index.value(), t.tex_coord)),
                alpha: match m.alpha_mode {
                    Checked::Valid(AlphaMode::Mask) => {
                        ImportedAlpha::Mask(m.alpha_cutoff.map_or(0.5, |c| finite(c.0, 0.5)))
                    }
                    Checked::Valid(AlphaMode::Blend) => ImportedAlpha::Blend,
                    _ => ImportedAlpha::Opaque,
                },
                double_sided: m.double_sided,
                unlit: m.extensions.as_ref().is_some_and(|e| e.unlit.is_some()),
            }
        }

        /// The encoded bytes and MIME type of a file's image.
        fn image(&self, index: usize) -> Option<ImportedImage> {
            let im = self.root.images.get(index)?;
            let (bytes, uri_mime) = if let Some(view) = im.buffer_view {
                (self.view(view.value())?.0.to_vec(), None)
            } else {
                let uri = im.uri.as_deref()?;
                match data_uri(uri) {
                    Some((bytes, mime)) => (bytes, mime),
                    None if uri.starts_with("data:") => return None,
                    None => ((self.load)(&percent_decode(uri))?, None),
                }
            };
            if bytes.is_empty() {
                return None;
            }
            let declared = im.mime_type.as_ref().map(|m| m.0.clone()).or(uri_mime);
            let mime = sniff(&bytes)
                .map(str::to_owned)
                .or(declared.filter(|m| m.starts_with("image/")))?;
            Some(ImportedImage { mime, bytes })
        }

        // ---- skins and animations ----

        fn skin(&self, s: &json::Skin) -> Option<ImportedSkin> {
            let joints: Vec<usize> = s.joints.iter().map(|j| j.value()).collect();
            if joints.is_empty() || joints.iter().any(|&j| j >= self.root.nodes.len()) {
                return None;
            }
            let matrices = s
                .inverse_bind_matrices
                .and_then(|a| self.floats(a.value(), false))
                .filter(|(v, n)| *n == 16 && v.len() >= joints.len() * 16);
            let inverse_bind = match matrices {
                Some((v, _)) => v.as_chunks::<16>().0[..joints.len()].to_vec(),
                None => vec![IDENTITY; joints.len()],
            };
            Some(ImportedSkin {
                joints,
                inverse_bind,
            })
        }

        fn animation(&self, a: &json::Animation) -> Option<ImportedAnimation> {
            let channels: Vec<ImportedChannel> = a
                .channels
                .iter()
                .filter_map(|c| self.channel(a, c))
                .collect();
            (!channels.is_empty()).then(|| ImportedAnimation {
                name: a.name.clone(),
                channels,
            })
        }

        fn channel(
            &self,
            a: &json::Animation,
            c: &json::animation::Channel,
        ) -> Option<ImportedChannel> {
            let node = c.target.node.value();
            if node >= self.root.nodes.len() {
                return None;
            }
            let (path, width) = match c.target.path {
                Checked::Valid(Property::Translation) => (ImportedPath::Translation, 3),
                Checked::Valid(Property::Rotation) => (ImportedPath::Rotation, 4),
                Checked::Valid(Property::Scale) => (ImportedPath::Scale, 3),
                _ => return None,
            };
            let sampler = a.samplers.get(c.sampler.value())?;
            let interpolation = match sampler.interpolation {
                Checked::Valid(Interpolation::Linear) => ImportedInterpolation::Linear,
                Checked::Valid(Interpolation::Step) => ImportedInterpolation::Step,
                Checked::Valid(Interpolation::CubicSpline) => ImportedInterpolation::CubicSpline,
                Checked::Invalid => return None,
            };
            let (times, n) = self.floats(sampler.input.value(), false)?;
            if n != 1 || times.is_empty() || times.windows(2).any(|w| w[0] > w[1]) {
                return None;
            }
            // Integer rotations are always normalized (KHR_mesh_quantization).
            let normalized = path == ImportedPath::Rotation;
            let (mut values, n) = self.floats(sampler.output.value(), normalized)?;
            let per_key = if interpolation == ImportedInterpolation::CubicSpline {
                3
            } else {
                1
            };
            let wanted = times.len() * per_key * width;
            if n != width || values.len() < wanted {
                return None;
            }
            values.truncate(wanted);
            if path == ImportedPath::Rotation {
                for (i, q) in values.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    // Cubic splines: only the value of each triplet is a rotation.
                    if per_key == 1 || i % 3 == 1 {
                        *q = normalize_quat(*q);
                    }
                }
            }
            Some(ImportedChannel {
                node,
                path,
                interpolation,
                times,
                values,
            })
        }
    }

    /// Images used by textures, deduplicated: per file image, its index in
    /// `out` once read (`Some(None)`: unreadable).
    struct Images {
        cache: Vec<Option<Option<usize>>>,
        out: Vec<ImportedImage>,
    }

    impl Images {
        /// The image of a texture: its `EXT_texture_webp` source if
        /// readable, else its core `source`.
        fn texture(&mut self, reader: &Reader<'_>, texture: usize) -> Option<usize> {
            let t = reader.root.textures.get(texture)?;
            let webp = t
                .extensions
                .as_ref()
                .and_then(|e| e.others.get("EXT_texture_webp")?.get("source")?.as_u64())
                .and_then(|i| usize::try_from(i).ok());
            webp.into_iter()
                .chain([t.source.value()])
                .find_map(|image| self.image(reader, image))
        }

        fn image(&mut self, reader: &Reader<'_>, image: usize) -> Option<usize> {
            let slot = self.cache.get_mut(image)?;
            if let Some(done) = *slot {
                return done;
            }
            let index = reader.image(image).map(|im| {
                match self.out.iter().position(|o| o.bytes == im.bytes) {
                    Some(i) => i,
                    None => {
                        self.out.push(im);
                        self.out.len() - 1
                    }
                }
            });
            self.cache[image] = Some(index);
            index
        }
    }

    /// Children per node, so the node graph is a forest: a node keeps its
    /// first parent (none for a scene root), self-references and cycles are
    /// cut. Also returns the parentless nodes in index order.
    fn forest(root: &json::Root, scene_roots: &[usize]) -> (Vec<Vec<usize>>, Vec<usize>) {
        let count = root.nodes.len();
        let mut claimed = vec![false; count];
        for &r in scene_roots {
            claimed[r] = true;
        }
        let mut parent: Vec<Option<usize>> = vec![None; count];
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); count];
        for (i, node) in root.nodes.iter().enumerate() {
            for c in node.children.iter().flatten() {
                let c = c.value();
                if c < count && c != i && !claimed[c] {
                    claimed[c] = true;
                    parent[c] = Some(i);
                    children[i].push(c);
                }
            }
        }
        let parentless: Vec<usize> = (0..count).filter(|&i| parent[i].is_none()).collect();
        // Every node has at most one parent now: those not reached from a
        // parentless node are on (or under) a cycle; cut one edge per cycle.
        let mut reached = vec![false; count];
        fn mark(start: usize, reached: &mut [bool], children: &[Vec<usize>]) {
            let mut stack = vec![start];
            while let Some(i) = stack.pop() {
                if !std::mem::replace(&mut reached[i], true) {
                    stack.extend(&children[i]);
                }
            }
        }
        for &i in &parentless {
            mark(i, &mut reached, &children);
        }
        for i in 0..count {
            if !reached[i] {
                if let Some(p) = parent[i].take() {
                    children[p].retain(|&c| c != i);
                }
                mark(i, &mut reached, &children);
            }
        }
        (children, parentless)
    }

    // ---- conversions ----

    fn to_f32(ct: ComponentType, b: &[u8], normalized: bool) -> f32 {
        let v = match ct {
            ComponentType::I8 => {
                let x = f32::from(b[0] as i8);
                if normalized { (x / 127.).max(-1.) } else { x }
            }
            ComponentType::U8 => {
                let x = f32::from(b[0]);
                if normalized { x / 255. } else { x }
            }
            ComponentType::I16 => {
                let x = f32::from(i16::from_le_bytes([b[0], b[1]]));
                if normalized { (x / 32767.).max(-1.) } else { x }
            }
            ComponentType::U16 => {
                let x = f32::from(u16::from_le_bytes([b[0], b[1]]));
                if normalized { x / 65535. } else { x }
            }
            ComponentType::U32 => {
                let x = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                if normalized {
                    (f64::from(x) / f64::from(u32::MAX)) as f32
                } else {
                    x as f32
                }
            }
            ComponentType::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        };
        finite(v, 0.)
    }

    fn to_u32(ct: ComponentType, b: &[u8]) -> u32 {
        match ct {
            ComponentType::I8 => u32::try_from(b[0] as i8).unwrap_or(u32::MAX),
            ComponentType::U8 => u32::from(b[0]),
            ComponentType::I16 => {
                u32::try_from(i16::from_le_bytes([b[0], b[1]])).unwrap_or(u32::MAX)
            }
            ComponentType::U16 => u32::from(u16::from_le_bytes([b[0], b[1]])),
            ComponentType::U32 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            ComponentType::F32 => {
                let f = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                if f >= 0. && f < u32::MAX as f32 && f.fract() == 0. {
                    f as u32
                } else {
                    u32::MAX
                }
            }
        }
    }

    fn finite(v: f32, fallback: f32) -> f32 {
        if v.is_finite() { v } else { fallback }
    }

    const IDENTITY: [f32; 16] = [
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ];

    /// Triangle list from a primitive's indices, dropping triangles with an
    /// out-of-range index (and the degenerate ones of strips and fans).
    pub(super) fn triangles(mode: Mode, idx: &[u32], vertices: u32) -> Vec<u32> {
        let mut out = Vec::with_capacity(idx.len().max(3) / 3 * 3);
        let mut push = |a: u32, b: u32, c: u32, skip_degenerate: bool| {
            if a < vertices && b < vertices && c < vertices {
                if skip_degenerate && (a == b || b == c || a == c) {
                    return;
                }
                out.extend([a, b, c]);
            }
        };
        match mode {
            Mode::Triangles => {
                for &[a, b, c] in idx.as_chunks::<3>().0 {
                    push(a, b, c, false);
                }
            }
            Mode::TriangleStrip => {
                for i in 0..idx.len().saturating_sub(2) {
                    let (b, c) = if i % 2 == 0 {
                        (i + 1, i + 2)
                    } else {
                        (i + 2, i + 1)
                    };
                    push(idx[i], idx[b], idx[c], true);
                }
            }
            Mode::TriangleFan => {
                for i in 1..idx.len().saturating_sub(1) {
                    push(idx[i], idx[i + 1], idx[0], true);
                }
            }
            _ => {}
        }
        out
    }

    /// Per-vertex normals: the area-weighted sum of the adjacent faces'
    /// normals (the unnormalized cross product is twice the area).
    fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
        let mut sum = vec![[0f32; 3]; positions.len()];
        for t in indices.as_chunks::<3>().0 {
            let [a, b, c] = t.map(|i| i as usize);
            let (pa, pb, pc) = (positions[a], positions[b], positions[c]);
            let e1 = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
            let e2 = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
            let n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            for v in [a, b, c] {
                for k in 0..3 {
                    sum[v][k] += n[k];
                }
            }
        }
        sum.into_iter().map(normalize3).collect()
    }

    /// Unit vector; `+Y` for a zero or non-finite one.
    fn normalize3(v: [f32; 3]) -> [f32; 3] {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if l > 1e-20 && l.is_finite() {
            v.map(|c| c / l)
        } else {
            [0., 1., 0.]
        }
    }

    /// Unit quaternion; identity for a zero or non-finite one.
    pub(super) fn normalize_quat(q: [f32; 4]) -> [f32; 4] {
        let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        if l > 1e-20 && l.is_finite() {
            q.map(|c| c / l)
        } else {
            [0., 0., 0., 1.]
        }
    }

    /// Weights summing to 1 (all on the first joint when they sum to 0).
    fn normalize_weights(w: [f32; 4]) -> [f32; 4] {
        let w = w.map(|x| x.max(0.));
        let sum: f32 = w.iter().sum();
        if sum > 1e-12 {
            w.map(|x| x / sum)
        } else {
            [1., 0., 0., 0.]
        }
    }

    fn node_trs(n: &json::Node) -> ([f32; 3], [f32; 4], [f32; 3]) {
        let (t, r, s) = match n.matrix {
            Some(m) if m != IDENTITY => decompose(m),
            _ => (
                n.translation.unwrap_or([0.; 3]),
                n.rotation.map_or([0., 0., 0., 1.], |q| q.0),
                n.scale.unwrap_or([1.; 3]),
            ),
        };
        (
            t.map(|c| finite(c, 0.)),
            normalize_quat(r),
            s.map(|c| finite(c, 1.)),
        )
    }

    /// A column-major affine matrix as translation, unit rotation and scale.
    /// Shear is lost; a mirroring matrix (negative determinant) gets a
    /// negative X scale; a zero-length axis leaves the rotation identity.
    pub(super) fn decompose(m: [f32; 16]) -> ([f32; 3], [f32; 4], [f32; 3]) {
        let t = [m[12], m[13], m[14]];
        let cols = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
        let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let mut s = cols.map(len);
        let [a, b, c] = cols;
        let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - b[0] * (a[1] * c[2] - a[2] * c[1])
            + c[0] * (a[1] * b[2] - a[2] * b[1]);
        if det < 0. {
            s[0] = -s[0];
        }
        // (Written so a NaN scale also lands here.)
        if s.iter()
            .any(|x| x.abs().partial_cmp(&1e-12) != Some(std::cmp::Ordering::Greater))
        {
            return (t, [0., 0., 0., 1.], s);
        }
        // r[col][row] of the rotation.
        let r: [[f32; 3]; 3] = std::array::from_fn(|i| cols[i].map(|x| x / s[i]));
        let (m00, m11, m22) = (r[0][0], r[1][1], r[2][2]);
        let (m01, m02) = (r[1][0], r[2][0]);
        let (m10, m12) = (r[0][1], r[2][1]);
        let (m20, m21) = (r[0][2], r[1][2]);
        let trace = m00 + m11 + m22;
        let q = if trace > 0. {
            let k = 0.5 / (trace + 1.).sqrt();
            [(m21 - m12) * k, (m02 - m20) * k, (m10 - m01) * k, 0.25 / k]
        } else if m00 > m11 && m00 > m22 {
            let k = 2. * (1. + m00 - m11 - m22).sqrt();
            [0.25 * k, (m01 + m10) / k, (m02 + m20) / k, (m21 - m12) / k]
        } else if m11 > m22 {
            let k = 2. * (1. + m11 - m00 - m22).sqrt();
            [(m01 + m10) / k, 0.25 * k, (m12 + m21) / k, (m02 - m20) / k]
        } else {
            let k = 2. * (1. + m22 - m00 - m11).sqrt();
            [(m02 + m20) / k, (m12 + m21) / k, 0.25 * k, (m10 - m01) / k]
        };
        (t, normalize_quat(q), s)
    }

    // ---- URIs ----

    /// The bytes and MIME type of a `data:` URI.
    fn data_uri(uri: &str) -> Option<(Vec<u8>, Option<String>)> {
        let rest = uri.strip_prefix("data:")?;
        let (header, body) = rest.split_once(',')?;
        let (base64, header) = match header.strip_suffix(";base64") {
            Some(h) => (true, h),
            None => (false, header),
        };
        let mime = header.split(';').next().filter(|m| !m.is_empty());
        let bytes = if base64 {
            decode_base64(body)?
        } else {
            percent_decode_bytes(body)
        };
        Some((bytes, mime.map(str::to_owned)))
    }

    /// Standard or URL-safe base64, padding optional, whitespace ignored.
    pub(super) fn decode_base64(s: &str) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(s.len() / 4 * 3 + 3);
        let (mut acc, mut bits) = (0u32, 0u32);
        for c in s.bytes() {
            let v = match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' | b'-' => 62,
                b'/' | b'_' => 63,
                b'=' => break,
                b' ' | b'\n' | b'\r' | b'\t' => continue,
                _ => return None,
            };
            acc = (acc << 6) | u32::from(v);
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        Some(out)
    }

    fn percent_decode_bytes(s: &str) -> Vec<u8> {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let hex = |c: u8| (c as char).to_digit(16);
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%'
                && let (Some(h), Some(l)) = (
                    b.get(i + 1).copied().and_then(hex),
                    b.get(i + 2).copied().and_then(hex),
                )
            {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
            out.push(b[i]);
            i += 1;
        }
        out
    }

    pub(super) fn percent_decode(s: &str) -> String {
        String::from_utf8_lossy(&percent_decode_bytes(s)).into_owned()
    }

    /// The MIME type of an encoded image, from its first bytes.
    pub(super) fn sniff(b: &[u8]) -> Option<&'static str> {
        if b.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some("image/png")
        } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some("image/jpeg")
        } else if b.len() >= 12 && b.starts_with(b"RIFF") && &b[8..12] == b"WEBP" {
            Some("image/webp")
        } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
            Some("image/gif")
        } else if b.starts_with(b"\xABKTX 20\xBB\r\n\x1A\n") {
            Some("image/ktx2")
        } else {
            None
        }
    }
}

#[cfg(all(test, not(feature = "gltf")))]
mod disabled_tests {
    use super::*;

    #[test]
    fn without_the_feature_import_is_unsupported() {
        assert!(matches!(
            import_glb(b"glTF"),
            Err(ModelError::Unsupported(_))
        ));
        assert!(matches!(
            import_gltf(b"{}", &|_| None),
            Err(ModelError::Unsupported(_))
        ));
    }
}

#[cfg(all(test, feature = "gltf"))]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// A BIN chunk under construction: each push is 4-byte aligned and
    /// returns its buffer view index (views are recorded in `views`).
    #[derive(Default)]
    struct Bin {
        bytes: Vec<u8>,
        views: Vec<Value>,
    }

    impl Bin {
        fn raw(&mut self, data: &[u8], stride: Option<usize>) -> usize {
            while !self.bytes.len().is_multiple_of(4) {
                self.bytes.push(0);
            }
            let mut view = json!({
                "buffer": 0, "byteOffset": self.bytes.len(), "byteLength": data.len()
            });
            if let Some(s) = stride {
                view["byteStride"] = json!(s);
            }
            self.bytes.extend_from_slice(data);
            self.views.push(view);
            self.views.len() - 1
        }
        fn f32s(&mut self, v: &[f32]) -> usize {
            let b: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            self.raw(&b, None)
        }
        fn u16s(&mut self, v: &[u16]) -> usize {
            let b: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            self.raw(&b, None)
        }
    }

    /// `doc` with the BIN's buffer and views added, as a GLB.
    fn glb(mut doc: Value, bin: &Bin) -> Vec<u8> {
        if !bin.bytes.is_empty() {
            doc["buffers"] = json!([{ "byteLength": bin.bytes.len() }]);
            doc["bufferViews"] = json!(bin.views);
        }
        assemble(&serde_json::to_vec(&doc).unwrap(), &bin.bytes)
    }

    /// A GLB container from a JSON chunk and a BIN chunk.
    fn assemble(json: &[u8], bin: &[u8]) -> Vec<u8> {
        let pad = |b: &[u8], with: u8| {
            let mut v = b.to_vec();
            while !v.len().is_multiple_of(4) {
                v.push(with);
            }
            v
        };
        let (json, bin) = (pad(json, b' '), pad(bin, 0));
        let mut out = b"glTF".to_vec();
        out.extend(2u32.to_le_bytes());
        let total = 12 + 8 + json.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
        out.extend((total as u32).to_le_bytes());
        out.extend((json.len() as u32).to_le_bytes());
        out.extend(b"JSON");
        out.extend(&json);
        if !bin.is_empty() {
            out.extend((bin.len() as u32).to_le_bytes());
            out.extend(b"BIN\0");
            out.extend(&bin);
        }
        out
    }

    fn accessor(view: usize, ct: u32, count: usize, ty: &str) -> Value {
        json!({ "bufferView": view, "componentType": ct, "count": count, "type": ty })
    }

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    const FLOAT: u32 = 5126;
    const UBYTE: u32 = 5121;
    const USHORT: u32 = 5123;

    /// One triangle, interleaved positions + normals (stride 24), u16 indices.
    fn triangle() -> (Value, Bin) {
        let mut bin = Bin::default();
        #[rustfmt::skip]
        let interleaved: Vec<u8> = [
            0., 0., 0., 0., 0., 2.,
            1., 0., 0., 0., 0., 2.,
            0., 1., 0., 0., 0., 2.,
        ]
        .iter()
        .flat_map(|x: &f32| x.to_le_bytes())
        .collect();
        let vb = bin.raw(&interleaved, Some(24));
        let ib = bin.u16s(&[0, 1, 2]);
        let mut normals = accessor(vb, FLOAT, 3, "VEC3");
        normals["byteOffset"] = json!(12);
        let doc = json!({
            "asset": { "version": "2.0" },
            "scenes": [{ "nodes": [0] }],
            "nodes": [{ "name": "tri", "mesh": 0 }],
            "meshes": [{ "name": "m", "primitives": [{
                "attributes": { "POSITION": 0, "NORMAL": 1 }, "indices": 2
            }]}],
            "accessors": [accessor(vb, FLOAT, 3, "VEC3"), normals, accessor(ib, USHORT, 3, "SCALAR")],
        });
        (doc, bin)
    }

    #[test]
    fn triangle_with_indices() {
        let (doc, bin) = triangle();
        let m = import_glb(&glb(doc, &bin)).unwrap();
        assert_eq!(m.roots, vec![0]);
        assert_eq!(m.nodes[0].name.as_deref(), Some("tri"));
        assert_eq!(m.nodes[0].mesh, Some(0));
        assert_eq!(m.nodes[0].rotation, [0., 0., 0., 1.]);
        assert_eq!(m.meshes[0].name.as_deref(), Some("m"));
        let p = &m.meshes[0].primitives[0];
        assert_eq!(p.positions, vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]);
        // Given normals are normalized.
        assert_eq!(p.normals, vec![[0., 0., 1.]; 3]);
        assert_eq!(p.indices, vec![0, 1, 2]);
        assert!(p.uvs.is_empty() && p.colors.is_empty() && p.joints.is_empty());
        assert_eq!(p.material, None);
    }

    #[test]
    fn unindexed_mesh_gets_indices_and_flat_normals() {
        let mut bin = Bin::default();
        #[rustfmt::skip]
        let quad = bin.f32s(&[
            0., 0., 0.,  1., 0., 0.,  1., 1., 0.,
            0., 0., 0.,  1., 1., 0.,  0., 1., 0.,
        ]);
        let doc = json!({
            "asset": { "version": "2.0" },
            "nodes": [{ "mesh": 0 }],
            "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }],
            "accessors": [accessor(quad, FLOAT, 6, "VEC3")],
        });
        let m = import_glb(&glb(doc, &bin)).unwrap();
        // No scene: every parentless node is a root.
        assert_eq!(m.roots, vec![0]);
        let p = &m.meshes[0].primitives[0];
        assert_eq!(p.indices, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(p.normals, vec![[0., 0., 1.]; 6]);
    }

    #[test]
    fn computed_normals_are_area_weighted() {
        let mut bin = Bin::default();
        // Triangle A in the XY plane (normal +Z, area 0.5) and triangle B in
        // the XZ plane (normal +Y, area 2) share vertex 0.
        #[rustfmt::skip]
        let pos = bin.f32s(&[
            0., 0., 0.,  1., 0., 0.,  0., 1., 0.,
            0., 0., 2.,  2., 0., 0.,
        ]);
        let idx = bin.u16s(&[0, 1, 2, 0, 3, 4]);
        let doc = json!({
            "asset": { "version": "2.0" },
            "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 }, "indices": 1 }] }],
            "accessors": [accessor(pos, FLOAT, 5, "VEC3"), accessor(idx, USHORT, 6, "SCALAR")],
        });
        let m = import_glb(&glb(doc, &bin)).unwrap();
        let n = &m.meshes[0].primitives[0].normals;
        let l = 17f32.sqrt();
        assert!(close(&n[0], &[0., 4. / l, 1. / l]), "{:?}", n[0]);
        assert!(close(&n[1], &[0., 0., 1.]));
        assert!(close(&n[3], &[0., 1., 0.]));
    }

    #[test]
    fn strips_and_fans_become_lists_points_are_skipped() {
        let mut bin = Bin::default();
        #[rustfmt::skip]
        let pos = bin.f32s(&[0., 0., 0.,  1., 0., 0.,  0., 1., 0.,  1., 1., 0.]);
        let prim = |mode: u32| json!({ "attributes": { "POSITION": 0 }, "mode": mode });
        let doc = json!({
            "asset": { "version": "2.0" },
            "meshes": [{ "primitives": [prim(5), prim(6), prim(0), prim(1)] }],
            "accessors": [accessor(pos, FLOAT, 4, "VEC3")],
        });
        let m = import_glb(&glb(doc, &bin)).unwrap();
        let p = &m.meshes[0].primitives;
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].indices, vec![0, 1, 2, 1, 3, 2]);
        assert_eq!(p[1].indices, vec![1, 2, 0, 2, 3, 0]);
    }

    #[test]
    fn sparse_accessors() {
        let mut bin = Bin::default();
        let pos = bin.f32s(&[0., 0., 0., 1., 0., 0., 0., 1., 0.]);
        let sidx = bin.raw(&[2u8], None);
        let svals = bin.f32s(&[5., 5., 5.]);
        let sparse = json!({
            "count": 1,
            "indices": { "bufferView": sidx, "componentType": UBYTE },
            "values": { "bufferView": svals }
        });
        let mut over = accessor(pos, FLOAT, 3, "VEC3");
        over["sparse"] = sparse.clone();
        // No buffer view: zeros plus the sparse values.
        let zeros = json!({ "componentType": FLOAT, "count": 3, "type": "VEC3", "sparse": sparse });
        let doc = json!({
            "asset": { "version": "2.0" },
            "meshes": [{ "primitives": [
                { "attributes": { "POSITION": 0 } },
                { "attributes": { "POSITION": 1 } }
            ] }],
            "accessors": [over, zeros],
        });
        let m = import_glb(&glb(doc, &bin)).unwrap();
        let p = &m.meshes[0].primitives;
        assert_eq!(
            p[0].positions,
            vec![[0., 0., 0.], [1., 0., 0.], [5., 5., 5.]]
        );
        assert_eq!(
            p[1].positions,
            vec![[0., 0., 0.], [0., 0., 0.], [5., 5., 5.]]
        );
    }

    #[test]
    fn node_hierarchy_with_matrices_and_trs() {
        let h = std::f32::consts::FRAC_1_SQRT_2;
        // 90° about Z, scale 2, translation (1, 2, 3), column-major.
        #[rustfmt::skip]
        let rot_scale = [
            0., 2., 0., 0.,
            -2., 0., 0., 0.,
            0., 0., 2., 0.,
            1., 2., 3., 1.,
        ];
        // A mirror: scale (-1, 1, 1).
        #[rustfmt::skip]
        let mirror = [
            -1., 0., 0., 0.,
            0., 1., 0., 0.,
            0., 0., 1., 0.,
            0., 0., 0., 1.,
        ];
        let doc = json!({
            "asset": { "version": "2.0" },
            "scene": 1,
            "scenes": [{ "nodes": [3] }, { "nodes": [0, 99, 0] }],
            "nodes": [
                { "name": "root", "translation": [1, 0, 0], "rotation": [0, 0, 0, 2],
                  "scale": [3, 3, 3], "children": [1, 2, 0, 42] },
                { "matrix": rot_scale, "children": [0] },
                { "matrix": mirror, "children": [1] },
                { "children": [4] },
                { "children": [3] },
            ],
        });
        let m = import_glb(&glb(doc, &Bin::default())).unwrap();
        // Scene 1, out-of-range and duplicate roots dropped.
        assert_eq!(m.roots, vec![0]);
        let n = &m.nodes;
        assert_eq!(n.len(), 5);
        assert_eq!(n[0].translation, [1., 0., 0.]);
        assert_eq!(n[0].rotation, [0., 0., 0., 1.]);
        assert_eq!(n[0].scale, [3., 3., 3.]);
        // Self and out-of-range children dropped.
        assert_eq!(n[0].children, vec![1, 2]);
        assert_eq!(n[1].translation, [1., 2., 3.]);
        assert!(
            close(&n[1].rotation, &[0., 0., h, h]),
            "{:?}",
            n[1].rotation
        );
        assert!(close(&n[1].scale, &[2., 2., 2.]));
        // Back edges to the root, and a second parent, are cut.
        assert!(n[1].children.is_empty());
        assert!(n[2].children.is_empty());
        assert_eq!(n[2].scale, [-1., 1., 1.]);
        assert_eq!(n[2].rotation, [0., 0., 0., 1.]);
        // The 3 <-> 4 cycle is cut once.
        assert_eq!(n[3].children.len() + n[4].children.len(), 1);
    }

    #[test]
    fn decompose_round_trips() {
        let (t, r, s) = imp::decompose([
            1., 0., 0., 0., 0., 0., 1., 0., 0., -1., 0., 0., 4., 5., 6., 1.,
        ]);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert_eq!(t, [4., 5., 6.]);
        assert!(close(&r, &[h, 0., 0., h]), "{r:?}");
        assert!(close(&s, &[1., 1., 1.]));
        // Zero scale: identity rotation.
        let (_, r, s) = imp::decompose([0.; 16]);
        assert_eq!(r, [0., 0., 0., 1.]);
        assert_eq!(s, [0., 0., 0.]);
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake png";
    const JPEG: &[u8] = b"\xFF\xD8\xFF\xE0fake jpeg";

    fn base64(b: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in b.chunks(3) {
            let n = (u32::from(c[0]) << 16)
                | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                | u32::from(*c.get(2).unwrap_or(&0));
            for i in 0..4 {
                if i <= c.len() {
                    s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    s.push('=');
                }
            }
        }
        s
    }

    #[test]
    fn base64_decodes() {
        for len in 0..10 {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 5) as u8).collect();
            assert_eq!(imp::decode_base64(&base64(&data)).unwrap(), data);
        }
        assert_eq!(imp::decode_base64("aGk").unwrap(), b"hi");
        assert!(imp::decode_base64("a$b").is_none());
    }

    #[test]
    fn materials_and_images() {
        let (mut doc, mut bin) = triangle();
        let png = bin.raw(PNG, None);
        let png_again = bin.raw(PNG, None);
        let webp = bin.raw(b"RIFF\0\0\0\0WEBPVP8 ", None);
        doc["images"] = json!([
            { "bufferView": png, "mimeType": "image/png" },
            { "uri": format!("data:image/jpeg;base64,{}", base64(JPEG)) },
            { "bufferView": png_again, "mimeType": "image/png" },
            { "bufferView": webp, "mimeType": "image/png" },
            { "uri": "data:text/plain;base64,aGk=" },
        ]);
        doc["textures"] = json!([
            { "source": 0 }, { "source": 0 }, { "source": 1 }, { "source": 2 },
            { "source": 3 }, { "source": 4 }, { "source": 77 }, {}
        ]);
        doc["materials"] = json!([
            {
                "name": "full",
                "pbrMetallicRoughness": {
                    "baseColorFactor": [0.5, 0.25, 1, 0.5],
                    "baseColorTexture": { "index": 0 },
                    "metallicFactor": 0.3, "roughnessFactor": 0.7,
                    "metallicRoughnessTexture": { "index": 1 }
                },
                "normalTexture": { "index": 2, "scale": 0.5 },
                "emissiveTexture": { "index": 3 },
                "emissiveFactor": [1, 0.5, 0],
                "occlusionTexture": { "index": 4, "strength": 0.5 },
                "alphaMode": "MASK", "alphaCutoff": 0.3,
                "doubleSided": true,
                "extensions": {
                    "KHR_materials_emissive_strength": { "emissiveStrength": 4 },
                    "KHR_materials_unlit": {}
                }
            },
            {},
            {
                "alphaMode": "BLEND",
                "pbrMetallicRoughness": { "baseColorTexture": { "index": 5 } },
                "normalTexture": { "index": 6 },
                "emissiveTexture": { "index": 7 },
                "occlusionTexture": { "index": 0, "texCoord": 1 }
            }
        ]);
        let prim = doc["meshes"][0]["primitives"][0].clone();
        let with = |m: Value| {
            let mut p = prim.clone();
            p["material"] = m;
            p
        };
        doc["meshes"][0]["primitives"] =
            json!([with(json!(0)), prim.clone(), with(json!(1)), with(json!(9))]);
        doc["extensionsUsed"] = json!(["KHR_materials_unlit", "KHR_materials_emissive_strength"]);
        doc["extensionsRequired"] = json!(["KHR_materials_unlit"]);
        let m = import_glb(&glb(doc, &bin)).unwrap();

        // PNG (shared by two textures and a byte-identical image), JPEG from a
        // data: URI, WebP sniffed despite the declared type.
        assert_eq!(m.images.len(), 3);
        assert_eq!(
            m.images[0],
            ImportedImage {
                mime: "image/png".into(),
                bytes: PNG.to_vec()
            }
        );
        assert_eq!(
            m.images[1],
            ImportedImage {
                mime: "image/jpeg".into(),
                bytes: JPEG.to_vec()
            }
        );
        assert_eq!(m.images[2].mime, "image/webp");

        let full = &m.materials[0];
        assert_eq!(
            *full,
            ImportedMaterial {
                name: Some("full".into()),
                base_color: [0.5, 0.25, 1., 0.5],
                base_texture: Some(0),
                metallic: 0.3,
                roughness: 0.7,
                mr_texture: Some(0),
                normal_texture: Some(1),
                normal_scale: 0.5,
                emissive: [4., 2., 0.],
                emissive_texture: Some(0),
                occlusion_texture: Some(2),
                alpha: ImportedAlpha::Mask(0.3),
                double_sided: true,
                unlit: true,
            }
        );
        let default = &m.materials[1];
        assert_eq!(default.base_color, [1.; 4]);
        assert_eq!(
            (default.metallic, default.roughness, default.normal_scale),
            (1., 1., 1.)
        );
        assert_eq!(default.emissive, [0.; 3]);
        assert_eq!(default.alpha, ImportedAlpha::Opaque);
        assert!(!default.double_sided && !default.unlit && default.base_texture.is_none());
        // Non-image data, missing sources and other UV sets give no texture.
        let bad = &m.materials[2];
        assert_eq!(bad.alpha, ImportedAlpha::Blend);
        assert_eq!(
            (
                bad.base_texture,
                bad.normal_texture,
                bad.emissive_texture,
                bad.occlusion_texture
            ),
            (None, None, None, None)
        );

        let p = &m.meshes[0].primitives;
        let mats: Vec<_> = p.iter().map(|p| p.material).collect();
        assert_eq!(mats, vec![Some(0), None, Some(1), None]);
    }

    #[test]
    fn webp_extension_source_is_preferred() {
        let (mut doc, mut bin) = triangle();
        let png = bin.raw(PNG, None);
        let webp = bin.raw(b"RIFF\0\0\0\0WEBPVP8 ", None);
        doc["images"] = json!([
            { "bufferView": png, "mimeType": "image/png" },
            { "bufferView": webp, "mimeType": "image/webp" },
            { "uri": "missing.webp" },
        ]);
        let webp_texture =
            |source: usize| json!({ "extensions": { "EXT_texture_webp": { "source": source } } });
        let mut with_fallback = webp_texture(2);
        with_fallback["source"] = json!(0);
        doc["textures"] = json!([webp_texture(1), with_fallback]);
        doc["materials"] = json!([{
            "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } },
            "emissiveTexture": { "index": 1 }
        }]);
        doc["extensionsRequired"] = json!(["EXT_texture_webp"]);
        let m = import_glb(&glb(doc, &bin)).unwrap();
        assert_eq!(
            m.images[m.materials[0].base_texture.unwrap()].mime,
            "image/webp"
        );
        // The WebP file is missing: the core source is used.
        assert_eq!(
            m.images[m.materials[0].emissive_texture.unwrap()].mime,
            "image/png"
        );
    }

    #[test]
    fn skin_and_animations() {
        let mut bin = Bin::default();
        let pos = bin.f32s(&[0., 0., 0., 1., 0., 0., 0., 1., 0.]);
        let joints = bin.raw(&[0, 1, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0], None);
        let weights = bin.raw(&[255, 255, 0, 0, 0, 0, 0, 0, 10, 30, 0, 0], None);
        let colors = bin.raw(&[255, 0, 0, 0, 255, 0, 0, 0, 255], None);
        let mut ibm_data = vec![0f32; 32];
        for i in 0..4 {
            ibm_data[i * 5] = 1.;
            ibm_data[16 + i * 5] = 1.;
        }
        ibm_data[16 + 13] = -1.;
        let ibm = bin.f32s(&ibm_data);
        let t2 = bin.f32s(&[0., 1.]);
        let tvals = bin.f32s(&[0., 0., 0., 1., 2., 3.]);
        let rvals = bin.f32s(&[0., 0., 0., 2., 0., 0., 3., 0.]);
        // Cubic: in, value, out per key.
        #[rustfmt::skip]
        let svals = bin.f32s(&[
            0., 0., 0.,  1., 1., 1.,  0.5, 0.5, 0.5,
            0., 0., 0.,  2., 2., 2.,  0., 0., 0.,
        ]);
        let short = bin.f32s(&[1., 2., 3.]);
        let backwards = bin.f32s(&[1., 0.]);

        let mut wacc = accessor(weights, UBYTE, 3, "VEC4");
        wacc["normalized"] = json!(true);
        let mut cacc = accessor(colors, UBYTE, 3, "VEC3");
        cacc["normalized"] = json!(true);
        let sampler = |input: usize, output: usize, interp: &str| json!({ "input": input, "output": output, "interpolation": interp });
        let channel = |sampler: usize, node: usize, path: &str| json!({ "sampler": sampler, "target": { "node": node, "path": path } });
        let doc = json!({
            "asset": { "version": "2.0" },
            "scenes": [{ "nodes": [0, 1] }],
            "nodes": [
                { "mesh": 0, "skin": 0 },
                { "name": "hip", "children": [2] },
                { "name": "knee", "translation": [0, 1, 0] },
                { "mesh": 0, "skin": 2 },
            ],
            "skins": [
                { "joints": [1, 2], "inverseBindMatrices": 3 },
                { "joints": [1] },
                { "joints": [1, 50] },
            ],
            "meshes": [{ "primitives": [{
                "attributes": { "POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2, "COLOR_0": 4 }
            }] }],
            "accessors": [
                accessor(pos, FLOAT, 3, "VEC3"),
                accessor(joints, UBYTE, 3, "VEC4"),
                wacc,
                accessor(ibm, FLOAT, 2, "MAT4"),
                cacc,
                accessor(t2, FLOAT, 2, "SCALAR"),
                accessor(tvals, FLOAT, 2, "VEC3"),
                accessor(rvals, FLOAT, 2, "VEC4"),
                accessor(svals, FLOAT, 6, "VEC3"),
                accessor(short, FLOAT, 1, "VEC3"),
                accessor(backwards, FLOAT, 2, "SCALAR"),
            ],
            "animations": [
                {
                    "name": "Walk",
                    "samplers": [
                        sampler(5, 6, "LINEAR"),
                        sampler(5, 7, "STEP"),
                        sampler(5, 8, "CUBICSPLINE"),
                        sampler(5, 9, "LINEAR"),
                        sampler(10, 6, "LINEAR"),
                    ],
                    "channels": [
                        channel(0, 1, "translation"),
                        channel(1, 2, "rotation"),
                        channel(2, 2, "scale"),
                        channel(0, 1, "weights"),
                        channel(0, 99, "translation"),
                        channel(9, 1, "translation"),
                        channel(3, 1, "translation"),
                        channel(4, 1, "translation"),
                    ]
                },
                { "name": "Morph", "samplers": [sampler(5, 6, "LINEAR")],
                  "channels": [channel(0, 0, "weights")] }
            ],
        });
        let m = import_glb(&glb(doc, &bin)).unwrap();

        assert_eq!(m.roots, vec![0, 1]);
        assert_eq!(m.nodes[0].skin, Some(0));
        // A skin with an out-of-range joint is dropped.
        assert_eq!(m.nodes[3].skin, None);
        assert_eq!(m.skins.len(), 3);
        assert!(m.skins[2].joints.is_empty());
        assert_eq!(m.skins[0].joints, vec![1, 2]);
        assert_eq!(m.skins[0].inverse_bind[0], imp_identity());
        assert_eq!(m.skins[0].inverse_bind[1][13], -1.);
        assert_eq!(m.skins[1].inverse_bind, vec![imp_identity()]);

        let p = &m.meshes[0].primitives[0];
        assert_eq!(p.joints, vec![[0, 1, 0, 0], [1, 0, 0, 0], [0, 1, 0, 0]]);
        assert!(close(&p.weights[0], &[0.5, 0.5, 0., 0.]));
        // All-zero weights go to the first joint.
        assert_eq!(p.weights[1], [1., 0., 0., 0.]);
        assert!(close(&p.weights[2], &[0.25, 0.75, 0., 0.]));
        assert_eq!(
            p.colors,
            vec![[1., 0., 0., 1.], [0., 1., 0., 1.], [0., 0., 1., 1.]]
        );

        // The weights-only animation is gone.
        assert_eq!(m.animations.len(), 1);
        let a = &m.animations[0];
        assert_eq!(a.name.as_deref(), Some("Walk"));
        assert_eq!(a.channels.len(), 3);
        let [t, r, s] = [&a.channels[0], &a.channels[1], &a.channels[2]];
        assert_eq!(
            (t.node, t.path, t.interpolation),
            (1, ImportedPath::Translation, ImportedInterpolation::Linear)
        );
        assert_eq!(t.times, vec![0., 1.]);
        assert_eq!(t.values, vec![0., 0., 0., 1., 2., 3.]);
        assert_eq!(
            (r.node, r.path, r.interpolation),
            (2, ImportedPath::Rotation, ImportedInterpolation::Step)
        );
        assert_eq!(r.values, vec![0., 0., 0., 1., 0., 0., 1., 0.]);
        assert_eq!(
            (s.path, s.interpolation),
            (ImportedPath::Scale, ImportedInterpolation::CubicSpline)
        );
        assert_eq!(s.values.len(), 18);
        assert_eq!(&s.values[3..9], &[1., 1., 1., 0.5, 0.5, 0.5]);
    }

    fn imp_identity() -> [f32; 16] {
        std::array::from_fn(|i| if i % 5 == 0 { 1. } else { 0. })
    }

    #[test]
    fn cubic_rotation_normalizes_values_not_tangents() {
        let mut bin = Bin::default();
        let t = bin.f32s(&[0.]);
        let v = bin.f32s(&[3., 0., 0., 0., 0., 0., 0., 2., 0., 4., 0., 0.]);
        let doc = json!({
            "asset": { "version": "2.0" },
            "nodes": [{}],
            "accessors": [accessor(t, FLOAT, 1, "SCALAR"), accessor(v, FLOAT, 3, "VEC4")],
            "animations": [{
                "samplers": [{ "input": 0, "output": 1, "interpolation": "CUBICSPLINE" }],
                "channels": [{ "sampler": 0, "target": { "node": 0, "path": "rotation" } }]
            }],
        });
        let m = import_glb(&glb(doc, &bin)).unwrap();
        assert_eq!(
            m.animations[0].channels[0].values,
            vec![3., 0., 0., 0., 0., 0., 0., 1., 0., 4., 0., 0.]
        );
    }

    fn gltf_doc(uri: &str) -> Vec<u8> {
        let (mut doc, bin) = triangle();
        doc["buffers"] = json!([{ "byteLength": bin.bytes.len(), "uri": uri }]);
        doc["bufferViews"] = json!(bin.views);
        serde_json::to_vec(&doc).unwrap()
    }

    #[test]
    fn gltf_with_data_uri_buffer() {
        let bin = triangle().1.bytes;
        let uri = format!("data:application/octet-stream;base64,{}", base64(&bin));
        let m = import_gltf(&gltf_doc(&uri), &|_| panic!("nothing to load")).unwrap();
        assert_eq!(m.meshes[0].primitives[0].indices, vec![0, 1, 2]);
    }

    #[test]
    fn gltf_with_external_buffer() {
        let bin = triangle().1.bytes;
        let asked = std::cell::RefCell::new(Vec::new());
        let load = |uri: &str| {
            asked.borrow_mut().push(uri.to_owned());
            (uri == "my mesh.bin").then(|| bin.clone())
        };
        let m = import_gltf(&gltf_doc("my%20mesh.bin"), &load).unwrap();
        assert_eq!(m.meshes[0].primitives[0].positions.len(), 3);
        assert_eq!(*asked.borrow(), vec!["my mesh.bin".to_owned()]);
        // A missing file: the file still imports, without the mesh's data.
        let m = import_gltf(&gltf_doc("gone.bin"), &load).unwrap();
        assert!(m.meshes[0].primitives.is_empty());
        assert_eq!(m.nodes.len(), 1);
    }

    #[test]
    fn bad_parts_are_skipped() {
        let (mut doc, mut bin) = triangle();
        let tiny = bin.raw(&[0; 4], None);
        doc["nodes"] = json!([{ "mesh": 5, "skin": 3 }, { "mesh": 0 }]);
        doc["accessors"].as_array_mut().unwrap().extend([
            accessor(tiny, FLOAT, 1000, "VEC3"),
            accessor(77, FLOAT, 3, "VEC3"),
            json!({ "componentType": FLOAT, "count": 4_000_000_000u64, "type": "MAT4" }),
            json!({ "bufferView": 0, "componentType": 1234, "count": 3, "type": "VEC3" }),
        ]);
        let prim = |pos: usize| json!({ "attributes": { "POSITION": pos } });
        let mut idx_oob = prim(0);
        idx_oob["indices"] = json!(3);
        doc["meshes"][0]["primitives"]
            .as_array_mut()
            .unwrap()
            .extend([prim(3), prim(4), prim(5), prim(6), prim(500), idx_oob]);
        let m = import_glb(&glb(doc, &bin)).unwrap();
        assert_eq!(m.nodes[0].mesh, None);
        assert_eq!(m.nodes[0].skin, None);
        assert_eq!(m.meshes[0].primitives.len(), 1);
    }

    #[test]
    fn unreadable_files_are_errors() {
        let parse = |r: Result<Imported, ModelError>| matches!(r, Err(ModelError::Parse(_)));
        assert!(parse(import_glb(b"")));
        assert!(parse(import_glb(b"glTF")));
        assert!(parse(import_glb(b"not a model at all")));
        assert!(parse(import_gltf(b"{", &|_| None)));
        assert!(parse(import_gltf(b"{}", &|_| None)));
        assert!(parse(import_gltf(b"\xFF\xFE garbage", &|_| None)));
        // GLB header length below the header itself.
        let mut glb_bytes = glb(triangle().0, &triangle().1);
        glb_bytes[8..12].copy_from_slice(&4u32.to_le_bytes());
        assert!(parse(import_glb(&glb_bytes)));

        let unsupported =
            |r: Result<Imported, ModelError>| matches!(r, Err(ModelError::Unsupported(_)));
        let v1 = br#"{"asset":{"version":"1.0"},"scenes":{"s":{"nodes":[]}}}"#;
        assert!(unsupported(import_gltf(v1, &|_| None)));
        assert!(unsupported(import_gltf(
            br#"{"asset":{"version":"3.0"}}"#,
            &|_| None
        )));
        let draco =
            br#"{"asset":{"version":"2.0"},"extensionsRequired":["KHR_draco_mesh_compression"]}"#;
        assert!(unsupported(import_gltf(draco, &|_| None)));
        let mut v1_glb = glb(triangle().0, &Bin::default());
        v1_glb[4] = 1;
        assert!(unsupported(import_glb(&v1_glb)));
        // An empty but valid file.
        let m = import_gltf(br#"{"asset":{"version":"2.0"}}"#, &|_| None).unwrap();
        assert_eq!(m, Imported::default());
    }

    #[test]
    fn truncated_and_corrupted_glbs_never_panic() {
        let (doc, bin) = triangle();
        let full = glb(doc, &bin);
        for len in 0..full.len() {
            assert!(import_glb(&full[..len]).is_err(), "prefix of {len} bytes");
        }
        assert!(import_glb(&full).is_ok());
        // Every byte overwritten with a few values: errors or partial
        // imports, never a panic.
        for i in 0..full.len() {
            for v in [0x00, 0xFF, b'9', b'"'] {
                let mut bytes = full.clone();
                bytes[i] = v;
                let _ = import_glb(&bytes);
            }
        }
    }

    #[test]
    fn glb_through_import_gltf_and_padding() {
        let (doc, bin) = triangle();
        let json = serde_json::to_vec(&doc_with_bin(doc, &bin)).unwrap();
        // NUL padding (some exporters) and a BOM are tolerated.
        let mut padded = b"\xEF\xBB\xBF".to_vec();
        padded.extend(&json);
        padded.extend([0, 0, 0]);
        let bytes = assemble(&padded, &bin.bytes);
        assert!(import_gltf(&bytes, &|_| None).is_ok());
    }

    fn doc_with_bin(mut doc: Value, bin: &Bin) -> Value {
        doc["buffers"] = json!([{ "byteLength": bin.bytes.len() }]);
        doc["bufferViews"] = json!(bin.views);
        doc
    }

    #[test]
    fn mime_sniffing_and_uris() {
        assert_eq!(imp::sniff(PNG), Some("image/png"));
        assert_eq!(imp::sniff(JPEG), Some("image/jpeg"));
        assert_eq!(imp::sniff(b"nope"), None);
        assert_eq!(imp::percent_decode("a%20b%zz%2"), "a b%zz%2");
    }
}
