//! SVG markup turned into a [`Drawing`], so an SVG made by a server (a QR
//! code, an icon, a chart) or exported by a design tool is drawn natively
//! on every backend, by the same vector machinery as
//! [`shape`](crate::shape()).
//!
//! Supported:
//!
//! - structure: `svg` (`viewBox`, `width`, `height`,
//!   `preserveAspectRatio`, nested too), `g`, `a`, `defs`, `symbol` and
//!   `use`; `transform` (`matrix`, `translate`, `scale`, `rotate`, `skewX`,
//!   `skewY`); `display` / `visibility`;
//! - shapes: `path` (every command, arcs included), `rect` (with `rx` /
//!   `ry`), `circle`, `ellipse`, `line`, `polyline`, `polygon`; lengths in
//!   `px`, `pt`, `pc`, `mm`, `cm`, `in`, `em`, `ex` and `%` of the
//!   `viewBox`;
//! - paint: `fill`, `stroke`, `stroke-width`, `fill-opacity`,
//!   `stroke-opacity`, `stroke-linecap` / `stroke-linejoin` (round or not),
//!   `stroke-dasharray` (its first dash and gap), `fill-rule`; colors as
//!   hex, `rgb()` / `rgba()`, common names, `currentColor` (the `color`
//!   attribute, else [`Color::current`]: the text color of the node the SVG
//!   is drawn in, so `svg(icon).text_color(…)` tints it and an adaptive text
//!   color follows dark mode);
//! - gradients on fills and strokes: `linearGradient`, `radialGradient`,
//!   `gradientUnits`, `gradientTransform`, `spreadMethod`, `stop-color`,
//!   `stop-opacity`, `href` from one gradient to another;
//! - `opacity` on a group (the group as one picture), `clip-path="url(#…)"`
//!   (`clipPathUnits`, `clip-rule`), `mask="url(#…)"` (luminance and alpha,
//!   `maskContentUnits`), `filter="url(#…)"` when it is a drop shadow
//!   (`feDropShadow`, or `feOffset` / `feGaussianBlur` / `feFlood` /
//!   `feColorMatrix` / `feComposite` merged under the source, as design
//!   tools export them) or a blur of the source (`feGaussianBlur`), and the
//!   CSS `drop-shadow()` and `blur()`;
//! - `text` and `tspan`: `x`, `y`, `dx`, `dy`, `font-family`, `font-size`,
//!   `font-weight`, `font-style`, `letter-spacing`, `text-anchor`,
//!   `dominant-baseline`, `fill`, `fill-opacity`;
//! - `image` with a `data:` URL (PNG, JPEG): `x`, `y`, `width`, `height`,
//!   `preserveAspectRatio`;
//! - `style` sheets: type, `.class` and `#id` selectors, combined
//!   (`rect.a.b`), in lists (`a, b`), with descendant (`a b`) and child
//!   (`a > b`) combinators; `style` attributes; presentation attributes.
//!
//! Not drawn, and listed in [`Svg::skipped`]: patterns, markers,
//! `foreignObject`, `textPath`, other filters (lighting, turbulence, inner
//! shadows…), images that are not `data:` URLs, CSS selectors with pseudo
//! classes or attributes, animations.

use std::collections::{BTreeMap, HashMap};

use crate::shape::{
    Anchor, Baseline, Clip, Drawing, Fit, Gradient, GradientShape, Group, Image, Item, Mask, Path,
    Segment, Shadow, Spread, Stop, Text, TextRun,
};
use crate::style::{Color, FontFamily, rgba};

/// A parsed SVG: its own size in points and what it draws.
#[derive(Clone, Debug, PartialEq)]
pub struct Svg {
    /// `width` (or the `viewBox` width), points.
    pub width: f32,
    /// `height` (or the `viewBox` height), points.
    pub height: f32,
    /// What it draws, in fractions of `width` × `height`; stroke widths,
    /// shadows and blurs in points at that size
    /// ([`Drawing::size`](crate::shape::Drawing::size)). A document of
    /// plain shapes has them in [`Drawing::paths`]; text, images and groups
    /// put everything in [`Drawing::items`].
    pub drawing: Drawing,
    /// What the document has and is not drawn, by kind with its count
    /// (`"<foreignObject> ×2"`, `"<filter id=\"glow\">"`), so an app can
    /// show the document another way. Empty: everything is drawn.
    pub skipped: Vec<String>,
}

/// Reads SVG markup. `None` when there is no `<svg>` element.
pub fn parse(markup: &str) -> Option<Svg> {
    let doc = Doc::read(markup);
    let root = doc.nodes.iter().position(|n| n.name == "svg")?;
    let props = doc.props(root);
    let (vx, vy, vw, vh, width, height) = viewport(&props);
    let (vw, vh) = (vw.max(1e-9), vh.max(1e-9));
    // The viewBox in the width × height viewport, then in fractions of it.
    let fit = fit_box(
        (vx, vy, vw, vh),
        (0., 0., width as f64, height as f64),
        props.get("preserveAspectRatio").unwrap_or(""),
    );
    let (w, h) = ((width as f64).max(1e-9), (height as f64).max(1e-9));
    let mut walker = Walker {
        doc: &doc,
        view: (vw, vh),
        to_frame: multiply([1. / w, 0., 0., 1. / h, 0., 0.], fit),
        unit: scale_of(fit),
        skipped: BTreeMap::new(),
        using: Vec::new(),
        depth: 0,
    };
    if doc.unread_css {
        walker.skip("<style> rules with unsupported selectors");
    }
    let lengths = Lengths {
        font_size: 16.,
        view: (vw, vh),
    };
    let mut state = State::default().inherit(&props, &lengths);
    // The root's own x / y do not move its content.
    state.transform = own_transform(&props);
    let mut items = Vec::new();
    if !state.display_none {
        let mut content = Vec::new();
        walker.children(root, &state, &mut content);
        walker.wrap(root, &props, &state, content, &mut items);
    }
    let skipped = walker
        .skipped
        .into_iter()
        .map(|(what, count)| {
            if count > 1 {
                format!("{what} ×{count}")
            } else {
                what
            }
        })
        .collect();
    let mut drawing = Drawing {
        inset: (0., 0.),
        paths: Vec::new(),
        size: Some((width, height)),
        items: Vec::new(),
    };
    if items.iter().all(|i| matches!(i, Item::Path(_))) {
        drawing.paths = items
            .into_iter()
            .filter_map(|i| match i {
                Item::Path(p) => Some(p),
                _ => None,
            })
            .collect();
    } else {
        drawing.items = items;
    }
    Some(Svg {
        width,
        height,
        drawing,
        skipped,
    })
}

// --- Markup ---------------------------------------------------------------------

enum Tag<'a> {
    Open {
        name: &'a str,
        attrs: Vec<(&'a str, String)>,
        self_closing: bool,
    },
    Close,
    /// Characters between tags; `true`: as they are (a CDATA section).
    Text(&'a str, bool),
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    /// The next tag or stretch of text (comments, declarations and
    /// processing instructions skipped).
    fn next_tag(&mut self) -> Option<Tag<'a>> {
        loop {
            let rest = &self.src[self.pos..];
            if rest.is_empty() {
                return None;
            }
            if !rest.starts_with('<') {
                let end = rest.find('<').unwrap_or(rest.len());
                self.pos += end;
                return Some(Tag::Text(&rest[..end], false));
            }
            if let Some(body) = rest.strip_prefix("<![CDATA[") {
                let end = body.find("]]>");
                self.pos += "<![CDATA[".len() + end.map_or(body.len(), |e| e + 3);
                return Some(Tag::Text(&body[..end.unwrap_or(body.len())], true));
            }
            if let Some((_, close)) = [("<!--", "-->"), ("<?", "?>")]
                .into_iter()
                .find(|(open, _)| rest.starts_with(open))
            {
                self.pos += rest.find(close).map_or(rest.len(), |e| e + close.len());
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

/// Character references read: the named ones of XML, `&nbsp;`, and numbers
/// (`&#8212;`, `&#x2014;`).
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let end = rest.find(';').filter(|e| *e <= 10);
        let ch = end.and_then(|e| match &rest[1..e] {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "amp" => Some('&'),
            "nbsp" => Some('\u{a0}'),
            n => {
                let n = n.strip_prefix('#')?;
                let code = match n.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => n.parse().ok()?,
                };
                char::from_u32(code)
            }
        });
        match (ch, end) {
            (Some(c), Some(e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// One element.
struct Node<'a> {
    /// Its name without a namespace prefix.
    name: &'a str,
    attrs: Vec<(&'a str, String)>,
    parent: Option<usize>,
    children: Vec<Child>,
}

enum Child {
    Node(usize),
    Text(String),
}

/// Elements whose characters count (everywhere else they are layout).
const WITH_TEXT: &[&str] = &["text", "tspan", "textPath", "a", "style"];

/// The document: its elements, those with an `id`, its style sheets.
struct Doc<'a> {
    nodes: Vec<Node<'a>>,
    ids: HashMap<String, usize>,
    rules: Vec<Rule>,
    /// A style sheet has selectors that are not read.
    unread_css: bool,
}

impl<'a> Doc<'a> {
    fn read(markup: &'a str) -> Doc<'a> {
        let mut parser = Parser {
            src: markup,
            pos: 0,
        };
        let mut nodes: Vec<Node> = Vec::new();
        let mut open: Vec<usize> = Vec::new();
        while let Some(tag) = parser.next_tag() {
            match tag {
                Tag::Text(text, raw) => {
                    if let Some(&top) = open.last()
                        && WITH_TEXT.contains(&nodes[top].name)
                    {
                        let text = if raw {
                            text.to_string()
                        } else {
                            unescape(text)
                        };
                        nodes[top].children.push(Child::Text(text));
                    }
                }
                Tag::Close => {
                    open.pop();
                }
                Tag::Open {
                    name,
                    attrs,
                    self_closing,
                } => {
                    let index = nodes.len();
                    nodes.push(Node {
                        name: name.rsplit(':').next().unwrap_or(name),
                        attrs,
                        parent: open.last().copied(),
                        children: Vec::new(),
                    });
                    if let Some(&parent) = open.last() {
                        nodes[parent].children.push(Child::Node(index));
                    }
                    if !self_closing {
                        open.push(index);
                    }
                }
            }
        }
        let mut ids = HashMap::new();
        let mut css = String::new();
        for (i, n) in nodes.iter().enumerate() {
            if let Some(id) = attr(&n.attrs, "id") {
                ids.entry(id.to_string()).or_insert(i);
            }
            if n.name == "style" {
                for c in &n.children {
                    if let Child::Text(t) = c {
                        css.push_str(t);
                        css.push('\n');
                    }
                }
            }
        }
        let (rules, unread_css) = style_sheet(&css);
        Doc {
            nodes,
            ids,
            rules,
            unread_css,
        }
    }

    /// The element `value` points to: `#id`, or `url(#id)`.
    fn target(&self, value: &str) -> Option<usize> {
        let v = value.trim();
        let v = match v.strip_prefix("url(") {
            Some(rest) => rest.split(')').next().unwrap_or(""),
            None => v,
        };
        let id = v.trim().trim_matches(['"', '\'']).strip_prefix('#')?;
        self.ids.get(id).copied()
    }

    fn child_nodes(&self, index: usize) -> impl Iterator<Item = usize> + '_ {
        self.nodes[index].children.iter().filter_map(|c| match c {
            Child::Node(i) => Some(*i),
            Child::Text(_) => None,
        })
    }

    /// An element's properties, the ones that win last: its attributes,
    /// then the style sheet rules that match it (the most specific last),
    /// then its `style` attribute.
    fn props(&self, index: usize) -> Props {
        let n = &self.nodes[index];
        let mut list: Vec<(String, String)> = n
            .attrs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        if !self.rules.is_empty() {
            let mut matched: Vec<&Rule> = self
                .rules
                .iter()
                .filter(|r| r.selector.matches(self, index))
                .collect();
            matched.sort_by_key(|r| (r.selector.specificity, r.order));
            for r in matched {
                list.extend(r.declarations.iter().cloned());
            }
        }
        if let Some(style) = attr(&n.attrs, "style") {
            list.extend(declarations(style));
        }
        Props(list)
    }
}

fn attr<'b>(attrs: &'b [(&str, String)], key: &str) -> Option<&'b str> {
    attrs
        .iter()
        .rev()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.as_str())
}

/// An element's properties, in the order they apply (the last one wins).
struct Props(Vec<(String, String)>);

impl Props {
    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// `href`, or the older `xlink:href`.
    fn href(&self) -> Option<&str> {
        self.get("href").or_else(|| self.get("xlink:href"))
    }
}

// --- Style sheets ---------------------------------------------------------------

/// `a: b; c: d` as pairs (`!important` dropped).
fn declarations(body: &str) -> Vec<(String, String)> {
    body.split(';')
        .filter_map(|decl| {
            let (k, v) = decl.split_once(':')?;
            let v = v.trim();
            let v = v.strip_suffix("!important").unwrap_or(v).trim();
            Some((k.trim().to_string(), v.to_string()))
        })
        .collect()
}

/// A type, an id, classes: `rect#a.b.c`.
#[derive(Default)]
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

struct Selector {
    /// The compounds from the left; `true`: the next one is a child of
    /// this one (`>`), else a descendant.
    parts: Vec<(Compound, bool)>,
    /// Ids, then classes, then types.
    specificity: (usize, usize, usize),
}

struct Rule {
    selector: Selector,
    declarations: Vec<(String, String)>,
    order: usize,
}

impl Compound {
    fn matches(&self, n: &Node) -> bool {
        self.tag.as_deref().is_none_or(|t| t == n.name)
            && self
                .id
                .as_deref()
                .is_none_or(|id| attr(&n.attrs, "id") == Some(id))
            && (self.classes.is_empty() || {
                let own = attr(&n.attrs, "class").unwrap_or("");
                self.classes
                    .iter()
                    .all(|c| own.split_whitespace().any(|o| o == c))
            })
    }
}

impl Selector {
    /// One selector (no comma). `None`: something that is not read (a
    /// pseudo class, an attribute, a sibling combinator).
    fn read(text: &str) -> Option<Selector> {
        let mut parts: Vec<(Compound, bool)> = Vec::new();
        let mut specificity = (0, 0, 0);
        let spaced = text.replace('>', " > ");
        let mut child = false;
        for token in spaced.split_whitespace() {
            if token == ">" {
                child = true;
                continue;
            }
            if let Some((_, linked)) = parts.last_mut() {
                *linked = child;
            }
            child = false;
            let mut c = Compound::default();
            let mut rest = token;
            let end = |s: &str| s.find(['.', '#']).unwrap_or(s.len());
            let first = end(rest);
            match &rest[..first] {
                "" | "*" => {}
                tag => {
                    c.tag = Some(tag.to_string());
                    specificity.2 += 1;
                }
            }
            rest = &rest[first..];
            while !rest.is_empty() {
                let kind = rest.as_bytes()[0];
                let len = end(&rest[1..]);
                let name = &rest[1..1 + len];
                if name.is_empty() {
                    return None;
                }
                if kind == b'#' {
                    c.id = Some(name.to_string());
                    specificity.0 += 1;
                } else {
                    c.classes.push(name.to_string());
                    specificity.1 += 1;
                }
                rest = &rest[1 + len..];
            }
            let plain = |s: &str| {
                s.chars()
                    .all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | '\\'))
            };
            if !c.tag.as_deref().is_none_or(plain)
                || !c.id.as_deref().is_none_or(plain)
                || !c.classes.iter().all(|s| plain(s))
            {
                return None;
            }
            parts.push((c, false));
        }
        (!parts.is_empty() && !child).then_some(Selector { parts, specificity })
    }

    fn matches(&self, doc: &Doc, index: usize) -> bool {
        fn at(doc: &Doc, parts: &[(Compound, bool)], index: usize) -> bool {
            let Some(((c, _), before)) = parts.split_last() else {
                return true;
            };
            if !c.matches(&doc.nodes[index]) {
                return false;
            }
            let Some((_, child)) = before.last() else {
                return true;
            };
            let mut up = doc.nodes[index].parent;
            while let Some(p) = up {
                if at(doc, before, p) {
                    return true;
                }
                if *child {
                    return false;
                }
                up = doc.nodes[p].parent;
            }
            false
        }
        at(doc, &self.parts, index)
    }
}

/// The rules of a style sheet, and whether some selectors were left out.
fn style_sheet(css: &str) -> (Vec<Rule>, bool) {
    let mut text = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        text.push_str(&rest[..start]);
        rest = rest[start..]
            .find("*/")
            .map_or("", |e| &rest[start + e + 2..]);
    }
    text.push_str(rest);
    let (mut rules, mut unread) = (Vec::new(), false);
    let mut rest = text.as_str();
    while let Some(open) = rest.find('{') {
        let selectors = rest[..open].trim();
        let body = &rest[open + 1..];
        if selectors.starts_with('@') {
            // An at-rule: its whole block, nested ones included.
            let mut depth = 1;
            let end = body
                .char_indices()
                .find(|(_, c)| {
                    depth += match *c {
                        '{' => 1,
                        '}' => -1,
                        _ => 0,
                    };
                    depth == 0
                })
                .map_or(body.len(), |(i, _)| i + 1);
            unread |= !selectors.starts_with("@charset") && !selectors.starts_with("@font-face");
            rest = &body[end..];
            continue;
        }
        let close = body.find('}').unwrap_or(body.len());
        let list = declarations(&body[..close]);
        for s in selectors.split(',') {
            match Selector::read(s.trim()) {
                Some(selector) => rules.push(Rule {
                    selector,
                    declarations: list.clone(),
                    order: rules.len(),
                }),
                None => unread = true,
            }
        }
        rest = body.get(close + 1..).unwrap_or("");
    }
    (rules, unread)
}

// --- Lengths --------------------------------------------------------------------

/// What relative lengths are measured against.
#[derive(Clone, Copy)]
struct Lengths {
    /// `1em`, user units.
    font_size: f64,
    /// The `viewBox`'s width and height (`100%` across and down).
    view: (f64, f64),
}

impl Lengths {
    /// A length in user units; `percent` is what `100%` is.
    fn read(&self, s: &str, percent: f64) -> Option<f64> {
        let s = s.trim();
        let unit_at = s
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_ascii_digit() || *c == '.')
            .map_or(0, |(i, _)| i + 1);
        let (number, unit) = s.split_at(unit_at);
        let v: f64 = number.trim().parse().ok()?;
        Some(match unit.trim() {
            "" | "px" => v,
            "pt" => v * 4. / 3.,
            "pc" => v * 16.,
            "mm" => v * 96. / 25.4,
            "cm" => v * 96. / 2.54,
            "in" => v * 96.,
            "em" | "rem" => v * self.font_size,
            "ex" => v * self.font_size / 2.,
            "%" => v / 100. * percent,
            _ => return None,
        })
    }

    fn x(&self, s: &str) -> Option<f64> {
        self.read(s, self.view.0)
    }

    fn y(&self, s: &str) -> Option<f64> {
        self.read(s, self.view.1)
    }

    /// A length in no direction (a radius, a stroke width).
    fn other(&self, s: &str) -> Option<f64> {
        let (w, h) = self.view;
        self.read(s, ((w * w + h * h) / 2.).sqrt())
    }
}

/// A length of the root `<svg>` (`px`, `pt` and unitless; a percentage is
/// of a container this document does not have).
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
fn viewport(props: &Props) -> (f64, f64, f64, f64, f32, f32) {
    let vb = props
        .get("viewBox")
        .map(number_list)
        .filter(|v| v.len() == 4);
    let w = props.get("width").and_then(length);
    let h = props.get("height").and_then(length);
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

/// `preserveAspectRatio`: where the content sits in its box
/// (`0`, `0.5` or `1` across and down), whether it covers it (`slice`),
/// whether it is stretched to it (`none`).
fn aspect(value: &str) -> ((f64, f64), bool, bool) {
    let v = value.trim();
    if v.starts_with("none") {
        return ((0.5, 0.5), false, true);
    }
    let side = |min: &str, max: &str| {
        if v.contains(min) {
            0.
        } else if v.contains(max) {
            1.
        } else {
            0.5
        }
    };
    (
        (side("xMin", "xMax"), side("YMin", "YMax")),
        v.contains("slice"),
        false,
    )
}

/// The transform that shows the box `from` (`x, y, w, h`) in the box `to`,
/// as `preserveAspectRatio` says.
fn fit_box(from: (f64, f64, f64, f64), to: (f64, f64, f64, f64), ratio: &str) -> [f64; 6] {
    let ((ax, ay), slice, stretch) = aspect(ratio);
    let (sx, sy) = (to.2 / from.2.max(1e-9), to.3 / from.3.max(1e-9));
    let (sx, sy) = match (stretch, slice) {
        (true, _) => (sx, sy),
        (false, false) => (sx.min(sy), sx.min(sy)),
        (false, true) => (sx.max(sy), sx.max(sy)),
    };
    [
        sx,
        0.,
        0.,
        sy,
        to.0 + (to.2 - from.2 * sx) * ax - from.0 * sx,
        to.1 + (to.3 - from.3 * sy) * ay - from.1 * sy,
    ]
}

/// How much a transform scales lengths (the square root of its area
/// factor).
fn scale_of(m: [f64; 6]) -> f64 {
    (m[0] * m[3] - m[1] * m[2]).abs().sqrt()
}

// --- Paint ----------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Paint {
    None,
    Color(Color),
    Current,
    /// `url(#id)` (a gradient), with the color after it if any.
    Server(String, Option<Color>),
}

/// What an element's children inherit.
#[derive(Clone, Debug)]
struct State {
    fill: Paint,
    stroke: Paint,
    stroke_width: f64,
    fill_opacity: f64,
    stroke_opacity: f64,
    round: bool,
    even_odd: bool,
    /// `clip-rule`, for the shapes of a `clipPath`.
    clip_even_odd: bool,
    dash: Option<(f64, f64)>,
    color: Color,
    /// `display: none`: the element and what is in it are left out.
    display_none: bool,
    /// `visibility: hidden`: a child may be visible again.
    hidden: bool,
    transform: [f64; 6],
    font_size: f64,
    family: FontFamily,
    weight: u16,
    italic: bool,
    spacing: f64,
    anchor: Anchor,
    baseline: Baseline,
    /// `xml:space="preserve"`: spaces are kept as written.
    keep_spaces: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            fill: Paint::Color(Color::BLACK),
            stroke: Paint::None,
            stroke_width: 1.,
            fill_opacity: 1.,
            stroke_opacity: 1.,
            round: false,
            even_odd: false,
            clip_even_odd: false,
            dash: None,
            color: Color::current(),
            display_none: false,
            hidden: false,
            transform: IDENTITY,
            font_size: 16.,
            family: FontFamily::Serif,
            weight: 400,
            italic: false,
            spacing: 0.,
            anchor: Anchor::Start,
            baseline: Baseline::Alphabetic,
            keep_spaces: false,
        }
    }
}

/// `0.5` or `50%`.
fn fraction(v: &str) -> Option<f64> {
    let v = v.trim();
    match v.strip_suffix('%') {
        Some(p) => p.trim().parse::<f64>().ok().map(|p| p / 100.),
        None => v.parse().ok(),
    }
}

/// A CSS `font-family` list as the family Prism draws: its first entry, a
/// generic family or a font by name.
fn family(v: &str) -> FontFamily {
    let first = v.split(',').next().unwrap_or("").trim();
    let name = first.trim_matches(['"', '\'']).trim();
    match name.to_ascii_lowercase().as_str() {
        "" | "sans-serif" | "system-ui" | "-apple-system" | "blinkmacsystemfont"
        | "ui-sans-serif" | "cursive" | "fantasy" | "inherit" => FontFamily::System,
        "monospace" | "ui-monospace" => FontFamily::Monospace,
        "serif" | "ui-serif" => FontFamily::Serif,
        "ui-rounded" => FontFamily::Rounded,
        _ => FontFamily::Named(name.to_string()),
    }
}

impl State {
    /// This element's state: the parent's, with the element's own
    /// properties applied (its transform too).
    fn inherit(&self, props: &Props, lengths: &Lengths) -> State {
        let mut s = self.clone();
        // `display` does not inherit: it is this element's only.
        s.display_none = false;
        // `color` first: `currentColor` in this element refers to it.
        if let Some(v) = props.get("color")
            && let Some(Paint::Color(c)) = paint(v, self.color)
        {
            s.color = c;
        }
        // `font-size` next: this element's `em` lengths are of it.
        if let Some(v) = props.get("font-size") {
            let parent = Lengths {
                font_size: self.font_size,
                ..*lengths
            };
            s.font_size = match v.trim() {
                "xx-small" => 9.,
                "x-small" => 10.,
                "small" => 13.,
                "medium" => 16.,
                "large" => 18.,
                "x-large" => 24.,
                "xx-large" => 32.,
                "smaller" => self.font_size / 1.2,
                "larger" => self.font_size * 1.2,
                v => parent
                    .read(v, self.font_size)
                    .filter(|v| *v >= 0.)
                    .unwrap_or(self.font_size),
            };
        }
        let lengths = Lengths {
            font_size: s.font_size,
            ..*lengths
        };
        for (k, v) in &props.0 {
            let v = v.trim();
            match k.as_str() {
                "fill" => s.fill = paint(v, s.color).unwrap_or(s.fill),
                "stroke" => s.stroke = paint(v, s.color).unwrap_or(s.stroke),
                "stroke-width" => s.stroke_width = lengths.other(v).unwrap_or(s.stroke_width),
                "fill-opacity" => s.fill_opacity = fraction(v).unwrap_or(s.fill_opacity),
                "stroke-opacity" => s.stroke_opacity = fraction(v).unwrap_or(s.stroke_opacity),
                "stroke-linecap" | "stroke-linejoin" => s.round = v == "round",
                "fill-rule" => s.even_odd = v == "evenodd",
                "clip-rule" => s.clip_even_odd = v == "evenodd",
                "stroke-dasharray" => {
                    let d: Vec<f64> = v
                        .split(|c: char| c == ',' || c.is_whitespace())
                        .filter(|p| !p.is_empty())
                        .filter_map(|p| lengths.other(p))
                        .collect();
                    s.dash = match d.as_slice() {
                        [] => None,
                        [a] => Some((*a, *a)),
                        [a, b, ..] => Some((*a, *b)),
                    }
                }
                "display" => s.display_none = v == "none",
                "visibility" => s.hidden = v == "hidden" || v == "collapse",
                "font-family" => s.family = family(v),
                "font-weight" => {
                    s.weight = match v {
                        "normal" => 400,
                        "bold" => 700,
                        "bolder" => (self.weight + 300).min(900),
                        "lighter" => self.weight.saturating_sub(300).max(100),
                        v => v
                            .parse::<f64>()
                            .map_or(s.weight, |w| w.clamp(1., 1000.) as u16),
                    }
                }
                "font-style" => s.italic = v == "italic" || v.starts_with("oblique"),
                "letter-spacing" => s.spacing = lengths.other(v).unwrap_or(0.),
                "text-anchor" => {
                    s.anchor = match v {
                        "middle" => Anchor::Middle,
                        "end" => Anchor::End,
                        _ => Anchor::Start,
                    }
                }
                "dominant-baseline" | "alignment-baseline" => {
                    s.baseline = match v {
                        "middle" => Baseline::Middle,
                        "central" => Baseline::Central,
                        "hanging" | "text-before-edge" | "text-top" => Baseline::Hanging,
                        _ => Baseline::Alphabetic,
                    }
                }
                "xml:space" => s.keep_spaces = v == "preserve",
                "white-space" => s.keep_spaces = v.starts_with("pre"),
                _ => {}
            }
        }
        s.transform = multiply(self.transform, own_transform(props));
        s
    }

    fn lengths(&self, view: (f64, f64)) -> Lengths {
        Lengths {
            font_size: self.font_size,
            view,
        }
    }

    /// A plain paint as a color at `alpha` (`None`: nothing is painted, or
    /// it is a paint server).
    fn resolve(&self, p: &Paint, alpha: f64) -> Option<Color> {
        let c = match p {
            Paint::None | Paint::Server(..) => return None,
            Paint::Color(c) => *c,
            Paint::Current => self.color,
        };
        let a = (c.a as f64 * alpha) as f32;
        (a > 0.).then_some(Color { a, ..c })
    }

    /// How much the transform scales lengths.
    fn transform_scale(&self) -> f64 {
        scale_of(self.transform)
    }
}

/// A paint value; `None` for something unreadable (keeps the inherited one).
fn paint(v: &str, current: Color) -> Option<Paint> {
    let v = v.trim();
    if v == "none" || v == "transparent" {
        return Some(Paint::None);
    }
    if v.eq_ignore_ascii_case("currentColor") {
        return Some(Paint::Current);
    }
    // `url(#gradient) red`: the paint server, then the color without it.
    if let Some(rest) = v.strip_prefix("url(") {
        let (target, fallback) = rest.split_once(')').unwrap_or((rest, ""));
        let id = target
            .trim()
            .trim_matches(['"', '\''])
            .trim_start_matches('#');
        let fallback = match fallback.trim() {
            "" | "none" => None,
            f if f.eq_ignore_ascii_case("currentColor") => Some(current),
            f => color(f),
        };
        return Some(Paint::Server(id.to_string(), fallback));
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

fn own_transform(props: &Props) -> [f64; 6] {
    props.get("transform").map_or(IDENTITY, transform_list)
}

/// A `transform` attribute's list, left to right (the CSS property too:
/// its `px` and `deg` are the attribute's units).
fn transform_list(s: &str) -> [f64; 6] {
    let s = s.replace("px", "").replace("deg", "");
    let mut m = IDENTITY;
    let mut rest = s.as_str();
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

// --- Drawing --------------------------------------------------------------------

/// `c` at `alpha` times its own opacity (both variants of an adaptive one).
fn faded(c: Color, alpha: f32) -> Color {
    Color {
        a: c.a * alpha,
        dark: c.dark.map(|[r, g, b, a]| [r, g, b, a * alpha]),
        ..c
    }
}

/// Multiplies `item`'s opacity by `alpha` when that looks the same as
/// drawing it alone at that opacity (one paint, no overlap in itself).
fn fade(item: &mut Item, alpha: f32) -> bool {
    match item {
        Item::Path(p) if p.fill.is_some() != p.stroke.is_some() => {
            p.map_colors(&mut |c| faded(c, alpha));
        }
        Item::Path(_) => return false,
        Item::Text(t) => {
            for r in &mut t.runs {
                r.fill = faded(r.fill, alpha);
            }
        }
        Item::Image(i) => i.opacity *= alpha,
        Item::Group(g) => g.opacity *= alpha,
    }
    true
}

/// What `url(#id)` paints with.
enum Server {
    Gradient(Gradient),
    Color(Color),
    /// Nothing that is drawn: the color after it, if any.
    Missing,
}

/// What a filter does, as far as it is drawn.
#[derive(Default)]
struct Effects {
    shadows: Vec<Shadow>,
    /// A blur of the element itself, points.
    blur: f32,
    /// A blurred copy under the element (a glow), points.
    glow: Option<f32>,
    /// The filter's region (`x, y, width, height` in the element's
    /// coordinates): nothing of it shows outside.
    region: Option<[f64; 4]>,
}

/// The lines of a `<text>` being read.
struct Lines {
    transform: crate::shape::Matrix,
    lines: Vec<Text>,
    /// The `x` a line without one starts at.
    origin_x: f64,
    /// The current line's `y`, and how far below it the next run sits.
    y: f64,
    rise: f64,
    /// Room before the next run.
    dx: f64,
    /// The last character was a space (or the line just started): the
    /// next space is dropped.
    after_space: bool,
}

impl Lines {
    fn start(&mut self, x: f64, y: f64, state: &State) {
        self.lines.push(Text {
            transform: self.transform,
            x: x as f32,
            y: y as f32,
            anchor: state.anchor,
            baseline: state.baseline,
            runs: Vec::new(),
        });
        (self.y, self.rise, self.after_space) = (y, 0., true);
    }

    fn push(&mut self, text: &str, state: &State, fill: Color) {
        let mut s = String::with_capacity(text.len());
        if state.keep_spaces {
            s.extend(text.chars().map(|c| if c.is_control() { ' ' } else { c }));
        } else {
            for c in text.chars() {
                if c.is_whitespace() && c != '\u{a0}' {
                    if !self.after_space {
                        s.push(' ');
                    }
                    self.after_space = true;
                } else {
                    s.push(c);
                    self.after_space = false;
                }
            }
        }
        let Some(line) = self.lines.last_mut() else {
            return;
        };
        if s.is_empty() {
            return;
        }
        // Room before a line's first run moves the whole line, however it
        // is anchored (as browsers do).
        if line.runs.is_empty() {
            line.x += std::mem::take(&mut self.dx) as f32;
        }
        line.runs.push(TextRun {
            text: s,
            family: state.family.clone(),
            size: state.font_size as f32,
            weight: state.weight,
            italic: state.italic,
            spacing: state.spacing as f32,
            fill,
            dx: std::mem::take(&mut self.dx) as f32,
            dy: self.rise as f32,
        });
    }
}

struct Walker<'a> {
    doc: &'a Doc<'a>,
    /// The `viewBox`'s width and height.
    view: (f64, f64),
    /// User units of the root to fractions of the frame.
    to_frame: [f64; 6],
    /// Points per user unit of the root.
    unit: f64,
    /// What is not drawn, with how many times it was met.
    skipped: BTreeMap<String, usize>,
    /// The `use` targets and masks being drawn (one in itself is left out).
    using: Vec<usize>,
    depth: usize,
}

impl<'a> Walker<'a> {
    fn skip(&mut self, what: impl Into<String>) {
        *self.skipped.entry(what.into()).or_default() += 1;
    }

    /// `m` (to user units of the root) as a transform to the frame.
    fn frame(&self, m: [f64; 6]) -> crate::shape::Matrix {
        multiply(self.to_frame, m).map(|v| v as f32)
    }

    fn segments(&self, segs: &[Seg], m: [f64; 6]) -> Vec<Segment> {
        let m = multiply(self.to_frame, m);
        let pt = |x: f64, y: f64| {
            let (x, y) = apply(m, x, y);
            (x as f32, y as f32)
        };
        segs.iter()
            .map(|s| match *s {
                Seg::Move(x, y) => {
                    let (x, y) = pt(x, y);
                    Segment::Move(x, y)
                }
                Seg::Line(x, y) => {
                    let (x, y) = pt(x, y);
                    Segment::Line(x, y)
                }
                Seg::Cubic(a, b, c, d, e, f) => {
                    let ((a, b), (c, d), (e, f)) = (pt(a, b), pt(c, d), pt(e, f));
                    Segment::Cubic(a, b, c, d, e, f)
                }
                Seg::Close => Segment::Close,
            })
            .collect()
    }

    fn children(&mut self, index: usize, state: &State, out: &mut Vec<Item>) {
        let doc = self.doc;
        for child in doc.child_nodes(index) {
            self.element(child, state, out);
        }
    }

    fn element(&mut self, index: usize, parent: &State, out: &mut Vec<Item>) {
        if self.depth > 64 {
            return;
        }
        let doc = self.doc;
        let name = doc.nodes[index].name;
        let props = doc.props(index);
        let lengths = parent.lengths(self.view);
        let mut state = parent.inherit(&props, &lengths);
        if state.display_none {
            return;
        }
        let mut items = Vec::new();
        self.depth += 1;
        match name {
            "g" | "a" | "switch" => self.children(index, &state, &mut items),
            "svg" => {
                let l = state.lengths(self.view);
                let x = props.get("x").and_then(|v| l.x(v)).unwrap_or(0.);
                let y = props.get("y").and_then(|v| l.y(v)).unwrap_or(0.);
                let w = props.get("width").and_then(|v| l.x(v));
                let h = props.get("height").and_then(|v| l.y(v));
                let view_box = props
                    .get("viewBox")
                    .map(number_list)
                    .filter(|v| v.len() == 4);
                let m = match (view_box, w, h) {
                    (Some(v), Some(w), Some(h)) => fit_box(
                        (v[0], v[1], v[2], v[3]),
                        (x, y, w, h),
                        props.get("preserveAspectRatio").unwrap_or(""),
                    ),
                    _ => [1., 0., 0., 1., x, y],
                };
                state.transform = multiply(state.transform, m);
                self.children(index, &state, &mut items);
            }
            "use" => self.use_element(&props, &state, &mut items),
            "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" => {
                if !state.hidden
                    && let Some(p) = self.shape(name, &props, &state)
                {
                    items.push(Item::Path(p));
                }
            }
            "image" => {
                if !state.hidden {
                    self.image(&props, &state, &mut items);
                }
            }
            "text" => self.text(index, &props, &state, &mut items),
            // Drawn where they are referred to, or not at all.
            "defs" | "clipPath" | "mask" | "linearGradient" | "radialGradient" | "filter"
            | "symbol" | "pattern" | "marker" | "stop" | "style" | "script" | "title" | "desc"
            | "metadata" => {}
            other => self.skip(format!("<{other}>")),
        }
        self.depth -= 1;
        self.wrap(index, &props, &state, items, out);
    }

    /// Adds `items` (what element `index` draws) to `out`, in a group when
    /// the element has an opacity, a clip, a mask or a filter.
    fn wrap(
        &mut self,
        index: usize,
        props: &Props,
        state: &State,
        mut items: Vec<Item>,
        out: &mut Vec<Item>,
    ) {
        let opacity = props
            .get("opacity")
            .and_then(fraction)
            .unwrap_or(1.)
            .clamp(0., 1.);
        if opacity <= 0. || items.is_empty() {
            return;
        }
        let set = |v: &&str| !matches!(v.trim(), "" | "none");
        let named = |kind: &str, v: &str| match v.trim().strip_prefix("url(") {
            Some(rest) => format!(
                "<{kind} id=\"{}\">",
                rest.split(')')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches(['"', '\''])
                    .trim_start_matches('#')
            ),
            None => format!("{kind}: {}", v.trim()),
        };
        let mut g = Group {
            opacity: opacity as f32,
            ..Group::default()
        };
        let mut glow = None;
        let mut region = None;
        if let Some(v) = props.get("filter").filter(set) {
            match self.filter(v, index, state) {
                Some(fx) => {
                    (g.shadows, g.blur, glow) = (fx.shadows, fx.blur, fx.glow);
                    region = fx.region.map(|[x, y, w, h]| Clip {
                        segments: self.segments(
                            &[
                                Seg::Move(x, y),
                                Seg::Line(x + w, y),
                                Seg::Line(x + w, y + h),
                                Seg::Line(x, y + h),
                                Seg::Close,
                            ],
                            state.transform,
                        ),
                        even_odd: false,
                    });
                }
                None => self.skip(named("filter", v)),
            }
        }
        if let Some(v) = props.get("clip-path").filter(set) {
            match self.clip(v, index, state) {
                // Nothing is inside it.
                Some(c) if c.segments.is_empty() => return,
                Some(c) => g.clip = Some(c),
                None => self.skip(named("clipPath", v)),
            }
        }
        if let Some(blur) = glow.take() {
            // The element blurred, under itself.
            items.insert(
                0,
                Item::Group(Group {
                    blur,
                    items: items.clone(),
                    ..Group::default()
                }),
            );
        }
        match (region, &g.clip) {
            // Cut to the filter's region, then to the clip path.
            (Some(region), Some(_)) => {
                items = vec![Item::Group(Group {
                    clip: Some(region),
                    shadows: std::mem::take(&mut g.shadows),
                    blur: std::mem::take(&mut g.blur),
                    items,
                    ..Group::default()
                })];
            }
            (Some(region), None) => g.clip = Some(region),
            (None, _) => {}
        }
        if let Some(v) = props.get("mask").filter(set) {
            match self.mask(v, index, state) {
                Some(m) => g.mask = Some(m),
                None => self.skip(named("mask", v)),
            }
        }
        let plain = g.clip.is_none() && g.mask.is_none() && g.shadows.is_empty() && g.blur == 0.;
        if plain && (g.opacity >= 1. || (items.len() == 1 && fade(&mut items[0], g.opacity))) {
            out.append(&mut items);
            return;
        }
        g.items = items;
        out.push(Item::Group(g));
    }

    fn use_element(&mut self, props: &Props, state: &State, out: &mut Vec<Item>) {
        let doc = self.doc;
        let Some(target) = props.href().and_then(|h| doc.target(h)) else {
            self.skip("<use> of an element that is not in the document");
            return;
        };
        if self.using.contains(&target) || self.using.len() > 16 {
            return;
        }
        let l = state.lengths(self.view);
        let x = props.get("x").and_then(|v| l.x(v)).unwrap_or(0.);
        let y = props.get("y").and_then(|v| l.y(v)).unwrap_or(0.);
        let mut placed = state.clone();
        placed.transform = multiply(state.transform, [1., 0., 0., 1., x, y]);
        self.using.push(target);
        if matches!(doc.nodes[target].name, "symbol" | "svg") {
            let own = doc.props(target);
            let mut inner = placed.inherit(&own, &l);
            if let Some(v) = own.get("viewBox").map(number_list).filter(|v| v.len() == 4) {
                let side = |key: &str, all: f64| {
                    props
                        .get(key)
                        .or_else(|| own.get(key))
                        .and_then(|v| l.read(v, all))
                        .unwrap_or(all)
                };
                let (w, h) = (side("width", self.view.0), side("height", self.view.1));
                inner.transform = multiply(
                    inner.transform,
                    fit_box(
                        (v[0], v[1], v[2], v[3]),
                        (0., 0., w, h),
                        own.get("preserveAspectRatio").unwrap_or(""),
                    ),
                );
            }
            let mut items = Vec::new();
            self.children(target, &inner, &mut items);
            self.wrap(target, &own, &inner, items, out);
        } else {
            self.element(target, &placed, out);
        }
        self.using.pop();
    }

    fn shape(&mut self, name: &str, props: &Props, state: &State) -> Option<Path> {
        let segs = geometry(name, props, &state.lengths(self.view))?;
        if ["marker", "marker-start", "marker-mid", "marker-end"]
            .iter()
            .any(|k| props.get(k).is_some_and(|v| v.trim() != "none"))
        {
            self.skip("markers");
        }
        let bbox = bounds(&segs);
        let mut p = Path::new();
        p.segments = self.segments(&segs, state.transform);
        p.even_odd = state.even_odd;
        (p.fill, p.fill_gradient) = self.paint(&state.fill, state.fill_opacity, bbox, state);
        let scale = state.transform_scale() * self.unit;
        if state.stroke_width > 0. {
            (p.stroke, p.stroke_gradient) =
                self.paint(&state.stroke, state.stroke_opacity, bbox, state);
            if p.stroke.is_some() {
                p.width = (state.stroke_width * scale) as f32;
            }
        }
        p.round = state.round;
        p.dash = state
            .dash
            .map(|(d, g)| ((d * scale) as f32, (g * scale) as f32));
        (p.fill.is_some() || p.stroke.is_some()).then_some(p)
    }

    /// A fill or a stroke: its flat color, and its gradient if it has one.
    fn paint(
        &mut self,
        paint: &Paint,
        alpha: f64,
        bbox: Option<[f64; 4]>,
        state: &State,
    ) -> (Option<Color>, Option<Gradient>) {
        let Paint::Server(id, fallback) = paint else {
            return (state.resolve(paint, alpha), None);
        };
        let flat = |c: Color| Some(faded(c, alpha as f32)).filter(|c| c.a > 0.);
        match self.gradient(id, bbox, state, alpha) {
            Server::Gradient(g) => {
                let color = fallback.and_then(flat).unwrap_or(g.color_at(0.5));
                (Some(color), Some(g))
            }
            Server::Color(c) => (Some(c).filter(|c| c.a > 0.), None),
            Server::Missing => (fallback.and_then(flat), None),
        }
    }

    /// The gradient `id`, for a shape with the box `bbox` (its own
    /// coordinates) drawn in `state`, at `alpha`.
    fn gradient(&mut self, id: &str, bbox: Option<[f64; 4]>, state: &State, alpha: f64) -> Server {
        let doc = self.doc;
        let Some(&first) = doc.ids.get(id) else {
            self.skip(format!("url(#{id}) that is not in the document"));
            return Server::Missing;
        };
        let kind = doc.nodes[first].name;
        if kind != "linearGradient" && kind != "radialGradient" {
            self.skip(format!("<{kind}> as a paint"));
            return Server::Missing;
        }
        // The gradients this one takes what it does not say from.
        let href = |i: usize| {
            let a = &doc.nodes[i].attrs;
            attr(a, "href").or_else(|| attr(a, "xlink:href"))
        };
        let mut chain = vec![first];
        while chain.len() < 8
            && let Some(next) = href(chain[chain.len() - 1]).and_then(|h| doc.target(h))
            && !chain.contains(&next)
        {
            chain.push(next);
        }
        let get = |key: &str| chain.iter().find_map(|i| attr(&doc.nodes[*i].attrs, key));
        let with_stops = chain
            .iter()
            .find(|i| doc.child_nodes(**i).any(|c| doc.nodes[c].name == "stop"));
        let mut stops: Vec<Stop> = Vec::new();
        for c in with_stops.into_iter().flat_map(|i| doc.child_nodes(*i)) {
            if doc.nodes[c].name != "stop" {
                continue;
            }
            let p = doc.props(c);
            let offset = p
                .get("offset")
                .and_then(fraction)
                .unwrap_or(0.)
                .clamp(0., 1.)
                .max(stops.last().map_or(0., |s| s.offset as f64));
            let color = match p.get("stop-color").and_then(|v| paint(v, state.color)) {
                Some(Paint::Color(c)) => c,
                Some(Paint::Current) => state.color,
                Some(Paint::None) => Color::TRANSPARENT,
                Some(Paint::Server(..)) | None => Color::BLACK,
            };
            let opacity = p.get("stop-opacity").and_then(fraction).unwrap_or(1.);
            stops.push(Stop {
                offset: offset as f32,
                color: faded(color, (opacity.clamp(0., 1.) * alpha) as f32),
            });
        }
        let Some(last) = stops.last().copied() else {
            return Server::Missing;
        };
        if stops.len() == 1 {
            return Server::Color(last.color);
        }
        let user = get("gradientUnits") == Some("userSpaceOnUse");
        let l = state.lengths(self.view);
        let coord = |key: &str, default: &str, axis: u8| -> f64 {
            let v = get(key).unwrap_or(default);
            if user {
                match axis {
                    0 => l.x(v),
                    1 => l.y(v),
                    _ => l.other(v),
                }
                .unwrap_or(0.)
            } else {
                fraction(v).unwrap_or(0.)
            }
        };
        let shape = if kind == "linearGradient" {
            let (x1, y1) = (coord("x1", "0%", 0), coord("y1", "0%", 1));
            let (x2, y2) = (coord("x2", "100%", 0), coord("y2", "0%", 1));
            if (x1, y1) == (x2, y2) {
                return Server::Color(last.color);
            }
            GradientShape::Linear {
                x1: x1 as f32,
                y1: y1 as f32,
                x2: x2 as f32,
                y2: y2 as f32,
            }
        } else {
            let (cx, cy, r) = (
                coord("cx", "50%", 0),
                coord("cy", "50%", 1),
                coord("r", "50%", 2),
            );
            if r <= 0. {
                return Server::Color(last.color);
            }
            let fx = get("fx").map_or(cx, |_| coord("fx", "50%", 0));
            let fy = get("fy").map_or(cy, |_| coord("fy", "50%", 1));
            GradientShape::Radial {
                cx: cx as f32,
                cy: cy as f32,
                r: r as f32,
                fx: fx as f32,
                fy: fy as f32,
            }
        };
        let mut m = state.transform;
        if !user {
            match bbox {
                Some([x, y, w, h]) if w > 0. && h > 0. => m = multiply(m, [w, 0., 0., h, x, y]),
                // A box without an area has no gradient across it.
                _ => return Server::Missing,
            }
        }
        if let Some(t) = get("gradientTransform") {
            m = multiply(m, transform_list(t));
        }
        Server::Gradient(Gradient {
            shape,
            stops,
            spread: match get("spreadMethod") {
                Some("reflect") => Spread::Reflect,
                Some("repeat") => Spread::Repeat,
                _ => Spread::Pad,
            },
            transform: self.frame(m),
        })
    }

    /// The box of what element `index` draws, in its own coordinates
    /// (text is not measured, so it is left out).
    fn bbox(&self, index: usize, lengths: &Lengths, depth: usize) -> Option<[f64; 4]> {
        if depth > 16 {
            return None;
        }
        let doc = self.doc;
        let props = doc.props(index);
        let union = |a: Option<[f64; 4]>, b: Option<[f64; 4]>| match (a, b) {
            (Some(a), Some(b)) => {
                let (x, y) = (a[0].min(b[0]), a[1].min(b[1]));
                let (r, bt) = (
                    (a[0] + a[2]).max(b[0] + b[2]),
                    (a[1] + a[3]).max(b[1] + b[3]),
                );
                Some([x, y, r - x, bt - y])
            }
            (a, b) => a.or(b),
        };
        // `b` (in a child's coordinates) through the child's transform.
        let through = |b: [f64; 4], m: [f64; 6]| {
            let corners = [
                apply(m, b[0], b[1]),
                apply(m, b[0] + b[2], b[1]),
                apply(m, b[0], b[1] + b[3]),
                apply(m, b[0] + b[2], b[1] + b[3]),
            ];
            let min =
                |f: fn(&(f64, f64)) -> f64| corners.iter().map(f).fold(f64::INFINITY, f64::min);
            let max =
                |f: fn(&(f64, f64)) -> f64| corners.iter().map(f).fold(f64::NEG_INFINITY, f64::max);
            let (x, y) = (min(|c| c.0), min(|c| c.1));
            [x, y, max(|c| c.0) - x, max(|c| c.1) - y]
        };
        match doc.nodes[index].name {
            "g" | "a" | "svg" | "switch" | "symbol" => {
                doc.child_nodes(index).fold(None, |all, c| {
                    let m = own_transform(&doc.props(c));
                    union(all, self.bbox(c, lengths, depth + 1).map(|b| through(b, m)))
                })
            }
            "use" => {
                let target = props.href().and_then(|h| doc.target(h))?;
                let x = props.get("x").and_then(|v| lengths.x(v)).unwrap_or(0.);
                let y = props.get("y").and_then(|v| lengths.y(v)).unwrap_or(0.);
                let m = multiply([1., 0., 0., 1., x, y], own_transform(&doc.props(target)));
                self.bbox(target, lengths, depth + 1).map(|b| through(b, m))
            }
            "image" => {
                let x = props.get("x").and_then(|v| lengths.x(v)).unwrap_or(0.);
                let y = props.get("y").and_then(|v| lengths.y(v)).unwrap_or(0.);
                let w = props.get("width").and_then(|v| lengths.x(v))?;
                let h = props.get("height").and_then(|v| lengths.y(v))?;
                Some([x, y, w, h])
            }
            name => bounds(&geometry(name, &props, lengths)?),
        }
    }

    /// The outline `clip-path` (`url(#id)`) cuts element `index` to.
    fn clip(&mut self, value: &str, index: usize, state: &State) -> Option<Clip> {
        let doc = self.doc;
        let node = doc
            .target(value)
            .filter(|i| doc.nodes[*i].name == "clipPath")?;
        let props = doc.props(node);
        let lengths = state.lengths(self.view);
        let mut base = multiply(state.transform, own_transform(&props));
        if props.get("clipPathUnits") == Some("objectBoundingBox") {
            let [x, y, w, h] = self.bbox(index, &lengths, 0)?;
            base = multiply(base, [w, 0., 0., h, x, y]);
        }
        let inherited = State::default().inherit(&props, &lengths);
        let mut clip = Clip::default();
        for c in doc.child_nodes(node) {
            let mut shape = c;
            let mut own = doc.props(c);
            let mut m = multiply(base, own_transform(&own));
            if doc.nodes[c].name == "use" {
                let Some(target) = own.href().and_then(|h| doc.target(h)) else {
                    continue;
                };
                let x = own.get("x").and_then(|v| lengths.x(v)).unwrap_or(0.);
                let y = own.get("y").and_then(|v| lengths.y(v)).unwrap_or(0.);
                own = doc.props(target);
                m = multiply(multiply(m, [1., 0., 0., 1., x, y]), own_transform(&own));
                shape = target;
            }
            let st = inherited.inherit(&own, &lengths);
            if st.display_none || st.hidden {
                continue;
            }
            let name = doc.nodes[shape].name;
            if name == "text" {
                self.skip("<text> in a <clipPath>");
                continue;
            }
            if let Some(segs) = geometry(name, &own, &lengths) {
                clip.segments.extend(self.segments(&segs, m));
                clip.even_odd |= st.clip_even_odd;
            }
        }
        Some(clip)
    }

    /// What `mask` (`url(#id)`) shows element `index` through.
    fn mask(&mut self, value: &str, index: usize, state: &State) -> Option<Mask> {
        let doc = self.doc;
        let node = doc.target(value).filter(|i| doc.nodes[*i].name == "mask")?;
        if self.using.contains(&node) {
            return None;
        }
        let props = doc.props(node);
        let lengths = state.lengths(self.view);
        let mut transform = state.transform;
        if props.get("maskContentUnits") == Some("objectBoundingBox") {
            let [x, y, w, h] = self.bbox(index, &lengths, 0)?;
            transform = multiply(transform, [w, 0., 0., h, x, y]);
        }
        // Its content inherits its properties; a mask has no transform.
        let mut inner = State::default().inherit(&props, &lengths);
        inner.transform = transform;
        let mut items = Vec::new();
        self.using.push(node);
        self.children(node, &inner, &mut items);
        self.using.pop();
        Some(Mask {
            luminance: props.get("mask-type").is_none_or(|v| v.trim() != "alpha"),
            items,
        })
    }

    /// What `filter` draws, when it is shadows, a blur or a glow.
    fn filter(&mut self, value: &str, index: usize, state: &State) -> Option<Effects> {
        let scale = state.transform_scale() * self.unit;
        let m = state.transform;
        let unit = self.unit;
        let lengths = state.lengths(self.view);
        let shadow = |dx: f64, dy: f64, blur: f64, color: Color| Shadow {
            dx: ((m[0] * dx + m[2] * dy) * unit) as f32,
            dy: ((m[1] * dx + m[3] * dy) * unit) as f32,
            blur: (blur.max(0.) * scale) as f32,
            color,
        };
        let mut fx = Effects::default();
        let v = value.trim();
        if !v.starts_with("url(") {
            // CSS filter functions: `drop-shadow(0 4px 8px #0004) blur(2px)`.
            let mut rest = v;
            while !rest.is_empty() {
                let open = rest.find('(')?;
                let mut depth = 0;
                let close = rest.char_indices().find(|(_, c)| {
                    depth += match *c {
                        '(' => 1,
                        ')' => -1,
                        _ => 0,
                    };
                    depth == 0 && *c == ')'
                })?;
                let args = rest[open + 1..close.0].trim();
                match rest[..open].trim() {
                    "blur" => fx.blur = (lengths.other(args)?.max(0.) * scale) as f32,
                    "drop-shadow" => {
                        // Lengths, and a color before or after them.
                        let mut parts: Vec<String> = Vec::new();
                        let mut depth = 0;
                        let mut word = String::new();
                        for c in args.chars() {
                            depth += match c {
                                '(' => 1,
                                ')' => -1,
                                _ => 0,
                            };
                            if c.is_whitespace() && depth == 0 {
                                parts.extend((!word.is_empty()).then(|| std::mem::take(&mut word)));
                            } else {
                                word.push(c);
                            }
                        }
                        parts.extend((!word.is_empty()).then_some(word));
                        let sizes: Vec<f64> =
                            parts.iter().filter_map(|p| lengths.other(p)).collect();
                        let tint = parts
                            .iter()
                            .find(|p| lengths.other(p).is_none())
                            .and_then(|p| color(p))
                            .unwrap_or(state.color);
                        let (&dx, &dy) = (sizes.first()?, sizes.get(1)?);
                        fx.shadows
                            .push(shadow(dx, dy, sizes.get(2).copied().unwrap_or(0.), tint));
                    }
                    _ => return None,
                }
                rest = rest[close.0 + 1..].trim_start();
            }
            return Some(fx);
        }
        let doc = self.doc;
        let node = doc.target(v).filter(|i| doc.nodes[*i].name == "filter")?;
        let own = doc.props(node);
        if own.get("primitiveUnits") == Some("objectBoundingBox") {
            return None;
        }
        // Its region: a tenth around the element's box unless it says.
        fx.region = if own.get("filterUnits") == Some("userSpaceOnUse") {
            match (
                own.get("x").and_then(|v| lengths.x(v)),
                own.get("y").and_then(|v| lengths.y(v)),
                own.get("width").and_then(|v| lengths.x(v)),
                own.get("height").and_then(|v| lengths.y(v)),
            ) {
                (Some(x), Some(y), Some(w), Some(h)) => Some([x, y, w, h]),
                _ => None,
            }
        } else {
            let part = |key: &str, default: f64| own.get(key).and_then(fraction).unwrap_or(default);
            self.bbox(index, &lengths, 0).map(|[x, y, w, h]| {
                [
                    x + part("x", -0.1) * w,
                    y + part("y", -0.1) * h,
                    part("width", 1.2) * w,
                    part("height", 1.2) * h,
                ]
            })
        };
        let number = |p: &Props, key: &str, default: f64| {
            p.get(key)
                .map(number_list)
                .filter(|v| !v.is_empty())
                .map_or(default, |v| v.iter().sum::<f64>() / v.len() as f64)
        };
        let flood = |p: &Props| {
            let c = match p.get("flood-color").and_then(|v| paint(v, state.color)) {
                Some(Paint::Color(c)) => c,
                Some(Paint::Current) => state.color,
                _ => Color::BLACK,
            };
            let opacity = p.get("flood-opacity").and_then(fraction).unwrap_or(1.);
            faded(c, opacity.clamp(0., 1.) as f32)
        };
        // What the last primitive made: the element itself, its shape
        // (what a shadow is made of), or something else.
        #[derive(PartialEq)]
        enum Made {
            Source,
            Shape,
            Blurred,
            Other,
        }
        let mut made = Made::Source;
        // The shadow being made: its offset, blur, color and opacity.
        let (mut offset, mut sigma, mut tint, mut alpha) = (None, None, None, 1.);
        let mut source_drawn = false;
        let mut blurred = None;
        let primitives: Vec<usize> = doc.child_nodes(node).collect();
        for p in primitives {
            let name = doc.nodes[p].name;
            let props = doc.props(p);
            let input = props.get("in").unwrap_or("");
            let from_shape =
                input == "SourceAlpha" || (input != "SourceGraphic" && made == Made::Shape);
            let mut finish = false;
            match name {
                "feDropShadow" => {
                    fx.shadows.push(shadow(
                        number(&props, "dx", 2.),
                        number(&props, "dy", 2.),
                        number(&props, "stdDeviation", 2.),
                        flood(&props),
                    ));
                    source_drawn = true;
                    made = Made::Source;
                }
                "feGaussianBlur" if from_shape => {
                    sigma = Some(number(&props, "stdDeviation", 0.));
                    made = Made::Shape;
                }
                "feGaussianBlur" if made == Made::Source || input == "SourceGraphic" => {
                    blurred = Some(number(&props, "stdDeviation", 0.));
                    made = Made::Blurred;
                }
                "feOffset" if from_shape => {
                    offset = Some((number(&props, "dx", 0.), number(&props, "dy", 0.)));
                    made = Made::Shape;
                }
                "feMorphology" if from_shape => made = Made::Shape,
                "feFlood" => {
                    let c = flood(&props);
                    if c.a > 0. {
                        tint = Some(c);
                    }
                    if made != Made::Shape {
                        made = Made::Other;
                    }
                }
                "feColorMatrix" => {
                    let values = props.get("values").map(number_list).unwrap_or_default();
                    let matrix = props.get("type").is_none_or(|t| t == "matrix");
                    if !matrix || values.len() != 20 {
                        return None;
                    }
                    if input == "SourceAlpha" {
                        // The element's shape, to make a shadow of.
                        made = Made::Shape;
                    } else if made == Made::Shape {
                        tint = Some(Color {
                            r: values[4].clamp(0., 1.) as f32,
                            g: values[9].clamp(0., 1.) as f32,
                            b: values[14].clamp(0., 1.) as f32,
                            a: values[18].clamp(0., 1.) as f32,
                            dark: None,
                        });
                    } else {
                        return None;
                    }
                }
                "feComponentTransfer" if made == Made::Shape => {
                    for f in doc.child_nodes(p) {
                        let fp = doc.props(f);
                        match doc.nodes[f].name {
                            "feFuncA" if fp.get("type") == Some("linear") => {
                                alpha *= number(&fp, "slope", 1.).clamp(0., 1.);
                            }
                            "feFuncA" => {}
                            _ => return None,
                        }
                    }
                }
                "feComposite" => {
                    let operator = props.get("operator").unwrap_or("over");
                    let other = props.get("in2").unwrap_or("");
                    match operator {
                        // A color cut to the shadow's shape, or a shadow
                        // without what the element covers.
                        "in" | "out" => {
                            if other == "SourceGraphic" || input == "SourceGraphic" {
                                return None;
                            }
                            made = Made::Shape;
                        }
                        "over" => {
                            finish = true;
                            if input == "SourceGraphic" || other == "SourceGraphic" {
                                source_drawn = true;
                                made = Made::Source;
                            }
                        }
                        _ => return None,
                    }
                }
                "feBlend" => {
                    if props.get("mode").is_some_and(|m| m != "normal") {
                        return None;
                    }
                    finish = true;
                    if input == "SourceGraphic" || props.get("in2") == Some("SourceGraphic") {
                        source_drawn = true;
                        made = Made::Source;
                    }
                }
                "feMerge" => {
                    finish = true;
                    let over_source = doc
                        .child_nodes(p)
                        .any(|n| attr(&doc.nodes[n].attrs, "in") == Some("SourceGraphic"));
                    if over_source {
                        source_drawn = true;
                        made = Made::Source;
                    }
                }
                _ => return None,
            }
            if finish && (offset.is_some() || sigma.is_some()) {
                let (dx, dy) = offset.take().unwrap_or((0., 0.));
                let color = faded(tint.take().unwrap_or(Color::BLACK), alpha as f32);
                fx.shadows
                    .push(shadow(dx, dy, sigma.take().unwrap_or(0.), color));
                alpha = 1.;
            }
        }
        if offset.is_some() || sigma.is_some() {
            // A shadow that nothing puts under the element.
            return None;
        }
        match blurred {
            Some(s) if source_drawn => fx.glow = Some((s.max(0.) * scale) as f32),
            Some(s) => fx.blur = (s.max(0.) * scale) as f32,
            None if !source_drawn => return None,
            None => {}
        }
        Some(fx)
    }

    fn image(&mut self, props: &Props, state: &State, out: &mut Vec<Item>) {
        let Some(href) = props.href() else {
            return;
        };
        let src: String = href.chars().filter(|c| !c.is_whitespace()).collect();
        let kind = src
            .get(..src.len().min(24))
            .unwrap_or("")
            .to_ascii_lowercase();
        if !kind.starts_with("data:") {
            self.skip("<image> that is not a data: URL");
            return;
        }
        if !["png", "jpeg", "jpg", "gif", "webp"]
            .iter()
            .any(|k| kind.starts_with(&format!("data:image/{k}")))
        {
            self.skip("<image> that is not a PNG or a JPEG");
            return;
        }
        let l = state.lengths(self.view);
        let x = props.get("x").and_then(|v| l.x(v)).unwrap_or(0.);
        let y = props.get("y").and_then(|v| l.y(v)).unwrap_or(0.);
        let w = props.get("width").and_then(|v| l.x(v));
        let h = props.get("height").and_then(|v| l.y(v));
        let (Some(w), Some(h)) = (w, h) else {
            self.skip("<image> without a width and a height");
            return;
        };
        if w <= 0. || h <= 0. {
            return;
        }
        let (align, slice, stretch) = aspect(props.get("preserveAspectRatio").unwrap_or(""));
        out.push(Item::Image(Image {
            transform: self.frame(state.transform),
            x: x as f32,
            y: y as f32,
            w: w as f32,
            h: h as f32,
            src,
            fit: match (stretch, slice) {
                (true, _) => Fit::Stretch,
                (false, true) => Fit::Slice,
                (false, false) => Fit::Meet,
            },
            align: (align.0 as f32, align.1 as f32),
            opacity: 1.,
        }));
    }

    fn text(&mut self, index: usize, props: &Props, state: &State, out: &mut Vec<Item>) {
        let mut lines = Lines {
            transform: self.frame(state.transform),
            lines: Vec::new(),
            origin_x: 0.,
            y: 0.,
            rise: 0.,
            dx: 0.,
            after_space: true,
        };
        self.text_content(index, props, state, true, &mut lines);
        for mut line in lines.lines {
            if !state.keep_spaces
                && let Some(last) = line.runs.last_mut()
            {
                last.text.truncate(last.text.trim_end_matches(' ').len());
            }
            line.runs.retain(|r| !r.text.is_empty());
            if !line.runs.is_empty() {
                out.push(Item::Text(line));
            }
        }
    }

    /// The characters of a `<text>` or one of its `<tspan>`s, added to
    /// `lines`.
    fn text_content(
        &mut self,
        index: usize,
        props: &Props,
        state: &State,
        root: bool,
        lines: &mut Lines,
    ) {
        let doc = self.doc;
        let l = state.lengths(self.view);
        // The first value: one position for the whole run.
        let first = |key: &str, across: bool| {
            let v = props.get(key)?;
            let v = v
                .split(|c: char| c == ',' || c.is_whitespace())
                .find(|p| !p.is_empty())?;
            if across { l.x(v) } else { l.y(v) }
        };
        let (x, y) = (first("x", true), first("y", false));
        let dx = first("dx", true).unwrap_or(0.);
        let dy = first("dy", false).unwrap_or(0.);
        if root {
            lines.origin_x = x.unwrap_or(0.);
            lines.start(lines.origin_x + dx, y.unwrap_or(0.) + dy, state);
        } else if let Some(x) = x {
            // A new line; without a `y` it stays where the text is.
            lines.start(x + dx, y.unwrap_or(lines.y + lines.rise) + dy, state);
        } else {
            if let Some(y) = y {
                lines.rise = y - lines.y;
            }
            lines.dx += dx;
            lines.rise += dy;
        }
        let fill = match &state.fill {
            _ if state.hidden => Color::TRANSPARENT,
            // A gradient is not laid across text: its middle color.
            Paint::Server(id, fallback) => {
                let alpha = state.fill_opacity;
                match self.gradient(id, Some([0., 0., 1., 1.]), state, alpha) {
                    Server::Gradient(g) => {
                        fallback.map_or(g.color_at(0.5), |c| faded(c, alpha as f32))
                    }
                    Server::Color(c) => c,
                    Server::Missing => {
                        fallback.map_or(Color::TRANSPARENT, |c| faded(c, alpha as f32))
                    }
                }
            }
            plain => state
                .resolve(plain, state.fill_opacity)
                .unwrap_or(Color::TRANSPARENT),
        };
        for child in &doc.nodes[index].children {
            match child {
                Child::Text(s) => lines.push(s, state, fill),
                Child::Node(c) => match doc.nodes[*c].name {
                    "tspan" | "a" => {
                        let own = doc.props(*c);
                        let inner = state.inherit(&own, &l);
                        if !inner.display_none {
                            self.text_content(*c, &own, &inner, false, lines);
                        }
                    }
                    "title" | "desc" => {}
                    other => self.skip(format!("<{other}> in a <text>")),
                },
            }
        }
    }
}

/// The box around `segs`: `[x, y, width, height]`.
fn bounds(segs: &[Seg]) -> Option<[f64; 4]> {
    let (mut x0, mut y0) = (f64::INFINITY, f64::INFINITY);
    let (mut x1, mut y1) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut add = |x: f64, y: f64| {
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    };
    let (mut cur, mut start) = ((0f64, 0f64), (0f64, 0f64));
    for s in segs {
        match *s {
            Seg::Move(x, y) => {
                add(x, y);
                (cur, start) = ((x, y), (x, y));
            }
            Seg::Line(x, y) => {
                add(x, y);
                cur = (x, y);
            }
            Seg::Cubic(ax, ay, bx, by, x, y) => {
                add(x, y);
                // Where the curve turns back, across and down.
                let at = |t: f64, p: [f64; 4]| {
                    let u = 1. - t;
                    u * u * u * p[0]
                        + 3. * u * u * t * p[1]
                        + 3. * u * t * t * p[2]
                        + t * t * t * p[3]
                };
                let (px, py) = ([cur.0, ax, bx, x], [cur.1, ay, by, y]);
                for p in [px, py] {
                    let a = -p[0] + 3. * p[1] - 3. * p[2] + p[3];
                    let b = 2. * (p[0] - 2. * p[1] + p[2]);
                    let c = p[1] - p[0];
                    let roots = if a.abs() < 1e-12 {
                        [if b.abs() < 1e-12 { -1. } else { -c / b }, -1.]
                    } else {
                        let d = b * b - 4. * a * c;
                        if d < 0. {
                            [-1., -1.]
                        } else {
                            [(-b + d.sqrt()) / (2. * a), (-b - d.sqrt()) / (2. * a)]
                        }
                    };
                    for t in roots {
                        if t > 0. && t < 1. {
                            add(at(t, px), at(t, py));
                        }
                    }
                }
                cur = (x, y);
            }
            Seg::Close => cur = start,
        }
    }
    (x0 <= x1 && y0 <= y1).then_some([x0, y0, x1 - x0, y1 - y0])
}

// --- Geometry -------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    Cubic(f64, f64, f64, f64, f64, f64),
    Close,
}

/// The outline of a shape element, in its user space.
fn geometry(name: &str, props: &Props, lengths: &Lengths) -> Option<Vec<Seg>> {
    // Across, down, or neither (a circle's radius).
    let side = |k: &str| match k {
        "x" | "width" | "cx" | "x1" | "x2" | "rx" => props.get(k).and_then(|v| lengths.x(v)),
        "r" => props.get(k).and_then(|v| lengths.other(v)),
        _ => props.get(k).and_then(|v| lengths.y(v)),
    };
    let n = |k: &str| side(k).unwrap_or(0.);
    let segs = match name {
        "path" => path_data(props.get("d")?),
        "rect" => {
            let (x, y, w, h) = (n("x"), n("y"), n("width"), n("height"));
            if w <= 0. || h <= 0. {
                return None;
            }
            let (rx, ry) = match (side("rx"), side("ry")) {
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
            let v = number_list(props.get("points")?);
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
        // The group is one picture at half opacity (see
        // `a_group_s_opacity_is_the_group_s`); as plain paths, each has it.
        assert!(matches!(&s.drawing.items[0], Item::Group(g) if g.opacity == 0.5));
        let paths = s.drawing.flattened().paths;
        let [line, ring, last] = &paths[..] else {
            panic!("{paths:?}")
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
              <g display="none"><rect width="5" height="5" visibility="visible"/></g>
              <rect width="5" height="5" visibility="hidden"/>
            </svg>"##,
        )
        .unwrap();
        assert_eq!(s.drawing.paths.len(), 1);
        assert_eq!(s.drawing.paths[0].fill, Some(rgba(0x00ff00, 1.)));
        assert!(s.drawing.paths[0].even_odd);
    }

    #[test]
    fn current_color_follows_the_node_unless_the_svg_sets_color() {
        // #51: `currentColor` without a `color` attribute is the node's
        // text color (`Color::current()`), at the path's opacity.
        let s = parse(
            r#"<svg viewBox="0 0 10 10">
              <path d="M0 0H5V5Z" fill="currentColor" fill-opacity=".5"/>
              <path d="M0 0L10 10" stroke="currentColor" fill="none"/>
            </svg>"#,
        )
        .unwrap();
        let fill = s.drawing.paths[0].fill.unwrap();
        assert!(fill.is_current());
        assert_eq!(fill.a, 0.5);
        assert!(s.drawing.paths[1].stroke.unwrap().is_current());
        // Encoded for backends and read back.
        let d = crate::shape::Drawing::decode(&s.drawing.encode());
        assert!(d.uses_current_color());
        assert!((d.paths[0].fill.unwrap().a - 0.5).abs() < 0.01);
        // Resolved against a text color, adaptive included.
        let text = Color::adaptive(rgba(0x111111, 1.), rgba(0xeeeeee, 1.));
        let r = d.with_current_color(text);
        assert_eq!(r.paths[1].stroke, Some(text));
        assert!(!r.uses_current_color());
        // A fill left to its default stays black.
        let black = parse(r#"<svg viewBox="0 0 1 1"><rect width="1" height="1"/></svg>"#).unwrap();
        assert_eq!(black.drawing.paths[0].fill, Some(Color::BLACK));
        // Tinted: every painted path, each at its own opacity.
        let t = black.drawing.tinted(text);
        assert_eq!(t.paths[0].fill, Some(text));
        let half = d.tinted(rgba(0xff0000, 1.));
        assert!((half.paths[0].fill.unwrap().a - 0.5).abs() < 0.01);
        assert_eq!(half.paths[1].stroke, Some(rgba(0xff0000, 1.)));
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

    /// Everything `markup` draws, in order.
    fn items(markup: &str) -> Vec<Item> {
        parse(markup).unwrap().drawing.all_items()
    }

    fn path(item: &Item) -> &Path {
        match item {
            Item::Path(p) => p,
            other => panic!("not a path: {other:?}"),
        }
    }

    fn group(item: &Item) -> &Group {
        match item {
            Item::Group(g) => g,
            other => panic!("not a group: {other:?}"),
        }
    }

    fn text(item: &Item) -> &Text {
        match item {
            Item::Text(t) => t,
            other => panic!("not a text: {other:?}"),
        }
    }

    /// A point of a gradient's or an item's own coordinates, in fractions
    /// of the frame.
    fn through(m: crate::shape::Matrix, x: f32, y: f32) -> (f32, f32) {
        (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
    }

    #[test]
    fn gradients_paint_fills_and_strokes() {
        let all = items(
            r##"<svg viewBox="0 0 100 50">
              <defs>
                <linearGradient id="base">
                  <stop offset="0" stop-color="#ff0000"/>
                  <stop offset="50%" stop-color="#00ff00" stop-opacity="0.5"/>
                  <stop offset="1" style="stop-color: blue"/>
                </linearGradient>
                <linearGradient id="down" href="#base" x1="0" y1="0" x2="0" y2="1" spreadMethod="reflect"/>
                <linearGradient id="user" xlink:href="#base" gradientUnits="userSpaceOnUse"
                  x1="10" y1="0" x2="30" y2="0" gradientTransform="translate(50 0)"/>
                <radialGradient id="spot" cx="0.5" cy="0.5" r="0.5" fx="25%">
                  <stop offset="0" stop-color="white"/><stop offset="1" stop-color="black"/>
                </radialGradient>
              </defs>
              <rect x="20" y="10" width="40" height="20" fill="url(#down)" fill-opacity="0.5"/>
              <rect x="0" y="0" width="100" height="50" fill="none" stroke="url(#user) #123456" stroke-width="2"/>
              <circle cx="50" cy="25" r="10" fill="url(#spot)"/>
              <rect width="10" height="10" fill="url(#missing) #abcdef"/>
              <rect width="10" height="10" fill="url(#missing)"/>
            </svg>"##,
        );
        // Across the shape's box, top to bottom, with the stops of the
        // gradient it refers to; the fill's opacity is in the stops.
        let p = path(&all[0]);
        let g = p.fill_gradient.as_ref().unwrap();
        assert_eq!(g.spread, Spread::Reflect);
        assert_eq!(
            g.shape,
            GradientShape::Linear {
                x1: 0.,
                y1: 0.,
                x2: 0.,
                y2: 1.
            }
        );
        let (x, y) = through(g.transform, 0., 0.);
        assert!(close(x, 0.2) && close(y, 0.2), "{x} {y}");
        let (x, y) = through(g.transform, 1., 1.);
        assert!(close(x, 0.6) && close(y, 0.6), "{x} {y}");
        let stops: Vec<(f32, Color)> = g.stops.iter().map(|s| (s.offset, s.color)).collect();
        assert_eq!(
            stops,
            vec![
                (0., rgba(0xff0000, 0.5)),
                (0.5, rgba(0x00ff00, 0.25)),
                (1., rgba(0x0000ff, 0.5))
            ]
        );
        // Without gradients the shape has the color halfway.
        assert_eq!(p.fill, Some(rgba(0x00ff00, 0.25)));
        // In user units, moved by its own transform; the color after
        // `url()` is the flat one.
        let p = path(&all[1]);
        let g = p.stroke_gradient.as_ref().unwrap();
        let GradientShape::Linear { x1, x2, .. } = g.shape else {
            panic!()
        };
        assert!(close(through(g.transform, x1, 0.).0, 0.6));
        assert!(close(through(g.transform, x2, 0.).0, 0.8));
        assert_eq!((p.stroke, p.width), (Some(rgba(0x123456, 1.)), 2.));
        assert!(p.fill.is_none());
        // A circle with its focal point off center.
        let g = path(&all[2]).fill_gradient.as_ref().unwrap();
        assert_eq!(
            g.shape,
            GradientShape::Radial {
                cx: 0.5,
                cy: 0.5,
                r: 0.5,
                fx: 0.25,
                fy: 0.5
            }
        );
        let (x, y) = through(g.transform, 0.5, 0.5);
        assert!(close(x, 0.5) && close(y, 0.5));
        // A paint that is not there: the color after it, or nothing.
        assert_eq!(path(&all[3]).fill, Some(rgba(0xabcdef, 1.)));
        assert!(path(&all[3]).fill_gradient.is_none());
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn clip_paths_cut_shapes_and_groups() {
        let s = parse(
            r##"<svg viewBox="0 0 100 100">
              <clipPath id="left"><rect width="50" height="100"/></clipPath>
              <clipPath id="half" clipPathUnits="objectBoundingBox">
                <rect width="0.5" height="1" clip-rule="evenodd"/>
              </clipPath>
              <clipPath id="empty"/>
              <g clip-path="url(#left)" transform="translate(10 0)">
                <circle cx="50" cy="50" r="40" fill="red"/>
                <circle cx="20" cy="50" r="10" fill="blue"/>
              </g>
              <rect x="20" y="40" width="60" height="20" clip-path="url(#half)"/>
              <rect width="10" height="10" clip-path="url(#empty)"/>
              <rect width="10" height="10" clip-path="url(#nothing)"/>
            </svg>"##,
        )
        .unwrap();
        let all = s.drawing.all_items();
        // In the group's coordinates: moved with it.
        let g = group(&all[0]);
        let clip = g.clip.as_ref().unwrap();
        assert_eq!(clip.segments[0], Segment::Move(0.1, 0.));
        assert_eq!(clip.segments[2], Segment::Line(0.6, 1.));
        assert_eq!(g.items.len(), 2);
        // In fractions of the shape's box.
        let g = group(&all[1]);
        let clip = g.clip.as_ref().unwrap();
        assert!(clip.even_odd);
        assert_eq!(clip.segments[0], Segment::Move(0.2, 0.4));
        assert_eq!(clip.segments[2], Segment::Line(0.5, 0.6));
        // A clip with nothing in it shows nothing; one that is not in the
        // document is left out, and said so.
        assert!(matches!(all[2], Item::Path(_)));
        assert_eq!(all.len(), 3);
        assert_eq!(s.skipped, vec![r#"<clipPath id="nothing">"#]);
    }

    #[test]
    fn text_is_lines_of_runs() {
        let all = items(
            r##"<svg viewBox="0 0 200 100" font-family="Inter, sans-serif">
              <text x="100" y="50" font-size="20" text-anchor="middle" fill="#fff" fill-opacity="0.5">
                Hello
                <tspan font-weight="bold" font-style="italic" dy="-5" letter-spacing="1">big</tspan>
                world &amp; co&#8212;
                <tspan x="10" dy="1.2em" fill="red" font-family="monospace">second
                line</tspan>
              </text>
              <text transform="translate(20 30) rotate(90)" font-size="50%" dominant-baseline="central">Axis</text>
              <text x="0" y="0" font-size="12">  </text>
            </svg>"##,
        );
        let t = text(&all[0]);
        assert_eq!((t.x, t.y, t.anchor), (100., 50., Anchor::Middle));
        let runs: Vec<(&str, u16, bool, f32)> = t
            .runs
            .iter()
            .map(|r| (r.text.as_str(), r.weight, r.italic, r.dy))
            .collect();
        // Spaces are one space, and none at the ends.
        assert_eq!(
            runs,
            vec![
                ("Hello ", 400, false, 0.),
                ("big", 700, true, -5.),
                (" world & co\u{2014}", 400, false, -5.)
            ]
        );
        assert_eq!(t.runs[0].family, FontFamily::Named("Inter".into()));
        assert_eq!((t.runs[0].size, t.runs[1].spacing), (20., 1.));
        assert_eq!(t.runs[0].fill, rgba(0xffffff, 0.5));
        // Its own coordinates are the document's: to fractions of the frame.
        let (x, y) = through(t.transform, t.x, t.y);
        assert!(close(x, 0.5) && close(y, 0.5));
        // A `tspan` with an `x` starts a line, `dy` below where the text was.
        let t = text(&all[1]);
        assert_eq!((t.x, t.y), (10., 50. - 5. + 24.));
        assert_eq!(t.runs[0].text, "second line");
        assert_eq!(t.runs[0].family, FontFamily::Monospace);
        assert_eq!(t.runs[0].fill, rgba(0xff0000, 0.5));
        // A rotated label: the rotation is in its transform.
        let t = text(&all[2]);
        assert_eq!((t.runs[0].size, t.baseline), (8., Baseline::Central));
        let (x, y) = through(t.transform, 10., 0.);
        assert!(close(x, 20. / 200.) && close(y, 40. / 100.), "{x} {y}");
        // Nothing but spaces: no text.
        assert_eq!(all.len(), 3);

        // Room before a line moves the line, anchored or not; room before
        // a later run is the run's.
        let all = items(
            r#"<svg viewBox="0 0 200 100">
              <text x="100" y="50" dx="6" text-anchor="end">a<tspan dx="3">b</tspan></text>
            </svg>"#,
        );
        let t = text(&all[0]);
        assert_eq!((t.x, t.runs[0].dx, t.runs[1].dx), (106., 0., 3.));
    }

    #[test]
    fn images_keep_their_rectangle_and_how_they_fill_it() {
        let png = "data:image/png;base64,iVBORw0KGgo=";
        let s = parse(&format!(
            r##"<svg viewBox="0 0 320 200" xmlns:xlink="http://www.w3.org/1999/xlink">
              <image x="200" y="20" width="100" height="160" preserveAspectRatio="xMinYMax slice" href="{png}"/>
              <image width="10" height="10" preserveAspectRatio="none" opacity="0.5" xlink:href="data:image/jpeg;base64,
                 /9j/4AAQ"/>
              <image width="10" height="10" href="photo.png"/>
              <image width="10" height="10" href="data:image/svg+xml;base64,PHN2Zz4="/>
              <image href="{png}"/>
            </svg>"##
        ))
        .unwrap();
        let all = s.drawing.all_items();
        let Item::Image(i) = &all[0] else { panic!() };
        assert_eq!((i.x, i.y, i.w, i.h), (200., 20., 100., 160.));
        assert_eq!((i.fit, i.align, i.opacity), (Fit::Slice, (0., 1.), 1.));
        assert_eq!(i.src, png);
        let (x, y) = through(i.transform, i.x + i.w, i.y + i.h);
        assert!(close(x, 300. / 320.) && close(y, 0.9));
        // Line breaks leave the URL; the opacity is the image's.
        let Item::Image(i) = &all[1] else { panic!() };
        assert_eq!((i.fit, i.opacity), (Fit::Stretch, 0.5));
        assert_eq!(i.src, "data:image/jpeg;base64,/9j/4AAQ");
        assert_eq!(all.len(), 2);
        assert_eq!(
            s.skipped,
            vec![
                "<image> that is not a PNG or a JPEG",
                "<image> that is not a data: URL",
                "<image> without a width and a height"
            ]
        );
    }

    #[test]
    fn a_group_s_opacity_is_the_group_s() {
        let s = parse(
            r##"<svg viewBox="0 0 100 100">
              <g opacity="0.5">
                <rect width="40" height="40" fill="#000"/>
                <rect x="20" y="10" width="40" height="40" fill="#000"/>
              </g>
              <g opacity="0.5"><rect width="10" height="10" fill="#f00" opacity="0.5"/></g>
              <rect width="10" height="10" fill="#f00" stroke="#00f" opacity="0.5"/>
              <g opacity="0"><rect width="10" height="10"/></g>
            </svg>"##,
        )
        .unwrap();
        let all = s.drawing.all_items();
        // Two shapes that overlap: drawn together, then shown at 50%.
        let g = group(&all[0]);
        assert_eq!((g.opacity, g.items.len()), (0.5, 2));
        assert_eq!(path(&g.items[0]).fill, Some(Color::BLACK));
        // One paint alone: the opacity is in its color.
        assert_eq!(path(&all[1]).fill, Some(rgba(0xff0000, 0.25)));
        // A fill under its stroke: one picture too.
        let g = group(&all[2]);
        assert_eq!((g.opacity, g.items.len()), (0.5, 1));
        assert_eq!(all.len(), 3);
        // Where groups are not drawn, each path has the opacity.
        let flat = s.drawing.flattened();
        assert_eq!(flat.paths[0].fill, Some(rgba(0, 0.5)));
        assert_eq!(flat.paths[3].stroke, Some(rgba(0x0000ff, 0.5)));
    }

    #[test]
    fn filters_are_shadows_blurs_and_glows() {
        let s = parse(
            r##"<svg viewBox="0 0 100 100" width="200" height="200">
              <filter id="drop"><feDropShadow dx="0" dy="4" stdDeviation="8" flood-color="#000" flood-opacity="0.25"/></filter>
              <filter id="chain">
                <feGaussianBlur in="SourceAlpha" stdDeviation="3"/>
                <feOffset dx="2" dy="2" result="offsetblur"/>
                <feFlood flood-color="rgb(255,0,0)"/>
                <feComposite in2="offsetblur" operator="in"/>
                <feComponentTransfer><feFuncA type="linear" slope="0.5"/></feComponentTransfer>
                <feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
              </filter>
              <filter id="tool" filterUnits="userSpaceOnUse" color-interpolation-filters="sRGB">
                <feFlood flood-opacity="0" result="BackgroundImageFix"/>
                <feColorMatrix in="SourceAlpha" type="matrix" values="0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 127 0" result="hardAlpha"/>
                <feOffset dy="4"/>
                <feGaussianBlur stdDeviation="2"/>
                <feComposite in2="hardAlpha" operator="out"/>
                <feColorMatrix type="matrix" values="0 0 0 0 0 0 0 0 0 0 0 0 0 0 1 0 0 0 0.25 0"/>
                <feBlend mode="normal" in2="BackgroundImageFix" result="effect1_dropShadow"/>
                <feColorMatrix in="SourceAlpha" type="matrix" values="0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 127 0" result="hardAlpha"/>
                <feOffset dy="1"/>
                <feGaussianBlur stdDeviation="1"/>
                <feColorMatrix type="matrix" values="0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0.5 0"/>
                <feBlend mode="normal" in2="effect1_dropShadow" result="effect2_dropShadow"/>
                <feBlend mode="normal" in="SourceGraphic" in2="effect2_dropShadow" result="shape"/>
              </filter>
              <filter id="soft"><feGaussianBlur stdDeviation="2.5"/></filter>
              <filter id="glow">
                <feGaussianBlur stdDeviation="2" result="b"/>
                <feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge>
              </filter>
              <filter id="noise"><feTurbulence baseFrequency="0.05"/></filter>
              <rect width="10" height="10" filter="url(#drop)"/>
              <rect width="10" height="10" filter="url(#chain)"/>
              <rect width="10" height="10" filter="url(#tool)"/>
              <rect width="10" height="10" filter="url(#soft)"/>
              <rect width="10" height="10" filter="url(#glow)"/>
              <rect width="10" height="10" style="filter: drop-shadow(1px 2px 3px rgba(0, 0, 255, 0.5)) blur(1px)"/>
              <rect width="10" height="10" filter="url(#noise)"/>
            </svg>"##,
        )
        .unwrap();
        let all = s.drawing.all_items();
        // Points at the document's size: 2 per unit here.
        assert_eq!(
            group(&all[0]).shadows,
            vec![Shadow {
                dx: 0.,
                dy: 8.,
                blur: 16.,
                color: rgba(0, 0.25)
            }]
        );
        assert_eq!(
            group(&all[1]).shadows,
            vec![Shadow {
                dx: 4.,
                dy: 4.,
                blur: 6.,
                color: rgba(0xff0000, 0.5)
            }]
        );
        // What design tools export: one shadow per effect, the first at
        // the bottom, its color in a matrix.
        assert_eq!(
            group(&all[2]).shadows,
            vec![
                Shadow {
                    dx: 0.,
                    dy: 8.,
                    blur: 4.,
                    color: rgba(0x0000ff, 0.25)
                },
                Shadow {
                    dx: 0.,
                    dy: 2.,
                    blur: 2.,
                    color: rgba(0, 0.5)
                }
            ]
        );
        // Nothing of a filter shows outside its region: a tenth around
        // the shape's box unless the filter says.
        let g = group(&all[3]);
        assert_eq!((g.blur, g.shadows.len()), (5., 0));
        let region = &g.clip.as_ref().unwrap().segments;
        assert_eq!(region[0], Segment::Move(-0.01, -0.01));
        assert_eq!(region[2], Segment::Line(0.11, 0.11));
        // A glow: the shape blurred, under itself.
        let g = group(&all[4]);
        assert!(g.clip.is_some());
        assert_eq!(group(&g.items[0]).blur, 4.);
        assert!(matches!(g.items[1], Item::Path(_)));
        // CSS functions have no region.
        let g = group(&all[5]);
        assert!(g.clip.is_none());
        assert_eq!(g.blur, 2.);
        assert_eq!(
            g.shadows,
            vec![Shadow {
                dx: 2.,
                dy: 4.,
                blur: 6.,
                color: rgba(0x0000ff, 0.5)
            }]
        );
        // What is not a shadow or a blur: the shape as it is, and said so.
        assert!(matches!(all[6], Item::Path(_)));
        assert_eq!(all.len(), 7);
        assert_eq!(s.skipped, vec![r#"<filter id="noise">"#]);
    }

    #[test]
    fn masks_are_drawings_a_group_shows_through() {
        let s = parse(
            r##"<svg viewBox="0 0 100 100">
              <linearGradient id="fade"><stop offset="0" stop-color="#fff"/><stop offset="1" stop-color="#000"/></linearGradient>
              <mask id="m"><rect width="100" height="100" fill="url(#fade)"/></mask>
              <mask id="a" mask-type="alpha" maskContentUnits="objectBoundingBox" fill="red">
                <circle cx="0.5" cy="0.5" r="0.5"/>
              </mask>
              <g mask="url(#m)"><rect width="100" height="100" fill="blue"/></g>
              <rect x="50" y="50" width="20" height="40" mask="url(#a)"/>
            </svg>"##,
        )
        .unwrap();
        let all = s.drawing.all_items();
        let m = group(&all[0]).mask.as_ref().unwrap();
        assert!(m.luminance);
        assert!(path(&m.items[0]).fill_gradient.is_some());
        // In fractions of the masked shape's box, with the mask's paint.
        let m = group(&all[1]).mask.as_ref().unwrap();
        assert!(!m.luminance);
        let circle = path(&m.items[0]);
        assert_eq!(circle.fill, Some(rgba(0xff0000, 1.)));
        assert_eq!(circle.segments[0], Segment::Move(0.7, 0.7));
        assert!(s.skipped.is_empty());
        // A mask is not what plain renderers draw.
        assert_eq!(s.drawing.flattened().paths.len(), 2);
    }

    #[test]
    fn use_draws_symbols_and_other_elements() {
        let all = items(
            r##"<svg viewBox="0 0 100 100" xmlns:xlink="http://www.w3.org/1999/xlink">
              <defs>
                <rect id="dot" width="10" height="10"/>
                <g id="pair" fill="blue"><use href="#dot"/><use href="#dot" x="20"/></g>
              </defs>
              <symbol id="icon" viewBox="0 0 10 10"><path d="M0 0H10V10Z"/></symbol>
              <use xlink:href="#dot" x="30" y="40" fill="red"/>
              <use href="#pair" transform="translate(0 50)" opacity="0.5"/>
              <use href="#icon" x="60" y="60" width="20" height="40" fill="green"/>
              <use href="#loop" id="loop"/>
            </svg>"##,
        );
        // Moved by `x` and `y`, with what the `use` gives it to inherit.
        let p = path(&all[0]);
        assert_eq!(p.fill, Some(rgba(0xff0000, 1.)));
        assert_eq!(p.segments[0], Segment::Move(0.3, 0.4));
        let g = group(&all[1]);
        assert_eq!((g.opacity, g.items.len()), (0.5, 2));
        assert_eq!(path(&g.items[1]).fill, Some(rgba(0x0000ff, 1.)));
        assert_eq!(path(&g.items[1]).segments[0], Segment::Move(0.2, 0.5));
        // A symbol's viewBox in the `use`'s box, whole and centered.
        let p = path(&all[2]);
        assert_eq!(p.fill, Some(rgba(0x008000, 1.)));
        assert_eq!(p.segments[0], Segment::Move(0.6, 0.7));
        assert_eq!(p.segments[1], Segment::Line(0.8, 0.7));
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn style_sheets_apply_by_type_class_and_id() {
        let s = parse(
            r##"<svg viewBox="0 0 10 10">
              <style><![CDATA[
                /* The most specific rule wins, then the last one. */
                rect { fill: red; stroke: #000 }
                .a, .b { fill: lime; }
                rect.a.big { fill: blue !important }
                #one { fill: #fff }
                g > .child { fill: orange }
                g .deep { fill: purple }
                svg > .deep { fill: black }
                @media print { rect { fill: black } }
                rect:hover { fill: pink }
              ]]></style>
              <rect class="a" width="1" height="1"/>
              <rect class="a big" width="1" height="1"/>
              <rect id="one" class="a big" width="1" height="1"/>
              <rect class="b" width="1" height="1" style="fill: teal" fill="gray"/>
              <g><g><rect class="child deep" width="1" height="1"/></g></g>
              <circle r="1" fill="gray"/>
            </svg>"##,
        )
        .unwrap();
        let fills: Vec<Color> = s.drawing.paths.iter().map(|p| p.fill.unwrap()).collect();
        assert_eq!(
            fills,
            vec![
                rgba(0x00ff00, 1.),
                rgba(0x0000ff, 1.),
                rgba(0xffffff, 1.),
                rgba(0x008080, 1.),
                rgba(0x800080, 1.),
                rgba(0x808080, 1.),
            ]
        );
        assert!(s.drawing.paths[0].stroke.is_some());
        assert!(s.drawing.paths[5].stroke.is_none());
        assert_eq!(s.skipped, vec!["<style> rules with unsupported selectors"]);
    }

    #[test]
    fn what_is_not_drawn_is_listed() {
        let s = parse(
            r##"<svg viewBox="0 0 10 10">
              <pattern id="dots" width="2" height="2"><circle r="1"/></pattern>
              <foreignObject width="10" height="10"><div xmlns="http://www.w3.org/1999/xhtml">Hi</div></foreignObject>
              <foreignObject width="10" height="10"/>
              <rect width="5" height="5" fill="url(#dots) red"/>
              <path d="M0 0L5 5" stroke="#000" marker-end="url(#arrow)"/>
              <text><textPath href="#p">Round</textPath></text>
              <rect width="1" height="1"><animate attributeName="x" to="5"/></rect>
            </svg>"##,
        )
        .unwrap();
        assert_eq!(
            s.skipped,
            vec![
                "<foreignObject> ×2",
                "<pattern> as a paint",
                "<textPath> in a <text>",
                "markers"
            ]
        );
        // The rest is drawn: the pattern's shape in the color after it.
        assert_eq!(s.drawing.paths[0].fill, Some(rgba(0xff0000, 1.)));
        assert_eq!(s.drawing.paths.len(), 3);
        assert!(
            parse(r#"<svg viewBox="0 0 1 1"><rect width="1" height="1"/></svg>"#)
                .unwrap()
                .skipped
                .is_empty()
        );
    }

    #[test]
    fn percentages_and_nested_documents_follow_the_view_box() {
        let s = parse(
            r##"<svg width="200" height="100" viewBox="0 0 100 100">
              <rect width="100%" height="50%" fill="red"/>
              <svg x="50" y="50" width="50" height="50" viewBox="0 0 10 10"><rect width="10" height="5"/></svg>
            </svg>"##,
        )
        .unwrap();
        // A square viewBox in a wide frame: whole and centered, like a
        // browser shows it.
        assert_eq!((s.width, s.height), (200., 100.));
        assert_eq!(s.drawing.size, Some((200., 100.)));
        let p = &s.drawing.paths[0];
        assert_eq!(p.segments[0], Segment::Move(0.25, 0.));
        assert_eq!(p.segments[2], Segment::Line(0.75, 0.5));
        let p = &s.drawing.paths[1];
        assert_eq!(p.segments[0], Segment::Move(0.5, 0.5));
        assert_eq!(p.segments[2], Segment::Line(0.75, 0.75));
    }

    /// The document of #342: what a browser draws is a card with rounded
    /// corners, a diagonal gradient and a soft shadow, an image cropped to
    /// its right edge, a bold white "Hello", and two squares at 50% as one.
    #[test]
    fn a_design_export_is_drawn_whole() {
        let s = parse(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="320" height="200" viewBox="0 0 320 200">
              <defs>
                <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
                  <stop offset="0" stop-color="#6366f1"/><stop offset="1" stop-color="#ec4899"/>
                </linearGradient>
                <clipPath id="card"><rect x="20" y="20" width="280" height="160" rx="16"/></clipPath>
                <filter id="shadow" x="-20%" y="-20%" width="140%" height="140%">
                  <feDropShadow dx="0" dy="4" stdDeviation="8" flood-color="#000" flood-opacity="0.25"/>
                </filter>
              </defs>
              <g filter="url(#shadow)">
                <g clip-path="url(#card)">
                  <rect x="0" y="0" width="320" height="200" fill="url(#bg) #6366f1"/>
                  <image x="200" y="20" width="100" height="160" preserveAspectRatio="xMidYMid slice"
                         href="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg=="/>
                </g>
              </g>
              <text x="40" y="110" font-family="sans-serif" font-size="24" font-weight="700" fill="#fff">Hello</text>
              <g opacity="0.5">
                <rect x="240" y="140" width="40" height="40" fill="#000"/>
                <rect x="260" y="150" width="40" height="40" fill="#000"/>
              </g>
            </svg>"##,
        )
        .unwrap();
        assert!(s.skipped.is_empty(), "{:?}", s.skipped);
        let [shadow, hello, squares] = &s.drawing.items[..] else {
            panic!("{:?}", s.drawing.items)
        };
        let shadow = group(shadow);
        // The filter's region: a fifth around the card's box.
        assert_eq!(
            shadow.clip.as_ref().unwrap().segments[0],
            Segment::Move(-0.2, -0.2)
        );
        assert_eq!(
            shadow.shadows,
            vec![Shadow {
                dx: 0.,
                dy: 4.,
                blur: 8.,
                color: rgba(0, 0.25)
            }]
        );
        let card = group(&shadow.items[0]);
        // The rounded rectangle: it starts after the corner's radius.
        assert_eq!(
            card.clip.as_ref().unwrap().segments[0],
            Segment::Move(36. / 320., 0.1)
        );
        let fill = path(&card.items[0]);
        assert_eq!(fill.fill, Some(rgba(0x6366f1, 1.)));
        let g = fill.fill_gradient.as_ref().unwrap();
        assert_eq!(g.stops[1].color, rgba(0xec4899, 1.));
        let (x, y) = through(g.transform, 1., 1.);
        assert!(close(x, 1.) && close(y, 1.));
        let Item::Image(image) = &card.items[1] else {
            panic!()
        };
        assert_eq!((image.x, image.w, image.fit), (200., 100., Fit::Slice));
        let hello = text(hello);
        let run = &hello.runs[0];
        assert_eq!(
            (run.text.as_str(), run.size, run.weight),
            ("Hello", 24., 700)
        );
        assert_eq!((run.fill, &run.family), (Color::WHITE, &FontFamily::System));
        let squares = group(squares);
        assert_eq!((squares.opacity, squares.items.len()), (0.5, 2));
        // Read back from what backends receive, to a hundredth of a point.
        let sent = s.drawing.encode();
        let back = Drawing::decode(&sent);
        assert_eq!(back.encode(), sent);
        assert_eq!(group(&back.items[0]).shadows[0].blur, 8.);
        let clip = group(&group(&back.items[0]).items[0]).clip.clone().unwrap();
        assert_eq!(clip.segments[0], Segment::Move(0.1125, 0.1));
        assert_eq!(text(&back.items[1]), hello);
        assert_eq!(back.items.len(), 3);
        // Shown twice as large: the shadow is too, and the text.
        let at = s.drawing.resolve_items(640., 400., 1.);
        assert_eq!(group(&at[0]).shadows[0].blur, 16.);
        let hello = text(&at[1]);
        assert_eq!(hello.runs[0].size, 48.);
        assert_eq!((hello.x, hello.y), (80., 220.));
        assert_eq!(hello.transform, crate::shape::IDENTITY);
    }
}
