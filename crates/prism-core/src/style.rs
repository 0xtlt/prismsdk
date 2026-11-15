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

    /// CSS notation; adaptive colors use `light-dark()`.
    pub fn to_css(self) -> String {
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
}

/// Font weight (system font).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
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
}

impl FontWeight {
    /// The CSS `font-weight` number (400, 500, 600, 700).
    pub fn css(self) -> u16 {
        match self {
            FontWeight::Regular => 400,
            FontWeight::Medium => 500,
            FontWeight::Semibold => 600,
            FontWeight::Bold => 700,
        }
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
    /// Inner spacing, points.
    pub padding: Edges,
    /// Outer spacing, points.
    pub margin: Edges,
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

    /// Background color.
    pub background: Option<Color>,
    /// Corner radius, points.
    pub corner_radius: f32,
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
    /// The class of a window `width` points wide.
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

/// What responsive rules are resolved against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
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
            return self.clone();
        }
        let mut rules = self.responsive.0.clone();
        rules.sort_by_key(|(c, _)| match c {
            Condition::From(b) => Some(*b),
            _ => None,
        });
        let mut s = self.clone();
        s.responsive = Responsive::default();
        for (condition, rule) in rules {
            let applies = match condition {
                Condition::From(b) => b <= cx.breakpoint,
                Condition::On(set) => cx.platform.is_some_and(|p| set.contains(p)),
                Condition::Orientation(o) => o == cx.orientation,
                Condition::Posture(p) => p == cx.posture,
                Condition::Kind(k) => k == cx.kind,
            };
            if applies {
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
            background: None,
            corner_radius: 0.,
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

    // Flex item
    /// Grows and shrinks from a zero basis: siblings with `flex_1` share the space equally.
    fn flex_1(self) -> Self {
        self.with_style(|s| {
            s.flex_grow = 1.;
            s.flex_shrink = 1.;
            s.flex_basis = Length::Px(0.)
        })
    }
    /// Grows into the free space.
    fn flex_grow(self) -> Self {
        self.with_style(|s| s.flex_grow = 1.)
    }
    /// Never shrinks below its size.
    fn flex_shrink_0(self) -> Self {
        self.with_style(|s| s.flex_shrink = 0.)
    }

    // Spacing
    /// Padding on every edge, points.
    fn p(self, v: f32) -> Self {
        self.with_style(|s| s.padding = Edges::all(v))
    }
    /// Leading and trailing padding, points.
    fn px(self, v: f32) -> Self {
        self.with_style(|s| {
            s.padding.left = v;
            s.padding.right = v
        })
    }
    /// Top and bottom padding, points.
    fn py(self, v: f32) -> Self {
        self.with_style(|s| {
            s.padding.top = v;
            s.padding.bottom = v
        })
    }
    /// Top padding, points.
    fn pt(self, v: f32) -> Self {
        self.with_style(|s| s.padding.top = v)
    }
    /// Trailing padding (right in left-to-right languages), points.
    fn pr(self, v: f32) -> Self {
        self.with_style(|s| s.padding.right = v)
    }
    /// Bottom padding, points.
    fn pb(self, v: f32) -> Self {
        self.with_style(|s| s.padding.bottom = v)
    }
    /// Leading padding (left in left-to-right languages), points.
    fn pl(self, v: f32) -> Self {
        self.with_style(|s| s.padding.left = v)
    }
    /// Margin on every edge, points.
    fn m(self, v: f32) -> Self {
        self.with_style(|s| s.margin = Edges::all(v))
    }
    /// Leading and trailing margin, points.
    fn mx(self, v: f32) -> Self {
        self.with_style(|s| {
            s.margin.left = v;
            s.margin.right = v
        })
    }
    /// Top and bottom margin, points.
    fn my(self, v: f32) -> Self {
        self.with_style(|s| {
            s.margin.top = v;
            s.margin.bottom = v
        })
    }
    /// Top margin, points.
    fn mt(self, v: f32) -> Self {
        self.with_style(|s| s.margin.top = v)
    }
    /// Leading margin (flips in right-to-left languages).
    fn ml(self, v: f32) -> Self {
        self.with_style(|s| s.margin.left = v)
    }
    /// Trailing margin.
    fn mr(self, v: f32) -> Self {
        self.with_style(|s| s.margin.right = v)
    }
    /// Bottom margin, points.
    fn mb(self, v: f32) -> Self {
        self.with_style(|s| s.margin.bottom = v)
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
        self.with_style(|s| s.corner_radius = r)
    }
    /// Border width, points.
    fn border(self, w: f32) -> Self {
        self.with_style(|s| s.border_width = w)
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
}
