//! What a shape draws, with cairo: paths in flat colors or gradients, and
//! what an SVG document adds (#342): lines of text (pango), bitmaps, and
//! groups shown as one picture (at an opacity, through a clip or a mask,
//! blurred, over shadows).
//!
//! Everything arrives in points for the view's size. Blurs, shadows and
//! luminance masks are computed on bitmaps at the screen's scale; the rest
//! stays vector.

use std::collections::HashMap;

use gtk::prelude::*;
use gtk::{cairo, gdk, glib, pango};
use prism_core::shape::{
    Anchor, Baseline, Clip, Fit, Gradient, GradientShape, Group, Image, Item, Mask, Matrix, Spread,
    Text, TextRun,
};
use prism_core::{Color, Path, Segment};

/// The decoded bitmap of each image by its `src` (`None`: it does not
/// decode, and draws nothing).
type Images = HashMap<String, Option<cairo::ImageSurface>>;

/// The largest bitmap a blur, a shadow or a luminance mask is computed on
/// (pixels); a larger one is computed at a lower scale.
const MAX_PIXELS: f64 = 4096. * 4096.;

/// What one shape view draws.
#[derive(Default)]
pub(crate) struct Drawing {
    items: Vec<Item>,
    images: Images,
}

impl Drawing {
    /// A drawing that is only paths (a chart, an icon).
    pub(crate) fn set_paths(&mut self, paths: &[Path]) {
        self.items = paths.iter().cloned().map(Item::Path).collect();
        self.images.clear();
    }

    /// A drawing with text, images or groups. Its images are decoded here,
    /// once: those of the previous drawing are kept.
    pub(crate) fn set_items(&mut self, items: &[Item]) {
        self.items = items.to_vec();
        let mut old = std::mem::take(&mut self.images);
        decode_all(items, &mut old, &mut self.images);
    }

    /// Draws it in `area`.
    pub(crate) fn draw(&self, area: &gtk::DrawingArea, cr: &cairo::Context) {
        Scene {
            pango: area.pango_context(),
            images: &self.images,
            dark: crate::imp::dark(),
            scale: area.scale_factor().max(1) as f64,
        }
        .items(cr, &self.items);
    }
}

fn decode_all(items: &[Item], old: &mut Images, images: &mut Images) {
    for item in items {
        match item {
            Item::Image(i) => {
                if !images.contains_key(&i.src) {
                    let bitmap = old.remove(&i.src).unwrap_or_else(|| decode(&i.src));
                    images.insert(i.src.clone(), bitmap);
                }
            }
            Item::Group(g) => {
                decode_all(&g.items, old, images);
                if let Some(m) = &g.mask {
                    decode_all(&m.items, old, images);
                }
            }
            Item::Path(_) | Item::Text(_) => {}
        }
    }
}

/// The bitmap of a `data:` URL (PNG, JPEG, and what gdk-pixbuf's loaders
/// read), as a cairo surface.
fn decode(src: &str) -> Option<cairo::ImageSurface> {
    let bytes = prism_native::images::data_url(src)?;
    let texture = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok()?;
    let mut surface =
        cairo::ImageSurface::create(cairo::Format::ARgb32, texture.width(), texture.height())
            .ok()?;
    let stride = surface.stride() as usize;
    // A texture downloads as cairo's ARGB32 (premultiplied).
    texture.download(&mut surface.data().ok()?, stride);
    Some(surface)
}

/// A rectangle, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Rect {
    /// Larger by `dx` on the left and on the right, `dy` above and below.
    fn grow(self, dx: f64, dy: f64) -> Rect {
        Rect {
            x: self.x - dx,
            y: self.y - dy,
            w: self.w + 2. * dx,
            h: self.h + 2. * dy,
        }
    }

    /// What it shares with `other`, if anything.
    fn intersect(self, other: Rect) -> Option<Rect> {
        let (x, y) = (self.x.max(other.x), self.y.max(other.y));
        let w = (self.x + self.w).min(other.x + other.w) - x;
        let h = (self.y + self.h).min(other.y + other.h) - y;
        (w > 0. && h > 0.).then_some(Rect { x, y, w, h })
    }
}

/// The light or the dark variant of `c`, as cairo takes colors.
fn rgba(c: Color, dark: bool) -> (f64, f64, f64, f64) {
    let c = if dark {
        c.dark_variant()
    } else {
        c.light_variant()
    };
    (c.r as f64, c.g as f64, c.b as f64, c.a as f64)
}

/// A Prism matrix as cairo's (the same six numbers, in the same order).
fn matrix(m: Matrix) -> cairo::Matrix {
    cairo::Matrix::new(
        m[0] as f64,
        m[1] as f64,
        m[2] as f64,
        m[3] as f64,
        m[4] as f64,
        m[5] as f64,
    )
}

/// Makes `segments` cairo's current path.
fn outline(cr: &cairo::Context, segments: &[Segment]) {
    cr.new_path();
    for s in segments {
        match *s {
            Segment::Move(x, y) => cr.move_to(x as f64, y as f64),
            Segment::Line(x, y) => cr.line_to(x as f64, y as f64),
            Segment::Cubic(a, b, c, d, e, f) => {
                cr.curve_to(a as f64, b as f64, c as f64, d as f64, e as f64, f as f64)
            }
            Segment::Close => cr.close_path(),
            // Already turned into curves.
            Segment::Arc { .. } => {}
        }
    }
}

fn fill_rule(even_odd: bool) -> cairo::FillRule {
    if even_odd {
        cairo::FillRule::EvenOdd
    } else {
        cairo::FillRule::Winding
    }
}

/// A gradient as a cairo pattern: its line or circles in its own
/// coordinates, its matrix taking the drawing's points back to them (so a
/// skewed gradient stays skewed). `None` for a gradient without stops, or
/// whose transform flattens it: the flat color is painted instead.
fn gradient(g: &Gradient, dark: bool) -> Option<cairo::Pattern> {
    let last = g.stops.last()?;
    let pattern = match g.shape {
        GradientShape::Linear { x1, y1, x2, y2 } if (x1, y1) != (x2, y2) => cairo::Pattern::clone(
            &cairo::LinearGradient::new(x1 as f64, y1 as f64, x2 as f64, y2 as f64),
        ),
        // Offset 0 at the focal point, 1 on the circle.
        GradientShape::Radial { cx, cy, r, fx, fy } if r > 0. => cairo::Pattern::clone(
            &cairo::RadialGradient::new(fx as f64, fy as f64, 0., cx as f64, cy as f64, r as f64),
        ),
        // No length, no radius: all of it is past the last stop.
        _ => {
            let (r, g, b, a) = rgba(last.color, dark);
            return Some(cairo::Pattern::clone(&cairo::SolidPattern::from_rgba(
                r, g, b, a,
            )));
        }
    };
    let stops = cairo::Gradient::try_from(pattern).ok()?;
    for s in &g.stops {
        let (r, g, b, a) = rgba(s.color, dark);
        stops.add_color_stop_rgba(s.offset as f64, r, g, b, a);
    }
    stops.set_matrix(pattern_matrix(g.transform)?);
    stops.set_extend(match g.spread {
        Spread::Pad => cairo::Extend::Pad,
        Spread::Reflect => cairo::Extend::Reflect,
        Spread::Repeat => cairo::Extend::Repeat,
    });
    Some(cairo::Pattern::clone(&stops))
}

/// The matrix of a cairo pattern whose own coordinates `transform` maps to
/// the drawing's: cairo wants the way back.
fn pattern_matrix(transform: Matrix) -> Option<cairo::Matrix> {
    matrix(transform).try_invert().ok()
}

/// Makes the gradient, else the flat color, what cairo paints with. False
/// when there is neither.
fn source(cr: &cairo::Context, g: Option<&Gradient>, flat: Option<Color>, dark: bool) -> bool {
    if let Some(pattern) = g.and_then(|g| gradient(g, dark)) {
        return cr.set_source(&pattern).is_ok();
    }
    let Some(c) = flat else {
        return false;
    };
    let (r, g, b, a) = rgba(c, dark);
    cr.set_source_rgba(r, g, b, a);
    true
}

/// Fills, then strokes, one path.
pub(crate) fn path(cr: &cairo::Context, p: &Path, dark: bool) {
    outline(cr, &p.segments);
    if source(cr, p.fill_gradient.as_ref(), p.fill, dark) {
        cr.set_fill_rule(fill_rule(p.even_odd));
        let _ = cr.fill_preserve();
    }
    // A gradient stroke is the gradient seen through the stroke: in the
    // same coordinates as a fill's.
    if source(cr, p.stroke_gradient.as_ref(), p.stroke, dark) {
        cr.set_line_width(p.width as f64);
        let (cap, join) = if p.round {
            (cairo::LineCap::Round, cairo::LineJoin::Round)
        } else {
            (cairo::LineCap::Butt, cairo::LineJoin::Miter)
        };
        cr.set_line_cap(cap);
        cr.set_line_join(join);
        match p.dash {
            Some((d, g)) => cr.set_dash(&[d as f64, g as f64], 0.),
            None => cr.set_dash(&[], 0.),
        }
        let _ = cr.stroke_preserve();
    }
    cr.new_path();
}

/// Where a line of text `width` wide starts, anchored at `x`.
fn anchored(anchor: Anchor, x: f64, width: f64) -> f64 {
    match anchor {
        Anchor::Start => x,
        Anchor::Middle => x - width / 2.,
        Anchor::End => x - width,
    }
}

/// How far below a text's `y` its baseline is, from its first font's
/// metrics.
fn below(baseline: Baseline, ascent: f64, descent: f64, x_height: f64) -> f64 {
    match baseline {
        Baseline::Alphabetic => 0.,
        Baseline::Middle => x_height / 2.,
        Baseline::Central => (ascent - descent) / 2.,
        Baseline::Hanging => 0.8 * ascent,
    }
}

/// Where a bitmap of size `bitmap` goes in `frame`: stretched to it, whole
/// inside it or covering it (then larger than the frame, which crops it),
/// the room left or the overflow split by `align`.
fn placed(fit: Fit, align: (f32, f32), frame: Rect, bitmap: (f64, f64)) -> Rect {
    let (sx, sy) = (frame.w / bitmap.0, frame.h / bitmap.1);
    let (sx, sy) = match fit {
        Fit::Stretch => (sx, sy),
        Fit::Meet => (sx.min(sy), sx.min(sy)),
        Fit::Slice => (sx.max(sy), sx.max(sy)),
    };
    let (w, h) = (bitmap.0 * sx, bitmap.1 * sy);
    Rect {
        x: frame.x + (frame.w - w) * align.0 as f64,
        y: frame.y + (frame.h - h) * align.1 as f64,
        w,
        h,
    }
}

/// The pixels of a bitmap of `area` at `scale` pixels per point: its left
/// and top edges in points (on a pixel's edge), its width and height in
/// pixels, and its scale (lower than `scale` for an area of more than
/// [`MAX_PIXELS`]).
fn grid(area: Rect, scale: f64) -> Option<(f64, f64, i32, i32, f64)> {
    if !(area.w > 0. && area.h > 0. && scale > 0.) {
        return None;
    }
    let scale = scale
        * (MAX_PIXELS / (area.w * area.h * scale * scale))
            .sqrt()
            .min(1.);
    let (left, top) = ((area.x * scale).floor(), (area.y * scale).floor());
    let w = ((area.x + area.w) * scale).ceil() - left;
    let h = ((area.y + area.h) * scale).ceil() - top;
    Some((left / scale, top / scale, w as i32, h as i32, scale))
}

/// A bitmap of a part of the drawing: `scale` pixels per point, its top
/// left corner at `(x, y)` points.
struct Bitmap {
    surface: cairo::ImageSurface,
    x: f64,
    y: f64,
    scale: f64,
}

impl Bitmap {
    /// The bitmap as a paint, at its place moved by `(dx, dy)` points.
    fn pattern(&self, dx: f64, dy: f64) -> cairo::SurfacePattern {
        let pattern = cairo::SurfacePattern::create(&self.surface);
        // From the drawing's points to the bitmap's pixels.
        pattern.set_matrix(cairo::Matrix::new(
            self.scale,
            0.,
            0.,
            self.scale,
            -(self.x + dx) * self.scale,
            -(self.y + dy) * self.scale,
        ));
        pattern
    }

    /// A copy in `format`: all of it (`ARgb32`) or its alpha (`A8`).
    fn copy(&self, format: cairo::Format) -> Option<Bitmap> {
        let surface =
            cairo::ImageSurface::create(format, self.surface.width(), self.surface.height())
                .ok()?;
        let cr = cairo::Context::new(&surface).ok()?;
        cr.set_operator(cairo::Operator::Source);
        cr.set_source_surface(&self.surface, 0., 0.).ok()?;
        cr.paint().ok()?;
        drop(cr);
        Some(Bitmap { surface, ..*self })
    }

    /// Blurs it: a Gaussian of standard deviation `sigma` points.
    fn blur(&mut self, sigma: f64) {
        let (w, h) = (self.surface.width(), self.surface.height());
        let stride = self.surface.stride() as usize;
        let channels = match self.surface.format() {
            cairo::Format::A8 => 1,
            _ => 4,
        };
        if let Ok(mut data) = self.surface.data() {
            // Premultiplied channels blur each on its own.
            gaussian_blur(
                &mut data,
                w as usize,
                h as usize,
                stride,
                channels,
                sigma * self.scale,
            );
        }
    }

    /// For the bitmap of a luminance mask: how much each pixel lets
    /// through, as an alpha bitmap.
    fn luminance(&self) -> Option<Bitmap> {
        let (w, h) = (
            self.surface.width() as usize,
            self.surface.height() as usize,
        );
        let mut surface =
            cairo::ImageSurface::create(cairo::Format::A8, w as i32, h as i32).ok()?;
        let (from, to) = (self.surface.stride() as usize, surface.stride() as usize);
        {
            let mut alpha = surface.data().ok()?;
            self.surface
                .with_data(|pixels| {
                    for y in 0..h {
                        let row = &pixels[y * from..y * from + 4 * w];
                        for (x, px) in row.as_chunks::<4>().0.iter().enumerate() {
                            alpha[y * to + x] = luma(u32::from_ne_bytes(*px));
                        }
                    }
                })
                .ok()?;
        }
        Some(Bitmap { surface, ..*self })
    }
}

/// How much a pixel of a luminance mask (cairo's premultiplied ARGB) lets
/// through, `0..=255`: its luminance times its alpha, which the
/// premultiplied channels give at once.
fn luma(argb: u32) -> u8 {
    let (r, g, b) = ((argb >> 16) & 0xff, (argb >> 8) & 0xff, argb & 0xff);
    ((2126 * r + 7152 * g + 722 * b + 5000) / 10000) as u8
}

/// How a line of pixels is blurred.
#[derive(Debug, PartialEq)]
enum Kernel {
    /// The Gaussian's own weights, from the pixel outwards: exact, for a
    /// small blur.
    Weights(Vec<f64>),
    /// Three running averages one after the other, which is nearly a
    /// Gaussian and costs the same whatever the blur: how many pixels each
    /// takes on either side.
    Boxes([usize; 3]),
}

impl Kernel {
    /// For a standard deviation of `sigma` pixels.
    fn new(sigma: f64) -> Kernel {
        if sigma < 4. {
            // Three standard deviations hold all that shows.
            let radius = (3. * sigma).ceil() as usize;
            let mut weights: Vec<f64> = (0..=radius)
                .map(|k| (-((k * k) as f64) / (2. * sigma * sigma)).exp())
                .collect();
            let sum = weights[0] + 2. * weights[1..].iter().sum::<f64>();
            for w in &mut weights {
                *w /= sum;
            }
            return Kernel::Weights(weights);
        }
        // Three averages over odd widths have the variance
        // Σ (width² − 1) / 12: the two nearest odd widths, as many of the
        // narrow one as brings that closest to sigma².
        let ideal = (4. * sigma * sigma + 1.).sqrt().floor() as usize;
        let narrow = if ideal.is_multiple_of(2) {
            ideal - 1
        } else {
            ideal
        };
        let n = narrow as f64;
        let count = ((12. * sigma * sigma - 3. * n * n - 12. * n - 9.) / (-4. * n - 4.)).round();
        Kernel::Boxes(std::array::from_fn(|i| {
            if (i as f64) < count {
                narrow / 2
            } else {
                narrow / 2 + 1
            }
        }))
    }

    /// Blurs `line` (what is past its ends is transparent). `spare` is
    /// room to work in.
    fn apply(&self, line: &mut Vec<f64>, spare: &mut Vec<f64>) {
        match self {
            Kernel::Weights(weights) => {
                spare.clear();
                for i in 0..line.len() {
                    let mut v = weights[0] * line[i];
                    for (k, w) in weights.iter().enumerate().skip(1) {
                        if i >= k {
                            v += w * line[i - k];
                        }
                        if let Some(after) = line.get(i + k) {
                            v += w * after;
                        }
                    }
                    spare.push(v);
                }
                std::mem::swap(line, spare);
            }
            Kernel::Boxes(sides) => {
                for &side in sides {
                    // The sum of the pixels around the current one: the
                    // next one in, the last one out.
                    spare.clear();
                    let mut sum: f64 = line.iter().take(side + 1).sum();
                    for i in 0..line.len() {
                        spare.push(sum / (2 * side + 1) as f64);
                        if let Some(next) = line.get(i + side + 1) {
                            sum += next;
                        }
                        if i >= side {
                            sum -= line[i - side];
                        }
                    }
                    std::mem::swap(line, spare);
                }
            }
        }
    }
}

/// Blurs a bitmap in place with a Gaussian of standard deviation `sigma`
/// pixels: `height` rows of `stride` bytes, each `width` pixels of
/// `channels` bytes. What is outside the bitmap is transparent.
fn gaussian_blur(
    data: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    channels: usize,
    sigma: f64,
) {
    if sigma <= 0. || width == 0 || height == 0 {
        return;
    }
    let kernel = Kernel::new(sigma);
    let (mut line, mut spare) = (Vec::new(), Vec::new());
    // Each channel of each row, then of each column: `at(line, i)` is the
    // byte of the line's `i`th pixel.
    let mut pass = |lines: usize, len: usize, at: &dyn Fn(usize, usize) -> usize| {
        for l in 0..lines {
            line.clear();
            line.extend((0..len).map(|i| data[at(l, i)] as f64));
            kernel.apply(&mut line, &mut spare);
            for (i, v) in line.iter().enumerate() {
                data[at(l, i)] = v.round().clamp(0., 255.) as u8;
            }
        }
    };
    pass(height * channels, width, &|l, i| {
        (l / channels) * stride + i * channels + l % channels
    });
    pass(width * channels, height, &|l, i| i * stride + l);
}

/// The part of the drawing cairo draws in now.
fn visible(cr: &cairo::Context) -> Rect {
    // A surface without bounds says infinity: far enough.
    const FAR: f64 = 1e6;
    let (x1, y1, x2, y2) = cr.clip_extents().unwrap_or((0., 0., 0., 0.));
    let (x1, y1, x2, y2) = (x1.max(-FAR), y1.max(-FAR), x2.min(FAR), y2.min(FAR));
    Rect {
        x: x1,
        y: y1,
        w: x2 - x1,
        h: y2 - y1,
    }
}

/// One run of a text laid out.
struct Run {
    layout: pango::Layout,
    /// What it takes on the line, its letter spacing included.
    width: f64,
    /// How far below the layout's top its baseline is.
    baseline: f64,
}

/// What draws the items of one drawing.
struct Scene<'a> {
    /// The view's fonts.
    pango: pango::Context,
    images: &'a Images,
    dark: bool,
    /// Pixels per point of the bitmaps of blurs, shadows and masks.
    scale: f64,
}

impl Scene<'_> {
    fn items(&self, cr: &cairo::Context, items: &[Item]) {
        for item in items {
            match item {
                Item::Path(p) => path(cr, p, self.dark),
                Item::Text(t) => self.text(cr, t),
                Item::Image(i) => self.image(cr, i),
                Item::Group(g) => self.group(cr, g),
            }
        }
    }

    /// The font of a run: the view's own (the system font, like a label's)
    /// with the run's family, size, weight and slant.
    fn font(&self, r: &TextRun) -> pango::FontDescription {
        let mut font = self.pango.font_description().unwrap_or_default();
        if let Some(family) = crate::text::family_name(&r.family) {
            font.set_family(&family);
        }
        // In the text's own units, whatever the screen's text scale: the
        // text's transform makes them points.
        font.set_absolute_size(r.size as f64 * pango::SCALE as f64);
        font.set_weight(unsafe { glib::translate::from_glib(r.weight as i32) });
        font.set_style(if r.italic {
            pango::Style::Italic
        } else {
            pango::Style::Normal
        });
        font
    }

    fn run(&self, r: &TextRun) -> Run {
        let layout = pango::Layout::new(&self.pango);
        layout.set_font_description(Some(&self.font(r)));
        // One line, whatever the text holds.
        layout.set_single_paragraph_mode(true);
        layout.set_text(&r.text);
        if r.spacing != 0. {
            let attrs = pango::AttrList::new();
            attrs.insert(pango::AttrInt::new_letter_spacing(
                crate::text::pango_units(r.spacing),
            ));
            layout.set_attributes(Some(&attrs));
        }
        let unit = pango::SCALE as f64;
        // Pango leaves the spacing out after the line's last character; a
        // run has it after each.
        let last = if r.text.is_empty() {
            0.
        } else {
            r.spacing as f64
        };
        Run {
            width: layout.size().0 as f64 / unit + last,
            baseline: layout.baseline() as f64 / unit,
            layout,
        }
    }

    /// How far below the text's `y` its baseline is, from the metrics of
    /// its first run's font.
    fn baseline(&self, baseline: Baseline, first: &TextRun) -> f64 {
        if baseline == Baseline::Alphabetic {
            return 0.;
        }
        let unit = pango::SCALE as f64;
        let font = self.font(first);
        let metrics = self.pango.metrics(Some(&font), None);
        // Pango has no x-height: how far an "x" rises above its baseline.
        let x = pango::Layout::new(&self.pango);
        x.set_font_description(Some(&font));
        x.set_text("x");
        let x_height = (x.baseline() - x.extents().0.y()) as f64 / unit;
        below(
            baseline,
            metrics.ascent() as f64 / unit,
            metrics.descent() as f64 / unit,
            x_height,
        )
    }

    /// One line: its runs one after the other, each in its font and color.
    fn text(&self, cr: &cairo::Context, t: &Text) {
        let Some(first) = t.runs.first() else {
            return;
        };
        // A transform that flattens the text shows nothing (and cairo
        // would refuse it for good).
        let transform = matrix(t.transform);
        if transform.try_invert().is_err() {
            return;
        }
        let runs: Vec<Run> = t.runs.iter().map(|r| self.run(r)).collect();
        let width: f64 = t
            .runs
            .iter()
            .zip(&runs)
            .map(|(r, l)| r.dx as f64 + l.width)
            .sum();
        let mut x = anchored(t.anchor, t.x as f64, width);
        let y = t.y as f64 + self.baseline(t.baseline, first);
        let _ = cr.save();
        cr.transform(transform);
        for (r, l) in t.runs.iter().zip(&runs) {
            x += r.dx as f64;
            let (red, green, blue, alpha) = rgba(r.fill, self.dark);
            // A transparent run only takes its room.
            if alpha > 0. {
                // The layout's glyphs as GTK draws a label's, in cairo.
                let snapshot = gtk::Snapshot::new();
                let color = gdk::RGBA::new(red as f32, green as f32, blue as f32, alpha as f32);
                snapshot.append_layout(&l.layout, &color);
                if let Some(glyphs) = snapshot.to_node() {
                    let _ = cr.save();
                    cr.translate(x, y + r.dy as f64 - l.baseline);
                    glyphs.draw(cr);
                    let _ = cr.restore();
                }
            }
            x += l.width;
        }
        let _ = cr.restore();
    }

    /// A bitmap in its rectangle.
    fn image(&self, cr: &cairo::Context, i: &Image) {
        let Some(Some(bitmap)) = self.images.get(&i.src) else {
            return;
        };
        let size = (bitmap.width() as f64, bitmap.height() as f64);
        let transform = matrix(i.transform);
        if i.opacity <= 0.
            || !(i.w > 0. && i.h > 0. && size.0 > 0. && size.1 > 0.)
            || transform.try_invert().is_err()
        {
            return;
        }
        let frame = Rect {
            x: i.x as f64,
            y: i.y as f64,
            w: i.w as f64,
            h: i.h as f64,
        };
        let at = placed(i.fit, i.align, frame, size);
        // What shows: all of the bitmap, or what the rectangle crops.
        let Some(shown) = at.intersect(frame) else {
            return;
        };
        let _ = cr.save();
        cr.transform(transform);
        cr.rectangle(shown.x, shown.y, shown.w, shown.h);
        cr.clip();
        cr.translate(at.x, at.y);
        cr.scale(at.w / size.0, at.h / size.1);
        if cr.set_source_surface(bitmap, 0., 0.).is_ok() {
            // The edge pixels go on past the edges, so an enlarged bitmap
            // does not fade out along its sides. Scaling is smooth
            // (cairo's default filter: bilinear when enlarging).
            cr.source().set_extend(cairo::Extend::Pad);
            let _ = cr.paint_with_alpha(i.opacity.min(1.) as f64);
        }
        let _ = cr.restore();
    }

    /// Items shown as one picture: blurred and over its shadows, then cut
    /// to the clip, seen through the mask, at the opacity.
    fn group(&self, cr: &cairo::Context, g: &Group) {
        if g.opacity <= 0. || g.items.is_empty() {
            return;
        }
        let content = |cr: &cairo::Context| {
            if g.blur > 0. || !g.shadows.is_empty() {
                self.effects(cr, g);
            } else {
                self.items(cr, &g.items);
            }
        };
        let _ = cr.save();
        if let Some(Clip { segments, even_odd }) = &g.clip {
            outline(cr, segments);
            cr.set_fill_rule(fill_rule(*even_odd));
            cr.clip();
        }
        if g.mask.is_none() && g.opacity >= 1. {
            // Nothing to see it through: drawn as it is.
            content(cr);
        } else {
            cr.push_group();
            content(cr);
            if let Ok(layer) = cr.pop_group() {
                self.composite(cr, &layer, g.mask.as_ref(), g.opacity as f64);
            }
        }
        let _ = cr.restore();
    }

    /// Paints a group's picture through its mask, at its opacity.
    fn composite(
        &self,
        cr: &cairo::Context,
        layer: &cairo::Pattern,
        mask: Option<&Mask>,
        opacity: f64,
    ) {
        let Some(mask) = mask else {
            let _ = cr.set_source(layer);
            let _ = cr.paint_with_alpha(opacity);
            return;
        };
        // A mask that draws nothing shows nothing.
        let Some(mask) = self.mask(cr, mask) else {
            return;
        };
        if opacity < 1. {
            // Through the mask first, then at the opacity.
            cr.push_group();
            let _ = cr.set_source(layer);
            let _ = cr.mask(&mask);
            let _ = cr.pop_group_to_source();
            let _ = cr.paint_with_alpha(opacity);
        } else {
            let _ = cr.set_source(layer);
            let _ = cr.mask(&mask);
        }
    }

    /// A mask as a paint whose alpha is how much of the group shows: what
    /// it draws as it is, or as bright as it is on a bitmap.
    fn mask(&self, cr: &cairo::Context, m: &Mask) -> Option<cairo::Pattern> {
        if !m.luminance {
            cr.push_group();
            self.items(cr, &m.items);
            return cr.pop_group().ok();
        }
        let (recorded, ink) = self.record(&m.items, visible(cr))?;
        let colors = self.bitmap(&recorded, ink, cairo::Format::ARgb32)?;
        Some(cairo::Pattern::clone(&colors.luminance()?.pattern(0., 0.)))
    }

    /// Draws what `items` show inside `within` on a surface that remembers
    /// it: bitmaps of them are drawn from it. With how far they reach.
    fn record(&self, items: &[Item], within: Rect) -> Option<(cairo::RecordingSurface, Rect)> {
        // A surface with bounds: on one without, cairo does not know how
        // far a layer (a group at an opacity, a masked one) reaches.
        let (x, y) = (within.x.floor(), within.y.floor());
        let bounds = cairo::Rectangle::new(
            x,
            y,
            (within.x + within.w).ceil() - x,
            (within.y + within.h).ceil() - y,
        );
        let surface =
            cairo::RecordingSurface::create(cairo::Content::ColorAlpha, Some(bounds)).ok()?;
        let cr = cairo::Context::new(&surface).ok()?;
        self.items(&cr, items);
        drop(cr);
        let (x, y, w, h) = surface.ink_extents();
        let ink = Rect { x, y, w, h }.intersect(within)?;
        Some((surface, ink))
    }

    /// The part `area` of recorded items as a bitmap at the screen's scale.
    fn bitmap(
        &self,
        recorded: &cairo::RecordingSurface,
        area: Rect,
        format: cairo::Format,
    ) -> Option<Bitmap> {
        let (x, y, w, h, scale) = grid(area, self.scale)?;
        let surface = cairo::ImageSurface::create(format, w, h).ok()?;
        let cr = cairo::Context::new(&surface).ok()?;
        cr.scale(scale, scale);
        cr.translate(-x, -y);
        cr.set_source_surface(recorded, 0., 0.).ok()?;
        cr.paint().ok()?;
        drop(cr);
        Some(Bitmap {
            surface,
            x,
            y,
            scale,
        })
    }

    /// A group's items blurred, over the group's shadows.
    fn effects(&self, cr: &cairo::Context, g: &Group) {
        let sigma = g.blur.max(0.) as f64;
        // The bitmap the blurs are computed on: what the items cover, as
        // far as a blur spreads them (three standard deviations hold all
        // of a Gaussian that shows), but no more than what can reach the
        // part of the view being drawn.
        let most = |f: &dyn Fn(&prism_core::shape::Shadow) -> f32| {
            g.shadows
                .iter()
                .map(|s| f(s).abs() as f64)
                .fold(0., f64::max)
        };
        let reach = 3. * (sigma + most(&|s| s.blur.max(0.)));
        let seen = visible(cr).grow(reach + most(&|s| s.dx), reach + most(&|s| s.dy));
        let Some((recorded, ink)) = self.record(&g.items, seen) else {
            return;
        };
        let Some(area) = ink.grow(reach, reach).intersect(seen) else {
            return;
        };
        let blurred = (sigma > 0.)
            .then(|| self.bitmap(&recorded, area, cairo::Format::ARgb32))
            .flatten()
            .map(|mut b| {
                b.blur(sigma);
                b
            });
        // Each shadow: the picture's alpha, blurred, filled with the
        // shadow's color, moved. The first one at the bottom.
        let alpha = match (&blurred, g.shadows.is_empty()) {
            (_, true) => None,
            (Some(b), false) => b.copy(cairo::Format::A8),
            (None, false) => self.bitmap(&recorded, area, cairo::Format::A8),
        };
        for s in &g.shadows {
            let Some(mut shadow) = alpha.as_ref().and_then(|a| a.copy(cairo::Format::A8)) else {
                break;
            };
            shadow.blur(s.blur.max(0.) as f64);
            let (red, green, blue, a) = rgba(s.color, self.dark);
            cr.set_source_rgba(red, green, blue, a);
            let _ = cr.mask(shadow.pattern(s.dx as f64, s.dy as f64));
        }
        match blurred {
            Some(b) => {
                let _ = cr.set_source(b.pattern(0., 0.));
                let _ = cr.paint();
            }
            // Not blurred: the items themselves, as sharp as without
            // shadows.
            None => self.items(cr, &g.items),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::shape::{Shadow, Stop};

    const fn color(r: f32, g: f32, b: f32) -> Color {
        Color {
            r,
            g,
            b,
            a: 1.,
            dark: None,
        }
    }

    const RED: Color = color(1., 0., 0.);
    const GREEN: Color = color(0., 1., 0.);
    const BLUE: Color = color(0., 0., 1.);

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    /// A filled rectangle, in points.
    fn square(x: f32, y: f32, w: f32, h: f32, color: Color) -> Path {
        Path {
            segments: vec![
                Segment::Move(x, y),
                Segment::Line(x + w, y),
                Segment::Line(x + w, y + h),
                Segment::Line(x, y + h),
                Segment::Close,
            ],
            fill: Some(color),
            ..Path::default()
        }
    }

    /// Draws `items` on a `w` × `h` bitmap and returns its pixels'
    /// `[r, g, b, a]` (premultiplied).
    fn render(items: &[Item], w: i32, h: i32) -> impl Fn(i32, i32) -> [u8; 4] + use<> {
        render_at(items, w, h, 1)
    }

    /// The same on a screen of `scale` pixels per point: `w` × `h` points,
    /// read by pixel.
    fn render_at(
        items: &[Item],
        w: i32,
        h: i32,
        scale: i32,
    ) -> impl Fn(i32, i32) -> [u8; 4] + use<> {
        let mut drawing = Drawing::default();
        drawing.set_items(items);
        let surface =
            cairo::ImageSurface::create(cairo::Format::ARgb32, w * scale, h * scale).unwrap();
        surface.set_device_scale(scale as f64, scale as f64);
        let cr = cairo::Context::new(&surface).unwrap();
        Scene {
            // No text in these drawings: no fonts needed.
            pango: pango::Context::new(),
            images: &drawing.images,
            dark: false,
            scale: scale as f64,
        }
        .items(&cr, &drawing.items);
        drop(cr);
        let stride = surface.stride() as usize;
        let mut pixels = Vec::new();
        surface.with_data(|d| pixels = d.to_vec()).unwrap();
        move |x, y| {
            let at = y as usize * stride + x as usize * 4;
            let v = u32::from_ne_bytes(pixels[at..at + 4].try_into().unwrap());
            [(v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8]
        }
    }

    fn near(a: [u8; 4], b: [u8; 4], by: u8) -> bool {
        a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= by)
    }

    fn two_stops(shape: GradientShape, transform: Matrix, spread: Spread) -> Gradient {
        Gradient {
            shape,
            stops: vec![
                Stop {
                    offset: 0.,
                    color: RED,
                },
                Stop {
                    offset: 1.,
                    color: BLUE,
                },
            ],
            spread,
            transform,
        }
    }

    #[test]
    fn images_fit_their_rectangle() {
        let frame = rect(10., 20., 100., 50.);
        // A square bitmap in a wide rectangle.
        assert_eq!(
            placed(Fit::Stretch, (0.5, 0.5), frame, (8., 8.)),
            rect(10., 20., 100., 50.)
        );
        assert_eq!(
            placed(Fit::Meet, (0.5, 0.5), frame, (8., 8.)),
            rect(35., 20., 50., 50.)
        );
        assert_eq!(
            placed(Fit::Meet, (0., 0.), frame, (8., 8.)),
            rect(10., 20., 50., 50.)
        );
        assert_eq!(
            placed(Fit::Meet, (1., 1.), frame, (8., 8.)),
            rect(60., 20., 50., 50.)
        );
        // Covering: as wide as the rectangle, the overflow above and below.
        assert_eq!(
            placed(Fit::Slice, (0.5, 0.5), frame, (8., 8.)),
            rect(10., -5., 100., 100.)
        );
        assert_eq!(
            placed(Fit::Slice, (0., 0.), frame, (8., 8.)),
            rect(10., 20., 100., 100.)
        );
        assert_eq!(
            placed(Fit::Slice, (1., 1.), frame, (8., 8.)),
            rect(10., -30., 100., 100.)
        );
    }

    #[test]
    fn text_is_anchored_and_aligned() {
        assert_eq!(anchored(Anchor::Start, 100., 40.), 100.);
        assert_eq!(anchored(Anchor::Middle, 100., 40.), 80.);
        assert_eq!(anchored(Anchor::End, 100., 40.), 60.);
        assert_eq!(below(Baseline::Alphabetic, 10., 2., 5.), 0.);
        assert_eq!(below(Baseline::Middle, 10., 2., 5.), 2.5);
        assert_eq!(below(Baseline::Central, 10., 2., 5.), 4.);
        assert_eq!(below(Baseline::Hanging, 10., 2., 5.), 8.);
    }

    #[test]
    fn rectangles_grow_and_intersect() {
        let r = rect(10., 10., 20., 20.);
        assert_eq!(r.grow(5., 2.), rect(5., 8., 30., 24.));
        assert_eq!(
            r.intersect(rect(20., 0., 50., 15.)),
            Some(rect(20., 10., 10., 5.))
        );
        assert_eq!(r.intersect(rect(30., 0., 5., 5.)), None);
    }

    #[test]
    fn bitmaps_sit_on_whole_pixels() {
        // At 2 pixels per point: from 2.5 to 13.5 points is pixels 5..27.
        assert_eq!(
            grid(rect(2.7, 1., 10.7, 4.), 2.),
            Some((2.5, 1., 22, 8, 2.))
        );
        assert_eq!(grid(rect(0., 0., 0., 4.), 2.), None);
        // Too large: fewer pixels per point.
        let (_, _, w, h, scale) = grid(rect(0., 0., 10000., 10000.), 2.).unwrap();
        assert!(scale < 0.5 && (w as f64 * h as f64) <= MAX_PIXELS * 1.01);
    }

    #[test]
    fn a_pattern_matrix_goes_back_to_the_gradient() {
        // Scaled, skewed and moved.
        let t: Matrix = [2., 0.5, -1., 3., 10., 20.];
        let back = pattern_matrix(t).unwrap();
        let (x, y) = matrix(t).transform_point(3., 4.);
        let (gx, gy) = back.transform_point(x, y);
        assert!((gx - 3.).abs() < 1e-9 && (gy - 4.).abs() < 1e-9);
        // Flattened: no way back.
        assert!(pattern_matrix([1., 2., 2., 4., 0., 0.]).is_none());
    }

    #[test]
    fn a_linear_gradient_follows_its_transform() {
        let line = GradientShape::Linear {
            x1: 0.,
            y1: 0.,
            x2: 1.,
            y2: 0.,
        };
        let with = |transform, spread| {
            let p = Path {
                fill_gradient: Some(two_stops(line, transform, spread)),
                ..square(0., 0., 100., 100., Color::BLACK)
            };
            render(&[Item::Path(p)], 100, 100)
        };
        // Along x over the whole width.
        let px = with([100., 0., 0., 100., 0., 0.], Spread::Pad);
        assert!(near(px(1, 50), [252, 0, 3, 255], 4));
        assert!(near(px(50, 10), [127, 0, 127, 255], 4));
        assert!(near(px(98, 90), [3, 0, 252, 255], 4));
        // Turned a quarter: along y.
        let px = with([0., 100., -100., 0., 0., 0.], Spread::Pad);
        assert!(near(px(50, 1), [252, 0, 3, 255], 4));
        assert!(near(px(10, 98), [3, 0, 252, 255], 4));
        // Over the first half only: padded, started over, or back.
        let half = [50., 0., 0., 50., 0., 0.];
        assert!(near(with(half, Spread::Pad)(75, 50), [0, 0, 255, 255], 4));
        assert!(near(
            with(half, Spread::Repeat)(75, 50),
            [127, 0, 127, 255],
            6
        ));
        assert!(near(
            with(half, Spread::Repeat)(52, 50),
            [245, 0, 10, 255],
            8
        ));
        assert!(near(
            with(half, Spread::Reflect)(52, 50),
            [10, 0, 245, 255],
            8
        ));
    }

    #[test]
    fn a_radial_gradient_starts_at_its_focal_point() {
        let ball = GradientShape::Radial {
            cx: 50.,
            cy: 50.,
            r: 50.,
            fx: 30.,
            fy: 30.,
        };
        let p = Path {
            fill_gradient: Some(two_stops(ball, prism_core::shape::IDENTITY, Spread::Pad)),
            ..square(0., 0., 100., 100., Color::BLACK)
        };
        let px = render(&[Item::Path(p)], 100, 100);
        assert!(near(px(30, 30), [255, 0, 0, 255], 8));
        // Redder at the center than at the same distance on the other
        // side of the focal point, and blue past the circle.
        assert!(px(50, 50)[0] < 200 && px(50, 50)[0] > px(80, 80)[0]);
        assert!(near(px(99, 99), [0, 0, 255, 255], 2));
    }

    #[test]
    fn a_gradient_without_length_is_its_last_color() {
        let point = GradientShape::Linear {
            x1: 0.5,
            y1: 0.5,
            x2: 0.5,
            y2: 0.5,
        };
        let p = Path {
            fill_gradient: Some(two_stops(point, prism_core::shape::IDENTITY, Spread::Pad)),
            ..square(0., 0., 10., 10., Color::BLACK)
        };
        assert_eq!(render(&[Item::Path(p)], 10, 10)(5, 5), [0, 0, 255, 255]);
    }

    #[test]
    fn a_gradient_stroke_is_in_the_path_s_coordinates() {
        let line = GradientShape::Linear {
            x1: 0.,
            y1: 0.,
            x2: 100.,
            y2: 0.,
        };
        let p = Path {
            fill: None,
            stroke: Some(Color::BLACK),
            stroke_gradient: Some(two_stops(line, prism_core::shape::IDENTITY, Spread::Pad)),
            width: 10.,
            ..square(10., 10., 80., 80., Color::BLACK)
        };
        let px = render(&[Item::Path(p)], 100, 100);
        // Left side red, right side blue, nothing inside.
        assert!(px(10, 50)[0] > 200 && px(10, 50)[2] < 55);
        assert!(px(90, 50)[2] > 200 && px(90, 50)[0] < 55);
        assert_eq!(px(50, 50), [0, 0, 0, 0]);
    }

    #[test]
    fn a_group_at_an_opacity_is_one_picture() {
        let g = Group {
            opacity: 0.5,
            items: vec![
                Item::Path(square(0., 0., 20., 20., Color::BLACK)),
                Item::Path(square(10., 10., 20., 20., Color::BLACK)),
            ],
            ..Group::default()
        };
        let px = render(&[Item::Group(g)], 40, 40);
        // Where the squares overlap is not darker.
        assert!(near(px(5, 5), [0, 0, 0, 128], 1));
        assert_eq!(px(15, 15), px(5, 5));
        assert_eq!(px(25, 25), px(5, 5));
        assert_eq!(px(35, 5), [0, 0, 0, 0]);
    }

    #[test]
    fn a_group_is_cut_to_its_clip() {
        let g = Group {
            clip: Some(Clip {
                segments: square(10., 10., 10., 10., Color::BLACK).segments,
                even_odd: false,
            }),
            items: vec![Item::Path(square(0., 0., 40., 40., RED))],
            ..Group::default()
        };
        let px = render(&[Item::Group(g)], 40, 40);
        assert_eq!(px(15, 15), [255, 0, 0, 255]);
        assert_eq!(px(5, 15), [0, 0, 0, 0]);
        assert_eq!(px(25, 15), [0, 0, 0, 0]);
    }

    #[test]
    fn masks_show_by_brightness_or_by_alpha() {
        let masked = |luminance, opacity| {
            let g = Group {
                opacity,
                mask: Some(Mask {
                    luminance,
                    items: vec![
                        Item::Path(square(0., 0., 10., 30., Color::WHITE)),
                        Item::Path(square(10., 0., 10., 30., Color::BLACK)),
                        Item::Path(square(20., 0., 10., 30., GREEN)),
                    ],
                }),
                items: vec![Item::Path(square(0., 0., 40., 30., RED))],
                ..Group::default()
            };
            render(&[Item::Group(g)], 40, 30)
        };
        // White shows, black hides, green shows as much as it is bright,
        // and where the mask draws nothing, nothing shows.
        let px = masked(true, 1.);
        assert_eq!(px(5, 15), [255, 0, 0, 255]);
        assert_eq!(px(15, 15), [0, 0, 0, 0]);
        assert!(near(px(25, 15), [182, 0, 0, 182], 2));
        assert_eq!(px(35, 15), [0, 0, 0, 0]);
        // By alpha: the three opaque squares show alike.
        let px = masked(false, 1.);
        assert_eq!(px(5, 15), [255, 0, 0, 255]);
        assert_eq!(px(15, 15), [255, 0, 0, 255]);
        assert_eq!(px(35, 15), [0, 0, 0, 0]);
        // The opacity comes on top of the mask.
        let px = masked(true, 0.5);
        assert!(near(px(5, 15), [128, 0, 0, 128], 1));
        assert!(near(px(25, 15), [91, 0, 0, 91], 2));
    }

    #[test]
    fn luminance_is_brightness_times_alpha() {
        assert_eq!(luma(0xffff_ffff), 255);
        assert_eq!(luma(0xff00_0000), 0);
        assert_eq!(luma(0x0000_0000), 0);
        assert_eq!(luma(0xffff_0000), 54);
        assert_eq!(luma(0xff00_ff00), 182);
        assert_eq!(luma(0xff00_00ff), 18);
        // White at half opacity (premultiplied).
        assert_eq!(luma(0x8080_8080), 128);
    }

    #[test]
    fn shadows_are_under_the_group_and_moved() {
        let shadow = |blur| Shadow {
            dx: 0.,
            dy: 20.,
            blur,
            color: BLUE.with_alpha(0.5),
        };
        let with = |shadows| {
            let g = Group {
                shadows,
                items: vec![Item::Path(square(20., 10., 20., 20., RED))],
                ..Group::default()
            };
            render(&[Item::Group(g)], 60, 70)
        };
        // Sharp: a copy 20 points lower, in the shadow's color.
        let px = with(vec![shadow(0.)]);
        assert_eq!(px(30, 20), [255, 0, 0, 255]);
        assert!(near(px(30, 40), [0, 0, 128, 128], 1));
        assert_eq!(px(30, 55), [0, 0, 0, 0]);
        assert_eq!(px(10, 40), [0, 0, 0, 0]);
        // Blurred: fainter at its edge, and spread past it.
        let px = with(vec![shadow(3.)]);
        assert_eq!(px(30, 20), [255, 0, 0, 255]);
        assert!(near(px(30, 40), [0, 0, 128, 128], 3));
        // Half of it on its lower edge, between these two rows.
        assert!(px(30, 49)[3] > 64 && px(30, 50)[3] < 64 && px(30, 50)[3] > 40);
        assert!(px(30, 53)[3] > 0 && px(30, 53)[3] < 64);
        assert!(px(17, 40)[3] > 0);
        assert_eq!(px(30, 65), [0, 0, 0, 0]);
        // Two shadows: the second one over the first.
        let px = with(vec![
            shadow(0.),
            Shadow {
                dx: 0.,
                dy: 30.,
                blur: 0.,
                color: GREEN,
            },
        ]);
        assert_eq!(px(30, 45), [0, 255, 0, 255]);
        assert!(near(px(30, 35), [0, 0, 128, 128], 1));
    }

    #[test]
    fn a_blurred_group_fades_at_its_edges() {
        let g = Group {
            blur: 2.,
            items: vec![Item::Path(square(20., 20., 20., 20., RED))],
            ..Group::default()
        };
        let px = render(&[Item::Group(g)], 60, 60);
        assert!(near(px(30, 30), [255, 0, 0, 255], 2));
        // Half of it at the edge, some of it past the edge.
        assert!(near(px(20, 30), [140, 0, 0, 140], 16));
        assert!(px(17, 30)[3] > 0 && px(17, 30)[3] < 64);
        assert_eq!(px(5, 30), [0, 0, 0, 0]);
    }

    #[test]
    fn effects_are_computed_at_the_screen_s_scale() {
        let g = Group {
            shadows: vec![Shadow {
                dx: 5.,
                dy: 20.,
                blur: 0.,
                color: BLUE,
            }],
            mask: Some(Mask {
                luminance: true,
                items: vec![Item::Path(square(0., 0., 30.5, 70., Color::WHITE))],
            }),
            items: vec![Item::Path(square(20.5, 10., 19.5, 20., RED))],
            ..Group::default()
        };
        // Two pixels per point: the shadow and the mask's edge fall where
        // they do in points, to the pixel.
        let px = render_at(&[Item::Group(g)], 60, 70, 2);
        assert_eq!(px(40, 30), [0, 0, 0, 0]);
        assert_eq!(px(41, 30), [255, 0, 0, 255]);
        assert_eq!(px(60, 30), [255, 0, 0, 255]);
        assert_eq!(px(61, 30), [0, 0, 0, 0]);
        assert_eq!(px(55, 61), [0, 0, 255, 255]);
        assert_eq!(px(55, 99), [0, 0, 255, 255]);
        assert_eq!(px(55, 100), [0, 0, 0, 0]);
        assert_eq!(px(50, 80), [0, 0, 0, 0]);
    }

    #[test]
    fn groups_nest() {
        // A masked group at an opacity, in a group with a shadow, in a
        // clipped group.
        let masked = Group {
            opacity: 0.5,
            mask: Some(Mask {
                luminance: true,
                items: vec![Item::Path(square(0., 0., 30., 60., Color::WHITE))],
            }),
            items: vec![Item::Path(square(20., 10., 20., 20., RED))],
            ..Group::default()
        };
        let raised = Group {
            shadows: vec![Shadow {
                dx: 0.,
                dy: 25.,
                blur: 0.,
                color: BLUE,
            }],
            items: vec![Item::Group(masked)],
            ..Group::default()
        };
        let cut = Group {
            clip: Some(Clip {
                segments: square(0., 0., 60., 50., Color::BLACK).segments,
                even_odd: false,
            }),
            items: vec![Item::Group(raised)],
            ..Group::default()
        };
        let px = render(&[Item::Group(cut)], 60, 60);
        // Half of the red square shows, at half opacity.
        assert!(near(px(25, 20), [128, 0, 0, 128], 1));
        assert_eq!(px(35, 20), [0, 0, 0, 0]);
        // Its shadow is as faint as it is, and stops at the clip.
        assert!(near(px(25, 45), [0, 0, 128, 128], 1));
        assert_eq!(px(35, 45), [0, 0, 0, 0]);
        assert_eq!(px(25, 52), [0, 0, 0, 0]);
    }

    /// The sum, the mean position and the standard deviation of a blurred
    /// single pixel.
    fn spread_of(sigma: f64) -> (f64, f64, f64) {
        let mut line = vec![0.; 401];
        line[200] = 1.;
        Kernel::new(sigma).apply(&mut line, &mut Vec::new());
        let sum: f64 = line.iter().sum();
        let mean = line
            .iter()
            .enumerate()
            .map(|(i, v)| i as f64 * v)
            .sum::<f64>()
            / sum;
        let var = line
            .iter()
            .enumerate()
            .map(|(i, v)| (i as f64 - mean).powi(2) * v)
            .sum::<f64>()
            / sum;
        (sum, mean, var.sqrt())
    }

    #[test]
    fn a_blur_is_a_gaussian_of_its_standard_deviation() {
        for sigma in [0.5, 1., 2.5, 3.9, 4., 5., 6., 8., 16., 40.] {
            let (sum, mean, deviation) = spread_of(sigma);
            assert!((sum - 1.).abs() < 1e-9, "{sigma}: sum {sum}");
            assert!((mean - 200.).abs() < 1e-6, "{sigma}: mean {mean}");
            // A blur of less than a pixel is a little narrow: its weights
            // are the Gaussian's at whole pixels.
            let off = if sigma < 1. { 0.1 } else { 0.05 };
            assert!(
                (deviation / sigma - 1.).abs() < off,
                "{sigma}: deviation {deviation}"
            );
        }
        assert!(matches!(Kernel::new(3.9), Kernel::Weights(_)));
        assert_eq!(Kernel::new(5.), Kernel::Boxes([4, 4, 5]));
    }

    #[test]
    fn a_blur_keeps_what_is_even_and_fades_the_edges() {
        // 3 rows of 8 one-byte pixels in rows of 12 bytes: the padding
        // stays as it is.
        let (w, h, stride) = (8, 3, 12);
        let mut data = vec![7u8; stride * h];
        for y in 0..h {
            data[y * stride..y * stride + w].fill(200);
        }
        let before = data.clone();
        gaussian_blur(&mut data, w, h, stride, 1, 0.);
        assert_eq!(data, before);
        gaussian_blur(&mut data, w, h, stride, 1, 0.6);
        assert!(data[stride..stride + w].iter().all(|v| *v <= 200));
        // The middle row's middle keeps the most, its ends less.
        assert!(data[stride + 4] > data[stride] && data[stride] > data[0]);
        assert!(
            data.chunks(stride)
                .all(|row| row[w..].iter().all(|v| *v == 7))
        );
        // Four channels: each on its own.
        let mut px = vec![0u8; 5 * 4];
        px[8..12].copy_from_slice(&[10, 0, 200, 255]);
        gaussian_blur(&mut px, 5, 1, 20, 4, 1.);
        assert!(px[9] == 0 && px[5] == 0 && px[13] == 0);
        assert!(px[4 + 3] > 0 && px[4 + 3] == px[12 + 3] && px[8 + 3] > px[4 + 3]);
    }

    #[test]
    fn an_image_is_decoded_once_and_drawn_in_its_rectangle() {
        // 2 × 2 pixels: red, green / blue, white.
        let src = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEklEQVR4nGP4z8DAAMIM/4EAAB/uBfsL2WiLAAAAAElFTkSuQmCC";
        let image = |fit, x: f32, w: f32, h: f32| {
            Item::Image(Image {
                x,
                y: 0.,
                w,
                h,
                src: src.into(),
                fit,
                ..Image::default()
            })
        };
        let mut drawing = Drawing::default();
        drawing.set_items(&[image(Fit::Stretch, 0., 40., 40.)]);
        let decoded = drawing.images[src].clone().expect("the PNG decodes");
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
        // The same picture in the next drawing: the same bitmap.
        drawing.set_items(&[image(Fit::Meet, 0., 40., 40.)]);
        assert_eq!(
            drawing.images[src].as_ref().map(|s| s.to_raw_none()),
            Some(decoded.to_raw_none())
        );
        // Stretched: each pixel a quarter of the rectangle, sharp at the
        // rectangle's corners.
        let px = render(&[image(Fit::Stretch, 0., 40., 40.)], 40, 40);
        assert!(near(px(1, 1), [255, 0, 0, 255], 12));
        assert!(near(px(38, 1), [0, 255, 0, 255], 12));
        assert!(near(px(1, 38), [0, 0, 255, 255], 12));
        assert!(near(px(38, 38), [255, 255, 255, 255], 12));
        // Whole in a wide rectangle: centered, nothing beside it.
        let px = render(&[image(Fit::Meet, 0., 80., 40.)], 80, 40);
        assert_eq!(px(10, 20), [0, 0, 0, 0]);
        assert_eq!(px(70, 20), [0, 0, 0, 0]);
        assert!(near(px(21, 1), [255, 0, 0, 255], 12));
        // Covering a tall rectangle: its middle columns, nothing outside.
        let px = render(&[image(Fit::Slice, 10., 20., 40.)], 40, 40);
        assert_eq!(px(5, 20), [0, 0, 0, 0]);
        assert_eq!(px(35, 20), [0, 0, 0, 0]);
        assert!(px(11, 1)[0] > 200 && px(28, 1)[1] > 200);
        // What does not decode draws nothing.
        let mut broken = Drawing::default();
        broken.set_items(&[Item::Image(Image {
            w: 10.,
            h: 10.,
            src: "data:image/png;base64,AAAA".into(),
            ..Image::default()
        })]);
        assert!(broken.images.values().all(Option::is_none));
    }
}
