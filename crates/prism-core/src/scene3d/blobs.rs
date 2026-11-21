//! Heavy scene data as content-addressed binary blobs: meshes, instance
//! sets, animation clips, skins, environments and embedded images travel
//! once, by id, instead of inside every scene update.
//!
//! The store holds [`Weak`] references: a blob stays fetchable while a
//! [`BlobRef`] to it lives somewhere (the scene values that use it — a
//! [`Mesh`](super::Mesh), [`Instances`](super::Instances),
//! [`AnimationClip`](super::AnimationClip), [`Environment`](super::Environment),
//! a [`Model`](super::Model) — and the last scene the reconciler sent for
//! each mounted scene element).
//!
//! How backends get a blob by id: the web `GET <base>/_prism/3d/<id>`
//! (served by the app server); Linux [`get`]; Apple `prism_scene_blob`
//! (prism-native); Android a JNI call (prism-android).
//!
//! ## Binary layouts
//!
//! Little-endian, every section 4-byte aligned. Each blob except images
//! starts with `"P3D"`, its kind (one ASCII byte, [`BlobKind::code`]) and
//! `u32 version = 1`.
//!
//! - **`M` mesh**: `u32 n` (vertices), `u32 m` (indices), `u32 flags`
//!   ([`MESH_UV`], [`MESH_SKIN`], [`MESH_COLOR`]), `f32 bounds[6]`
//!   (min xyz, max xyz); `f32 pos[3n]`, `f32 nrm[3n]`, `[f32 uv[2n]]`,
//!   `[u16 joints[4n]; f32 weights[4n]]`, `[f32 color[4n]]` (linear RGBA),
//!   `u32 idx[m]` (three per triangle, counter-clockwise from the front).
//! - **`I` instances**: `u32 count`, `u32 flags` (1: colors), `f32
//!   bounds[6]` (all instances of the node's mesh, in the node's space);
//!   per instance `f32 [px, py, pz, qx, qy, qz, qw, sx, sy, sz]` (relative
//!   to the node: `T · R(q) · S`); then `[f32 color[4 · count]]` (linear
//!   RGBA, multiplies the material).
//! - **`A` clip**: `u32 channels`, `f32 duration` (seconds); per channel:
//!   `u32 len`, utf-8 target (`len` bytes, zero-padded to 4), `u8 path`
//!   (0 translation, 1 rotation quaternion, 2 scale), `u8 interp`
//!   (0 linear, 1 step, 2 cubic spline), `u16 0`, `u32 k`, `f32 times[k]`,
//!   `f32 values[k · c · (cubic ? 3 : 1)]` (`c`: 3, or 4 for rotations;
//!   cubic keys are in-tangent, value, out-tangent).
//! - **`K` skin**: `u32 joints`, `f32 inverse_bind[16 · joints]`
//!   (column-major); then per joint `u32 len`, utf-8 path (zero-padded to
//!   4): child indices from the skin's root node (`"0/2"`; `""`: the root).
//! - **`E` environment**: see [`env`](super::env) (`EnvMap::encode`).
//! - **images** (kind `P`): the PNG, JPEG or WebP bytes as they are, mime
//!   `image/*`, no header.
//!
//! Decoders return `None` on anything malformed (never panic).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use super::clip::{ClipChannel, ClipData, ClipProperty, Interp};
use super::math::{Aabb, Mat4};

/// What a blob holds (its id hashes this too).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlobKind {
    /// Vertices and triangles (`M`).
    Mesh,
    /// Per-instance transforms and colors (`I`).
    Instances,
    /// An animation clip (`A`).
    Clip,
    /// A skin's joints (`K`).
    Skin,
    /// A prefiltered environment (`E`).
    Environment,
    /// An encoded image, PNG, JPEG or WebP (`P`).
    Image,
}

impl BlobKind {
    /// The kind's byte in headers and ids.
    pub fn code(self) -> u8 {
        match self {
            BlobKind::Mesh => b'M',
            BlobKind::Instances => b'I',
            BlobKind::Clip => b'A',
            BlobKind::Skin => b'K',
            BlobKind::Environment => b'E',
            BlobKind::Image => b'P',
        }
    }
}

/// The MIME type of every blob but images.
pub const BINARY_MIME: &str = "application/octet-stream";

/// Mesh flag: `uv` present.
pub const MESH_UV: u32 = 1;
/// Mesh flag: `joints` and `weights` present.
pub const MESH_SKIN: u32 = 4;
/// Mesh flag: `color` present.
pub const MESH_COLOR: u32 = 8;

struct Blob {
    mime: String,
    bytes: OnceLock<Arc<[u8]>>,
    /// Makes the bytes the first time they are read ([`put_lazy`]).
    make: Option<fn() -> Vec<u8>>,
}

impl Blob {
    fn new(mime: &str, bytes: Vec<u8>) -> Blob {
        Blob {
            mime: mime.to_string(),
            bytes: OnceLock::from(Arc::<[u8]>::from(bytes)),
            make: None,
        }
    }

    fn bytes(&self) -> Arc<[u8]> {
        self.bytes
            .get_or_init(|| self.make.map_or_else(Vec::new, |make| make()).into())
            .clone()
    }
}

/// A blob: its id, and (in the process that made it) its bytes, kept alive
/// while this reference lives. Cheap to clone; equal when the ids are.
/// Serializes as its id; deserializing finds the bytes again if the store
/// still has them.
#[derive(Clone)]
pub struct BlobRef {
    id: Arc<str>,
    blob: Option<Arc<Blob>>,
}

impl BlobRef {
    /// The id: 32 lowercase hex characters.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The bytes, if this process still has them.
    pub fn bytes(&self) -> Option<Arc<[u8]>> {
        match &self.blob {
            Some(b) => Some(b.bytes()),
            None => get(&self.id).map(|(bytes, _)| bytes),
        }
    }

    /// A reference to blob `id` (bytes attached if the store has them);
    /// `None` if `id` is not 32 hex characters.
    pub fn from_id(id: &str) -> Option<BlobRef> {
        let key = parse_id(id)?;
        let blob = store().lock().ok().and_then(|s| s.get(&key)?.upgrade());
        Some(BlobRef {
            id: id.to_ascii_lowercase().into(),
            blob,
        })
    }
}

impl PartialEq for BlobRef {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for BlobRef {}

impl std::hash::Hash for BlobRef {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state)
    }
}

impl std::fmt::Debug for BlobRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BlobRef({})", self.id)
    }
}

impl serde::Serialize for BlobRef {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.id)
    }
}

impl<'de> serde::Deserialize<'de> for BlobRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let id = String::deserialize(d)?;
        BlobRef::from_id(&id).ok_or_else(|| serde::de::Error::custom("not a blob id"))
    }
}

type Store = Mutex<HashMap<u128, Weak<Blob>>>;

fn store() -> &'static Store {
    static STORE: OnceLock<Store> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// 128-bit FNV-1a of the kind's byte, then the bytes.
fn fnv128(kind: BlobKind, bytes: &[u8]) -> u128 {
    const PRIME: u128 = 0x0000000001000000000000000000013B;
    let mut h: u128 = 0x6c62272e07bb014262b821756295c58d;
    for b in std::iter::once(kind.code()).chain(bytes.iter().copied()) {
        h ^= b as u128;
        h = h.wrapping_mul(PRIME);
    }
    h
}

fn parse_id(id: &str) -> Option<u128> {
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u128::from_str_radix(id, 16).ok()
}

/// Stores `bytes` and returns a reference keeping them alive. The same
/// kind and bytes give the same id (and the same storage).
pub fn put(kind: BlobKind, mime: &str, bytes: Vec<u8>) -> BlobRef {
    let key = fnv128(kind, &bytes);
    insert(key, || Blob::new(mime, bytes))
}

/// Stores bytes `make` gives the first time they are read (a reader
/// waits for them), under an id made from `kind` and `name`: what costs
/// time to make (a built-in environment) is made when a renderer needs
/// it, not while the app's tree is built. `make` must always give the
/// same bytes.
pub fn put_lazy(kind: BlobKind, mime: &str, name: &str, make: fn() -> Vec<u8>) -> BlobRef {
    let key = fnv128(kind, format!("lazy:{name}").as_bytes());
    insert(key, || Blob {
        mime: mime.to_string(),
        bytes: OnceLock::new(),
        make: Some(make),
    })
}

/// The stored blob under `key`, or `blob()` stored there.
fn insert(key: u128, blob: impl FnOnce() -> Blob) -> BlobRef {
    let id: Arc<str> = format!("{key:032x}").into();
    let Ok(mut store) = store().lock() else {
        // A poisoned store: still a usable reference, just not shared.
        return BlobRef {
            id,
            blob: Some(Arc::new(blob())),
        };
    };
    if let Some(blob) = store.get(&key).and_then(Weak::upgrade) {
        return BlobRef {
            id,
            blob: Some(blob),
        };
    }
    // Forget dead entries now and then (amortized O(1) per put).
    if store.len() >= 64 && store.len().is_power_of_two() {
        store.retain(|_, w| w.strong_count() > 0);
    }
    let blob = Arc::new(blob());
    store.insert(key, Arc::downgrade(&blob));
    BlobRef {
        id,
        blob: Some(blob),
    }
}

/// The bytes and MIME type of blob `id`, while something holds it.
pub fn get(id: &str) -> Option<(Arc<[u8]>, String)> {
    let key = parse_id(id)?;
    let blob = store().lock().ok()?.get(&key)?.upgrade()?;
    // Made (if lazy) outside the store's lock.
    Some((blob.bytes(), blob.mime.clone()))
}

/// The size of blob `id` while something holds it, without making a lazy
/// blob's bytes (0 until something reads them).
pub fn size(id: &str) -> Option<usize> {
    let key = parse_id(id)?;
    let blob = store().lock().ok()?.get(&key)?.upgrade()?;
    Some(blob.bytes.get().map_or(0, |b| b.len()))
}

// --- Writing and reading -----------------------------------------------------------------

/// Little-endian writer with 4-byte aligned sections.
struct Writer(Vec<u8>);

impl Writer {
    fn new(kind: BlobKind) -> Writer {
        let mut w = Writer(Vec::new());
        w.0.extend_from_slice(b"P3D");
        w.0.push(kind.code());
        w.u32(1);
        w
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32s<'a>(&mut self, v: impl IntoIterator<Item = &'a f32>) {
        for x in v {
            self.f32(*x);
        }
    }
    fn u16s<'a>(&mut self, v: impl IntoIterator<Item = &'a u16>) {
        for x in v {
            self.0.extend_from_slice(&x.to_le_bytes());
        }
        self.pad();
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
        self.pad();
    }
    fn pad(&mut self) {
        while !self.0.len().is_multiple_of(4) {
            self.0.push(0);
        }
    }
}

/// Little-endian reader: every method returns `None` past the end.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    /// After checking the header of a `kind` blob, version 1.
    fn new(bytes: &'a [u8], kind: BlobKind) -> Option<Reader<'a>> {
        let mut r = Reader { bytes, at: 0 };
        let magic = r.take(4)?;
        if magic[..3] != *b"P3D" || magic[3] != kind.code() || r.u32()? != 1 {
            return None;
        }
        Some(r)
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    /// `n` floats (checked against the bytes left before allocating).
    fn f32s(&mut self, n: usize) -> Option<Vec<f32>> {
        let raw = self.take(n.checked_mul(4)?)?;
        Some(
            raw.as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect(),
        )
    }
    fn u16s(&mut self, n: usize) -> Option<Vec<u16>> {
        let raw = self.take(n.checked_mul(2)?)?;
        let v = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
            .collect();
        self.align();
        Some(v)
    }
    fn u32s(&mut self, n: usize) -> Option<Vec<u32>> {
        let raw = self.take(n.checked_mul(4)?)?;
        Some(
            raw.as_chunks::<4>()
                .0
                .iter()
                .map(|b| u32::from_le_bytes(*b))
                .collect(),
        )
    }
    fn str(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        let s = std::str::from_utf8(self.take(len)?).ok()?.to_string();
        self.align();
        Some(s)
    }
    fn align(&mut self) {
        self.at = self.at.next_multiple_of(4).min(self.bytes.len());
    }
    fn bounds(&mut self) -> Option<Aabb> {
        self.f32s(6)?.try_into().ok()
    }
}

fn group<const N: usize>(v: &[f32]) -> Vec<[f32; N]> {
    v.as_chunks::<N>().0.to_vec()
}

// --- M: meshes -----------------------------------------------------------------------------

/// A mesh's vertices and triangles (the `M` blob's contents). Optional
/// attributes are empty when absent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshBlob {
    /// Positions.
    pub positions: Vec<[f32; 3]>,
    /// Unit normals (one per position).
    pub normals: Vec<[f32; 3]>,
    /// Texture coordinates, `(0, 0)` top-left.
    pub uvs: Vec<[f32; 2]>,
    /// Four joint indices per vertex (into the skin's joints).
    pub joints: Vec<[u16; 4]>,
    /// Four joint weights per vertex.
    pub weights: Vec<[f32; 4]>,
    /// Linear RGBA per vertex (multiplies the material).
    pub colors: Vec<[f32; 4]>,
    /// Three per triangle, counter-clockwise from the front.
    pub indices: Vec<u32>,
    /// The positions' box (computed by [`encode_mesh`]).
    pub bounds: Aabb,
}

/// The `M` blob of `mesh`. Attributes whose length is not the vertex count
/// are left out; the bounds are computed from the positions.
pub fn encode_mesh(mesh: &MeshBlob) -> Vec<u8> {
    let n = mesh.positions.len();
    let uv = mesh.uvs.len() == n && n > 0;
    let skin = mesh.joints.len() == n && mesh.weights.len() == n && n > 0;
    let color = mesh.colors.len() == n && n > 0;
    let flags = (uv as u32 * MESH_UV) | (skin as u32 * MESH_SKIN) | (color as u32 * MESH_COLOR);
    let mut w = Writer::new(BlobKind::Mesh);
    w.u32(n as u32);
    w.u32(mesh.indices.len() as u32);
    w.u32(flags);
    w.f32s(&super::math::aabb_of(mesh.positions.iter().copied()));
    w.f32s(mesh.positions.iter().flatten());
    if mesh.normals.len() == n {
        w.f32s(mesh.normals.iter().flatten());
    } else {
        for _ in 0..n {
            w.f32s(&[0., 1., 0.]);
        }
    }
    if uv {
        w.f32s(mesh.uvs.iter().flatten());
    }
    if skin {
        w.u16s(mesh.joints.iter().flatten());
        w.f32s(mesh.weights.iter().flatten());
    }
    if color {
        w.f32s(mesh.colors.iter().flatten());
    }
    for i in &mesh.indices {
        w.u32(*i);
    }
    w.0
}

/// Reads an `M` blob (`None` if malformed or an index is out of range).
pub fn decode_mesh(bytes: &[u8]) -> Option<MeshBlob> {
    let mut r = Reader::new(bytes, BlobKind::Mesh)?;
    let n = r.u32()? as usize;
    let m = r.u32()? as usize;
    let flags = r.u32()?;
    let bounds = r.bounds()?;
    let positions = group(&r.f32s(n.checked_mul(3)?)?);
    let normals = group(&r.f32s(n * 3)?);
    let uvs = if flags & MESH_UV != 0 {
        group(&r.f32s(n * 2)?)
    } else {
        Vec::new()
    };
    let (joints, weights) = if flags & MESH_SKIN != 0 {
        let j = r.u16s(n * 4)?;
        (j.as_chunks::<4>().0.to_vec(), group(&r.f32s(n * 4)?))
    } else {
        (Vec::new(), Vec::new())
    };
    let colors = if flags & MESH_COLOR != 0 {
        group(&r.f32s(n * 4)?)
    } else {
        Vec::new()
    };
    let indices = r.u32s(m)?;
    if indices.iter().any(|i| *i as usize >= n) {
        return None;
    }
    Some(MeshBlob {
        positions,
        normals,
        uvs,
        joints,
        weights,
        colors,
        indices,
        bounds,
    })
}

// --- I: instances --------------------------------------------------------------------------

/// One instance's placement, relative to its node: `T · R(q) · S`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InstanceTransform {
    /// Translation.
    pub position: [f32; 3],
    /// Rotation, a unit quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    /// Scale along each axis.
    pub scale: [f32; 3],
}

impl InstanceTransform {
    /// Its matrix.
    pub fn matrix(&self) -> Mat4 {
        super::math::compose_quat(self.position, self.rotation, self.scale)
    }
}

/// An instance set (the `I` blob's contents).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InstancesBlob {
    /// Placements.
    pub transforms: Vec<InstanceTransform>,
    /// Linear RGBA per instance, or empty.
    pub colors: Vec<[f32; 4]>,
    /// Every instance of the node's mesh, in the node's space.
    pub bounds: Aabb,
}

/// The `I` blob of `set` (colors left out unless one per instance).
pub fn encode_instances(set: &InstancesBlob) -> Vec<u8> {
    let count = set.transforms.len();
    let colors = set.colors.len() == count && count > 0;
    let mut w = Writer::new(BlobKind::Instances);
    w.u32(count as u32);
    w.u32(colors as u32);
    w.f32s(&set.bounds);
    for t in &set.transforms {
        w.f32s(t.position.iter().chain(&t.rotation).chain(&t.scale));
    }
    if colors {
        w.f32s(set.colors.iter().flatten());
    }
    w.0
}

/// Reads an `I` blob.
pub fn decode_instances(bytes: &[u8]) -> Option<InstancesBlob> {
    let mut r = Reader::new(bytes, BlobKind::Instances)?;
    let count = r.u32()? as usize;
    let flags = r.u32()?;
    let bounds = r.bounds()?;
    let raw = r.f32s(count.checked_mul(10)?)?;
    let transforms = raw
        .as_chunks::<10>()
        .0
        .iter()
        .map(|v| InstanceTransform {
            position: [v[0], v[1], v[2]],
            rotation: [v[3], v[4], v[5], v[6]],
            scale: [v[7], v[8], v[9]],
        })
        .collect();
    let colors = if flags & 1 != 0 {
        group(&r.f32s(count * 4)?)
    } else {
        Vec::new()
    };
    Some(InstancesBlob {
        transforms,
        colors,
        bounds,
    })
}

// --- A: clips ------------------------------------------------------------------------------

/// The `A` blob of `clip`.
pub fn encode_clip(clip: &ClipData) -> Vec<u8> {
    let mut w = Writer::new(BlobKind::Clip);
    w.u32(clip.channels.len() as u32);
    w.f32(clip.duration);
    for c in &clip.channels {
        w.str(&c.target);
        w.0.push(match c.property {
            ClipProperty::Translation => 0,
            ClipProperty::Rotation => 1,
            ClipProperty::Scale => 2,
        });
        w.0.push(match c.interpolation {
            Interp::Linear => 0,
            Interp::Step => 1,
            Interp::CubicSpline => 2,
        });
        w.0.extend_from_slice(&[0, 0]);
        w.u32(c.times.len() as u32);
        w.f32s(&c.times);
        w.f32s(&c.values);
    }
    w.0
}

/// Reads an `A` blob (`None` if a channel's value count does not match
/// its keys).
pub fn decode_clip(bytes: &[u8]) -> Option<ClipData> {
    let mut r = Reader::new(bytes, BlobKind::Clip)?;
    let count = r.u32()? as usize;
    let duration = r.f32()?;
    // Each channel takes at least 12 bytes: no huge allocation on bad input.
    let mut channels = Vec::with_capacity(count.min(bytes.len() / 12));
    for _ in 0..count {
        let target = r.str()?;
        let property = match r.u8()? {
            0 => ClipProperty::Translation,
            1 => ClipProperty::Rotation,
            2 => ClipProperty::Scale,
            _ => return None,
        };
        let interpolation = match r.u8()? {
            0 => Interp::Linear,
            1 => Interp::Step,
            2 => Interp::CubicSpline,
            _ => return None,
        };
        r.take(2)?;
        let k = r.u32()? as usize;
        let times = r.f32s(k)?;
        let per_key = property.components()
            * if interpolation == Interp::CubicSpline {
                3
            } else {
                1
            };
        let values = r.f32s(k.checked_mul(per_key)?)?;
        channels.push(ClipChannel {
            target,
            property,
            interpolation,
            times,
            values,
        });
    }
    Some(ClipData { duration, channels })
}

// --- K: skins ------------------------------------------------------------------------------

/// A skin (the `K` blob's contents).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkinBlob {
    /// One per joint, column-major.
    pub inverse_bind: Vec<Mat4>,
    /// Each joint's child-index path from the skin's root node.
    pub joints: Vec<String>,
}

/// The `K` blob of `skin` (identity inverse binds for joints without one).
pub fn encode_skin(skin: &SkinBlob) -> Vec<u8> {
    let mut w = Writer::new(BlobKind::Skin);
    w.u32(skin.joints.len() as u32);
    for i in 0..skin.joints.len() {
        w.f32s(skin.inverse_bind.get(i).unwrap_or(&super::math::IDENTITY));
    }
    for j in &skin.joints {
        w.str(j);
    }
    w.0
}

/// Reads a `K` blob.
pub fn decode_skin(bytes: &[u8]) -> Option<SkinBlob> {
    let mut r = Reader::new(bytes, BlobKind::Skin)?;
    let n = r.u32()? as usize;
    let inverse_bind = group(&r.f32s(n.checked_mul(16)?)?);
    let joints = (0..n).map(|_| r.str()).collect::<Option<Vec<_>>>()?;
    Some(SkinBlob {
        inverse_bind,
        joints,
    })
}
