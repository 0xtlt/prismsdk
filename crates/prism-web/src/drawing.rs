//! A drawing with more than paths in flat colors (an SVG document's
//! gradients, text, images and groups, #342) as the inline `<svg>`
//! `shapeDocument` (client.js) builds: the browser draws its own text,
//! gradients, clips, masks and filters.
//!
//! What refers to a definition (`fill="url(#…)"`, `clip-path`, `mask`,
//! `filter`) is an attribute, so it names an element of the page whatever
//! style sheet the rest of the paint is in; colors stay CSS (`light-dark()`,
//! `currentColor`).

use std::fmt::Write as _;

use prism_core::Path;
use prism_core::shape::{
    Anchor, Baseline, Drawing, Fit, Gradient, GradientShape, Group, IDENTITY, Image, Item, Mask,
    Matrix, Segment, Spread, Text,
};

use crate::html::escape;

/// Whether `d` has more than paths in flat colors, or a size of its own
/// (its strokes follow the size it is shown at).
pub(crate) fn is_document(d: &Drawing) -> bool {
    let gradient = |p: &Path| p.fill_gradient.is_some() || p.stroke_gradient.is_some();
    d.size.is_some() || !d.items.is_empty() || d.paths.iter().any(gradient)
}

/// The size `d` is drawn at whatever its box: its own, when it has one and
/// no insets. The `<svg>` is then stretched to the box
/// (`preserveAspectRatio="none"`), strokes, text and shadows with it.
pub(crate) fn natural_size(d: &Drawing) -> Option<(f32, f32)> {
    d.size
        .filter(|(w, h)| *w > 0. && *h > 0. && d.inset == (0., 0.))
}

/// `d` as an `<svg>`: at its own size if it has one, else in the box's
/// pixels (`size`), else nothing until the client measures the box. Each
/// definition's id starts with `prefix` (unique in the page); `style` turns
/// CSS into an attribute.
pub(crate) fn document_svg(
    d: &Drawing,
    size: Option<(f32, f32)>,
    prefix: &str,
    style: &mut dyn FnMut(&str) -> String,
) -> String {
    let natural = natural_size(d);
    let Some((w, h)) = natural.or(size) else {
        return String::new();
    };
    let items = d.resolve_items(w, h, 1.);
    if items.is_empty() {
        return String::new();
    }
    // Pixels need two decimals; a small document shown large needs more.
    let decimals = match natural {
        Some(_) => (5. - w.max(h).log10().floor()).clamp(2., 6.),
        None => 2.,
    };
    let mut writer = Writer {
        prefix,
        next: 0,
        defs: String::new(),
        unit: 10f32.powf(decimals),
        size: (w, h),
        style,
    };
    let mut body = String::new();
    writer.items(&items, &mut body);
    let mut o = format!(
        r#"<svg aria-hidden="true" viewBox="0 0 {} {}""#,
        writer.num(w),
        writer.num(h)
    );
    if natural.is_some() {
        o.push_str(r#" preserveAspectRatio="none""#);
    }
    o.push('>');
    if !writer.defs.is_empty() {
        let _ = write!(o, "<defs>{}</defs>", writer.defs);
    }
    o.push_str(&body);
    o.push_str("</svg>");
    o
}

/// A number of a matrix or of a gradient's own coordinates: six decimals.
fn fine(v: f32) -> String {
    let v = (v * 1e6).round() / 1e6;
    if v == 0. {
        "0".to_string()
    } else {
        v.to_string()
    }
}

/// Whether `m` moves nothing (to the six decimals it is written with).
fn still(m: &Matrix) -> bool {
    m.iter().zip(IDENTITY).all(|(a, b)| (a - b).abs() < 5e-7)
}

fn matrix(m: &Matrix) -> String {
    let v: Vec<String> = m.iter().map(|v| fine(*v)).collect();
    format!("matrix({})", v.join(" "))
}

struct Writer<'a> {
    prefix: &'a str,
    next: usize,
    /// Gradients, clip paths, masks and filters, in the order they are met.
    defs: String,
    /// Coordinates are rounded to `1 / unit`.
    unit: f32,
    size: (f32, f32),
    style: &'a mut dyn FnMut(&str) -> String,
}

impl Writer<'_> {
    fn num(&self, v: f32) -> String {
        let v = (v * self.unit).round() / self.unit;
        if v == 0. {
            "0".to_string()
        } else {
            v.to_string()
        }
    }

    fn id(&mut self) -> String {
        self.next += 1;
        format!("{}_{}", self.prefix, self.next)
    }

    fn css(&mut self, css: &str) -> String {
        if css.is_empty() {
            String::new()
        } else {
            (self.style)(css)
        }
    }

    fn items(&mut self, items: &[Item], out: &mut String) {
        for item in items {
            match item {
                Item::Path(p) => self.path(p, out),
                Item::Text(t) => self.text(t, out),
                Item::Image(i) => self.image(i, out),
                Item::Group(g) => self.group(g, out),
            }
        }
    }

    /// Path data of segments in points (arcs are curves already).
    fn data(&self, segments: &[Segment]) -> String {
        let mut d = String::new();
        for s in segments {
            match *s {
                Segment::Move(x, y) => {
                    let _ = write!(d, "M{} {}", self.num(x), self.num(y));
                }
                Segment::Line(x, y) => {
                    let _ = write!(d, "L{} {}", self.num(x), self.num(y));
                }
                Segment::Cubic(a, b, c, e, f, g) => {
                    let _ = write!(
                        d,
                        "C{} {} {} {} {} {}",
                        self.num(a),
                        self.num(b),
                        self.num(c),
                        self.num(e),
                        self.num(f),
                        self.num(g)
                    );
                }
                Segment::Arc { .. } => {}
                Segment::Close => d.push('Z'),
            }
        }
        d
    }

    /// Defines `g` and gives its id (none without stops: nothing painted).
    fn gradient(&mut self, g: &Gradient) -> Option<String> {
        if g.stops.is_empty() {
            return None;
        }
        let id = self.id();
        let (tag, geometry) = match g.shape {
            GradientShape::Linear { x1, y1, x2, y2 } => (
                "linearGradient",
                format!(
                    r#"x1="{}" y1="{}" x2="{}" y2="{}""#,
                    fine(x1),
                    fine(y1),
                    fine(x2),
                    fine(y2)
                ),
            ),
            GradientShape::Radial { cx, cy, r, fx, fy } => (
                "radialGradient",
                format!(
                    r#"cx="{}" cy="{}" r="{}" fx="{}" fy="{}""#,
                    fine(cx),
                    fine(cy),
                    fine(r),
                    fine(fx),
                    fine(fy)
                ),
            ),
        };
        let mut o = format!(r#"<{tag} id="{id}" gradientUnits="userSpaceOnUse" {geometry}"#);
        if !still(&g.transform) {
            let _ = write!(o, r#" gradientTransform="{}""#, matrix(&g.transform));
        }
        match g.spread {
            Spread::Pad => {}
            Spread::Reflect => o.push_str(r#" spreadMethod="reflect""#),
            Spread::Repeat => o.push_str(r#" spreadMethod="repeat""#),
        }
        o.push('>');
        for s in &g.stops {
            let color = self.css(&format!("stop-color:{};", s.color.to_css()));
            let _ = write!(o, r#"<stop offset="{}"{color}></stop>"#, fine(s.offset));
        }
        let _ = write!(o, "</{tag}>");
        self.defs.push_str(&o);
        Some(id)
    }

    fn path(&mut self, p: &Path, out: &mut String) {
        let mut attrs = String::new();
        let mut css = String::new();
        let flat = |c: Option<prism_core::Color>| c.map_or("none".to_string(), |c| c.to_css());
        match p.fill_gradient.as_ref().and_then(|g| self.gradient(g)) {
            Some(id) => {
                let _ = write!(attrs, r#" fill="url(#{id})""#);
            }
            None if p.fill_gradient.is_some() => css.push_str("fill:none;"),
            None => {
                let _ = write!(css, "fill:{};", flat(p.fill));
            }
        }
        match p.stroke_gradient.as_ref().and_then(|g| self.gradient(g)) {
            Some(id) => {
                let _ = write!(attrs, r#" stroke="url(#{id})""#);
            }
            None if p.stroke_gradient.is_some() => css.push_str("stroke:none;"),
            None => {
                let _ = write!(css, "stroke:{};", flat(p.stroke));
            }
        }
        if p.stroke.is_some() || p.stroke_gradient.is_some() {
            let _ = write!(css, "stroke-width:{};", self.num(p.width));
        }
        if p.round {
            css.push_str("stroke-linecap:round;stroke-linejoin:round;");
        }
        if let Some((dash, gap)) = p.dash {
            let _ = write!(
                css,
                "stroke-dasharray:{} {};",
                self.num(dash),
                self.num(gap)
            );
        }
        if p.even_odd {
            css.push_str("fill-rule:evenodd;");
        }
        let css = self.css(&css);
        let _ = write!(
            out,
            r#"<path d="{}"{attrs}{css}></path>"#,
            self.data(&p.segments)
        );
    }

    /// One line: a `<text>` of `<tspan>`s, in a `<g>` with its transform
    /// (CSS turns the `<text>` itself around in right-to-left pages).
    fn text(&mut self, t: &Text, out: &mut String) {
        if t.runs.is_empty() {
            return;
        }
        let moved = !still(&t.transform);
        if moved {
            let _ = write!(out, r#"<g transform="{}">"#, matrix(&t.transform));
        }
        let _ = write!(out, r#"<text x="{}" y="{}""#, self.num(t.x), self.num(t.y));
        match t.anchor {
            Anchor::Start => {}
            Anchor::Middle => out.push_str(r#" text-anchor="middle""#),
            Anchor::End => out.push_str(r#" text-anchor="end""#),
        }
        match t.baseline {
            Baseline::Alphabetic => {}
            Baseline::Middle => out.push_str(r#" dominant-baseline="middle""#),
            Baseline::Central => out.push_str(r#" dominant-baseline="central""#),
            Baseline::Hanging => out.push_str(r#" dominant-baseline="hanging""#),
        }
        // Spaces are the ones the drawing has (also `white-space: pre`,
        // prism.css).
        out.push_str(r#" xml:space="preserve">"#);
        // A `<tspan>`'s `dy` moves what follows too: each run gets the
        // difference with the one before.
        let mut below = 0.;
        for r in &t.runs {
            let mut css = format!(
                "font-family:{};font-size:{}px;fill:{};",
                r.family.css(),
                self.num(r.size),
                r.fill.to_css()
            );
            if r.weight != 400 {
                let _ = write!(css, "font-weight:{};", r.weight);
            }
            if r.italic {
                css.push_str("font-style:italic;");
            }
            if r.spacing != 0. {
                let _ = write!(css, "letter-spacing:{}px;", self.num(r.spacing));
            }
            out.push_str("<tspan");
            if r.dx != 0. {
                let _ = write!(out, r#" dx="{}""#, self.num(r.dx));
            }
            if r.dy != below {
                let _ = write!(out, r#" dy="{}""#, self.num(r.dy - below));
                below = r.dy;
            }
            let css = self.css(&css);
            let _ = write!(out, "{css}>{}</tspan>", escape(&r.text));
        }
        out.push_str("</text>");
        if moved {
            out.push_str("</g>");
        }
    }

    /// A bitmap from a `data:` URL (nothing else is fetched for a drawing).
    fn image(&mut self, i: &Image, out: &mut String) {
        if !i.src.starts_with("data:image/") {
            return;
        }
        let moved = !still(&i.transform);
        if moved {
            let _ = write!(out, r#"<g transform="{}">"#, matrix(&i.transform));
        }
        let side = |a: f32| match a {
            a if a < 0.25 => "Min",
            a if a > 0.75 => "Max",
            _ => "Mid",
        };
        let fit = match i.fit {
            Fit::Stretch => "none".to_string(),
            Fit::Meet => format!("x{}Y{}", side(i.align.0), side(i.align.1)),
            Fit::Slice => format!("x{}Y{} slice", side(i.align.0), side(i.align.1)),
        };
        let _ = write!(
            out,
            r#"<image x="{}" y="{}" width="{}" height="{}" preserveAspectRatio="{fit}" href="{}""#,
            fine(i.x),
            fine(i.y),
            fine(i.w),
            fine(i.h),
            escape(&i.src)
        );
        if i.opacity != 1. {
            let _ = write!(out, r#" opacity="{}""#, fine(i.opacity));
        }
        out.push_str("></image>");
        if moved {
            out.push_str("</g>");
        }
    }

    /// A `<g>`: its filter (blur, shadows), clip, mask and opacity, which
    /// SVG applies in that order.
    fn group(&mut self, g: &Group, out: &mut String) {
        let mut attrs = String::new();
        if g.blur > 0. || !g.shadows.is_empty() {
            let id = self.filter(g);
            let _ = write!(attrs, r#" filter="url(#{id})""#);
        }
        if let Some(c) = &g.clip {
            let id = self.id();
            let rule = if c.even_odd {
                r#" clip-rule="evenodd""#
            } else {
                ""
            };
            let _ = write!(
                self.defs,
                r#"<clipPath id="{id}" clipPathUnits="userSpaceOnUse"><path d="{}"{rule}></path></clipPath>"#,
                self.data(&c.segments)
            );
            let _ = write!(attrs, r#" clip-path="url(#{id})""#);
        }
        if let Some(m) = &g.mask {
            let id = self.mask(m);
            let _ = write!(attrs, r#" mask="url(#{id})""#);
        }
        if g.opacity != 1. {
            let _ = write!(attrs, r#" opacity="{}""#, fine(g.opacity));
        }
        let _ = write!(out, "<g{attrs}>");
        self.items(&g.items, out);
        out.push_str("</g>");
    }

    /// The group blurred, then its shadows under it (the first at the
    /// bottom): each is the group's outline blurred, moved and filled.
    fn filter(&mut self, g: &Group) -> String {
        let id = self.id();
        // What a blur shows beyond its source: three standard deviations.
        let reach = g
            .shadows
            .iter()
            .map(|s| s.dx.abs().max(s.dy.abs()) + 3. * s.blur)
            .fold(0., f32::max)
            + 3. * g.blur;
        let (w, h) = self.size;
        let mut o = format!(
            r#"<filter id="{id}" filterUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}" color-interpolation-filters="sRGB">"#,
            self.num(-reach),
            self.num(-reach),
            self.num(w + 2. * reach),
            self.num(h + 2. * reach)
        );
        let source = if g.blur > 0. {
            let _ = write!(
                o,
                r#"<feGaussianBlur in="SourceGraphic" stdDeviation="{}" result="c"></feGaussianBlur>"#,
                self.num(g.blur)
            );
            "c"
        } else {
            "SourceGraphic"
        };
        if !g.shadows.is_empty() {
            for (k, s) in g.shadows.iter().enumerate() {
                let mut from = format!(r#" in="{source}""#);
                if s.blur > 0. {
                    let _ = write!(
                        o,
                        r#"<feGaussianBlur{from} stdDeviation="{}"></feGaussianBlur>"#,
                        self.num(s.blur)
                    );
                    from.clear();
                }
                let color = self.css(&format!("flood-color:{};", s.color.to_css()));
                let _ = write!(
                    o,
                    r#"<feOffset{from} dx="{}" dy="{}" result="o{k}"></feOffset><feFlood{color}></feFlood><feComposite in2="o{k}" operator="in" result="s{k}"></feComposite>"#,
                    self.num(s.dx),
                    self.num(s.dy)
                );
            }
            o.push_str("<feMerge>");
            for k in 0..g.shadows.len() {
                let _ = write!(o, r#"<feMergeNode in="s{k}"></feMergeNode>"#);
            }
            let _ = write!(o, r#"<feMergeNode in="{source}"></feMergeNode></feMerge>"#);
        }
        o.push_str("</filter>");
        self.defs.push_str(&o);
        id
    }

    /// What the mask draws, over the box and half of it around.
    fn mask(&mut self, m: &Mask) -> String {
        let id = self.id();
        let (w, h) = self.size;
        let mut o = format!(
            r#"<mask id="{id}" maskUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}" mask-type="{}">"#,
            self.num(-w / 2.),
            self.num(-h / 2.),
            self.num(2. * w),
            self.num(2. * h),
            if m.luminance { "luminance" } else { "alpha" }
        );
        self.items(&m.items, &mut o);
        o.push_str("</mask>");
        self.defs.push_str(&o);
        id
    }
}
