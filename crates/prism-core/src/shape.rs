//! Vector drawing: lines, curves, arcs, filled or stroked, drawn natively
//! (UIKit/AppKit Bézier paths, Android `Canvas`, cairo, XAML `Path`s on
//! Windows, SVG). Charts are
//! built on it (see `prism-ui`).
//!
//! Coordinates are fractions of the shape's frame (`0..=1`, top-left
//! origin), so a drawing follows its layout size; radii are fractions of the
//! smaller side; angles are degrees, clockwise from 12 o'clock. Stroke widths
//! and insets are points.
//!
//! ```ignore
//! shape()
//!     .stroke_inset(2., 2.)
//!     .path(Path::line([(0., 1.), (0.5, 0.2), (1., 0.6)]).stroke(theme.accent, 2.).round())
//!     .path(Path::arc(0.5, 0.5, 0.5, 0., 270.).stroke(theme.success, 8.).round())
//!     .size(120., 80.)
//! ```

use std::fmt::Write;

use crate::style::{Color, FontFamily};

/// One step of a [`Path`]; coordinates are fractions of the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    /// Starts a new subpath at `(x, y)`.
    Move(f32, f32),
    /// A straight line to `(x, y)`.
    Line(f32, f32),
    /// Control point 1, control point 2, end.
    Cubic(f32, f32, f32, f32, f32, f32),
    /// Center, radius, start and end angles (degrees, clockwise from the
    /// top). Joined to the current point by a line, if any.
    Arc {
        /// Center x.
        cx: f32,
        /// Center y.
        cy: f32,
        /// Radius, a fraction of the frame's smaller side.
        r: f32,
        /// Start angle, degrees.
        start: f32,
        /// End angle, degrees.
        end: f32,
    },
    /// Closes the subpath with a line to its start.
    Close,
}

/// One path and how it is painted.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Path {
    /// The segments, in drawing order.
    pub segments: Vec<Segment>,
    /// Fill color, if filled.
    pub fill: Option<Color>,
    /// Stroke color, if stroked.
    pub stroke: Option<Color>,
    /// Stroke width, points.
    pub width: f32,
    /// Round caps and joins.
    pub round: bool,
    /// Dash and gap lengths, points.
    pub dash: Option<(f32, f32)>,
    /// Fills with the even-odd rule (overlaps cut holes) instead of
    /// non-zero winding (SVG `fill-rule="evenodd"`).
    pub even_odd: bool,
    /// A gradient the fill is painted with, in place of
    /// [`fill`](Self::fill) (which stays the flat color of the renderers
    /// without gradients).
    pub fill_gradient: Option<Gradient>,
    /// A gradient the stroke is painted with, in place of
    /// [`stroke`](Self::stroke).
    pub stroke_gradient: Option<Gradient>,
}

impl Path {
    /// An empty path, stroke width 1 point, neither filled nor stroked.
    pub fn new() -> Self {
        Path {
            width: 1.,
            ..Default::default()
        }
    }

    /// Starts a new subpath at `(x, y)`.
    pub fn move_to(mut self, x: f32, y: f32) -> Self {
        self.segments.push(Segment::Move(x, y));
        self
    }
    /// A straight line to `(x, y)`.
    pub fn line_to(mut self, x: f32, y: f32) -> Self {
        self.segments.push(Segment::Line(x, y));
        self
    }
    /// A cubic Bézier curve to `to`, with control points `c1` and `c2`.
    pub fn curve_to(mut self, c1: (f32, f32), c2: (f32, f32), to: (f32, f32)) -> Self {
        self.segments
            .push(Segment::Cubic(c1.0, c1.1, c2.0, c2.1, to.0, to.1));
        self
    }
    /// An arc around `(cx, cy)`, from `start` to `end` degrees (clockwise
    /// from the top; `end < start` runs counter-clockwise).
    pub fn arc_to(mut self, cx: f32, cy: f32, r: f32, start: f32, end: f32) -> Self {
        self.segments.push(Segment::Arc {
            cx,
            cy,
            r,
            start,
            end,
        });
        self
    }
    /// Closes the current subpath.
    pub fn close(mut self) -> Self {
        self.segments.push(Segment::Close);
        self
    }

    /// Straight segments through `points`.
    pub fn line(points: impl IntoIterator<Item = (f32, f32)>) -> Self {
        let mut p = Path::new();
        for (i, (x, y)) in points.into_iter().enumerate() {
            p = if i == 0 {
                p.move_to(x, y)
            } else {
                p.line_to(x, y)
            };
        }
        p
    }

    /// A smooth curve through `points` (sorted by x), without overshooting
    /// them (monotone cubic interpolation: a peak stays the highest point).
    pub fn smooth(points: impl IntoIterator<Item = (f32, f32)>) -> Self {
        let pts: Vec<(f32, f32)> = points.into_iter().collect();
        if pts.len() < 3 {
            return Path::line(pts);
        }
        let n = pts.len();
        let slope = |i: usize| {
            let dx = pts[i + 1].0 - pts[i].0;
            if dx == 0. {
                0.
            } else {
                (pts[i + 1].1 - pts[i].1) / dx
            }
        };
        let d: Vec<f32> = (0..n - 1).map(slope).collect();
        // Fritsch–Carlson tangents.
        let mut m = vec![0f32; n];
        m[0] = d[0];
        m[n - 1] = d[n - 2];
        for i in 1..n - 1 {
            m[i] = if d[i - 1] * d[i] <= 0. {
                0.
            } else {
                3. * (d[i - 1] + d[i])
                    / ((2. * d[i] + d[i - 1]) / d[i - 1] + (d[i] + 2. * d[i - 1]) / d[i])
            };
        }
        let mut p = Path::new().move_to(pts[0].0, pts[0].1);
        for i in 0..n - 1 {
            let ((x0, y0), (x1, y1)) = (pts[i], pts[i + 1]);
            let h = (x1 - x0) / 3.;
            p = p.curve_to(
                (x0 + h, y0 + m[i] * h),
                (x1 - h, y1 - m[i + 1] * h),
                (x1, y1),
            );
        }
        p
    }

    /// An arc on its own (a ring, a gauge).
    pub fn arc(cx: f32, cy: f32, r: f32, start: f32, end: f32) -> Self {
        Path::new().arc_to(cx, cy, r, start, end)
    }

    /// A pie slice (`inner` > 0: a donut segment), angles in degrees.
    pub fn sector(cx: f32, cy: f32, r: f32, inner: f32, start: f32, end: f32) -> Self {
        if inner > 0. {
            Path::arc(cx, cy, r, start, end)
                .arc_to(cx, cy, inner, end, start)
                .close()
        } else {
            Path::new()
                .move_to(cx, cy)
                .arc_to(cx, cy, r, start, end)
                .close()
        }
    }

    /// A rectangle with its top-left corner at `(x, y)`, `w` wide and `h` tall.
    pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Self {
        Path::new()
            .move_to(x, y)
            .line_to(x + w, y)
            .line_to(x + w, y + h)
            .line_to(x, y + h)
            .close()
    }

    /// A full circle around `(cx, cy)`.
    pub fn circle(cx: f32, cy: f32, r: f32) -> Self {
        Path::arc(cx, cy, r, 0., 360.).close()
    }

    /// Fills the path with `color`.
    pub fn fill(mut self, color: Color) -> Self {
        self.fill = Some(color);
        self
    }
    /// Strokes the path with `color`, `width` points wide.
    pub fn stroke(mut self, color: Color, width: f32) -> Self {
        self.stroke = Some(color);
        self.width = width;
        self
    }
    /// Fills the path with `gradient` (its middle color where gradients
    /// are not drawn).
    pub fn fill_gradient(mut self, gradient: Gradient) -> Self {
        self.fill = Some(gradient.color_at(0.5));
        self.fill_gradient = Some(gradient);
        self
    }
    /// Strokes the path with `gradient`, `width` points wide.
    pub fn stroke_gradient(mut self, gradient: Gradient, width: f32) -> Self {
        self.stroke = Some(gradient.color_at(0.5));
        self.stroke_gradient = Some(gradient);
        self.width = width;
        self
    }
    /// Round caps and joins.
    pub fn round(mut self) -> Self {
        self.round = true;
        self
    }
    /// Dashed stroke: `dash` points drawn, `gap` points skipped.
    pub fn dashed(mut self, dash: f32, gap: f32) -> Self {
        self.dash = Some((dash, gap));
        self
    }
    /// Fills with the even-odd rule: where subpaths overlap, holes.
    pub fn even_odd(mut self) -> Self {
        self.even_odd = true;
        self
    }

    /// Flips horizontally in a `width`-wide box (right-to-left layouts).
    pub fn mirror_x(&mut self, width: f32) {
        mirror_segments(&mut self.segments, width);
        for g in [&mut self.fill_gradient, &mut self.stroke_gradient]
            .into_iter()
            .flatten()
        {
            g.transform = concat([-1., 0., 0., 1., width, 0.], g.transform);
        }
    }
}

/// A 2D affine transform `[a, b, c, d, e, f]`, as in SVG's `matrix()`:
/// `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
pub type Matrix = [f32; 6];

/// The transform that changes nothing.
pub const IDENTITY: Matrix = [1., 0., 0., 1., 0., 0.];

/// `m · n`: `n` applied first, then `m`.
pub fn concat(m: Matrix, n: Matrix) -> Matrix {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}

/// A gradient's color at `offset` (`0.0..=1.0` along the gradient).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    /// Where along the gradient, `0.0..=1.0`.
    pub offset: f32,
    /// The color there.
    pub color: Color,
}

/// Where a [`Gradient`] runs, in its own coordinates (see
/// [`Gradient::transform`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientShape {
    /// From `(x1, y1)` (offset 0) to `(x2, y2)` (offset 1).
    Linear {
        /// Start x.
        x1: f32,
        /// Start y.
        y1: f32,
        /// End x.
        x2: f32,
        /// End y.
        y2: f32,
    },
    /// From the focal point `(fx, fy)` (offset 0) to the circle of radius
    /// `r` around `(cx, cy)` (offset 1).
    Radial {
        /// Center x.
        cx: f32,
        /// Center y.
        cy: f32,
        /// Radius.
        r: f32,
        /// Focal point x (`cx` for a plain radial gradient).
        fx: f32,
        /// Focal point y (`cy` for a plain radial gradient).
        fy: f32,
    },
}

/// What a [`Gradient`] paints before its first stop and after its last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Spread {
    /// The first and last colors go on (the default).
    #[default]
    Pad,
    /// The gradient runs back and forth.
    Reflect,
    /// The gradient starts over.
    Repeat,
}

/// A linear or radial gradient, the paint of a [`Path`]
/// ([`Path::fill_gradient`], [`Path::stroke_gradient`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    /// A line or a circle, in the gradient's own coordinates.
    pub shape: GradientShape,
    /// The colors, by increasing offset.
    pub stops: Vec<Stop>,
    /// What is painted outside `0.0..=1.0`.
    pub spread: Spread,
    /// Maps the gradient's own coordinates to the drawing's (fractions of
    /// the frame in a [`Drawing`], points once resolved). The identity for a
    /// gradient given in fractions of the frame.
    pub transform: Matrix,
}

impl Gradient {
    /// A gradient along the line `from` → `to` (fractions of the frame),
    /// through `stops` (`(offset, color)`, offsets `0.0..=1.0`).
    pub fn linear(
        from: (f32, f32),
        to: (f32, f32),
        stops: impl IntoIterator<Item = (f32, Color)>,
    ) -> Self {
        Gradient {
            shape: GradientShape::Linear {
                x1: from.0,
                y1: from.1,
                x2: to.0,
                y2: to.1,
            },
            stops: stops
                .into_iter()
                .map(|(offset, color)| Stop { offset, color })
                .collect(),
            spread: Spread::Pad,
            transform: IDENTITY,
        }
    }

    /// A gradient from `center` outwards to `r` (fractions of the frame: an
    /// ellipse in a frame that is not square), through `stops`.
    pub fn radial(
        center: (f32, f32),
        r: f32,
        stops: impl IntoIterator<Item = (f32, Color)>,
    ) -> Self {
        Gradient {
            shape: GradientShape::Radial {
                cx: center.0,
                cy: center.1,
                r,
                fx: center.0,
                fy: center.1,
            },
            ..Gradient::linear((0., 0.), (0., 0.), stops)
        }
    }

    /// What is painted outside the gradient.
    pub fn spread(mut self, spread: Spread) -> Self {
        self.spread = spread;
        self
    }

    /// The color at `t` (`0.0..=1.0`): the flat color of renderers without
    /// gradients. Between two plain colors it is their mix; next to an
    /// adaptive or [`Color::current`] stop, the nearer stop.
    pub fn color_at(&self, t: f32) -> Color {
        let Some(first) = self.stops.first() else {
            return Color::TRANSPARENT;
        };
        let mut before = *first;
        for s in &self.stops {
            if s.offset >= t {
                let span = s.offset - before.offset;
                let k = if span > 0. {
                    (t - before.offset) / span
                } else {
                    1.
                };
                let (a, b) = (before.color, s.color);
                if a.is_current() || b.is_current() || a.dark.is_some() || b.dark.is_some() {
                    return if k < 0.5 { a } else { b };
                }
                let mix = |x: f32, y: f32| x + (y - x) * k.clamp(0., 1.);
                return Color {
                    r: mix(a.r, b.r),
                    g: mix(a.g, b.g),
                    b: mix(a.b, b.b),
                    a: mix(a.a, b.a),
                    dark: None,
                };
            }
            before = *s;
        }
        before.color
    }

    fn map_colors(&mut self, f: &mut dyn FnMut(Color) -> Color) {
        for s in &mut self.stops {
            s.color = f(s.color);
        }
    }
}

/// Where a [`Text`] sits around its `x`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Anchor {
    /// It starts at `x` (the default).
    #[default]
    Start,
    /// It is centered on `x`.
    Middle,
    /// It ends at `x`.
    End,
}

/// Which line of a [`Text`]'s font sits on its `y`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Baseline {
    /// The baseline letters stand on (the default).
    #[default]
    Alphabetic,
    /// Half the x-height above the baseline (SVG `middle`).
    Middle,
    /// Halfway between the ascent and the descent (SVG `central`).
    Central,
    /// The top of the letters (SVG `hanging`, `text-before-edge`).
    Hanging,
}

/// One stretch of a [`Text`] in one font and color.
#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    /// The characters.
    pub text: String,
    /// The font family.
    pub family: FontFamily,
    /// The font size, in the text's own units.
    pub size: f32,
    /// The weight, 100 to 900 (400 regular, 700 bold).
    pub weight: u16,
    /// Italic.
    pub italic: bool,
    /// Space added after each character, in the text's own units.
    pub spacing: f32,
    /// The color (transparent: the run takes its room and draws nothing).
    pub fill: Color,
    /// Room left before the run, in the text's own units.
    pub dx: f32,
    /// How far below the text's `y` the run's baseline sits.
    pub dy: f32,
}

impl Default for TextRun {
    fn default() -> Self {
        TextRun {
            text: String::new(),
            family: FontFamily::System,
            size: 16.,
            weight: 400,
            italic: false,
            spacing: 0.,
            fill: Color::BLACK,
            dx: 0.,
            dy: 0.,
        }
    }
}

/// One line of text: runs laid out one after the other from `(x, y)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    /// Maps the text's own coordinates (`x`, `y`, sizes) to the drawing's
    /// (fractions of the frame in a [`Drawing`], points once resolved): it
    /// carries the text's rotation and scale.
    pub transform: Matrix,
    /// Where the line is anchored.
    pub x: f32,
    /// Where its baseline is.
    pub y: f32,
    /// Whether the line starts at, is centered on or ends at `x`.
    pub anchor: Anchor,
    /// Which line of the font sits on `y`.
    pub baseline: Baseline,
    /// The runs, in reading order.
    pub runs: Vec<TextRun>,
}

/// How an [`Image`] fills its rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Fit {
    /// Whole, as large as fits (letterboxed; the default).
    #[default]
    Meet,
    /// Covering the rectangle, cropped.
    Slice,
    /// Stretched to the rectangle.
    Stretch,
}

/// A bitmap in a drawing.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    /// Maps the image's own coordinates to the drawing's (fractions of the
    /// frame in a [`Drawing`], points once resolved).
    pub transform: Matrix,
    /// Its rectangle's left edge, in its own coordinates.
    pub x: f32,
    /// Its rectangle's top edge.
    pub y: f32,
    /// Its rectangle's width.
    pub w: f32,
    /// Its rectangle's height.
    pub h: f32,
    /// The bitmap: a `data:` URL (PNG, JPEG).
    pub src: String,
    /// How the bitmap fills the rectangle.
    pub fit: Fit,
    /// Which part stays when it is letterboxed or cropped: `(0, 0)` the top
    /// left, `(0.5, 0.5)` the center, `(1, 1)` the bottom right.
    pub align: (f32, f32),
    /// Opacity, `0.0..=1.0`.
    pub opacity: f32,
}

/// What a [`Group`] is drawn through: only the inside of this outline shows.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Clip {
    /// The outline, like a [`Path`]'s segments.
    pub segments: Vec<Segment>,
    /// The inside by the even-odd rule instead of non-zero winding.
    pub even_odd: bool,
}

/// What a [`Group`] is seen through: it shows where the mask is drawn.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Mask {
    /// The group shows as much as the mask is bright (white: fully, black
    /// or nothing: not at all), times its opacity. Otherwise only the
    /// mask's opacity counts.
    pub luminance: bool,
    /// What the mask draws.
    pub items: Vec<Item>,
}

/// A blurred, colored copy of a [`Group`] drawn under it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    /// How far right of the group, points.
    pub dx: f32,
    /// How far below the group, points.
    pub dy: f32,
    /// The blur's standard deviation, points (0: a sharp copy).
    pub blur: f32,
    /// Its color.
    pub color: Color,
}

/// Items drawn together, then shown as one picture: at an opacity, through
/// a clip or a mask, blurred or over shadows. The shadows and the blur come
/// first, then the clip, the mask and the opacity.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// Opacity of the whole group, `0.0..=1.0` (overlapping items do not
    /// show through each other).
    pub opacity: f32,
    /// Only the inside of this outline shows.
    pub clip: Option<Clip>,
    /// The group shows where this is drawn.
    pub mask: Option<Mask>,
    /// Shadows under the group, the first one at the bottom.
    pub shadows: Vec<Shadow>,
    /// A blur of the group itself: its standard deviation, points.
    pub blur: f32,
    /// What the group draws.
    pub items: Vec<Item>,
}

impl Default for Group {
    fn default() -> Self {
        Group {
            opacity: 1.,
            clip: None,
            mask: None,
            shadows: Vec::new(),
            blur: 0.,
            items: Vec::new(),
        }
    }
}

/// One thing a [`Drawing`] draws.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// A path.
    Path(Path),
    /// A line of text.
    Text(Text),
    /// A bitmap.
    Image(Image),
    /// Items drawn together.
    Group(Group),
}

/// What a shape draws.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Drawing {
    /// Points kept free on the left/right and top/bottom (so strokes on the
    /// edges are not clipped).
    pub inset: (f32, f32),
    /// The paths, painted in order (later ones on top).
    pub paths: Vec<Path>,
    /// The size (points) the stroke widths, dashes, shadows and blurs are
    /// given at: they grow and shrink with the frame, like the rest of the
    /// drawing (an SVG document: its own size). `None`: they are points
    /// whatever the frame's size.
    pub size: Option<(f32, f32)>,
    /// Text, images and groups (and paths among them), painted in order
    /// over [`paths`](Self::paths).
    pub items: Vec<Item>,
}

/// How many steps a fraction of the frame is written in: thousandths.
const STEPS: f32 = 1000.;

/// `v` to the nearest `1 / steps`.
fn num(o: &mut String, v: f32, steps: f32) {
    let v = (v * steps).round() / steps;
    let _ = write!(o, "{v} ");
}

/// A color's text form: `#RRGGBBAA`, `light/dark` for an adaptive color,
/// `~AA` for [`Color::current`] at an alpha.
fn color_text(c: Color) -> String {
    if c.is_current() {
        return format!("~{:02X}", (c.a.clamp(0., 1.) * 255.).round() as u8);
    }
    let mut o = c.light_variant().hex();
    if c.dark.is_some() {
        let _ = write!(o, "/{}", c.dark_variant().hex());
    }
    o
}

fn color(o: &mut String, tag: char, c: Color) {
    // `Color::current()`: `~` and its alpha (`~FF`), resolved by the
    // backend against the node's text color.
    let _ = write!(o, "{tag}{} ", color_text(c));
}

fn parse_hex(s: &str) -> Option<Color> {
    let s = s.strip_prefix('#')?;
    let v = u32::from_str_radix(s, 16).ok()?;
    let (rgb, a) = if s.len() == 8 {
        (v >> 8, (v & 0xff) as f32 / 255.)
    } else {
        (v, 1.)
    };
    Some(crate::style::rgba(rgb, a))
}

fn parse_color(s: &str) -> Option<Color> {
    if let Some(a) = s.strip_prefix('~') {
        let a = u8::from_str_radix(a, 16).map_or(1., |a| a as f32 / 255.);
        return Some(Color::current().with_alpha(a));
    }
    match s.split_once('/') {
        Some((l, d)) => Some(Color::adaptive(parse_hex(l)?, parse_hex(d)?)),
        None => parse_hex(s),
    }
}

/// Numbers joined by commas (a matrix, a rectangle), as short as they read
/// back exactly.
fn list(values: &[f32]) -> String {
    values
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_list(s: &str) -> Vec<f32> {
    s.split(',').filter_map(|v| v.parse().ok()).collect()
}

fn parse_matrix(s: &str) -> Matrix {
    <[f32; 6]>::try_from(parse_list(s)).unwrap_or(IDENTITY)
}

/// A gradient as one token (no spaces): `l` or `r` and its coordinates,
/// its transform (empty: none), its spread (`p`, `f`, `t`), its stops
/// (`offset,color` joined by `;`), fields joined by `:`.
fn gradient_text(g: &Gradient) -> String {
    let shape = match g.shape {
        GradientShape::Linear { x1, y1, x2, y2 } => format!("l{}", list(&[x1, y1, x2, y2])),
        GradientShape::Radial { cx, cy, r, fx, fy } => {
            format!("r{}", list(&[cx, cy, r, fx, fy]))
        }
    };
    let transform = if g.transform == IDENTITY {
        String::new()
    } else {
        list(&g.transform)
    };
    let spread = match g.spread {
        Spread::Pad => 'p',
        Spread::Reflect => 'f',
        Spread::Repeat => 't',
    };
    let stops: Vec<String> = g
        .stops
        .iter()
        .map(|s| format!("{},{}", s.offset, color_text(s.color)))
        .collect();
    format!("{shape}:{transform}:{spread}:{}", stops.join(";"))
}

fn parse_gradient(s: &str) -> Option<Gradient> {
    let mut fields = s.split(':');
    let shape = fields.next()?;
    let v = parse_list(shape.get(1..)?);
    let shape = match (shape.as_bytes().first()?, v.as_slice()) {
        (b'l', &[x1, y1, x2, y2]) => GradientShape::Linear { x1, y1, x2, y2 },
        (b'r', &[cx, cy, r, fx, fy]) => GradientShape::Radial { cx, cy, r, fx, fy },
        _ => return None,
    };
    let transform = match fields.next()? {
        "" => IDENTITY,
        m => parse_matrix(m),
    };
    let spread = match fields.next()? {
        "f" => Spread::Reflect,
        "t" => Spread::Repeat,
        _ => Spread::Pad,
    };
    let stops = fields
        .next()?
        .split(';')
        .filter_map(|s| {
            let (offset, color) = s.split_once(',')?;
            Some(Stop {
                offset: offset.parse().ok()?,
                color: parse_color(color)?,
            })
        })
        .collect();
    Some(Gradient {
        shape,
        stops,
        spread,
        transform,
    })
}

/// Separates the runs of a text line, and the fields of a run.
const RUN: char = '\u{1e}';
const FIELD: char = '\u{1f}';

fn family_text(f: &FontFamily) -> String {
    match f {
        FontFamily::System => "s".into(),
        FontFamily::Monospace => "m".into(),
        FontFamily::Serif => "r".into(),
        FontFamily::Rounded => "o".into(),
        FontFamily::Named(n) => format!("n{n}"),
    }
}

fn parse_family(s: &str) -> FontFamily {
    match s.split_at(s.len().min(1)) {
        ("m", _) => FontFamily::Monospace,
        ("r", _) => FontFamily::Serif,
        ("o", _) => FontFamily::Rounded,
        ("n", name) if !name.is_empty() => FontFamily::Named(name.to_string()),
        _ => FontFamily::System,
    }
}

/// Text as a drawing line holds it: on one line, without the separators.
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn segments_text(o: &mut String, segments: &[Segment], steps: f32) {
    for s in segments {
        match *s {
            Segment::Move(x, y) => {
                o.push_str("M ");
                num(o, x, steps);
                num(o, y, steps);
            }
            Segment::Line(x, y) => {
                o.push_str("L ");
                num(o, x, steps);
                num(o, y, steps);
            }
            Segment::Cubic(a, b, c, d, e, f) => {
                o.push_str("C ");
                for v in [a, b, c, d, e, f] {
                    num(o, v, steps);
                }
            }
            Segment::Arc {
                cx,
                cy,
                r,
                start,
                end,
            } => {
                o.push_str("A ");
                for v in [cx, cy, r, start, end] {
                    num(o, v, steps);
                }
            }
            Segment::Close => o.push_str("Z "),
        }
    }
}

fn parse_segments(text: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut tokens = text.split_whitespace();
    while let Some(cmd) = tokens.next() {
        let count = match cmd {
            "M" | "L" => 2,
            "C" => 6,
            "A" => 5,
            "Z" => 0,
            _ => continue,
        };
        let v: Vec<f32> = (0..count)
            .filter_map(|_| tokens.next()?.parse().ok())
            .collect();
        if v.len() != count {
            break;
        }
        segments.push(match cmd {
            "M" => Segment::Move(v[0], v[1]),
            "L" => Segment::Line(v[0], v[1]),
            "C" => Segment::Cubic(v[0], v[1], v[2], v[3], v[4], v[5]),
            "A" => Segment::Arc {
                cx: v[0],
                cy: v[1],
                r: v[2],
                start: v[3],
                end: v[4],
            },
            _ => Segment::Close,
        });
    }
    segments
}

fn parse_path(paint: &str, segs: &str) -> Path {
    let mut p = Path::new();
    for t in paint.split_whitespace() {
        let (tag, v) = t.split_at(1);
        match tag {
            "f" => p.fill = parse_color(v),
            "s" => p.stroke = parse_color(v),
            "F" => p.fill_gradient = parse_gradient(v),
            "S" => p.stroke_gradient = parse_gradient(v),
            "w" => p.width = v.parse().unwrap_or(1.),
            "r" => p.round = true,
            "e" => p.even_odd = true,
            "d" => {
                p.dash = v
                    .split_once(',')
                    .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
            }
            _ => {}
        }
    }
    p.segments = parse_segments(segs);
    p
}

fn parse_text(line: &str) -> Text {
    let (head, runs) = line.split_once(';').unwrap_or((line, ""));
    let mut t = Text {
        transform: IDENTITY,
        x: 0.,
        y: 0.,
        anchor: Anchor::Start,
        baseline: Baseline::Alphabetic,
        runs: Vec::new(),
    };
    for tok in head.split_whitespace() {
        let (tag, v) = tok.split_at(1);
        match tag {
            "m" => t.transform = parse_matrix(v),
            "p" => {
                if let [x, y] = parse_list(v)[..] {
                    (t.x, t.y) = (x, y);
                }
            }
            "a" => {
                t.anchor = match v {
                    "m" => Anchor::Middle,
                    "e" => Anchor::End,
                    _ => Anchor::Start,
                }
            }
            "b" => {
                t.baseline = match v {
                    "m" => Baseline::Middle,
                    "c" => Baseline::Central,
                    "h" => Baseline::Hanging,
                    _ => Baseline::Alphabetic,
                }
            }
            _ => {}
        }
    }
    for run in runs.split(RUN) {
        let mut fields = run.split(FIELD);
        let mut r = TextRun {
            text: fields.next().unwrap_or("").to_string(),
            ..TextRun::default()
        };
        for f in fields {
            let (tag, v) = f.split_at(f.len().min(1));
            match tag {
                "F" => r.family = parse_family(v),
                "z" => r.size = v.parse().unwrap_or(r.size),
                "w" => r.weight = v.parse().unwrap_or(r.weight),
                "i" => r.italic = true,
                "k" => r.spacing = v.parse().unwrap_or(0.),
                "c" => r.fill = parse_color(v).unwrap_or(r.fill),
                "x" => r.dx = v.parse().unwrap_or(0.),
                "y" => r.dy = v.parse().unwrap_or(0.),
                _ => {}
            }
        }
        if !r.text.is_empty() {
            t.runs.push(r);
        }
    }
    t
}

fn parse_image(line: &str) -> Image {
    let (head, src) = line.split_once(';').unwrap_or((line, ""));
    let mut i = Image {
        transform: IDENTITY,
        x: 0.,
        y: 0.,
        w: 0.,
        h: 0.,
        src: src.to_string(),
        fit: Fit::Meet,
        align: (0.5, 0.5),
        opacity: 1.,
    };
    for tok in head.split_whitespace() {
        let (tag, v) = tok.split_at(1);
        match tag {
            "m" => i.transform = parse_matrix(v),
            "r" => {
                if let [x, y, w, h] = parse_list(v)[..] {
                    (i.x, i.y, i.w, i.h) = (x, y, w, h);
                }
            }
            "f" => {
                i.fit = match v {
                    "s" => Fit::Slice,
                    "n" => Fit::Stretch,
                    _ => Fit::Meet,
                }
            }
            "a" => {
                if let [x, y] = parse_list(v)[..] {
                    i.align = (x, y);
                }
            }
            "o" => i.opacity = v.parse().unwrap_or(1.),
            _ => {}
        }
    }
    i
}

fn parse_group(line: &str) -> Group {
    let (head, clip) = match line.split_once(';') {
        Some((head, clip)) => (head, Some(clip)),
        None => (line, None),
    };
    let mut g = Group::default();
    let mut even_odd = false;
    for tok in head.split_whitespace() {
        let (tag, v) = tok.split_at(1);
        match tag {
            "o" => g.opacity = v.parse().unwrap_or(1.),
            "b" => g.blur = v.parse().unwrap_or(0.),
            "e" => even_odd = true,
            "h" => {
                let mut it = v.splitn(4, ',');
                let mut n = || it.next().and_then(|v| v.parse::<f32>().ok());
                if let (Some(dx), Some(dy), Some(blur)) = (n(), n(), n())
                    && let Some(color) = it.next().and_then(parse_color)
                {
                    g.shadows.push(Shadow {
                        dx,
                        dy,
                        blur,
                        color,
                    });
                }
            }
            _ => {}
        }
    }
    g.clip = clip.map(|c| Clip {
        segments: parse_segments(c),
        even_odd,
    });
    g
}

impl Item {
    /// One or more lines of [`Drawing::encode`].
    fn encode(&self, o: &mut String, steps: f32) {
        match self {
            Item::Path(p) => p.encode_in(o, steps),
            Item::Text(t) => {
                o.push_str("t ");
                if t.transform != IDENTITY {
                    let _ = write!(o, "m{} ", list(&t.transform));
                }
                let _ = write!(o, "p{}", list(&[t.x, t.y]));
                match t.anchor {
                    Anchor::Start => {}
                    Anchor::Middle => o.push_str(" am"),
                    Anchor::End => o.push_str(" ae"),
                }
                match t.baseline {
                    Baseline::Alphabetic => {}
                    Baseline::Middle => o.push_str(" bm"),
                    Baseline::Central => o.push_str(" bc"),
                    Baseline::Hanging => o.push_str(" bh"),
                }
                o.push(';');
                for (i, r) in t.runs.iter().enumerate() {
                    if i > 0 {
                        o.push(RUN);
                    }
                    o.push_str(&clean(&r.text));
                    let _ = write!(
                        o,
                        "{FIELD}F{}{FIELD}z{}{FIELD}c{}",
                        clean(&family_text(&r.family)),
                        r.size,
                        color_text(r.fill)
                    );
                    if r.weight != 400 {
                        let _ = write!(o, "{FIELD}w{}", r.weight);
                    }
                    if r.italic {
                        let _ = write!(o, "{FIELD}i");
                    }
                    for (tag, v) in [('k', r.spacing), ('x', r.dx), ('y', r.dy)] {
                        if v != 0. {
                            let _ = write!(o, "{FIELD}{tag}{v}");
                        }
                    }
                }
                o.push('\n');
            }
            Item::Image(i) => {
                o.push_str("p ");
                if i.transform != IDENTITY {
                    let _ = write!(o, "m{} ", list(&i.transform));
                }
                let _ = write!(o, "r{}", list(&[i.x, i.y, i.w, i.h]));
                match i.fit {
                    Fit::Meet => {}
                    Fit::Slice => o.push_str(" fs"),
                    Fit::Stretch => o.push_str(" fn"),
                }
                if i.align != (0.5, 0.5) {
                    let _ = write!(o, " a{}", list(&[i.align.0, i.align.1]));
                }
                if i.opacity != 1. {
                    let _ = write!(o, " o{}", i.opacity);
                }
                let _ = writeln!(o, ";{}", clean(&i.src));
            }
            Item::Group(g) => {
                o.push('g');
                if g.opacity != 1. {
                    let _ = write!(o, " o{}", g.opacity);
                }
                if g.blur != 0. {
                    let _ = write!(o, " b{}", g.blur);
                }
                for s in &g.shadows {
                    let _ = write!(o, " h{},{},{},{}", s.dx, s.dy, s.blur, color_text(s.color));
                }
                if let Some(c) = &g.clip {
                    if c.even_odd {
                        o.push_str(" e");
                    }
                    o.push(';');
                    segments_text(o, &c.segments, steps);
                }
                o.push('\n');
                if let Some(m) = &g.mask {
                    o.push_str(if m.luminance { "m l\n" } else { "m a\n" });
                    for i in &m.items {
                        i.encode(o, steps);
                    }
                    o.push_str("M\n");
                }
                for i in &g.items {
                    i.encode(o, steps);
                }
                o.push_str("G\n");
            }
        }
    }

    fn map_colors(&mut self, shadows: bool, f: &mut dyn FnMut(Color) -> Color) {
        match self {
            Item::Path(p) => p.map_colors(f),
            Item::Text(t) => {
                for r in &mut t.runs {
                    r.fill = f(r.fill);
                }
            }
            Item::Image(_) => {}
            Item::Group(g) => {
                if shadows {
                    for s in &mut g.shadows {
                        s.color = f(s.color);
                    }
                }
                // A mask's colors are how much shows, not what is seen.
                if shadows && let Some(m) = &mut g.mask {
                    for i in &mut m.items {
                        i.map_colors(shadows, f);
                    }
                }
                for i in &mut g.items {
                    i.map_colors(shadows, f);
                }
            }
        }
    }

    fn any_color(&self, f: &dyn Fn(Color) -> bool) -> bool {
        match self {
            Item::Path(p) => p.any_color(f),
            Item::Text(t) => t.runs.iter().any(|r| f(r.fill)),
            Item::Image(_) => false,
            Item::Group(g) => {
                g.shadows.iter().any(|s| f(s.color))
                    || g.mask
                        .as_ref()
                        .is_some_and(|m| m.items.iter().any(|i| i.any_color(f)))
                    || g.items.iter().any(|i| i.any_color(f))
            }
        }
    }

    /// Flips horizontally in a `width`-wide box (right-to-left layouts):
    /// paths and clips are mirrored; text and images move to the mirrored
    /// place and stay readable.
    pub fn mirror_x(&mut self, width: f32) {
        let flip = |about: f32| [-1., 0., 0., 1., about, 0.];
        match self {
            Item::Path(p) => p.mirror_x(width),
            Item::Text(t) => {
                t.transform = concat(flip(width), concat(t.transform, flip(2. * t.x)));
                t.anchor = match t.anchor {
                    Anchor::Start => Anchor::End,
                    Anchor::Middle => Anchor::Middle,
                    Anchor::End => Anchor::Start,
                };
            }
            Item::Image(i) => {
                i.transform = concat(flip(width), concat(i.transform, flip(2. * i.x + i.w)));
                i.align.0 = 1. - i.align.0;
            }
            Item::Group(g) => {
                if let Some(c) = &mut g.clip {
                    mirror_segments(&mut c.segments, width);
                }
                for s in &mut g.shadows {
                    s.dx = -s.dx;
                }
                if let Some(m) = &mut g.mask {
                    for i in &mut m.items {
                        i.mirror_x(width);
                    }
                }
                for i in &mut g.items {
                    i.mirror_x(width);
                }
            }
        }
    }
}

fn mirror_segments(segments: &mut [Segment], width: f32) {
    let m = |x: &mut f32| *x = width - *x;
    for s in segments {
        match s {
            Segment::Move(x, _) | Segment::Line(x, _) => m(x),
            Segment::Cubic(a, _, c, _, e, _) => {
                m(a);
                m(c);
                m(e);
            }
            Segment::Arc { cx, start, end, .. } => {
                m(cx);
                (*start, *end) = (-*start, -*end);
            }
            Segment::Close => {}
        }
    }
}

/// The paths among `items` as renderers without text, images and groups
/// draw them: each group's opacity in its paths' colors, clips, masks,
/// shadows, text and images left out.
pub fn flatten(items: &[Item]) -> Vec<Path> {
    fn walk(items: &[Item], alpha: f32, out: &mut Vec<Path>) {
        for i in items {
            match i {
                Item::Path(p) => {
                    let mut p = p.clone();
                    if alpha < 1. {
                        p.map_colors(&mut |c: Color| {
                            let a = c.a * alpha;
                            Color {
                                a,
                                dark: c.dark.map(|[r, g, b, da]| [r, g, b, da * alpha]),
                                ..c
                            }
                        });
                    }
                    out.push(p);
                }
                Item::Group(g) => walk(&g.items, alpha * g.opacity, out),
                Item::Text(_) | Item::Image(_) => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(items, 1., &mut out);
    out
}

/// A frame in points: fractions `(x, y)` are `(ix + x·iw, iy + y·ih)`.
#[derive(Clone, Copy)]
struct Frame {
    ix: f32,
    iy: f32,
    iw: f32,
    ih: f32,
    /// What stroke widths, dashes, shadows and blurs are multiplied by.
    scale: f32,
}

impl Frame {
    fn matrix(&self) -> Matrix {
        [self.iw, 0., 0., self.ih, self.ix, self.iy]
    }

    fn segments(&self, segments: &[Segment]) -> Vec<Segment> {
        let side = self.iw.min(self.ih);
        let pt = |x: f32, y: f32| (self.ix + x * self.iw, self.iy + y * self.ih);
        let mut segs = Vec::with_capacity(segments.len());
        let mut open = false;
        for s in segments {
            match *s {
                Segment::Move(x, y) => {
                    let (x, y) = pt(x, y);
                    segs.push(Segment::Move(x, y));
                    open = true;
                }
                Segment::Line(x, y) => {
                    let (x, y) = pt(x, y);
                    segs.push(if open {
                        Segment::Line(x, y)
                    } else {
                        Segment::Move(x, y)
                    });
                    open = true;
                }
                Segment::Cubic(a, b, c, d, e, f) => {
                    let ((a, b), (c, d), (e, f)) = (pt(a, b), pt(c, d), pt(e, f));
                    if !open {
                        segs.push(Segment::Move(a, b));
                    }
                    segs.push(Segment::Cubic(a, b, c, d, e, f));
                    open = true;
                }
                Segment::Arc {
                    cx,
                    cy,
                    r,
                    start,
                    end,
                } => {
                    let (cx, cy) = pt(cx, cy);
                    arc(&mut segs, open, cx, cy, r * side, start, end);
                    open = true;
                }
                Segment::Close => {
                    segs.push(Segment::Close);
                    open = false;
                }
            }
        }
        segs
    }

    fn gradient(&self, g: &Gradient) -> Gradient {
        Gradient {
            transform: concat(self.matrix(), g.transform),
            ..g.clone()
        }
    }

    fn path(&self, p: &Path) -> Path {
        Path {
            segments: self.segments(&p.segments),
            width: p.width * self.scale,
            dash: p.dash.map(|(a, b)| (a * self.scale, b * self.scale)),
            fill_gradient: p.fill_gradient.as_ref().map(|g| self.gradient(g)),
            stroke_gradient: p.stroke_gradient.as_ref().map(|g| self.gradient(g)),
            ..p.clone()
        }
    }

    fn item(&self, item: &Item) -> Item {
        match item {
            Item::Path(p) => Item::Path(self.path(p)),
            Item::Text(t) => {
                let mut t = t.clone();
                let m = concat(self.matrix(), t.transform);
                // Sizes in points, the transform left with the rotation: a
                // font is picked at the size it is drawn at.
                let k = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
                if k > 0. && k.is_finite() {
                    t.transform = [m[0] / k, m[1] / k, m[2] / k, m[3] / k, m[4], m[5]];
                    t.x *= k;
                    t.y *= k;
                    for r in &mut t.runs {
                        r.size *= k;
                        r.spacing *= k;
                        r.dx *= k;
                        r.dy *= k;
                    }
                } else {
                    t.transform = m;
                }
                Item::Text(t)
            }
            Item::Image(i) => Item::Image(Image {
                transform: concat(self.matrix(), i.transform),
                ..i.clone()
            }),
            Item::Group(g) => Item::Group(Group {
                opacity: g.opacity,
                clip: g.clip.as_ref().map(|c| Clip {
                    segments: self.segments(&c.segments),
                    even_odd: c.even_odd,
                }),
                mask: g.mask.as_ref().map(|m| Mask {
                    luminance: m.luminance,
                    items: m.items.iter().map(|i| self.item(i)).collect(),
                }),
                shadows: g
                    .shadows
                    .iter()
                    .map(|s| Shadow {
                        dx: s.dx * self.scale,
                        dy: s.dy * self.scale,
                        blur: s.blur * self.scale,
                        color: s.color,
                    })
                    .collect(),
                blur: g.blur * self.scale,
                items: g.items.iter().map(|i| self.item(i)).collect(),
            }),
        }
    }
}

impl Drawing {
    /// The text form backends receive: an `i` inset line, an `n` size line
    /// if any, then one line per path (paint tokens, `|`, SVG-like segments
    /// with `A cx cy r a0 a1`) and per text (`t`) and image (`p`); a group
    /// is a `g` line, its mask between `m` and `M`, its items, then `G`.
    pub fn encode(&self) -> String {
        let mut o = String::new();
        let _ = writeln!(o, "i{},{}", self.inset.0, self.inset.1);
        // A drawing with a size (an SVG document) may be shown large:
        // steps of a hundredth of a point at that size.
        let mut steps = STEPS;
        if let Some((w, h)) = self.size {
            let _ = writeln!(o, "n{w},{h}");
            let side = w.max(h);
            while steps < 1e6 && side * 100. > steps {
                steps *= 10.;
            }
        }
        for p in &self.paths {
            p.encode_in(&mut o, steps);
        }
        for i in &self.items {
            i.encode(&mut o, steps);
        }
        o
    }

    /// Reads [`encode`](Self::encode)'s output (unknown tokens and lines are
    /// skipped). A drawing of paths only has them in
    /// [`paths`](Self::paths); any other has everything in
    /// [`items`](Self::items).
    pub fn decode(text: &str) -> Drawing {
        let mut d = Drawing::default();
        // The groups being read, innermost last, each with the items read
        // before its mask started (`Some`: the mask is being read).
        let mut open: Vec<(Group, Option<Vec<Item>>)> = Vec::new();
        let mut top: Vec<Item> = Vec::new();
        for line in text.lines() {
            let item = match line.split_at(line.len().min(1)) {
                ("i", inset) => {
                    let mut it = inset.split(',').filter_map(|v| v.parse().ok());
                    d.inset = (it.next().unwrap_or(0.), it.next().unwrap_or(0.));
                    continue;
                }
                ("n", size) => {
                    if let [w, h] = parse_list(size)[..] {
                        d.size = Some((w, h));
                    }
                    continue;
                }
                ("g", rest) => {
                    open.push((parse_group(rest), None));
                    continue;
                }
                ("m", kind) => {
                    if let Some((g, before)) = open.last_mut() {
                        *before = Some(std::mem::take(&mut g.items));
                        g.mask = Some(Mask {
                            luminance: kind.trim() != "a",
                            items: Vec::new(),
                        });
                    }
                    continue;
                }
                ("M", _) => {
                    if let Some((g, before)) = open.last_mut()
                        && let Some(before) = before.take()
                        && let Some(m) = &mut g.mask
                    {
                        m.items = std::mem::replace(&mut g.items, before);
                    }
                    continue;
                }
                ("G", _) => match open.pop() {
                    Some((g, _)) => Item::Group(g),
                    None => continue,
                },
                ("t", rest) => Item::Text(parse_text(rest)),
                ("p", rest) => Item::Image(parse_image(rest)),
                _ => match line.split_once('|') {
                    Some((paint, segs)) => Item::Path(parse_path(paint, segs)),
                    None => continue,
                },
            };
            match open.last_mut() {
                Some((g, _)) => g.items.push(item),
                None => top.push(item),
            }
        }
        // A group left open (cut text): what it has is drawn.
        while let Some((g, _)) = open.pop() {
            match open.last_mut() {
                Some((parent, _)) => parent.items.push(Item::Group(g)),
                None => top.push(Item::Group(g)),
            }
        }
        if top.iter().all(|i| matches!(i, Item::Path(_))) {
            d.paths = top
                .into_iter()
                .filter_map(|i| match i {
                    Item::Path(p) => Some(p),
                    _ => None,
                })
                .collect();
        } else {
            d.items = top;
        }
        d
    }

    /// Everything the drawing draws, in order: its paths, then its items.
    pub fn all_items(&self) -> Vec<Item> {
        self.paths
            .iter()
            .cloned()
            .map(Item::Path)
            .chain(self.items.iter().cloned())
            .collect()
    }

    /// Whether the drawing has only paths (no text, image or group).
    pub fn is_flat(&self) -> bool {
        self.items.iter().all(|i| matches!(i, Item::Path(_)))
    }

    /// The drawing as paths only, as renderers without text, images and
    /// groups draw it (see [`flatten`]).
    pub fn flattened(&self) -> Drawing {
        Drawing {
            inset: self.inset,
            paths: flatten(&self.all_items()),
            size: self.size,
            items: Vec::new(),
        }
    }

    /// The drawing as the renderers of widgets and other surfaces draw it:
    /// [`flattened`](Self::flattened), each path in its flat color.
    pub fn plain(&self) -> Drawing {
        let mut d = self.flattened();
        for p in &mut d.paths {
            (p.fill_gradient, p.stroke_gradient) = (None, None);
        }
        d
    }

    fn map_colors(&self, shadows: bool, f: &mut dyn FnMut(Color) -> Color) -> Drawing {
        let mut d = self.clone();
        for p in &mut d.paths {
            p.map_colors(f);
        }
        for i in &mut d.items {
            i.map_colors(shadows, f);
        }
        d
    }

    /// The drawing with every [`Color::current`] resolved against `text`
    /// (the node's text color): what backends paint.
    pub fn with_current_color(&self, text: Color) -> Drawing {
        self.map_colors(true, &mut |c| c.resolve_current(text))
    }

    /// Whether something is painted in [`Color::current`].
    pub fn uses_current_color(&self) -> bool {
        self.paths.iter().any(|p| p.any_color(&Color::is_current))
            || self.items.iter().any(|i| i.any_color(&Color::is_current))
    }

    /// The drawing with every painted fill, stroke and text in `color`
    /// (each keeping its own opacity): a monochrome icon tinted. Images,
    /// shadows and masks stay as they are.
    pub fn tinted(&self, color: Color) -> Drawing {
        self.map_colors(false, &mut |c: Color| {
            let a = c.a;
            Color {
                a: color.a * a,
                dark: color.dark.map(|[r, g, b, da]| [r, g, b, da * a]),
                ..color
            }
        })
    }

    fn frame(&self, w: f32, h: f32, zoom: f32) -> Frame {
        let (ix, iy) = (self.inset.0 * zoom, self.inset.1 * zoom);
        let (iw, ih) = ((w - 2. * ix).max(0.), (h - 2. * iy).max(0.));
        let scale = match self.size {
            Some((sw, sh)) if sw > 0. && sh > 0. => (iw / sw).min(ih / sh),
            _ => zoom,
        };
        Frame {
            ix,
            iy,
            iw,
            ih,
            scale,
        }
    }

    /// The paths in points for a `w` × `h` frame, arcs turned into cubic
    /// curves (so a backend only needs move, line, curve and close): what a
    /// renderer without text, images and groups draws (see [`flatten`]).
    /// Stroke widths and dashes are multiplied by `zoom`, or follow the
    /// frame when the drawing has a [`size`](Self::size); insets are
    /// multiplied by `zoom`.
    pub fn resolve(&self, w: f32, h: f32, zoom: f32) -> Vec<Path> {
        if self.items.is_empty() {
            let f = self.frame(w, h, zoom);
            return self.paths.iter().map(|p| f.path(p)).collect();
        }
        flatten(&self.resolve_items(w, h, zoom))
    }

    /// Everything the drawing draws, in points for a `w` × `h` frame:
    /// paths as [`resolve`](Self::resolve) gives them, gradients, text and
    /// images with their transform to points (a text's sizes in points, its
    /// transform left with the rotation), clips, shadows and blurs in
    /// points.
    pub fn resolve_items(&self, w: f32, h: f32, zoom: f32) -> Vec<Item> {
        let f = self.frame(w, h, zoom);
        self.paths
            .iter()
            .map(|p| Item::Path(f.path(p)))
            .chain(self.items.iter().map(|i| f.item(i)))
            .collect()
    }
}

impl Path {
    /// One line of [`Drawing::encode`].
    pub fn encode(&self, o: &mut String) {
        self.encode_in(o, STEPS);
    }

    /// [`encode`](Self::encode), coordinates to the nearest `1 / steps`.
    fn encode_in(&self, o: &mut String, steps: f32) {
        let p = self;
        if let Some(c) = p.fill {
            color(o, 'f', c);
        }
        if let Some(g) = &p.fill_gradient {
            let _ = write!(o, "F{} ", gradient_text(g));
        }
        if let Some(c) = p.stroke {
            color(o, 's', c);
        }
        if let Some(g) = &p.stroke_gradient {
            let _ = write!(o, "S{} ", gradient_text(g));
        }
        if p.stroke.is_some() || p.stroke_gradient.is_some() {
            let _ = write!(o, "w{} ", p.width);
        }
        if p.round {
            o.push_str("r ");
        }
        if let Some((d, g)) = p.dash {
            let _ = write!(o, "d{d},{g} ");
        }
        if p.even_odd {
            o.push_str("e ");
        }
        o.push('|');
        segments_text(o, &p.segments, steps);
        o.push('\n');
    }

    /// Every color of the path through `f`: its fill, its stroke, its
    /// gradients' stops.
    pub(crate) fn map_colors(&mut self, f: &mut dyn FnMut(Color) -> Color) {
        self.fill = self.fill.map(&mut *f);
        self.stroke = self.stroke.map(&mut *f);
        if let Some(g) = &mut self.fill_gradient {
            g.map_colors(f);
        }
        if let Some(g) = &mut self.stroke_gradient {
            g.map_colors(f);
        }
    }

    fn any_color(&self, f: &dyn Fn(Color) -> bool) -> bool {
        let gradient = |g: &Option<Gradient>| {
            g.as_ref()
                .is_some_and(|g| g.stops.iter().any(|s| f(s.color)))
        };
        self.fill.is_some_and(f)
            || self.stroke.is_some_and(f)
            || gradient(&self.fill_gradient)
            || gradient(&self.stroke_gradient)
    }
}

/// Appends an arc as cubic Béziers (at most 90° each).
fn arc(segs: &mut Vec<Segment>, open: bool, cx: f32, cy: f32, r: f32, start: f32, end: f32) {
    // Clockwise from the top: (sin a, -cos a).
    let at = |a: f32| {
        let a = a.to_radians();
        (cx + r * a.sin(), cy - r * a.cos())
    };
    let (x, y) = at(start);
    segs.push(if open {
        Segment::Line(x, y)
    } else {
        Segment::Move(x, y)
    });
    let sweep = (end - start).clamp(-360., 360.);
    let n = (sweep.abs() / 90.).ceil().max(1.) as usize;
    let step = sweep / n as f32;
    let k = 4. / 3. * (step.to_radians() / 4.).tan() * r;
    for i in 0..n {
        let (a0, a1) = (start + step * i as f32, start + step * (i + 1) as f32);
        let ((x0, y0), (x1, y1)) = (at(a0), at(a1));
        let (r0, r1) = (a0.to_radians(), a1.to_radians());
        // Tangents of (sin, -cos): (cos, sin).
        segs.push(Segment::Cubic(
            x0 + k * r0.cos(),
            y0 + k * r0.sin(),
            x1 - k * r1.cos(),
            y1 - k * r1.sin(),
            x1,
            y1,
        ));
    }
}

/// "Nice" axis ticks covering `min..=max`: about `count` round values
/// (multiples of 1, 2 or 5 × 10ⁿ), first ≤ min, last ≥ max.
pub fn nice_ticks(min: f64, max: f64, count: usize) -> Vec<f64> {
    // Missing data (NaN, infinities): a unit range next to the finite bound.
    let (min, max) = match (min.is_finite(), max.is_finite()) {
        (true, true) => (min, max),
        (true, false) => (min, min + 1.),
        (false, true) => (max - 1., max),
        (false, false) => (0., 1.),
    };
    let (min, max) = if min == max {
        (min - 1., max + 1.)
    } else {
        (min.min(max), max.max(min))
    };
    let raw = (max - min) / count.max(1) as f64;
    // Too wide or too narrow for f64 steps: just the bounds.
    if !(raw.is_finite() && raw > 0.) {
        return vec![min, max];
    }
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1., 2., 2.5, 5., 10.]
        .iter()
        .map(|m| m * mag)
        .find(|s| *s >= raw)
        .unwrap_or(10. * mag);
    let first = (min / step).floor();
    // Rounded to the step's decimals: 0.30000000000000004 → 0.3.
    let scale = 10f64.powi(((-step.log10().floor()) as i32 + 1).max(0));
    let mut ticks = Vec::new();
    for i in 0.. {
        let v = ((first + i as f64) * step * scale).round() / scale;
        ticks.push(v);
        if v >= max - step * 1e-9 && ticks.len() >= 2 {
            break;
        }
    }
    ticks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{adaptive, rgb};

    #[test]
    fn encodes_and_decodes() {
        let d = Drawing {
            inset: (2., 3.5),
            paths: vec![
                Path::line([(0., 1.), (0.5, 0.25)])
                    .stroke(adaptive(rgb(0x112233), rgb(0xAABBCC)), 2.5)
                    .round()
                    .dashed(4., 2.),
                Path::sector(0.5, 0.5, 0.5, 0.3, 0., 90.).fill(rgb(0xFF0000)),
            ],
            ..Default::default()
        };
        assert_eq!(Drawing::decode(&d.encode()), d);
    }

    #[test]
    fn resolves_arcs_to_curves_in_points() {
        let d = Drawing {
            inset: (0., 0.),
            paths: vec![Path::arc(0.5, 0.5, 0.5, 0., 90.)],
            ..Default::default()
        };
        let r = d.resolve(100., 50., 2.);
        // Radius 0.5 of the smaller side (50): from the top (50, 0) to the right (75, 25).
        assert_eq!(r[0].segments[0], Segment::Move(50., 0.));
        let Segment::Cubic(.., x, y) = r[0].segments[1] else {
            panic!("curve");
        };
        assert!((x - 75.).abs() < 1e-3 && (y - 25.).abs() < 1e-3);
        assert_eq!(r[0].width, 2.);
    }

    #[test]
    fn smooth_curves_do_not_overshoot() {
        let p = Path::smooth([(0., 1.), (0.5, 0.), (1., 1.)]);
        for s in &p.segments {
            if let Segment::Cubic(_, y1, _, y2, _, _) = *s {
                assert!(y1 >= 0. && y2 >= 0.);
            }
        }
    }

    #[test]
    fn ticks_are_round() {
        assert_eq!(nice_ticks(3., 12., 4), vec![2.5, 5., 7.5, 10., 12.5]);
        assert_eq!(nice_ticks(0., 1., 5), vec![0., 0.2, 0.4, 0.6, 0.8, 1.]);
        assert_eq!(nice_ticks(5., 5., 4), vec![4., 4.5, 5., 5.5, 6.]);
    }

    fn rich() -> Drawing {
        let gradient = Gradient::linear(
            (0., 0.),
            (1., 0.),
            [
                (0., rgb(0xFF0000)),
                (1., adaptive(rgb(0x00FF00), rgb(0x0000FF))),
            ],
        )
        .spread(Spread::Reflect);
        Drawing {
            inset: (1., 1.),
            paths: vec![Path::rect(0., 0., 1., 1.).fill_gradient(gradient.clone())],
            size: Some((200., 100.)),
            items: vec![
                Item::Group(Group {
                    opacity: 0.5,
                    clip: Some(Clip {
                        segments: Path::circle(0.5, 0.5, 0.5).segments,
                        even_odd: true,
                    }),
                    mask: Some(Mask {
                        luminance: false,
                        items: vec![Item::Path(Path::rect(0., 0., 0.5, 1.).fill(rgb(0xFFFFFF)))],
                    }),
                    shadows: vec![Shadow {
                        dx: 1.,
                        dy: 2.,
                        blur: 3.,
                        color: rgb(0x112233),
                    }],
                    blur: 0.5,
                    items: vec![
                        Item::Path(
                            Path::line([(0., 0.), (1., 1.)])
                                .stroke_gradient(
                                    Gradient::radial(
                                        (0.5, 0.5),
                                        0.5,
                                        [(0., rgb(0)), (1., rgb(0x666666))],
                                    ),
                                    2.,
                                )
                                .fill(Color::current().with_alpha(0.2)),
                        ),
                        Item::Group(Group::default()),
                    ],
                }),
                Item::Text(Text {
                    transform: [0.005, 0., 0., 0.01, 0., 0.],
                    x: 100.,
                    y: 50.,
                    anchor: Anchor::Middle,
                    baseline: Baseline::Central,
                    runs: vec![
                        TextRun {
                            text: "a|b; c".into(),
                            family: FontFamily::Named("Inter".into()),
                            size: 20.,
                            weight: 700,
                            italic: true,
                            spacing: 1.5,
                            fill: Color::current(),
                            dx: 2.,
                            dy: -3.,
                        },
                        TextRun {
                            text: "plain".into(),
                            ..TextRun::default()
                        },
                    ],
                }),
                Item::Image(Image {
                    transform: [0.005, 0., 0., 0.01, 0., 0.],
                    x: 10.,
                    y: 20.,
                    w: 30.,
                    h: 40.,
                    src: "data:image/png;base64,AAAA".into(),
                    fit: Fit::Slice,
                    align: (0., 1.),
                    opacity: 0.25,
                }),
            ],
        }
    }

    #[test]
    fn text_images_and_groups_encode_and_decode() {
        let d = rich();
        let back = Drawing::decode(&d.encode());
        // Everything is in `items` once there is more than paths.
        assert!(back.paths.is_empty());
        assert_eq!(back.items, d.all_items());
        assert_eq!((back.inset, back.size), (d.inset, d.size));
        // Older readers skip what they do not know: the lines of paths
        // have a `|`, the others never start like one.
        for line in d.encode().lines() {
            let known = line.starts_with(['i', 'n', 'g', 'G', 'm', 'M', 't', 'p']);
            assert!(
                known != (line.contains('|') && !line.starts_with(['t', 'p'])),
                "{line}"
            );
        }
        // Only paths: they stay in `paths`.
        let flat = d.flattened();
        let back = Drawing::decode(&flat.encode());
        assert!(back.items.is_empty());
        assert_eq!(back.encode(), flat.encode());
    }

    #[test]
    fn flattening_keeps_the_paths_with_their_group_s_opacity() {
        let d = rich();
        let flat = d.flattened();
        assert!(flat.items.is_empty() && flat.is_flat());
        // The path, then the group's (its mask's is not drawn).
        assert_eq!(flat.paths.len(), 2);
        assert_eq!(flat.paths[0], d.paths[0]);
        let line = &flat.paths[1];
        assert!(line.fill.unwrap().is_current());
        assert_eq!(line.fill.unwrap().a, 0.1);
        assert_eq!(line.stroke_gradient.as_ref().unwrap().stops[1].color.a, 0.5);
        // For surfaces: flat colors only.
        let plain = d.plain();
        assert!(
            plain
                .paths
                .iter()
                .all(|p| p.fill_gradient.is_none() && p.stroke_gradient.is_none())
        );
        assert_eq!(
            plain.paths[0].fill,
            Some(d.paths[0].fill_gradient.as_ref().unwrap().color_at(0.5))
        );
        // Colors: the current one resolved everywhere, a tint on what is
        // painted but not on shadows.
        assert!(d.uses_current_color());
        let ink = rgb(0x334455);
        let r = d.with_current_color(ink);
        assert!(!r.uses_current_color());
        let Item::Text(t) = &r.items[1] else { panic!() };
        assert_eq!(t.runs[0].fill, ink);
        let tinted = d.tinted(ink);
        let Item::Group(g) = &tinted.items[0] else {
            panic!()
        };
        assert_eq!(g.shadows[0].color, rgb(0x112233));
        assert_eq!(
            tinted.paths[0].fill_gradient.as_ref().unwrap().stops[0].color,
            ink
        );
    }

    #[test]
    fn a_sized_drawing_scales_with_its_frame() {
        let d = rich();
        // Twice its size (the insets stay points): strokes, shadows and
        // blurs are twice as large.
        let items = d.resolve_items(402., 202., 1.);
        let Item::Path(p) = &items[0] else { panic!() };
        assert_eq!(p.segments[0], Segment::Move(1., 1.));
        assert_eq!(p.segments[2], Segment::Line(401., 201.));
        // A gradient in fractions of the frame: its transform to points.
        assert_eq!(
            p.fill_gradient.as_ref().unwrap().transform,
            [400., 0., 0., 200., 1., 1.]
        );
        let Item::Group(g) = &items[1] else { panic!() };
        assert_eq!((g.blur, g.shadows[0].dy, g.shadows[0].blur), (1., 4., 6.));
        assert!(matches!(
            g.clip.as_ref().unwrap().segments[0],
            Segment::Move(..)
        ));
        let Item::Path(line) = &g.items[0] else {
            panic!()
        };
        assert_eq!(line.width, 4.);
        // Text: sizes in points, the transform left with what is not a
        // scale (here nothing but the inset).
        let Item::Text(t) = &items[2] else { panic!() };
        assert_eq!(t.transform, [1., 0., 0., 1., 1., 1.]);
        assert_eq!((t.x, t.y), (200., 100.));
        assert_eq!(
            (t.runs[0].size, t.runs[0].spacing, t.runs[0].dy),
            (40., 3., -6.)
        );
        let Item::Image(i) = &items[3] else { panic!() };
        assert_eq!(i.transform, [2., 0., 0., 2., 1., 1.]);
        // Without a size, strokes are points whatever the frame.
        let plain = Drawing {
            paths: vec![Path::line([(0., 0.), (1., 1.)]).stroke(rgb(0), 2.)],
            ..Default::default()
        };
        assert_eq!(plain.resolve(400., 200., 1.)[0].width, 2.);
        // What a renderer of paths gets: the same, flattened.
        assert_eq!(d.resolve(402., 202., 1.).len(), 2);
    }

    #[test]
    fn mirroring_moves_text_and_images_and_keeps_them_readable() {
        let d = rich();
        let mut items = d.resolve_items(402., 202., 1.);
        for i in &mut items {
            i.mirror_x(402.);
        }
        let at =
            |m: Matrix, x: f32, y: f32| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
        // The text's anchor is at the mirrored place, and a point after it
        // is still to its right.
        let Item::Text(t) = &items[2] else { panic!() };
        let anchor = at(t.transform, t.x, t.y);
        let after = at(t.transform, t.x + 10., t.y);
        assert!((anchor.0 - 201.).abs() < 0.1, "{anchor:?}");
        assert!(after.0 > anchor.0);
        assert_eq!(t.anchor, Anchor::Middle);
        // The image's rectangle is mirrored, its picture is not.
        let Item::Image(i) = &items[3] else { panic!() };
        let (left, right) = (at(i.transform, i.x, i.y), at(i.transform, i.x + i.w, i.y));
        assert!(
            (left.0 - 321.).abs() < 0.1 && right.0 > left.0,
            "{left:?} {right:?}"
        );
        assert_eq!(i.align, (1., 1.));
        let Item::Group(g) = &items[1] else { panic!() };
        assert_eq!(g.shadows[0].dx, -2.);
    }

    #[test]
    fn a_gradient_s_flat_color_is_its_middle() {
        let g = Gradient::linear(
            (0., 0.),
            (1., 0.),
            [(0., rgb(0x000000)), (1., rgb(0xFFFFFF))],
        );
        let c = g.color_at(0.5);
        assert!((c.r - 0.5).abs() < 1e-6 && c.a == 1.);
        assert_eq!(g.color_at(2.), rgb(0xFFFFFF));
        assert_eq!(
            Gradient::linear((0., 0.), (1., 0.), []).color_at(0.5),
            Color::TRANSPARENT
        );
    }
}
