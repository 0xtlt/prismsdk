//! Keyframe animation clips: built in Rust ([`AnimationClip::new`]) or read
//! from a glTF file ([`Model::clips`](super::Model::clips)), played on the
//! device by a node ([`Node3d::play`](super::Node3d::play)).
//!
//! A clip is a set of channels, each animating the translation, rotation
//! or scale of one node. Channel targets: `""` is the playing node,
//! `"0/2"` a child-index path from it (its first child's third child),
//! `"#name"` the node named `name` anywhere in the scene.
//!
//! [`ClipChannel::sample`] is the reference sampler every backend ports.

use std::sync::{Arc, OnceLock};

use super::blobs::{self, BINARY_MIME, BlobKind, BlobRef};
use super::math;

/// How a channel goes from one key to the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Interp {
    /// Straight (spherical for rotations).
    #[default]
    Linear,
    /// Holds each key until the next.
    Step,
    /// Cubic Hermite (glTF's `CUBICSPLINE`): each key has an in-tangent, a
    /// value and an out-tangent. Keys given without tangents get flat ones
    /// (they ease in and out).
    CubicSpline,
}

/// What a channel animates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipProperty {
    /// Position (3 values per key).
    Translation,
    /// Rotation, a unit quaternion `[x, y, z, w]` (4 values per key).
    Rotation,
    /// Scale (3 values per key).
    Scale,
}

impl ClipProperty {
    /// Values per key (without tangents).
    pub fn components(self) -> usize {
        match self {
            ClipProperty::Rotation => 4,
            _ => 3,
        }
    }
}

/// One animated property of one node.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipChannel {
    /// `""`, a child-index path (`"0/2"`) or `"#name"` (see the module docs).
    pub target: String,
    /// What it animates.
    pub property: ClipProperty,
    /// How it interpolates.
    pub interpolation: Interp,
    /// Key times, seconds, increasing.
    pub times: Vec<f32>,
    /// Key values, flat: `components()` per key, × 3 for cubic splines
    /// (in-tangent, value, out-tangent).
    pub values: Vec<f32>,
}

impl ClipChannel {
    /// The value at `t` seconds (`[x, y, z, _]` for translations and
    /// scales, a unit quaternion for rotations): the first key's value
    /// before it, the last one's after it.
    pub fn sample(&self, t: f32) -> [f32; 4] {
        let c = self.property.components();
        let cubic = self.interpolation == Interp::CubicSpline;
        let stride = if cubic { 3 * c } else { c };
        let k = self.times.len().min(self.values.len() / stride.max(1));
        if k == 0 {
            return match self.property {
                ClipProperty::Rotation => math::QUAT_IDENTITY,
                ClipProperty::Scale => [1., 1., 1., 0.],
                ClipProperty::Translation => [0.; 4],
            };
        }
        // A key's value: the middle third of a cubic key.
        let value = |i: usize| -> [f32; 4] {
            let at = i * stride + if cubic { c } else { 0 };
            std::array::from_fn(|j| if j < c { self.values[at + j] } else { 0. })
        };
        let finish = |v: [f32; 4]| match self.property {
            ClipProperty::Rotation => math::quat_normalize(v),
            _ => v,
        };
        if t <= self.times[0] || k == 1 {
            return finish(value(0));
        }
        if t >= self.times[k - 1] {
            return finish(value(k - 1));
        }
        // The key before t (times increase: binary search).
        let i = self.times[..k]
            .partition_point(|x| *x <= t)
            .saturating_sub(1);
        let (t0, t1) = (self.times[i], self.times[i + 1]);
        let dt = t1 - t0;
        let s = if dt > 0. { (t - t0) / dt } else { 0. };
        match self.interpolation {
            Interp::Step => finish(value(i)),
            Interp::Linear => match self.property {
                ClipProperty::Rotation => math::slerp(value(i), value(i + 1), s),
                _ => {
                    let (a, b) = (value(i), value(i + 1));
                    std::array::from_fn(|j| math::lerp(a[j], b[j], s))
                }
            },
            Interp::CubicSpline => {
                // glTF: p(s) = (2s³ − 3s² + 1)·v0 + (s³ − 2s² + s)·dt·b0
                //            + (−2s³ + 3s²)·v1 + (s³ − s²)·dt·a1
                // (b0: key i's out-tangent, a1: key i+1's in-tangent).
                let (s2, s3) = (s * s, s * s * s);
                let v0 = i * stride + c;
                let b0 = i * stride + 2 * c;
                let a1 = (i + 1) * stride;
                let v1 = (i + 1) * stride + c;
                let v = std::array::from_fn(|j| {
                    if j >= c {
                        return 0.;
                    }
                    (2. * s3 - 3. * s2 + 1.) * self.values[v0 + j]
                        + (s3 - 2. * s2 + s) * dt * self.values[b0 + j]
                        + (-2. * s3 + 3. * s2) * self.values[v1 + j]
                        + (s3 - s2) * dt * self.values[a1 + j]
                });
                finish(v)
            }
        }
    }
}

/// A clip's channels (the `A` blob's contents).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipData {
    /// Seconds: the last key time of all channels.
    pub duration: f32,
    /// Its channels.
    pub channels: Vec<ClipChannel>,
}

/// Keyframes for some nodes' position, rotation and scale, played on the
/// device by [`Node3d::play`](super::Node3d::play). Cheap to clone.
///
/// ```ignore
/// let bob = AnimationClip::new("bob")
///     .translation("", &[0., 0.5, 1.], &[[0., 0., 0.], [0., 0.3, 0.], [0., 0., 0.]])
///     .rotation("0", &[0., 1.], &[[0., 0., 0., 1.], [0., 1., 0., 0.]])
///     .interpolation(Interp::CubicSpline);
/// ```
#[derive(Clone, Debug)]
pub struct AnimationClip {
    name: String,
    data: Arc<ClipData>,
    interpolation: Interp,
    /// The `A` blob, encoded on first use (shared by clones).
    blob: Arc<OnceLock<BlobRef>>,
}

impl PartialEq for AnimationClip {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.data == other.data
    }
}

impl AnimationClip {
    /// An empty clip.
    pub fn new(name: impl Into<String>) -> Self {
        AnimationClip {
            name: name.into(),
            data: Arc::default(),
            interpolation: Interp::Linear,
            blob: Arc::default(),
        }
    }

    /// A clip of these channels (glTF import).
    pub(crate) fn from_data(name: String, data: ClipData) -> Self {
        AnimationClip {
            name,
            data: Arc::new(data),
            interpolation: Interp::Linear,
            blob: Arc::default(),
        }
    }

    /// Its name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its length, seconds.
    pub fn duration(&self) -> f32 {
        self.data.duration
    }

    /// Its channels.
    pub fn data(&self) -> &ClipData {
        &self.data
    }

    /// Its `A` blob (what the scene sends).
    pub fn blob(&self) -> BlobRef {
        self.blob
            .get_or_init(|| blobs::put(BlobKind::Clip, BINARY_MIME, blobs::encode_clip(&self.data)))
            .clone()
    }

    fn channel(
        mut self,
        target: &str,
        property: ClipProperty,
        times: &[f32],
        values: Vec<f32>,
    ) -> Self {
        let data = Arc::make_mut(&mut self.data);
        data.channels.push(ClipChannel {
            target: target.to_string(),
            property,
            interpolation: Interp::Linear,
            times: times.to_vec(),
            values,
        });
        self.blob = Arc::default();
        let interpolation = self.interpolation;
        self.set_interpolation(interpolation)
    }

    /// Moves `target` (`""`: the playing node; `"0/2"`: a child-index path
    /// from it; `"#name"`: a named node) through `positions` at `times`
    /// (seconds).
    pub fn translation(self, target: &str, times: &[f32], positions: &[[f32; 3]]) -> Self {
        let values = positions.iter().flatten().copied().collect();
        self.channel(target, ClipProperty::Translation, times, values)
    }

    /// Turns `target` through `rotations` (unit quaternions `[x, y, z, w]`)
    /// at `times`.
    pub fn rotation(self, target: &str, times: &[f32], rotations: &[[f32; 4]]) -> Self {
        let values = rotations
            .iter()
            .flat_map(|q| math::quat_normalize(*q))
            .collect();
        self.channel(target, ClipProperty::Rotation, times, values)
    }

    /// Scales `target` through `scales` at `times`.
    pub fn scale(self, target: &str, times: &[f32], scales: &[[f32; 3]]) -> Self {
        let values = scales.iter().flatten().copied().collect();
        self.channel(target, ClipProperty::Scale, times, values)
    }

    /// How every channel interpolates (those added before and after;
    /// default linear). [`Interp::CubicSpline`] on keys given without
    /// tangents eases in and out of each key.
    pub fn interpolation(mut self, interpolation: Interp) -> Self {
        self.interpolation = interpolation;
        self.blob = Arc::default();
        self.set_interpolation(interpolation)
    }

    fn set_interpolation(mut self, interpolation: Interp) -> Self {
        let data = Arc::make_mut(&mut self.data);
        for c in &mut data.channels {
            let n = c.property.components();
            let k = c.times.len();
            match (c.interpolation, interpolation) {
                (Interp::CubicSpline, Interp::CubicSpline) => {}
                // Flat tangents around each key.
                (_, Interp::CubicSpline) if c.values.len() == k * n => {
                    c.values = c
                        .values
                        .chunks(n)
                        .flat_map(|v| {
                            std::iter::repeat_n(0., n)
                                .chain(v.iter().copied())
                                .chain(std::iter::repeat_n(0., n))
                        })
                        .collect();
                }
                // Back to plain keys: drop the tangents.
                (Interp::CubicSpline, _) if c.values.len() == k * n * 3 => {
                    c.values = c
                        .values
                        .chunks(n * 3)
                        .flat_map(|v| v[n..2 * n].to_vec())
                        .collect();
                }
                _ => {}
            }
            c.interpolation = interpolation;
        }
        data.duration = data
            .channels
            .iter()
            .filter_map(|c| c.times.last().copied())
            .fold(0., f32::max);
        self
    }
}

/// How a node plays a clip ([`Node3d::play`](super::Node3d::play)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Playback {
    /// Starts over at the end.
    pub looping: bool,
    /// Rate: 1 is real time, 2 twice as fast.
    pub speed: f32,
    /// Seconds over which this clip blends in while the node's previous
    /// clip blends out (0: it replaces it at once).
    pub fade: f32,
    /// Changing it restarts the clip.
    pub restart: u32,
}

impl Playback {
    /// Plays once, then holds the last pose.
    pub fn once() -> Self {
        Playback {
            looping: false,
            speed: 1.,
            fade: 0.,
            restart: 0,
        }
    }
    /// Plays over and over.
    pub fn looping() -> Self {
        Playback {
            looping: true,
            ..Self::once()
        }
    }
    /// Rate (1: real time).
    pub fn speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }
    /// Crossfades from the node's previous clip over `seconds`.
    pub fn fade(mut self, seconds: f32) -> Self {
        self.fade = seconds.max(0.);
        self
    }
    /// Restarts the clip whenever `n` changes (e.g. a counter of the times
    /// the user asked for it).
    pub fn restart(mut self, n: u32) -> Self {
        self.restart = n;
        self
    }
}
