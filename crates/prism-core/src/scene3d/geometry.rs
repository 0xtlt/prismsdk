//! Triangles of the primitive meshes, the same on every OpenGL backend
//! (the web and Android clients port these functions line by line).
//!
//! Conventions: triangles are counter-clockwise seen from outside, normals
//! point out, texture coordinates have `(0, 0)` at the top-left of the image.

use super::Mesh;
use super::math::{Vec3, cross, normalize, sub};

/// Segments around a sphere, cylinder, cone or torus.
pub const SEGMENTS: u32 = 48;
/// Rings from pole to pole of a sphere, and around a torus' tube.
pub const RINGS: u32 = 24;

/// Vertex data ready for a GPU: flat `x, y, z` / `u, v` arrays and three
/// indices per triangle. Optional attributes (colors, skin) are empty when
/// absent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    /// `x, y, z` per vertex.
    pub positions: Vec<f32>,
    /// `x, y, z` per vertex, unit length.
    pub normals: Vec<f32>,
    /// `u, v` per vertex.
    pub uvs: Vec<f32>,
    /// Linear `r, g, b, a` per vertex.
    pub colors: Vec<f32>,
    /// Four joint indices per vertex.
    pub joints: Vec<u16>,
    /// Four joint weights per vertex.
    pub weights: Vec<f32>,
    /// Three indices per triangle.
    pub indices: Vec<u32>,
}

impl MeshData {
    fn vertex(&mut self, p: Vec3, n: Vec3, uv: [f32; 2]) -> u32 {
        let i = (self.positions.len() / 3) as u32;
        self.positions.extend(p);
        self.normals.extend(n);
        self.uvs.extend(uv);
        i
    }

    fn triangle(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend([a, b, c]);
    }

    /// Vertex `i`'s position.
    pub fn position(&self, i: u32) -> Vec3 {
        let i = i as usize * 3;
        [
            self.positions[i],
            self.positions[i + 1],
            self.positions[i + 2],
        ]
    }

    /// The number of vertices.
    pub fn vertex_count(&self) -> usize {
        self.positions.len() / 3
    }
}

/// The triangles of `mesh`.
pub fn generate(mesh: &Mesh) -> MeshData {
    match mesh {
        Mesh::Box { size } => cuboid(*size),
        Mesh::Sphere { radius } => sphere(*radius),
        Mesh::Plane { size } => plane(*size),
        Mesh::Cylinder { radius, height } => cylinder(*radius, *radius, *height),
        Mesh::Cone { radius, height } => cylinder(*radius, 0., *height),
        Mesh::Torus { radius, tube } => torus(*radius, *tube),
        Mesh::Blob { id, .. } => id
            .bytes()
            .and_then(|b| super::blobs::decode_mesh(&b))
            .map(|m| MeshData {
                uvs: if m.uvs.is_empty() {
                    vec![0.; m.positions.len() * 2]
                } else {
                    m.uvs.into_iter().flatten().collect()
                },
                positions: m.positions.into_iter().flatten().collect(),
                normals: m.normals.into_iter().flatten().collect(),
                colors: m.colors.into_iter().flatten().collect(),
                joints: m.joints.into_iter().flatten().collect(),
                weights: m.weights.into_iter().flatten().collect(),
                // `decode_mesh` checked the indices; whole triangles only.
                indices: {
                    let mut i = m.indices;
                    i.truncate(i.len() / 3 * 3);
                    i
                },
            })
            .unwrap_or_default(),
    }
}

/// A box centered on the origin: faces +X, −X, +Y, −Y, +Z, −Z, four
/// vertices each.
pub fn cuboid(size: [f32; 3]) -> MeshData {
    let [hx, hy, hz] = size.map(|s| s / 2.);
    let mut m = MeshData::default();
    // (normal, right, up) per face, seen from outside: right × up = normal.
    let faces: [(Vec3, Vec3, Vec3); 6] = [
        ([1., 0., 0.], [0., 0., -1.], [0., 1., 0.]),
        ([-1., 0., 0.], [0., 0., 1.], [0., 1., 0.]),
        ([0., 1., 0.], [1., 0., 0.], [0., 0., -1.]),
        ([0., -1., 0.], [1., 0., 0.], [0., 0., 1.]),
        ([0., 0., 1.], [1., 0., 0.], [0., 1., 0.]),
        ([0., 0., -1.], [-1., 0., 0.], [0., 1., 0.]),
    ];
    let half = [hx, hy, hz];
    let extent = |v: Vec3| v[0].abs() * half[0] + v[1].abs() * half[1] + v[2].abs() * half[2];
    for (n, r, u) in faces {
        let (dn, dr, du) = (extent(n), extent(r), extent(u));
        let at = |sr: f32, su: f32| -> Vec3 {
            [
                n[0] * dn + r[0] * dr * sr + u[0] * du * su,
                n[1] * dn + r[1] * dr * sr + u[1] * du * su,
                n[2] * dn + r[2] * dr * sr + u[2] * du * su,
            ]
        };
        // Top-left, top-right, bottom-right, bottom-left.
        let a = m.vertex(at(-1., 1.), n, [0., 0.]);
        let b = m.vertex(at(1., 1.), n, [1., 0.]);
        let c = m.vertex(at(1., -1.), n, [1., 1.]);
        let d = m.vertex(at(-1., -1.), n, [0., 1.]);
        m.triangle(a, d, c);
        m.triangle(a, c, b);
    }
    m
}

/// A UV sphere: `RINGS` rings from the top pole (v = 0) to the bottom one,
/// `SEGMENTS` + 1 vertices around each (u = 0 and u = 1 meet at +Z…−X…).
pub fn sphere(radius: f32) -> MeshData {
    let mut m = MeshData::default();
    let (rings, segs) = (RINGS, SEGMENTS);
    for i in 0..=rings {
        let v = i as f32 / rings as f32;
        let theta = v * std::f32::consts::PI;
        for j in 0..=segs {
            let u = j as f32 / segs as f32;
            let phi = u * std::f32::consts::TAU;
            let n = [
                -theta.sin() * phi.sin(),
                theta.cos(),
                theta.sin() * phi.cos(),
            ];
            m.vertex(n.map(|c| c * radius), n, [u, v]);
        }
    }
    let row = segs + 1;
    for i in 0..rings {
        for j in 0..segs {
            let (a, b) = (i * row + j, i * row + j + 1);
            let (c, d) = ((i + 1) * row + j, (i + 1) * row + j + 1);
            if i != 0 {
                m.triangle(a, b, c);
            }
            if i != rings - 1 {
                m.triangle(b, d, c);
            }
        }
    }
    m
}

/// A rectangle in the XZ plane facing +Y; u along +X, v along +Z.
pub fn plane(size: [f32; 2]) -> MeshData {
    let (hx, hz) = (size[0] / 2., size[1] / 2.);
    let mut m = MeshData::default();
    let n = [0., 1., 0.];
    let a = m.vertex([-hx, 0., -hz], n, [0., 0.]);
    let b = m.vertex([hx, 0., -hz], n, [1., 0.]);
    let c = m.vertex([hx, 0., hz], n, [1., 1.]);
    let d = m.vertex([-hx, 0., hz], n, [0., 1.]);
    m.triangle(a, d, c);
    m.triangle(a, c, b);
    m
}

/// A capped cylinder along Y (a cone when `top` is 0), centered: side,
/// then the top cap (if any), then the bottom cap.
pub fn cylinder(bottom: f32, top: f32, height: f32) -> MeshData {
    let mut m = MeshData::default();
    let segs = SEGMENTS;
    let h = height / 2.;
    // The side's normal leans up by the slope (r_bottom - r_top) / height.
    let slope = if height > 0. {
        (bottom - top) / height
    } else {
        0.
    };
    let around = |j: u32| {
        let u = j as f32 / segs as f32;
        let phi = u * std::f32::consts::TAU;
        (u, phi.sin(), phi.cos())
    };
    for j in 0..=segs {
        let (u, s, c) = around(j);
        let n = normalize([s, slope, c]);
        m.vertex([top * s, h, top * c], n, [u, 0.]);
        m.vertex([bottom * s, -h, bottom * c], n, [u, 1.]);
    }
    for j in 0..segs {
        let (t0, b0, t1, b1) = (j * 2, j * 2 + 1, j * 2 + 2, j * 2 + 3);
        if top > 0. {
            m.triangle(t0, b0, t1);
        }
        m.triangle(t1, b0, b1);
    }
    for (radius, y, ny) in [(top, h, 1f32), (bottom, -h, -1.)] {
        if radius <= 0. {
            continue;
        }
        let center = m.vertex([0., y, 0.], [0., ny, 0.], [0.5, 0.5]);
        let first = center + 1;
        for j in 0..=segs {
            let (_, s, c) = around(j);
            m.vertex(
                [radius * s, y, radius * c],
                [0., ny, 0.],
                [0.5 + 0.5 * s, 0.5 + 0.5 * c * -ny],
            );
        }
        for j in 0..segs {
            if ny > 0. {
                m.triangle(center, first + j, first + j + 1);
            } else {
                m.triangle(center, first + j + 1, first + j);
            }
        }
    }
    m
}

/// A torus in the XZ plane: `SEGMENTS` around the ring, `RINGS` around the tube.
pub fn torus(radius: f32, tube: f32) -> MeshData {
    let mut m = MeshData::default();
    let (segs, rings) = (SEGMENTS, RINGS);
    for i in 0..=segs {
        let u = i as f32 / segs as f32;
        let phi = u * std::f32::consts::TAU;
        let (s, c) = phi.sin_cos();
        for j in 0..=rings {
            let v = j as f32 / rings as f32;
            let theta = v * std::f32::consts::TAU;
            let (ts, tc) = theta.sin_cos();
            // Outward from the tube's center circle.
            let n = [s * tc, ts, c * tc];
            let p = [
                s * (radius + tube * tc),
                tube * ts,
                c * (radius + tube * tc),
            ];
            m.vertex(p, n, [u, v]);
        }
    }
    let row = rings + 1;
    for i in 0..segs {
        for j in 0..rings {
            let (a, b) = (i * row + j, i * row + j + 1);
            let (c, d) = ((i + 1) * row + j, (i + 1) * row + j + 1);
            m.triangle(a, c, b);
            m.triangle(b, c, d);
        }
    }
    m
}

/// Smooth per-vertex normals: each vertex gets the average of its faces'
/// normals (weighted by their area).
pub fn smooth_normals(positions: &[f32], indices: &[u32]) -> Vec<f32> {
    let n = positions.len() / 3;
    let mut normals = vec![0f32; n * 3];
    let at = |i: u32| {
        let i = i as usize * 3;
        [positions[i], positions[i + 1], positions[i + 2]]
    };
    for t in indices.as_chunks::<3>().0 {
        if t.iter().any(|i| *i as usize >= n) {
            continue;
        }
        let (a, b, c) = (at(t[0]), at(t[1]), at(t[2]));
        let face = cross(sub(b, a), sub(c, a));
        for i in t {
            let i = *i as usize * 3;
            normals[i] += face[0];
            normals[i + 1] += face[1];
            normals[i + 2] += face[2];
        }
    }
    for v in normals.as_chunks_mut::<3>().0 {
        let u = normalize(*v);
        *v = if u == [0.; 3] { [0., 1., 0.] } else { u };
    }
    normals
}

/// Where the ray `origin + t · dir` crosses triangle `a b c` (either face):
/// `t`, if it does in front of the origin (Möller–Trumbore). Edges are
/// widened by a hair, so a ray along an edge shared by two triangles hits
/// one of them.
pub fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    const EDGE: f32 = 1e-5;
    let (e1, e2) = (sub(b, a), sub(c, a));
    let p = cross(dir, e2);
    let det = super::math::dot(e1, p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1. / det;
    let s = sub(origin, a);
    let u = super::math::dot(s, p) * inv;
    if !(-EDGE..=1. + EDGE).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = super::math::dot(dir, q) * inv;
    if v < -EDGE || u + v > 1. + EDGE {
        return None;
    }
    let t = super::math::dot(e2, q) * inv;
    (t > 1e-6).then_some(t)
}
