//! The little linear algebra 3D scenes need: column-major 4×4 matrices
//! (OpenGL's layout: element `(row, col)` is at `col * 4 + row`), column
//! vectors, right-handed, OpenGL clip space (z in `-1..=1`).

/// A column-major 4×4 matrix.
pub type Mat4 = [f32; 16];
/// A 3D vector.
pub type Vec3 = [f32; 3];
/// A unit quaternion `[x, y, z, w]` (a rotation).
pub type Quat = [f32; 4];
/// An axis-aligned box `[min x, min y, min z, max x, max y, max z]`.
pub type Aabb = [f32; 6];

/// The rotation that does nothing.
pub const QUAT_IDENTITY: Quat = [0., 0., 0., 1.];

/// The identity.
pub const IDENTITY: Mat4 = [
    1., 0., 0., 0., //
    0., 1., 0., 0., //
    0., 0., 1., 0., //
    0., 0., 0., 1.,
];

/// `a · b` (`b` applies first).
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    out
}

/// A translation.
pub fn translation(v: Vec3) -> Mat4 {
    let mut m = IDENTITY;
    m[12] = v[0];
    m[13] = v[1];
    m[14] = v[2];
    m
}

/// A scale along each axis.
pub fn scaling(v: Vec3) -> Mat4 {
    let mut m = IDENTITY;
    m[0] = v[0];
    m[5] = v[1];
    m[10] = v[2];
    m
}

/// A rotation of `deg` degrees around X (counter-clockwise looking down the axis).
pub fn rotation_x(deg: f32) -> Mat4 {
    let (s, c) = deg.to_radians().sin_cos();
    let mut m = IDENTITY;
    m[5] = c;
    m[6] = s;
    m[9] = -s;
    m[10] = c;
    m
}

/// A rotation of `deg` degrees around Y.
pub fn rotation_y(deg: f32) -> Mat4 {
    let (s, c) = deg.to_radians().sin_cos();
    let mut m = IDENTITY;
    m[0] = c;
    m[2] = -s;
    m[8] = s;
    m[10] = c;
    m
}

/// A rotation of `deg` degrees around Z.
pub fn rotation_z(deg: f32) -> Mat4 {
    let (s, c) = deg.to_radians().sin_cos();
    let mut m = IDENTITY;
    m[0] = c;
    m[1] = s;
    m[4] = -s;
    m[5] = c;
    m
}

/// Euler angles in degrees: `Rx · Ry · Rz` (Z applies first).
pub fn rotation(deg: Vec3) -> Mat4 {
    mul(
        &rotation_x(deg[0]),
        &mul(&rotation_y(deg[1]), &rotation_z(deg[2])),
    )
}

/// A node's local matrix: `T · R · S`.
pub fn compose(position: Vec3, rotation_deg: Vec3, scale: Vec3) -> Mat4 {
    mul(
        &translation(position),
        &mul(&rotation(rotation_deg), &scaling(scale)),
    )
}

/// A node's local matrix from a quaternion: `T · R(q) · S`.
pub fn compose_quat(position: Vec3, q: Quat, scale: Vec3) -> Mat4 {
    let mut m = quat_to_mat4(q);
    for (c, s) in scale.iter().enumerate() {
        for r in 0..3 {
            m[c * 4 + r] *= s;
        }
    }
    m[12] = position[0];
    m[13] = position[1];
    m[14] = position[2];
    m
}

/// The rotation matrix of a unit quaternion.
pub fn quat_to_mat4(q: Quat) -> Mat4 {
    let [x, y, z, w] = q;
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);
    [
        1. - 2. * (yy + zz),
        2. * (xy + wz),
        2. * (xz - wy),
        0.,
        2. * (xy - wz),
        1. - 2. * (xx + zz),
        2. * (yz + wx),
        0.,
        2. * (xz + wy),
        2. * (yz - wx),
        1. - 2. * (xx + yy),
        0.,
        0.,
        0.,
        0.,
        1.,
    ]
}

/// `a · b` (`b` applies first).
pub fn quat_mul(a: Quat, b: Quat) -> Quat {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// The quaternion of Euler angles in degrees, the same rotation as
/// [`rotation`] (`Rx · Ry · Rz`: Z applies first).
pub fn quat_from_euler(deg: Vec3) -> Quat {
    let half = |d: f32| (d.to_radians() / 2.).sin_cos();
    let (sx, cx) = half(deg[0]);
    let (sy, cy) = half(deg[1]);
    let (sz, cz) = half(deg[2]);
    quat_mul(
        [sx, 0., 0., cx],
        quat_mul([0., sy, 0., cy], [0., 0., sz, cz]),
    )
}

/// Unit length (the identity if `q` is zero or not finite).
pub fn quat_normalize(q: Quat) -> Quat {
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if l > 1e-12 && l.is_finite() {
        q.map(|c| c / l)
    } else {
        QUAT_IDENTITY
    }
}

/// Spherical interpolation from `a` to `b` the shorter way around
/// (normalized linear interpolation when they are nearly equal).
pub fn slerp(a: Quat, b: Quat, t: f32) -> Quat {
    let mut d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let b = if d < 0. {
        d = -d;
        b.map(|c| -c)
    } else {
        b
    };
    if d > 0.9995 {
        return quat_normalize(std::array::from_fn(|i| lerp(a[i], b[i], t)));
    }
    let theta = d.clamp(-1., 1.).acos();
    let s = theta.sin();
    let (wa, wb) = (((1. - t) * theta).sin() / s, (t * theta).sin() / s);
    quat_normalize(std::array::from_fn(|i| a[i] * wa + b[i] * wb))
}

/// The box holding `aabb` once transformed by `m` (its 8 corners').
pub fn transform_aabb(m: &Mat4, aabb: &Aabb) -> Aabb {
    let center = [
        (aabb[0] + aabb[3]) / 2.,
        (aabb[1] + aabb[4]) / 2.,
        (aabb[2] + aabb[5]) / 2.,
    ];
    let half = [
        (aabb[3] - aabb[0]) / 2.,
        (aabb[4] - aabb[1]) / 2.,
        (aabb[5] - aabb[2]) / 2.,
    ];
    let c = transform_point(m, center);
    // Each axis' extent: |M| · half (the 3×3 part, absolute values).
    let e: [f32; 3] = std::array::from_fn(|r| (0..3).map(|k| m[k * 4 + r].abs() * half[k]).sum());
    [
        c[0] - e[0],
        c[1] - e[1],
        c[2] - e[2],
        c[0] + e[0],
        c[1] + e[1],
        c[2] + e[2],
    ]
}

/// The box holding both boxes.
pub fn aabb_union(a: &Aabb, b: &Aabb) -> Aabb {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].min(b[2]),
        a[3].max(b[3]),
        a[4].max(b[4]),
        a[5].max(b[5]),
    ]
}

/// The box of `points` (an empty box at the origin if there are none).
pub fn aabb_of(points: impl IntoIterator<Item = Vec3>) -> Aabb {
    let mut b = [f32::MAX, f32::MAX, f32::MAX, f32::MIN, f32::MIN, f32::MIN];
    for p in points {
        b = aabb_union(&b, &[p[0], p[1], p[2], p[0], p[1], p[2]]);
    }
    if b[0] > b[3] { [0.; 6] } else { b }
}

/// The six planes `(a, b, c, d)` of the clip volume of `view_proj`
/// (left, right, bottom, top, near, far): a point is inside when
/// `a·x + b·y + c·z + d ≥ 0` for all of them (Gribb–Hartmann).
pub fn frustum_planes(view_proj: &Mat4) -> [[f32; 4]; 6] {
    let row = |r: usize| {
        [
            view_proj[r],
            view_proj[4 + r],
            view_proj[8 + r],
            view_proj[12 + r],
        ]
    };
    let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
    let add = |a: [f32; 4], b: [f32; 4]| std::array::from_fn(|i| a[i] + b[i]);
    let sub = |a: [f32; 4], b: [f32; 4]| std::array::from_fn(|i| a[i] - b[i]);
    [
        add(r3, r0),
        sub(r3, r0),
        add(r3, r1),
        sub(r3, r1),
        add(r3, r2),
        sub(r3, r2),
    ]
}

/// Whether `aabb` is at least partly inside the frustum (conservative: a
/// box near a corner may pass while outside).
pub fn aabb_in_frustum(planes: &[[f32; 4]; 6], aabb: &Aabb) -> bool {
    planes.iter().all(|p| {
        // The corner farthest along the plane's normal.
        let x = if p[0] >= 0. { aabb[3] } else { aabb[0] };
        let y = if p[1] >= 0. { aabb[4] } else { aabb[1] };
        let z = if p[2] >= 0. { aabb[5] } else { aabb[2] };
        p[0] * x + p[1] * y + p[2] * z + p[3] >= 0.
    })
}

/// Where the ray `origin + t · dir` enters `aabb`: the smallest `t ≥ 0`, if
/// it meets it (the slab test).
pub fn ray_aabb(origin: Vec3, dir: Vec3, aabb: &Aabb) -> Option<f32> {
    let (mut t0, mut t1) = (0f32, f32::MAX);
    for a in 0..3 {
        let (lo, hi) = (aabb[a], aabb[a + 3]);
        if dir[a].abs() < 1e-12 {
            if origin[a] < lo || origin[a] > hi {
                return None;
            }
            continue;
        }
        let inv = 1. / dir[a];
        let (mut n, mut f) = ((lo - origin[a]) * inv, (hi - origin[a]) * inv);
        if n > f {
            std::mem::swap(&mut n, &mut f);
        }
        t0 = t0.max(n);
        t1 = t1.min(f);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// A perspective projection: vertical field of view in degrees.
pub fn perspective(fov_deg: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1. / (fov_deg.to_radians() / 2.).tan();
    let nf = 1. / (near - far);
    let mut m = [0.; 16];
    m[0] = f / aspect;
    m[5] = f;
    m[10] = (far + near) * nf;
    m[11] = -1.;
    m[14] = 2. * far * near * nf;
    m
}

/// An orthographic projection showing `height` units vertically.
pub fn orthographic(height: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let (t, r) = (height / 2., height / 2. * aspect);
    let mut m = IDENTITY;
    m[0] = 1. / r;
    m[5] = 1. / t;
    m[10] = -2. / (far - near);
    m[14] = -(far + near) / (far - near);
    m
}

/// The view matrix of a camera at `eye` looking at `target`, `up` up.
pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let mut f = normalize(sub(target, eye));
    if length(f) == 0. {
        f = [0., 0., -1.];
    }
    let mut s = cross(f, up);
    if length(s) < 1e-6 {
        // Looking straight up or down: any horizontal side vector.
        s = cross(f, [0., 0., -1.]);
    }
    let s = normalize(s);
    let u = cross(s, f);
    [
        s[0],
        u[0],
        -f[0],
        0.,
        s[1],
        u[1],
        -f[1],
        0.,
        s[2],
        u[2],
        -f[2],
        0.,
        -dot(s, eye),
        -dot(u, eye),
        dot(f, eye),
        1.,
    ]
}

/// The inverse (the identity if `m` is singular: zero determinant).
pub fn invert(m: &Mat4) -> Mat4 {
    let mut inv = [0f32; 16];
    inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15]
        + m[9] * m[7] * m[14]
        + m[13] * m[6] * m[11]
        - m[13] * m[7] * m[10];
    inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15]
        - m[8] * m[7] * m[14]
        - m[12] * m[6] * m[11]
        + m[12] * m[7] * m[10];
    inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15]
        + m[8] * m[7] * m[13]
        + m[12] * m[5] * m[11]
        - m[12] * m[7] * m[9];
    inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14]
        - m[8] * m[6] * m[13]
        - m[12] * m[5] * m[10]
        + m[12] * m[6] * m[9];
    inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15]
        - m[9] * m[3] * m[14]
        - m[13] * m[2] * m[11]
        + m[13] * m[3] * m[10];
    inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15]
        + m[8] * m[3] * m[14]
        + m[12] * m[2] * m[11]
        - m[12] * m[3] * m[10];
    inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15]
        - m[8] * m[3] * m[13]
        - m[12] * m[1] * m[11]
        + m[12] * m[3] * m[9];
    inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14]
        + m[8] * m[2] * m[13]
        + m[12] * m[1] * m[10]
        - m[12] * m[2] * m[9];
    inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15]
        + m[5] * m[3] * m[14]
        + m[13] * m[2] * m[7]
        - m[13] * m[3] * m[6];
    inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15]
        - m[4] * m[3] * m[14]
        - m[12] * m[2] * m[7]
        + m[12] * m[3] * m[6];
    inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15]
        + m[4] * m[3] * m[13]
        + m[12] * m[1] * m[7]
        - m[12] * m[3] * m[5];
    inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14]
        - m[4] * m[2] * m[13]
        - m[12] * m[1] * m[6]
        + m[12] * m[2] * m[5];
    inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11]
        - m[5] * m[3] * m[10]
        - m[9] * m[2] * m[7]
        + m[9] * m[3] * m[6];
    inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11]
        + m[4] * m[3] * m[10]
        + m[8] * m[2] * m[7]
        - m[8] * m[3] * m[6];
    inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11]
        - m[4] * m[3] * m[9]
        - m[8] * m[1] * m[7]
        + m[8] * m[3] * m[5];
    inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10]
        + m[4] * m[2] * m[9]
        + m[8] * m[1] * m[6]
        - m[8] * m[2] * m[5];
    let det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
    // Only a truly singular matrix: a node scaled to 1/100 twice (a
    // centimeter rig) has a determinant of 1e-12 and a fine inverse.
    if det == 0. || !det.is_finite() {
        return IDENTITY;
    }
    let out = inv.map(|x| x / det);
    if out.iter().all(|x| x.is_finite()) {
        out
    } else {
        IDENTITY
    }
}

/// The normal matrix (inverse transpose of the upper 3×3), column-major 3×3.
pub fn normal_matrix(m: &Mat4) -> [f32; 9] {
    let inv = invert(m);
    // Transpose of the inverse's upper 3×3, column-major.
    [
        inv[0], inv[4], inv[8], //
        inv[1], inv[5], inv[9], //
        inv[2], inv[6], inv[10],
    ]
}

/// `m · (p, 1)`, divided by w.
pub fn transform_point(m: &Mat4, p: Vec3) -> Vec3 {
    let x = m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12];
    let y = m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13];
    let z = m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14];
    let w = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15];
    if w != 0. && w != 1. {
        [x / w, y / w, z / w]
    } else {
        [x, y, z]
    }
}

/// `m · (v, 0)`.
pub fn transform_dir(m: &Mat4, v: Vec3) -> Vec3 {
    [
        m[0] * v[0] + m[4] * v[1] + m[8] * v[2],
        m[1] * v[0] + m[5] * v[1] + m[9] * v[2],
        m[2] * v[0] + m[6] * v[1] + m[10] * v[2],
    ]
}

/// `a + b`.
pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
/// `a - b`.
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
/// `v · s`.
pub fn scale(v: Vec3, s: f32) -> Vec3 {
    [v[0] * s, v[1] * s, v[2] * s]
}
/// Dot product.
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
/// Cross product.
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
/// Length.
pub fn length(v: Vec3) -> f32 {
    dot(v, v).sqrt()
}
/// Unit vector (zero stays zero).
pub fn normalize(v: Vec3) -> Vec3 {
    let l = length(v);
    if l == 0. { v } else { scale(v, 1. / l) }
}
/// Linear interpolation.
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
/// Linear interpolation of vectors.
pub fn lerp3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

/// sRGB component to linear light.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light to an sRGB component (the inverse of [`srgb_to_linear`]).
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1. / 2.4) - 0.055
    }
}
