//! Box decorations: shadows, per-side and dashed borders, gradients, and the
//! text-relative color ([`Color::current`]). Every backend draws them
//! natively (CSS, Core Animation layers, Android drawables, GTK CSS, SwiftUI
//! modifiers); see the styling guide for what each platform does.

use crate::style::Color;

/// A drop shadow, or an inner shadow with [`inset`](Shadow::inset).
///
/// Offsets are physical (positive `x` goes right in every language), like
/// CSS `box-shadow`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    /// Horizontal offset, points (positive: to the right).
    pub x: f32,
    /// Vertical offset, points (positive: down).
    pub y: f32,
    /// Blur radius, points (`0`: a hard edge).
    pub blur: f32,
    /// Grows (positive) or shrinks (negative) the shadow's shape, points.
    pub spread: f32,
    /// Shadow color (its alpha is the shadow's strength).
    pub color: Color,
    /// Drawn inside the box, over the background, instead of under it.
    pub inset: bool,
}

impl Shadow {
    /// A drop shadow offset by `x`, `y`, blurred by `blur` points.
    pub fn new(x: f32, y: f32, blur: f32, color: Color) -> Shadow {
        Shadow {
            x,
            y,
            blur,
            spread: 0.,
            color,
            inset: false,
        }
    }

    /// The shadow's shape grown by `v` points (negative shrinks it).
    pub fn spread(mut self, v: f32) -> Shadow {
        self.spread = v;
        self
    }

    /// An inner shadow (drawn inside the box, over its background).
    pub fn inset(mut self) -> Shadow {
        self.inset = true;
        self
    }

    /// A ring of `width` points just outside the box (a focus ring):
    /// a hard shadow with no offset or blur, spread by `width`.
    pub fn ring(width: f32, color: Color) -> Shadow {
        Shadow::new(0., 0., 0., color).spread(width)
    }

    /// CSS `box-shadow` value of this shadow.
    pub fn to_css(&self) -> String {
        format!(
            "{}{}px {}px {}px {}px {}",
            if self.inset { "inset " } else { "" },
            self.x,
            self.y,
            self.blur,
            self.spread,
            self.color.to_css()
        )
    }

    /// The same shadow `factor` times bigger (TV zoom).
    pub fn scaled(self, factor: f32) -> Shadow {
        Shadow {
            x: self.x * factor,
            y: self.y * factor,
            blur: self.blur * factor,
            spread: self.spread * factor,
            ..self
        }
    }
}

/// A small, soft elevation (cards).
pub fn shadow_sm() -> Shadow {
    Shadow::new(0., 1., 3., Color::BLACK.with_alpha(0.12))
}

/// A medium elevation (menus, popovers).
pub fn shadow_md() -> Shadow {
    Shadow::new(0., 4., 12., Color::BLACK.with_alpha(0.14))
}

/// A large elevation (dialogs, floating panels).
pub fn shadow_lg() -> Shadow {
    Shadow::new(0., 12., 32., Color::BLACK.with_alpha(0.18))
}

/// How a border's line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BorderStyle {
    /// A continuous line (the default).
    #[default]
    Solid,
    /// Dashes.
    Dashed,
    /// Round dots.
    Dotted,
}

impl BorderStyle {
    /// The CSS `border-style` keyword.
    pub fn css(self) -> &'static str {
        match self {
            BorderStyle::Solid => "solid",
            BorderStyle::Dashed => "dashed",
            BorderStyle::Dotted => "dotted",
        }
    }

    /// The dash pattern `(on, off)` for a line `width` points wide, as
    /// native backends stroke it (`None`: solid). Dots are drawn as zero-length
    /// dashes with round caps, `2 × width` apart.
    pub fn dash(self, width: f32) -> Option<(f32, f32)> {
        let w = width.max(1.);
        match self {
            BorderStyle::Solid => None,
            BorderStyle::Dashed => Some((3. * w, 2. * w)),
            BorderStyle::Dotted => Some((0., 2. * w)),
        }
    }
}

/// The border of one side of a box.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct BorderSide {
    /// Width, points.
    pub width: f32,
    /// Color (`None`: the text color, like CSS `currentColor`).
    pub color: Option<Color>,
}

/// Per-side borders: a side set here replaces the uniform
/// [`border_width`](crate::Style::border_width) / [`border_color`](crate::Style::border_color)
/// on that side. `left` and `right` are the leading and trailing sides (they
/// swap in right-to-left languages, like margins).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct BorderSides {
    /// Top side.
    pub top: Option<BorderSide>,
    /// Trailing side (right in left-to-right languages).
    pub right: Option<BorderSide>,
    /// Bottom side.
    pub bottom: Option<BorderSide>,
    /// Leading side (left in left-to-right languages).
    pub left: Option<BorderSide>,
}

impl BorderSides {
    /// Whether no side is set.
    pub fn is_empty(&self) -> bool {
        self.top.is_none() && self.right.is_none() && self.bottom.is_none() && self.left.is_none()
    }

    /// Leading and trailing swapped (right-to-left layouts).
    pub fn mirrored(self) -> BorderSides {
        BorderSides {
            left: self.right,
            right: self.left,
            ..self
        }
    }
}

/// The geometry of a [`Gradient`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientKind {
    /// Along a line at `angle` degrees, CSS-style: `0` goes up, `90` to the
    /// right, `180` (the default of CSS) down.
    Linear {
        /// Degrees, clockwise from "to top".
        angle: f32,
    },
    /// From the center out to the corners (an ellipse the box's shape, like
    /// CSS `radial-gradient()`).
    Radial,
    /// Around the center, like CSS `conic-gradient()`: the first color at
    /// `from` degrees, the others following clockwise, the last one meeting
    /// the first after a full turn.
    Conic {
        /// Degrees, clockwise from "up" (`0`: up, `90`: right).
        from: f32,
    },
}

/// One color of a [`Gradient`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop {
    /// The color.
    pub color: Color,
    /// Where it is reached, `0.0` (start) to `1.0` (end).
    pub at: f32,
}

/// A background gradient (see [`Styled::bg_gradient`](crate::Styled::bg_gradient)).
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    /// Linear, radial or conic.
    pub kind: GradientKind,
    /// The colors, in order of position.
    pub stops: Vec<GradientStop>,
}

fn even(colors: impl IntoIterator<Item = Color>) -> Vec<GradientStop> {
    let colors: Vec<Color> = colors.into_iter().collect();
    let n = colors.len();
    colors
        .into_iter()
        .enumerate()
        .map(|(i, color)| GradientStop {
            color,
            at: if n > 1 { i as f32 / (n - 1) as f32 } else { 0. },
        })
        .collect()
}

impl Gradient {
    /// A linear gradient at `angle` degrees (`180`: top to bottom, `90`: left
    /// to right) through `colors`, evenly spaced.
    pub fn linear(angle: f32, colors: impl IntoIterator<Item = Color>) -> Gradient {
        Gradient {
            kind: GradientKind::Linear { angle },
            stops: even(colors),
        }
    }

    /// A radial gradient from the center (first color) to the corners (last
    /// color), evenly spaced.
    pub fn radial(colors: impl IntoIterator<Item = Color>) -> Gradient {
        Gradient {
            kind: GradientKind::Radial,
            stops: even(colors),
        }
    }

    /// A conic gradient around the center (a hue wheel, a pie): the first
    /// color at `from` degrees (`0`: up, `90`: right), the colors evenly
    /// spaced clockwise over a full turn. Repeat the first color last for a
    /// seamless wheel; [`with_stops`](Self::with_stops) gives hard edges
    /// (two stops at the same position).
    pub fn conic(from: f32, colors: impl IntoIterator<Item = Color>) -> Gradient {
        Gradient {
            kind: GradientKind::Conic { from },
            stops: even(colors),
        }
    }

    /// Replaces the stops with `(color, position)` pairs (positions `0.0..=1.0`).
    pub fn with_stops(mut self, stops: impl IntoIterator<Item = (Color, f32)>) -> Gradient {
        self.stops = stops
            .into_iter()
            .map(|(color, at)| GradientStop { color, at })
            .collect();
        self
    }

    /// Every color changed by `f` (backends resolve [`Color::current`]).
    pub fn map_colors(&self, f: impl Fn(Color) -> Color) -> Gradient {
        Gradient {
            kind: self.kind,
            stops: self
                .stops
                .iter()
                .map(|s| GradientStop {
                    color: f(s.color),
                    at: s.at,
                })
                .collect(),
        }
    }

    /// CSS `background-image` value.
    pub fn to_css(&self) -> String {
        let stops: Vec<String> = self
            .stops
            .iter()
            .map(|s| format!("{} {}%", s.color.to_css(), s.at * 100.))
            .collect();
        match self.kind {
            GradientKind::Linear { angle } => {
                format!("linear-gradient({angle}deg, {})", stops.join(", "))
            }
            GradientKind::Radial => format!("radial-gradient({})", stops.join(", ")),
            GradientKind::Conic { from } => {
                format!("conic-gradient(from {from}deg, {})", stops.join(", "))
            }
        }
    }

    /// Where a linear gradient starts and ends in a `w` × `h` box, in
    /// points from the top-left corner: `[x0, y0, x1, y1]`, the same line CSS
    /// uses (its length makes the corners reach the first and last colors).
    /// For a radial gradient: the center and the radii `[cx, cy, rx, ry]`.
    /// For a conic gradient: the center and the point where the first color
    /// starts, on the largest circle inside the box: `[cx, cy, x1, y1]`.
    pub fn geometry(&self, w: f32, h: f32) -> [f32; 4] {
        match self.kind {
            GradientKind::Linear { angle } => {
                let a = angle.to_radians();
                let (dx, dy) = (a.sin(), -a.cos());
                let half = (w * dx.abs() + h * dy.abs()) / 2.;
                let (cx, cy) = (w / 2., h / 2.);
                [
                    cx - dx * half,
                    cy - dy * half,
                    cx + dx * half,
                    cy + dy * half,
                ]
            }
            GradientKind::Radial => {
                let k = std::f32::consts::SQRT_2 / 2.;
                [w / 2., h / 2., w * k, h * k]
            }
            GradientKind::Conic { from } => {
                let a = from.to_radians();
                let r = w.min(h) / 2.;
                let (cx, cy) = (w / 2., h / 2.);
                [cx, cy, cx + a.sin() * r, cy - a.cos() * r]
            }
        }
    }
}

impl Color {
    /// The text color of the node it is used on (CSS `currentColor`), at
    /// full strength; `Color::current().with_alpha(0.12)` is a 12% tint of
    /// it (`color-mix(in srgb, currentColor 12%, transparent)`). Backgrounds,
    /// borders, shadows and gradients can use it; each backend resolves it
    /// against the node's [`text_color`](crate::Style::text_color), or the
    /// platform's label color when unset.
    ///
    /// It is marked by negative components: read colors through
    /// [`resolve_current`](Self::resolve_current) before using their channels.
    pub const fn current() -> Color {
        Color {
            r: -1.,
            g: -1.,
            b: -1.,
            a: 1.,
            dark: None,
        }
    }

    /// Whether this is [`Color::current`] (at some alpha).
    pub fn is_current(self) -> bool {
        self.r < 0.
    }

    /// This color with [`Color::current`] replaced by `text` (alphas
    /// multiplied, both variants of an adaptive `text`); other colors
    /// unchanged.
    pub fn resolve_current(self, text: Color) -> Color {
        if !self.is_current() {
            return self;
        }
        let text = if text.is_current() {
            Color::BLACK
        } else {
            text
        };
        let a = self.a;
        Color {
            a: text.a * a,
            dark: text.dark.map(|[r, g, b, da]| [r, g, b, da * a]),
            ..text
        }
    }
}

/// The pointer's shape over a node (see [`Styled::cursor`](crate::Styled::cursor)).
/// Clickable nodes get [`Pointer`](Cursor::Pointer) by default where there is
/// a pointer (web, macOS, iPad, Linux, Windows, Android with a mouse).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cursor {
    /// The platform's arrow.
    Default,
    /// A pointing hand: clickable.
    Pointer,
    /// An I-beam: selectable or editable text.
    Text,
    /// A crosshair: precise selection.
    Crosshair,
    /// Four arrows: movable.
    Move,
    /// An open hand: can be dragged.
    Grab,
    /// A closed hand: being dragged.
    Grabbing,
    /// A forbidden sign: unavailable.
    NotAllowed,
    /// Left and right arrows: resizable horizontally.
    ResizeHorizontal,
    /// Up and down arrows: resizable vertically.
    ResizeVertical,
    /// A question mark: help is available.
    Help,
    /// Busy in the background, still usable.
    Progress,
    /// Resizing from the top edge.
    ResizeN,
    /// Resizing from the bottom edge.
    ResizeS,
    /// Resizing from the right edge.
    ResizeE,
    /// Resizing from the left edge.
    ResizeW,
    /// Resizing from the top right corner.
    ResizeNe,
    /// Resizing from the top left corner.
    ResizeNw,
    /// Resizing from the bottom right corner.
    ResizeSe,
    /// Resizing from the bottom left corner.
    ResizeSw,
    /// Diagonal arrows, bottom left to top right (#223).
    ResizeNeSw,
    /// Diagonal arrows, top left to bottom right (#223).
    ResizeNwSe,
    /// A magnifier with a plus.
    ZoomIn,
    /// A magnifier with a minus.
    ZoomOut,
}

impl Cursor {
    /// The CSS `cursor` keyword (also GTK's cursor name).
    pub fn css(self) -> &'static str {
        match self {
            Cursor::Default => "default",
            Cursor::Pointer => "pointer",
            Cursor::Text => "text",
            Cursor::Crosshair => "crosshair",
            Cursor::Move => "move",
            Cursor::Grab => "grab",
            Cursor::Grabbing => "grabbing",
            Cursor::NotAllowed => "not-allowed",
            Cursor::ResizeHorizontal => "ew-resize",
            Cursor::ResizeVertical => "ns-resize",
            Cursor::Help => "help",
            Cursor::Progress => "progress",
            Cursor::ResizeN => "n-resize",
            Cursor::ResizeS => "s-resize",
            Cursor::ResizeE => "e-resize",
            Cursor::ResizeW => "w-resize",
            Cursor::ResizeNe => "ne-resize",
            Cursor::ResizeNw => "nw-resize",
            Cursor::ResizeSe => "se-resize",
            Cursor::ResizeSw => "sw-resize",
            Cursor::ResizeNeSw => "nesw-resize",
            Cursor::ResizeNwSe => "nwse-resize",
            Cursor::ZoomIn => "zoom-in",
            Cursor::ZoomOut => "zoom-out",
        }
    }

    /// The cursor for a CSS keyword (as [`css`](Self::css) writes it).
    pub fn from_css(name: &str) -> Option<Cursor> {
        Some(match name {
            "default" => Cursor::Default,
            "pointer" => Cursor::Pointer,
            "text" => Cursor::Text,
            "crosshair" => Cursor::Crosshair,
            "move" => Cursor::Move,
            "grab" => Cursor::Grab,
            "grabbing" => Cursor::Grabbing,
            "not-allowed" => Cursor::NotAllowed,
            "ew-resize" => Cursor::ResizeHorizontal,
            "ns-resize" => Cursor::ResizeVertical,
            "help" => Cursor::Help,
            "progress" => Cursor::Progress,
            "n-resize" => Cursor::ResizeN,
            "s-resize" => Cursor::ResizeS,
            "e-resize" => Cursor::ResizeE,
            "w-resize" => Cursor::ResizeW,
            "ne-resize" => Cursor::ResizeNe,
            "nw-resize" => Cursor::ResizeNw,
            "se-resize" => Cursor::ResizeSe,
            "sw-resize" => Cursor::ResizeSw,
            "nesw-resize" => Cursor::ResizeNeSw,
            "nwse-resize" => Cursor::ResizeNwSe,
            "zoom-in" => Cursor::ZoomIn,
            "zoom-out" => Cursor::ZoomOut,
            _ => return None,
        })
    }
}

impl crate::style::Style {
    /// The border of each side, `[top, right, bottom, left]`: the side's own
    /// border where set ([`border_sides`](Self::border_sides)), else the
    /// uniform `border_width` and `border_color`.
    pub fn borders(&self) -> [BorderSide; 4] {
        let uniform = BorderSide {
            width: self.border_width,
            color: self.border_color,
        };
        let b = self.border_sides;
        [b.top, b.right, b.bottom, b.left].map(|s| s.unwrap_or(uniform))
    }

    /// Whether the sides have different borders (drawn side by side rather
    /// than as one outline).
    pub fn has_side_borders(&self) -> bool {
        let [t, r, b, l] = self.borders();
        !self.border_sides.is_empty() && !(t == r && r == b && b == l)
    }

    /// The style with every [`Color::current`] resolved against its text
    /// color, or `label` (the platform's default text color) when it has
    /// none: what native backends draw.
    pub fn with_current_color(&self, label: Color) -> crate::style::Style {
        let text = self
            .text_color
            .map(|c| c.resolve_current(label))
            .unwrap_or(label);
        let c = |c: Color| c.resolve_current(text);
        let side = |s: Option<BorderSide>| {
            s.map(|s| BorderSide {
                color: s.color.map(c),
                ..s
            })
        };
        let b = self.border_sides;
        crate::style::Style {
            text_color: self.text_color.map(|t| t.resolve_current(label)),
            background: self.background.map(c),
            border_color: self.border_color.map(c),
            border_sides: BorderSides {
                top: side(b.top),
                right: side(b.right),
                bottom: side(b.bottom),
                left: side(b.left),
            },
            shadows: self
                .shadows
                .iter()
                .map(|s| Shadow {
                    color: c(s.color),
                    ..*s
                })
                .collect(),
            gradient: self.gradient.as_ref().map(|g| g.map_colors(c)),
            states: self.states.map_resolved(|s| s.with_current_color(label)),
            ..self.clone()
        }
    }
}
