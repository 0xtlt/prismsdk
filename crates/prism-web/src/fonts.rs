//! Fonts an app bundles in `assets/fonts/` become `@font-face` rules of the
//! page stylesheet, so `.font_family("Inter")` works on the web like it does
//! natively (where the platform registers the same files).
//!
//! The family, weight and style come from the font file itself (its `name`
//! and `OS/2` tables) for TrueType and OpenType fonts; WOFF and WOFF2 files
//! (compressed) are described by their name: `Inter-SemiBoldItalic.woff2` is
//! family `Inter`, weight 600, italic. When the same face exists as a WOFF2
//! (or WOFF) and as a TTF/OTF, the browser gets the compressed one first.

use std::collections::BTreeMap;

/// What a font file holds.
#[derive(Clone, Debug, PartialEq)]
pub struct FontFace {
    /// Family name (`Inter`).
    pub family: String,
    /// CSS weight, or a range for variable fonts (`100 900`).
    pub weight: String,
    /// Italic face.
    pub italic: bool,
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
        *b.get(at + 2)?,
        *b.get(at + 3)?,
    ]))
}

/// The offset of table `tag` in a TrueType / OpenType font.
fn table(b: &[u8], tag: &[u8; 4]) -> Option<(usize, usize)> {
    let tables = be16(b, 4)? as usize;
    (0..tables).find_map(|i| {
        let rec = 12 + i * 16;
        (b.get(rec..rec + 4)? == tag)
            .then(|| Some((be32(b, rec + 8)? as usize, be32(b, rec + 12)? as usize)))
            .flatten()
    })
}

/// Family name from the `name` table: the typographic family (16) when
/// there is one, else the family (1); Windows Unicode or Mac Roman records.
fn family(b: &[u8]) -> Option<String> {
    let (at, _) = table(b, b"name")?;
    let count = be16(b, at + 2)? as usize;
    let strings = at + be16(b, at + 4)? as usize;
    let mut best: Option<(u16, String)> = None;
    for i in 0..count {
        let rec = at + 6 + i * 12;
        let (platform, name_id) = (be16(b, rec)?, be16(b, rec + 6)?);
        if name_id != 1 && name_id != 16 {
            continue;
        }
        let (len, off) = (be16(b, rec + 8)? as usize, be16(b, rec + 10)? as usize);
        let raw = b.get(strings + off..strings + off + len)?;
        let text = match platform {
            // Windows / Unicode: UTF-16 big endian.
            0 | 3 => {
                let units: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_be_bytes(*c))
                    .collect();
                String::from_utf16(&units).ok()?
            }
            // Macintosh Roman: ASCII is enough for family names.
            1 => raw.iter().map(|&c| c as char).collect(),
            _ => continue,
        };
        if text.is_empty() {
            continue;
        }
        if best.as_ref().is_none_or(|(id, _)| name_id > *id) {
            best = Some((name_id, text));
        }
    }
    best.map(|(_, t)| t)
}

/// A TrueType / OpenType font's family, weight and style.
pub fn sfnt_face(b: &[u8]) -> Option<FontFace> {
    let magic = b.get(0..4)?;
    if magic != [0, 1, 0, 0] && magic != b"OTTO" && magic != b"true" {
        return None;
    }
    let family = family(b)?;
    let (os2, _) = table(b, b"OS/2")?;
    let weight = be16(b, os2 + 4)?;
    let selection = be16(b, os2 + 62)?;
    // A variable font with a weight axis covers the whole range.
    let variable = table(b, b"fvar").is_some_and(|(at, _)| {
        let axes = be16(b, at + 8).unwrap_or(0) as usize;
        let start = at + be16(b, at + 4).unwrap_or(16) as usize;
        let size = be16(b, at + 10).unwrap_or(20) as usize;
        (0..axes).any(|i| b.get(start + i * size..start + i * size + 4) == Some(b"wght"))
    });
    Some(FontFace {
        family,
        weight: if variable {
            "100 900".into()
        } else {
            weight.clamp(1, 1000).to_string()
        },
        italic: selection & 1 == 1,
    })
}

/// A face described by its file name: `Inter-SemiBoldItalic` → Inter, 600, italic.
pub fn face_from_name(stem: &str) -> FontFace {
    let (family, style) = stem.split_once('-').unwrap_or((stem, ""));
    let s = style.to_ascii_lowercase();
    let weight = [
        ("extralight", 200),
        ("ultralight", 200),
        ("semibold", 600),
        ("demibold", 600),
        ("extrabold", 800),
        ("ultrabold", 800),
        ("thin", 100),
        ("light", 300),
        ("medium", 500),
        ("bold", 700),
        ("black", 900),
        ("heavy", 900),
    ]
    .iter()
    .find(|(name, _)| s.contains(name))
    .map_or(400, |(_, w)| *w);
    let variable = s.contains("variable") || stem.contains('[');
    FontFace {
        family: family
            .split('[')
            .next()
            .unwrap_or(family)
            .trim()
            .to_string(),
        weight: if variable {
            "100 900".into()
        } else {
            weight.to_string()
        },
        italic: s.contains("italic") || s.contains("oblique"),
    }
}

/// A face: (family, weight, italic).
type FaceKey = (String, String, bool);
/// Its sources: (preference, URL, CSS format).
type Sources = Vec<(u8, String, &'static str)>;

/// `@font-face` rules for `files`: (logical path under `assets/`, served
/// URL, content). Only files under `fonts/` count.
pub fn font_faces(files: &[(String, String, Vec<u8>)]) -> String {
    // (family, weight, italic) → sources by preference.
    let mut faces: BTreeMap<FaceKey, Sources> = BTreeMap::new();
    for (logical, url, bytes) in files {
        let Some(name) = logical.strip_prefix("fonts/") else {
            continue;
        };
        let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
        let stem = stem.rsplit('/').next().unwrap_or(stem);
        let (rank, format, face) = match ext.to_ascii_lowercase().as_str() {
            "woff2" => (0, "woff2", face_from_name(stem)),
            "woff" => (1, "woff", face_from_name(stem)),
            "ttf" => (
                2,
                "truetype",
                sfnt_face(bytes).unwrap_or_else(|| face_from_name(stem)),
            ),
            "otf" => (
                2,
                "opentype",
                sfnt_face(bytes).unwrap_or_else(|| face_from_name(stem)),
            ),
            _ => continue,
        };
        faces
            .entry((face.family, face.weight, face.italic))
            .or_default()
            .push((rank, url.clone(), format));
    }
    let mut css = String::new();
    for ((family, weight, italic), mut sources) in faces {
        sources.sort();
        let src: Vec<String> = sources
            .iter()
            .map(|(_, url, format)| format!("url(\"{url}\") format(\"{format}\")"))
            .collect();
        css.push_str(&format!(
            "@font-face{{font-family:\"{}\";src:{};font-weight:{weight};font-style:{};font-display:swap}}\n",
            family.replace(['"', '\\', '<', '>'], ""),
            src.join(","),
            if italic { "italic" } else { "normal" },
        ));
    }
    css
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal OpenType font: `name` (family records) and `OS/2` tables.
    fn tiny_font(family: &str, weight: u16, italic: bool) -> Vec<u8> {
        let name: Vec<u8> = {
            let utf16: Vec<u8> = family
                .encode_utf16()
                .flat_map(|u| u.to_be_bytes())
                .collect();
            let mut t = vec![];
            t.extend(0u16.to_be_bytes()); // format
            t.extend(1u16.to_be_bytes()); // count
            t.extend(18u16.to_be_bytes()); // string offset (6 + 12)
            for v in [3u16, 1, 0x409, 1, utf16.len() as u16, 0] {
                t.extend(v.to_be_bytes());
            }
            t.extend(utf16);
            t
        };
        let mut os2 = vec![0u8; 78];
        os2[4..6].copy_from_slice(&weight.to_be_bytes());
        os2[62..64].copy_from_slice(&(italic as u16).to_be_bytes());
        let mut font = vec![0, 1, 0, 0];
        font.extend(2u16.to_be_bytes());
        font.extend([0u8; 6]);
        let header = 12 + 2 * 16;
        let tables = [(b"OS/2", &os2), (b"name", &name)];
        let mut offset = header;
        for (tag, data) in &tables {
            font.extend(*tag);
            font.extend([0u8; 4]);
            font.extend((offset as u32).to_be_bytes());
            font.extend((data.len() as u32).to_be_bytes());
            offset += data.len();
        }
        for (_, data) in &tables {
            font.extend(data.iter());
        }
        font
    }

    #[test]
    fn faces_come_from_the_font_tables() {
        let face = sfnt_face(&tiny_font("Inter Display", 650, true)).unwrap();
        assert_eq!(
            face,
            FontFace {
                family: "Inter Display".into(),
                weight: "650".into(),
                italic: true
            }
        );
        assert_eq!(sfnt_face(b"not a font"), None);
    }

    #[test]
    fn compressed_faces_are_described_by_their_name() {
        let f = face_from_name("Inter-SemiBoldItalic");
        assert_eq!(
            (f.family.as_str(), f.weight.as_str(), f.italic),
            ("Inter", "600", true)
        );
        let f = face_from_name("Mono");
        assert_eq!(
            (f.family.as_str(), f.weight.as_str(), f.italic),
            ("Mono", "400", false)
        );
        let f = face_from_name("Inter[wght]");
        assert_eq!((f.family.as_str(), f.weight.as_str()), ("Inter", "100 900"));
    }

    #[test]
    fn font_files_become_font_face_rules() {
        let files = vec![
            (
                "fonts/Brand.ttf".to_string(),
                "../assets/fonts/Brand.abc.ttf".to_string(),
                tiny_font("Brand", 700, false),
            ),
            (
                "fonts/Brand-Bold.woff2".to_string(),
                "../assets/fonts/Brand-Bold.def.woff2".to_string(),
                vec![],
            ),
            (
                "logo.png".to_string(),
                "../assets/logo.png".to_string(),
                vec![],
            ),
        ];
        let css = font_faces(&files);
        assert_eq!(
            css,
            "@font-face{font-family:\"Brand\";src:url(\"../assets/fonts/Brand-Bold.def.woff2\") format(\"woff2\"),url(\"../assets/fonts/Brand.abc.ttf\") format(\"truetype\");font-weight:700;font-style:normal;font-display:swap}\n"
        );
    }
}
