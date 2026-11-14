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
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
    /// Dark-mode variant `[r, g, b, a]`, if the color adapts.
    pub dark: Option<[f32; 4]>,
}

impl Color {
    pub const BLACK: Color = Color {
        r: 0.,
        g: 0.,
        b: 0.,
        a: 1.,
        dark: None,
    };
    pub const WHITE: Color = Color {
        r: 1.,
        g: 1.,
        b: 1.,
        a: 1.,
        dark: None,
    };
    pub const TRANSPARENT: Color = Color {
        r: 0.,
        g: 0.,
        b: 0.,
        a: 0.,
        dark: None,
    };

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FlexDirection {
    Row,
    #[default]
    Column,
}

/// Cross-axis alignment (`align-items` / `align-self`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    Start,
    Center,
    End,
    #[default]
    Stretch,
}

/// Main-axis distribution (`justify-content`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FontWeight {
    #[default]
    Regular,
    Medium,
    Semibold,
    Bold,
}

impl FontWeight {
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
    #[default]
    Left,
    Center,
    Right,
}

/// Per-edge values (padding, margin).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    pub fn all(v: f32) -> Self {
        Edges {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }
}

pub const DEFAULT_FONT_SIZE: f32 = 16.;

/// The complete style of a node.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    /// Removed from layout and hidden (`display: none`).
    pub display_none: bool,
    pub direction: FlexDirection,
    pub wrap: bool,
    pub align_items: Align,
    pub align_self: Option<Align>,
    pub justify_content: Justify,
    pub gap: f32,
    pub padding: Edges,
    pub margin: Edges,
    pub width: Length,
    pub height: Length,
    pub min_width: Length,
    pub min_height: Length,
    pub max_width: Length,
    pub max_height: Length,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Length,

    pub background: Option<Color>,
    pub corner_radius: f32,
    pub border_width: f32,
    pub border_color: Option<Color>,
    pub opacity: f32,

    pub text_color: Option<Color>,
    pub font_size: f32,
    pub font_weight: FontWeight,
    pub text_align: TextAlign,

    /// Animates frame, opacity and color changes (skipped with reduce motion).
    pub transition: Option<Transition>,

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
    #[default]
    Compact,
    Medium,
    Expanded,
    Large,
}

impl Breakpoint {
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
    pub breakpoint: Breakpoint,
    pub platform: Option<crate::platform::Platform>,
    pub orientation: crate::device_info::Orientation,
    pub posture: crate::device_info::Posture,
    pub kind: crate::device_info::DeviceKind,
}

impl StyleContext {
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
    pub curve: Curve,
}

impl Transition {
    pub fn ease(ms: u32) -> Self {
        Transition {
            duration: ms as f32 / 1000.,
            curve: Curve::EaseInOut,
        }
    }
    pub fn linear(ms: u32) -> Self {
        Transition {
            duration: ms as f32 / 1000.,
            curve: Curve::Linear,
        }
    }
    pub fn spring(ms: u32) -> Self {
        Transition {
            duration: ms as f32 / 1000.,
            curve: Curve::Spring,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Curve {
    #[default]
    EaseInOut,
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
            transition: None,
            responsive: Responsive::default(),
            platforms: None,
        }
    }
}

/// GPUI-style fluent styling methods, available on every element builder.
pub trait Styled: Sized {
    fn style_mut(&mut self) -> &mut Style;

    fn with_style(mut self, f: impl FnOnce(&mut Style)) -> Self {
        f(self.style_mut());
        self
    }

    // Flex container
    fn flex_row(self) -> Self {
        self.with_style(|s| s.direction = FlexDirection::Row)
    }
    fn flex_col(self) -> Self {
        self.with_style(|s| s.direction = FlexDirection::Column)
    }
    fn flex_wrap(self) -> Self {
        self.with_style(|s| s.wrap = true)
    }
    fn gap(self, v: f32) -> Self {
        self.with_style(|s| s.gap = v)
    }
    fn items_start(self) -> Self {
        self.with_style(|s| s.align_items = Align::Start)
    }
    fn items_center(self) -> Self {
        self.with_style(|s| s.align_items = Align::Center)
    }
    fn items_end(self) -> Self {
        self.with_style(|s| s.align_items = Align::End)
    }
    fn items_stretch(self) -> Self {
        self.with_style(|s| s.align_items = Align::Stretch)
    }
    fn self_start(self) -> Self {
        self.with_style(|s| s.align_self = Some(Align::Start))
    }
    fn self_center(self) -> Self {
        self.with_style(|s| s.align_self = Some(Align::Center))
    }
    fn self_end(self) -> Self {
        self.with_style(|s| s.align_self = Some(Align::End))
    }
    fn justify_start(self) -> Self {
        self.with_style(|s| s.justify_content = Justify::Start)
    }
    fn justify_center(self) -> Self {
        self.with_style(|s| s.justify_content = Justify::Center)
    }
    fn justify_end(self) -> Self {
        self.with_style(|s| s.justify_content = Justify::End)
    }
    fn justify_between(self) -> Self {
        self.with_style(|s| s.justify_content = Justify::SpaceBetween)
    }
    fn justify_around(self) -> Self {
        self.with_style(|s| s.justify_content = Justify::SpaceAround)
    }
    fn justify_evenly(self) -> Self {
        self.with_style(|s| s.justify_content = Justify::SpaceEvenly)
    }

    // Flex item
    fn flex_1(self) -> Self {
        self.with_style(|s| {
            s.flex_grow = 1.;
            s.flex_shrink = 1.;
            s.flex_basis = Length::Px(0.)
        })
    }
    fn flex_grow(self) -> Self {
        self.with_style(|s| s.flex_grow = 1.)
    }
    fn flex_shrink_0(self) -> Self {
        self.with_style(|s| s.flex_shrink = 0.)
    }

    // Spacing
    fn p(self, v: f32) -> Self {
        self.with_style(|s| s.padding = Edges::all(v))
    }
    fn px(self, v: f32) -> Self {
        self.with_style(|s| {
            s.padding.left = v;
            s.padding.right = v
        })
    }
    fn py(self, v: f32) -> Self {
        self.with_style(|s| {
            s.padding.top = v;
            s.padding.bottom = v
        })
    }
    fn pt(self, v: f32) -> Self {
        self.with_style(|s| s.padding.top = v)
    }
    fn pr(self, v: f32) -> Self {
        self.with_style(|s| s.padding.right = v)
    }
    fn pb(self, v: f32) -> Self {
        self.with_style(|s| s.padding.bottom = v)
    }
    fn pl(self, v: f32) -> Self {
        self.with_style(|s| s.padding.left = v)
    }
    fn m(self, v: f32) -> Self {
        self.with_style(|s| s.margin = Edges::all(v))
    }
    fn mx(self, v: f32) -> Self {
        self.with_style(|s| {
            s.margin.left = v;
            s.margin.right = v
        })
    }
    fn my(self, v: f32) -> Self {
        self.with_style(|s| {
            s.margin.top = v;
            s.margin.bottom = v
        })
    }
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
    fn mb(self, v: f32) -> Self {
        self.with_style(|s| s.margin.bottom = v)
    }

    // Size
    fn w(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.width = v)
    }
    fn h(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.height = v)
    }
    fn size(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| {
            s.width = v;
            s.height = v
        })
    }
    fn w_full(self) -> Self {
        self.w(relative(1.))
    }
    fn h_full(self) -> Self {
        self.h(relative(1.))
    }
    fn size_full(self) -> Self {
        self.w_full().h_full()
    }
    fn min_w(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.min_width = v)
    }
    fn min_h(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.min_height = v)
    }
    fn max_w(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.max_width = v)
    }
    fn max_h(self, v: impl Into<Length>) -> Self {
        let v = v.into();
        self.with_style(|s| s.max_height = v)
    }

    // Visuals
    fn bg(self, c: Color) -> Self {
        self.with_style(|s| s.background = Some(c))
    }
    fn rounded(self, r: f32) -> Self {
        self.with_style(|s| s.corner_radius = r)
    }
    fn border(self, w: f32) -> Self {
        self.with_style(|s| s.border_width = w)
    }
    fn border_color(self, c: Color) -> Self {
        self.with_style(|s| s.border_color = Some(c))
    }
    fn opacity(self, o: f32) -> Self {
        self.with_style(|s| s.opacity = o)
    }

    // Text
    fn text_color(self, c: Color) -> Self {
        self.with_style(|s| s.text_color = Some(c))
    }
    fn text_size(self, v: f32) -> Self {
        self.with_style(|s| s.font_size = v)
    }
    fn font_weight(self, w: FontWeight) -> Self {
        self.with_style(|s| s.font_weight = w)
    }
    fn medium(self) -> Self {
        self.font_weight(FontWeight::Medium)
    }
    fn semibold(self) -> Self {
        self.font_weight(FontWeight::Semibold)
    }
    fn bold(self) -> Self {
        self.font_weight(FontWeight::Bold)
    }
    fn text_center(self) -> Self {
        self.with_style(|s| s.text_align = TextAlign::Center)
    }
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

    // Motion
    /// Animates changes of this element over `ms` milliseconds.
    fn animate(self, ms: u32) -> Self {
        self.transition(Transition::ease(ms))
    }
    fn transition(self, t: Transition) -> Self {
        self.with_style(|s| s.transition = Some(t))
    }
}
