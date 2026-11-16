//! Environments for image-based lighting: equirectangular radiance,
//! GGX-prefiltered into roughness levels, and its irradiance as 9 spherical
//! harmonics (see the 'E' blob layout in the v2 spec).
//!
//! # Conventions (mirrored by every shader)
//!
//! **Equirectangular mapping** ([`direction`], [`uv`]): +Y is up, the image's
//! top row is +Y, and
//!
//! ```text
//! u = 0.5 + atan2(d.x, -d.z) / (2π)      v = acos(d.y) / π
//! d = (sin θ · sin φ, cos θ, −sin θ · cos φ)   with θ = v·π, φ = (u − 0.5)·2π
//! ```
//!
//! so the image center (u = 0.5) looks down −Z (where the default camera
//! looks), u = 0.75 is +X and u = 0 / 1 is +Z (behind the camera). Texel
//! `(x, y)` of a `w × h` level has its center at `((x + ½)/w, (y + ½)/h)`;
//! sampling wraps in u and clamps in v.
//!
//! **Levels**: level `i` of `n` holds the radiance prefiltered for roughness
//! `i / (n − 1)` (GGX, `α = roughness²`, split-sum with N = V = R); level 0 is
//! the mirror image. The shader reads `textureLod(env, uv(reflect),
//! roughness · (n − 1))`.
//!
//! **Irradiance** ([`EnvMap::sh`]): 9 RGB coefficients stored as
//! `sh[3·i + channel]`, already convolved with the clamped cosine, so
//! `E(n) = Σᵢ sh[i] · Yᵢ(n)` is the irradiance (W/m² for radiance in W/m²/sr:
//! a uniform radiance `L` gives `E = π·L`). A Lambertian surface's outgoing
//! radiance is therefore `albedo · E(n) / π`. The basis, in this order, is
//! the standard real SH evaluated on the world-space unit normal
//! `n = (x, y, z)`:
//!
//! | i | Yᵢ        | value                       |
//! |---|-----------|-----------------------------|
//! | 0 | Y₀₀       | `0.282095`                  |
//! | 1 | Y₁₋₁      | `0.488603 · y`              |
//! | 2 | Y₁₀       | `0.488603 · z`              |
//! | 3 | Y₁₁       | `0.488603 · x`              |
//! | 4 | Y₂₋₂      | `1.092548 · x·y`            |
//! | 5 | Y₂₋₁      | `1.092548 · y·z`            |
//! | 6 | Y₂₀       | `0.315392 · (3z² − 1)`      |
//! | 7 | Y₂₁       | `1.092548 · x·z`            |
//! | 8 | Y₂₂       | `0.546274 · (x² − y²)`      |
//!
//! The coefficients are `sh = A_l · L_lm`, where `L_lm = ∫ L(ω) Y_lm(ω) dω`
//! is the radiance's projection and `A₀ = π`, `A₁ = 2π/3`, `A₂ = π/4` are the
//! clamped-cosine convolution constants (Ramamoorthi & Hanrahan 2001). The
//! basis being complete per band, the result does not depend on which axis
//! the formulas call "z". Clamp `E` at 0 in the shader (order-2 ringing next
//! to a very bright, small source can dip slightly below zero).

use std::f32::consts::PI;
use std::sync::OnceLock;

use super::math::{Vec3, cross, dot, lerp3, normalize, scale};

/// Width of level 0 of every environment built here.
pub const WIDTH: u32 = 256;
/// Height of level 0 of every environment built here.
pub const HEIGHT: u32 = 128;
/// Number of roughness levels (256×128 … 8×4).
pub const LEVELS: usize = 6;

/// The 'E' blob's magic and version.
const MAGIC: &[u8; 4] = b"P3DE";
const VERSION: u32 = 1;
/// Largest finite half float; radiance is clamped to it.
const HALF_MAX: f32 = 65504.;
/// GGX samples per texel for levels 1…5 (level 0 is not filtered).
const SAMPLES: [u32; LEVELS] = [1, 24, 48, 64, 96, 128];

/// A prefiltered environment, ready to encode as an 'E' blob.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvMap {
    /// Width of level 0 (a power of two; height is half of it).
    pub width: u32,
    /// Height of level 0.
    pub height: u32,
    /// Level `i` is `(width >> i) × (height >> i)` RGBA half floats, row-major,
    /// top row = +Y; level 0 is roughness 0, the last roughness 1.
    pub levels: Vec<Vec<[u16; 4]>>,
    /// Irradiance as 9 RGB spherical-harmonic coefficients (already convolved).
    pub sh: [f32; 27],
}

impl EnvMap {
    /// The 'E' blob bytes (header included): `"P3DE"`, `u32 1`, `u32 width`,
    /// `u32 height`, `u32 levels`, `f32 sh[27]`, then every level's RGBA16F
    /// texels, all little-endian.
    pub fn encode(&self) -> Vec<u8> {
        let texels: usize = self.levels.iter().map(Vec::len).sum();
        let mut out = Vec::with_capacity(20 + 27 * 4 + texels * 8);
        out.extend_from_slice(MAGIC);
        for v in [VERSION, self.width, self.height, self.levels.len() as u32] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for c in self.sh {
            out.extend_from_slice(&c.to_le_bytes());
        }
        for level in &self.levels {
            for t in level {
                for c in t {
                    out.extend_from_slice(&c.to_le_bytes());
                }
            }
        }
        out
    }

    /// Parses an 'E' blob ([`EnvMap::encode`]'s output); `None` if malformed.
    pub fn decode(bytes: &[u8]) -> Option<EnvMap> {
        let mut r = Reader { bytes, at: 0 };
        if r.take(4)? != MAGIC || r.u32()? != VERSION {
            return None;
        }
        let (width, height, n) = (r.u32()?, r.u32()?, r.u32()? as usize);
        if width == 0 || height == 0 || n == 0 || n > 32 {
            return None;
        }
        let mut sh = [0.; 27];
        for c in &mut sh {
            *c = f32::from_le_bytes(r.take(4)?.try_into().ok()?);
        }
        let mut levels = Vec::with_capacity(n);
        for i in 0..n {
            let w = (width >> i).max(1) as usize;
            let h = (height >> i).max(1) as usize;
            let raw = r.take(w.checked_mul(h)?.checked_mul(8)?)?;
            let level: Vec<[u16; 4]> = raw
                .as_chunks::<8>()
                .0
                .iter()
                .map(|t| std::array::from_fn(|c| u16::from_le_bytes([t[2 * c], t[2 * c + 1]])))
                .collect();
            levels.push(level);
        }
        (r.at == bytes.len()).then_some(EnvMap {
            width,
            height,
            levels,
            sh,
        })
    }

    /// The irradiance `E(n) = Σ sh[i] · Yᵢ(n)` around the unit normal `n`
    /// (what the shaders compute), clamped at 0.
    pub fn irradiance(&self, n: Vec3) -> Vec3 {
        let y = sh_basis(n);
        std::array::from_fn(|c| {
            (0..9)
                .map(|i| self.sh[3 * i + c] * y[i])
                .sum::<f32>()
                .max(0.)
        })
    }

    /// Texel `(x, y)` of level `level` as linear RGB.
    pub fn texel(&self, level: usize, x: usize, y: usize) -> Vec3 {
        let w = (self.width >> level).max(1) as usize;
        let t = self.levels[level][y * w + x];
        [f16_to_f32(t[0]), f16_to_f32(t[1]), f16_to_f32(t[2])]
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}

// --- Presets ----------------------------------------------------------------------------

static STUDIO: OnceLock<EnvMap> = OnceLock::new();
static NEUTRAL: OnceLock<EnvMap> = OnceLock::new();
static SUNSET: OnceLock<EnvMap> = OnceLock::new();

/// Soft boxes over a gray gradient: a neutral product-shot light. A large key
/// box overhead in front (toward +Z, where the camera usually stands), a cool
/// fill strip on the left, a warm rim strip behind on the right, over a
/// gray cyclorama (darker floor, soft horizon). Built once, then cloned.
pub fn studio() -> EnvMap {
    STUDIO
        .get_or_init(|| build(rasterize(studio_radiance)))
        .clone()
}

/// An even gray room, slightly brighter above. Built once, then cloned.
pub fn neutral() -> EnvMap {
    NEUTRAL
        .get_or_init(|| build(rasterize(neutral_radiance)))
        .clone()
}

/// A warm sky gradient with a low sun: orange horizon, deep blue zenith, a
/// small bright sun disk just above the horizon (ahead and to the right, in
/// front of the default camera) and dim ground. Built once, then cloned.
pub fn sunset() -> EnvMap {
    SUNSET
        .get_or_init(|| build(rasterize(sunset_radiance)))
        .clone()
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// A soft rectangular light seen from the origin: centered on `center`, its
/// vertical edge aligned with world +Y, `half` its half extents on the
/// tangent plane at distance 1 (`tan` of the half angles).
struct SoftBox {
    center: Vec3,
    right: Vec3,
    up: Vec3,
    half: [f32; 2],
    color: Vec3,
}

impl SoftBox {
    fn new(center: Vec3, half: [f32; 2], color: Vec3) -> SoftBox {
        let center = normalize(center);
        let right = normalize(cross([0., 1., 0.], center));
        let up = cross(center, right);
        SoftBox {
            center,
            right,
            up,
            half,
            color,
        }
    }

    fn eval(&self, d: Vec3) -> Vec3 {
        let c = dot(d, self.center);
        if c <= 0.05 {
            return [0.; 3];
        }
        let (x, y) = (dot(d, self.right) / c, dot(d, self.up) / c);
        let edge = 0.035;
        let mask = |p: f32, h: f32| 1. - smoothstep(h - edge, h + edge, p.abs());
        // A diffuser: brightest in the middle, a little dimmer at the edges.
        let falloff = 1. - 0.25 * ((x / self.half[0]).powi(2) + (y / self.half[1]).powi(2)).min(1.);
        scale(
            self.color,
            mask(x, self.half[0]) * mask(y, self.half[1]) * falloff,
        )
    }
}

fn studio_radiance(d: Vec3) -> Vec3 {
    static BOXES: OnceLock<[SoftBox; 3]> = OnceLock::new();
    let boxes = BOXES.get_or_init(|| {
        [
            // Key: overhead, in front.
            SoftBox::new([0., 0.85, 0.55], [0.55, 0.32], [5., 5., 5.]),
            // Fill: a tall strip on the left, slightly cool.
            SoftBox::new([-1., 0.12, 0.3], [0.16, 0.6], [1.7, 1.8, 1.95]),
            // Rim: a tall strip behind on the right, slightly warm.
            SoftBox::new([0.85, 0.25, -0.7], [0.12, 0.55], [3.4, 3.2, 3.]),
        ]
    });
    let y = d[1];
    let floor = 0.05 + 0.11 * (1. + y.max(-1.)); // 0.05 straight down, 0.16 at the horizon.
    let wall = 0.2 - 0.09 * y.max(0.).powf(0.7); // 0.20 at the horizon, 0.11 at the ceiling.
    let g = floor + (wall - floor) * smoothstep(-0.12, 0.06, y);
    let mut c = [g, g, g * 1.02];
    for b in boxes {
        let l = b.eval(d);
        c = [c[0] + l[0], c[1] + l[1], c[2] + l[2]];
    }
    c
}

fn neutral_radiance(d: Vec3) -> Vec3 {
    let y = d[1];
    let t = 0.5 + 0.5 * y;
    // A soft, wide glow overhead (a bright ceiling) over a gentle gradient.
    let g = 0.32 + 0.22 * t * t + 0.18 * y.max(0.).powi(3);
    [g, g, g]
}

fn sunset_radiance(d: Vec3) -> Vec3 {
    let sun = sun_direction();
    let y = d[1];
    let cos_sun = dot(d, sun).clamp(-1., 1.);
    let angle = cos_sun.acos();
    if y < 0. {
        // Dim warm ground, a little brighter at the horizon, blended into the sky.
        let t = (-y).min(1.);
        let ground = lerp3([0.09, 0.06, 0.045], [0.025, 0.02, 0.018], t.powf(0.5));
        let sky = sky(0., angle);
        return lerp3(sky, ground, smoothstep(0., 0.06, -y));
    }
    let mut c = sky(y, angle);
    // Sun disk (≈ 3° wide, wider than the real one so that it spans a few
    // texels of a 256×128 map) with a soft limb.
    let disk = 1. - smoothstep(0.02, 0.03, angle);
    let s = 250. * disk;
    c = [c[0] + s, c[1] + s * 0.78, c[2] + s * 0.52];
    c
}

fn sun_direction() -> Vec3 {
    // Elevation 4°, azimuth 35° to the right of −Z.
    let (el, az) = (4f32.to_radians(), 35f32.to_radians());
    [az.sin() * el.cos(), el.sin(), -az.cos() * el.cos()]
}

/// Sky radiance at height `y ≥ 0` (sin of the elevation), `angle` radians
/// away from the sun.
fn sky(y: f32, angle: f32) -> Vec3 {
    let horizon = [2.2, 0.95, 0.38];
    let mid = [0.75, 0.42, 0.42];
    let zenith = [0.05, 0.1, 0.32];
    let e = y.clamp(0., 1.).asin() / (PI / 2.);
    let base = if e < 0.25 {
        lerp3(horizon, mid, smoothstep(0., 0.25, e))
    } else {
        lerp3(mid, zenith, smoothstep(0.25, 1., e).powf(0.8))
    };
    // Warmer and brighter toward the sun, near the horizon.
    let glow = (-angle / 0.35).exp() * 3. + (-angle / 1.2).exp() * 0.6 * (1. - e);
    [base[0] + glow, base[1] + glow * 0.55, base[2] + glow * 0.22]
}

/// Evaluates a procedural radiance at 256×128 with 2×2 supersampling.
fn rasterize(f: impl Fn(Vec3) -> Vec3) -> Img {
    let (w, h) = (WIDTH as usize, HEIGHT as usize);
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let mut c = [0.; 3];
            for (sx, sy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let l = f(direction(
                    (x as f32 + sx) / w as f32,
                    (y as f32 + sy) / h as f32,
                ));
                c = [c[0] + l[0], c[1] + l[1], c[2] + l[2]];
            }
            px.push(sanitize(scale(c, 0.25)));
        }
    }
    Img { w, h, px }
}

// --- Import -----------------------------------------------------------------------------

/// A Radiance `.hdr` (RGBE) image, resampled; `None` if unreadable.
///
/// Reads `#?RADIANCE` / `#?RGBE` files in `32-bit_rle_rgbe` format, `-Y h +X
/// w` (or bottom-up `+Y h +X w`) orientation, with new-style RLE, old-style
/// RLE or flat scanlines. `EXPOSURE` and other header variables are ignored:
/// the pixels are taken as linear radiance.
pub fn from_hdr(bytes: &[u8]) -> Option<EnvMap> {
    let (w, h, px) = decode_hdr(bytes)?;
    let img = reduce(w, h, |i| rgbe(px[i]));
    Some(build(img.resample(WIDTH as usize, HEIGHT as usize)))
}

/// Linear radiance in an equirectangular layout (`width × height`, row-major,
/// top row = +Y).
///
/// Resampled to 256×128 (box-filtered first when larger, then bilinear).
/// Negative or non-finite values count as 0 and radiance is clamped to the
/// largest half float. If `rgb` holds fewer than `width × height` texels (or
/// a size is 0), the environment is black.
pub fn from_equirect(width: u32, height: u32, rgb: &[[f32; 3]]) -> EnvMap {
    let (w, h) = (width as usize, height as usize);
    let img = match w.checked_mul(h) {
        Some(n) if n > 0 && rgb.len() >= n => reduce(w, h, |i| sanitize(rgb[i])),
        _ => Img {
            w: 1,
            h: 1,
            px: vec![[0.; 3]],
        },
    };
    build(img.resample(WIDTH as usize, HEIGHT as usize))
}

fn sanitize(c: Vec3) -> Vec3 {
    c.map(|v| {
        if v.is_nan() {
            0.
        } else {
            v.clamp(0., HALF_MAX)
        }
    })
}

fn rgbe(p: [u8; 4]) -> Vec3 {
    if p[3] == 0 {
        return [0.; 3];
    }
    // 2^(e − 128 − 8): the mantissas are 8-bit fractions.
    let f = 2f32.powi(p[3] as i32 - 136);
    sanitize([p[0] as f32 * f, p[1] as f32 * f, p[2] as f32 * f])
}

/// Decodes a Radiance file to its RGBE texels (top row first).
fn decode_hdr(bytes: &[u8]) -> Option<(usize, usize, Vec<[u8; 4]>)> {
    let mut at = 0;
    let mut line = || -> Option<&[u8]> {
        let rest = bytes.get(at..)?;
        let end = rest.iter().position(|&b| b == b'\n')?;
        at += end + 1;
        let l = &rest[..end];
        Some(l.strip_suffix(b"\r").unwrap_or(l))
    };
    let first = line()?;
    if !(first.starts_with(b"#?RADIANCE") || first.starts_with(b"#?RGBE")) {
        return None;
    }
    loop {
        let l = line()?;
        if l.is_empty() {
            break;
        }
        if let Some(format) = l.strip_prefix(b"FORMAT=")
            && format.trim_ascii() != b"32-bit_rle_rgbe"
        {
            return None;
        }
    }
    let res = std::str::from_utf8(line()?).ok()?;
    let mut tokens = res.split_ascii_whitespace();
    let (ay, h, ax, w) = (
        tokens.next()?,
        tokens.next()?,
        tokens.next()?,
        tokens.next()?,
    );
    if tokens.next().is_some() || ax != "+X" {
        return None;
    }
    let flip = match ay {
        "-Y" => false,
        "+Y" => true,
        _ => return None,
    };
    let (w, h): (usize, usize) = (w.parse().ok()?, h.parse().ok()?);
    if w == 0 || h == 0 || w > 1 << 15 || h > 1 << 15 || w * h > 1 << 27 {
        return None;
    }
    let data = &bytes[at..];
    let mut r = 0; // Read position in `data`.
    // Grows as scanlines decode, so a truncated file fails before a big allocation.
    let mut px: Vec<[u8; 4]> = Vec::new();
    let mut row = vec![[0u8; 4]; w];
    for _ in 0..h {
        let head = data.get(r..r + 4);
        let rle = (8..=0x7fff).contains(&w)
            && head.is_some_and(|b| b[0] == 2 && b[1] == 2 && b[2] & 0x80 == 0);
        if rle {
            let head = head?;
            if (((head[2] as usize) << 8) | head[3] as usize) != w {
                return None;
            }
            r += 4;
            for c in 0..4 {
                let mut x = 0;
                while x < w {
                    let n = *data.get(r)? as usize;
                    r += 1;
                    if n > 128 {
                        let (n, v) = (n - 128, *data.get(r)?);
                        r += 1;
                        if x + n > w {
                            return None;
                        }
                        row[x..x + n].iter_mut().for_each(|p| p[c] = v);
                        x += n;
                    } else {
                        if n == 0 || x + n > w {
                            return None;
                        }
                        let src = data.get(r..r + n)?;
                        r += n;
                        for (p, &v) in row[x..x + n].iter_mut().zip(src) {
                            p[c] = v;
                        }
                        x += n;
                    }
                }
            }
        } else {
            // Flat pixels, with old-style runs: (1, 1, 1, n) repeats the
            // previous pixel n << shift times, shift growing by 8 per
            // consecutive run marker.
            let (mut x, mut shift) = (0, 0u32);
            while x < w {
                let p: [u8; 4] = data.get(r..r + 4)?.try_into().ok()?;
                r += 4;
                if p[0] == 1 && p[1] == 1 && p[2] == 1 {
                    if x == 0 || shift > 16 {
                        return None;
                    }
                    let n = (p[3] as usize) << shift;
                    if x + n > w {
                        return None;
                    }
                    let prev = row[x - 1];
                    row[x..x + n].fill(prev);
                    x += n;
                    shift += 8;
                } else {
                    row[x] = p;
                    x += 1;
                    shift = 0;
                }
            }
        }
        px.extend_from_slice(&row);
    }
    if flip {
        let mut flipped = Vec::with_capacity(px.len());
        for y in (0..h).rev() {
            flipped.extend_from_slice(&px[y * w..(y + 1) * w]);
        }
        px = flipped;
    }
    Some((w, h, px))
}

// --- Mapping and half floats ------------------------------------------------------------

/// The unit direction at equirectangular coordinates `(u, v)` (see the
/// module docs): `θ = v·π`, `φ = (u − 0.5)·2π`,
/// `d = (sin θ sin φ, cos θ, −sin θ cos φ)`.
pub fn direction(u: f32, v: f32) -> Vec3 {
    let (st, ct) = (v * PI).sin_cos();
    let (sp, cp) = ((u - 0.5) * 2. * PI).sin_cos();
    [st * sp, ct, -st * cp]
}

/// The equirectangular coordinates of a direction (normalized here):
/// `u = 0.5 + atan2(d.x, −d.z)/(2π)` in [0, 1], `v = acos(d.y)/π` in [0, 1].
pub fn uv(dir: Vec3) -> (f32, f32) {
    let d = normalize(dir);
    let u = 0.5 + d[0].atan2(-d[2]) / (2. * PI);
    let v = d[1].clamp(-1., 1.).acos() / PI;
    (u, v)
}

/// The 9 real SH basis functions at the unit direction `n`, in the order of
/// [`EnvMap::sh`] (see the module docs).
pub fn sh_basis(n: Vec3) -> [f32; 9] {
    let [x, y, z] = n;
    [
        0.282_094_8,
        0.488_602_5 * y,
        0.488_602_5 * z,
        0.488_602_5 * x,
        1.092_548_4 * x * y,
        1.092_548_4 * y * z,
        0.315_391_57 * (3. * z * z - 1.),
        1.092_548_4 * x * z,
        0.546_274_2 * (x * x - y * y),
    ]
}

/// The clamped-cosine convolution constant of each basis function's band.
const SH_BAND: [f32; 9] = [
    PI,
    2. * PI / 3.,
    2. * PI / 3.,
    2. * PI / 3.,
    PI / 4.,
    PI / 4.,
    PI / 4.,
    PI / 4.,
    PI / 4.,
];

/// An f32 as IEEE 754 binary16 bits, rounded to nearest even; overflow gives
/// ±infinity, NaN stays NaN, tiny values become subnormals or ±0.
pub fn f32_to_f16(f: f32) -> u16 {
    let x = f.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xff) as i32;
    let man = x & 0x7f_ffff;
    if exp == 0xff {
        // Infinity, or NaN (kept quiet and non-zero).
        return sign
            | 0x7c00
            | if man != 0 {
                0x200 | (man >> 13) as u16
            } else {
                0
            };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        // Subnormal half (or zero): value = m · 2^(e − 14) units of 2^−24.
        if e < -10 {
            return sign;
        }
        let m = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let mut h = m >> shift;
        let rem = m & ((1 << shift) - 1);
        let half = 1 << (shift - 1);
        if rem > half || (rem == half && h & 1 == 1) {
            h += 1; // May carry into the smallest normal, which is right.
        }
        return sign | h as u16;
    }
    let mut h = ((e as u32) << 10) | (man >> 13);
    let rem = man & 0x1fff;
    if rem > 0x1000 || (rem == 0x1000 && h & 1 == 1) {
        h += 1; // May carry into the exponent (up to infinity), which is right.
    }
    sign | h as u16
}

/// IEEE 754 binary16 bits as an f32 (exact).
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h as u32) & 0x8000) << 16;
    let exp = ((h >> 10) & 0x1f) as u32;
    let man = (h & 0x3ff) as u32;
    let bits = match (exp, man) {
        (0, 0) => sign,
        (0, _) => {
            // Subnormal: normalize.
            let mut e = 127 - 15 + 1;
            let mut m = man;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((m & 0x3ff) << 13)
        }
        (0x1f, _) => sign | 0x7f80_0000 | (man << 13),
        _ => sign | ((exp + 127 - 15) << 23) | (man << 13),
    };
    f32::from_bits(bits)
}

// --- Images -----------------------------------------------------------------------------

/// A linear-RGB equirectangular image.
#[derive(Clone)]
struct Img {
    w: usize,
    h: usize,
    px: Vec<Vec3>,
}

impl Img {
    /// Bilinear sample at `(u, v)`, wrapping in u and clamping in v.
    fn sample(&self, u: f32, v: f32) -> Vec3 {
        let x = u * self.w as f32 - 0.5;
        let y = (v * self.h as f32 - 0.5).clamp(0., (self.h - 1) as f32);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let xa = (x0 as i64).rem_euclid(self.w as i64) as usize;
        let xb = (xa + 1) % self.w;
        let ya = y0 as usize;
        let yb = (ya + 1).min(self.h - 1);
        let p = |x: usize, y: usize| self.px[y * self.w + x];
        let top = lerp3(p(xa, ya), p(xb, ya), fx);
        let bottom = lerp3(p(xa, yb), p(xb, yb), fx);
        lerp3(top, bottom, fy)
    }

    /// Bilinear resampling to `w × h` (the image itself if it already is).
    fn resample(self, w: usize, h: usize) -> Img {
        if self.w == w && self.h == h {
            return self;
        }
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            let v = (y as f32 + 0.5) / h as f32;
            for x in 0..w {
                px.push(self.sample((x as f32 + 0.5) / w as f32, v));
            }
        }
        Img { w, h, px }
    }

    /// Half the size (2×2 box), for power-of-two images; 1 stays 1.
    fn half(&self) -> Img {
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let mut c = [0.; 3];
                let (x0, y0) = (2 * x, 2 * y);
                let xs = [x0, (x0 + 1).min(self.w - 1)];
                let ys = [y0, (y0 + 1).min(self.h - 1)];
                for yy in ys {
                    for xx in xs {
                        let p = self.px[yy * self.w + xx];
                        c = [c[0] + p[0], c[1] + p[1], c[2] + p[2]];
                    }
                }
                px.push(scale(c, 0.25));
            }
        }
        Img { w, h, px }
    }
}

/// Box-filters a `w × h` image (texel `i` given by `get`) by powers of two
/// until it is less than twice 256×128, so that bilinear resampling to it
/// does not skip texels.
fn reduce(w: usize, h: usize, get: impl Fn(usize) -> Vec3) -> Img {
    let factor = |n: usize, target: usize| {
        let mut f = 1;
        while n / (f * 2) >= target {
            f *= 2;
        }
        f
    };
    let (fx, fy) = (factor(w, WIDTH as usize), factor(h, HEIGHT as usize));
    let (ow, oh) = (w.div_ceil(fx), h.div_ceil(fy));
    let mut px = Vec::with_capacity(ow * oh);
    for oy in 0..oh {
        let ys = oy * fy..((oy + 1) * fy).min(h);
        for ox in 0..ow {
            let xs = ox * fx..((ox + 1) * fx).min(w);
            let mut c = [0.; 3];
            for y in ys.clone() {
                for x in xs.clone() {
                    let p = get(y * w + x);
                    c = [c[0] + p[0], c[1] + p[1], c[2] + p[2]];
                }
            }
            px.push(scale(c, 1. / (ys.len() * xs.len()) as f32));
        }
    }
    Img { w: ow, h: oh, px }
}

// --- Prefiltering -----------------------------------------------------------------------

/// Builds the levels and the SH of a 256×128 radiance image.
fn build(radiance: Img) -> EnvMap {
    debug_assert_eq!((radiance.w, radiance.h), (WIDTH as usize, HEIGHT as usize));
    let mut mips = vec![radiance];
    while mips.last().is_some_and(|m| m.w > 1 || m.h > 1) {
        let next = mips.last().map(Img::half);
        mips.extend(next);
    }
    let mut levels = Vec::with_capacity(LEVELS);
    levels.push(to_half(&mips[0]));
    for (i, &n) in SAMPLES.iter().enumerate().skip(1) {
        let roughness = i as f32 / (LEVELS - 1) as f32;
        levels.push(to_half(&prefilter(
            &mips,
            roughness,
            n,
            WIDTH as usize >> i,
            HEIGHT as usize >> i,
        )));
    }
    EnvMap {
        width: WIDTH,
        height: HEIGHT,
        levels,
        sh: project_sh(&mips[0]),
    }
}

fn to_half(img: &Img) -> Vec<[u16; 4]> {
    img.px
        .iter()
        .map(|c| [f32_to_f16(c[0]), f32_to_f16(c[1]), f32_to_f16(c[2]), 0x3c00])
        .collect()
}

/// Trilinear sample of the mip chain at a fractional level.
fn sample_lod(mips: &[Img], u: f32, v: f32, lod: f32) -> Vec3 {
    let lod = lod.clamp(0., (mips.len() - 1) as f32);
    let l0 = lod as usize;
    let t = lod - l0 as f32;
    let a = mips[l0].sample(u, v);
    if t <= 0. || l0 + 1 >= mips.len() {
        a
    } else {
        lerp3(a, mips[l0 + 1].sample(u, v), t)
    }
}

/// One GGX sample in the tangent frame of N (= V): its light direction, and
/// the solid angle it stands for.
struct Sample {
    l: Vec3,
    omega: f32,
}

/// Importance-sampled GGX (split-sum, N = V = R), `n` Hammersley samples per
/// texel, each read from the mip whose texels match its solid angle (Karis'
/// "filtered importance sampling", which keeps a sun from turning into
/// fireflies), weighted by N·L.
fn prefilter(mips: &[Img], roughness: f32, n: u32, w: usize, h: usize) -> Img {
    let a2 = (roughness * roughness).powi(2);
    let samples: Vec<Sample> = (0..n)
        .filter_map(|i| {
            let e1 = i as f32 / n as f32;
            let e2 = i.reverse_bits() as f32 * 2.328_306_4e-10;
            let phi = 2. * PI * e1;
            let cos_h = ((1. - e2) / (1. + (a2 - 1.) * e2)).sqrt();
            let sin_h = (1. - cos_h * cos_h).max(0.).sqrt();
            let hv = [sin_h * phi.cos(), sin_h * phi.sin(), cos_h];
            // L = reflect(−V, H) with V = (0, 0, 1).
            let l = [
                2. * cos_h * hv[0],
                2. * cos_h * hv[1],
                2. * cos_h * cos_h - 1.,
            ];
            if l[2] <= 1e-4 {
                return None;
            }
            // pdf(L) = D(H)·(N·H) / (4 (V·H)) = D(H) / 4.
            let d = (1. + (a2 - 1.) * cos_h * cos_h).powi(2);
            let pdf = a2 / (PI * d) / 4.;
            Some(Sample {
                l,
                omega: 1. / (n as f32 * pdf),
            })
        })
        .collect();
    let base = &mips[0];
    // Solid angle of a level-0 texel on the equator.
    let texel = (2. * PI / base.w as f32) * (PI / base.h as f32);
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        let v = (y as f32 + 0.5) / h as f32;
        for x in 0..w {
            let nrm = direction((x as f32 + 0.5) / w as f32, v);
            let up = if nrm[1].abs() < 0.999 {
                [0., 1., 0.]
            } else {
                [1., 0., 0.]
            };
            let t = normalize(cross(up, nrm));
            let b = cross(nrm, t);
            let mut sum = [0.; 3];
            let mut weight = 0.;
            for s in &samples {
                let l = [
                    t[0] * s.l[0] + b[0] * s.l[1] + nrm[0] * s.l[2],
                    t[1] * s.l[0] + b[1] * s.l[1] + nrm[1] * s.l[2],
                    t[2] * s.l[0] + b[2] * s.l[1] + nrm[2] * s.l[2],
                ];
                let (u, sv) = uv(l);
                // Equirect texels shrink toward the poles (sin θ).
                let sin_t = (1. - l[1] * l[1]).max(0.).sqrt().max(0.05);
                let lod = 0.5 * (s.omega / (texel * sin_t)).log2() + 1.;
                let c = sample_lod(mips, u, sv, lod);
                let wgt = s.l[2];
                sum = [
                    sum[0] + c[0] * wgt,
                    sum[1] + c[1] * wgt,
                    sum[2] + c[2] * wgt,
                ];
                weight += wgt;
            }
            px.push(if weight > 0. {
                scale(sum, 1. / weight)
            } else {
                [0.; 3]
            });
        }
    }
    Img { w, h, px }
}

/// Projects the radiance on the SH basis (exact solid angle per texel row)
/// and convolves with the clamped cosine.
fn project_sh(img: &Img) -> [f32; 27] {
    let mut acc = [0f64; 27];
    let dphi = 2. * PI / img.w as f32;
    for y in 0..img.h {
        let t0 = y as f32 * PI / img.h as f32;
        let t1 = (y + 1) as f32 * PI / img.h as f32;
        let d_omega = dphi * (t0.cos() - t1.cos());
        let v = (y as f32 + 0.5) / img.h as f32;
        for x in 0..img.w {
            let basis = sh_basis(direction((x as f32 + 0.5) / img.w as f32, v));
            let c = img.px[y * img.w + x];
            for (i, yi) in basis.iter().enumerate() {
                for ch in 0..3 {
                    acc[3 * i + ch] += (c[ch] * yi * d_omega) as f64;
                }
            }
        }
    }
    std::array::from_fn(|k| acc[k] as f32 * SH_BAND[k / 3])
}

#[cfg(test)]
mod tests {
    use super::super::math;
    use super::*;
    use std::time::Instant;

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.)
    }

    #[test]
    fn half_floats_round_trip() {
        for v in [
            0.,
            1.,
            -1.,
            0.5,
            2.,
            65504.,
            -65504.,
            6.1035156e-5,
            5.9604645e-8,
            1024.,
        ] {
            assert_eq!(f16_to_f32(f32_to_f16(v)), v, "{v}");
        }
        assert_eq!(f32_to_f16(1.), 0x3c00);
        assert_eq!(f32_to_f16(-2.), 0xc000);
        assert_eq!(f32_to_f16(0.), 0);
        assert_eq!(f32_to_f16(-0.), 0x8000);
        assert_eq!(f32_to_f16(65504.), 0x7bff);
        assert_eq!(f32_to_f16(65520.), 0x7c00); // Rounds up to infinity.
        assert_eq!(f32_to_f16(1e9), 0x7c00);
        assert_eq!(f32_to_f16(f32::INFINITY), 0x7c00);
        assert_eq!(f32_to_f16(f32::NEG_INFINITY), 0xfc00);
        assert!(f16_to_f32(f32_to_f16(f32::NAN)).is_nan());
        assert_eq!(f32_to_f16(5.9604645e-8), 1); // Smallest subnormal.
        assert_eq!(f32_to_f16(2.9802322e-8), 0); // Half of it: ties to even (0).
        assert_eq!(f32_to_f16(3.0e-8), 1);
        assert_eq!(f32_to_f16(1e-10), 0);
        assert_eq!(f32_to_f16(6.1035156e-5), 0x0400); // Smallest normal.
        // 1 + 2^-11 is halfway between 1 and the next half: ties to even (1).
        assert_eq!(f32_to_f16(1. + 2f32.powi(-11)), 0x3c00);
        assert_eq!(f32_to_f16(1. + 3. * 2f32.powi(-11)), 0x3c02);
        // Every half (but NaNs) survives f16 → f32 → f16.
        for h in 0..=u16::MAX {
            let f = f16_to_f32(h);
            if f.is_nan() {
                assert!(h & 0x7c00 == 0x7c00 && h & 0x3ff != 0);
            } else {
                assert_eq!(f32_to_f16(f), h, "{h:#06x}");
            }
        }
        // Rounding error is at most half an ulp.
        for i in 0..10_000 {
            let v = (i as f32 * 0.7371).sin() * 300.;
            let r = f16_to_f32(f32_to_f16(v));
            assert!(
                (r - v).abs() <= v.abs() * 2f32.powi(-11) + 1e-7,
                "{v} → {r}"
            );
        }
    }

    #[test]
    fn uv_and_direction_round_trip() {
        // The spec's axes: center = −Z, u = 0.75 = +X, top = +Y.
        let d = direction(0.5, 0.5);
        assert!(close(d[2], -1., 1e-6) && d[0].abs() < 1e-6 && d[1].abs() < 1e-6);
        let d = direction(0.75, 0.5);
        assert!(close(d[0], 1., 1e-6));
        assert!(close(direction(0.3, 0.)[1], 1., 1e-6));
        assert!(close(direction(0.3, 1.)[1], -1., 1e-6));
        assert_eq!(uv([0., 0., -1.]), (0.5, 0.5));
        for i in 0..64 {
            for j in 1..32 {
                let (u, v) = (i as f32 / 64. + 0.003, j as f32 / 32.);
                let d = direction(u, v);
                assert!(close(math::length(d), 1., 1e-5));
                let (u2, v2) = uv(d);
                assert!(
                    (u - u2).abs() < 1e-4 && (v - v2).abs() < 1e-4,
                    "{u},{v} → {u2},{v2}"
                );
                // And direction → uv → direction.
                let d2 = direction(u2, v2);
                assert!(dot(d, d2) > 1. - 1e-6);
            }
        }
    }

    #[test]
    fn constant_radiance_gives_pi_l_irradiance() {
        let l = [0.5, 1., 2.];
        let env = from_equirect(4, 2, &[l; 8]);
        for n in [
            [0., 1., 0.],
            [0., -1., 0.],
            [1., 0., 0.],
            normalize([1., -2., 3.]),
        ] {
            let e = env.irradiance(n);
            for c in 0..3 {
                assert!(close(e[c], PI * l[c], 2e-3), "{n:?}: {e:?}");
            }
        }
        // Only the DC term is non-zero: sh[0] · Y00 = π L.
        assert!(close(env.sh[0] * 0.282_094_8, PI * 0.5, 1e-3));
        assert!(env.sh[3..].iter().all(|c| c.abs() < 2e-3), "{:?}", env.sh);
        // Every prefiltered level is the same constant.
        for i in 0..LEVELS {
            let t = env.texel(i, 1, 1);
            assert!(close(t[2], 2., 1e-3), "level {i}: {t:?}");
        }
    }

    #[test]
    fn sh_matches_a_brute_force_cosine_integral() {
        let env = sunset();
        let img = Img {
            w: WIDTH as usize,
            h: HEIGHT as usize,
            px: (0..(WIDTH * HEIGHT) as usize)
                .map(|i| env.texel(0, i % WIDTH as usize, i / WIDTH as usize))
                .collect(),
        };
        let normals = [
            [0., 1., 0.],
            [0., -1., 0.],
            normalize([1., 0.2, -1.]),
            [0., 0., 1.],
        ];
        let exact = normals.map(|n| {
            let mut e = [0f32; 3];
            for y in 0..img.h {
                let t0 = y as f32 * PI / img.h as f32;
                let t1 = (y + 1) as f32 * PI / img.h as f32;
                let d_omega = 2. * PI / img.w as f32 * (t0.cos() - t1.cos());
                for x in 0..img.w {
                    let d = direction(
                        (x as f32 + 0.5) / img.w as f32,
                        (y as f32 + 0.5) / img.h as f32,
                    );
                    let c = dot(d, n).max(0.) * d_omega;
                    let p = img.px[y * img.w + x];
                    e = [e[0] + p[0] * c, e[1] + p[1] * c, e[2] + p[2] * c];
                }
            }
            e
        });
        // Order-2 SH approximates the clamped cosine: a few percent of the
        // brightest irradiance (the bright horizon rings into the dim ground).
        let max = exact.iter().flatten().fold(0f32, |a, &b| a.max(b));
        for (n, e) in normals.iter().zip(exact) {
            let sh = env.irradiance(*n);
            for c in 0..3 {
                assert!(
                    (sh[c] - e[c]).abs() < 0.07 * max,
                    "{n:?}: SH {sh:?} vs {e:?}"
                );
            }
        }
    }

    #[test]
    fn brighter_sky_lights_from_above() {
        let (w, h) = (64, 32);
        let rgb: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                if i / w < h / 2 {
                    [2., 2., 2.]
                } else {
                    [0.2, 0.2, 0.2]
                }
            })
            .collect();
        let env = from_equirect(w as u32, h as u32, &rgb);
        let up = env.irradiance([0., 1., 0.]);
        let down = env.irradiance([0., -1., 0.]);
        let side = env.irradiance([1., 0., 0.]);
        assert!(
            up[0] > side[0] && side[0] > down[0],
            "{up:?} {side:?} {down:?}"
        );
        // A hemisphere of L above gives π·L·(1 + cos)/2 … ≈ 2π up (SH-smoothed).
        assert!(close(up[0], 2. * PI, 0.1), "{up:?}");
        for p in [studio(), neutral(), sunset()] {
            assert!(p.irradiance([0., 1., 0.])[1] > p.irradiance([0., -1., 0.])[1]);
        }
    }

    /// Solid-angle-weighted mean and variance of a level's luminance.
    fn stats(env: &EnvMap, level: usize) -> (f32, f32) {
        let (w, h) = (
            (env.width >> level) as usize,
            (env.height >> level) as usize,
        );
        let (mut sum, mut sq, mut total) = (0f64, 0f64, 0f64);
        for y in 0..h {
            let t0 = y as f64 * std::f64::consts::PI / h as f64;
            let t1 = (y + 1) as f64 * std::f64::consts::PI / h as f64;
            let d = t0.cos() - t1.cos();
            for x in 0..w {
                let c = env.texel(level, x, y);
                let l = (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]) as f64;
                sum += l * d;
                sq += l * l * d;
                total += d;
            }
        }
        let mean = sum / total;
        (mean as f32, (sq / total - mean * mean) as f32)
    }

    #[test]
    fn prefiltered_levels_keep_the_mean_and_get_smoother() {
        // A dim room with a bright patch: the hardest case for importance sampling.
        let (w, h) = (128, 64);
        let rgb: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if (40..48).contains(&x) && (20..26).contains(&y) {
                    [40., 30., 20.]
                } else {
                    [0.3, 0.3, 0.3]
                }
            })
            .collect();
        for env in [
            from_equirect(w as u32, h as u32, &rgb),
            studio(),
            sunset(),
            neutral(),
        ] {
            assert_eq!(env.levels.len(), LEVELS);
            let (mean0, mut var) = stats(&env, 0);
            for i in 1..LEVELS {
                let (mean, v) = stats(&env, i);
                assert!(
                    close(mean, mean0, 0.12),
                    "level {i}: mean {mean} vs {mean0}"
                );
                assert!(v < var, "level {i}: variance {v} ≥ {var}");
                var = v;
            }
            // Alpha is 1 everywhere, every texel finite.
            for level in &env.levels {
                assert!(
                    level
                        .iter()
                        .all(|t| t[3] == 0x3c00 && (t[0] & 0x7c00) != 0x7c00)
                );
            }
        }
    }

    #[test]
    fn encode_layout() {
        let env = neutral();
        let bytes = env.encode();
        let texels: usize = (0..LEVELS).map(|i| (256 >> i) * (128 >> i)).sum();
        assert_eq!(texels, 43_680);
        assert_eq!(bytes.len(), 4 + 4 * 4 + 27 * 4 + texels * 8);
        assert_eq!(&bytes[..4], b"P3DE");
        let word = |i: usize| u32::from_le_bytes(bytes[4 + 4 * i..8 + 4 * i].try_into().unwrap());
        assert_eq!([word(0), word(1), word(2), word(3)], [1, 256, 128, 6]);
        assert_eq!(
            f32::from_le_bytes(bytes[20..24].try_into().unwrap()),
            env.sh[0]
        );
        let first = 20 + 27 * 4;
        assert_eq!(
            u16::from_le_bytes([bytes[first], bytes[first + 1]]),
            env.levels[0][0][0]
        );
        assert_eq!(
            u16::from_le_bytes([bytes[first + 6], bytes[first + 7]]),
            0x3c00
        );
        assert_eq!(EnvMap::decode(&bytes), Some(env));
        assert_eq!(EnvMap::decode(&bytes[..bytes.len() - 1]), None);
        assert_eq!(EnvMap::decode(b"P3DE"), None);
    }

    #[test]
    fn presets_are_cached_distinct_and_fast() {
        let start = Instant::now();
        let built = [studio(), neutral(), sunset()];
        let first = start.elapsed();
        let start = Instant::now();
        let again = [studio(), neutral(), sunset()];
        let cached = start.elapsed();
        assert_eq!(built, again);
        assert!(STUDIO.get().is_some() && NEUTRAL.get().is_some() && SUNSET.get().is_some());
        assert_ne!(built[0], built[1]);
        assert_ne!(built[1], built[2]);
        // Timing of one fresh build (the presets above may have been built by another test).
        let start = Instant::now();
        let fresh = build(rasterize(studio_radiance));
        let one = start.elapsed();
        assert_eq!(fresh, built[0]);
        eprintln!("presets: first {first:?}, cached {cached:?}, one build {one:?}");
        let limit = if cfg!(debug_assertions) { 5000 } else { 200 };
        assert!(
            one.as_millis() < limit,
            "building an environment took {one:?}"
        );
        // Looks: the sunset's sun is far brighter than anything in the studio;
        // the studio's key box is above and in front; neutral is soft.
        let max = |e: &EnvMap| {
            e.levels[0]
                .iter()
                .map(|t| f16_to_f32(t[0]))
                .fold(0., f32::max)
        };
        assert!(max(&built[2]) > 50.);
        assert!(max(&built[0]) > 3. && max(&built[0]) < 10.);
        assert!(max(&built[1]) < 1.);
        let s = &built[0];
        assert!(s.irradiance([0., 0.6, 0.8])[1] > 2. * s.irradiance([0., 0., -1.])[1]);
    }

    fn hdr_header(w: usize, h: usize) -> Vec<u8> {
        format!(
            "#?RADIANCE\n# made by a test\nFORMAT=32-bit_rle_rgbe\nEXPOSURE=2.0\n\n-Y {h} +X {w}\n"
        )
        .into_bytes()
    }

    /// Pixel (x, y) of the test image.
    fn test_pixel(x: usize, y: usize) -> [u8; 4] {
        if y == 0 {
            [128, 128, 128, 129] // 1.0 on the top row.
        } else if x < 10 {
            [200, 100, 50, 130] // Run-length friendly.
        } else {
            [(x * 7) as u8, (y * 13) as u8, 128, 128] // Varying.
        }
    }

    fn rle_scanline(row: &[[u8; 4]]) -> Vec<u8> {
        let w = row.len();
        let mut out = vec![2, 2, (w >> 8) as u8, w as u8];
        for c in 0..4 {
            let mut x = 0;
            while x < w {
                let v = row[x][c];
                let mut run = 1;
                while x + run < w && run < 127 && row[x + run][c] == v {
                    run += 1;
                }
                if run >= 3 {
                    out.extend_from_slice(&[128 + run as u8, v]);
                    x += run;
                } else {
                    let n = (w - x).min(128).min(4);
                    out.push(n as u8);
                    out.extend(row[x..x + n].iter().map(|p| p[c]));
                    x += n;
                }
            }
        }
        out
    }

    #[test]
    fn hdr_flat_and_rle() {
        let (w, h) = (32, 16);
        let rows: Vec<Vec<[u8; 4]>> = (0..h)
            .map(|y| (0..w).map(|x| test_pixel(x, y)).collect())
            .collect();
        let expected: Vec<[u8; 4]> = rows.concat();

        let mut flat = hdr_header(w, h);
        flat.extend(expected.iter().flatten());
        let mut rle = hdr_header(w, h);
        for row in &rows {
            rle.extend(rle_scanline(row));
        }
        // Old-style RLE: the 10 equal pixels as one pixel and a run of 9.
        let mut old = hdr_header(w, h);
        for row in &rows {
            if row[0] == row[9] && row[0] != row[10] {
                old.extend(row[0]);
                old.extend([1, 1, 1, 9]);
                old.extend(row[10..].iter().flatten());
            } else {
                old.extend(row.iter().flatten());
            }
        }
        assert!(rle.len() < flat.len() && old.len() < flat.len());
        for (name, file) in [("flat", &flat), ("rle", &rle), ("old", &old)] {
            let (dw, dh, px) = decode_hdr(file).unwrap_or_else(|| panic!("{name}"));
            assert_eq!((dw, dh), (w, h));
            assert_eq!(px, expected, "{name}");
        }
        assert_eq!(rgbe([128, 128, 128, 129]), [1., 1., 1.]);
        assert_eq!(rgbe([200, 100, 50, 130]), [3.125, 1.5625, 0.78125]);
        assert_eq!(rgbe([255, 0, 0, 0]), [0.; 3]);

        // Bottom-up files are flipped; #?RGBE and CRLF headers are accepted.
        let mut up = b"#?RGBE\r\nFORMAT=32-bit_rle_rgbe\r\n\r\n".to_vec();
        up.extend(format!("+Y {h} +X {w}\n").bytes());
        for row in rows.iter().rev() {
            up.extend(row.iter().flatten());
        }
        assert_eq!(decode_hdr(&up).unwrap().2, expected);

        // The whole path: the top row's 1.0 stays at the top of the map.
        let env = from_hdr(&rle).unwrap();
        assert_eq!(env, from_hdr(&flat).unwrap());
        assert!(env.texel(0, 5, 0)[0] > 0.9 && env.texel(0, 5, 0)[0] < 1.1);
    }

    #[test]
    fn hdr_rejects_malformed_input() {
        let (w, h) = (16, 8);
        let row: Vec<[u8; 4]> = (0..w).map(|x| test_pixel(x, 3)).collect();
        let mut rle = hdr_header(w, h);
        let mut flat = hdr_header(w, h);
        for _ in 0..h {
            rle.extend(rle_scanline(&row));
            flat.extend(row.iter().flatten());
        }
        assert!(from_hdr(&rle).is_some() && from_hdr(&flat).is_some());
        // Every truncation fails cleanly.
        for file in [&rle, &flat] {
            for n in 0..file.len() {
                assert!(from_hdr(&file[..n]).is_none(), "truncated at {n}");
            }
        }
        let bad_headers: [&[u8]; 9] = [
            b"",
            b"#?RADIANCE",
            b"P6\n16 8\n255\n",
            b"#?RADIANCE\nFORMAT=32-bit_rle_xyze\n\n-Y 8 +X 16\n",
            b"#?RADIANCE\n\n+X 16 -Y 8\n",
            b"#?RADIANCE\n\n-Y 0 +X 16\n",
            b"#?RADIANCE\n\n-Y 8 +X huge\n",
            b"#?RADIANCE\n\n-Y 99999 +X 99999\n",
            b"#?RADIANCE\n\n-Y 8 +X 16 extra\n",
        ];
        for b in bad_headers {
            assert!(from_hdr(b).is_none(), "{:?}", String::from_utf8_lossy(b));
        }
        // An RLE scanline whose width disagrees, a zero-length literal run,
        // a run past the end, an old-style run with nothing before it.
        let head = hdr_header(w, h);
        let cases: [&[u8]; 4] = [
            &[2, 2, 0, 17],
            &[2, 2, 0, 16, 0],
            &[2, 2, 0, 16, 128 + 17, 5],
            &[1, 1, 1, 4],
        ];
        for case in cases {
            let mut f = head.clone();
            f.extend_from_slice(case);
            f.extend(std::iter::repeat_n(7u8, 4096));
            assert!(from_hdr(&f).is_none(), "{case:?}");
        }
        // Garbage after a valid header, and random bit flips, never panic.
        let mut seed = 0x9e37_79b9u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for _ in 0..300 {
            let mut f = if next() % 2 == 0 {
                rle.clone()
            } else {
                flat.clone()
            };
            for _ in 0..1 + next() % 8 {
                let i = next() as usize % f.len();
                f[i] ^= 1 << (next() % 8);
            }
            let _ = decode_hdr(&f);
            let mut g = head.clone();
            g.extend((0..next() % 600).map(|_| next() as u8));
            let _ = decode_hdr(&g);
        }
    }

    #[test]
    fn from_equirect_handles_odd_input() {
        // Too few texels, zero sizes: black.
        for env in [from_equirect(4, 4, &[[1.; 3]; 3]), from_equirect(0, 0, &[])] {
            assert_eq!(env.width, 256);
            assert!(env.levels[0].iter().all(|t| t[..3] == [0, 0, 0]));
        }
        // NaN, negative, huge values are tamed.
        let env = from_equirect(2, 1, &[[f32::NAN, -1., 1e30], [f32::INFINITY, 1., 1.]]);
        for level in &env.levels {
            assert!(
                level
                    .iter()
                    .all(|t| t.iter().all(|&c| c & 0x7c00 != 0x7c00 && c & 0x8000 == 0))
            );
        }
        assert!(env.sh.iter().all(|c| c.is_finite()));
        // A large input is box-filtered: a 1024×512 checkerboard averages out.
        let (w, h) = (1024, 512);
        let rgb: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                if (i % w + i / w) % 2 == 0 {
                    [2.; 3]
                } else {
                    [0.; 3]
                }
            })
            .collect();
        let env = from_equirect(w as u32, h as u32, &rgb);
        assert!(
            env.levels[0]
                .iter()
                .all(|t| (f16_to_f32(t[0]) - 1.).abs() < 1e-3)
        );
        // A non-2:1 input still maps top to top.
        let rgb: Vec<[f32; 3]> = (0..30 * 20)
            .map(|i| if i / 30 < 10 { [1.; 3] } else { [0.; 3] })
            .collect();
        let env = from_equirect(30, 20, &rgb);
        assert!(env.texel(0, 0, 0)[0] > 0.99 && env.texel(0, 0, 127)[0] < 0.01);
    }
}
