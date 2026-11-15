//! SVG markup turned into a [`Drawing`], so an SVG made by a server (a QR
//! code, an icon, a chart) is drawn natively on every backend, widgets
//! included, by the same vector machinery as [`shape`](crate::shape()).
//!
//! Supported: `svg` (`viewBox`, `width`, `height`), `g`, `a`, `path` (every
//! command, arcs included), `rect` (with `rx`/`ry`), `circle`, `ellipse`,
//! `line`, `polyline`, `polygon`; `transform` (`matrix`, `translate`,
//! `scale`, `rotate`, `skewX`, `skewY`); `fill`, `stroke`, `stroke-width`,
//! `opacity`, `fill-opacity`, `stroke-opacity`, `stroke-linecap` /
//! `stroke-linejoin` (round or not), `stroke-dasharray` (its first dash and
//! gap), `fill-rule`, `display` / `visibility`, as attributes or in `style`.
//! Colors: hex, `rgb()` / `rgba()`, common names, `currentColor` (black,
//! or the `color` attribute). Skipped: text, gradients and patterns (their
//! shapes are drawn in the fallback color, or not at all), filters, masks,
//! clip paths, `use`, CSS style sheets.

use crate::shape::{Drawing, Path, Segment};
use crate::style::{Color, rgba};

/// A parsed SVG: its own size in points and what it draws.
#[derive(Clone, Debug, PartialEq)]
pub struct Svg {
    /// `width` (or the `viewBox` width), points.
    pub width: f32,
    /// `height` (or the `viewBox` height), points.
    pub height: f32,
    /// The paths, in fractions of the `viewBox`; stroke widths in points
    /// at `width` × `height`.
    pub drawing: Drawing,
}

/// Reads SVG markup. `None` when there is no `<svg>` element.
pub fn parse(markup: &str) -> Option<Svg> {
    let mut parser = Parser {
        src: markup,
        pos: 0,
    };
    let mut stack: Vec<State> = Vec::new();
    let mut skip_depth = 0usize;
    let mut out: Vec<(Vec<Seg>, State)> = Vec::new();
    let mut root: Option<(f64, f64, f64, f64, f32, f32)> = None;
    while let Some(tag) = parser.next_tag() {
        match tag {
            Tag::Close => {
                if skip_depth > 0 {
                    skip_depth -= 1;
                } else {
                    stack.pop();
                }
            }
            Tag::Open {
                name,
                attrs,
                self_closing,
            } => {
                if skip_depth > 0 {
                    if !self_closing {
                        skip_depth += 1;
                    }
                    continue;
                }
                let name = name.rsplit(':').next().unwrap_or(name);
                if SKIPPED.contains(&name) {
                    if !self_closing {
                        skip_depth = 1;
                    }
                    continue;
                }
                let parent = stack.last().cloned().unwrap_or_default();
                let mut state = parent.inherit(&attrs);
                if name == "svg" && root.is_none() {
                    root = Some(viewport(&attrs));
                    // The root's own x / y do not move its content.
                    state.transform = IDENTITY;
                    state.transform = multiply(state.transform, own_transform(&attrs));
                }
                if let Some(segs) = geometry(name, &attrs)
                    && state.visible()
                {
                    out.push((transformed(&segs, state.transform), state.clone()));
                }
                if !self_closing {
                    stack.push(state);
                }
            }
        }
    }
    let (vx, vy, vw, vh, width, height) = root?;
    let (vw, vh) = (vw.max(1e-9), vh.max(1e-9));
    // Points per user unit (the drawing is shown at width × height).
    let unit = (width as f64 / vw).min(height as f64 / vh);
    let frac = |x: f64, y: f64| (((x - vx) / vw) as f32, ((y - vy) / vh) as f32);
    let paths = out
        .into_iter()
        .filter_map(|(segs, st)| {
            let mut p = Path::new();
            for s in &segs {
                p.segments.push(match *s {
                    Seg::Move(x, y) => {
                        let (x, y) = frac(x, y);
                        Segment::Move(x, y)
                    }
                    Seg::Line(x, y) => {
                        let (x, y) = frac(x, y);
                        Segment::Line(x, y)
                    }
                    Seg::Cubic(a, b, c, d, e, f) => {
                        let ((a, b), (c, d), (e, f)) = (frac(a, b), frac(c, d), frac(e, f));
                        Segment::Cubic(a, b, c, d, e, f)
                    }
                    Seg::Close => Segment::Close,
                });
            }
            let scale = st.transform_scale() * unit;
            p.fill = st.fill_color();
            p.even_odd = st.even_odd;
            if let Some(c) = st.stroke_color() {
                p.stroke = Some(c);
                p.width = (st.stroke_width * scale) as f32;
            }
            p.round = st.round;
            p.dash = st
                .dash
                .map(|(d, g)| ((d * scale) as f32, (g * scale) as f32));
            (p.fill.is_some() || p.stroke.is_some()).then_some(p)
        })
        .collect();
    Some(Svg {
        width,
        height,
        drawing: Drawing {
            inset: (0., 0.),
            paths,
        },
    })
}

/// Elements whose content is never drawn directly.
const SKIPPED: &[&str] = &[
    "defs",
    "clipPath",
    "mask",
    "pattern",
    "symbol",
    "marker",
    "style",
    "script",
    "title",
    "desc",
    "metadata",
    "linearGradient",
    "radialGradient",
    "filter",
    "text",
    "foreignObject",
];

// --- Markup ---------------------------------------------------------------------

enum Tag<'a> {
    Open {
        name: &'a str,
        attrs: Vec<(&'a str, String)>,
        self_closing: bool,
    },
    Close,
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    /// The next element tag (comments, declarations, processing
    /// instructions and text skipped).
    fn next_tag(&mut self) -> Option<Tag<'a>> {
        loop {
            let rest = &self.src[self.pos..];
            let start = rest.find('<')?;
            self.pos += start;
            let rest = &self.src[self.pos..];
            for (open, close) in [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")] {
                if rest.starts_with(open) {
                    self.pos += rest.find(close).map_or(rest.len(), |e| e + close.len());
                    break;
                }
            }
            let rest = &self.src[self.pos..];
            if !rest.starts_with('<') {
                continue;
            }
            if rest.starts_with("<!") {
                self.pos += rest.find('>').map_or(rest.len(), |e| e + 1);
                continue;
            }
            if rest.starts_with("</") {
                self.pos += rest.find('>').map_or(rest.len(), |e| e + 1);
                return Some(Tag::Close);
            }
            return self.open_tag();
        }
    }

    fn open_tag(&mut self) -> Option<Tag<'a>> {
        let src = self.src;
        let bytes = src.as_bytes();
        let mut i = self.pos + 1;
        let name_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !b"/>".contains(&bytes[i]) {
            i += 1;
        }
        let name = &src[name_start..i];
        let mut attrs = Vec::new();
        loop {
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= bytes.len() {
                self.pos = i;
                return None;
            }
            match bytes[i] {
                b'>' => {
                    self.pos = i + 1;
                    return Some(Tag::Open {
                        name,
                        attrs,
                        self_closing: false,
                    });
                }
                b'/' => {
                    let end = src[i..].find('>').map_or(bytes.len(), |e| i + e + 1);
                    self.pos = end;
                    return Some(Tag::Open {
                        name,
                        attrs,
                        self_closing: true,
                    });
                }
                _ => {}
            }
            let key_start = i;
            while i < bytes.len() && !b"= \t\r\n/>".contains(&bytes[i]) {
                i += 1;
            }
            let key = &src[key_start..i];
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'=' {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                    let quote = bytes[i] as char;
                    let end = src[i + 1..].find(quote).map_or(bytes.len(), |e| i + 1 + e);
                    attrs.push((key, unescape(&src[i + 1..end.min(bytes.len())])));
                    i = (end + 1).min(bytes.len());
                } else {
                    let start = i;
                    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                        i += 1;
                    }
                    attrs.push((key, unescape(&src[start..i])));
                }
            } else if key.is_empty() {
                i += 1;
            }
        }
    }
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn attr<'b>(attrs: &'b [(&str, String)], key: &str) -> Option<&'b str> {
    attrs
        .iter()
        .rev()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.as_str())
}

/// A length in user units (`px`, `pt` and unitless; percentages and
/// font-relative units are not resolved).
fn length(s: &str) -> Option<f64> {
    let s = s.trim();
    let s = s.strip_suffix("px").unwrap_or(s);
    if let Some(pt) = s.strip_suffix("pt") {
        return pt.trim().parse::<f64>().ok().map(|v| v * 4. / 3.);
    }
    s.trim().parse().ok()
}

fn number_list(s: &str) -> Vec<f64> {
    let mut n = Numbers::new(s);
    std::iter::from_fn(|| n.number()).collect()
}

/// `(vx, vy, vw, vh, width, height)` of the root `<svg>`.
fn viewport(attrs: &[(&str, String)]) -> (f64, f64, f64, f64, f32, f32) {
    let vb = attr(attrs, "viewBox")
        .map(number_list)
        .filter(|v| v.len() == 4);
    let w = attr(attrs, "width").and_then(length);
    let h = attr(attrs, "height").and_then(length);
    let (vx, vy, vw, vh) = match &vb {
        Some(v) => (v[0], v[1], v[2], v[3]),
        None => (0., 0., w.unwrap_or(100.), h.unwrap_or(100.)),
    };
    // A missing side follows the viewBox's proportions.
    let (w, h) = match (w, h) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, w * vh / vw.max(1e-9)),
        (None, Some(h)) => (h * vw / vh.max(1e-9), h),
        (None, None) => (vw, vh),
    };
    (vx, vy, vw, vh, w as f32, h as f32)
}

// --- Paint ----------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Paint {
    None,
    Color(Color),
    Current,
}

#[derive(Clone, Debug)]
struct State {
    fill: Paint,
    stroke: Paint,
    stroke_width: f64,
    fill_opacity: f64,
    stroke_opacity: f64,
    opacity: f64,
    round: bool,
    even_odd: bool,
    dash: Option<(f64, f64)>,
    color: Color,
    hidden: bool,
    transform: [f64; 6],
}

impl Default for State {
    fn default() -> Self {
        State {
            fill: Paint::Color(Color::BLACK),
            stroke: Paint::None,
            stroke_width: 1.,
            fill_opacity: 1.,
            stroke_opacity: 1.,
            opacity: 1.,
            round: false,
            even_odd: false,
            dash: None,
            color: Color::BLACK,
            hidden: false,
            transform: IDENTITY,
        }
    }
}

impl State {
    /// This element's state: the parent's, with its own attributes and
    /// `style` declarations (which win) applied.
    fn inherit(&self, attrs: &[(&str, String)]) -> State {
        let mut s = self.clone();
        // `display` and `opacity` do not inherit as such: opacity multiplies.
        let mut props: Vec<(String, String)> = attrs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        if let Some(style) = attr(attrs, "style") {
            for decl in style.split(';') {
                if let Some((k, v)) = decl.split_once(':') {
                    props.push((k.trim().to_string(), v.trim().to_string()));
                }
            }
        }
        // `color` first: `currentColor` in this element refers to it.
        if let Some((_, v)) = props.iter().rev().find(|(k, _)| k == "color")
            && let Some(Paint::Color(c)) = paint(v, self.color)
        {
            s.color = c;
        }
        for (k, v) in &props {
            let v = v.trim();
            match k.as_str() {
                "fill" => s.fill = paint(v, s.color).unwrap_or(s.fill),
                "stroke" => s.stroke = paint(v, s.color).unwrap_or(s.stroke),
                "stroke-width" => s.stroke_width = length(v).unwrap_or(s.stroke_width),
                "fill-opacity" => s.fill_opacity = v.parse().unwrap_or(s.fill_opacity),
                "stroke-opacity" => s.stroke_opacity = v.parse().unwrap_or(s.stroke_opacity),
                "opacity" => s.opacity = self.opacity * v.parse::<f64>().unwrap_or(1.),
                "stroke-linecap" | "stroke-linejoin" => s.round = v == "round",
                "fill-rule" => s.even_odd = v == "evenodd",
                "stroke-dasharray" => {
                    let d = number_list(v);
                    s.dash = match d.as_slice() {
                        [] => None,
                        [a] => Some((*a, *a)),
                        [a, b, ..] => Some((*a, *b)),
                    }
                }
                "display" => s.hidden |= v == "none",
                "visibility" => s.hidden = v == "hidden" || v == "collapse",
                _ => {}
            }
        }
        s.transform = multiply(self.transform, own_transform(attrs));
        s
    }

    fn visible(&self) -> bool {
        !self.hidden && self.opacity > 0.
    }

    fn resolve(&self, p: Paint, alpha: f64) -> Option<Color> {
        let c = match p {
            Paint::None => return None,
            Paint::Color(c) => c,
            Paint::Current => self.color,
        };
        let a = (c.a as f64 * alpha * self.opacity) as f32;
        (a > 0.).then_some(Color { a, ..c })
    }

    fn fill_color(&self) -> Option<Color> {
        self.resolve(self.fill, self.fill_opacity)
    }

    fn stroke_color(&self) -> Option<Color> {
        if self.stroke_width <= 0. {
            return None;
        }
        self.resolve(self.stroke, self.stroke_opacity)
    }

    /// How much the transform scales lengths (the square root of its area
    /// factor).
    fn transform_scale(&self) -> f64 {
        let [a, b, c, d, _, _] = self.transform;
        (a * d - b * c).abs().sqrt()
    }
}

/// A paint value; `None` for something unreadable (keeps the inherited one).
fn paint(v: &str, current: Color) -> Option<Paint> {
    let v = v.trim();
    if v == "none" || v == "transparent" {
        return Some(Paint::None);
    }
    if v.eq_ignore_ascii_case("currentColor") {
        return Some(Paint::Color(current)).map(|_| Paint::Current);
    }
    // `url(#gradient) red`: the fallback color.
    let v = match v.strip_prefix("url(") {
        Some(rest) => rest.split_once(')').map_or("", |(_, f)| f.trim()),
        None => v,
    };
    if v.is_empty() {
        return Some(Paint::None);
    }
    color(v).map(Paint::Color)
}

fn color(v: &str) -> Option<Color> {
    let v = v.trim().to_ascii_lowercase();
    if let Some(hex) = v.strip_prefix('#') {
        let n = u32::from_str_radix(hex, 16).ok()?;
        let d = |x: u32| (x << 4) | x;
        return Some(match hex.len() {
            3 => rgba(
                (d(n >> 8 & 0xf) << 16) | (d(n >> 4 & 0xf) << 8) | d(n & 0xf),
                1.,
            ),
            4 => rgba(
                (d(n >> 12 & 0xf) << 16) | (d(n >> 8 & 0xf) << 8) | d(n >> 4 & 0xf),
                d(n & 0xf) as f32 / 255.,
            ),
            6 => rgba(n, 1.),
            8 => rgba(n >> 8, (n & 0xff) as f32 / 255.),
            _ => return None,
        });
    }
    if let Some(args) = v
        .strip_prefix("rgba(")
        .or_else(|| v.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'))
    {
        let parts: Vec<&str> = args
            .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .collect();
        let channel = |p: &str| -> Option<u32> {
            Some(
                match p.strip_suffix('%') {
                    Some(pct) => (pct.parse::<f64>().ok()? * 2.55).round(),
                    None => p.parse::<f64>().ok()?.round(),
                }
                .clamp(0., 255.) as u32,
            )
        };
        let (r, g, b) = (
            channel(parts.first()?)?,
            channel(parts.get(1)?)?,
            channel(parts.get(2)?)?,
        );
        let a = match parts.get(3) {
            Some(p) => match p.strip_suffix('%') {
                Some(pct) => pct.parse::<f32>().ok()? / 100.,
                None => p.parse::<f32>().ok()?,
            },
            None => 1.,
        };
        return Some(rgba((r << 16) | (g << 8) | b, a.clamp(0., 1.)));
    }
    NAMED
        .iter()
        .find(|(name, _)| *name == v)
        .map(|(_, hex)| rgba(*hex, 1.))
}

const NAMED: &[(&str, u32)] = &[
    ("black", 0x000000),
    ("white", 0xffffff),
    ("red", 0xff0000),
    ("green", 0x008000),
    ("lime", 0x00ff00),
    ("blue", 0x0000ff),
    ("yellow", 0xffff00),
    ("cyan", 0x00ffff),
    ("aqua", 0x00ffff),
    ("magenta", 0xff00ff),
    ("fuchsia", 0xff00ff),
    ("gray", 0x808080),
    ("grey", 0x808080),
    ("silver", 0xc0c0c0),
    ("maroon", 0x800000),
    ("olive", 0x808000),
    ("purple", 0x800080),
    ("teal", 0x008080),
    ("navy", 0x000080),
    ("orange", 0xffa500),
    ("gold", 0xffd700),
    ("pink", 0xffc0cb),
    ("brown", 0xa52a2a),
    ("crimson", 0xdc143c),
    ("indigo", 0x4b0082),
    ("violet", 0xee82ee),
    ("darkgray", 0xa9a9a9),
    ("darkgrey", 0xa9a9a9),
    ("lightgray", 0xd3d3d3),
    ("lightgrey", 0xd3d3d3),
    ("dimgray", 0x696969),
    ("whitesmoke", 0xf5f5f5),
    ("tomato", 0xff6347),
    ("coral", 0xff7f50),
    ("salmon", 0xfa8072),
    ("skyblue", 0x87ceeb),
    ("steelblue", 0x4682b4),
    ("royalblue", 0x4169e1),
    ("forestgreen", 0x228b22),
    ("seagreen", 0x2e8b57),
    ("darkgreen", 0x006400),
    ("darkred", 0x8b0000),
    ("darkblue", 0x00008b),
    ("darkorange", 0xff8c00),
];

// --- Transforms -----------------------------------------------------------------

const IDENTITY: [f64; 6] = [1., 0., 0., 1., 0., 0.];

/// `m · n` (n applied first).
fn multiply(m: [f64; 6], n: [f64; 6]) -> [f64; 6] {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}

fn apply(m: [f64; 6], x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn own_transform(attrs: &[(&str, String)]) -> [f64; 6] {
    attr(attrs, "transform").map_or(IDENTITY, transform_list)
}

/// A `transform` attribute's list, left to right.
fn transform_list(s: &str) -> [f64; 6] {
    let mut m = IDENTITY;
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let name = rest[..open]
            .trim_matches(|c: char| c.is_whitespace() || c == ',')
            .to_string();
        let Some(close) = rest[open..].find(')') else {
            break;
        };
        let args = number_list(&rest[open + 1..open + close]);
        rest = &rest[open + close + 1..];
        let a = |i: usize| args.get(i).copied().unwrap_or(0.);
        let t = match name.as_str() {
            "matrix" if args.len() == 6 => [a(0), a(1), a(2), a(3), a(4), a(5)],
            "translate" => [1., 0., 0., 1., a(0), a(1)],
            "scale" => {
                let sx = args.first().copied().unwrap_or(1.);
                [sx, 0., 0., args.get(1).copied().unwrap_or(sx), 0., 0.]
            }
            "rotate" => {
                let r = a(0).to_radians();
                let rot = [r.cos(), r.sin(), -r.sin(), r.cos(), 0., 0.];
                if args.len() >= 3 {
                    let (cx, cy) = (a(1), a(2));
                    multiply(
                        multiply([1., 0., 0., 1., cx, cy], rot),
                        [1., 0., 0., 1., -cx, -cy],
                    )
                } else {
                    rot
                }
            }
            "skewX" => [1., 0., a(0).to_radians().tan(), 1., 0., 0.],
            "skewY" => [1., a(0).to_radians().tan(), 0., 1., 0., 0.],
            _ => IDENTITY,
        };
        m = multiply(m, t);
    }
    m
}

// --- Geometry -------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    Cubic(f64, f64, f64, f64, f64, f64),
    Close,
}

fn transformed(segs: &[Seg], m: [f64; 6]) -> Vec<Seg> {
    if m == IDENTITY {
        return segs.to_vec();
    }
    segs.iter()
        .map(|s| match *s {
            Seg::Move(x, y) => {
                let (x, y) = apply(m, x, y);
                Seg::Move(x, y)
            }
            Seg::Line(x, y) => {
                let (x, y) = apply(m, x, y);
                Seg::Line(x, y)
            }
            Seg::Cubic(a, b, c, d, e, f) => {
                let ((a, b), (c, d), (e, f)) = (apply(m, a, b), apply(m, c, d), apply(m, e, f));
                Seg::Cubic(a, b, c, d, e, f)
            }
            Seg::Close => Seg::Close,
        })
        .collect()
}

/// The outline of a shape element, in its user space.
fn geometry(name: &str, attrs: &[(&str, String)]) -> Option<Vec<Seg>> {
    let n = |k: &str| attr(attrs, k).and_then(length).unwrap_or(0.);
    let segs = match name {
        "path" => path_data(attr(attrs, "d")?),
        "rect" => {
            let (x, y, w, h) = (n("x"), n("y"), n("width"), n("height"));
            if w <= 0. || h <= 0. {
                return None;
            }
            let (rx, ry) = match (
                attr(attrs, "rx").and_then(length),
                attr(attrs, "ry").and_then(length),
            ) {
                (Some(rx), Some(ry)) => (rx, ry),
                (Some(r), None) | (None, Some(r)) => (r, r),
                (None, None) => (0., 0.),
            };
            let (rx, ry) = (rx.clamp(0., w / 2.), ry.clamp(0., h / 2.));
            if rx == 0. || ry == 0. {
                vec![
                    Seg::Move(x, y),
                    Seg::Line(x + w, y),
                    Seg::Line(x + w, y + h),
                    Seg::Line(x, y + h),
                    Seg::Close,
                ]
            } else {
                let mut s = vec![Seg::Move(x + rx, y), Seg::Line(x + w - rx, y)];
                arc(
                    &mut s,
                    (x + w - rx, y),
                    rx,
                    ry,
                    0.,
                    false,
                    true,
                    (x + w, y + ry),
                );
                s.push(Seg::Line(x + w, y + h - ry));
                arc(
                    &mut s,
                    (x + w, y + h - ry),
                    rx,
                    ry,
                    0.,
                    false,
                    true,
                    (x + w - rx, y + h),
                );
                s.push(Seg::Line(x + rx, y + h));
                arc(
                    &mut s,
                    (x + rx, y + h),
                    rx,
                    ry,
                    0.,
                    false,
                    true,
                    (x, y + h - ry),
                );
                s.push(Seg::Line(x, y + ry));
                arc(&mut s, (x, y + ry), rx, ry, 0., false, true, (x + rx, y));
                s.push(Seg::Close);
                s
            }
        }
        "circle" | "ellipse" => {
            let (cx, cy) = (n("cx"), n("cy"));
            let (rx, ry) = if name == "circle" {
                (n("r"), n("r"))
            } else {
                (n("rx"), n("ry"))
            };
            if rx <= 0. || ry <= 0. {
                return None;
            }
            let mut s = vec![Seg::Move(cx + rx, cy)];
            arc(
                &mut s,
                (cx + rx, cy),
                rx,
                ry,
                0.,
                false,
                true,
                (cx - rx, cy),
            );
            arc(
                &mut s,
                (cx - rx, cy),
                rx,
                ry,
                0.,
                false,
                true,
                (cx + rx, cy),
            );
            s.push(Seg::Close);
            s
        }
        "line" => vec![Seg::Move(n("x1"), n("y1")), Seg::Line(n("x2"), n("y2"))],
        "polyline" | "polygon" => {
            let v = number_list(attr(attrs, "points")?);
            let mut s: Vec<Seg> = v
                .as_chunks::<2>()
                .0
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    if i == 0 {
                        Seg::Move(p[0], p[1])
                    } else {
                        Seg::Line(p[0], p[1])
                    }
                })
                .collect();
            if name == "polygon" && !s.is_empty() {
                s.push(Seg::Close);
            }
            s
        }
        _ => return None,
    };
    (!segs.is_empty()).then_some(segs)
}

/// Numbers in path data and lists: `1.5.5` is two numbers, `-1-2` too.
struct Numbers<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Numbers<'a> {
    fn new(s: &'a str) -> Self {
        Numbers {
            s: s.as_bytes(),
            i: 0,
        }
    }

    fn skip_separators(&mut self) {
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',')
        {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_separators();
        self.s.get(self.i).copied()
    }

    fn at_number(&mut self) -> bool {
        matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.')
    }

    fn number(&mut self) -> Option<f64> {
        if !self.at_number() {
            return None;
        }
        let start = self.i;
        let s = self.s;
        let mut i = self.i;
        if s[i] == b'-' || s[i] == b'+' {
            i += 1;
        }
        let mut dot = false;
        while i < s.len() && (s[i].is_ascii_digit() || (s[i] == b'.' && !dot)) {
            dot |= s[i] == b'.';
            i += 1;
        }
        if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
            let mut j = i + 1;
            if j < s.len() && (s[j] == b'-' || s[j] == b'+') {
                j += 1;
            }
            if j < s.len() && s[j].is_ascii_digit() {
                while j < s.len() && s[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
            }
        }
        self.i = i;
        std::str::from_utf8(&s[start..i]).ok()?.parse().ok()
    }

    /// An arc flag: one `0` or `1`, maybe glued to what follows.
    fn flag(&mut self) -> Option<bool> {
        match self.peek()? {
            b'0' => {
                self.i += 1;
                Some(false)
            }
            b'1' => {
                self.i += 1;
                Some(true)
            }
            _ => None,
        }
    }
}

/// Path data (`d`), every command, arcs and quadratics as cubic curves.
fn path_data(d: &str) -> Vec<Seg> {
    let mut n = Numbers::new(d);
    let mut out = Vec::new();
    let (mut cur, mut start) = ((0f64, 0f64), (0f64, 0f64));
    // The last control point, for S/s and T/t.
    let mut last_cubic: Option<(f64, f64)> = None;
    let mut last_quad: Option<(f64, f64)> = None;
    let mut cmd = 0u8;
    loop {
        let c = match n.peek() {
            None => break,
            Some(c) if c.is_ascii_alphabetic() && c != b'e' && c != b'E' => {
                n.i += 1;
                c
            }
            // Implicit repetition (after M, a repeated move is a line).
            Some(_) if cmd != 0 && n.at_number() => match cmd {
                b'M' => b'L',
                b'm' => b'l',
                c => c,
            },
            Some(_) => break,
        };
        cmd = c;
        let rel = c.is_ascii_lowercase();
        let base = if rel { cur } else { (0., 0.) };
        let point = |n: &mut Numbers| -> Option<(f64, f64)> {
            Some((base.0 + n.number()?, base.1 + n.number()?))
        };
        let (mut cubic_ctrl, mut quad_ctrl) = (None, None);
        match c.to_ascii_uppercase() {
            b'M' => {
                let Some(p) = point(&mut n) else { break };
                out.push(Seg::Move(p.0, p.1));
                (cur, start) = (p, p);
            }
            b'L' => {
                let Some(p) = point(&mut n) else { break };
                out.push(Seg::Line(p.0, p.1));
                cur = p;
            }
            b'H' => {
                let Some(x) = n.number() else { break };
                cur.0 = if rel { cur.0 + x } else { x };
                out.push(Seg::Line(cur.0, cur.1));
            }
            b'V' => {
                let Some(y) = n.number() else { break };
                cur.1 = if rel { cur.1 + y } else { y };
                out.push(Seg::Line(cur.0, cur.1));
            }
            b'C' => {
                let (Some(c1), Some(c2), Some(p)) = (point(&mut n), point(&mut n), point(&mut n))
                else {
                    break;
                };
                out.push(Seg::Cubic(c1.0, c1.1, c2.0, c2.1, p.0, p.1));
                cubic_ctrl = Some(c2);
                cur = p;
            }
            b'S' => {
                let (Some(c2), Some(p)) = (point(&mut n), point(&mut n)) else {
                    break;
                };
                let c1 = last_cubic.map_or(cur, |l| (2. * cur.0 - l.0, 2. * cur.1 - l.1));
                out.push(Seg::Cubic(c1.0, c1.1, c2.0, c2.1, p.0, p.1));
                cubic_ctrl = Some(c2);
                cur = p;
            }
            b'Q' | b'T' => {
                let q = if c.eq_ignore_ascii_case(&b'Q') {
                    let Some(q) = point(&mut n) else { break };
                    q
                } else {
                    last_quad.map_or(cur, |l| (2. * cur.0 - l.0, 2. * cur.1 - l.1))
                };
                let Some(p) = point(&mut n) else { break };
                // The cubic with the same curve.
                let c1 = (
                    cur.0 + 2. / 3. * (q.0 - cur.0),
                    cur.1 + 2. / 3. * (q.1 - cur.1),
                );
                let c2 = (p.0 + 2. / 3. * (q.0 - p.0), p.1 + 2. / 3. * (q.1 - p.1));
                out.push(Seg::Cubic(c1.0, c1.1, c2.0, c2.1, p.0, p.1));
                quad_ctrl = Some(q);
                cur = p;
            }
            b'A' => {
                let (Some(rx), Some(ry), Some(rot)) = (n.number(), n.number(), n.number()) else {
                    break;
                };
                let (Some(large), Some(sweep)) = (n.flag(), n.flag()) else {
                    break;
                };
                let Some(p) = point(&mut n) else { break };
                arc(&mut out, cur, rx, ry, rot, large, sweep, p);
                cur = p;
            }
            b'Z' => {
                out.push(Seg::Close);
                cur = start;
                // A command after Z starts at the subpath's start.
                cmd = 0;
            }
            _ => break,
        }
        last_cubic = cubic_ctrl;
        last_quad = quad_ctrl;
    }
    out
}

/// An elliptical arc from `from` to `to` (SVG endpoint parameters) as
/// cubic curves, at most a quarter turn each.
#[allow(clippy::too_many_arguments)]
fn arc(
    out: &mut Vec<Seg>,
    from: (f64, f64),
    rx: f64,
    ry: f64,
    rotation: f64,
    large: bool,
    sweep: bool,
    to: (f64, f64),
) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if from == to {
        return;
    }
    if rx == 0. || ry == 0. {
        out.push(Seg::Line(to.0, to.1));
        return;
    }
    let phi = rotation.to_radians();
    let (sin, cos) = phi.sin_cos();
    // SVG 1.1 implementation notes, F.6.5.
    let (dx, dy) = ((from.0 - to.0) / 2., (from.1 - to.1) / 2.);
    let (x1, y1) = (cos * dx + sin * dy, -sin * dx + cos * dy);
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1. {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut k = (num / den).max(0.).sqrt();
    if large == sweep {
        k = -k;
    }
    let (cxp, cyp) = (k * rx * y1 / ry, -k * ry * x1 / rx);
    let (cx, cy) = (
        cos * cxp - sin * cyp + (from.0 + to.0) / 2.,
        sin * cxp + cos * cyp + (from.1 + to.1) / 2.,
    );
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
        if a.is_nan() { 0. } else { a }
    };
    let theta = angle(1., 0., (x1 - cxp) / rx, (y1 - cyp) / ry);
    let mut delta = angle(
        (x1 - cxp) / rx,
        (y1 - cyp) / ry,
        (-x1 - cxp) / rx,
        (-y1 - cyp) / ry,
    );
    if !sweep && delta > 0. {
        delta -= std::f64::consts::TAU;
    } else if sweep && delta < 0. {
        delta += std::f64::consts::TAU;
    }
    let parts = (delta.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.) as usize;
    let step = delta / parts as f64;
    let kappa = 4. / 3. * (step / 4.).tan();
    let point = |t: f64| {
        let (s, c) = t.sin_cos();
        (
            cx + rx * c * cos - ry * s * sin,
            cy + rx * c * sin + ry * s * cos,
        )
    };
    let deriv = |t: f64| {
        let (s, c) = t.sin_cos();
        (-rx * s * cos - ry * c * sin, -rx * s * sin + ry * c * cos)
    };
    let mut t = theta;
    for i in 0..parts {
        let t2 = t + step;
        let (p0, d0, d1) = (point(t), deriv(t), deriv(t2));
        let p1 = if i + 1 == parts { to } else { point(t2) };
        out.push(Seg::Cubic(
            p0.0 + kappa * d0.0,
            p0.1 + kappa * d0.1,
            p1.0 - kappa * d1.0,
            p1.1 - kappa * d1.1,
            p1.0,
            p1.1,
        ));
        t = t2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn sizes_come_from_the_attributes_or_the_view_box() {
        let s = parse(r#"<svg viewBox="0 0 20 10"></svg>"#).unwrap();
        assert_eq!((s.width, s.height), (20., 10.));
        let s = parse(r#"<svg width="40" viewBox="0 0 20 10"/>"#).unwrap();
        assert_eq!((s.width, s.height), (40., 20.));
        let s = parse(r#"<svg width="30pt" height="15px"/>"#).unwrap();
        assert_eq!((s.width, s.height), (40., 15.));
        assert!(parse("<p>no svg</p>").is_none());
    }

    #[test]
    fn rects_become_fractions_of_the_view_box() {
        let s = parse(
            r##"<?xml version="1.0"?><!-- a comment -->
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="10 0 100 50">
              <rect x="10" y="0" width="50" height="25" fill="#ff0000"/>
            </svg>"##,
        )
        .unwrap();
        let p = &s.drawing.paths[0];
        assert_eq!(p.fill, Some(rgba(0xff0000, 1.)));
        assert_eq!(p.segments[0], Segment::Move(0., 0.));
        assert_eq!(p.segments[2], Segment::Line(0.5, 0.5));
    }

    #[test]
    fn groups_inherit_paint_and_compose_transforms() {
        let s = parse(
            r#"<svg viewBox="0 0 100 100">
              <g fill="blue" transform="translate(10 20)" opacity="0.5">
                <g transform="scale(2)"><path d="M0 0 L10 0"/></g>
                <circle cx="0" cy="0" r="5" style="fill: none; stroke: #000; stroke-width: 4"/>
              </g>
              <path d="M0 0h10" fill="red"/>
            </svg>"#,
        )
        .unwrap();
        let [line, ring, last] = &s.drawing.paths[..] else {
            panic!("{:?}", s.drawing.paths)
        };
        assert_eq!(line.segments[0], Segment::Move(0.1, 0.2));
        assert_eq!(line.segments[1], Segment::Line(0.3, 0.2));
        let blue = line.fill.unwrap();
        assert!(close(blue.b, 1.) && close(blue.a, 0.5));
        assert!(ring.fill.is_none());
        assert_eq!((ring.stroke.map(|c| c.a), ring.width), (Some(0.5), 4.));
        assert_eq!(last.fill, Some(rgba(0xff0000, 1.)));
    }

    #[test]
    fn path_data_handles_relative_implicit_and_glued_numbers() {
        let segs = path_data("m10,10 20-5 h5v5 Z l1.5.5");
        assert_eq!(
            segs,
            vec![
                Seg::Move(10., 10.),
                Seg::Line(30., 5.),
                Seg::Line(35., 5.),
                Seg::Line(35., 10.),
                Seg::Close,
                Seg::Line(11.5, 10.5),
            ]
        );
        // Smooth curves reflect the previous control point.
        let segs = path_data("M0 0 C 0 10 10 10 10 0 S 20 -10 20 0");
        assert_eq!(segs[2], Seg::Cubic(10., -10., 20., -10., 20., 0.));
        // A quadratic is the equivalent cubic.
        let segs = path_data("M0 0 Q 3 3 6 0");
        assert_eq!(segs[1], Seg::Cubic(2., 2., 4., 2., 6., 0.));
    }

    #[test]
    fn arcs_land_on_their_end_point_and_pass_through_the_circle() {
        // A half circle of radius 10 from (0, 0) to (20, 0), sweeping clockwise
        // (on screen: through the top, y = -10).
        let segs = path_data("M0 0 A10 10 0 0 1 20 0");
        let Some(Seg::Cubic(.., ex, ey)) = segs.last().copied() else {
            panic!("{segs:?}")
        };
        assert_eq!((ex, ey), (20., 0.));
        assert_eq!(segs.len(), 3, "two quarter turns");
        let Seg::Cubic(.., mx, my) = segs[1] else {
            panic!()
        };
        assert!(
            (mx - 10.).abs() < 1e-9 && (my + 10.).abs() < 1e-9,
            "{mx} {my}"
        );
        // Glued flags (`a1 1 0 00 1 1`).
        assert_eq!(path_data("M0 0a1 1 0 00 2 0").len(), 3);
    }

    #[test]
    fn colors_names_and_hidden_elements() {
        assert_eq!(color("#0f0"), Some(rgba(0x00ff00, 1.)));
        assert_eq!(color("rgb(255, 0, 0)"), Some(rgba(0xff0000, 1.)));
        assert_eq!(color("rgba(0,0,255,.5)"), Some(rgba(0x0000ff, 0.5)));
        assert_eq!(color("Orange"), Some(rgba(0xffa500, 1.)));
        let s = parse(
            r##"<svg viewBox="0 0 10 10" color="#00ff00">
              <defs><rect width="10" height="10"/></defs>
              <rect width="5" height="5" display="none"/>
              <rect width="5" height="5" fill="currentColor" fill-rule="evenodd"/>
              <text x="0" y="5">skipped</text>
            </svg>"##,
        )
        .unwrap();
        assert_eq!(s.drawing.paths.len(), 1);
        assert_eq!(s.drawing.paths[0].fill, Some(rgba(0x00ff00, 1.)));
        assert!(s.drawing.paths[0].even_odd);
    }

    #[test]
    fn stroke_widths_follow_the_view_box_scale() {
        // 10 user units shown 100 pt wide: a 2-unit stroke is 20 pt.
        let s = parse(
            r#"<svg width="100" height="100" viewBox="0 0 10 10">
              <line x1="0" y1="5" x2="10" y2="5" stroke="black" stroke-width="2" stroke-dasharray="1 0.5" stroke-linecap="round"/>
            </svg>"#,
        )
        .unwrap();
        let p = &s.drawing.paths[0];
        assert_eq!((p.width, p.dash, p.round), (20., Some((10., 5.)), true));
    }
}
