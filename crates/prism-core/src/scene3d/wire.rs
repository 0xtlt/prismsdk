//! What backends receive: a scene flattened into a pre-order table of
//! nodes ([`FlatScene`]), sent whole the first time and then as diffs
//! ([`SceneUpdate`]). [`SceneUpdate::to_json`] is the only serializer.
//!
//! ```json
//! { "v": 2,
//!   "full": true,               // first update (and after a reset): replace everything
//!   "settings": { "bg", "camera", "orbit", "lights", "t", "tap", "env", "exposure" },
//!                               // absent in a diff: unchanged; present: replaces all settings
//!   "nodes":  [FlatNode, …],    // full: every node, parents before children (pre-order)
//!   "upsert": [FlatNode, …],    // diff: nodes added, or whose props changed (replaced wholesale)
//!   "remove": ["key", …],       // diff: nodes gone
//!   "order":  ["key", …] }      // diff: the complete pre-order key list, present iff nodes
//!                               //       were added, removed or moved
//! ```
//!
//! Applying a diff ([`FlatScene::apply`]): drop the `remove`d keys, replace
//! or insert the `upsert`s, then if `order` is present reorder to it.
//!
//! A node's `key` identifies it across updates: `#name` for a named node,
//! else `<parent key>/<index>` (`<index>` at the top level). Keys are
//! opaque to backends (a key taken twice, by two nodes of the same name,
//! gets a `~n` suffix); they resolve `"#name"` clip targets and skin roots.
//! `parent` is the parent's key (absent at the top level). Defaults are
//! left out, as everywhere in the wire format.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::math::Aabb;
use super::{
    BlobRef, Camera, Light, Material, Mesh, Node3d, Orbit, Rgba, SceneData, SceneTransition,
    is_false, is_one, is_one3, is_true, is_zero, is_zero3, one, one3, yes,
};
use crate::style::Color;

/// Everything about a scene but its nodes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneSettings {
    /// Clear color; `None`: transparent (the element's own background shows).
    #[serde(rename = "bg", default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Rgba>,
    /// The point of view.
    #[serde(default)]
    pub camera: Camera,
    /// How the user can move the camera; `None`: it stays where it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orbit: Option<Orbit>,
    /// Lights (see [`effective_lights`](Self::effective_lights)).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lights: Vec<Light>,
    /// How changes animate; `None`: they apply at once.
    #[serde(rename = "t", default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<SceneTransition>,
    /// Taps are reported (`on_tap` is set).
    #[serde(default, skip_serializing_if = "is_false")]
    pub tap: bool,
    /// Image-based lighting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<EnvSettings>,
    /// Tone-mapping exposure (1: neutral).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub exposure: f32,
}

impl Default for SceneSettings {
    fn default() -> Self {
        SceneSettings {
            background: None,
            camera: Camera::default(),
            orbit: None,
            lights: Vec::new(),
            transition: None,
            tap: false,
            env: None,
            exposure: 1.,
        }
    }
}

impl SceneSettings {
    /// The lights to draw with: the app's, or — when it gave none and the
    /// scene has no environment — a soft default rig (ambient 0.35 and a
    /// white key light from the upper left).
    pub fn effective_lights(&self) -> Vec<Light> {
        if self.lights.is_empty() && self.env.is_none() {
            vec![
                Light::ambient(Color::WHITE, 0.35),
                Light::directional(Color::WHITE, 0.9).direction(-0.5, -1., -0.6),
            ]
        } else {
            self.lights.clone()
        }
    }
}

/// A scene's environment: `{"id", "intensity", "background"}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnvSettings {
    /// The `E` blob.
    pub id: BlobRef,
    /// Strength (default 1).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub intensity: f32,
    /// Drawn behind the scene.
    #[serde(default, skip_serializing_if = "is_false")]
    pub background: bool,
}

/// An instanced node's instances: `{"id", "count", "bounds"}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstancesRef {
    /// The `I` blob.
    pub id: BlobRef,
    /// How many instances.
    pub count: u32,
    /// Every instance of the node's mesh, in the node's space.
    pub bounds: Aabb,
}

/// The clip a node plays: `{"clip", "speed", "loop", "fade", "run"}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayRef {
    /// The `A` blob.
    pub clip: BlobRef,
    /// Rate (default 1).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub speed: f32,
    /// Starts over at the end.
    #[serde(rename = "loop", default, skip_serializing_if = "is_false")]
    pub looping: bool,
    /// Crossfade from the previous clip, seconds.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fade: f32,
    /// Changing it restarts the clip.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub run: u32,
}

fn is_zero_u32(x: &u32) -> bool {
    *x == 0
}

/// A skinned node's skin: `{"id", "root"}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkinRef {
    /// The `K` blob.
    pub id: BlobRef,
    /// The key of the node its joint paths start from.
    pub root: String,
}

/// One node without its children (they follow it in the table, `parent`
/// pointing back to it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlatNode {
    /// Its identity across updates (see the module docs).
    pub key: String,
    /// Its parent's key; `None`: top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Its name (taps report it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its label (taps report the nearest as `part`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Its shape; `None`: a group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<Mesh>,
    /// How its surface looks.
    #[serde(rename = "mat", default, skip_serializing_if = "Material::is_default")]
    pub material: Material,
    /// Position relative to the parent.
    #[serde(rename = "pos", default, skip_serializing_if = "is_zero3")]
    pub position: [f32; 3],
    /// Euler angles, degrees (used when `quat` is absent).
    #[serde(rename = "rot", default, skip_serializing_if = "is_zero3")]
    pub rotation: [f32; 3],
    /// Rotation quaternion `[x, y, z, w]` (wins over `rot`).
    #[serde(rename = "quat", default, skip_serializing_if = "Option::is_none")]
    pub quaternion: Option<[f32; 4]>,
    /// Scale.
    #[serde(default = "one3", skip_serializing_if = "is_one3")]
    pub scale: [f32; 3],
    /// Degrees per second around each axis.
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub spin: [f32; 3],
    /// Not drawn, nor its descendants.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// Instances of its mesh.
    #[serde(rename = "inst", default, skip_serializing_if = "Option::is_none")]
    pub instances: Option<InstancesRef>,
    /// The clip it plays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub play: Option<PlayRef>,
    /// Its skin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin: Option<SkinRef>,
    /// Its mesh casts shadows (default true).
    #[serde(rename = "cast", default = "yes", skip_serializing_if = "is_true")]
    pub cast_shadows: bool,
    /// Its mesh receives shadows (default true).
    #[serde(rename = "recv", default = "yes", skip_serializing_if = "is_true")]
    pub receive_shadows: bool,
}

impl FlatNode {
    /// A group keyed `key` at the top level, all defaults.
    pub fn new(key: impl Into<String>) -> FlatNode {
        FlatNode {
            key: key.into(),
            parent: None,
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
        }
    }
}

/// A scene as backends hold it: settings and nodes in pre-order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlatScene {
    /// Everything but the nodes.
    pub settings: SceneSettings,
    /// Every node, parents before children, siblings in order.
    pub nodes: Vec<FlatNode>,
}

impl SceneData {
    /// Its settings (everything but the nodes).
    pub fn settings(&self) -> SceneSettings {
        SceneSettings {
            background: self.background,
            camera: self.camera,
            orbit: self.orbit,
            lights: self.lights.clone(),
            transition: self.transition,
            tap: self.tap,
            env: self.environment.as_ref().map(|e| EnvSettings {
                id: e.blob.clone(),
                intensity: e.intensity,
                background: e.background,
            }),
            exposure: self.exposure,
        }
    }

    /// The scene as backends get it: its nodes in pre-order with their keys
    /// (O(nodes)).
    pub fn flatten(&self) -> FlatScene {
        let mut f = Flatten {
            out: Vec::new(),
            used: HashSet::new(),
            path: Vec::new(),
        };
        f.nodes(&self.nodes);
        FlatScene {
            settings: self.settings(),
            nodes: f.out,
        }
    }
}

struct Flatten {
    out: Vec<FlatNode>,
    used: HashSet<String>,
    /// Keys of the ancestors of the nodes being visited, top first.
    path: Vec<String>,
}

impl Flatten {
    fn nodes(&mut self, nodes: &[Node3d]) {
        for (i, n) in nodes.iter().enumerate() {
            let parent = self.path.last().cloned();
            let mut key = match (&n.name, &parent) {
                (Some(name), _) => format!("#{name}"),
                (None, None) => i.to_string(),
                (None, Some(p)) => format!("{p}/{i}"),
            };
            if self.used.contains(&key) {
                let mut k = 2;
                while self.used.contains(&format!("{key}~{k}")) {
                    k += 1;
                }
                key = format!("{key}~{k}");
            }
            self.used.insert(key.clone());
            // The skin's root: `up` levels above (0: this node).
            let skin = n.skin.as_ref().and_then(|s| {
                let up = s.up as usize;
                let root = match up {
                    0 => Some(key.clone()),
                    _ => self
                        .path
                        .len()
                        .checked_sub(up)
                        .map(|i| self.path[i].clone()),
                }?;
                Some(SkinRef {
                    id: s.blob.clone(),
                    root,
                })
            });
            self.out.push(FlatNode {
                key: key.clone(),
                parent,
                name: n.name.clone(),
                label: n.label.clone(),
                mesh: n.mesh.clone(),
                material: n.material.clone(),
                position: n.position,
                rotation: n.rotation,
                quaternion: n.quaternion,
                scale: n.scale,
                spin: n.spin,
                hidden: n.hidden,
                instances: n.instances.clone(),
                play: n.play.clone(),
                skin,
                cast_shadows: n.cast_shadows,
                receive_shadows: n.receive_shadows,
            });
            if !n.children.is_empty() {
                self.path.push(key);
                self.nodes(&n.children);
                self.path.pop();
            }
        }
    }
}

impl FlatScene {
    /// The update that turns `self` into `new` (`None`: they are equal).
    /// O(nodes).
    pub fn diff(&self, new: &FlatScene) -> Option<SceneUpdate> {
        let old: HashMap<&str, &FlatNode> =
            self.nodes.iter().map(|n| (n.key.as_str(), n)).collect();
        let kept: HashSet<&str> = new.nodes.iter().map(|n| n.key.as_str()).collect();
        let upsert: Vec<FlatNode> = new
            .nodes
            .iter()
            .filter(|n| old.get(n.key.as_str()).is_none_or(|o| *o != *n))
            .cloned()
            .collect();
        let remove: Vec<String> = self
            .nodes
            .iter()
            .filter(|n| !kept.contains(n.key.as_str()))
            .map(|n| n.key.clone())
            .collect();
        let same_order = self.nodes.len() == new.nodes.len()
            && self
                .nodes
                .iter()
                .zip(&new.nodes)
                .all(|(a, b)| a.key == b.key);
        let order = (!same_order).then(|| new.nodes.iter().map(|n| n.key.clone()).collect());
        let settings = (self.settings != new.settings).then(|| new.settings.clone());
        if upsert.is_empty() && remove.is_empty() && order.is_none() && settings.is_none() {
            return None;
        }
        Some(SceneUpdate::Diff(SceneDiff {
            settings,
            upsert,
            remove,
            order,
        }))
    }

    /// Applies an update (a full one replaces everything).
    pub fn apply(&mut self, update: &SceneUpdate) {
        let d = match update {
            SceneUpdate::Full(scene) => {
                *self = scene.clone();
                return;
            }
            SceneUpdate::Diff(d) => d,
        };
        if let Some(s) = &d.settings {
            self.settings = s.clone();
        }
        if !d.remove.is_empty() {
            let gone: HashSet<&str> = d.remove.iter().map(String::as_str).collect();
            self.nodes.retain(|n| !gone.contains(n.key.as_str()));
        }
        if !d.upsert.is_empty() {
            let mut at: HashMap<String, usize> = self
                .nodes
                .iter()
                .enumerate()
                .map(|(i, n)| (n.key.clone(), i))
                .collect();
            for u in &d.upsert {
                match at.get(&u.key) {
                    Some(i) => self.nodes[*i] = u.clone(),
                    None => {
                        at.insert(u.key.clone(), self.nodes.len());
                        self.nodes.push(u.clone());
                    }
                }
            }
        }
        if let Some(order) = &d.order {
            let mut by_key: HashMap<String, FlatNode> = std::mem::take(&mut self.nodes)
                .into_iter()
                .map(|n| (n.key.clone(), n))
                .collect();
            self.nodes = order.iter().filter_map(|k| by_key.remove(k)).collect();
        }
    }
}

/// A change of scene for backends.
#[derive(Clone, Debug, PartialEq)]
pub enum SceneUpdate {
    /// Replace everything (the first update, and after a reset).
    Full(FlatScene),
    /// Change some nodes or settings.
    Diff(SceneDiff),
}

/// The changes of a [`SceneUpdate::Diff`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneDiff {
    /// New settings (replacing all of them), if they changed.
    pub settings: Option<SceneSettings>,
    /// Nodes added or changed.
    pub upsert: Vec<FlatNode>,
    /// Keys of the nodes gone.
    pub remove: Vec<String>,
    /// Every key in pre-order, when nodes were added, removed or moved.
    pub order: Option<Vec<String>>,
}

impl SceneUpdate {
    /// The full update of `scene`.
    pub fn full(scene: &SceneData) -> SceneUpdate {
        SceneUpdate::Full(scene.flatten())
    }

    /// The JSON backends receive (see the module docs).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| r#"{"v":2}"#.into())
    }

    /// [`to_json`](Self::to_json) as a JSON value, to embed in a message.
    /// (Serializing straight into a value would widen every `f32` to a long
    /// `f64` such as `0.10000000149011612`.)
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::from_str(&self.to_json()).unwrap_or(serde_json::Value::Null)
    }

    /// Reads [`to_json`](Self::to_json)'s output (`None` if unreadable or
    /// not version 2).
    pub fn from_json(json: &str) -> Option<SceneUpdate> {
        serde_json::from_str(json).ok()
    }

    /// The nodes it carries: all of them (full) or the upserted ones.
    pub fn nodes(&self) -> &[FlatNode] {
        match self {
            SceneUpdate::Full(s) => &s.nodes,
            SceneUpdate::Diff(d) => &d.upsert,
        }
    }

    /// The nodes it carries, to change in place (texture URLs…).
    pub fn nodes_mut(&mut self) -> &mut [FlatNode] {
        match self {
            SceneUpdate::Full(s) => &mut s.nodes,
            SceneUpdate::Diff(d) => &mut d.upsert,
        }
    }

    /// The settings it carries (always for a full update).
    pub fn settings(&self) -> Option<&SceneSettings> {
        match self {
            SceneUpdate::Full(s) => Some(&s.settings),
            SceneUpdate::Diff(d) => d.settings.as_ref(),
        }
    }
}

/// The JSON form, borrowed for writing.
#[derive(Serialize)]
struct WireOut<'a> {
    v: u32,
    #[serde(skip_serializing_if = "is_false")]
    full: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    settings: Option<&'a SceneSettings>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    nodes: &'a [FlatNode],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    upsert: &'a [FlatNode],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    remove: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    order: Option<&'a [String]>,
}

/// The JSON form, owned for reading.
#[derive(Deserialize)]
struct WireIn {
    v: u32,
    #[serde(default)]
    full: bool,
    #[serde(default)]
    settings: Option<SceneSettings>,
    #[serde(default)]
    nodes: Vec<FlatNode>,
    #[serde(default)]
    upsert: Vec<FlatNode>,
    #[serde(default)]
    remove: Vec<String>,
    #[serde(default)]
    order: Option<Vec<String>>,
}

impl Serialize for SceneUpdate {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let out = match self {
            SceneUpdate::Full(scene) => WireOut {
                v: 2,
                full: true,
                settings: Some(&scene.settings),
                nodes: &scene.nodes,
                upsert: &[],
                remove: &[],
                order: None,
            },
            SceneUpdate::Diff(d) => WireOut {
                v: 2,
                full: false,
                settings: d.settings.as_ref(),
                nodes: &[],
                upsert: &d.upsert,
                remove: &d.remove,
                order: d.order.as_deref(),
            },
        };
        out.serialize(s)
    }
}

impl<'de> Deserialize<'de> for SceneUpdate {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = WireIn::deserialize(d)?;
        if w.v != 2 {
            return Err(serde::de::Error::custom("not a version 2 scene update"));
        }
        Ok(if w.full {
            SceneUpdate::Full(FlatScene {
                settings: w.settings.unwrap_or_default(),
                nodes: w.nodes,
            })
        } else {
            SceneUpdate::Diff(SceneDiff {
                settings: w.settings,
                upsert: w.upsert,
                remove: w.remove,
                order: w.order,
            })
        })
    }
}
