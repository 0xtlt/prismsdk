//! The Windows resources of an app's executable, written as a `.res` file
//! without a resource compiler:
//!
//! - its icon, from a PNG (`[app] icon`, 1024×1024) or an `.ico`: 16 to 256
//!   pixels, what Explorer, the taskbar, Alt+Tab and the title bar show;
//! - its version information (`VERSIONINFO`): the name Task Manager and the
//!   taskbar show (`FileDescription`), the version, the publisher, what
//!   Explorer's Details tab lists;
//! - its manifest (see [`winappsdk::app_manifest`](crate::winappsdk::app_manifest)).
//!
//! [`winappsdk::link_app`](crate::winappsdk::link_app) links it: MSVC's
//! `link.exe` and LLVM's `lld` take a `.res` file as it is, GNU `ld` once
//! `windres` has turned it into an object.

use std::path::PathBuf;

/// What the executable says about the app, from the variables `prism`
/// sets for cargo (`PRISM_APP_NAME`, `PRISM_VERSION`…, from `Prism.toml`)
/// or else the package's own `Cargo.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppInfo {
    /// The display name: `ProductName` and `FileDescription` (the name
    /// Task Manager and the taskbar show).
    pub name: String,
    /// The version as written (`1.2.0`): `ProductVersion`, `FileVersion`.
    pub version: String,
    /// The build number: the binary version's fourth part.
    pub build: Option<String>,
    /// Reverse-DNS id (`com.example.app`): the manifest's assembly name.
    pub id: String,
    /// `CompanyName`.
    pub publisher: Option<String>,
    /// `Comments`.
    pub description: Option<String>,
    /// The cargo package: `InternalName`, `OriginalFilename` (`<name>.exe`).
    pub package: String,
    /// A PNG or `.ico` file for the icon.
    pub icon: Option<PathBuf>,
}

impl AppInfo {
    /// From `PRISM_APP_NAME`, `PRISM_VERSION`, `PRISM_BUILD`,
    /// `PRISM_BUNDLE_ID`, `PRISM_PUBLISHER`, `PRISM_DESCRIPTION` and
    /// `PRISM_ICON`, else the package's name, version, authors and
    /// description. Run from a build script (it tells cargo to watch them).
    pub fn from_env() -> Self {
        let var = |k: &str| {
            println!("cargo:rerun-if-env-changed={k}");
            std::env::var(k).ok().filter(|v| !v.trim().is_empty())
        };
        let cargo = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let package = cargo("CARGO_PKG_NAME").unwrap_or_else(|| "app".into());
        AppInfo {
            name: var("PRISM_APP_NAME").unwrap_or_else(|| package.clone()),
            version: var("PRISM_VERSION")
                .or_else(|| cargo("CARGO_PKG_VERSION"))
                .unwrap_or_else(|| "0.1.0".into()),
            build: var("PRISM_BUILD"),
            id: var("PRISM_BUNDLE_ID")
                .unwrap_or_else(|| format!("dev.prism.{}", package.replace('_', "-"))),
            // `Jane Doe <jane@example.com>:Bob` → `Jane Doe`.
            publisher: var("PRISM_PUBLISHER").or_else(|| {
                cargo("CARGO_PKG_AUTHORS")?
                    .split(':')
                    .next()
                    .map(|a| a.split('<').next().unwrap_or(a).trim().to_string())
                    .filter(|a| !a.is_empty())
            }),
            description: var("PRISM_DESCRIPTION").or_else(|| cargo("CARGO_PKG_DESCRIPTION")),
            icon: var("PRISM_ICON").map(PathBuf::from),
            package,
        }
    }

    /// The binary version, four numbers up to 65535: the version's first
    /// three (`1.2.0-beta.1` → 1, 2, 0) and the build number, else the
    /// version's fourth.
    pub fn version_parts(&self) -> [u16; 4] {
        let core = self.version.split(['-', '+']).next().unwrap_or_default();
        let mut parts = [0u16; 4];
        for (part, text) in parts.iter_mut().zip(core.split('.')) {
            *part = text.trim().parse().unwrap_or(0);
        }
        if let Some(build) = self.build.as_deref().and_then(|b| b.trim().parse().ok()) {
            parts[3] = build;
        }
        parts
    }

    /// `1.2.0.7`.
    pub fn version_quad(&self) -> String {
        let [a, b, c, d] = self.version_parts();
        format!("{a}.{b}.{c}.{d}")
    }

    /// The id as an assembly name: letters, digits, `.`, `-` and `_`.
    pub fn assembly_name(&self) -> String {
        let name: String = self
            .id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let name = name.trim_matches('.').to_string();
        if name.is_empty() {
            "Prism.App".into()
        } else {
            name
        }
    }
}

/// `RT_ICON`: one image of an icon.
pub const RT_ICON: u16 = 3;
/// `RT_GROUP_ICON`: an icon's directory of images.
pub const RT_GROUP_ICON: u16 = 14;
/// `RT_VERSION`: `VS_VERSIONINFO`.
pub const RT_VERSION: u16 = 16;
/// `RT_MANIFEST`: the side-by-side manifest (id 1 for an executable).
pub const RT_MANIFEST: u16 = 24;

/// The app icon's resource id: `LoadIconW(instance, MAKEINTRESOURCE(1))`.
/// Explorer shows the executable's first icon group.
pub const ICON_ID: u16 = 1;

/// English (United States), what `rc.exe` and `windres` use by default.
const LANG_EN_US: u16 = 0x0409;

/// One resource of a `.res` file, named by numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub kind: u16,
    pub id: u16,
    pub data: Vec<u8>,
}

impl Resource {
    /// The manifest of an executable (`RT_MANIFEST` 1).
    pub fn manifest(xml: &str) -> Self {
        Resource {
            kind: RT_MANIFEST,
            id: 1,
            data: xml.as_bytes().to_vec(),
        }
    }
}

fn pad4(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn u16le(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn u32le(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// A 32-bit `.res` file: the empty entry it starts with, then each
/// resource's `RESOURCEHEADER` (numeric type and name) and data.
pub fn res_file(resources: &[Resource]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut entry = |kind: u16, id: u16, lang: u16, data: &[u8]| {
        // Memory flags, as rc.exe writes them (Win32 ignores them):
        // MOVEABLE | DISCARDABLE for images, | PURE for the rest.
        let flags = match kind {
            0 => 0,
            RT_ICON => 0x1010,
            RT_GROUP_ICON => 0x1030,
            _ => 0x0030,
        };
        u32le(&mut out, data.len() as u32);
        u32le(&mut out, 32); // header size
        u16le(&mut out, 0xFFFF);
        u16le(&mut out, kind);
        u16le(&mut out, 0xFFFF);
        u16le(&mut out, id);
        u32le(&mut out, 0); // data version
        u16le(&mut out, flags);
        u16le(&mut out, lang);
        u32le(&mut out, 0); // version
        u32le(&mut out, 0); // characteristics
        out.extend_from_slice(data);
        pad4(&mut out);
    };
    entry(0, 0, 0, &[]);
    for r in resources {
        entry(r.kind, r.id, LANG_EN_US, &r.data);
    }
    out
}

// --- Version information --------------------------------------------------------------

/// One block of `VS_VERSIONINFO`: a key, a binary or text value, children.
struct Block {
    key: &'static str,
    value: Value,
    children: Vec<Block>,
}

enum Value {
    None,
    Binary(Vec<u8>),
    Text(String),
}

fn utf16z(out: &mut Vec<u8>, s: &str) -> usize {
    let mut n = 0;
    for unit in s.encode_utf16().chain([0]) {
        u16le(out, unit);
        n += 1;
    }
    n
}

impl Block {
    fn text(key: &'static str, value: &str) -> Self {
        Block {
            key,
            value: Value::Text(value.to_string()),
            children: Vec::new(),
        }
    }

    /// `wLength`, `wValueLength`, `wType`, the key, the value and the
    /// children, each 32-bit aligned. The length leaves out the padding
    /// after the last byte, like rc.exe.
    fn write(&self) -> Vec<u8> {
        let mut b = vec![0u8; 6];
        utf16z(&mut b, self.key);
        pad4(&mut b);
        let (value_len, kind) = match &self.value {
            Value::None => (0, 1),
            Value::Binary(v) => {
                b.extend_from_slice(v);
                (v.len(), 0)
            }
            // In characters, the terminating null included.
            Value::Text(s) => (utf16z(&mut b, s), 1),
        };
        for child in &self.children {
            pad4(&mut b);
            b.extend(child.write());
        }
        let len = b.len() as u16;
        b[0..2].copy_from_slice(&len.to_le_bytes());
        b[2..4].copy_from_slice(&(value_len as u16).to_le_bytes());
        b[4..6].copy_from_slice(&(kind as u16).to_le_bytes());
        b
    }
}

/// `VS_VERSIONINFO`: `VS_FIXEDFILEINFO` (the binary version, an
/// application for Windows NT) and the strings in English (Unicode).
pub fn version_info(app: &AppInfo) -> Vec<u8> {
    let [a, b, c, d] = app.version_parts();
    let ms = (u32::from(a) << 16) | u32::from(b);
    let ls = (u32::from(c) << 16) | u32::from(d);
    let mut fixed = Vec::with_capacity(52);
    for v in [
        0xFEEF_04BD, // signature
        0x0001_0000, // structure version
        ms,          // file version
        ls,
        ms, // product version
        ls,
        0x3F,        // VS_FFI_FILEFLAGSMASK
        0,           // flags
        0x0004_0004, // VOS_NT_WINDOWS32
        0x1,         // VFT_APP
        0,           // subtype
        0,           // date
        0,
    ] {
        u32le(&mut fixed, v);
    }
    let mut strings = vec![
        Block::text("FileDescription", &app.name),
        Block::text("FileVersion", &app.version),
        Block::text("InternalName", &app.package),
        Block::text("OriginalFilename", &format!("{}.exe", app.package)),
        Block::text("ProductName", &app.name),
        Block::text("ProductVersion", &app.version),
    ];
    if let Some(p) = &app.publisher {
        strings.insert(0, Block::text("CompanyName", p));
    }
    if let Some(d) = &app.description {
        strings.insert(0, Block::text("Comments", d));
    }
    let mut translation = Vec::new();
    u16le(&mut translation, LANG_EN_US);
    u16le(&mut translation, 1200); // Unicode
    Block {
        key: "VS_VERSION_INFO",
        value: Value::Binary(fixed),
        children: vec![
            Block {
                key: "StringFileInfo",
                value: Value::None,
                children: vec![Block {
                    key: "040904b0",
                    value: Value::None,
                    children: strings,
                }],
            },
            Block {
                key: "VarFileInfo",
                value: Value::None,
                children: vec![Block {
                    key: "Translation",
                    value: Value::Binary(translation),
                    children: Vec::new(),
                }],
            },
        ],
    }
    .write()
}

// --- Icon ---------------------------------------------------------------------------

/// The sizes of the app icon: small icons at 100 %, 125 %, 150 % and 200 %
/// (16, 20, 24, 32), large ones (32, 40, 48, 64) and Explorer's largest
/// (256).
pub const ICON_SIZES: [u32; 8] = [256, 64, 48, 40, 32, 24, 20, 16];

/// The icon's images (`RT_ICON` 1…n) and their directory (`RT_GROUP_ICON`
/// [`ICON_ID`]), from a PNG (scaled to [`ICON_SIZES`]) or an `.ico` file
/// (its images as they are).
pub fn icon(file: &[u8]) -> Result<Vec<Resource>, String> {
    let images: Vec<IconImage> = if file.starts_with(b"\x89PNG") {
        let (w, h, rgba) = decode_png(file)?;
        let (side, square) = square(w, h, &rgba);
        ICON_SIZES
            .iter()
            .map(|&size| {
                let pixels = resize(&square, side, size);
                let data = if size >= 256 {
                    encode_png(size, &pixels)?
                } else {
                    dib(size, &pixels)
                };
                Ok(IconImage {
                    width: size,
                    height: size,
                    bit_count: 32,
                    data,
                })
            })
            .collect::<Result<_, String>>()?
    } else if file.starts_with(&[0, 0, 1, 0]) {
        read_ico(file)?
    } else {
        return Err("not a PNG or .ico file".into());
    };
    let mut resources = Vec::new();
    let mut group = Vec::new();
    u16le(&mut group, 0); // reserved
    u16le(&mut group, 1); // icons
    u16le(&mut group, images.len() as u16);
    for (i, img) in images.into_iter().enumerate() {
        let id = i as u16 + 1;
        // 256 is written 0.
        group.push(img.width.min(256) as u8);
        group.push(img.height.min(256) as u8);
        group.push(0); // colors
        group.push(0); // reserved
        u16le(&mut group, 1); // planes
        u16le(&mut group, img.bit_count);
        u32le(&mut group, img.data.len() as u32);
        u16le(&mut group, id);
        resources.push(Resource {
            kind: RT_ICON,
            id,
            data: img.data,
        });
    }
    resources.push(Resource {
        kind: RT_GROUP_ICON,
        id: ICON_ID,
        data: group,
    });
    Ok(resources)
}

struct IconImage {
    width: u32,
    height: u32,
    bit_count: u16,
    data: Vec<u8>,
}

/// The images of an `.ico` file (`ICONDIR`).
fn read_ico(file: &[u8]) -> Result<Vec<IconImage>, String> {
    let u16_at = |i: usize| file.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |i: usize| {
        file.get(i..i + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let bad = || "a damaged .ico file".to_string();
    let count = u16_at(4).ok_or_else(bad)? as usize;
    (0..count)
        .map(|i| {
            let e = 6 + i * 16;
            let entry = file.get(e..e + 16).ok_or_else(bad)?;
            let size = u32_at(e + 8).ok_or_else(bad)? as usize;
            let offset = u32_at(e + 12).ok_or_else(bad)? as usize;
            let data = file.get(offset..offset + size).ok_or_else(bad)?;
            let side = |b: u8| if b == 0 { 256 } else { u32::from(b) };
            Ok(IconImage {
                width: side(entry[0]),
                height: side(entry[1]),
                bit_count: u16_at(e + 6).ok_or_else(bad)?.max(1),
                data: data.to_vec(),
            })
        })
        .collect()
}

/// A PNG as 8-bit RGBA.
fn decode_png(file: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(file));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| format!("PNG: {e}"))?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("PNG: too large")?];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("PNG: {e}"))?;
    let (w, h) = (frame.width, frame.height);
    let px = &buf[..frame.buffer_size()];
    let rgba: Vec<u8> = match frame.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => px
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[g, a]| [g, g, g, a])
            .collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("PNG: unexpanded palette".into()),
    };
    if w == 0 || h == 0 {
        return Err("PNG: empty image".into());
    }
    Ok((w, h, rgba))
}

fn encode_png(size: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// The image centered in a transparent square, if it is not one.
fn square(w: u32, h: u32, rgba: &[u8]) -> (u32, Vec<u8>) {
    if w == h {
        return (w, rgba.to_vec());
    }
    let side = w.max(h);
    let (dx, dy) = ((side - w) / 2, (side - h) / 2);
    let mut out = vec![0u8; (side * side * 4) as usize];
    for y in 0..h {
        let src = (y * w * 4) as usize;
        let dst = (((y + dy) * side + dx) * 4) as usize;
        out[dst..dst + (w * 4) as usize].copy_from_slice(&rgba[src..src + (w * 4) as usize]);
    }
    (side, out)
}

/// Each output pixel's source pixels along one axis and how much of each
/// it covers (an area average: exact for any ratio).
fn weights(from: u32, to: u32) -> Vec<Vec<(usize, f32)>> {
    let scale = from as f64 / to as f64;
    (0..to)
        .map(|i| {
            let (start, end) = (i as f64 * scale, (i + 1) as f64 * scale);
            let mut w = Vec::new();
            let mut s = start.floor() as usize;
            while (s as f64) < end && s < from as usize {
                let cover = (end.min(s as f64 + 1.) - start.max(s as f64)) as f32;
                if cover > 0. {
                    w.push((s, cover));
                }
                s += 1;
            }
            let total: f32 = w.iter().map(|(_, c)| c).sum();
            w.iter_mut().for_each(|(_, c)| *c /= total);
            w
        })
        .collect()
}

/// A square RGBA image scaled to `to`×`to`, averaged with premultiplied
/// alpha (no dark fringes at transparent edges).
fn resize(rgba: &[u8], from: u32, to: u32) -> Vec<u8> {
    if from == to {
        return rgba.to_vec();
    }
    let premul: Vec<[f32; 4]> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[r, g, b, a]| {
            let a = f32::from(a) / 255.;
            [f32::from(r) * a, f32::from(g) * a, f32::from(b) * a, a]
        })
        .collect();
    let w = weights(from, to);
    let n = from as usize;
    // Rows first, then columns.
    let mut rows = vec![[0f32; 4]; n * to as usize];
    for y in 0..n {
        for (x, ws) in w.iter().enumerate() {
            let mut acc = [0f32; 4];
            for &(s, k) in ws {
                let p = premul[y * n + s];
                for c in 0..4 {
                    acc[c] += p[c] * k;
                }
            }
            rows[y * to as usize + x] = acc;
        }
    }
    let mut out = Vec::with_capacity((to * to * 4) as usize);
    for ws in &w {
        for x in 0..to as usize {
            let mut acc = [0f32; 4];
            for &(s, k) in ws {
                let p = rows[s * to as usize + x];
                for c in 0..4 {
                    acc[c] += p[c] * k;
                }
            }
            let a = acc[3];
            let un = |v: f32| {
                if a > 0. {
                    (v / a).round().clamp(0., 255.) as u8
                } else {
                    0
                }
            };
            out.extend([un(acc[0]), un(acc[1]), un(acc[2]), (a * 255.).round() as u8]);
        }
    }
    out
}

/// An icon image as a DIB: `BITMAPINFOHEADER` (twice the height: the
/// colors, then the mask), BGRA rows bottom-up, then the 1-bit mask
/// (transparent pixels set), rows padded to 32 bits.
fn dib(size: u32, rgba: &[u8]) -> Vec<u8> {
    let mask_row = size.div_ceil(32) as usize * 4;
    let image = (size * size * 4) as usize + mask_row * size as usize;
    let mut out = Vec::with_capacity(40 + image);
    u32le(&mut out, 40);
    u32le(&mut out, size);
    u32le(&mut out, size * 2);
    u16le(&mut out, 1); // planes
    u16le(&mut out, 32); // bits per pixel
    u32le(&mut out, 0); // BI_RGB
    u32le(&mut out, image as u32);
    for _ in 0..4 {
        u32le(&mut out, 0);
    }
    let s = size as usize;
    for y in (0..s).rev() {
        for x in 0..s {
            let p = &rgba[(y * s + x) * 4..][..4];
            out.extend([p[2], p[1], p[0], p[3]]);
        }
    }
    for y in (0..s).rev() {
        let mut row = vec![0u8; mask_row];
        for x in 0..s {
            if rgba[(y * s + x) * 4 + 3] == 0 {
                row[x / 8] |= 0x80 >> (x % 8);
            }
        }
        out.extend(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> AppInfo {
        AppInfo {
            name: "Counter".into(),
            version: "1.2.3-beta.1".into(),
            build: Some("7".into()),
            id: "dev.prism.counter".into(),
            publisher: Some("Prism".into()),
            description: None,
            package: "counter".into(),
            icon: None,
        }
    }

    #[test]
    fn versions_have_four_parts() {
        assert_eq!(app().version_parts(), [1, 2, 3, 7]);
        assert_eq!(app().version_quad(), "1.2.3.7");
        let plain = AppInfo {
            version: "2.0".into(),
            build: None,
            ..app()
        };
        assert_eq!(plain.version_parts(), [2, 0, 0, 0]);
        let four = AppInfo {
            version: "1.2.3.4".into(),
            build: Some("x".into()),
            ..app()
        };
        assert_eq!(four.version_parts(), [1, 2, 3, 4]);
        let id = AppInfo {
            id: "com.example.my app!".into(),
            ..app()
        };
        assert_eq!(id.assembly_name(), "com.example.my_app_");
    }

    /// Reads the `RESOURCEHEADER`s back: (type, id, data).
    fn entries(res: &[u8]) -> Vec<(u16, u16, Vec<u8>)> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < res.len() {
            let n = u32::from_le_bytes(res[i..i + 4].try_into().unwrap()) as usize;
            let header = u32::from_le_bytes(res[i + 4..i + 8].try_into().unwrap()) as usize;
            assert_eq!(header, 32);
            let kind = u16::from_le_bytes([res[i + 10], res[i + 11]]);
            let id = u16::from_le_bytes([res[i + 14], res[i + 15]]);
            out.push((kind, id, res[i + 32..i + 32 + n].to_vec()));
            i += (32 + n).next_multiple_of(4);
        }
        out
    }

    #[test]
    fn res_files_start_empty_and_align_entries() {
        let res = res_file(&[
            Resource::manifest("<x/>"),
            Resource {
                kind: RT_VERSION,
                id: 1,
                data: vec![1, 2, 3, 4, 5],
            },
        ]);
        assert_eq!(
            &res[..32],
            &[
                0, 0, 0, 0, 32, 0, 0, 0, 0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0
            ]
        );
        let e = entries(&res);
        assert_eq!(e.len(), 3);
        assert_eq!((e[1].0, e[1].1, e[1].2.as_slice()), (24, 1, &b"<x/>"[..]));
        assert_eq!((e[2].0, e[2].2.len()), (16, 5));
        assert_eq!(res.len() % 4, 0);
    }

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn version_info_has_the_fixed_info_and_strings() {
        let v = version_info(&app());
        assert_eq!(u16::from_le_bytes([v[0], v[1]]) as usize, v.len());
        assert_eq!(u16::from_le_bytes([v[2], v[3]]), 52);
        // Key "VS_VERSION_INFO" + null = 32 bytes, at 6: value at 40.
        assert_eq!(&v[6..36], utf16("VS_VERSION_INFO").as_slice());
        assert_eq!(&v[40..44], &0xFEEF_04BDu32.to_le_bytes());
        // 1.2 / 3.7
        assert_eq!(&v[48..52], &((1u32 << 16) | 2).to_le_bytes());
        assert_eq!(&v[52..56], &((3u32 << 16) | 7).to_le_bytes());
        let find = |s: &str| {
            let needle = utf16(s);
            v.windows(needle.len()).any(|w| w == needle.as_slice())
        };
        for s in [
            "StringFileInfo",
            "040904b0",
            "CompanyName",
            "Prism",
            "FileDescription",
            "Counter",
            "ProductVersion",
            "1.2.3-beta.1",
            "counter.exe",
            "VarFileInfo",
            "Translation",
        ] {
            assert!(find(s), "{s}");
        }
        assert!(!find("Comments"));
    }

    fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut out, w, h);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            e.write_header().unwrap().write_image_data(rgba).unwrap();
        }
        out
    }

    #[test]
    fn icons_from_a_png() {
        // 64×32, opaque red on the left, transparent on the right.
        let (w, h) = (64u32, 32u32);
        let mut rgba = Vec::new();
        for _ in 0..h {
            for x in 0..w {
                rgba.extend(if x < 32 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 0, 0]
                });
            }
        }
        let r = icon(&png(w, h, &rgba)).unwrap();
        assert_eq!(r.len(), ICON_SIZES.len() + 1);
        let group = &r.last().unwrap();
        assert_eq!((group.kind, group.id), (RT_GROUP_ICON, ICON_ID));
        assert_eq!(&group.data[..6], &[0, 0, 1, 0, 8, 0]);
        // The 256 image is a PNG, written as size 0; the others DIBs.
        assert!(r[0].data.starts_with(b"\x89PNG"));
        assert_eq!(group.data[6], 0);
        let small = &r[ICON_SIZES.len() - 1];
        assert_eq!(small.data.len(), 40 + 16 * 16 * 4 + 4 * 16);
        assert_eq!(&small.data[4..12], &[16, 0, 0, 0, 32, 0, 0, 0]);
        // Centered in a square: the top rows (transparent padding) are
        // masked, the middle row red then transparent.
        let px = |x: usize, y_from_top: usize| {
            let row = 15 - y_from_top;
            let i = 40 + (row * 16 + x) * 4;
            small.data[i..i + 4].to_vec()
        };
        assert_eq!(px(0, 8), [0, 0, 255, 255]);
        assert_eq!(px(15, 8), [0, 0, 0, 0]);
        assert_eq!(px(0, 0)[3], 0);
        // Entries name their images.
        let entry = |i: usize| &group.data[6 + i * 14..6 + (i + 1) * 14];
        assert_eq!(entry(7)[0], 16);
        assert_eq!(u16::from_le_bytes([entry(7)[12], entry(7)[13]]), 8);
    }

    #[test]
    fn icons_from_an_ico_keep_their_images() {
        let image = vec![7u8; 10];
        let mut ico = vec![0, 0, 1, 0, 1, 0];
        ico.extend([32, 32, 0, 0, 1, 0, 32, 0]);
        ico.extend(10u32.to_le_bytes());
        ico.extend(22u32.to_le_bytes());
        ico.extend(&image);
        let r = icon(&ico).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].data, image);
        assert_eq!(r[1].data[6], 32);
        assert!(icon(b"GIF89a").is_err());
        assert!(icon(&ico[..20]).is_err());
    }

    #[test]
    fn resizing_averages_areas() {
        // 3×3 → 2×2: each output pixel covers 1.5×1.5 source pixels.
        let rgba: Vec<u8> = (0..9)
            .flat_map(|i| [if i == 4 { 255 } else { 0 }, 0, 0, 255])
            .collect();
        let out = resize(&rgba, 3, 2);
        // The center pixel is a quarter (0.5×0.5 of 2.25) of each output.
        let red = (255. * 0.25 / 2.25f32).round() as u8;
        assert_eq!(out, [red, 0, 0, 255].repeat(4));
        // Transparent pixels do not darken opaque ones.
        let half = [255, 255, 255, 255, 0, 0, 0, 0];
        let out = resize(&[half, half].concat(), 2, 1);
        assert_eq!(out, [255, 255, 255, 128]);
    }
}
