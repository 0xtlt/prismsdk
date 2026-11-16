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
//!
//! The same runs are the value of an editable [`rich_input`](crate::rich_input)
//! (#115), which may also hold [`chip`]s: atomic inline objects (a mention
//! of a page, a person, a date) that take one character, [`OBJECT_CHAR`],
//! of its value. [`split_spans`], [`restyle_spans`] and [`splice_spans`]
//! edit runs by character offsets (a chip counts as one), as a formatting
//! command, a mention picker or splitting a block at the caret need.

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

/// The character an inline object ([`chip`]) takes in the value of a
/// [`rich_input`](crate::rich_input): U+FFFC OBJECT REPLACEMENT CHARACTER.
pub const OBJECT_CHAR: char = '\u{FFFC}';

/// An atomic inline object (#115): a mention of a page, a person or a date,
/// drawn as a small chip (an optional [`chip_icon`](Span::chip_icon), then
/// `label`). In a [`rich_input`](crate::rich_input) it is one character of
/// the value ([`OBJECT_CHAR`]) that the caret jumps over and one Backspace
/// deletes; `key` is the app's own identifier for it (a page id), given back
/// in [`RichInputEvent::spans`](crate::RichInputEvent::spans). In a
/// read-only [`rich_text`](crate::rich_text) it shows (and reads as) its
/// label. Style it like any span (`.bg(..)`, `.text_color(..)`); clickable
/// with [`on_click`](Span::on_click).
///
/// ```
/// # use prism_core::prelude::*;
/// let spans = [span("See "), chip("page:42", "Roadmap").chip_icon("doc.text")];
/// assert_eq!(prism_core::spans_text(&spans), "See \u{FFFC}");
/// ```
pub fn chip(key: impl Into<String>, label: impl Into<String>) -> Span {
    let mut s = span(OBJECT_CHAR.to_string());
    s.run.chip = Some(key.into());
    s.run.chip_label = Some(label.into());
    s
}

impl Span {
    /// A [`chip`]'s symbol (an SF Symbol name), drawn before its label.
    pub fn chip_icon(mut self, symbol: impl Into<String>) -> Self {
        self.run.chip_icon = Some(symbol.into());
        self
    }

    /// The run's text: [`OBJECT_CHAR`] for a [`chip`].
    pub fn text(&self) -> &str {
        &self.run.text
    }

    /// The same run with another text (its style, link and handler kept).
    /// A chip keeps its single [`OBJECT_CHAR`].
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        if !self.run.is_chip() {
            self.run.text = text.into();
        }
        self
    }

    /// Its length in characters (a chip is one).
    pub fn len(&self) -> usize {
        self.run.text.chars().count()
    }

    /// Whether it has no text.
    pub fn is_empty(&self) -> bool {
        self.run.text.is_empty()
    }

    /// Whether it is an inline object ([`chip`]).
    pub fn is_chip(&self) -> bool {
        self.run.is_chip()
    }

    /// A [`chip`]'s key.
    pub fn chip_key(&self) -> Option<&str> {
        self.run.chip.as_deref()
    }

    /// A [`chip`]'s label.
    pub fn chip_label(&self) -> Option<&str> {
        self.run.chip_label.as_deref()
    }

    /// Its weight, if it sets one.
    pub fn weight_value(&self) -> Option<FontWeight> {
        self.run.weight
    }

    /// Whether it is semibold or heavier.
    pub fn is_bold(&self) -> bool {
        self.run.weight.is_some_and(|w| w.css() >= 600)
    }

    /// Whether it is italic.
    pub fn is_italic(&self) -> bool {
        self.run.italic
    }

    /// Whether it is underlined (links are by default).
    pub fn is_underlined(&self) -> bool {
        self.run.underline
    }

    /// Whether it is struck through.
    pub fn is_strikethrough(&self) -> bool {
        self.run.strikethrough
    }

    /// Whether it uses the monospaced font (inline code).
    pub fn is_mono(&self) -> bool {
        self.run.family == Some(FontFamily::Monospace)
    }

    /// A [`link`]'s path or URL.
    pub fn href(&self) -> Option<&str> {
        self.run.href.as_deref()
    }

    /// Whether it reacts to activation (a [`link`], or `on_click`).
    pub fn is_clickable(&self) -> bool {
        self.run.href.is_some() || self.on_click.is_some()
    }

    /// Whether `other` looks and behaves the same (everything but the
    /// text): two such neighbours are one run. Chips never are.
    pub fn same_style(&self, other: &Span) -> bool {
        let handlers = match (&self.on_click, &other.on_click) {
            (None, None) => true,
            (Some(a), Some(b)) => std::rc::Rc::ptr_eq(a, b),
            _ => false,
        };
        let plain = |s: &Span| TextRun {
            text: String::new(),
            ..s.run.clone()
        };
        !self.is_chip()
            && !other.is_chip()
            && handlers
            && self.size == other.size
            && self.label == other.label
            && plain(self) == plain(other)
    }

    /// What a read-only text shows: a chip's label, else the text.
    pub(crate) fn display_text(&self) -> &str {
        match (&self.run.chip, &self.run.chip_label) {
            (Some(_), Some(label)) => label,
            _ => &self.run.text,
        }
    }

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
    /// An inline object ([`chip`], #115): the app's key for it. Its `text`
    /// is [`OBJECT_CHAR`] in a rich input, its label in a read-only text.
    pub chip: Option<String>,
    /// What a chip shows (and screen readers say).
    pub chip_label: Option<String>,
    /// A chip's symbol, drawn before its label (SF Symbol name).
    pub chip_icon: Option<String>,
}

impl TextRun {
    /// Whether the run reacts to activation.
    pub fn is_link(&self) -> bool {
        self.link.is_some()
    }

    /// Whether the run is an inline object ([`chip`]).
    pub fn is_chip(&self) -> bool {
        self.chip.is_some()
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
/// `h=/path` href, `g=star.fill` icon, and for a [`chip`] `k=key`, `n=label`,
/// `m=icon`. Control characters U+001E and U+001F are dropped from texts.
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
        if let Some(k) = &r.chip {
            field(format!("k={}", clean(k)));
        }
        if let Some(n) = &r.chip_label {
            field(format!("n={}", clean(n)));
        }
        if let Some(m) = &r.chip_icon {
            field(format!("m={}", clean(m)));
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
                    "k" => run.chip = Some(v.to_string()),
                    "n" => run.chip_label = Some(v.to_string()),
                    "m" => run.chip_icon = Some(v.to_string()),
                    _ => {}
                }
            }
            run
        })
        .collect()
}

// --- Editing runs by character offsets (#115) ----------------------------------

/// The value of `spans`: their texts one after the other, a [`chip`] being
/// [`OBJECT_CHAR`]. What a [`rich_input`](crate::rich_input) reports as
/// [`RichInputEvent::value`](crate::RichInputEvent::value).
pub fn spans_text(spans: &[Span]) -> String {
    spans.iter().map(|s| s.run.text.as_str()).collect()
}

/// `spans` without empty runs, neighbours of the same style
/// ([`Span::same_style`]) merged into one. Chips stay one run each.
pub fn normalize_spans(spans: impl IntoIterator<Item = Span>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for s in spans {
        if s.is_empty() {
            continue;
        }
        if let Some(last) = out.last_mut()
            && last.same_style(&s)
        {
            last.run.text.push_str(&s.run.text);
            continue;
        }
        out.push(s);
    }
    out
}

/// `span` cut at character `at` (not a chip: it has one).
fn cut(span: &Span, at: usize) -> (Span, Span) {
    let b = crate::element::char_to_byte(&span.run.text, at);
    let (mut l, mut r) = (span.clone(), span.clone());
    l.run.text = span.run.text[..b].to_string();
    r.run.text = span.run.text[b..].to_string();
    (l, r)
}

/// `spans` split at character `at` (clamped): the runs before it and the
/// runs after it, a run across it cut in two. Splitting a block at the
/// caret on Enter:
///
/// ```
/// # use prism_core::prelude::*;
/// # use prism_core::{split_spans, spans_text};
/// let (a, b) = split_spans(&[span("Hello "), span("world").bold()], 8);
/// assert_eq!((spans_text(&a), spans_text(&b)), ("Hello wo".into(), "rld".into()));
/// assert!(a[1].is_bold() && b[0].is_bold());
/// ```
pub fn split_spans(spans: &[Span], at: usize) -> (Vec<Span>, Vec<Span>) {
    let (mut before, mut after) = (Vec::new(), Vec::new());
    let mut pos = 0;
    for s in spans {
        let len = s.len();
        if pos + len <= at {
            before.push(s.clone());
        } else if pos >= at {
            after.push(s.clone());
        } else {
            let (l, r) = cut(s, at - pos);
            before.push(l);
            after.push(r);
        }
        pos += len;
    }
    (normalize_spans(before), normalize_spans(after))
}

/// `spans` with the characters in `range` restyled by `f` (runs cut at its
/// ends), then [normalized](normalize_spans): Bold on a selection,
/// `restyle_spans(&spans, sel.range(), Span::bold)`. `f` sees chips too.
pub fn restyle_spans(
    spans: &[Span],
    range: std::ops::Range<usize>,
    f: impl Fn(Span) -> Span,
) -> Vec<Span> {
    let (before, rest) = split_spans(spans, range.start);
    let (mid, after) = split_spans(&rest, range.end.saturating_sub(range.start));
    normalize_spans(
        before
            .into_iter()
            .chain(mid.into_iter().map(f))
            .chain(after),
    )
}

/// `spans` with the characters in `range` replaced by `insert`, then
/// [normalized](normalize_spans): a mention picker replacing "@ro" with a
/// [`chip`], `splice_spans(&spans, at - 3..at, [chip("page:42", "Roadmap")])`.
/// An empty range inserts.
pub fn splice_spans(
    spans: &[Span],
    range: std::ops::Range<usize>,
    insert: impl IntoIterator<Item = Span>,
) -> Vec<Span> {
    let (before, rest) = split_spans(spans, range.start);
    let (_, after) = split_spans(&rest, range.end.saturating_sub(range.start));
    normalize_spans(before.into_iter().chain(insert).chain(after))
}

/// Whether text typed right after (or before) `s` takes its style: not a
/// chip, not a link (typing at the end of a link writes plain text).
fn continues(s: &Span) -> bool {
    !s.is_chip() && !s.is_clickable()
}

/// The span whose style text typed at character `at` of `spans` takes
/// (#115), replacing `replaced` characters there: inside a run, that run;
/// over a selection, its first run; at the edge of two runs, the one before
/// (as editors do), unless it is a chip or a link, then the one after,
/// unless it is one too: plain text. The web client applies the same rule.
pub(crate) fn typing_style(spans: &[Span], at: usize, replaced: usize) -> Span {
    let (mut left, mut right) = (None, None);
    let mut pos = 0;
    for s in spans {
        let len = s.len();
        if pos < at && at <= pos + len {
            left = Some(s);
        }
        if right.is_none() && pos <= at && at < pos + len {
            right = Some(s);
        }
        pos += len;
    }
    let pick = match (left, right) {
        // Inside one run (a link included).
        (Some(l), Some(r)) if std::ptr::eq(l, r) && !l.is_chip() => Some(l),
        (_, Some(r)) if replaced > 0 && continues(r) => Some(r),
        (Some(l), _) if continues(l) => Some(l),
        (_, Some(r)) if continues(r) => Some(r),
        _ => None,
    };
    let mut style = pick.cloned().unwrap_or_else(|| span(""));
    style.run.text = String::new();
    style
}

/// The runs after the user changed the text of `old` to `new` (a backend
/// that edits plain text: GTK, Android, iOS, the macOS fallback): the
/// common start and end keep their runs, what was typed between takes
/// [`typing_style`]; a deleted chip is gone. [`OBJECT_CHAR`]s typed or
/// pasted are dropped (only the app makes chips).
pub(crate) fn edit_text(old: &[Span], new: &str) -> Vec<Span> {
    let o: Vec<char> = spans_text(old).chars().collect();
    let n: Vec<char> = new.chars().collect();
    let mut p = 0;
    while p < o.len() && p < n.len() && o[p] == n[p] {
        p += 1;
    }
    let mut s = 0;
    while s < o.len() - p && s < n.len() - p && o[o.len() - 1 - s] == n[n.len() - 1 - s] {
        s += 1;
    }
    let inserted: String = n[p..n.len() - s]
        .iter()
        .filter(|c| **c != OBJECT_CHAR)
        .collect();
    let replaced = o.len() - s - p;
    let style = typing_style(old, p, replaced);
    let (before, rest) = split_spans(old, p);
    let (_, after) = split_spans(&rest, replaced);
    normalize_spans(
        before
            .into_iter()
            .chain([style.with_text(inserted)])
            .chain(after),
    )
}

/// The style of a mounted run, to match the runs an editor reports: no
/// text, no link id.
pub(crate) fn run_key(run: &TextRun) -> TextRun {
    TextRun {
        text: String::new(),
        link: None,
        ..run.clone()
    }
}

/// The spans an editor reported as `runs` (the web client, AppKit: the
/// runs it shows, each with the style of the run it came from), rebuilt
/// from `old` (the spans rendered, with [`run_key`]s) so handlers, labels
/// and sizes come back as the app made them. A style no rendered span has
/// (the app changed it meanwhile) becomes a plain span of that style.
pub(crate) fn edit_runs(old: &[(TextRun, Span)], font_size: f32, runs: Vec<TextRun>) -> Vec<Span> {
    let mut out = Vec::with_capacity(runs.len());
    for r in runs {
        let key = run_key(&r);
        let found = old.iter().find(|(k, _)| *k == key).map(|(_, s)| s.clone());
        let mut s = found.unwrap_or_else(|| Span {
            size: r.size.filter(|_| font_size > 0.).map(|z| z * font_size),
            run: TextRun {
                link: None,
                size: None,
                ..r.clone()
            },
            on_click: None,
            label: None,
        });
        if s.is_chip() {
            // One run per object character.
            s.run.text = OBJECT_CHAR.to_string();
            let n = r.text.chars().filter(|c| *c == OBJECT_CHAR).count().max(1);
            out.extend(std::iter::repeat_n(s, n));
        } else {
            s.run.text = r.text.replace(OBJECT_CHAR, "");
            out.push(s);
        }
    }
    normalize_spans(out)
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
