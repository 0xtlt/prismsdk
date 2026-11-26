//! Styling primitives shared by every backend.
//!
//! The model is a flexbox subset (like React Native): every node is a flex
//! container, the default direction is `Column`, and text styles apply to the
//! node they are set on (no inheritance), so the web and native backends agree.

/// An sRGB color with straight alpha, components in `0.0..=1.0`.
///
/// A color can be **adaptive**: `Color::adaptive(rgb(0xFFFFFF), rgb(0x1C1C22))`
/// carries a dark-mode variant that every backend resolves natively (CSS
/// `light-dark()`, dynamic `UIColor`/`NSColor`, SwiftUI `colorScheme` in
/// widgets and Live Activities), so it follows the system appearance live.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    /// Red, `0.0..=1.0`.
    pub r: f32,
    /// Green, `0.0..=1.0`.
    pub g: f32,
    /// Blue, `0.0..=1.0`.
    pub b: f32,
    /// Alpha (opacity), `0.0..=1.0`.
    pub a: f32,
    /// Dark-mode variant `[r, g, b, a]`, if the color adapts.
    pub dark: Option<[f32; 4]>,
}

impl Color {
    /// Opaque black.
    pub const BLACK: Color = Color {
        r: 0.,
        g: 0.,
        b: 0.,
        a: 1.,
        dark: None,
    };
    /// Opaque white.
    pub const WHITE: Color = Color {
        r: 1.,
        g: 1.,
        b: 1.,
        a: 1.,
        dark: None,
    };
    /// Fully transparent.
    pub const TRANSPARENT: Color = Color {
        r: 0.,
        g: 0.,
        b: 0.,
        a: 0.,
        dark: None,
    };

    /// The same color with alpha `a` (both variants of an adaptive color).
    pub fn with_alpha(self, a: f32) -> Color {
        Color {
            a,
            dark: self.dark.map(|[r, g, b, _]| [r, g, b, a]),
            ..self
        }
    }

    /// A color with a light and a dark variant, resolved by the platform.
    pub fn adaptive(light: Color, dark: Color) -> Color {
        Color {
            dark: Some([dark.r, dark.g, dark.b, dark.a]),
            ..light
        }
    }

    /// The light (or only) variant.
    pub fn light_variant(self) -> Color {
        Color { dark: None, ..self }
    }

    /// The dark variant (the color itself if it does not adapt).
    pub fn dark_variant(self) -> Color {
        match self.dark {
            Some([r, g, b, a]) => Color {
                r,
                g,
                b,
                a,
                dark: None,
            },
            None => self,
        }
    }

    /// CSS notation; adaptive colors use `light-dark()`, [`Color::current`]
    /// `color-mix()` with `currentColor`.
    pub fn to_css(self) -> String {
        if self.is_current() {
            return format!(
                "color-mix(in srgb, currentColor {}%, transparent)",
                (self.a.clamp(0., 1.) * 1000.).round() / 10.
            );
        }
        let one = |c: Color| {
            let v = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
            format!("rgba({},{},{},{})", v(c.r), v(c.g), v(c.b), c.a)
        };
        match self.dark {
            Some(_) => format!(
                "light-dark({}, {})",
                one(self.light_variant()),
                one(self.dark_variant())
            ),
            None => one(self),
        }
    }

    /// `#RRGGBBAA` of one variant.
    pub fn hex(self) -> String {
        let v = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
        format!(
            "#{:02X}{:02X}{:02X}{:02X}",
            v(self.r),
            v(self.g),
            v(self.b),
            v(self.a)
        )
    }
}

/// `adaptive(light, dark)`: shorthand for [`Color::adaptive`].
pub fn adaptive(light: Color, dark: Color) -> Color {
    Color::adaptive(light, dark)
}

/// `rgb(0xRRGGBB)`.
pub fn rgb(hex: u32) -> Color {
    rgba(hex, 1.)
}

/// `rgba(0xRRGGBB, alpha)`.
pub fn rgba(hex: u32, a: f32) -> Color {
    let ch = |shift: u32| ((hex >> shift) & 0xff) as f32 / 255.;
    Color {
        r: ch(16),
        g: ch(8),
        b: ch(0),
        a,
        dark: None,
    }
}

/// A length along one axis.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Length {
    /// Sized by content and flex rules.
    #[default]
    Auto,
    /// Logical points (CSS px, AppKit/UIKit points).
    Px(f32),
    /// Fraction of the parent's size, `1.0` is 100%.
    Fraction(f32),
}

impl From<f32> for Length {
    fn from(v: f32) -> Self {
        Length::Px(v)
    }
}

impl From<i32> for Length {
    fn from(v: i32) -> Self {
        Length::Px(v as f32)
    }
}

/// `px(12.)` → a length in logical points.
pub fn px(v: f32) -> Length {
    Length::Px(v)
}

/// `relative(0.5)` → 50% of the parent.
pub fn relative(f: f32) -> Length {
    Length::Fraction(f)
}

/// Main axis of a flex container (`flex-direction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FlexDirection {
    /// Children side by side (leading to trailing).
    Row,
    /// Children stacked top to bottom (the default).
    #[default]
    Column,
    /// Children side by side from trailing to leading (`row-reverse`): the
    /// last child at the leading edge, and `justify_start` packs them
    /// against the trailing edge.
    RowReverse,
    /// Children stacked bottom to top (`column-reverse`): the last child on
    /// top, and `justify_start` packs them against the bottom.
    ColumnReverse,
}

impl FlexDirection {
    /// `Row` or `RowReverse`: the main axis is horizontal.
    pub fn is_row(self) -> bool {
        matches!(self, FlexDirection::Row | FlexDirection::RowReverse)
    }

    /// `RowReverse` or `ColumnReverse`: children run from the end of the
    /// main axis.
    pub fn is_reverse(self) -> bool {
        matches!(
            self,
            FlexDirection::RowReverse | FlexDirection::ColumnReverse
        )
    }
}

/// Cross-axis alignment (`align-items` / `align-self`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    /// Aligned to the leading or top edge.
    Start,
    /// Centered.
    Center,
    /// Aligned to the trailing or bottom edge.
    End,
    /// Stretched to fill the cross axis (the default).
    #[default]
    Stretch,
    /// First text baselines lined up (a unit beside a big figure, an icon
    /// beside a label). Native backends measure each leaf's baseline.
    Baseline,
}

/// Main-axis distribution (`justify-content`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Justify {
    /// Packed at the start (the default).
    #[default]
    Start,
    /// Packed in the center.
    Center,
    /// Packed at the end.
    End,
    /// Equal space between children, none at the edges.
    SpaceBetween,
    /// Equal space around each child (half at the edges).
    SpaceAround,
    /// Equal space between children and at the edges.
    SpaceEvenly,
    /// A grid's `auto` columns grow to fill its width (CSS
    /// `justify-content: stretch`, #284); flex containers pack their
    /// children at the start, as with [`Start`](Justify::Start).
    Stretch,
}

/// Font weight: the CSS scale from 100 (thin) to 900 (black). Any number
/// in between works too ([`FontWeight::Custom`], e.g. 650): variable fonts
/// (the system fonts, most web fonts) render it exactly, others use the
/// nearest face.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum FontWeight {
    /// 400 (the default).
    #[default]
    Regular,
    /// 500.
    Medium,
    /// 600.
    Semibold,
    /// 700.
    Bold,
    /// 100.
    Thin,
    /// 200.
    ExtraLight,
    /// 300.
    Light,
    /// 800.
    ExtraBold,
    /// 900.
    Black,
    /// Any weight from 100 to 900 (clamped), e.g. `FontWeight::Custom(650)`.
    Custom(u16),
}

impl FontWeight {
    /// The CSS `font-weight` number (100 to 900).
    pub fn css(self) -> u16 {
        match self {
            FontWeight::Thin => 100,
            FontWeight::ExtraLight => 200,
            FontWeight::Light => 300,
            FontWeight::Regular => 400,
            FontWeight::Medium => 500,
            FontWeight::Semibold => 600,
            FontWeight::Bold => 700,
            FontWeight::ExtraBold => 800,
            FontWeight::Black => 900,
            FontWeight::Custom(w) => w.clamp(100, 900),
        }
    }

    /// The weight for a CSS number (named when it is a multiple of 100).
    pub fn from_css(w: u16) -> FontWeight {
        match w.clamp(100, 900) {
            100 => FontWeight::Thin,
            200 => FontWeight::ExtraLight,
            300 => FontWeight::Light,
            400 => FontWeight::Regular,
            500 => FontWeight::Medium,
            600 => FontWeight::Semibold,
            700 => FontWeight::Bold,
            800 => FontWeight::ExtraBold,
            900 => FontWeight::Black,
            w => FontWeight::Custom(w),
        }
    }

    /// The nearest multiple of 100 (for platforms with nine fixed weights).
    pub fn nearest_hundred(self) -> u16 {
        (self.css() + 50) / 100 * 100
    }
}

impl From<u16> for FontWeight {
    fn from(w: u16) -> Self {
        FontWeight::from_css(w)
    }
}

/// `Left` and `Right` are the leading and trailing edges: they swap in
/// right-to-left languages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextAlign {
    /// Leading edge (the default).
    #[default]
    Left,
    /// Centered.
    Center,
    /// Trailing edge.
    Right,
}

/// Per-edge values (padding, margin).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Edges {
    /// Top, points.
    pub top: f32,
    /// Right (trailing in right-to-left layouts), points.
    pub right: f32,
    /// Bottom, points.
    pub bottom: f32,
    /// Left (leading in left-to-right layouts), points.
    pub left: f32,
}

impl Edges {
    /// Whether every edge is zero.
    pub fn is_zero(&self) -> bool {
        (self.top, self.right, self.bottom, self.left) == (0., 0., 0., 0.)
    }
    /// Left and right swapped (leading/trailing in a right-to-left layout).
    pub fn mirrored(self) -> Self {
        Edges {
            left: self.right,
            right: self.left,
            ..self
        }
    }
    /// The same value on every edge.
    pub fn all(v: f32) -> Self {
        Edges {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }
}

/// How a node is placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Position {
    /// In the flex flow of its parent.
    #[default]
    Relative,
    /// Out of the flow, at its [`Inset`] from the parent's edges (padding
    /// box), drawn over earlier siblings: overlays, toasts, badges.
    Absolute,
    /// In the flow, but stays `inset.top` points from the top of its scroll
    /// view while its parent is visible (`position: sticky`): section
    /// headers, a toolbar that stays.
    Sticky,
    /// Out of the flow, at its [`Inset`] from the window's edges (natively:
    /// from the screen it is on, inside the safe area), whatever its parent
    /// and however far its scroll view is scrolled: floating action
    /// buttons, a floating tab bar.
    Fixed,
}

/// Distances from the parent's edges of an absolutely positioned node
/// (`Auto`: unconstrained). `left` and `right` are the leading and trailing
/// edges: they swap in right-to-left languages, like margins.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Inset {
    /// Distance from the top edge.
    pub top: Length,
    /// Distance from the right (trailing) edge.
    pub right: Length,
    /// Distance from the bottom edge.
    pub bottom: Length,
    /// Distance from the left (leading) edge.
    pub left: Length,
}

/// Which corners of a box [`Style::corner_radius`] rounds. Leading and
/// trailing follow the text direction (left and right in left-to-right
/// languages), like margins. Combine with `|`: `Corners::TOP | Corners::BOTTOM_LEADING`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Corners(u8);

impl Corners {
    /// No corner.
    pub const NONE: Corners = Corners(0);
    /// The top leading corner (top left in left-to-right languages).
    pub const TOP_LEADING: Corners = Corners(1);
    /// The top trailing corner (top right in left-to-right languages).
    pub const TOP_TRAILING: Corners = Corners(2);
    /// The bottom trailing corner (bottom right in left-to-right languages).
    pub const BOTTOM_TRAILING: Corners = Corners(4);
    /// The bottom leading corner (bottom left in left-to-right languages).
    pub const BOTTOM_LEADING: Corners = Corners(8);
    /// Both top corners.
    pub const TOP: Corners = Corners(1 | 2);
    /// Both bottom corners.
    pub const BOTTOM: Corners = Corners(4 | 8);
    /// Both leading corners.
    pub const LEADING: Corners = Corners(1 | 8);
    /// Both trailing corners.
    pub const TRAILING: Corners = Corners(2 | 4);
    /// Every corner (the default).
    pub const ALL: Corners = Corners(15);

    /// Whether every corner of `other` is in `self`.
    pub fn contains(self, other: Corners) -> bool {
        self.0 & other.0 == other.0
    }

    /// The corners as bits: 1 top leading, 2 top trailing, 4 bottom trailing,
    /// 8 bottom leading.
    pub fn bits(self) -> u8 {
        self.0
    }

    /// The corners for `bits` (see [`bits`](Self::bits)); other bits are ignored.
    pub fn from_bits(bits: u8) -> Corners {
        Corners(bits & 15)
    }

    /// Leading and trailing swapped (right-to-left layouts).
    pub fn mirrored(self) -> Corners {
        let b = self.0;
        Corners((b & 1) << 1 | (b & 2) >> 1 | (b & 4) << 1 | (b & 8) >> 1)
    }
}

impl Default for Corners {
    fn default() -> Self {
        Corners::ALL
    }
}

impl std::ops::BitOr for Corners {
    type Output = Corners;
    fn bitor(self, rhs: Corners) -> Corners {
        Corners(self.0 | rhs.0)
    }
}

/// A font family. `System` is the platform's UI font (San Francisco,
/// Roboto, Cantarell…, `system-ui` on the web); `Monospace`, `Serif` and
/// `Rounded` are the platform's own designs of those (SF Mono, New York and
/// SF Rounded on Apple platforms). `Named` is a family by name: an installed
/// font, or one the app bundles in `assets/fonts/` (see the typography
/// guide).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum FontFamily {
    /// The platform's UI font (the default).
    #[default]
    System,
    /// A fixed-width font (codes, identifiers, addresses).
    Monospace,
    /// A serif font.
    Serif,
    /// A rounded font (the system font where there is none).
    Rounded,
    /// A family by name (`"Inter"`), with the system font as fallback.
    Named(String),
}

impl FontFamily {
    /// A CSS `font-family` value (with fallbacks).
    pub fn css(&self) -> String {
        const SYSTEM: &str = "-apple-system,BlinkMacSystemFont,\"Segoe UI\",system-ui,sans-serif";
        match self {
            FontFamily::System => SYSTEM.into(),
            FontFamily::Monospace => {
                "ui-monospace,SFMono-Regular,Menlo,Consolas,\"Liberation Mono\",monospace".into()
            }
            FontFamily::Serif => "ui-serif,\"New York\",Georgia,\"Times New Roman\",serif".into(),
            FontFamily::Rounded => format!("ui-rounded,\"SF Pro Rounded\",{SYSTEM}"),
            FontFamily::Named(name) => {
                format!("\"{}\",{SYSTEM}", name.replace(['"', '\\', '<', '>'], ""))
            }
        }
    }

    /// Short name: `system`, `mono`, `serif`, `rounded`, or the family name
    /// (what hosts and text runs receive).
    pub fn encode(&self) -> &str {
        match self {
            FontFamily::System => "system",
            FontFamily::Monospace => "mono",
            FontFamily::Serif => "serif",
            FontFamily::Rounded => "rounded",
            FontFamily::Named(name) => name,
        }
    }

    /// Parses [`encode`](Self::encode)'s output.
    pub fn decode(s: &str) -> FontFamily {
        match s {
            "" | "system" => FontFamily::System,
            "mono" => FontFamily::Monospace,
            "serif" => FontFamily::Serif,
            "rounded" => FontFamily::Rounded,
            name => FontFamily::Named(name.to_string()),
        }
    }
}

impl From<&str> for FontFamily {
    fn from(name: &str) -> Self {
        FontFamily::Named(name.to_string())
    }
}

impl From<String> for FontFamily {
    fn from(name: String) -> Self {
        FontFamily::Named(name)
    }
}

/// Case of the rendered text (`text-transform`). The content itself, and
/// what screen readers read, is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TextTransform {
    /// As written (the default).
    #[default]
    None,
    /// ALL CAPITALS.
    Uppercase,
    /// all lower case.
    Lowercase,
    /// The First Letter Of Each Word In Capitals.
    Capitalize,
}

impl TextTransform {
    /// `text` transformed (Unicode case mapping, like CSS).
    pub fn apply(self, text: &str) -> String {
        self.apply_after("", text)
    }

    /// `text` transformed as it reads after `before`, the text drawn just
    /// before it in the same paragraph. Rich-text runs pass the previous
    /// run, so a word cut by a run boundary (`span("Hel")`, `span("lo")`)
    /// is capitalized once, like CSS across inline elements.
    pub fn apply_after(self, before: &str, text: &str) -> String {
        match self {
            TextTransform::None => text.to_string(),
            TextTransform::Uppercase => text.to_uppercase(),
            TextTransform::Lowercase => text.to_lowercase(),
            TextTransform::Capitalize => {
                let mut out = String::with_capacity(text.len());
                let mut start = before.chars().last().is_none_or(starts_word_after);
                for c in text.chars() {
                    if start && c.is_alphanumeric() {
                        out.extend(c.to_uppercase());
                    } else {
                        out.push(c);
                    }
                    start = starts_word_after(c);
                }
                out
            }
        }
    }

    /// The CSS keyword.
    pub fn css(self) -> &'static str {
        match self {
            TextTransform::None => "none",
            TextTransform::Uppercase => "uppercase",
            TextTransform::Lowercase => "lowercase",
            TextTransform::Capitalize => "capitalize",
        }
    }
}

/// How the lines of a wrapping text are broken (CSS `text-wrap`, #95).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TextWrap {
    /// The platform's usual breaking (the default).
    #[default]
    Auto,
    /// Avoids a lone word on the last line: subtitles, short paragraphs.
    Pretty,
    /// Lines of about the same length: headings, titles of a few lines.
    Balance,
}

impl TextWrap {
    /// The CSS keyword.
    pub fn css(self) -> &'static str {
        match self {
            TextWrap::Auto => "wrap",
            TextWrap::Pretty => "pretty",
            TextWrap::Balance => "balance",
        }
    }
}

/// Whether a word starts after `c`: anything but a letter, a digit or an
/// apostrophe (« it's », « l'été » keep one capital, like CSS).
fn starts_word_after(c: char) -> bool {
    !c.is_alphanumeric() && c != '\'' && c != '\u{2019}'
}

/// Default text size, in points.
pub const DEFAULT_FONT_SIZE: f32 = 16.;

/// The complete style of a node.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    /// Removed from layout and hidden (`display: none`).
    pub display_none: bool,
    /// Main axis of the children (`Column` by default).
    pub direction: FlexDirection,
    /// Children wrap onto new lines (`flex-wrap`).
    pub wrap: bool,
    /// Cross-axis alignment of the children.
    pub align_items: Align,
    /// Cross-axis alignment of this node, overriding its parent's `align_items`.
    pub align_self: Option<Align>,
    /// Main-axis distribution of the children.
    pub justify_content: Justify,
    /// Space between children, points.
    pub gap: f32,
    /// Inner spacing, points, on containers and leaves alike (see
    /// [`Styled::p`]).
    pub padding: Edges,
    /// Outer spacing, points.
    pub margin: Edges,
    /// Sides whose margin is `auto` (#228), taking the free space like CSS:
    /// top, trailing, bottom, leading (the order of [`Edges`]). Overrides
    /// the side's `margin`.
    pub margin_auto: [bool; 4],
    /// In the flow (default) or absolutely positioned at `inset`.
    pub position: Position,
    /// Distances from the parent's edges when `position` is `Absolute`.
    pub inset: Inset,
    /// Width.
    pub width: Length,
    /// Height.
    pub height: Length,
    /// Minimum width.
    pub min_width: Length,
    /// Minimum height.
    pub min_height: Length,
    /// Maximum width.
    pub max_width: Length,
    /// Maximum height.
    pub max_height: Length,
    /// Share of the free space this node grows into (`0` by default).
    pub flex_grow: f32,
    /// How much this node shrinks when space is short (`1` by default).
    pub flex_shrink: f32,
    /// Main-axis size before growing or shrinking.
    pub flex_basis: Length,
    /// Place among its siblings (`order`, `0` by default): the layout sorts
    /// a parent's children by it, keeping their order among equals. Only the
    /// visual position changes: focus and assistive technologies keep the
    /// children's order in the tree.
    pub order: i32,

    /// Background color.
    pub background: Option<Color>,
    /// Corner radius, points.
    pub corner_radius: f32,
    /// The corners `corner_radius` rounds (all by default; the others stay
    /// square). In the style backends receive from prism-native, leading
    /// means left: right-to-left layouts are already mirrored.
    pub rounded_corners: Corners,
    /// Border width, points.
    pub border_width: f32,
    /// Border color.
    pub border_color: Option<Color>,
    /// Opacity, `0.0..=1.0` (`1` by default).
    pub opacity: f32,

    /// Text color (the platform's default label color when unset).
    pub text_color: Option<Color>,
    /// Text size, points ([`DEFAULT_FONT_SIZE`] by default).
    pub font_size: f32,
    /// Text weight.
    pub font_weight: FontWeight,
    /// Text alignment.
    pub text_align: TextAlign,

    /// Visual offset, scale and rotation (no effect on layout).
    pub transform: crate::animation::Transform,
    /// Animates frame, opacity, color and transform changes (skipped with
    /// reduce motion).
    pub transition: Option<Transition>,
    /// Enter, exit and looping animations.
    pub animations: crate::animation::Animations,

    /// Changes applied from a window width up (`.md(|s| s.flex_row())`) or
    /// on some platforms (`.on(Platform::Web, |s| …)`).
    pub responsive: Responsive,
    /// Platforms where the element exists (`.only(…)`, `.except(…)`).
    pub platforms: Option<crate::platform::Platforms>,

    // Grid, per-axis gaps, aspect ratio, scroll axis (see `crate::layout`).
    /// Lays the children out on a grid (`display: grid`) instead of flexbox.
    pub grid: bool,
    /// Column tracks of a grid (`grid-template-columns`; empty: one column).
    pub grid_columns: Vec<crate::layout::GridTrack>,
    /// Row tracks of a grid (`grid-template-rows`; empty: rows as tall as
    /// their content, every cell of a row as tall as the row).
    pub grid_rows: Vec<crate::layout::GridTrack>,
    /// Size of the rows not listed in `grid_rows` (`grid-auto-rows`; `None`: auto).
    pub grid_auto_rows: Option<crate::layout::GridTrack>,
    /// The column line this grid item starts at, from 1 (negative: from the
    /// end); `None`: placed automatically.
    pub col_start: Option<i16>,
    /// How many columns this grid item spans (1 by default).
    pub col_span: u16,
    /// The row line this grid item starts at, from 1; `None`: automatic.
    pub row_start: Option<i16>,
    /// How many rows this grid item spans (1 by default).
    pub row_span: u16,
    /// Space between columns (and between the items of a row), overriding `gap`.
    pub gap_x: Option<f32>,
    /// Space between rows (and between the items of a column), overriding `gap`.
    pub gap_y: Option<f32>,
    /// Width ÷ height: the other side follows the one that is known (CSS
    /// `aspect-ratio`). [`svg`](crate::svg()) pictures keep their `viewBox`
    /// proportions with it.
    pub aspect_ratio: Option<f32>,
    /// Which way a scroll view scrolls.
    pub scroll_axis: crate::layout::ScrollAxis,
    /// How a scroll view shows its scroll bars (#132).
    pub scrollbar: crate::layout::Scrollbar,
    // Typography (text, buttons, inputs; not inherited, like the other text
    // styles).
    /// Font family (the system font by default).
    pub font_family: FontFamily,
    /// Italic (the font's italic face, or a slanted one).
    pub italic: bool,
    /// Line height: points (`line_height(20.)`), or a multiple of the font
    /// size (`line_height(relative(1.4))`, `leading(1.4)`); `Auto`: the
    /// font's own. Extra space is split above and below each line, like CSS.
    pub line_height: Length,
    /// Extra space after each character, points (negative tightens).
    pub letter_spacing: f32,
    /// Rendered case (upper, lower, capitalized).
    pub text_transform: TextTransform,
    /// Underlined.
    pub underline: bool,
    /// Struck through.
    pub strikethrough: bool,
    /// Digits of equal width (`font-variant-numeric: tabular-nums`):
    /// counters, clocks, columns of figures.
    pub tabular_numbers: bool,
    /// At most this many lines, the last cut with an ellipsis (`Some(1)`:
    /// one line, see [`Styled::truncate`]). `None`: as many as needed.
    pub max_lines: Option<u32>,
    /// Never wraps: one line per paragraph, cut at the node's edge
    /// (`white-space: pre`, clipped).
    pub no_wrap: bool,
    /// How wrapping lines are broken: [`TextWrap::Pretty`] or
    /// [`TextWrap::Balance`] (CSS `text-wrap`).
    pub text_wrap: TextWrap,

    // Box decorations (shadows, per-side and dashed borders, clipping,
    // gradients, backdrop blur).
    /// Drop and inner shadows, the first drawn on top (like CSS `box-shadow`).
    pub shadows: Vec<crate::decoration::Shadow>,
    /// Borders of single sides, replacing `border_width` / `border_color` on
    /// those sides (see [`Style::borders`]).
    pub border_sides: crate::decoration::BorderSides,
    /// Solid (the default), dashed or dotted borders.
    pub border_style: crate::decoration::BorderStyle,
    /// Children are clipped to the box, rounded corners included (CSS
    /// `overflow: hidden`).
    pub clip: bool,
    /// Background gradient, drawn over `background`.
    pub gradient: Option<crate::decoration::Gradient>,
    /// Blurs what is behind the node by this radius, points (`0`: none): a
    /// translucent bar over scrolling content. Give it a translucent
    /// `background` to tint it.
    pub backdrop_blur: f32,

    // Interaction states.
    /// Refinements while hovered, pressed, focused… (`.hover(|s| …)`).
    pub states: crate::interaction::States,
    /// The pointer's shape over the node (`None`: the platform's default,
    /// a pointing hand on clickable nodes).
    pub cursor: Option<crate::decoration::Cursor>,
    /// Descendants' `group_*` refinements follow this node's states.
    pub group: bool,
    /// A named group (#212): descendants' `group_*_named` refinements with
    /// this name follow this node's states (and only theirs).
    pub group_name: Option<std::rc::Rc<str>>,
    /// Paint order among siblings (#232): higher is drawn above (and hit
    /// first); 0 keeps the tree order. See [`Styled::z_index`].
    pub z_index: i32,

    // Motion: offsets relative to the element's own size.
    /// Extra offset in percent of the element's own width and height (CSS
    /// `translate(x%, y%)`; `(-50., 0.)` centers it on its position). Leading
    /// / trailing like `transform.x`; no effect on the layout around it.
    pub translate_percent: (f32, f32),

    // --- Hit testing (#19) ---
    /// The element and everything in it take pointer input (`true` by
    /// default). `false`: clicks, taps and hovers go to what is underneath
    /// (`pointer-events: none`).
    pub hit_test: bool,
    /// The element and everything in it are out of reach: no pointer input,
    /// no keyboard focus, hidden from assistive technologies (`inert`).
    /// Still drawn.
    pub inert: bool,

    // --- Padding on every node, borderless fields (#14) ---
    /// `.border(0.)` was called: no border at all, not even the platform's
    /// default one around text inputs, text areas, pickers and date pickers.
    pub border_none: bool,
    /// A padding setter (`.p()`, `.px()`, `.pt()`…) was called, even with
    /// zero: an explicit `.p(0.)` replaces a button's, text input's, text
    /// area's or picker's own inset with none (#69). See
    /// [`Style::has_padding`].
    pub padding_set: bool,

    // --- Bar tint (#30) ---
    /// The tint of a tab bar (its selected tab) or a stack's header bar
    /// (its buttons, Back included): `Tabs::tint`, `Stack::tint`, set by
    /// the router from `RouterBuilder::tint` or the app's accent color.
    /// `None`: the platform's.
    pub tint: Option<Color>,

    // --- Custom window chrome (#67) ---
    /// Dragging the element moves the window and double-clicking it zooms
    /// (or minimizes) it, like a title bar: see [`Styled::window_drag`].
    pub window_drag: bool,
}

impl Style {
    /// Whether descendants' group refinements follow this node: a
    /// [`group`](Styled::group) or a [`named one`](Styled::group_named).
    pub fn is_group(&self) -> bool {
        self.group || self.group_name.is_some()
    }
}

impl Style {
    /// Line height in points for this style's font size, if set.
    pub fn line_height_points(&self) -> Option<f32> {
        match self.line_height {
            Length::Auto => None,
            Length::Px(v) => Some(v),
            Length::Fraction(f) => Some(f * self.font_size),
        }
    }

    /// Whether the text is cut to fit its box (a line clamp, or no
    /// wrapping): the box can then be narrower than the text's words.
    pub fn truncates(&self) -> bool {
        self.max_lines.is_some() || self.no_wrap
    }

    /// Whether the app gave this node a padding, an explicit zero included
    /// (`.p(0.)`): a button, text input, text area or picker then uses it
    /// in place of its own inset, and sizes to its content plus it (#69).
    pub fn has_padding(&self) -> bool {
        self.padding_set || !self.padding.is_zero()
    }

    /// Whether a text field draws its own look, so it gets no platform
    /// focus ring (#52, #274): a padding (an explicit zero included), a
    /// background, a border, `.border(0.)`, or a `focus` / `focus_within`
    /// refinement. The caret, or the app's own focus style, shows the
    /// focus; an unstyled field keeps the platform's ring.
    pub fn own_field_look(&self) -> bool {
        use crate::interaction::Interaction;
        self.has_padding()
            || self.background.is_some()
            || self.borders().iter().any(|b| b.width > 0.)
            || self.border_none
            || self
                .states
                .resolved()
                .any(|(i, _)| matches!(i, Interaction::Focus | Interaction::FocusWithin))
    }
}

/// Window size classes, by width in points (CSS pixels on the web).
///
/// | | Width | Typically |
/// |---|---|---|
/// | `Compact` | < 600 | phones |
/// | `Medium` | 600 – 1023 | tablets portrait, small windows |
/// | `Expanded` | 1024 – 1439 | tablets landscape, laptops |
/// | `Large` | ≥ 1440 | desktops |
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Breakpoint {
    /// Narrower than 600 (the default).
    #[default]
    Compact,
    /// 600 to 1023 wide.
    Medium,
    /// 1024 to 1439 wide.
    Expanded,
    /// 1440 and wider.
    Large,
}

impl Breakpoint {
    /// The default smallest widths of `Medium`, `Expanded` and `Large`.
    pub const DEFAULT_WIDTHS: [f32; 3] = [600., 1024., 1440.];

    /// The class of a window `width` points wide, with the smallest widths
    /// of `Medium`, `Expanded` and `Large` in `widths`.
    pub fn for_width_in(width: f32, widths: [f32; 3]) -> Self {
        match width {
            w if w >= widths[2] => Breakpoint::Large,
            w if w >= widths[1] => Breakpoint::Expanded,
            w if w >= widths[0] => Breakpoint::Medium,
            _ => Breakpoint::Compact,
        }
    }

    /// The class of a window `width` points wide (default breakpoints).
    pub fn for_width(width: f32) -> Self {
        match width {
            w if w >= 1440. => Breakpoint::Large,
            w if w >= 1024. => Breakpoint::Expanded,
            w if w >= 600. => Breakpoint::Medium,
            _ => Breakpoint::Compact,
        }
    }

    /// The smallest width of this class.
    pub fn min_width(self) -> f32 {
        match self {
            Breakpoint::Compact => 0.,
            Breakpoint::Medium => 600.,
            Breakpoint::Expanded => 1024.,
            Breakpoint::Large => 1440.,
        }
    }
}

type Rule = std::rc::Rc<dyn Fn(Style) -> Style>;

/// When a rule applies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Condition {
    /// The window is at least this wide.
    From(Breakpoint),
    /// The app runs on one of these platforms.
    On(crate::platform::Platforms),
    /// The screen is in this orientation.
    Orientation(crate::device_info::Orientation),
    /// A foldable is in this posture.
    Posture(crate::device_info::Posture),
    /// The device is of this kind.
    Kind(crate::device_info::DeviceKind),
    /// The window is at least this many points wide (`.at_width(768., …)`).
    Width(f32),
    /// The element itself is at least this many points wide
    /// (`.when_width_at_least(480., …)`): resolved by the backend from the
    /// element's laid-out width, not by the app.
    OwnWidth(f32),
}

/// What responsive rules are resolved against.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct StyleContext {
    /// The window's size class.
    pub breakpoint: Breakpoint,
    /// The platform the app runs on (`None`: platform rules never apply).
    pub platform: Option<crate::platform::Platform>,
    /// The screen's orientation.
    pub orientation: crate::device_info::Orientation,
    /// A foldable's posture.
    pub posture: crate::device_info::Posture,
    /// The kind of device.
    pub kind: crate::device_info::DeviceKind,
    /// The window's width, points (for [`Condition::Width`]).
    pub width: f32,
}

impl StyleContext {
    /// A context for `breakpoint` on `platform`; device fields at their defaults.
    pub fn new(breakpoint: Breakpoint, platform: crate::platform::Platform) -> Self {
        StyleContext {
            breakpoint,
            platform: Some(platform),
            ..Default::default()
        }
    }
}

/// Style changes by breakpoint or platform; resolved for the current window
/// and platform when the tree is rendered.
#[derive(Clone, Default)]
pub struct Responsive(Vec<(Condition, Rule)>);

impl Responsive {
    /// Whether no rule is set.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Responsive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|(b, _)| b))
            .finish()
    }
}

/// Rules are closures: two styles with rules are never equal (resolved
/// styles, which have none, compare normally).
impl PartialEq for Responsive {
    fn eq(&self, other: &Self) -> bool {
        self.0.is_empty() && other.0.is_empty()
    }
}

impl Style {
    /// This style in `cx`: device rules (platform, kind, orientation,
    /// posture) first, in the order they were written, then width rules from
    /// the smallest breakpoint up.
    pub fn resolve(&self, cx: StyleContext) -> Style {
        if self.responsive.is_empty() {
            let mut s = self.clone();
            s.resolve_states(cx);
            return s;
        }
        let mut rules = self.responsive.0.clone();
        rules.sort_by_key(|(c, _)| match c {
            Condition::From(b) => Some((b.min_width() * 64.) as i64),
            Condition::Width(w) => Some((w * 64.) as i64),
            _ => None,
        });
        let mut s = self.clone();
        s.responsive = Responsive::default();
        // Rules on the element's own width wait for the layout.
        let mut own_width = Vec::new();
        for (condition, rule) in rules {
            let applies = match condition {
                Condition::From(b) => b <= cx.breakpoint,
                Condition::On(set) => cx.platform.is_some_and(|p| set.contains(p)),
                Condition::Orientation(o) => o == cx.orientation,
                Condition::Posture(p) => p == cx.posture,
                Condition::Kind(k) => k == cx.kind,
                Condition::Width(w) => cx.width >= w,
                Condition::OwnWidth(_) => {
                    own_width.push((condition, rule.clone()));
                    false
                }
            };
            if applies {
                s = rule(s);
            }
        }
        s.responsive = Responsive(own_width);
        s.resolve_states(cx);
        s
    }

    /// The widths of the element's own width rules
    /// (`.when_width_at_least(…)`), smallest first: the backend resolves them
    /// with [`resolve_own_width`](Self::resolve_own_width) once it knows the
    /// element's width.
    pub fn own_width_thresholds(&self) -> Vec<f32> {
        let mut t: Vec<f32> = self
            .responsive
            .0
            .iter()
            .filter_map(|(c, _)| match c {
                Condition::OwnWidth(w) => Some(*w),
                _ => None,
            })
            .collect();
        t.sort_by(f32::total_cmp);
        t.dedup();
        t
    }

    /// The window widths this style's `.at_width(…)` rules switch at.
    pub fn window_width_thresholds(&self) -> impl Iterator<Item = f32> + '_ {
        self.responsive.0.iter().filter_map(|(c, _)| match c {
            Condition::Width(w) => Some(*w),
            _ => None,
        })
    }

    /// This (resolved) style for an element `width` points wide: its own
    /// width rules applied, smallest first. Other rules are dropped (resolve
    /// them first with [`resolve`](Self::resolve)).
    pub fn resolve_own_width(&self, width: f32) -> Style {
        if self.responsive.is_empty() {
            return self.clone();
        }
        let mut rules: Vec<(f32, Rule)> = self
            .responsive
            .0
            .iter()
            .filter_map(|(c, r)| match c {
                Condition::OwnWidth(w) => Some((*w, r.clone())),
                _ => None,
            })
            .collect();
        rules.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut s = self.clone();
        s.responsive = Responsive::default();
        for (w, rule) in rules {
            if width >= w {
                s = rule(s);
            }
        }
        s
    }

    /// Every length multiplied by `factor` (points, text, radii, borders;
    /// fractions unchanged): native backends zoom the whole UI with it, e.g.
    /// to TV size.
    pub fn scaled(&self, factor: f32) -> Style {
        if factor == 1. {
            return self.clone();
        }
        let len = |l: Length| match l {
            Length::Px(v) => Length::Px(v * factor),
            other => other,
        };
        let edges = |e: Edges| Edges {
            top: e.top * factor,
            right: e.right * factor,
            bottom: e.bottom * factor,
            left: e.left * factor,
        };
        Style {
            gap: self.gap * factor,
            padding: edges(self.padding),
            margin: edges(self.margin),
            inset: Inset {
                top: len(self.inset.top),
                right: len(self.inset.right),
                bottom: len(self.inset.bottom),
                left: len(self.inset.left),
            },
            width: len(self.width),
            height: len(self.height),
            min_width: len(self.min_width),
            min_height: len(self.min_height),
            max_width: len(self.max_width),
            max_height: len(self.max_height),
            flex_basis: len(self.flex_basis),
            corner_radius: self.corner_radius * factor,
            border_width: self.border_width * factor,
            font_size: (self.font_size * factor).round(),
            transform: crate::animation::Transform {
                x: self.transform.x * factor,
                y: self.transform.y * factor,
                ..self.transform
            },
            animations: self.animations.scaled(factor, false),
            gap_x: self.gap_x.map(|g| g * factor),
            gap_y: self.gap_y.map(|g| g * factor),
            grid_columns: self.grid_columns.iter().map(|t| t.scaled(factor)).collect(),
            grid_rows: self.grid_rows.iter().map(|t| t.scaled(factor)).collect(),
            grid_auto_rows: self.grid_auto_rows.as_ref().map(|t| t.scaled(factor)),
            line_height: len(self.line_height),
            letter_spacing: self.letter_spacing * factor,
            shadows: self.shadows.iter().map(|sh| sh.scaled(factor)).collect(),
            border_sides: {
                let side = |b: Option<crate::decoration::BorderSide>| {
                    b.map(|b| crate::decoration::BorderSide {
                        width: b.width * factor,
                        ..b
                    })
                };
                let b = self.border_sides;
                crate::decoration::BorderSides {
                    top: side(b.top),
                    right: side(b.right),
                    bottom: side(b.bottom),
                    left: side(b.left),
                }
            },
            backdrop_blur: self.backdrop_blur * factor,
            states: self.states.map_resolved(|s| s.scaled(factor)),
            ..self.clone()
        }
    }

    /// Whether the element exists on `platform`.
    pub fn exists_on(&self, platform: crate::platform::Platform) -> bool {
        self.platforms.is_none_or(|set| set.contains(platform))
    }
}

/// Lets responsive rules use the usual builders: `.md(|s| s.flex_row().gap(24.))`.
impl Styled for Style {
    fn style_mut(&mut self) -> &mut Style {
        self
    }
}

/// How style and layout changes animate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transition {
    /// Seconds.
    pub duration: f32,
    /// The timing curve.
    pub curve: Curve,
    /// Seconds before it starts.
    pub delay: f32,
}

impl Transition {
    fn new(ms: u32, curve: Curve) -> Self {
        Transition {
            duration: ms as f32 / 1000.,
            curve,
            delay: 0.,
        }
    }
    /// Eases in and out, over `ms` milliseconds.
    pub fn ease(ms: u32) -> Self {
        Self::new(ms, Curve::EaseInOut)
    }
    /// Starts fast, settles slowly (things arriving).
    pub fn ease_out(ms: u32) -> Self {
        Self::new(ms, Curve::EaseOut)
    }
    /// Starts slowly, leaves fast (things leaving).
    pub fn ease_in(ms: u32) -> Self {
        Self::new(ms, Curve::EaseIn)
    }
    /// Constant speed, over `ms` milliseconds.
    pub fn linear(ms: u32) -> Self {
        Self::new(ms, Curve::Linear)
    }
    /// A spring with a slight overshoot, over `ms` milliseconds.
    pub fn spring(ms: u32) -> Self {
        Self::new(ms, Curve::Spring)
    }
    /// Waits `ms` milliseconds first.
    pub fn delay(mut self, ms: u32) -> Self {
        self.delay = ms as f32 / 1000.;
        self
    }

    /// CSS `<easing-function>`.
    pub fn css_easing(&self) -> &'static str {
        match self.curve {
            Curve::EaseInOut => "ease-in-out",
            Curve::EaseOut => "cubic-bezier(.2,.8,.2,1)",
            Curve::EaseIn => "cubic-bezier(.4,0,1,1)",
            Curve::Linear => "linear",
            Curve::Spring => "cubic-bezier(.34,1.56,.64,1)",
        }
    }

    /// The curve as cubic Bézier control points `(x1, y1, x2, y2)`, the same
    /// as [`css_easing`](Self::css_easing) (springs overshoot slightly).
    pub fn bezier(&self) -> (f32, f32, f32, f32) {
        match self.curve {
            Curve::EaseInOut => (0.42, 0., 0.58, 1.),
            Curve::EaseOut => (0.2, 0.8, 0.2, 1.),
            Curve::EaseIn => (0.4, 0., 1., 1.),
            Curve::Linear => (0., 0., 1., 1.),
            Curve::Spring => (0.34, 1.56, 0.64, 1.),
        }
    }

    /// The eased progress at linear progress `p` (0 to 1), for backends that
    /// animate frame by frame. Springs overshoot past 1 a little.
    pub fn ease_at(&self, p: f32) -> f32 {
        let p = p.clamp(0., 1.);
        if p == 0. || p == 1. {
            return p;
        }
        let (x1, y1, x2, y2) = self.bezier();
        let at = |a: f32, b: f32, t: f32| {
            let u = 1. - t;
            3. * u * u * t * a + 3. * u * t * t * b + t * t * t
        };
        // The curve's parameter where x = p (x is increasing: bisection).
        let (mut lo, mut hi) = (0f32, 1f32);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.;
            if at(x1, x2, mid) < p {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        at(y1, y2, (lo + hi) / 2.)
    }
}

/// The timing curve of a [`Transition`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Curve {
    /// Slow start and end (the default).
    #[default]
    EaseInOut,
    /// Fast start, slow end.
    EaseOut,
    /// Slow start, fast end.
    EaseIn,
    /// Constant speed.
    Linear,
    /// Slight overshoot, like iOS springs.
    Spring,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            display_none: false,
            direction: FlexDirection::Column,
            wrap: false,
            align_items: Align::Stretch,
            align_self: None,
            justify_content: Justify::Start,
            gap: 0.,
            padding: Edges::default(),
            margin: Edges::default(),
            margin_auto: [false; 4],
            position: Position::Relative,
            inset: Inset::default(),
            width: Length::Auto,
            height: Length::Auto,
            min_width: Length::Auto,
            min_height: Length::Auto,
            max_width: Length::Auto,
            max_height: Length::Auto,
            flex_grow: 0.,
            flex_shrink: 1.,
            flex_basis: Length::Auto,
            order: 0,
            background: None,
            corner_radius: 0.,
            rounded_corners: Corners::ALL,
            border_width: 0.,
            border_color: None,
            opacity: 1.,
            text_color: None,
            font_size: DEFAULT_FONT_SIZE,
            font_weight: FontWeight::Regular,
            text_align: TextAlign::Left,
            transform: crate::animation::Transform::IDENTITY,
            transition: None,
            animations: crate::animation::Animations::default(),
            responsive: Responsive::default(),
            platforms: None,

            grid: false,
            grid_columns: Vec::new(),
            grid_rows: Vec::new(),
            grid_auto_rows: None,
            col_start: None,
            col_span: 1,
            row_start: None,
            row_span: 1,
            gap_x: None,
            gap_y: None,
            aspect_ratio: None,
            scroll_axis: crate::layout::ScrollAxis::Vertical,
            scrollbar: crate::layout::Scrollbar::Auto,
            font_family: FontFamily::System,
            italic: false,
            line_height: Length::Auto,
            letter_spacing: 0.,
            text_transform: TextTransform::None,
            underline: false,
            strikethrough: false,
            tabular_numbers: false,
            max_lines: None,
            no_wrap: false,
            text_wrap: TextWrap::Auto,
            shadows: Vec::new(),
            border_sides: crate::decoration::BorderSides::default(),
            border_style: crate::decoration::BorderStyle::Solid,
            clip: false,
            gradient: None,
            backdrop_blur: 0.,
            states: crate::interaction::States::default(),
            cursor: None,
            group: false,
            group_name: None,
            z_index: 0,
            translate_percent: (0., 0.),
            hit_test: true,
            inert: false,

            border_none: false,
            padding_set: false,

            tint: None,

            window_drag: false,
        }
    }
}

/// GPUI-style fluent styling methods, available on every element builder.
pub trait Styled: Sized {
    /// The style the builders change.
    fn style_mut(&mut self) -> &mut Style;

    /// Changes the style with `f`.
    fn with_style(mut self, f: impl FnOnce(&mut Style)) -> Self {
        f(self.style_mut());
        self
    }

    // Flex container
    /// Lays children out in a row.
    fn flex_row(self) -> Self {
        self.with_style(|s| s.direction = FlexDirection::Row)
    }
    /// Lays children out in a column (the default).
    fn flex_col(self) -> Self {
        self.with_style(|s| s.direction = FlexDirection::Column)
    }
    /// Lays children out in a row from trailing to leading
    /// (`flex-direction: row-reverse`): the first child ends up at the
    /// trailing edge. Usable in refinements (`.md(|s| s.flex_row_reverse())`).
    fn flex_row_reverse(self) -> Self {
        self.with_style(|s| s.direction = FlexDirection::RowReverse)
    }
    /// Lays children out in a column from bottom to top
    /// (`flex-direction: column-reverse`).
    fn flex_col_reverse(self) -> Self {
        self.with_style(|s| s.direction = FlexDirection::ColumnReverse)
    }
    /// Wraps children onto new lines when they don't fit.
    fn flex_wrap(self) -> Self {
        self.with_style(|s| s.wrap = true)
    }
    /// Space between children, points.
    fn gap(self, v: f32) -> Self {
        self.with_style(|s| s.gap = v)
    }
    /// Aligns children to the start of the cross axis.
    fn items_start(self) -> Self {
        self.with_style(|s| s.align_items = Align::Start)
    }
    /// Centers children on the cross axis.
    fn items_center(self) -> Self {
        self.with_style(|s| s.align_items = Align::Center)
    }
    /// Aligns children to the end of the cross axis.
    fn items_end(self) -> Self {
        self.with_style(|s| s.align_items = Align::End)
    }
    /// Stretches children across the cross axis (the default).
    fn items_stretch(self) -> Self {
        self.with_style(|s| s.align_items = Align::Stretch)
    }
    /// Aligns this node to the start of its parent's cross axis.
    fn self_start(self) -> Self {
        self.with_style(|s| s.align_self = Some(Align::Start))
    }
    /// Centers this node on its parent's cross axis.
    fn self_center(self) -> Self {
        self.with_style(|s| s.align_self = Some(Align::Center))
    }
    /// Aligns this node to the end of its parent's cross axis.
    fn self_end(self) -> Self {
        self.with_style(|s| s.align_self = Some(Align::End))
    }
    /// Packs children at the start of the main axis (the default).
    fn justify_start(self) -> Self {
        self.with_style(|s| s.justify_content = Justify::Start)
    }
    /// Packs children in the center of the main axis.
    fn justify_center(self) -> Self {
        self.with_style(|s| s.justify_content = Justify::Center)
    }
    /// Packs children at the end of the main axis.
    fn justify_end(self) -> Self {
        self.with_style(|s| s.justify_content = Justify::End)
    }
    /// Equal space between children, none at the edges.
    fn justify_between(self) -> Self {
        self.with_style(|s| s.justify_content = Justify::SpaceBetween)
    }
    /// Equal space around each child.
    fn justify_around(self) -> Self {
        self.with_style(|s| s.justify_content = Justify::SpaceAround)
    }
    /// Equal space between children and at the edges.
    fn justify_evenly(self) -> Self {
        self.with_style(|s| s.justify_content = Justify::SpaceEvenly)
    }
    /// On a grid, `auto` columns grow to fill its width (CSS
    /// `justify-content: stretch`, #284), on the web and natively alike.
    /// Flex containers pack their children at the start.
    fn justify_stretch(self) -> Self {
        self.with_style(|s| s.justify_content = Justify::Stretch)
    }

    // Flex item
    /// Grows and shrinks from a zero basis: siblings with `flex_1` share the space equally.
    fn flex_1(self) -> Self {
        self.with_style(|s| {
            s.flex_grow = 1.;
            s.flex_shrink = 1.;
            s.flex_basis = Length::Px(0.)
        })
    }
    /// Grows into the free space, `weight` times as much as a sibling
    /// with `flex_grow(1.)` (`flex-grow`).
    fn flex_grow(self, weight: f32) -> Self {
        self.with_style(|s| s.flex_grow = weight.max(0.))
    }
    /// Shrinks when space is short, `weight` times as much as a sibling with
    /// `flex_shrink(1.)` (`flex-shrink`; `0`: never shrinks).
    fn flex_shrink(self, weight: f32) -> Self {
        self.with_style(|s| s.flex_shrink = weight.max(0.))
    }
    /// Never shrinks below its size.
    fn flex_shrink_0(self) -> Self {
        self.with_style(|s| s.flex_shrink = 0.)
    }
    /// Place among its siblings (CSS `order`, `0` by default): lower first,
    /// the children's order among equals. Moves the node visually only;
    /// focus order and assistive technologies follow the tree. Usable in
    /// refinements: `.order(1).md(|s| s.order(0))`.
    fn order(self, order: i32) -> Self {
        self.with_style(|s| s.order = order)
    }

    // Spacing
    /// Padding on every edge, points. Works on every kind of node: the box
    /// (background, border, rounding) grows by it and the content (a text,
    /// an input's text, a button's title, a control) is inset by it. On a
    /// button, text input, text area or picker it replaces the control's
    /// own inset.
    fn p(self, v: f32) -> Self {
        self.with_style(|s| {
            s.padding = Edges::all(v);
            s.padding_set = true
        })
    }
    /// Leading and trailing padding, points.
    fn px(self, v: f32) -> Self {
        self.with_style(|s| {
            s.padding.left = v;
            s.padding.right = v;
            s.padding_set = true
        })
    }
    /// Top and bottom padding, points.
    fn py(self, v: f32) -> Self {
        self.with_style(|s| {
            s.padding.top = v;
            s.padding.bottom = v;
            s.padding_set = true
        })
    }
    /// Top padding, points.
    fn pt(self, v: f32) -> Self {
        self.with_style(|s| {
            s.padding.top = v;
            s.padding_set = true
        })
    }
    /// Trailing padding (right in left-to-right languages), points.
    fn pr(self, v: f32) -> Self {
        self.with_style(|s| {
            s.padding.right = v;
            s.padding_set = true
        })
    }
    /// Bottom padding, points.
    fn pb(self, v: f32) -> Self {
        self.with_style(|s| {
            s.padding.bottom = v;
            s.padding_set = true
        })
    }
    /// Leading padding (left in left-to-right languages), points.
    fn pl(self, v: f32) -> Self {
        self.with_style(|s| {
            s.padding.left = v;
            s.padding_set = true
        })
    }
    /// Margin on every edge, points.
    fn m(self, v: f32) -> Self {
        self.with_style(|s| {
            s.margin = Edges::all(v);
            s.margin_auto = [false; 4]
        })
    }
    /// Leading and trailing margin, points.
    fn mx(self, v: f32) -> Self {
        self.with_style(|s| {
            s.margin.left = v;
            s.margin.right = v;
            s.margin_auto[1] = false;
            s.margin_auto[3] = false
        })
    }
    /// Top and bottom margin, points.
    fn my(self, v: f32) -> Self {
        self.with_style(|s| {
            s.margin.top = v;
            s.margin.bottom = v;
            s.margin_auto[0] = false;
            s.margin_auto[2] = false
        })
    }
    /// Top margin, points.
    fn mt(self, v: f32) -> Self {
        self.with_style(|s| {
            s.margin.top = v;
            s.margin_auto[0] = false
        })
    }
    /// Leading margin (flips in right-to-left languages).
    fn ml(self, v: f32) -> Self {
        self.with_style(|s| {
            s.margin.left = v;
            s.margin_auto[3] = false
        })
    }
    /// Trailing margin.
    fn mr(self, v: f32) -> Self {
        self.with_style(|s| {
            s.margin.right = v;
            s.margin_auto[1] = false
        })
    }
    /// Bottom margin, points.
    fn mb(self, v: f32) -> Self {
        self.with_style(|s| {
            s.margin.bottom = v;
            s.margin_auto[2] = false
        })
    }
    /// `auto` margins on every side (#228): centered in the parent, both
    /// ways, like CSS `margin: auto`.
    fn m_auto(self) -> Self {
        self.with_style(|s| s.margin_auto = [true; 4])
    }
    /// `auto` leading and trailing margins (#228): a column centered in
    /// its parent (`w_full().max_w(px(1152.)).mx_auto()`), without making
    /// the parent `items_center()`.
    fn mx_auto(self) -> Self {
        self.with_style(|s| {
            s.margin_auto[1] = true;
            s.margin_auto[3] = true
        })
    }
    /// `auto` top and bottom margins: centered vertically.
    fn my_auto(self) -> Self {
        self.with_style(|s| {
            s.margin_auto[0] = true;
            s.margin_auto[2] = true
        })
    }
    /// An `auto` top margin: pushed to the bottom of a column (a footer).
    fn mt_auto(self) -> Self {
        self.with_style(|s| s.margin_auto[0] = true)
    }
    /// An `auto` leading margin: pushed to the end of a row (the last
    /// button of a toolbar), with no spacer. Flips in right-to-left
    /// languages, like `ml`.
    fn ml_auto(self) -> Self {
        self.with_style(|s| s.margin_auto[3] = true)
    }
    /// An `auto` trailing margin: what follows is pushed to the end.
    fn mr_auto(self) -> Self {
        self.with_style(|s| s.margin_auto[1] = true)
    }
    /// An `auto` bottom margin: what follows is pushed to the bottom.
    fn mb_auto(self) -> Self {
        self.with_style(|s| s.margin_auto[2] = true)
    }

    // Position
    /// Takes the node out of the flex flow: it is placed at its insets from
    /// the parent (`.top()`, `.right()`, `.bottom()`, `.left()`) and drawn
    /// over earlier siblings.
    fn absolute(self) -> Self {
        self.with_style(|s| s.position = Position::Absolute)
    }
    /// Distance from the top edge.
    fn top(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.inset.top = v)
    }
    /// Distance from the bottom edge.
    fn bottom(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.inset.bottom = v)
    }
    /// Distance from the leading edge (the right one, right to left).
    fn left(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.inset.left = v)
    }
    /// Distance from the trailing edge.
    fn right(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.inset.right = v)
    }
    /// The same distance from every edge (`.absolute().inset(0.)` covers the parent).
    fn inset(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| {
            s.inset = Inset {
                top: v,
                right: v,
                bottom: v,
                left: v,
            }
        })
    }

    // Size
    /// Width: points (`w(120.)`) or a fraction of the parent (`w(relative(0.5))`).
    fn w(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.width = v)
    }
    /// Height: points or a fraction of the parent.
    fn h(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.height = v)
    }
    /// Width and height.
    fn size(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| {
            s.width = v;
            s.height = v
        })
    }
    /// The parent's full width.
    fn w_full(self) -> Self {
        self.w(relative(1.))
    }
    /// The parent's full height.
    fn h_full(self) -> Self {
        self.h(relative(1.))
    }
    /// The parent's full width and height.
    fn size_full(self) -> Self {
        self.w_full().h_full()
    }
    /// Minimum width.
    fn min_w(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.min_width = v)
    }
    /// Minimum height.
    fn min_h(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.min_height = v)
    }
    /// Maximum width.
    fn max_w(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.max_width = v)
    }
    /// Maximum height.
    fn max_h(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.max_height = v)
    }

    // Visuals
    /// Background color.
    fn bg(self, c: Color) -> Self {
        self.with_style(|s| s.background = Some(c))
    }
    /// Corner radius, points.
    fn rounded(self, r: f32) -> Self {
        self.with_style(|s| {
            s.corner_radius = r;
            s.rounded_corners = Corners::ALL;
        })
    }
    /// Rounds only `corners` by `r` points; the others stay square.
    fn rounded_corners(self, r: f32, corners: Corners) -> Self {
        self.with_style(|s| {
            s.corner_radius = r;
            s.rounded_corners = corners;
        })
    }
    /// Rounds the top corners only.
    fn rounded_top(self, r: f32) -> Self {
        self.rounded_corners(r, Corners::TOP)
    }
    /// Rounds the bottom corners only.
    fn rounded_bottom(self, r: f32) -> Self {
        self.rounded_corners(r, Corners::BOTTOM)
    }
    /// Border width, points. `border(0.)` removes every border, the
    /// default one of text inputs, text areas, pickers and date pickers
    /// included (a borderless field).
    fn border(self, w: f32) -> Self {
        self.with_style(|s| {
            s.border_width = w;
            s.border_none = w <= 0.;
        })
    }
    /// Border color.
    fn border_color(self, c: Color) -> Self {
        self.with_style(|s| s.border_color = Some(c))
    }
    /// Opacity, `0.0..=1.0`.
    fn opacity(self, o: f32) -> Self {
        self.with_style(|s| s.opacity = o)
    }

    // Text
    /// Text color.
    fn text_color(self, c: Color) -> Self {
        self.with_style(|s| s.text_color = Some(c))
    }
    /// Text size, points.
    fn text_size(self, v: f32) -> Self {
        self.with_style(|s| s.font_size = v)
    }
    /// Text weight.
    fn font_weight(self, w: FontWeight) -> Self {
        self.with_style(|s| s.font_weight = w)
    }
    /// Medium text weight (500).
    fn medium(self) -> Self {
        self.font_weight(FontWeight::Medium)
    }
    /// Semibold text weight (600).
    fn semibold(self) -> Self {
        self.font_weight(FontWeight::Semibold)
    }
    /// Bold text weight (700).
    fn bold(self) -> Self {
        self.font_weight(FontWeight::Bold)
    }
    /// Centers the text.
    fn text_center(self) -> Self {
        self.with_style(|s| s.text_align = TextAlign::Center)
    }
    /// Aligns the text to the trailing edge (left in right-to-left languages).
    fn text_right(self) -> Self {
        self.with_style(|s| s.text_align = TextAlign::Right)
    }

    // Responsive: applied when the window is at least this wide.
    /// From `breakpoint` up.
    fn at(self, breakpoint: Breakpoint, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.with_style(|s| {
            s.responsive
                .0
                .push((Condition::From(breakpoint), std::rc::Rc::new(f)))
        })
    }

    // Platforms
    /// Only on these platforms (`.only(Platform::Ios)`, `.only(Platform::MOBILE)`).
    fn only(self, platforms: impl Into<crate::platform::Platforms>) -> Self {
        let set = platforms.into();
        self.with_style(|s| s.platforms = Some(set))
    }
    /// Everywhere but these platforms.
    fn except(self, platforms: impl Into<crate::platform::Platforms>) -> Self {
        let set = platforms.into().complement();
        self.with_style(|s| s.platforms = Some(set))
    }
    /// Style changes on these platforms (`.on(Platform::Web, |s| s.p(24.))`).
    fn on(
        self,
        platforms: impl Into<crate::platform::Platforms>,
        f: impl Fn(Style) -> Style + 'static,
    ) -> Self {
        let set = platforms.into();
        self.with_style(|s| {
            s.responsive
                .0
                .push((Condition::On(set), std::rc::Rc::new(f)))
        })
    }

    // Device
    /// Style changes while the screen is landscape.
    fn landscape(self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.style_when(
            Condition::Orientation(crate::device_info::Orientation::Landscape),
            f,
        )
    }
    /// Style changes while the screen is portrait.
    fn portrait(self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.style_when(
            Condition::Orientation(crate::device_info::Orientation::Portrait),
            f,
        )
    }
    /// Style changes while a foldable is in `posture`
    /// (`.posture(Posture::Tabletop, |s| s.justify_end())`).
    fn posture(
        self,
        posture: crate::device_info::Posture,
        f: impl Fn(Style) -> Style + 'static,
    ) -> Self {
        self.style_when(Condition::Posture(posture), f)
    }
    /// Style changes on this kind of device
    /// (`.on_device(DeviceKind::Tablet, |s| s.p(32.))`).
    fn on_device(
        self,
        kind: crate::device_info::DeviceKind,
        f: impl Fn(Style) -> Style + 'static,
    ) -> Self {
        self.style_when(Condition::Kind(kind), f)
    }
    /// Style changes when `condition` holds.
    fn style_when(self, condition: Condition, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.with_style(|s| s.responsive.0.push((condition, std::rc::Rc::new(f))))
    }

    /// Tablets and up (≥ 600).
    fn md(self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.at(Breakpoint::Medium, f)
    }
    /// Laptops and up (≥ 1024).
    fn lg(self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.at(Breakpoint::Expanded, f)
    }
    /// Desktops (≥ 1440).
    fn xl(self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.at(Breakpoint::Large, f)
    }
    /// Hidden from `breakpoint` up (`hidden_from(Medium)`: phones only).
    fn hidden_from(self, breakpoint: Breakpoint) -> Self {
        self.at(breakpoint, |s| s.with_style(|s| s.display_none = true))
    }
    /// Hidden below `breakpoint` (`visible_from(Expanded)`: large screens only).
    fn visible_from(self, breakpoint: Breakpoint) -> Self {
        self.with_style(|s| s.display_none = true)
            .at(breakpoint, |s| s.with_style(|s| s.display_none = false))
    }

    // Motion (see the `animation` module)
    /// Animates changes of this element over `ms` milliseconds.
    fn animate(self, ms: u32) -> Self {
        self.transition(Transition::ease(ms))
    }
    /// Animates changes with a slightly bouncy spring.
    fn spring(self, ms: u32) -> Self {
        self.transition(Transition::spring(ms))
    }
    /// Animates changes of this element with `t` (keeps an earlier `delay`).
    fn transition(self, t: Transition) -> Self {
        self.with_style(|s| {
            let delay = s.transition.map_or(0., |t| t.delay);
            s.transition = Some(Transition {
                delay: if t.delay > 0. { t.delay } else { delay },
                ..t
            })
        })
    }
    /// Waits `ms` before animating (its transition, or a spring).
    fn delay(self, ms: u32) -> Self {
        self.with_style(|s| {
            s.transition = Some(
                s.transition
                    .unwrap_or(crate::animation::DEFAULT_ENTER)
                    .delay(ms),
            )
        })
    }
    /// The `index`-th of a list, `ms` after the previous one (cascading entrances).
    fn stagger(self, index: usize, ms: u32) -> Self {
        self.delay(index as u32 * ms)
    }
    /// Moves it `x`, `y` points without changing the layout.
    fn offset(self, x: f32, y: f32) -> Self {
        self.with_style(|s| {
            s.transform.x = x;
            s.transform.y = y;
        })
    }
    /// Scales it around its center without changing the layout.
    fn scale(self, scale: f32) -> Self {
        self.with_style(|s| s.transform.scale = scale)
    }
    /// Rotates it clockwise around its center, in degrees.
    fn rotate(self, degrees: f32) -> Self {
        self.with_style(|s| s.transform.rotate = degrees)
    }
    /// Appears from `pose` (with its transition, or a spring).
    fn enter(self, pose: crate::animation::Pose) -> Self {
        self.with_style(|s| s.animations.enter = Some(pose))
    }
    /// Animates to `pose` when removed, then disappears.
    fn exit(self, pose: crate::animation::Pose) -> Self {
        self.with_style(|s| s.animations.exit = Some(pose))
    }
    /// Goes back and forth between its style and `pose`, forever (pulses,
    /// breathing, loading indicators).
    fn repeat(self, pose: crate::animation::Pose, t: Transition) -> Self {
        self.with_style(|s| s.animations.repeat = Some((pose, t)))
    }

    // Grid (see `crate::layout`)
    /// Lays the children out on a grid (`display: grid`); set its columns
    /// with [`grid_cols`](Self::grid_cols) or [`cols`](Self::cols). Cells of
    /// a row are as tall as the row (equal-height cards).
    fn grid(self) -> Self {
        self.with_style(|s| s.grid = true)
    }
    /// The grid's column tracks: `grid_cols([auto_fill(minmax(172., fr(1.)))])`,
    /// `grid_cols([px(240.).into(), fr(1.)])`.
    fn grid_cols(self, tracks: impl IntoIterator<Item = crate::layout::GridTrack>) -> Self {
        let tracks: Vec<_> = tracks.into_iter().collect();
        self.with_style(|s| s.grid_columns = tracks)
    }
    /// `n` equal columns (`grid-template-columns: repeat(n, minmax(0, 1fr))`).
    fn cols(self, n: u16) -> Self {
        use crate::layout::{fr, minmax, repeat};
        self.grid_cols([repeat(n, minmax(0f32, fr(1.)))])
    }
    /// The grid's row tracks (`grid-template-rows`).
    fn grid_rows(self, tracks: impl IntoIterator<Item = crate::layout::GridTrack>) -> Self {
        let tracks: Vec<_> = tracks.into_iter().collect();
        self.with_style(|s| s.grid_rows = tracks)
    }
    /// Size of the grid's other rows (`grid-auto-rows`): `fr(1.)` makes every
    /// row as tall as the tallest.
    fn grid_auto_rows(self, track: impl Into<crate::layout::GridTrack>) -> Self {
        let track = track.into();
        self.with_style(|s| s.grid_auto_rows = Some(track))
    }
    /// This grid item spans `n` columns.
    fn col_span(self, n: u16) -> Self {
        self.with_style(|s| s.col_span = n.max(1))
    }
    /// This grid item spans `n` rows.
    fn row_span(self, n: u16) -> Self {
        self.with_style(|s| s.row_span = n.max(1))
    }
    /// This grid item starts at column line `line` (1 is the first column;
    /// negative counts from the end).
    fn col_start(self, line: i16) -> Self {
        self.with_style(|s| s.col_start = Some(line))
    }
    /// This grid item starts at row line `line` (1 is the first row).
    fn row_start(self, line: i16) -> Self {
        self.with_style(|s| s.row_start = Some(line))
    }
    /// Space between columns, and between the items of a row, points.
    fn gap_x(self, v: f32) -> Self {
        self.with_style(|s| s.gap_x = Some(v))
    }
    /// Space between rows, and between the items of a column, points.
    fn gap_y(self, v: f32) -> Self {
        self.with_style(|s| s.gap_y = Some(v))
    }

    // Alignment, sizing, positioning
    /// Lines the children up on their first text baseline.
    fn items_baseline(self) -> Self {
        self.with_style(|s| s.align_items = Align::Baseline)
    }
    /// Lines this node up on its siblings' text baseline.
    fn self_baseline(self) -> Self {
        self.with_style(|s| s.align_self = Some(Align::Baseline))
    }
    /// Keeps width ÷ height at `ratio` (`16. / 9.`): the height follows the
    /// width (or the width the height, when only that is known).
    fn aspect_ratio(self, ratio: f32) -> Self {
        self.with_style(|s| s.aspect_ratio = (ratio > 0.).then_some(ratio))
    }
    /// Stays `.top(…)` points (0 by default) from the top of its scroll view
    /// while its parent is in view, then scrolls away with it.
    fn sticky(self) -> Self {
        self.with_style(|s| s.position = Position::Sticky)
    }
    /// Positioned at its insets (`.bottom(16.).right(16.)`) from the window
    /// (natively: the screen it is on, inside the safe area), not from its
    /// parent, and never scrolls.
    fn fixed(self) -> Self {
        self.with_style(|s| s.position = Position::Fixed)
    }

    // Width rules
    /// Style changes when the window is at least `width` points wide: like
    /// `.md(…)` with any threshold. The app re-renders when the window
    /// crosses it.
    fn at_width(self, width: f32, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.style_when(Condition::Width(width), f)
    }
    /// Style changes when this element itself is at least `width` points
    /// wide, whatever the window (a container query): a card that turns
    /// into a row in a wide column. The backend applies them after layout,
    /// from the element's laid-out width; they should change how the
    /// children are arranged (direction, gaps, sizes, visibility), not the
    /// element's own width, or they would flip back and forth.
    fn when_width_at_least(self, width: f32, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.style_when(Condition::OwnWidth(width), f)
    }

    // Typography
    /// Text weight as a CSS number, 100 to 900 (`.weight(650)`).
    fn weight(self, w: u16) -> Self {
        self.font_weight(FontWeight::from_css(w))
    }
    /// Font family: `FontFamily::Serif`, or a name (`.font_family("Inter")`).
    fn font_family(self, family: impl Into<FontFamily>) -> Self {
        let family = family.into();
        self.with_style(|s| s.font_family = family)
    }
    /// The platform's monospaced font (SF Mono, monospace, `ui-monospace`).
    fn font_mono(self) -> Self {
        self.font_family(FontFamily::Monospace)
    }
    /// The platform's serif font (New York on Apple platforms).
    fn font_serif(self) -> Self {
        self.font_family(FontFamily::Serif)
    }
    /// The platform's rounded font (SF Rounded on Apple platforms).
    fn font_rounded(self) -> Self {
        self.font_family(FontFamily::Rounded)
    }
    /// Italic text.
    fn italic(self) -> Self {
        self.with_style(|s| s.italic = true)
    }
    /// Line height: points, or a multiple of the font size with
    /// `relative(1.4)` (see [`leading`](Self::leading)).
    fn line_height(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.line_height = v)
    }
    /// Line height as a multiple of the font size (`leading(1.2)`).
    fn leading(self, multiple: f32) -> Self {
        self.line_height(Length::Fraction(multiple))
    }
    /// Extra space between letters, points (`letter_spacing(0.8)` for
    /// small capital titles).
    fn letter_spacing(self, points: f32) -> Self {
        self.with_style(|s| s.letter_spacing = points)
    }
    /// Shorthand for [`letter_spacing`](Self::letter_spacing).
    fn tracking(self, points: f32) -> Self {
        self.letter_spacing(points)
    }
    /// Rendered case of the text.
    fn text_transform(self, t: TextTransform) -> Self {
        self.with_style(|s| s.text_transform = t)
    }
    /// Renders the text in capitals (screen readers read it as written).
    fn uppercase(self) -> Self {
        self.text_transform(TextTransform::Uppercase)
    }
    /// Renders the text in lower case.
    fn lowercase(self) -> Self {
        self.text_transform(TextTransform::Lowercase)
    }
    /// Renders the first letter of each word in capitals (CSS
    /// `text-transform: capitalize`; screen readers read it as written).
    /// Text fields have their own `capitalize(Capitalize)`, the keyboard's
    /// automatic capitalization.
    fn capitalize(self) -> Self {
        self.text_transform(TextTransform::Capitalize)
    }
    /// Underlined text.
    fn underline(self) -> Self {
        self.with_style(|s| s.underline = true)
    }
    /// Struck-through text.
    fn strikethrough(self) -> Self {
        self.with_style(|s| s.strikethrough = true)
    }
    /// Shorthand for [`strikethrough`](Self::strikethrough) (CSS `line-through`).
    fn line_through(self) -> Self {
        self.strikethrough()
    }
    /// Digits of equal width, so changing figures don't move.
    fn tabular_nums(self) -> Self {
        self.with_style(|s| s.tabular_numbers = true)
    }
    /// One line, cut with an ellipsis when too long.
    fn truncate(self) -> Self {
        self.line_clamp(1)
    }
    /// At most `lines` lines, the last cut with an ellipsis (`0`: no limit).
    fn line_clamp(self, lines: u32) -> Self {
        self.with_style(|s| s.max_lines = (lines > 0).then_some(lines))
    }
    /// Never wraps (each paragraph on one line, cut at the edge).
    fn nowrap(self) -> Self {
        self.with_style(|s| s.no_wrap = true)
    }
    /// How the lines are broken when the text wraps (CSS `text-wrap`):
    /// [`TextWrap::Pretty`] or [`TextWrap::Balance`].
    fn text_wrap(self, wrap: TextWrap) -> Self {
        self.with_style(|s| s.text_wrap = wrap)
    }
    /// No lone word on the last line (`text-wrap: pretty`).
    fn text_pretty(self) -> Self {
        self.text_wrap(TextWrap::Pretty)
    }
    /// Lines of about the same length (`text-wrap: balance`).
    fn text_balance(self) -> Self {
        self.text_wrap(TextWrap::Balance)
    }

    // Box decorations (see the `decoration` module)
    /// Adds a shadow (several stack, the first on top):
    /// `.shadow(Shadow::new(0., 4., 12., rgba(0x000000, 0.15)))`.
    fn shadow(self, shadow: crate::decoration::Shadow) -> Self {
        self.with_style(|s| s.shadows.push(shadow))
    }
    /// Removes every shadow (e.g. in a state refinement).
    fn shadow_none(self) -> Self {
        self.with_style(|s| s.shadows.clear())
    }
    /// The border of the top side only: `width` points of `color`.
    fn border_top(self, width: f32, color: Color) -> Self {
        let side = crate::decoration::BorderSide {
            width,
            color: Some(color),
        };
        self.with_style(|s| s.border_sides.top = Some(side))
    }
    /// The border of the bottom side only (a row separator).
    fn border_bottom(self, width: f32, color: Color) -> Self {
        let side = crate::decoration::BorderSide {
            width,
            color: Some(color),
        };
        self.with_style(|s| s.border_sides.bottom = Some(side))
    }
    /// The border of the leading side only (left in left-to-right languages).
    fn border_left(self, width: f32, color: Color) -> Self {
        let side = crate::decoration::BorderSide {
            width,
            color: Some(color),
        };
        self.with_style(|s| s.border_sides.left = Some(side))
    }
    /// The border of the trailing side only (right in left-to-right languages).
    fn border_right(self, width: f32, color: Color) -> Self {
        let side = crate::decoration::BorderSide {
            width,
            color: Some(color),
        };
        self.with_style(|s| s.border_sides.right = Some(side))
    }
    /// Dashed borders.
    fn border_dashed(self) -> Self {
        self.with_style(|s| s.border_style = crate::decoration::BorderStyle::Dashed)
    }
    /// Dotted borders.
    fn border_dotted(self) -> Self {
        self.with_style(|s| s.border_style = crate::decoration::BorderStyle::Dotted)
    }
    /// Clips the children to the box, rounded corners included (CSS
    /// `overflow: hidden`).
    fn clip(self) -> Self {
        self.with_style(|s| s.clip = true)
    }
    /// A gradient background: `.bg_gradient(Gradient::linear(180., [a, b]))`,
    /// drawn over the [`bg`](Self::bg) color (which shows through its
    /// translucent stops), clipped to the rounded corners. Its stops are
    /// [normalized](crate::Gradient::normalized); a gradient without stops
    /// removes it.
    fn bg_gradient(self, gradient: crate::decoration::Gradient) -> Self {
        self.with_style(|s| s.gradient = gradient.normalized())
    }
    /// A linear gradient background through `(color, position)` stops
    /// (positions `0.0..=1.0` along the gradient line, at least two stops
    /// for a gradient), at `angle` degrees in the CSS convention: `0` to
    /// the top, `90` to the right, `180` to the bottom, `270` to the left.
    /// Shorthand for `.bg_gradient(Gradient::linear_stops(angle, stops))`.
    ///
    /// The end of a scrolling list fading into its background:
    /// `.bg_linear_gradient(180., [(bg.with_alpha(0.), 0.), (bg, 1.)])`.
    fn bg_linear_gradient(self, angle: f32, stops: impl IntoIterator<Item = (Color, f32)>) -> Self {
        self.bg_gradient(crate::decoration::Gradient::linear_stops(angle, stops))
    }
    /// The whole background: a solid color (`.bg_fill(rgb(0x007AFF))`) or a
    /// gradient (`.bg_fill(Gradient::linear(90., [a, b]))`), replacing
    /// whatever was there, where `bg` and `bg_gradient` layer. Handy in
    /// state refinements, which start from the base style:
    /// `.bg_gradient(g).hover(|s| s.bg_fill(accent))`.
    fn bg_fill(self, fill: impl Into<crate::decoration::Fill>) -> Self {
        let fill = fill.into();
        self.with_style(|s| match fill {
            crate::decoration::Fill::Solid(c) => {
                s.background = Some(c);
                s.gradient = None;
            }
            crate::decoration::Fill::Gradient(g) => {
                s.background = None;
                s.gradient = g.normalized();
            }
        })
    }
    /// Blurs what is behind the node (`radius` points), under a translucent
    /// `bg`: frosted bars and panels.
    fn backdrop_blur(self, radius: f32) -> Self {
        self.with_style(|s| s.backdrop_blur = radius)
    }

    // Interaction states (see the `interaction` module)
    /// Style changes while the pointer is over the node:
    /// `.hover(|s| s.bg(rgb(0xEEEEF2)))`.
    fn hover(self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.with_style(|s| {
            s.states
                .push(crate::interaction::Interaction::Hover, std::rc::Rc::new(f))
        })
    }
    /// Style changes while the node is pressed.
    fn active(self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.with_style(|s| {
            s.states
                .push(crate::interaction::Interaction::Active, std::rc::Rc::new(f))
        })
    }
    /// Style changes while the node has the keyboard focus.
    fn focus(self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.with_style(|s| {
            s.states
                .push(crate::interaction::Interaction::Focus, std::rc::Rc::new(f))
        })
    }
    /// Style changes while the node or a descendant has the keyboard focus
    /// (a row whose field is being edited).
    fn focus_within(self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.with_style(|s| {
            s.states.push(
                crate::interaction::Interaction::FocusWithin,
                std::rc::Rc::new(f),
            )
        })
    }
    /// Style changes while the node is disabled (`accessibility_disabled`);
    /// a disabled node ignores its hover, press and focus refinements.
    fn disabled(self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.with_style(|s| {
            s.states.push(
                crate::interaction::Interaction::Disabled,
                std::rc::Rc::new(f),
            )
        })
    }
    /// Style changes while files are dragged over a drop target that
    /// accepts them ([`Div::on_drop_files`](crate::Div::on_drop_files),
    /// #111): the highlight of an upload zone. Also while an app payload it
    /// accepts is dragged over it ([`Div::on_drop_payload`](crate::Div::on_drop_payload),
    /// #251): the column a card would land in.
    fn drop_hover(self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.with_style(|s| {
            s.states.push(
                crate::interaction::Interaction::DropHover,
                std::rc::Rc::new(f),
            )
        })
    }
    /// The pointer's shape over the node (clickable nodes default to
    /// [`Cursor::Pointer`](crate::decoration::Cursor::Pointer)).
    fn cursor(self, cursor: crate::decoration::Cursor) -> Self {
        self.with_style(|s| s.cursor = Some(cursor))
    }
    /// Marks the node as a group: descendants' `group_hover`,
    /// `group_active`, `group_focus_within` and `group_drop_hover`
    /// refinements follow its states.
    fn group(self) -> Self {
        self.with_style(|s| s.group = true)
    }
    /// Style changes while the nearest [`group`](Self::group) ancestor is
    /// hovered (a row's actions appear under the pointer).
    fn group_hover(self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.with_style(|s| {
            s.states.push(
                crate::interaction::Interaction::GroupHover,
                std::rc::Rc::new(f),
            )
        })
    }
    /// Draws the node above its siblings and what is in them (#232), like
    /// CSS `z-index`: a handle sticking out of a grid cell over the next
    /// cell, a badge over a neighbour. Higher is above; 0 (the default)
    /// keeps the tree order; siblings with the same value keep theirs.
    /// Natively a child's value also lifts its ancestors among their own
    /// siblings (up to one that sets its own), so the handle of a cell is
    /// above the next cell. Clicks go to what is drawn on top (with #213).
    /// Works in refinements: `.hover(|s| s.z_index(1))`.
    fn z_index(self, z: i32) -> Self {
        self.with_style(|s| s.z_index = z)
    }
    /// Marks the node as a group named `name` (#212): descendants'
    /// [`group_hover_named`](Self::group_hover_named) (and
    /// `group_active_named`, `group_focus_within_named`) refinements with
    /// that name follow its states, whatever groups sit in between. Like
    /// CSS `.group/name`: plain `group_hover` refinements don't follow it.
    ///
    /// ```ignore
    /// div().group_named("row").child(
    ///     grip().opacity(0.).group_hover_named("row", |s| s.opacity(1.).w(px(16.))),
    /// )
    /// ```
    fn group_named(self, name: &str) -> Self {
        let name: std::rc::Rc<str> = name.into();
        self.with_style(move |s| s.group_name = Some(name.clone()))
    }
    /// Style changes while the [`group_named`](Self::group_named) ancestor
    /// `name` is hovered, resolved by the backend like `group_hover` (no
    /// app round trip). Layout changes (a width, a padding) apply too.
    fn group_hover_named(self, name: &str, f: impl Fn(Style) -> Style + 'static) -> Self {
        let name: std::rc::Rc<str> = name.into();
        self.with_style(move |s| {
            s.states.push_named(
                crate::interaction::Interaction::GroupHover,
                name.clone(),
                std::rc::Rc::new(f),
            )
        })
    }
    /// Style changes while the [`group_named`](Self::group_named) ancestor
    /// `name` is pressed.
    fn group_active_named(self, name: &str, f: impl Fn(Style) -> Style + 'static) -> Self {
        let name: std::rc::Rc<str> = name.into();
        self.with_style(move |s| {
            s.states.push_named(
                crate::interaction::Interaction::GroupActive,
                name.clone(),
                std::rc::Rc::new(f),
            )
        })
    }
    /// Style changes while the [`group_named`](Self::group_named) ancestor
    /// `name` contains the keyboard focus.
    fn group_focus_within_named(self, name: &str, f: impl Fn(Style) -> Style + 'static) -> Self {
        let name: std::rc::Rc<str> = name.into();
        self.with_style(move |s| {
            s.states.push_named(
                crate::interaction::Interaction::GroupFocusWithin,
                name.clone(),
                std::rc::Rc::new(f),
            )
        })
    }
    /// Style changes while files a [`group`](Self::group) ancestor accepts
    /// are dragged over it (#290): that ancestor is the drop target
    /// ([`Div::on_drop_files`](crate::Div::on_drop_files)), its children
    /// change with it.
    ///
    /// ```ignore
    /// div().group().on_drop_files(["image/*"], on_drop).child(
    ///     text("Drop images here")
    ///         .opacity(0.6)
    ///         .group_drop_hover(|s| s.opacity(1.).bold()),
    /// )
    /// ```
    fn group_drop_hover(self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.with_style(|s| {
            s.states.push(
                crate::interaction::Interaction::GroupDropHover,
                std::rc::Rc::new(f),
            )
        })
    }
    /// Style changes while files the [`group_named`](Self::group_named)
    /// ancestor `name` accepts are dragged over it (#290).
    fn group_drop_hover_named(self, name: &str, f: impl Fn(Style) -> Style + 'static) -> Self {
        let name: std::rc::Rc<str> = name.into();
        self.with_style(move |s| {
            s.states.push_named(
                crate::interaction::Interaction::GroupDropHover,
                name.clone(),
                std::rc::Rc::new(f),
            )
        })
    }
    /// Style changes while a [`group`](Self::group) ancestor is pressed.
    fn group_active(self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.with_style(|s| {
            s.states.push(
                crate::interaction::Interaction::GroupActive,
                std::rc::Rc::new(f),
            )
        })
    }
    /// Style changes while a [`group`](Self::group) ancestor contains the
    /// keyboard focus: a field's clear button with
    /// `.with_style(|s| s.display_none = true)` and
    /// `.group_focus_within(|s| s.with_style(|s| s.display_none = false))`
    /// shows only while the field is edited.
    fn group_focus_within(self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.with_style(|s| {
            s.states.push(
                crate::interaction::Interaction::GroupFocusWithin,
                std::rc::Rc::new(f),
            )
        })
    }

    // Motion: loops that start over, non-uniform scale, relative offsets.
    /// Plays from its style to `pose`, then starts over, forever: with
    /// `Pose::new().rotate(360.)` and a linear transition, a spinner.
    fn repeat_from_start(self, pose: crate::animation::Pose, t: Transition) -> Self {
        self.with_style(|s| {
            s.animations.repeat = Some((pose, t));
            s.animations.restart = true;
        })
    }
    /// Scales it horizontally around its center (with [`scale`](Self::scale)).
    fn scale_x(self, factor: f32) -> Self {
        self.with_style(|s| s.transform.scale_x = factor)
    }
    /// Scales it vertically around its center (with [`scale`](Self::scale)).
    fn scale_y(self, factor: f32) -> Self {
        self.with_style(|s| s.transform.scale_y = factor)
    }
    /// Moves it by `x` % of its own width and `y` % of its own height,
    /// without changing the layout: `.absolute().left(relative(0.3))
    /// .translate_percent(-50., 0.)` centers a label on 30 % of its parent.
    fn translate_percent(self, x: f32, y: f32) -> Self {
        self.with_style(|s| s.translate_percent = (x, y))
    }

    // --- Hit testing (#19) ---

    /// `false`: the element and everything in it let clicks, taps and
    /// hovers through to what is underneath (`pointer-events: none`): a
    /// decorative overlay, a scrim drawn over content.
    fn hit_test(self, on: bool) -> Self {
        self.with_style(|s| s.hit_test = on)
    }

    /// Makes the element and everything in it unreachable while `on`: no
    /// pointer input, no keyboard focus, hidden from screen readers (the
    /// screen behind a custom overlay). It stays visible.
    fn inert(self, on: bool) -> Self {
        self.with_style(|s| s.inert = on)
    }

    // --- Custom window chrome (#67) ---

    /// Makes the element a title bar for the window: dragging it moves the
    /// window, and a double click does what the user chose in System
    /// Settings (« Double-click a window's title bar »: zoom by default,
    /// minimize, or nothing; on Windows it maximizes or restores).
    /// Clickable descendants (buttons, tabs, `on_click` views) keep their
    /// clicks; put it on a container (on a clickable element the click
    /// wins). Pairs with a custom window chrome
    /// (`Application::window_chrome`). macOS and Windows: ignored elsewhere.
    fn window_drag(self) -> Self {
        self.with_style(|s| s.window_drag = true)
    }
}
