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
//! Pango attributes (GTK), XAML `Run`s and `Hyperlink`s (Windows) and
//! inline `<span>`/`<a>` on the web.
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
//!
//! A chip's box and label can be styled on every backend (#329): a
//! [`tooltip`](Span::tooltip), a ring, a radius, a height and a maximum
//! width that cuts its label with an ellipsis
//! ([`chip_ring`](Span::chip_ring), [`chip_radius`](Span::chip_radius),
//! [`chip_height`](Span::chip_height),
//! [`chip_max_width`](Span::chip_max_width)), a label made of styled runs
//! ([`chip_label_runs`](Span::chip_label_runs)), a vector icon
//! ([`ChipIcon`]), and a state and a test id that stylesheets and tests
//! find it by ([`chip_state`](Span::chip_state),
//! [`chip_test_id`](Span::chip_test_id), [`chip_attr`](Span::chip_attr)).

use crate::element::{ClickEvent, Handler};
use crate::shape::{Drawing, Path};
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
/// around it ([`text_size`](Span::text_size), [`text_color`](Span::text_color)).
/// Decorative unless given a [`label`](Span::label). In a
/// [`rich_input`](crate::rich_input) it is one character of the value,
/// [`OBJECT_CHAR`], like a [`chip`] (#221): the caret jumps over it and one
/// Backspace deletes it; a read-only text shows it in place of its glyph.
pub fn icon(name: impl Into<String>) -> Span {
    let mut s = span(OBJECT_CHAR.to_string());
    s.run.icon = Some(name.into());
    s
}

/// How an underline is drawn (#221): [`Span::underline_style`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum UnderlineStyle {
    /// One thin line (the default).
    #[default]
    Single,
    /// One thicker line.
    Thick,
    /// Two thin lines.
    Double,
    /// A dotted line (spelling-style marks).
    Dotted,
    /// A dashed line.
    Dashed,
}

impl UnderlineStyle {
    /// Its name in the run encoding and in CSS-like terms (`"thick"`).
    pub fn name(self) -> &'static str {
        match self {
            UnderlineStyle::Single => "single",
            UnderlineStyle::Thick => "thick",
            UnderlineStyle::Double => "double",
            UnderlineStyle::Dotted => "dotted",
            UnderlineStyle::Dashed => "dashed",
        }
    }

    /// The style named `name` ([`name`](Self::name)); unknown names are
    /// [`Single`](Self::Single).
    pub fn from_name(name: &str) -> UnderlineStyle {
        match name {
            "thick" => UnderlineStyle::Thick,
            "double" => UnderlineStyle::Double,
            "dotted" => UnderlineStyle::Dotted,
            "dashed" => UnderlineStyle::Dashed,
            _ => UnderlineStyle::Single,
        }
    }
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
/// with [`on_click`](Span::on_click). Its box, label, icon and identity
/// have their own builders (#329): [`tooltip`](Span::tooltip),
/// [`chip_ring`](Span::chip_ring), [`chip_radius`](Span::chip_radius),
/// [`chip_height`](Span::chip_height),
/// [`chip_max_width`](Span::chip_max_width),
/// [`chip_label_runs`](Span::chip_label_runs),
/// [`chip_icon`](Span::chip_icon) (a symbol or a [`ChipIcon`] drawing),
/// [`chip_state`](Span::chip_state), [`chip_test_id`](Span::chip_test_id)
/// and [`chip_attr`](Span::chip_attr).
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
    /// An inline symbol `points` tall (#221): `icon(name).text_size(points)`.
    /// See [`icon`].
    pub fn icon(name: impl Into<String>, points: f32) -> Span {
        icon(name).text_size(points)
    }

    /// A [`chip`]'s icon, drawn before its label: an SF Symbol name
    /// (`"doc.text"`, a glyph where SF Symbols don't exist, see
    /// [`glyphs`](crate::glyphs)), or a vector drawing (#329): an SVG
    /// ([`ChipIcon::svg`]) or paths ([`ChipIcon::shape`], a
    /// [`Drawing`] or a [`Path`]), one em tall and painted in the chip's
    /// text color where it uses [`Color::current`].
    ///
    /// ```
    /// # use prism_core::prelude::*;
    /// # use prism_core::ChipIcon;
    /// let dot = Path::circle(0.5, 0.5, 0.35).fill(Color::current());
    /// let a = chip("page:1", "Roadmap").chip_icon("doc.text");
    /// let b = chip("user:7", "Ada").chip_icon(dot);
    /// let c = chip("tag:2", "Ops").chip_icon(ChipIcon::svg(
    ///     r#"<svg viewBox="0 0 16 16"><rect x="2" y="2" width="12" height="12" fill="currentColor"/></svg>"#,
    /// ));
    /// assert_eq!(a.chip_icon_value(), Some(ChipIcon::Symbol("doc.text".into())));
    /// assert!(matches!(b.chip_icon_value(), Some(ChipIcon::Vector { .. })));
    /// assert!(matches!(c.chip_icon_value(), Some(ChipIcon::Vector { .. })));
    /// ```
    pub fn chip_icon(mut self, icon: impl Into<ChipIcon>) -> Self {
        match icon.into() {
            ChipIcon::Symbol(name) => {
                self.run.chip_icon = Some(name);
                self.run.chip_drawing = None;
                self.run.chip_icon_ratio = None;
            }
            ChipIcon::Vector { drawing, ratio } => {
                self.run.chip_icon = None;
                self.run.chip_drawing = Some(drawing);
                self.run.chip_icon_ratio = (ratio > 0. && ratio != 1.).then_some(ratio);
            }
        }
        self
    }

    /// A [`chip`]'s icon, if it has one.
    pub fn chip_icon_value(&self) -> Option<ChipIcon> {
        self.run.chip_icon_value()
    }

    /// What the chip (or any run) shows on hover (#329): `title` on the
    /// web, the platform's tool tip natively (macOS, Linux, Windows; on
    /// iOS, iPadOS and visionOS where UIKit shows tool tips: a pointer or
    /// gaze hover; on Android a mouse hover). Touch screens have no hover:
    /// there it is not shown. Natively, only a rich input's chips and
    /// inline icons show it. The way to read a label cut by
    /// [`chip_max_width`](Self::chip_max_width).
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.run.tooltip = Some(text.into());
        self
    }

    /// A ring `width` points wide in `color` drawn inside the chip's box
    /// (#329), its size unchanged (an inset `box-shadow` on the web).
    /// Adaptive colors follow dark mode.
    pub fn chip_ring(mut self, width: f32, color: Color) -> Self {
        self.run.chip_ring = (width > 0.).then_some((width, color));
        self
    }

    /// The radius of the chip's corners, points (#329). By default 4.
    pub fn chip_radius(mut self, points: f32) -> Self {
        self.run.chip_radius = Some(points.max(0.));
        self
    }

    /// The chip's height, points (#329), its icon and label centered in
    /// it; never less than its label's own height (so a larger text size
    /// never cuts it). By default, its label's height.
    pub fn chip_height(mut self, points: f32) -> Self {
        self.run.chip_height = Some(points.max(0.));
        self
    }

    /// The chip's largest width, points, padding and icon included
    /// (#329): a longer label is cut with an ellipsis (all its
    /// [runs](Self::chip_label_runs) together). Give it a
    /// [`tooltip`](Self::tooltip) to read the whole label.
    pub fn chip_max_width(mut self, points: f32) -> Self {
        self.run.chip_max_width = Some(points.max(0.));
        self
    }

    /// A [`chip`]'s label made of several runs, each with its own weight,
    /// italic and text color (#329): a bold name, then a grey summary.
    /// Other span styles are ignored. The chip's plain label (what a copy
    /// writes and screen readers say) becomes their text.
    ///
    /// ```
    /// # use prism_core::prelude::*;
    /// let c = chip("user:7", "").chip_label_runs([
    ///     span("Ada").bold(),
    ///     span(" · 3 tasks").text_color(rgb(0x8e8e93)),
    /// ]);
    /// assert_eq!(c.chip_label(), Some("Ada · 3 tasks"));
    /// ```
    pub fn chip_label_runs(mut self, runs: impl IntoIterator<Item = Span>) -> Self {
        let runs: Vec<TextRun> = runs
            .into_iter()
            .map(|s| TextRun {
                text: s.run.text,
                weight: s.run.weight,
                italic: s.run.italic,
                color: s.run.color,
                ..TextRun::default()
            })
            .filter(|r| !r.text.is_empty())
            .collect();
        self.run.chip_label = Some(runs.iter().map(|r| r.text.as_str()).collect());
        self.run.chip_label_runs = runs;
        self
    }

    /// Marks the chip with a state (#329), one word such as `"open"` or
    /// `"selected"` (letters, digits, `-` and `_`; others are dropped), so
    /// a stylesheet or a test finds it: on the web `data-state="open"`
    /// and the class `prism-chip-open`; on Linux the CSS class
    /// `prism-chip-open` of the chip's widget; on Apple platforms the
    /// `PrismChipState` attribute of its character; on Android the chip
    /// span's `state`. Windows has no place for it in the field (a chip is
    /// a picture RichEdit embeds); it is in the runs there too. The app
    /// styles it on native backends with the other chip builders.
    pub fn chip_state(mut self, state: impl AsRef<str>) -> Self {
        let state: String = state
            .as_ref()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            .collect();
        self.run.chip_state = (!state.is_empty()).then_some(state);
        self
    }

    /// A name tests find the chip by (#329): `data-testid` on the web; the
    /// accessibility identifier of its image on iOS, iPadOS and visionOS;
    /// the `PrismChipTestID` attribute of its character on Apple
    /// platforms; the chip widget's name on Linux; the chip span's
    /// `testId` on Android. Windows has no per-chip automation id (a chip
    /// is a picture RichEdit embeds, named by its label). On every backend
    /// it is in the runs the host receives ([`TextRun::chip_test_id`]),
    /// which `prism-testing` reads (`Host::chip`).
    pub fn chip_test_id(mut self, id: impl Into<String>) -> Self {
        self.run.chip_test_id = Some(id.into());
        self
    }

    /// Sets an attribute of the chip's element on the web (#329), with the
    /// rules of an element's `attr`: a `data-*` name an app may set (see
    /// [`is_app_attribute`](crate::attributes::is_app_attribute)), other
    /// than `data-state` and `data-testid` (see
    /// [`chip_state`](Self::chip_state), [`chip_test_id`](Self::chip_test_id));
    /// other names are ignored and fail a debug assertion. Setting a name
    /// again replaces its value. Native backends ignore it.
    pub fn chip_attr(mut self, name: impl AsRef<str>, value: impl Into<String>) -> Self {
        let name = name.as_ref().to_ascii_lowercase();
        let allowed = name.starts_with("data-")
            && crate::attributes::is_app_attribute(&name)
            && !crate::attributes::CHIP_DATA_ATTRIBUTES.contains(&&name[5..]);
        debug_assert!(
            allowed,
            "`{name}` is not an attribute a chip may set (a `data-*` name, see prism_core::attributes)"
        );
        if allowed {
            let value = value.into();
            match self.run.chip_attrs.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => slot.1 = value,
                None => self.run.chip_attrs.push((name, value)),
            }
        }
        self
    }

    /// Its [`tooltip`](Self::tooltip), if it has one.
    pub fn tooltip_value(&self) -> Option<&str> {
        self.run.tooltip.as_deref()
    }

    /// A [`chip`]'s [state](Self::chip_state), if it has one.
    pub fn chip_state_value(&self) -> Option<&str> {
        self.run.chip_state.as_deref()
    }

    /// A [`chip`]'s [test id](Self::chip_test_id), if it has one.
    pub fn chip_test_id_value(&self) -> Option<&str> {
        self.run.chip_test_id.as_deref()
    }

    /// The run's text: [`OBJECT_CHAR`] for a [`chip`].
    pub fn text(&self) -> &str {
        &self.run.text
    }

    /// The same run with another text (its style, link and handler kept).
    /// A chip or an icon keeps its single [`OBJECT_CHAR`].
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        if !self.run.is_object() {
            self.run.text = text.into();
        }
        self
    }

    /// Its length in characters (a chip or an icon is one).
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

    /// Whether it is an inline [`icon`].
    pub fn is_icon(&self) -> bool {
        self.run.icon.is_some()
    }

    /// An [`icon`]'s symbol name.
    pub fn icon_name(&self) -> Option<&str> {
        self.run.icon.as_deref()
    }

    /// Whether it takes one [`OBJECT_CHAR`] of a rich input's value: a
    /// [`chip`] or an [`icon`].
    pub fn is_object(&self) -> bool {
        self.run.is_object()
    }

    /// Its text color, if it sets one.
    pub fn text_color_value(&self) -> Option<Color> {
        self.run.color
    }

    /// Its underline's color, if it sets one (#221).
    pub fn underline_color_value(&self) -> Option<Color> {
        self.run.underline_color
    }

    /// How its underline is drawn (#221).
    pub fn underline_style_value(&self) -> UnderlineStyle {
        self.run.underline_style
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
    /// text): two such neighbours are one run. Chips and icons never are.
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
        !self.is_object()
            && !other.is_object()
            && handlers
            && self.size == other.size
            && self.label == other.label
            && plain(self) == plain(other)
    }

    /// What a read-only text shows: a chip's label, an icon's fallback
    /// glyph, else the text.
    pub(crate) fn display_text(&self) -> &str {
        self.run.display_text()
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
    /// The paragraph's weight again, not bold (#294): what a « bold »
    /// command toggling off does to the selected runs.
    pub fn no_bold(mut self) -> Self {
        self.run.weight = None;
        self
    }
    /// Italic.
    pub fn italic(mut self) -> Self {
        self.run.italic = true;
        self
    }
    /// Not italic (#294).
    pub fn no_italic(mut self) -> Self {
        self.run.italic = false;
        self
    }
    /// Makes the run a link to an app path or a URL (#294), underlined
    /// like [`link`]: what a « set link » command does to the selected
    /// runs.
    pub fn link_to(mut self, path: impl Into<String>) -> Self {
        self.run.href = Some(path.into());
        self.run.underline = true;
        self
    }
    /// The run without its link and the link's underline (#294).
    pub fn no_link(mut self) -> Self {
        if self.run.href.take().is_some() {
            self.run.underline = false;
        }
        self
    }
    /// Underlined.
    pub fn underline(mut self) -> Self {
        self.run.underline = true;
        self
    }
    /// Underlined in `c` (#221), whatever the text's color: a commented
    /// passage, a spelling mark. Adaptive colors follow dark mode.
    pub fn underline_color(mut self, c: Color) -> Self {
        self.run.underline = true;
        self.run.underline_color = Some(c);
        self
    }
    /// Underlined with this line (#221): thick, double, dotted, dashed.
    pub fn underline_style(mut self, style: UnderlineStyle) -> Self {
        self.run.underline = true;
        self.run.underline_style = style;
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
    /// The paragraph's text color again (#299).
    pub fn no_text_color(mut self) -> Self {
        self.run.color = None;
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
    /// The text. An icon's fallback glyph in a read-only text; a chip's
    /// label there; [`OBJECT_CHAR`] for both in a rich input.
    pub text: String,
    /// Weight (`None`: the paragraph's).
    pub weight: Option<FontWeight>,
    /// Italic.
    pub italic: bool,
    /// Underlined.
    pub underline: bool,
    /// Underline color (`None`: the text's, #221).
    pub underline_color: Option<Color>,
    /// How the underline is drawn (#221).
    pub underline_style: UnderlineStyle,
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
    /// An inline symbol (SF Symbol name). In a read-only text, `text` is
    /// its fallback glyph; in a rich input, [`OBJECT_CHAR`].
    pub icon: Option<String>,
    /// An inline object ([`chip`], #115): the app's key for it. Its `text`
    /// is [`OBJECT_CHAR`] in a rich input, its label in a read-only text.
    pub chip: Option<String>,
    /// What a chip shows (and screen readers say).
    pub chip_label: Option<String>,
    /// A chip's symbol, drawn before its label (SF Symbol name).
    pub chip_icon: Option<String>,
    /// A chip's vector icon (#329), drawn before its label in place of a
    /// symbol: one em tall, [`chip_icon_ratio`](Self::chip_icon_ratio)
    /// times as wide, [`Color::current`] being the chip's text color.
    pub chip_drawing: Option<Drawing>,
    /// The width of [`chip_drawing`](Self::chip_drawing) over its height
    /// (`None`: 1, a square).
    pub chip_icon_ratio: Option<f32>,
    /// What the run shows on hover (#329).
    pub tooltip: Option<String>,
    /// A ring inside the chip's box (#329): its width (points) and color.
    pub chip_ring: Option<(f32, Color)>,
    /// The radius of the chip's corners, points (`None`: 4, #329).
    pub chip_radius: Option<f32>,
    /// The chip's height, points (`None`: its label's, #329); never less
    /// than its label's.
    pub chip_height: Option<f32>,
    /// The chip's largest width, points, its label cut with an ellipsis
    /// (#329).
    pub chip_max_width: Option<f32>,
    /// A chip's label as styled runs (#329): their `text`, `weight`,
    /// `italic` and `color`. Empty: `chip_label` alone, in the run's style.
    /// Their texts make `chip_label`.
    pub chip_label_runs: Vec<TextRun>,
    /// A chip's state (#329): `data-state` and a `prism-chip-<state>`
    /// class on the web.
    pub chip_state: Option<String>,
    /// A chip's test id (#329): `data-testid` on the web, an accessibility
    /// identifier or test hook natively.
    pub chip_test_id: Option<String>,
    /// A chip's `data-*` attributes on the web (#329).
    pub chip_attrs: Vec<(String, String)>,
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

    /// Whether the run takes one [`OBJECT_CHAR`] in a rich input: a
    /// [`chip`] or an [`icon`].
    pub fn is_object(&self) -> bool {
        self.chip.is_some() || self.icon.is_some()
    }

    /// A chip's icon (#329): its symbol or its vector drawing.
    pub fn chip_icon_value(&self) -> Option<ChipIcon> {
        if let Some(d) = &self.chip_drawing {
            return Some(ChipIcon::Vector {
                drawing: d.clone(),
                ratio: self.chip_icon_ratio.unwrap_or(1.),
            });
        }
        self.chip_icon.clone().map(ChipIcon::Symbol)
    }

    /// A chip's label runs (#329): [`chip_label_runs`](Self::chip_label_runs),
    /// or its plain label as one run in the chip's own style.
    pub fn label_runs(&self) -> Vec<TextRun> {
        if !self.chip_label_runs.is_empty() {
            return self.chip_label_runs.clone();
        }
        vec![TextRun {
            text: self.chip_label.clone().unwrap_or_default(),
            ..TextRun::default()
        }]
    }

    /// What a read-only text shows for the run: a chip's label, an icon's
    /// fallback glyph ([`glyphs::glyph`](crate::glyphs::glyph)), else its text.
    pub fn display_text(&self) -> &str {
        match (&self.chip, &self.chip_label, &self.icon) {
            (Some(_), Some(label), _) => label,
            (None, _, Some(name)) if self.text == OBJECT_CHAR.to_string() => {
                crate::glyphs::glyph(name)
            }
            _ => &self.text,
        }
    }
}

const RUN_SEP: char = '\u{1e}';
const FIELD_SEP: char = '\u{1f}';
/// Separators of a list inside one field (a chip's label runs and
/// attributes, #329): items, then the parts of an item.
const ITEM_SEP: char = '\u{1d}';
const PART_SEP: char = '\u{1c}';

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
/// `rounded` or a name), `uc=…` underline color, `us=thick` underline style
/// (`double`, `dotted`, `dashed`; none: single), `z=1.25` size (× the
/// paragraph's), `l=123` link id,
/// `h=/path` href, `g=star.fill` icon, `tt=text` tooltip, and for a
/// [`chip`] `k=key`, `n=label`, `m=icon`, and (#329) `v=` a vector icon
/// ([`Drawing::encode`]) with `vr=1.5` its width over its height,
/// `ring=1.5,#RRGGBBAA` its ring, `rad=6` its radius, `ht=22` its height,
/// `mw=160` its largest width, `nr=` its label runs (each its text then
/// `w=`, `i`, `c=` fields, separated by U+001C; runs separated by
/// U+001D), `st=open` its state, `tid=` its test id and `at=` its
/// attributes (name U+001C value, separated by U+001D). Control
/// characters U+001C to U+001F are dropped from texts.
pub fn encode_runs(runs: &[TextRun]) -> String {
    let clean = |s: &str| s.replace([RUN_SEP, FIELD_SEP, ITEM_SEP, PART_SEP], "");
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
        if let Some(c) = r.underline_color {
            field(format!("uc={}", color_code(c)));
        }
        if r.underline_style != UnderlineStyle::Single {
            field(format!("us={}", r.underline_style.name()));
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
        if let Some(d) = &r.chip_drawing {
            field(format!("v={}", clean(&d.encode())));
        }
        if let Some(ratio) = r.chip_icon_ratio {
            field(format!("vr={ratio}"));
        }
        if let Some(t) = &r.tooltip {
            field(format!("tt={}", clean(t)));
        }
        if let Some((w, c)) = r.chip_ring {
            field(format!("ring={w},{}", color_code(c)));
        }
        if let Some(v) = r.chip_radius {
            field(format!("rad={v}"));
        }
        if let Some(v) = r.chip_height {
            field(format!("ht={v}"));
        }
        if let Some(v) = r.chip_max_width {
            field(format!("mw={v}"));
        }
        if !r.chip_label_runs.is_empty() {
            let runs: Vec<String> = r
                .chip_label_runs
                .iter()
                .map(|l| {
                    let mut o = clean(&l.text);
                    if let Some(w) = l.weight {
                        o.push(PART_SEP);
                        o.push_str(&format!("w={}", w.css()));
                    }
                    if l.italic {
                        o.push(PART_SEP);
                        o.push('i');
                    }
                    if let Some(c) = l.color {
                        o.push(PART_SEP);
                        o.push_str(&format!("c={}", color_code(c)));
                    }
                    o
                })
                .collect();
            field(format!("nr={}", runs.join(&ITEM_SEP.to_string())));
        }
        if let Some(st) = &r.chip_state {
            field(format!("st={}", clean(st)));
        }
        if let Some(t) = &r.chip_test_id {
            field(format!("tid={}", clean(t)));
        }
        if !r.chip_attrs.is_empty() {
            let attrs: Vec<String> = r
                .chip_attrs
                .iter()
                .map(|(n, v)| format!("{}{PART_SEP}{}", clean(n), clean(v)))
                .collect();
            field(format!("at={}", attrs.join(&ITEM_SEP.to_string())));
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
                    "uc" => run.underline_color = parse_color(v),
                    "us" => run.underline_style = UnderlineStyle::from_name(v),
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
                    "v" => run.chip_drawing = Some(Drawing::decode(v)),
                    "vr" => run.chip_icon_ratio = v.parse().ok().filter(|r: &f32| *r > 0.),
                    "tt" => run.tooltip = Some(v.to_string()),
                    "ring" => {
                        run.chip_ring = v.split_once(',').and_then(|(w, c)| {
                            Some((w.parse().ok().filter(|w: &f32| *w > 0.)?, parse_color(c)?))
                        })
                    }
                    "rad" => run.chip_radius = v.parse().ok(),
                    "ht" => run.chip_height = v.parse().ok(),
                    "mw" => run.chip_max_width = v.parse().ok(),
                    "nr" => run.chip_label_runs = decode_label_runs(v),
                    "st" => run.chip_state = Some(v.to_string()),
                    "tid" => run.chip_test_id = Some(v.to_string()),
                    "at" => {
                        run.chip_attrs = v
                            .split(ITEM_SEP)
                            .filter_map(|a| a.split_once(PART_SEP))
                            .map(|(n, v)| (n.to_string(), v.to_string()))
                            .collect()
                    }
                    _ => {}
                }
            }
            run
        })
        .collect()
}

/// A chip's label runs as `nr=` encodes them (#329).
fn decode_label_runs(v: &str) -> Vec<TextRun> {
    v.split(ITEM_SEP)
        .map(|r| {
            let mut parts = r.split(PART_SEP);
            let mut run = TextRun {
                text: parts.next().unwrap_or("").to_string(),
                ..TextRun::default()
            };
            for f in parts {
                let (k, v) = f.split_once('=').unwrap_or((f, ""));
                match k {
                    "w" => run.weight = v.parse().ok().map(FontWeight::from_css),
                    "i" => run.italic = true,
                    "c" => run.color = parse_color(v),
                    _ => {}
                }
            }
            run
        })
        .filter(|r| !r.text.is_empty())
        .collect()
}

// --- Chip icons (#329) ----------------------------------------------------------

/// What a [`chip`] draws before its label ([`Span::chip_icon`], #329): a
/// symbol, or a vector drawing one em tall.
#[derive(Clone, Debug, PartialEq)]
pub enum ChipIcon {
    /// An SF Symbol name (a glyph where SF Symbols don't exist, see
    /// [`glyphs`](crate::glyphs)).
    Symbol(String),
    /// Vector paths in fractions of the icon's box (see
    /// [`shape`](mod@crate::shape)), one em tall and `ratio` times as wide.
    Vector {
        /// What it draws; [`Color::current`] is the chip's text color.
        drawing: Drawing,
        /// Its width over its height.
        ratio: f32,
    },
}

impl ChipIcon {
    /// An SVG document as the icon (what [`svg`](crate::svg()) draws,
    /// see [`crate::svg`](mod@crate::svg)), keeping its proportions; its
    /// `currentColor` is the chip's text color. Unreadable markup draws
    /// nothing.
    pub fn svg(markup: impl AsRef<str>) -> ChipIcon {
        match crate::svg::parse(markup.as_ref()) {
            Some(s) => ChipIcon::Vector {
                drawing: s.drawing,
                ratio: if s.width > 0. && s.height > 0. {
                    s.width / s.height
                } else {
                    1.
                },
            },
            None => ChipIcon::Vector {
                drawing: Drawing::default(),
                ratio: 1.,
            },
        }
    }

    /// Paths as a square icon (like a [`shape`](crate::shape())'s, in
    /// fractions of its box).
    pub fn shape(paths: impl IntoIterator<Item = Path>) -> ChipIcon {
        ChipIcon::Vector {
            drawing: Drawing {
                paths: paths.into_iter().collect(),
                ..Drawing::default()
            },
            ratio: 1.,
        }
    }
}

impl From<&str> for ChipIcon {
    fn from(name: &str) -> Self {
        ChipIcon::Symbol(name.to_string())
    }
}

impl From<String> for ChipIcon {
    fn from(name: String) -> Self {
        ChipIcon::Symbol(name)
    }
}

impl From<&String> for ChipIcon {
    fn from(name: &String) -> Self {
        ChipIcon::Symbol(name.clone())
    }
}

impl From<Drawing> for ChipIcon {
    fn from(drawing: Drawing) -> Self {
        ChipIcon::Vector { drawing, ratio: 1. }
    }
}

impl From<Path> for ChipIcon {
    fn from(path: Path) -> Self {
        ChipIcon::shape([path])
    }
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

/// `span` cut at character `at` (not a chip or an icon: they have one).
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
/// chip or an icon, not a link (typing at the end of a link writes plain text).
fn continues(s: &Span) -> bool {
    !s.is_object() && !s.is_clickable()
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
        (Some(l), Some(r)) if std::ptr::eq(l, r) && !l.is_object() => Some(l),
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
/// text, no link id, as its encoding reads back (colors, sizes and a
/// chip's drawing as precise as the editor reports them).
pub(crate) fn run_key(run: &TextRun) -> TextRun {
    let plain = TextRun {
        text: String::new(),
        link: None,
        ..run.clone()
    };
    decode_runs(&encode_runs(std::slice::from_ref(&plain)))
        .pop()
        .unwrap_or(plain)
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
        if s.is_object() {
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
