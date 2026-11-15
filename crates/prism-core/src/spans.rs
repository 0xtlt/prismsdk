//! Rich text: one paragraph made of runs (spans) with their own weight,
//! color, font, decoration, links and inline icons.
//!
//! ```ignore
//! rich_text([
//!     span("Signed in as "),
//!     span("Ada").bold(),
//!     span(" · "),
//!     link("settings", "/settings"),
//!     icon("chevron.right"),
//! ])
//! ```
//!
//! The paragraph wraps between words (not between runs), is one element for
//! screen readers, and maps to `NSAttributedString` (AppKit, UIKit),
//! `AttributedString` (SwiftUI on watchOS), a `SpannableString` (Android),
//! Pango attributes (GTK) and inline `<span>`/`<a>` on the web.
//!
//! Backends receive the runs as the text node's config
//! ([`Patch::SetConfig`](crate::Patch::SetConfig)), encoded by
//! [`encode_runs`] and decoded by [`decode_runs`]. A clickable run (a link,
//! or a span with `on_click`) carries an id the backend reports with
//! [`UiEvent::MenuSelect`](crate::UiEvent::MenuSelect) when it is activated.

use crate::element::{ClickEvent, Handler};
use crate::style::{Color, FontFamily, FontWeight};

/// One run of a [`rich_text`](crate::rich_text) paragraph. Built with
/// [`span`], [`link`] or [`icon`].
#[derive(Clone)]
pub struct Span {
    pub(crate) run: TextRun,
    /// Absolute size in points (made relative to the paragraph's when mounted).
    pub(crate) size: Option<f32>,
    pub(crate) on_click: Option<Handler<ClickEvent>>,
    /// Accessibility label of an icon.
    pub(crate) label: Option<String>,
}

impl std::fmt::Debug for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Span")
            .field("run", &self.run)
            .field("size", &self.size)
            .field("clickable", &self.on_click.is_some())
            .finish()
    }
}

/// A run of text: `span("Home").bold()`.
pub fn span(text: impl Into<String>) -> Span {
    Span {
        run: TextRun {
            text: text.into(),
            ..TextRun::default()
        },
        size: None,
        on_click: None,
        label: None,
    }
}

/// An inline link to an app path (`/settings`) or a URL (`https://…`,
/// `mailto:`), underlined by default. Activating it navigates in the app (or
/// opens the URL); it is reachable from the keyboard and announced as a link
/// inside the paragraph.
pub fn link(text: impl Into<String>, path: impl Into<String>) -> Span {
    let mut s = span(text);
    s.run.href = Some(path.into());
    s.run.underline = true;
    s
}

/// An inline symbol (an SF Symbol name; a glyph where SF Symbols don't
/// exist, see [`glyphs`](crate::glyphs)), sized and colored like the text
/// around it. Decorative unless given a [`label`](Span::label).
pub fn icon(name: impl Into<String>) -> Span {
    let name = name.into();
    let mut s = span(crate::glyphs::glyph(&name));
    s.run.icon = Some(name);
    s
}

impl Span {
    /// Text weight.
    pub fn font_weight(mut self, w: FontWeight) -> Self {
        self.run.weight = Some(w);
        self
    }
    /// Text weight as a CSS number, 100 to 900.
    pub fn weight(self, w: u16) -> Self {
        self.font_weight(FontWeight::from_css(w))
    }
    /// Medium weight (500).
    pub fn medium(self) -> Self {
        self.font_weight(FontWeight::Medium)
    }
    /// Semibold weight (600).
    pub fn semibold(self) -> Self {
        self.font_weight(FontWeight::Semibold)
    }
    /// Bold weight (700).
    pub fn bold(self) -> Self {
        self.font_weight(FontWeight::Bold)
    }
    /// Italic.
    pub fn italic(mut self) -> Self {
        self.run.italic = true;
        self
    }
    /// Underlined.
    pub fn underline(mut self) -> Self {
        self.run.underline = true;
        self
    }
    /// Not underlined (links are by default).
    pub fn no_underline(mut self) -> Self {
        self.run.underline = false;
        self
    }
    /// Struck through.
    pub fn strikethrough(mut self) -> Self {
        self.run.strikethrough = true;
        self
    }
    /// Text color (adaptive colors follow dark mode).
    pub fn text_color(mut self, c: Color) -> Self {
        self.run.color = Some(c);
        self
    }
    /// Background behind the run (highlights, inline code).
    pub fn bg(mut self, c: Color) -> Self {
        self.run.background = Some(c);
        self
    }
    /// Font family.
    pub fn font_family(mut self, family: impl Into<FontFamily>) -> Self {
        self.run.family = Some(family.into());
        self
    }
    /// The platform's monospaced font.
    pub fn font_mono(self) -> Self {
        self.font_family(FontFamily::Monospace)
    }
    /// Text size, points (scaled with the paragraph by Dynamic Type and zoom).
    pub fn text_size(mut self, points: f32) -> Self {
        self.size = Some(points);
        self
    }
    /// Makes the run clickable: reachable from the keyboard and announced as
    /// a link inside the paragraph.
    pub fn on_click(mut self, handler: Handler<ClickEvent>) -> Self {
        self.on_click = Some(handler);
        self
    }
    /// What screen readers say for an [`icon`] (decorative without one) or
    /// instead of the run's text.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// A run as backends receive it (see [`encode_runs`]).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TextRun {
    /// The text (an icon's fallback glyph for icons).
    pub text: String,
    /// Weight (`None`: the paragraph's).
    pub weight: Option<FontWeight>,
    /// Italic.
    pub italic: bool,
    /// Underlined.
    pub underline: bool,
    /// Struck through.
    pub strikethrough: bool,
    /// Text color (`None`: the paragraph's).
    pub color: Option<Color>,
    /// Background color.
    pub background: Option<Color>,
    /// Font family (`None`: the paragraph's).
    pub family: Option<FontFamily>,
    /// Size as a multiple of the paragraph's font size (`None`: the same).
    pub size: Option<f32>,
    /// Clickable: reported as `UiEvent::MenuSelect(link)` when activated.
    pub link: Option<u64>,
    /// The link's path or URL (the web's `href`; native links report `link`).
    pub href: Option<String>,
    /// An inline symbol (SF Symbol name; `text` is its glyph).
    pub icon: Option<String>,
}

impl TextRun {
    /// Whether the run reacts to activation.
    pub fn is_link(&self) -> bool {
        self.link.is_some()
    }
}

const RUN_SEP: char = '\u{1e}';
const FIELD_SEP: char = '\u{1f}';

fn color_code(c: Color) -> String {
    match c.dark {
        Some(_) => format!("{}/{}", c.light_variant().hex(), c.dark_variant().hex()),
        None => c.hex(),
    }
}

/// Parses `#RRGGBBAA` or `#RRGGBBAA/#RRGGBBAA` (light/dark).
pub fn parse_color(s: &str) -> Option<Color> {
    let one = |h: &str| -> Option<Color> {
        let h = h.strip_prefix('#')?;
        let v = u32::from_str_radix(h, 16).ok()?;
        if h.len() != 8 {
            return None;
        }
        Some(crate::style::rgba(v >> 8, (v & 0xff) as f32 / 255.))
    };
    match s.split_once('/') {
        Some((l, d)) => Some(Color::adaptive(one(l)?, one(d)?)),
        None => one(s),
    }
}

/// The runs as one string: runs separated by U+001E, each run its text then
/// `key=value` fields separated by U+001F: `w=700` weight, `i` italic, `u`
/// underline, `s` strikethrough, `c=#RRGGBBAA[/#RRGGBBAA]` color (light /
/// dark), `b=…` background, `f=mono` family (`system`, `mono`, `serif`,
/// `rounded` or a name), `z=1.25` size (× the paragraph's), `l=123` link id,
/// `h=/path` href, `g=star.fill` icon. Control characters U+001E and U+001F
/// are dropped from texts.
pub fn encode_runs(runs: &[TextRun]) -> String {
    let clean = |s: &str| s.replace([RUN_SEP, FIELD_SEP], "");
    let mut out = String::new();
    for (i, r) in runs.iter().enumerate() {
        if i > 0 {
            out.push(RUN_SEP);
        }
        out.push_str(&clean(&r.text));
        let mut field = |f: String| {
            out.push(FIELD_SEP);
            out.push_str(&f);
        };
        if let Some(w) = r.weight {
            field(format!("w={}", w.css()));
        }
        if r.italic {
            field("i".into());
        }
        if r.underline {
            field("u".into());
        }
        if r.strikethrough {
            field("s".into());
        }
        if let Some(c) = r.color {
            field(format!("c={}", color_code(c)));
        }
        if let Some(c) = r.background {
            field(format!("b={}", color_code(c)));
        }
        if let Some(f) = &r.family {
            field(format!("f={}", clean(f.encode())));
        }
        if let Some(z) = r.size {
            field(format!("z={z}"));
        }
        if let Some(l) = r.link {
            field(format!("l={l}"));
        }
        if let Some(h) = &r.href {
            field(format!("h={}", clean(h)));
        }
        if let Some(g) = &r.icon {
            field(format!("g={}", clean(g)));
        }
    }
    out
}

/// Parses [`encode_runs`]' output (empty: no runs).
pub fn decode_runs(s: &str) -> Vec<TextRun> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split(RUN_SEP)
        .map(|r| {
            let mut fields = r.split(FIELD_SEP);
            let mut run = TextRun {
                text: fields.next().unwrap_or("").to_string(),
                ..TextRun::default()
            };
            for f in fields {
                let (k, v) = f.split_once('=').unwrap_or((f, ""));
                match k {
                    "w" => run.weight = v.parse().ok().map(FontWeight::from_css),
                    "i" => run.italic = true,
                    "u" => run.underline = true,
                    "s" => run.strikethrough = true,
                    "c" => run.color = parse_color(v),
                    "b" => run.background = parse_color(v),
                    "f" => run.family = Some(FontFamily::decode(v)),
                    "z" => run.size = v.parse().ok(),
                    "l" => run.link = v.parse().ok(),
                    "h" => run.href = Some(v.to_string()),
                    "g" => run.icon = Some(v.to_string()),
                    _ => {}
                }
            }
            run
        })
        .collect()
}

/// Link ids of a node's runs: `node << 16 | 0x8000 | index` (context menu
/// items of the node use `node << 16 | n` with small `n`).
pub(crate) fn link_id(node: u64, index: usize) -> u64 {
    node << 16 | 0x8000 | (index as u64 & 0x7fff)
}

/// Whether `path` leaves the app (a URL with a scheme) rather than navigating in it.
pub(crate) fn is_external(path: &str) -> bool {
    path.contains("://") || path.starts_with("mailto:") || path.starts_with("tel:")
}
