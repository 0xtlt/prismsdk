//! Vector drawing: lines, curves, arcs, filled or stroked, drawn natively
//! (UIKit/AppKit Bézier paths, Android `Canvas`, cairo, SVG). Charts are
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

use crate::style::Color;

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
        let m = |x: &mut f32| *x = width - *x;
        for s in &mut self.segments {
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
}

/// What a shape draws.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Drawing {
    /// Points kept free on the left/right and top/bottom (so strokes on the
    /// edges are not clipped).
    pub inset: (f32, f32),
    /// The paths, painted in order (later ones on top).
    pub paths: Vec<Path>,
}

fn num(o: &mut String, v: f32) {
    let v = (v * 1000.).round() / 1000.;
    let _ = write!(o, "{v} ");
}

fn color(o: &mut String, tag: char, c: Color) {
    let _ = write!(o, "{tag}{}", c.light_variant().hex());
    if c.dark.is_some() {
        let _ = write!(o, "/{}", c.dark_variant().hex());
    }
    o.push(' ');
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
    match s.split_once('/') {
        Some((l, d)) => Some(Color::adaptive(parse_hex(l)?, parse_hex(d)?)),
        None => parse_hex(s),
    }
}

impl Drawing {
    /// The text form backends receive (`i` inset line, then one line per
    /// path: paint tokens, `|`, SVG-like segments with `A cx cy r a0 a1`).
    pub fn encode(&self) -> String {
        let mut o = String::new();
        let _ = writeln!(o, "i{},{}", self.inset.0, self.inset.1);
        for p in &self.paths {
            p.encode(&mut o);
        }
        o
    }

    /// Reads [`encode`](Self::encode)'s output (unknown tokens are skipped).
    pub fn decode(text: &str) -> Drawing {
        let mut d = Drawing::default();
        for line in text.lines() {
            if let Some(inset) = line.strip_prefix('i') {
                let mut it = inset.split(',').filter_map(|v| v.parse().ok());
                d.inset = (it.next().unwrap_or(0.), it.next().unwrap_or(0.));
                continue;
            }
            let Some((paint, segs)) = line.split_once('|') else {
                continue;
            };
            let mut p = Path::new();
            for t in paint.split_whitespace() {
                let (tag, v) = t.split_at(1);
                match tag {
                    "f" => p.fill = parse_color(v),
                    "s" => p.stroke = parse_color(v),
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
            let mut tokens = segs.split_whitespace().peekable();
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
                p.segments.push(match cmd {
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
            d.paths.push(p);
        }
        d
    }

    /// The paths in points for a `w` × `h` frame, arcs turned into cubic
    /// curves (so a backend only needs move, line, curve and close).
    /// Stroke widths, dashes and insets are multiplied by `zoom`.
    pub fn resolve(&self, w: f32, h: f32, zoom: f32) -> Vec<Path> {
        let (ix, iy) = (self.inset.0 * zoom, self.inset.1 * zoom);
        let (iw, ih) = ((w - 2. * ix).max(0.), (h - 2. * iy).max(0.));
        let side = iw.min(ih);
        let pt = |x: f32, y: f32| (ix + x * iw, iy + y * ih);
        self.paths
            .iter()
            .map(|p| {
                let mut segs = Vec::with_capacity(p.segments.len());
                let mut open = false;
                for s in &p.segments {
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
                Path {
                    segments: segs,
                    width: p.width * zoom,
                    dash: p.dash.map(|(a, b)| (a * zoom, b * zoom)),
                    ..p.clone()
                }
            })
            .collect()
    }
}

impl Path {
    /// One line of [`Drawing::encode`].
    pub fn encode(&self, o: &mut String) {
        let p = self;
        if let Some(c) = p.fill {
            color(o, 'f', c);
        }
        if let Some(c) = p.stroke {
            color(o, 's', c);
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
        for s in &p.segments {
            match *s {
                Segment::Move(x, y) => {
                    o.push_str("M ");
                    num(o, x);
                    num(o, y);
                }
                Segment::Line(x, y) => {
                    o.push_str("L ");
                    num(o, x);
                    num(o, y);
                }
                Segment::Cubic(a, b, c, d, e, f) => {
                    o.push_str("C ");
                    for v in [a, b, c, d, e, f] {
                        num(o, v);
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
                        num(o, v);
                    }
                }
                Segment::Close => o.push_str("Z "),
            }
        }
        o.push('\n');
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
        };
        assert_eq!(Drawing::decode(&d.encode()), d);
    }

    #[test]
    fn resolves_arcs_to_curves_in_points() {
        let d = Drawing {
            inset: (0., 0.),
            paths: vec![Path::arc(0.5, 0.5, 0.5, 0., 90.)],
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
}
