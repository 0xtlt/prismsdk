use super::blobs::{self, BlobKind, InstanceTransform, InstancesBlob, MeshBlob, SkinBlob};
use super::clip::{ClipChannel, ClipData, ClipProperty};
use super::geometry::{self, MeshData};
use super::gltf::{
    Imported, ImportedAnimation, ImportedChannel, ImportedImage, ImportedInterpolation,
};
use super::gltf::{ImportedAlpha, ImportedMaterial, ImportedMesh, ImportedNode, ImportedPath};
use super::gltf::{ImportedPrimitive, ImportedSkin};
use super::math::{self, cross, dot, sub};
use super::player::{SHADOW_MAP_SIZE, ScenePlayer, ease};
use super::shaders::{self, Stage, Variant};
use super::*;
use crate::{Transition, rgb};

// --- Helpers -------------------------------------------------------------------------------

/// A player showing `d` from `now`.
fn player(d: &SceneData, now: f64) -> ScenePlayer {
    let mut p = ScenePlayer::new(now);
    p.apply(&SceneUpdate::full(d), now);
    p
}

/// Shows `new` on a player that showed `old`, as the reconciler sends it.
fn show(p: &mut ScenePlayer, old: &SceneData, new: &SceneData, now: f64) {
    if let Some(u) = old.flatten().diff(&new.flatten()) {
        p.apply(&u, now);
    }
}

fn names(p: &ScenePlayer) -> Vec<String> {
    p.frame(1., 1., false)
        .draws
        .iter()
        .map(|d| d.name.unwrap_or("-").to_string())
        .collect()
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

fn near_mat(a: &math::Mat4, b: &math::Mat4) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
}

// --- Geometry and math (v1) ------------------------------------------------------------------

fn faces_point_outward(m: &MeshData, center: [f32; 3]) {
    assert_eq!(m.indices.len() % 3, 0);
    for t in m.indices.as_chunks::<3>().0 {
        let (a, b, c) = (m.position(t[0]), m.position(t[1]), m.position(t[2]));
        let n = cross(sub(b, a), sub(c, a));
        if math::length(n) < 1e-7 {
            continue; // Degenerate (pole) triangles are allowed.
        }
        let mid = [
            (a[0] + b[0] + c[0]) / 3.,
            (a[1] + b[1] + c[1]) / 3.,
            (a[2] + b[2] + c[2]) / 3.,
        ];
        assert!(
            dot(n, sub(mid, center)) > 0.,
            "triangle {t:?} faces inward: {a:?} {b:?} {c:?}"
        );
        // Vertex normals agree with the winding.
        let vn = [
            m.normals[t[0] as usize * 3],
            m.normals[t[0] as usize * 3 + 1],
            m.normals[t[0] as usize * 3 + 2],
        ];
        assert!(
            dot(n, vn) > 0.,
            "normal of {t:?} disagrees with its winding"
        );
    }
    assert_eq!(m.normals.len(), m.positions.len());
    assert_eq!(m.uvs.len() / 2, m.positions.len() / 3);
}

#[test]
fn primitives_wind_counter_clockwise_from_outside() {
    faces_point_outward(&geometry::cuboid([1., 2., 3.]), [0.; 3]);
    faces_point_outward(&geometry::sphere(1.), [0.; 3]);
    faces_point_outward(&geometry::cylinder(1., 1., 2.), [0.; 3]);
    faces_point_outward(&geometry::cylinder(1., 0., 2.), [0.; 3]);
    let p = geometry::plane([2., 2.]);
    for t in p.indices.as_chunks::<3>().0 {
        let n = cross(
            sub(p.position(t[1]), p.position(t[0])),
            sub(p.position(t[2]), p.position(t[0])),
        );
        assert!(n[1] > 0., "the plane faces up");
    }
}

#[test]
fn torus_faces_away_from_its_tube() {
    let m = geometry::torus(2., 0.5);
    for t in m.indices.as_chunks::<3>().0 {
        let (a, b, c) = (m.position(t[0]), m.position(t[1]), m.position(t[2]));
        let n = cross(sub(b, a), sub(c, a));
        if math::length(n) < 1e-7 {
            continue;
        }
        let mid = [
            (a[0] + b[0] + c[0]) / 3.,
            (a[1] + b[1] + c[1]) / 3.,
            (a[2] + b[2] + c[2]) / 3.,
        ];
        // The nearest point of the tube's center circle.
        let r = (mid[0] * mid[0] + mid[2] * mid[2]).sqrt();
        let ring = [mid[0] / r * 2., 0., mid[2] / r * 2.];
        assert!(dot(n, sub(mid, ring)) > 0.);
    }
}

#[test]
fn box_has_the_requested_extent() {
    let m = geometry::cuboid([1., 2., 3.]);
    let max = |axis: usize| {
        m.positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| p[axis])
            .fold(f32::MIN, f32::max)
    };
    assert_eq!((max(0), max(1), max(2)), (0.5, 1., 1.5));
    assert_eq!(m.vertex_count(), 24);
    assert_eq!(m.indices.len(), 36);
    assert_eq!(
        Mesh::Box { size: [1., 2., 3.] }.bounds(),
        [-0.5, -1., -1.5, 0.5, 1., 1.5]
    );
}

#[test]
fn custom_meshes_are_blobs_with_normals() {
    let tri = Mesh::custom(&[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]], None);
    let Mesh::Blob { id, bounds } = &tri else {
        panic!("a blob mesh")
    };
    assert_eq!(id.id().len(), 32);
    assert_eq!(*bounds, [0., 0., 0., 1., 1., 0.]);
    let data = tri.data().unwrap();
    assert_eq!(data.indices, [0, 1, 2]);
    assert_eq!(data.normals[0], [0., 0., 1.]);
    let g = geometry::generate(&tri);
    assert_eq!(g.vertex_count(), 3);
    assert_eq!(g.uvs.len(), 6, "uvs default to (0, 0)");
    let other = Mesh::custom(&[[0., 0., 0.], [2., 0., 0.], [0., 1., 0.]], None);
    assert_ne!(tri, other);
    assert_ne!(tri.key(), other.key());
    // Uvs and colors make a new blob; out-of-range indices drop their triangle.
    let uv = tri.clone().with_uvs(&[[0., 0.], [1., 0.], [0., 1.]]);
    assert_eq!(uv.data().unwrap().uvs.len(), 3);
    assert_ne!(uv, tri);
    let bad = Mesh::custom(
        &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        Some(&[0, 1, 2, 0, 1, 9]),
    );
    assert_eq!(bad.data().unwrap().indices, [0, 1, 2]);
}

#[test]
fn matrices_compose_and_invert() {
    let m = math::compose([1., 2., 3.], [30., 45., 60.], [2., 2., 2.]);
    let back = math::mul(&math::invert(&m), &m);
    for (i, v) in back.iter().enumerate() {
        let expected = if i % 5 == 0 { 1. } else { 0. };
        assert!((v - expected).abs() < 1e-4, "{back:?}");
    }
    // Rz applies first: X turns to Y, then around Y by 90°: Y stays.
    let r = math::rotation([0., 90., 90.]);
    let p = math::transform_point(&r, [1., 0., 0.]);
    assert!((p[1] - 1.).abs() < 1e-5, "{p:?}");
    // Around Y by 90°, +Z goes to +X (counter-clockwise seen from above).
    let p = math::transform_point(&math::rotation_y(90.), [0., 0., 1.]);
    assert!((p[0] - 1.).abs() < 1e-5, "{p:?}");
}

#[test]
fn quaternions_match_euler_angles() {
    for e in [[30., 45., 60.], [0., 90., 0.], [-120., 10., 200.]] {
        let q = math::quat_from_euler(e);
        assert!(
            near_mat(&math::quat_to_mat4(q), &math::rotation(e)),
            "{e:?}"
        );
        let m = math::compose_quat([1., 2., 3.], q, [2., 3., 4.]);
        assert!(near_mat(&m, &math::compose([1., 2., 3.], e, [2., 3., 4.])));
    }
    // Slerp halfway between identity and a half turn around Y: a quarter turn.
    let half = math::slerp(math::QUAT_IDENTITY, [0., 1., 0., 0.], 0.5);
    assert!(near_mat(&math::quat_to_mat4(half), &math::rotation_y(90.)));
}

#[test]
fn boxes_transform_and_meet_rays() {
    let b = math::transform_aabb(
        &math::compose([10., 0., 0.], [0., 0., 45.], [1.; 3]),
        &[-1., -1., -1., 1., 1., 1.],
    );
    let s = 2f32.sqrt();
    assert!(
        near(b[0], 10. - s) && near(b[3], 10. + s) && near(b[2], -1.),
        "{b:?}"
    );
    assert_eq!(
        math::ray_aabb([0., 0., 5.], [0., 0., -1.], &[-1., -1., -1., 1., 1., 1.]),
        Some(4.)
    );
    assert_eq!(
        math::ray_aabb([3., 0., 5.], [0., 0., -1.], &[-1., -1., -1., 1., 1., 1.]),
        None
    );
}

#[test]
fn the_camera_looks_at_its_target() {
    let view = math::look_at([0., 0., 5.], [0.; 3], [0., 1., 0.]);
    let p = math::transform_point(&view, [0.; 3]);
    assert!(
        (p[2] + 5.).abs() < 1e-5,
        "the target is 5 units in front: {p:?}"
    );
    let proj = math::perspective(45., 1., 0.1, 100.);
    let c = math::transform_point(&math::mul(&proj, &view), [0.; 3]);
    assert!(c[0].abs() < 1e-5 && c[1].abs() < 1e-5 && c[2] > -1. && c[2] < 1.);
}

// --- Blobs ---------------------------------------------------------------------------------

#[test]
fn blobs_are_content_addressed_and_live_while_referenced() {
    let a = blobs::put(BlobKind::Image, "image/png", vec![1, 2, 3, 4, 5]);
    let b = blobs::put(BlobKind::Image, "image/png", vec![1, 2, 3, 4, 5]);
    let c = blobs::put(BlobKind::Mesh, "image/png", vec![1, 2, 3, 4, 5]);
    assert_eq!(a, b);
    assert_ne!(a, c, "the kind is part of the id");
    assert_eq!(a.id().len(), 32);
    assert!(a.id().bytes().all(|b| b.is_ascii_hexdigit()));
    let (bytes, mime) = blobs::get(a.id()).unwrap();
    assert_eq!(
        (&*bytes, mime.as_str()),
        (&[1u8, 2, 3, 4, 5][..], "image/png")
    );
    // Serialized as its id; read back, it finds its bytes.
    let json = serde_json::to_string(&a).unwrap();
    assert_eq!(json, format!("\"{}\"", a.id()));
    let back: BlobRef = serde_json::from_str(&json).unwrap();
    assert_eq!(back.bytes().as_deref(), Some(&[1u8, 2, 3, 4, 5][..]));
    assert!(serde_json::from_str::<BlobRef>("\"nope\"").is_err());
    let id = c.id().to_string();
    drop(c);
    assert!(blobs::get(&id).is_none(), "gone with its last reference");
    assert!(blobs::get("../../etc/passwd").is_none());
}

#[test]
fn lazy_blobs_are_made_when_first_read() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static MADE: AtomicUsize = AtomicUsize::new(0);
    fn make() -> Vec<u8> {
        MADE.fetch_add(1, Ordering::SeqCst);
        vec![7, 8, 9]
    }
    let a = blobs::put_lazy(BlobKind::Environment, blobs::BINARY_MIME, "test:lazy", make);
    let b = blobs::put_lazy(BlobKind::Environment, blobs::BINARY_MIME, "test:lazy", make);
    assert_eq!(a, b, "the same name, the same blob");
    assert_eq!(MADE.load(Ordering::SeqCst), 0, "not made until read");
    assert_eq!(a.bytes().as_deref(), Some(&[7u8, 8, 9][..]));
    let (bytes, mime) = blobs::get(b.id()).unwrap();
    assert_eq!(
        (&*bytes, mime.as_str()),
        (&[7u8, 8, 9][..], blobs::BINARY_MIME)
    );
    assert_eq!(MADE.load(Ordering::SeqCst), 1, "made once");
}

#[test]
fn builtin_environments_wait_for_a_renderer() {
    let e = Environment::studio();
    assert_eq!(e, Environment::studio());
    // Read like a renderer does: a prefiltered map.
    let map = env::EnvMap::decode(&e.blob.bytes().unwrap()).unwrap();
    let up = map.irradiance([0., 1., 0.]);
    assert!(up.iter().all(|c| *c > 0.), "{up:?}");
}

#[test]
fn mesh_blobs_round_trip_with_every_attribute() {
    let mesh = MeshBlob {
        positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 2., 0.]],
        normals: vec![[0., 0., 1.]; 3],
        uvs: vec![[0., 0.], [1., 0.], [0., 1.]],
        joints: vec![[0, 1, 2, 3], [1, 0, 0, 0], [65535, 0, 0, 0]],
        weights: vec![[0.25; 4], [1., 0., 0., 0.], [0.5, 0.5, 0., 0.]],
        colors: vec![[1., 0., 0., 1.], [0., 1., 0., 1.], [0., 0., 1., 0.5]],
        indices: vec![0, 1, 2],
        bounds: [0.; 6],
    };
    let bytes = blobs::encode_mesh(&mesh);
    assert_eq!(&bytes[..4], b"P3DM");
    assert_eq!(bytes.len() % 4, 0);
    let back = blobs::decode_mesh(&bytes).unwrap();
    assert_eq!(back.bounds, [0., 0., 0., 1., 2., 0.]);
    assert_eq!(
        MeshBlob {
            bounds: back.bounds,
            ..mesh.clone()
        },
        back
    );
    // Without optional attributes.
    let plain = MeshBlob {
        uvs: vec![],
        joints: vec![],
        weights: vec![],
        colors: vec![],
        ..mesh
    };
    let back = blobs::decode_mesh(&blobs::encode_mesh(&plain)).unwrap();
    assert!(back.uvs.is_empty() && back.joints.is_empty() && back.colors.is_empty());
    assert_eq!(back.positions, plain.positions);
}

#[test]
fn instance_clip_and_skin_blobs_round_trip() {
    let set = InstancesBlob {
        transforms: vec![
            InstanceTransform {
                position: [1., 2., 3.],
                rotation: [0., 0., 0., 1.],
                scale: [1., 2., 1.],
            };
            2
        ],
        colors: vec![[1., 0.5, 0.25, 1.]; 2],
        bounds: [-1., -2., -3., 1., 2., 3.],
    };
    assert_eq!(
        blobs::decode_instances(&blobs::encode_instances(&set)),
        Some(set)
    );

    let clip = ClipData {
        duration: 2.,
        channels: vec![
            ClipChannel {
                target: "0/2".into(),
                property: ClipProperty::Translation,
                interpolation: Interp::Linear,
                times: vec![0., 2.],
                values: vec![0., 0., 0., 1., 2., 3.],
            },
            ClipChannel {
                target: "#héllo".into(),
                property: ClipProperty::Rotation,
                interpolation: Interp::CubicSpline,
                times: vec![0.],
                values: vec![0., 0., 0., 0., 0., 0., 0., 1., 0., 0., 0., 0.],
            },
        ],
    };
    let bytes = blobs::encode_clip(&clip);
    assert_eq!(bytes.len() % 4, 0);
    assert_eq!(blobs::decode_clip(&bytes), Some(clip));

    let skin = SkinBlob {
        inverse_bind: vec![math::translation([0., -2., 0.]), math::IDENTITY],
        joints: vec!["0".into(), "0/1/12".into()],
    };
    assert_eq!(blobs::decode_skin(&blobs::encode_skin(&skin)), Some(skin));
}

#[test]
fn malformed_blobs_decode_to_none() {
    let mesh = blobs::encode_mesh(&MeshBlob {
        positions: vec![[0.; 3]; 3],
        normals: vec![[0., 1., 0.]; 3],
        indices: vec![0, 1, 2],
        ..Default::default()
    });
    let clip = AnimationClip::new("c")
        .translation("", &[0., 1.], &[[0.; 3], [1.; 3]])
        .data()
        .clone();
    let clip = blobs::encode_clip(&clip);
    // Every truncation, and garbage: no panic.
    for n in 0..mesh.len() {
        assert!(blobs::decode_mesh(&mesh[..n]).is_none());
    }
    for n in 0..clip.len() {
        assert!(blobs::decode_clip(&clip[..n]).is_none());
    }
    assert!(blobs::decode_skin(b"P3DK\x01\0\0\0\xff\xff\xff\xff").is_none());
    assert!(blobs::decode_instances(b"P3DI\x01\0\0\0\xff\xff\xff\x7f").is_none());
    assert!(blobs::decode_mesh(&clip).is_none(), "the wrong kind");
    // An index out of range.
    let mut bad = mesh.clone();
    let at = bad.len() - 4;
    bad[at..].copy_from_slice(&7u32.to_le_bytes());
    assert!(blobs::decode_mesh(&bad).is_none());
}

// --- Wire ----------------------------------------------------------------------------------

#[test]
fn scenes_round_trip_through_json() {
    let s = scene3d()
        .camera(Camera::perspective(50.).position(0., 1., 4.))
        .orbit(Orbit::new().auto_rotate(10.))
        .background(rgb(0x101010))
        .light(
            Light::point(rgb(0xffffff), 0.8)
                .position(1., 2., 3.)
                .range(10.),
        )
        .light(Light::directional(rgb(0xffffff), 1.).shadows(true))
        .exposure(1.5)
        .child(
            cube(1.)
                .name("cube")
                .color(crate::Color::adaptive(rgb(0xff0000), rgb(0x00ff00)))
                .spin(0., 90., 0.)
                .child(
                    sphere(0.2)
                        .position(0., 1., 0.)
                        .label("moon")
                        .quaternion(0., 1., 0., 1.),
                ),
        )
        .child(
            mesh(Mesh::custom(
                &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
                None,
            ))
            .cast_shadows(false),
        )
        .transition(Transition::ease(300));
    let Element::Node(node) = s.into_element() else {
        panic!()
    };
    assert_eq!(node.kind, NodeKind::Scene3d);
    assert!(node.text.is_empty(), "the scene is not text anymore");
    let data = node.scene.unwrap();
    let flat = data.flatten();
    let keys: Vec<_> = flat.nodes.iter().map(|n| n.key.as_str()).collect();
    assert_eq!(keys, ["#cube", "#cube/0", "1"]);
    assert_eq!(flat.nodes[1].parent.as_deref(), Some("#cube"));
    assert_eq!(flat.nodes[0].material.color.dark, Some([0., 1., 0., 1.]));
    assert_eq!(flat.settings.transition.map(|t| t.ms), Some(300.));
    assert!(!flat.settings.tap);
    let json = SceneUpdate::Full(flat.clone()).to_json();
    assert!(
        json.starts_with(r#"{"v":2,"full":true,"settings":{"#),
        "{json}"
    );
    for field in [
        r#""label":"moon""#,
        r#""quat":["#,
        r#""cast":false"#,
        r#""shadows":true"#,
        r#""exposure":1.5"#,
        r#""type":"blob""#,
    ] {
        assert!(json.contains(field), "{field} in {json}");
    }
    // Defaults stay out of the JSON.
    for absent in [
        "\"scale\"",
        "\"recv\"",
        "\"hidden\"",
        "\"normal_scale\"",
        "\"alpha\"",
    ] {
        assert!(!json.contains(absent), "{absent} in {json}");
    }
    // Colors keep 4 decimals in JSON: the JSON round trips.
    assert_eq!(
        SceneUpdate::from_json(&json).map(|u| u.to_json()),
        Some(json)
    );
    assert_eq!(SceneUpdate::from_json("not json"), None);
    assert_eq!(SceneUpdate::from_json(r#"{"v":1}"#), None);
    assert_eq!(
        SceneUpdate::from_json(
            r#"{"v":2,"full":true,"nodes":[{"key":"0","mesh":{"type":"blob","id":"zz","bounds":[0,0,0,0,0,0]}}]}"#
        ),
        None,
        "a bad blob id fails the update, not the process"
    );
}

#[test]
fn duplicate_names_get_distinct_keys() {
    let mut d = SceneData::default();
    d.nodes.push(cube(1.).name("a").child(cube(1.)));
    d.nodes.push(cube(1.).name("a").child(cube(1.)));
    let keys: Vec<_> = d.flatten().nodes.into_iter().map(|n| n.key).collect();
    assert_eq!(keys, ["#a", "#a/0", "#a~2", "#a~2/0"]);
}

#[test]
fn diffs_carry_only_what_changed() {
    let scene = |x: f32| {
        let mut d = SceneData::default();
        d.nodes.push(cube(1.).name("a").position(x, 0., 0.));
        d.nodes.push(sphere(1.).name("b").child(cube(1.)));
        d
    };
    let (a, b) = (scene(0.).flatten(), scene(1.).flatten());
    assert_eq!(a.diff(&a), None);
    let Some(SceneUpdate::Diff(d)) = a.diff(&b) else {
        panic!()
    };
    assert_eq!(d.upsert.len(), 1);
    assert_eq!(d.upsert[0].key, "#a");
    assert!(d.remove.is_empty() && d.order.is_none() && d.settings.is_none());
    assert_eq!(
        SceneUpdate::Diff(d).to_json(),
        r##"{"v":2,"upsert":[{"key":"#a","name":"a","mesh":{"type":"box","size":[1.0,1.0,1.0]},"pos":[1.0,0.0,0.0]}]}"##
    );
}

/// A small deterministic random generator (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
    fn f(&mut self) -> f32 {
        (self.below(2000) as f32 - 1000.) / 100.
    }
}

fn random_node(r: &mut Rng, depth: u32) -> Node3d {
    let mut n = match r.below(4) {
        0 => group(),
        1 => cube(1. + r.below(3) as f32),
        2 => sphere(0.5),
        _ => torus(1., 0.2),
    };
    if r.chance(40) {
        n = n.name(format!("n{}", r.below(8)));
    }
    if r.chance(20) {
        n = n.label(format!("l{}", r.below(4)));
    }
    n = n.position(r.f(), r.f(), r.f());
    if depth < 3 {
        for _ in 0..r.below(4) {
            n = n.child(random_node(r, depth + 1));
        }
    }
    n
}

/// Some random edit somewhere in `nodes`.
fn edit(r: &mut Rng, nodes: &mut Vec<Node3d>) {
    let k = r.below(nodes.len() as u64 + 1) as usize;
    if k == nodes.len() || r.chance(10) {
        nodes.insert(r.below(nodes.len() as u64 + 1) as usize, random_node(r, 1));
        return;
    }
    let n = &mut nodes[k];
    match r.below(9) {
        0 => n.position[0] = r.f(),
        1 => n.material.color = rgb(r.below(0xffffff) as u32).into(),
        2 => n.hidden = !n.hidden,
        3 => n.name = r.chance(50).then(|| format!("n{}", r.below(8))),
        4 => {
            nodes.remove(k);
        }
        5 => {
            let moved = nodes.remove(k);
            let to = r.below(nodes.len() as u64 + 1) as usize;
            nodes.insert(to, moved);
        }
        6 => n.quaternion = Some(math::quat_normalize([r.f(), r.f(), r.f(), 1.])),
        7 => n.spin[1] = r.f(),
        _ => edit(r, &mut n.children),
    }
}

#[test]
fn random_edits_diff_and_apply_back() {
    let mut r = Rng(0x9e3779b97f4a7c15);
    for _ in 0..200 {
        let mut old = SceneData::default();
        for _ in 0..r.below(5) {
            old.nodes.push(random_node(&mut r, 0));
        }
        let mut new = old.clone();
        for _ in 0..1 + r.below(6) {
            edit(&mut r, &mut new.nodes);
        }
        if r.chance(20) {
            new.camera.position[0] = r.f();
        }
        let (a, b) = (old.flatten(), new.flatten());
        let mut applied = a.clone();
        if let Some(u) = a.diff(&b) {
            applied.apply(&u);
            // The same through JSON (the colors' 4 decimals aside: compare JSON).
            let mut through = a.clone();
            through.apply(&SceneUpdate::from_json(&u.to_json()).expect("readable"));
            assert_eq!(
                SceneUpdate::Full(through).to_json(),
                SceneUpdate::Full(b.clone()).to_json()
            );
        }
        assert_eq!(applied, b);
        // A player fed the diff draws the same as one fed the whole scene.
        let mut p = player(&old, 0.);
        show(&mut p, &old, &new, 0.);
        let q = player(&new, 0.);
        let (f, g) = (p.frame(100., 100., false), q.frame(100., 100., false));
        let keys = |f: &player::Frame| {
            f.draws
                .iter()
                .map(|d| (d.key.to_string(), d.world))
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(&f), keys(&g));
    }
}

// --- The reconciler ------------------------------------------------------------------------

#[test]
fn the_reconciler_sends_the_scene_then_diffs() {
    use crate::{AppInit, Context, Patch, Render, Runtime, UiEvent};
    use std::sync::Arc;

    #[derive(Default)]
    struct View {
        x: f32,
    }
    impl Render for View {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            scene3d()
                .child(cube(1.).name("cube").position(self.x, 0., 0.))
                .child(sphere(1.).name("ball"))
                .on_tap(cx.listener(|this, _: &SceneTapEvent, _| this.x += 1.))
        }
    }
    let init: AppInit = Arc::new(|cx: &mut crate::App| {
        let root = cx.new_entity(|_| View::default());
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    let scene = |patches: &[Patch]| {
        patches
            .iter()
            .filter_map(|p| match p {
                Patch::SetScene { id, update } => Some((*id, update.clone())),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let first = scene(&rt.render());
    assert_eq!(first.len(), 1);
    let (id, SceneUpdate::Full(full)) = &first[0] else {
        panic!("the first update is full")
    };
    assert!(full.settings.tap);
    assert_eq!(full.nodes.len(), 2);
    let next = scene(&rt.dispatch(UiEvent::SceneTap(*id, SceneTapEvent::default())));
    let [(_, SceneUpdate::Diff(d))] = next.as_slice() else {
        panic!("a diff: {next:?}")
    };
    assert_eq!(
        d.upsert.iter().map(|n| n.key.as_str()).collect::<Vec<_>>(),
        ["#cube"]
    );
    assert!(d.settings.is_none() && d.order.is_none() && d.remove.is_empty());
    // Nothing changed: nothing sent.
    assert!(scene(&rt.render()).is_empty());
    let mut rt2 = Runtime::new(&init);
    let full2 = scene(&rt2.render());
    assert!(
        matches!(full2[0].1, SceneUpdate::Full(_)),
        "a new runtime (a reconnect) starts full"
    );
}

#[test]
fn taps_reach_the_handler() {
    use crate::{AppInit, Context, Patch, Render, Runtime, UiEvent};
    use std::sync::{Arc, Mutex};

    static SEEN: Mutex<Option<SceneTapEvent>> = Mutex::new(None);
    struct View;
    impl Render for View {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            scene3d()
                .child(cube(1.).name("cube"))
                .on_tap(std::rc::Rc::new(|e: &SceneTapEvent, _: &mut crate::App| {
                    *SEEN.lock().unwrap() = Some(e.clone())
                }))
        }
    }
    let init: AppInit = Arc::new(|cx: &mut crate::App| {
        let root = cx.new_entity(|_| View);
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    let patches = rt.render();
    let id = patches
        .iter()
        .find_map(|p| match p {
            Patch::Create {
                id,
                kind: NodeKind::Scene3d,
            } => Some(*id),
            _ => None,
        })
        .expect("a scene");
    let event =
        SceneTapEvent::decode(r#"{"node":"cube","part":"lid","instance":3,"point":[0,1,0]}"#);
    assert_eq!(event.instance, Some(3));
    rt.dispatch(UiEvent::SceneTap(id, event.clone()));
    assert_eq!(SEEN.lock().unwrap().as_ref(), Some(&event));
}

// --- The player: v1 behavior ---------------------------------------------------------------

/// Far enough to see x from −15 to 15 (nothing is culled).
fn far_camera() -> Camera {
    Camera::perspective(45.).position(0., 0., 40.)
}

fn named_scene(x: f32) -> SceneData {
    let mut d = SceneData {
        camera: far_camera(),
        ..Default::default()
    };
    d.nodes.push(cube(1.).name("a").position(x, 0., 0.));
    d.transition = Some(Transition::linear(1000).into());
    d
}

#[test]
fn changes_animate_with_the_scene_transition() {
    let mut p = player(&named_scene(0.), 0.);
    show(&mut p, &named_scene(0.), &named_scene(10.), 1.);
    p.advance(1.5);
    let f = p.frame(100., 100., false);
    let x = f.draws[0].world[12];
    assert!((x - 5.).abs() < 1e-3, "halfway: {x}");
    assert!(f.animating);
    p.advance(2.5);
    let f = p.frame(100., 100., false);
    assert_eq!(f.draws[0].world[12], 10.);
    assert!(!f.animating);
}

#[test]
fn a_change_mid_transition_starts_from_where_the_node_is() {
    let mut p = player(&named_scene(0.), 0.);
    show(&mut p, &named_scene(0.), &named_scene(10.), 0.);
    p.advance(0.5);
    show(&mut p, &named_scene(10.), &named_scene(0.), 0.5);
    p.advance(0.5);
    let x = p.frame(1., 1., false).draws[0].world[12];
    assert!((x - 5.).abs() < 1e-3, "{x}");
}

#[test]
fn rotations_transition_by_degrees_or_by_the_shorter_arc() {
    let scene = |n: Node3d| {
        let mut d = SceneData::default();
        d.nodes.push(n.name("a"));
        d.transition = Some(Transition::linear(1000).into());
        d
    };
    // Euler to Euler: every degree in between (a full turn is a half turn halfway).
    let (a, b) = (scene(cube(1.)), scene(cube(1.).rotation(0., 360., 0.)));
    let mut p = player(&a, 0.);
    show(&mut p, &a, &b, 0.);
    p.advance(0.5);
    let z = math::transform_dir(&p.frame(1., 1., false).draws[0].world, [0., 0., 1.]);
    assert!(near(z[2], -1.), "{z:?}");
    // With a quaternion at either end: slerp (a full turn is no turn).
    let (a, b) = (
        scene(cube(1.)),
        scene(cube(1.).quaternion(
            0.,
            std::f32::consts::FRAC_1_SQRT_2,
            0.,
            std::f32::consts::FRAC_1_SQRT_2,
        )),
    );
    let mut p = player(&a, 0.);
    show(&mut p, &a, &b, 0.);
    p.advance(0.5);
    let z = math::transform_dir(&p.frame(1., 1., false).draws[0].world, [0., 0., 1.]);
    let s = (45f32).to_radians();
    assert!(
        near(z[0], s.sin()) && near(z[2], s.cos()),
        "45° halfway: {z:?}"
    );
}

#[test]
fn spin_turns_from_when_the_node_appeared() {
    let mut d = SceneData::default();
    d.nodes.push(cube(1.).name("a").spin(0., 90., 0.));
    let mut p = player(&d, 10.);
    p.advance(11.);
    let f = p.frame(1., 1., false);
    assert!(f.animating);
    // A quarter turn around Y: +Z went to +X.
    let z = math::transform_dir(&f.draws[0].world, [0., 0., 1.]);
    assert!((z[0] - 1.).abs() < 1e-4, "{z:?}");
    // A new scene keeps the phase.
    p.apply(&SceneUpdate::full(&d), 11.);
    let f = p.frame(1., 1., false);
    let z = math::transform_dir(&f.draws[0].world, [0., 0., 1.]);
    assert!((z[0] - 1.).abs() < 1e-4, "{z:?}");
}

#[test]
fn dragging_orbits_the_camera_and_zoom_is_clamped() {
    let d = SceneData {
        camera: Camera::perspective(45.).position(0., 0., 4.),
        orbit: Some(Orbit::new()),
        ..Default::default()
    };
    let mut p = player(&d, 0.);
    p.advance(0.);
    let eye = p.frame(1., 1., false).camera_position;
    assert!((eye[2] - 4.).abs() < 1e-4);
    // Dragging right by 180 points: the camera goes 90° to the left (−X).
    p.drag(180., 0.);
    let eye = p.frame(1., 1., false).camera_position;
    assert!((eye[0] + 4.).abs() < 1e-3, "{eye:?}");
    p.drag(0., 10000.);
    let eye = p.frame(1., 1., false).camera_position;
    assert!(
        eye[1] < 4. && eye[1] > 3.99,
        "pitch stops short of straight up: {eye:?}"
    );
    p.zoom(100.);
    let eye = p.frame(1., 1., false).camera_position;
    assert!(
        (math::length(eye) - 1.).abs() < 1e-3,
        "a quarter of 4: {eye:?}"
    );
}

#[test]
fn without_orbit_gestures_do_nothing() {
    let mut p = player(&SceneData::default(), 0.);
    let before = p.frame(1., 1., false).camera_position;
    p.drag(100., 100.);
    p.zoom(2.);
    assert_eq!(p.frame(1., 1., false).camera_position, before);
}

#[test]
fn auto_rotation_pauses_while_dragging() {
    let d = SceneData {
        orbit: Some(Orbit::auto_only(90.)),
        ..Default::default()
    };
    let mut p = player(&d, 0.);
    p.advance(0.);
    p.advance(0.2);
    let a = p.frame(1., 1., false).camera_position;
    p.pointer_down();
    p.advance(0.4);
    assert_eq!(p.frame(1., 1., false).camera_position, a);
    p.pointer_up();
    p.advance(0.6);
    assert_ne!(p.frame(1., 1., false).camera_position, a);
}

#[test]
fn taps_hit_the_nearest_named_node_and_its_part() {
    let mut d = SceneData {
        camera: Camera::perspective(45.).position(0., 0., 5.),
        ..Default::default()
    };
    d.nodes.push(
        group()
            .name("robot")
            .label("body")
            .child(group().label("arm").child(cube(1.).position(0., 0., 1.))) // in front
            .child(cube(1.).name("back").position(0., 0., -2.)),
    );
    d.nodes.push(sphere(0.5).position(1.5, 0., 0.));
    let p = player(&d, 0.);
    let hit = p.hit(50., 50., 100., 100., false);
    assert_eq!(hit.node.as_deref(), Some("robot"));
    assert_eq!(
        hit.part.as_deref(),
        Some("arm"),
        "the nearest label below the named node"
    );
    assert_eq!(hit.instance, None);
    let point = hit.point.unwrap();
    assert!(
        (point[2] - 1.5).abs() < 1e-3,
        "the front face of the front cube: {point:?}"
    );
    // The unnamed sphere: hit, but nothing named.
    let x = 50. + 50. * 1.5 / (5. * (22.5f32).to_radians().tan());
    let hit = p.hit(x, 50., 100., 100., false);
    assert!(
        hit.point.is_some() && hit.node.is_none() && hit.part.is_none(),
        "{hit:?}"
    );
    assert_eq!(p.hit(1., 1., 100., 100., false), SceneTapEvent::default());
}

#[test]
fn transparent_meshes_draw_last_back_to_front() {
    let mut d = SceneData::default();
    d.nodes.push(
        cube(1.)
            .name("near")
            .color(crate::rgba(0xffffff, 0.5))
            .position(0., 0., 2.),
    );
    d.nodes.push(
        cube(1.)
            .name("far")
            .color(crate::rgba(0xffffff, 0.5))
            .position(0., 0., -2.),
    );
    d.nodes.push(cube(1.).name("solid"));
    d.nodes.push(
        cube(1.)
            .name("masked")
            .color(crate::rgba(0xffffff, 0.5))
            .alpha_mask(0.3),
    );
    assert_eq!(names(&player(&d, 0.)), ["solid", "masked", "far", "near"]);
}

#[test]
fn opaque_draws_group_by_program_texture_and_mesh() {
    let mut d = SceneData::default();
    d.nodes.push(sphere(1.).name("s1"));
    d.nodes.push(cube(1.).name("c1").texture("b.png"));
    d.nodes.push(cube(1.).name("c2"));
    d.nodes.push(sphere(1.).name("s2").texture("a.png"));
    d.nodes.push(
        cube(1.)
            .name("i")
            .instances(&Instances::new().push([0.; 3], [0.; 3], 1.)),
    );
    d.nodes.push(sphere(1.).name("s3"));
    assert_eq!(names(&player(&d, 0.)), ["c2", "s1", "s3", "s2", "c1", "i"]);
}

#[test]
fn default_lights_and_adaptive_background() {
    let d = SceneData {
        background: Some(crate::Color::adaptive(rgb(0xffffff), rgb(0x000000)).into()),
        ..Default::default()
    };
    let p = player(&d, 0.);
    let light = p.frame(1., 1., false);
    assert_eq!(light.clear, [1., 1., 1., 1.]);
    assert_eq!(p.frame(1., 1., true).clear, [0., 0., 0., 1.]);
    assert_eq!(light.lights.len(), 1);
    assert!(light.ambient[0] > 0.);
    assert_eq!(light.exposure, 1.);
    assert!(light.env.is_none() && light.shadow.is_none());
    // An environment lights the scene instead of the default rig.
    let lit = SceneData {
        environment: Some(Environment::neutral().intensity(0.5).background(true)),
        ..Default::default()
    };
    let f = player(&lit, 0.).frame(1., 1., false).lights.len();
    assert_eq!(f, 0);
    let p = player(&lit, 0.);
    let env = p.frame(1., 1., false).env.unwrap();
    assert_eq!(
        (env.intensity, env.background, env.id.len()),
        (0.5, true, 32)
    );
}

#[test]
fn easing_matches_the_style_curves() {
    for t in [
        Transition::ease(100),
        Transition::spring(100),
        Transition::linear(100),
        Transition::ease_out(100),
    ] {
        let s: SceneTransition = t.into();
        for p in [0.1, 0.5, 0.9] {
            assert!((ease(s.bezier, p) - t.ease_at(p)).abs() < 1e-5);
        }
    }
}

// --- The player: v2 ------------------------------------------------------------------------

#[test]
fn nodes_outside_the_view_are_culled() {
    let mut d = SceneData {
        camera: Camera::perspective(45.).position(0., 0., 5.),
        ..Default::default()
    };
    d.nodes.push(cube(1.).name("seen"));
    d.nodes.push(cube(1.).name("behind").position(0., 0., 20.));
    d.nodes.push(cube(1.).name("aside").position(50., 0., 0.));
    // Its instances reach into the view.
    d.nodes.push(
        cube(1.)
            .name("crowd")
            .position(50., 0., 0.)
            .instances(&Instances::new().push([-50., 1., 0.], [0.; 3], 1.)),
    );
    let p = player(&d, 0.);
    assert_eq!(names(&p), ["seen", "crowd"]);
    // Hidden subtrees draw nothing.
    let mut h = d.clone();
    h.nodes[0].hidden = true;
    assert_eq!(names(&player(&h, 0.)), ["crowd"]);
}

#[test]
fn instances_are_hit_one_by_one() {
    let mut d = SceneData {
        camera: Camera::perspective(45.).position(0., 0., 10.),
        tap: true,
        ..Default::default()
    };
    let set = Instances::new()
        .push([-3., 0., 0.], [0.; 3], 1.)
        .push([0., 0., 0.], [0., 45., 0.], 1.)
        .color(rgb(0xff0000))
        .push([3., 0., 0.], [0.; 3], 2.);
    d.nodes.push(
        group()
            .name("crowd")
            .child(cube(1.).label("box").instances(&set)),
    );
    let p = player(&d, 0.);
    let f = p.frame(100., 100., false);
    assert_eq!(f.draws.len(), 1);
    let inst = f.draws[0].instances.unwrap();
    assert_eq!(inst.count, 3);
    assert!(f.draws[0].variant().instanced);
    let bounds = f.draws[0].bounds;
    assert!(near(bounds[0], -3.5) && near(bounds[3], 4.), "{bounds:?}");
    let hit = p.hit(50., 50., 100., 100., false);
    assert_eq!(
        (hit.node.as_deref(), hit.part.as_deref(), hit.instance),
        (Some("crowd"), Some("box"), Some(1))
    );
    // The big one on the right.
    let x = 50. + 50. * 3. / (9. * (22.5f32).to_radians().tan());
    assert_eq!(p.hit(x, 50., 100., 100., false).instance, Some(2));
    assert_eq!(p.hit(50., 5., 100., 100., false).instance, None);
    // The color went linear into the blob.
    let (bytes, _) = blobs::get(inst.id).unwrap();
    let decoded = blobs::decode_instances(&bytes).unwrap();
    assert_eq!(decoded.colors[1], [1., 0., 0., 1.]);
    assert_eq!(decoded.colors[0], [1.; 4], "white where no color was given");
}

#[test]
fn shadows_fit_the_visible_draws() {
    let mut d = SceneData {
        camera: Camera::perspective(45.).position(0., 3., 8.),
        orbit: Some(Orbit::new()),
        ..Default::default()
    };
    d.lights.push(Light::ambient(rgb(0xffffff), 0.2));
    d.lights.push(Light::point(rgb(0xffffff), 1.));
    d.lights.push(
        Light::directional(rgb(0xffffff), 1.)
            .direction(-1., -2., -1.)
            .shadows(true),
    );
    d.nodes
        .push(plane(10., 10.).name("floor").cast_shadows(false));
    d.nodes.push(cube(1.).name("box").position(1., 0.5, 0.));
    d.nodes.push(sphere(0.5).name("ball").position(-2., 1., 1.));
    let mut p = player(&d, 0.);
    let f = p.frame(200., 100., false);
    let s = f.shadow.clone().expect("a shadow pass");
    assert_eq!(s.light, 1, "the directional light, after the point light");
    assert_eq!(s.map_size, SHADOW_MAP_SIZE);
    let casters: Vec<_> = s
        .casters
        .iter()
        .map(|i| f.draws[*i].name.unwrap())
        .collect();
    assert_eq!(casters.len(), 2);
    assert!(!casters.contains(&"floor"));
    // Every visible draw is inside the light's clip volume.
    for dr in &f.draws {
        let b = dr.bounds;
        for c in 0..8 {
            let corner = [
                b[if c & 1 == 0 { 0 } else { 3 }],
                b[if c & 2 == 0 { 1 } else { 4 }],
                b[if c & 4 == 0 { 2 } else { 5 }],
            ];
            let q = math::transform_point(&s.view_proj, corner);
            assert!(q.iter().all(|v| v.abs() <= 1.0001), "{corner:?} → {q:?}");
        }
    }
    assert!(s.bias > 0. && s.normal_bias > 0.);
    let visible = f.draws.len();
    // Turning the camera (everything still in view): the same map.
    p.drag(10., 0.);
    let g = p.frame(200., 100., false);
    if g.draws.len() == visible {
        assert_eq!(g.shadow.unwrap().view_proj, s.view_proj);
    }
    // No light with shadows: no pass.
    d.lights.pop();
    assert!(player(&d, 0.).frame(1., 1., false).shadow.is_none());
}

#[test]
fn clips_sample_linear_step_and_cubic_keys() {
    let lin = ClipChannel {
        target: String::new(),
        property: ClipProperty::Translation,
        interpolation: Interp::Linear,
        times: vec![0., 1., 3.],
        values: vec![0., 0., 0., 10., 0., 0., 10., 20., 0.],
    };
    assert_eq!(lin.sample(-1.)[0], 0.);
    assert_eq!(lin.sample(0.5)[0], 5.);
    assert_eq!(lin.sample(2.)[1], 10.);
    assert_eq!(lin.sample(9.)[1], 20.);
    let step = ClipChannel {
        interpolation: Interp::Step,
        ..lin.clone()
    };
    assert_eq!(step.sample(0.99)[0], 0.);
    assert_eq!(step.sample(1.)[0], 10.);
    // glTF cubic spline: keys (in, value, out), hand computed at s = 0.25:
    // (2s³−3s²+1)·0 + (s³−2s²+s)·dt·2 + (−2s³+3s²)·10 + (s³−s²)·dt·(−4), dt = 2.
    let cubic = ClipChannel {
        target: String::new(),
        property: ClipProperty::Scale,
        interpolation: Interp::CubicSpline,
        times: vec![1., 3.],
        values: vec![
            0., 0., 0., /**/ 0., 0., 0., /**/ 2., 0., 0., // key 0
            -4., 0., 0., /**/ 10., 0., 0., /**/ 0., 0., 0., // key 1
        ],
    };
    let s: f32 = 0.25;
    let expected = (s.powi(3) - 2. * s * s + s) * 2. * 2.
        + (-2. * s.powi(3) + 3. * s * s) * 10.
        + (s.powi(3) - s * s) * 2. * -4.;
    assert!(
        near(cubic.sample(1.5)[0], expected),
        "{} vs {expected}",
        cubic.sample(1.5)[0]
    );
    assert_eq!(cubic.sample(3.)[0], 10.);
    // Rotations slerp and stay unit length.
    let rot = ClipChannel {
        target: String::new(),
        property: ClipProperty::Rotation,
        interpolation: Interp::Linear,
        times: vec![0., 1.],
        values: vec![0., 0., 0., 1., 0., 1., 0., 0.],
    };
    let q = rot.sample(0.5);
    assert!(
        near_mat(&math::quat_to_mat4(q), &math::rotation_y(90.)),
        "{q:?}"
    );
    // Builder: flat tangents ease in and out.
    let clip = AnimationClip::new("ease")
        .translation("", &[0., 1.], &[[0.; 3], [1., 0., 0.]])
        .interpolation(Interp::CubicSpline);
    let c = &clip.data().channels[0];
    assert_eq!(c.values.len(), 18);
    assert!(near(c.sample(0.25)[0], 0.15625));
    assert!(near(c.sample(0.5)[0], 0.5));
    assert_eq!(clip.duration(), 1.);
    // Back to linear: the tangents go.
    let back = clip.interpolation(Interp::Linear);
    assert!(near(back.data().channels[0].sample(0.25)[0], 0.25));
}

fn clip_scene(clip: Option<(AnimationClip, Playback)>) -> SceneData {
    let mut d = SceneData {
        camera: far_camera(),
        ..Default::default()
    };
    let mut n = group()
        .name("rig")
        .child(cube(1.).name("box").child(sphere(0.2)));
    if let Some((c, pb)) = clip {
        n = n.play(c, pb);
    }
    d.nodes.push(n);
    d
}

fn x_of(p: &ScenePlayer, name: &str) -> f32 {
    p.frame(1., 1., false)
        .draws
        .iter()
        .find(|d| d.key == name)
        .map(|d| d.world[12])
        .unwrap()
}

#[test]
fn clips_play_on_their_targets_and_hold_or_loop() {
    let slide = AnimationClip::new("slide")
        .translation("0", &[0., 2.], &[[0.; 3], [4., 0., 0.]])
        .translation("#box/0", &[0., 2.], &[[0.; 3], [0., 0., 0.]]);
    let d = clip_scene(Some((slide.clone(), Playback::once())));
    let mut p = player(&d, 10.);
    p.advance(11.);
    assert!(near(x_of(&p, "#box"), 2.));
    assert!(p.frame(1., 1., false).animating);
    p.advance(13.);
    assert!(near(x_of(&p, "#box"), 4.), "holds the last pose");
    assert!(!p.frame(1., 1., false).animating);
    // An update that changes nothing about the clip keeps it where it is.
    let mut moved = d.clone();
    moved.camera.position[0] = 1.;
    show(&mut p, &d, &moved, 13.5);
    assert!(near(x_of(&p, "#box"), 4.));
    // Looping at double speed: 2.5 s in is 1 s into the second loop.
    let looping = clip_scene(Some((slide.clone(), Playback::looping().speed(2.))));
    let mut p = player(&looping, 0.);
    p.advance(2.5);
    assert!(near(x_of(&p, "#box"), 2.), "{}", x_of(&p, "#box"));
    assert!(p.frame(1., 1., false).animating);
    // Stopping: back to the node's own pose.
    show(&mut p, &looping, &clip_scene(None), 2.6);
    assert!(near(x_of(&p, "#box"), 0.));
    // Restarting with `restart(n)`.
    let again = clip_scene(Some((
        slide.clone(),
        Playback::looping().speed(2.).restart(1),
    )));
    show(&mut p, &clip_scene(None), &looping, 3.);
    show(&mut p, &looping, &again, 3.5);
    p.advance(4.);
    assert!(
        near(x_of(&p, "#box"), 2.),
        "restarted at 3.5: {}",
        x_of(&p, "#box")
    );
}

#[test]
fn clips_crossfade() {
    let at = |x: f32| AnimationClip::new(format!("at {x}")).translation("0", &[0.], &[[x, 0., 0.]]);
    let a = clip_scene(Some((at(0.), Playback::looping())));
    let b = clip_scene(Some((at(10.), Playback::looping().fade(1.))));
    let mut p = player(&a, 0.);
    show(&mut p, &a, &b, 1.);
    p.advance(1.5);
    assert!(near(x_of(&p, "#box"), 5.), "{}", x_of(&p, "#box"));
    assert!(p.frame(1., 1., false).animating, "fading");
    p.advance(2.5);
    assert!(near(x_of(&p, "#box"), 10.));
    // Without a fade: at once.
    let c = clip_scene(Some((at(3.), Playback::looping())));
    show(&mut p, &b, &c, 3.);
    assert!(near(x_of(&p, "#box"), 3.));
}

/// A skinned triangle under a bone, as glTF would have it.
fn skinned() -> Imported {
    let node = |name: &str, t: [f32; 3], children: Vec<usize>| ImportedNode {
        name: Some(name.into()),
        translation: t,
        rotation: [0., 0., 0., 1.],
        scale: [1.; 3],
        children,
        mesh: None,
        skin: None,
    };
    let mut body = node("body", [1., 0., 0.], vec![1]);
    body.mesh = Some(0);
    body.skin = Some(0);
    Imported {
        roots: vec![0, 2],
        nodes: vec![
            body,
            node("bone", [0., 2., 0.], vec![]),
            node("lamp", [0., 0., -1.], vec![]),
        ],
        meshes: vec![ImportedMesh {
            name: None,
            primitives: vec![ImportedPrimitive {
                positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
                normals: vec![[0., 0., 1.]; 3],
                uvs: vec![[0.; 2]; 3],
                colors: vec![],
                joints: vec![[0; 4]; 3],
                weights: vec![[1., 0., 0., 0.]; 3],
                indices: vec![0, 1, 2],
                material: Some(0),
            }],
        }],
        materials: vec![ImportedMaterial {
            name: None,
            base_color: [0.21404114, 1., 0., 0.5],
            base_texture: Some(0),
            metallic: 0.25,
            roughness: 0.75,
            mr_texture: None,
            normal_texture: Some(0),
            normal_scale: 0.5,
            emissive: [0.; 3],
            emissive_texture: None,
            occlusion_texture: None,
            alpha: ImportedAlpha::Mask(0.4),
            double_sided: true,
            unlit: false,
        }],
        images: vec![ImportedImage {
            mime: "image/png".into(),
            bytes: vec![0x89, b'P', b'N', b'G'],
        }],
        skins: vec![ImportedSkin {
            joints: vec![1],
            inverse_bind: vec![math::translation([0., -2., 0.])],
        }],
        animations: vec![ImportedAnimation {
            name: Some("lift".into()),
            channels: vec![ImportedChannel {
                node: 1,
                path: ImportedPath::Translation,
                interpolation: ImportedInterpolation::Linear,
                times: vec![0., 1.],
                values: vec![0., 2., 0., 0., 3., 0.],
            }],
        }],
    }
}

#[test]
fn models_become_nodes_blobs_and_clips() {
    let model = Model::from_imported(&skinned());
    let root = model.node();
    assert!(root.name.is_none());
    assert_eq!(root.children.len(), 2);
    let body = &root.children[0];
    assert_eq!(body.label.as_deref(), Some("body"));
    assert_eq!(body.position, [1., 0., 0.]);
    assert_eq!(body.children[0].label.as_deref(), Some("bone"));
    let prim = &body.children[1];
    let m = &prim.material;
    assert!(m.texture.as_deref().unwrap().starts_with("p3d:"));
    assert_eq!(m.texture, m.normal_map);
    assert_eq!(
        m.embedded.len(),
        1,
        "the image stays alive with the material"
    );
    assert!(blobs::get(&m.texture.as_deref().unwrap()[4..]).is_some());
    assert!(
        (m.color.light[0] - 0.5).abs() < 1e-3,
        "linear to sRGB: {:?}",
        m.color
    );
    assert_eq!(m.color.light[3], 0.5);
    assert_eq!(
        (m.alpha, m.cutoff, m.double_sided, m.normal_scale),
        (AlphaMode::Mask, 0.4, true, 0.5)
    );
    assert_eq!(prim.skin.as_ref().map(|s| s.up), Some(2));
    let data = prim.mesh.as_ref().unwrap().data().unwrap();
    assert_eq!(data.joints.len(), 3);
    assert_eq!(model.bounds(), [[1., 0., 0.], [2., 1., 0.]]);
    let clip = model.clip("lift").unwrap();
    assert_eq!(clip.data().channels[0].target, "0/0");
    assert_eq!(model.clips().len(), 1);
    assert!(model.clip("walk").is_none());
    // Flattened: the skin's root is the model's (named) node.
    let mut d = SceneData::default();
    d.nodes
        .push(group().name("stage").child(model.node().name("robot")));
    let flat = d.flatten();
    let skin = flat.nodes.iter().find_map(|n| n.skin.clone()).unwrap();
    assert_eq!(skin.root, "#robot");
}

#[test]
fn skins_compute_joint_matrices() {
    let model = Model::from_imported(&skinned());
    let scene = |play: bool| {
        let mut d = SceneData {
            camera: far_camera(),
            ..Default::default()
        };
        let mut robot = model.node().name("robot").position(5., 0., 0.);
        if play {
            robot = robot.play(model.clip("lift").unwrap(), Playback::once());
        }
        d.nodes.push(robot);
        d.tap = true;
        d
    };
    let mut p = player(&scene(false), 0.);
    let f = p.frame(1., 1., false);
    let draw = f
        .draws
        .iter()
        .find(|d| d.joints.is_some())
        .expect("a skinned draw");
    assert!(draw.variant().skinned);
    // At rest: inverse(mesh world) · bone world · inverse bind = identity.
    assert!(
        near_mat(&draw.joints.unwrap()[0], &math::IDENTITY),
        "{:?}",
        draw.joints
    );
    // The clip lifts the bone by 1: the joint moves the skin up by 1.
    show(&mut p, &scene(false), &scene(true), 0.);
    p.advance(1.);
    let f = p.frame(1., 1., false);
    let draw = f.draws.iter().find(|d| d.joints.is_some()).unwrap();
    assert!(
        near_mat(&draw.joints.unwrap()[0], &math::translation([0., 1., 0.])),
        "{:?}",
        draw.joints
    );
    // Its culling box: where the joint puts the mesh (lifted by 1).
    let b = draw.bounds;
    assert!(
        near(b[0], 6.) && near(b[3], 7.) && near(b[1], 1.) && near(b[4], 2.),
        "{b:?}"
    );
}

/// A Mixamo-style rig: the armature is scaled to 1/100 (centimeters), the
/// mesh node once more, and the hips joint (5 m to the side) places the
/// skin: the mesh node's own transform places nothing.
fn centimeter_rig() -> Imported {
    let node = |name: &str, t: [f32; 3], s: f32, children: Vec<usize>| ImportedNode {
        name: Some(name.into()),
        translation: t,
        rotation: [0., 0., 0., 1.],
        scale: [s; 3],
        children,
        mesh: None,
        skin: None,
    };
    let mut body = node("body", [0.; 3], 0.01, vec![]);
    body.mesh = Some(0);
    body.skin = Some(0);
    Imported {
        roots: vec![0],
        nodes: vec![
            node("Armature", [0.; 3], 0.01, vec![1, 2]),
            body,
            node("Hips", [500., 0., 0.], 1., vec![]),
        ],
        meshes: vec![ImportedMesh {
            name: None,
            // A 40 × 188 cm quad facing +Z.
            primitives: vec![ImportedPrimitive {
                positions: vec![
                    [-20., 0., 0.],
                    [20., 0., 0.],
                    [20., 188., 0.],
                    [-20., 188., 0.],
                ],
                normals: vec![[0., 0., 1.]; 4],
                joints: vec![[0; 4]; 4],
                weights: vec![[1., 0., 0., 0.]; 4],
                indices: vec![0, 1, 2, 0, 2, 3],
                ..Default::default()
            }],
        }],
        skins: vec![ImportedSkin {
            joints: vec![2],
            inverse_bind: vec![math::IDENTITY],
        }],
        animations: vec![ImportedAnimation {
            name: Some("rise".into()),
            channels: vec![ImportedChannel {
                node: 2,
                path: ImportedPath::Translation,
                interpolation: ImportedInterpolation::Linear,
                times: vec![0., 1.],
                values: vec![500., 0., 0., 500., 100., 0.],
            }],
        }],
        ..Default::default()
    }
}

#[test]
fn skinned_meshes_are_placed_by_their_joints() {
    let model = Model::from_imported(&centimeter_rig());
    let [lo, hi] = model.bounds();
    let ok = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3);
    assert!(
        ok(lo, [4.8, 0., 0.]) && ok(hi, [5.2, 1.88, 0.]),
        "{lo:?} {hi:?}"
    );

    let look_at = |y: f32| {
        Camera::perspective(45.)
            .position(5., y, 4.)
            .target(5., y, 0.)
    };
    let mut d = SceneData {
        camera: look_at(0.94),
        tap: true,
        ..Default::default()
    };
    d.nodes.push(model.node().name("soldier"));
    let mut p = player(&d, 0.);
    let f = p.frame(100., 100., false);
    // Not culled (its node, at the origin and 1.9 cm tall, is out of view).
    let draw = f.draws.iter().find(|d| d.joints.is_some()).expect("drawn");
    let b = draw.bounds;
    assert!(
        near(b[0], 4.8) && near(b[3], 5.2) && near(b[1], 0.) && near(b[4], 1.88),
        "{b:?}"
    );
    let hit = p.hit(50., 50., 100., 100., false);
    assert_eq!(
        (hit.node.as_deref(), hit.part.as_deref()),
        (Some("soldier"), Some("body"))
    );
    let point = hit.point.unwrap();
    assert!(
        near(point[0], 5.) && near(point[1], 0.94) && near(point[2], 0.),
        "{point:?}"
    );

    // Playing: the joint rises 1 m; the box, culling and hits follow it.
    let playing = SceneData {
        camera: look_at(1.94),
        nodes: vec![
            model
                .node()
                .name("soldier")
                .play(model.clip("rise").unwrap(), Playback::looping()),
        ],
        ..d.clone()
    };
    show(&mut p, &d, &playing, 0.);
    p.advance(0.999);
    let f = p.frame(100., 100., false);
    let b = f.draws[0].bounds;
    assert!(
        (b[1] - 1.).abs() < 0.01 && (b[4] - 2.88).abs() < 0.01,
        "{b:?}"
    );
    let hit = p.hit(50., 50., 100., 100., false);
    assert_eq!(hit.node.as_deref(), Some("soldier"));
    assert!((hit.point.unwrap()[1] - 1.94).abs() < 0.01, "{hit:?}");
}

#[test]
fn model_parts_are_reported_by_taps() {
    let model = Model::from_imported(&skinned());
    let mut d = SceneData {
        camera: Camera::perspective(45.).position(1.25, 0.25, 4.),
        ..Default::default()
    };
    d.camera.target = [1.25, 0.25, 0.];
    d.nodes.push(model.node().name("robot"));
    let hit = player(&d, 0.).hit(50., 50., 100., 100., false);
    assert_eq!(
        (hit.node.as_deref(), hit.part.as_deref()),
        (Some("robot"), Some("body"))
    );
}

#[test]
fn reading_a_glb_never_panics() {
    // A minimal GLB: header, then a JSON chunk (padded with spaces).
    let json = br#"{"asset":{"version":"2.0"},"scenes":[{"nodes":[]}],"scene":0}"#;
    let pad = (4 - json.len() % 4) % 4;
    let chunk_len = json.len() + pad;
    let mut glb = Vec::new();
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&((12 + 8 + chunk_len) as u32).to_le_bytes());
    glb.extend_from_slice(&(chunk_len as u32).to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(json);
    glb.extend(std::iter::repeat_n(b' ', pad));
    // The parser may not be written yet: an error is fine, a panic is not.
    if let Ok(model) = Model::from_glb(&glb) {
        assert!(model.node().children.is_empty());
        assert!(model.clips().is_empty());
    }
    assert!(Model::from_glb(&[1, 2, 3]).is_err());
    assert!(Model::from_glb(&glb[..glb.len() / 2]).is_err());
}

#[test]
fn updates_without_their_blobs_do_not_panic() {
    // Blob ids this process never had (as a port would receive them).
    let id = "0123456789abcdef0123456789abcdef";
    let json = format!(
        r##"{{"v":2,"full":true,"settings":{{"env":{{"id":"{id}"}}}},"nodes":[
            {{"key":"#a","name":"a","mesh":{{"type":"blob","id":"{id}","bounds":[-1,-1,-1,1,1,1]}},
              "inst":{{"id":"{id}","count":3,"bounds":[-1,-1,-1,1,1,1]}},
              "play":{{"clip":"{id}","loop":true}},"skin":{{"id":"{id}","root":"#a"}}}}]}}"##
    );
    let u = SceneUpdate::from_json(&json).expect("readable");
    let mut p = ScenePlayer::new(0.);
    p.apply(&u, 0.);
    p.advance(1.);
    let f = p.frame(100., 100., false);
    assert_eq!(f.draws.len(), 1);
    assert!(f.draws[0].joints.is_none(), "no skin until it arrives");
    assert_eq!(p.geometry(&f.draws[0]).vertex_count(), 0);
    assert_eq!(p.hit(50., 50., 100., 100., false), SceneTapEvent::default());
}

// --- Shaders -------------------------------------------------------------------------------

#[test]
fn programs_are_assembled_one_way() {
    let v = Variant {
        instanced: true,
        skinned: true,
        depth_only: true,
    };
    let src = shaders::program_source(shaders::ES_HEADER, Stage::Vertex, v);
    assert!(src.starts_with("#version 300 es\n#define INSTANCED\n#define SKINNED\n#define DEPTH_ONLY\nprecision highp float;"));
    let plain = shaders::program_source(shaders::CORE_HEADER, Stage::Fragment, Variant::default());
    assert_eq!(plain, format!("#version 330 core\n{}", shaders::FRAGMENT));
}
