//! Fonts and stylesheets added while the app runs.
//!
//! Fonts in `assets/fonts/` are registered at startup. A font only known at
//! run time (a catalogue served by your server, a file the user uploaded) is
//! registered with [`App::load_font`], for the session on the web and for
//! the process natively:
//!
//! ```ignore
//! cx.load_font(FontFace::url("Lora", "https://example.com/fonts/lora.woff2").weights(400, 700));
//! cx.load_font(FontFace::bytes("Brand", uploaded_bytes));
//! text("Lora").font_family("Lora")
//! ```
//!
//! | | Registers with | Formats |
//! |---|---|---|
//! | Web | `FontFace` + `document.fonts` | WOFF2, WOFF, TTF, OTF |
//! | Apple platforms | `CTFontManagerRegisterFontsForURL` (process scope) | TTF, OTF, TTC |
//! | Android | `Typeface.Builder(File)` | TTF, OTF, TTC |
//! | Linux | fontconfig (`FcConfigAppFontAddFile`), then Pango reloads | TTF, OTF, TTC |
//!
//! Natively the family is read from the font file: the name given to
//! [`FontFace`] becomes an alias of it, so `.font_family("Brand")` finds the
//! face whatever the file calls itself. Texts already shown in that family
//! are drawn again once it is registered.
//!
//! [`App::add_stylesheet`] adds a stylesheet to the page (web only) for as
//! long as the returned [`Stylesheet`] lives: keep it in the entity of the
//! screen that needs it, and it goes with the screen.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use crate::app::App;
use crate::surfaces::PlatformRequest;

/// Where a run-time font comes from.
#[derive(Clone, PartialEq, Eq)]
pub enum FontSource {
    /// An address: `https://…`, an app path (`/fonts/lora.woff2`, resolved
    /// against the page on the web), an asset (`fonts/extra.ttf`) or a file
    /// (`file:///…`) natively.
    Url(String),
    /// The font file itself.
    Bytes(Arc<[u8]>),
}

impl std::fmt::Debug for FontSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FontSource::Url(url) => f.debug_tuple("Url").field(url).finish(),
            FontSource::Bytes(b) => write!(f, "Bytes({} bytes)", b.len()),
        }
    }
}

/// A font face to register while the app runs ([`App::load_font`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontFace {
    /// The family texts use (`.font_family(…)`).
    pub family: String,
    /// The font file.
    pub source: FontSource,
    /// The weights it covers: one (`(700, 700)`), a range for a variable
    /// font (`(100, 900)`), or `None` to read it from the file.
    pub weight: Option<(u16, u16)>,
    /// An italic face.
    pub italic: bool,
}

impl FontFace {
    /// A face downloaded from `url` (see [`FontSource::Url`]).
    pub fn url(family: impl Into<String>, url: impl Into<String>) -> FontFace {
        FontFace::new(family, FontSource::Url(url.into()))
    }

    /// A face from the bytes of a font file.
    pub fn bytes(family: impl Into<String>, bytes: impl Into<Arc<[u8]>>) -> FontFace {
        FontFace::new(family, FontSource::Bytes(bytes.into()))
    }

    fn new(family: impl Into<String>, source: FontSource) -> FontFace {
        FontFace {
            family: family.into(),
            source,
            weight: None,
            italic: false,
        }
    }

    /// The one weight of this face (100–900).
    pub fn weight(self, weight: u16) -> FontFace {
        self.weights(weight, weight)
    }

    /// The weights a variable face covers (`weights(100, 900)`).
    pub fn weights(mut self, min: u16, max: u16) -> FontFace {
        let (a, b) = (min.clamp(1, 1000), max.clamp(1, 1000));
        self.weight = Some((a.min(b), a.max(b)));
        self
    }

    /// An italic face.
    pub fn italic(mut self) -> FontFace {
        self.italic = true;
        self
    }

    /// The CSS `font-weight` of the face: `400`, `100 900`, or `None`
    /// (unknown: the browser reads the file).
    pub fn css_weight(&self) -> Option<String> {
        self.weight.map(|(a, b)| {
            if a == b {
                a.to_string()
            } else {
                format!("{a} {b}")
            }
        })
    }

    /// The same face with its downloaded bytes (native backends resolve a
    /// [`FontSource::Url`] into this).
    pub fn with_bytes(&self, bytes: impl Into<Arc<[u8]>>) -> FontFace {
        FontFace {
            source: FontSource::Bytes(bytes.into()),
            ..self.clone()
        }
    }

    fn key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.family.hash(&mut h);
        self.weight.hash(&mut h);
        self.italic.hash(&mut h);
        match &self.source {
            FontSource::Url(url) => (0u8, url).hash(&mut h),
            FontSource::Bytes(b) => (1u8, &b[..]).hash(&mut h),
        }
        h.finish()
    }
}

/// Run-time fonts and stylesheets of an app.
#[derive(Default)]
pub(crate) struct Fonts {
    /// Faces already asked for (the same face again is not re-sent).
    loaded: HashSet<u64>,
    sheets: Rc<RefCell<Sheets>>,
}

#[derive(Default)]
struct Sheets {
    /// URL → handles alive.
    count: BTreeMap<String, usize>,
    /// Changes not handed to the backend yet.
    pending: Vec<PlatformRequest>,
}

/// A stylesheet added with [`App::add_stylesheet`]; removed from the page
/// when the last handle for its URL is dropped.
#[must_use = "the stylesheet is removed when its handle is dropped"]
pub struct Stylesheet {
    url: String,
    sheets: Weak<RefCell<Sheets>>,
}

impl Stylesheet {
    /// The stylesheet's address.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl std::fmt::Debug for Stylesheet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Stylesheet").field(&self.url).finish()
    }
}

impl Drop for Stylesheet {
    fn drop(&mut self) {
        let Some(sheets) = self.sheets.upgrade() else {
            return;
        };
        let Ok(mut s) = sheets.try_borrow_mut() else {
            return;
        };
        let left = match s.count.get_mut(&self.url) {
            Some(n) => {
                *n -= 1;
                *n
            }
            None => return,
        };
        if left == 0 {
            s.count.remove(&self.url);
            s.pending
                .push(PlatformRequest::RemoveStylesheet(self.url.clone()));
        }
    }
}

impl App {
    /// Registers a font face for this session (web) or process (native), so
    /// `.font_family(family)` draws with it; see [`crate::fonts`]. Asking for
    /// the same face again does nothing, so it can be called from `render`.
    pub fn load_font(&mut self, font: FontFace) {
        if self.fonts.loaded.insert(font.key()) {
            self.surfaces.requests.push(PlatformRequest::LoadFont(font));
        }
    }

    /// Adds the stylesheet at `url` to the page while the returned handle
    /// lives (web only; nothing natively). Handles for the same URL share
    /// one `<link>`.
    pub fn add_stylesheet(&mut self, url: impl Into<String>) -> Stylesheet {
        let url = url.into();
        let mut s = self.fonts.sheets.borrow_mut();
        let n = s.count.entry(url.clone()).or_insert(0);
        *n += 1;
        if *n == 1 {
            s.pending.push(PlatformRequest::AddStylesheet(url.clone()));
        }
        Stylesheet {
            url,
            sheets: Rc::downgrade(&self.fonts.sheets),
        }
    }

    /// The stylesheets added with [`add_stylesheet`](Self::add_stylesheet)
    /// and still alive, in URL order.
    pub fn stylesheets(&self) -> Vec<String> {
        self.fonts.sheets.borrow().count.keys().cloned().collect()
    }

    /// Stylesheet changes for the backend, in order.
    pub(crate) fn take_stylesheet_requests(&mut self) -> Vec<PlatformRequest> {
        std::mem::take(&mut self.fonts.sheets.borrow_mut().pending)
    }
}

// ---------------------------------------------------------------------------
// Reading font files
// ---------------------------------------------------------------------------

/// What a TrueType / OpenType font file says about itself ([`font_file_info`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontFileInfo {
    /// Family name: the typographic family when there is one (`Inter`).
    pub family: String,
    /// Weight of the default instance (`OS/2` `usWeightClass`, 1–1000).
    pub weight: u16,
    /// A variable font with a weight axis (it covers every weight).
    pub variable: bool,
    /// An italic face.
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

/// The offset of table `tag` in the font whose table directory starts at
/// `dir` (0, or a collection member's offset; table offsets are from the
/// start of the file either way).
fn table(b: &[u8], dir: usize, tag: &[u8; 4]) -> Option<usize> {
    let tables = be16(b, dir + 4)? as usize;
    (0..tables).find_map(|i| {
        let rec = dir + 12 + i * 16;
        (b.get(rec..rec + 4)? == tag)
            .then(|| be32(b, rec + 8).map(|o| o as usize))
            .flatten()
    })
}

/// Family name from the `name` table: the typographic family (16) when
/// there is one, else the family (1); Windows Unicode or Mac Roman records.
fn family(b: &[u8], dir: usize) -> Option<String> {
    let at = table(b, dir, b"name")?;
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

/// Family, weight and style of a TrueType or OpenType font (`.ttf`, `.otf`;
/// the first font of a `.ttc` collection). `None` for anything else (WOFF
/// and WOFF2 files are compressed).
pub fn font_file_info(b: &[u8]) -> Option<FontFileInfo> {
    let dir = match b.get(0..4)? {
        b"ttcf" => be32(b, 12)? as usize,
        [0, 1, 0, 0] | b"OTTO" | b"true" => 0,
        _ => return None,
    };
    let family = family(b, dir)?;
    let os2 = table(b, dir, b"OS/2")?;
    let weight = be16(b, os2 + 4)?.clamp(1, 1000);
    let selection = be16(b, os2 + 62)?;
    let variable = table(b, dir, b"fvar").is_some_and(|at| {
        let axes = be16(b, at + 8).unwrap_or(0) as usize;
        let start = at + be16(b, at + 4).unwrap_or(16) as usize;
        let size = be16(b, at + 10).unwrap_or(20) as usize;
        (0..axes).any(|i| b.get(start + i * size..start + i * size + 4) == Some(b"wght"))
    });
    Some(FontFileInfo {
        family,
        weight,
        variable,
        italic: selection & 1 == 1,
    })
}

/// The CSS format of a font file from its first bytes (`woff2`, `woff`,
/// `opentype`, `truetype`, `collection`), if it is one.
pub fn font_format(b: &[u8]) -> Option<&'static str> {
    Some(match b.get(0..4)? {
        b"wOF2" => "woff2",
        b"wOFF" => "woff",
        b"OTTO" => "opentype",
        b"ttcf" => "collection",
        [0, 1, 0, 0] | b"true" => "truetype",
        _ => return None,
    })
}
