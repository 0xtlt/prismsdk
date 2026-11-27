//! The arithmetic of a drawing's picture (`vector.rs`, #342): what
//! Direct2D's render targets have no call for (Gaussian blurs, shadows and
//! luminance masks, on pixels), and where things go (an image in its
//! rectangle, a line of text around its anchor, a gradient that is one
//! color). No Win32 types: tested on every host.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::Color;
use prism_core::shape::{Anchor, Baseline, Fit, Gradient, GradientShape, Item};

/// Whether `items` are paths in flat colors only: what XAML `Path`s draw
/// (`controls::set_paths`). Anything else is a picture.
pub fn is_flat(items: &[Item]) -> bool {
    items.iter().all(|i| match i {
        Item::Path(p) => p.fill_gradient.is_none() && p.stroke_gradient.is_none(),
        _ => false,
    })
}

/// How far outside the view (points) something can lie and still show in
/// it: the reach of the blurs and shadows of `items`, nested ones added.
pub fn margin(items: &[Item]) -> f32 {
    items
        .iter()
        .map(|i| match i {
            Item::Group(g) => {
                let shadows = g
                    .shadows
                    .iter()
                    .map(|s| 3. * s.blur.max(0.) + s.dx.abs().max(s.dy.abs()))
                    .fold(0., f32::max);
                let inside = margin(&g.items).max(g.mask.as_ref().map_or(0., |m| margin(&m.items)));
                3. * g.blur.max(0.) + shadows + inside
            }
            _ => 0.,
        })
        .fold(0., f32::max)
}

/// Where a `bitmap` (its width and height in pixels) shows in `rect`
/// (`x, y, w, h`): the rectangle it covers and the part of the bitmap
/// seen there, both as `[left, top, right, bottom]`. `None` when nothing
/// shows (an empty rectangle or bitmap).
pub fn fit(
    fit: Fit,
    align: (f32, f32),
    (x, y, w, h): (f32, f32, f32, f32),
    (bw, bh): (f32, f32),
) -> Option<([f32; 4], [f32; 4])> {
    if w <= 0. || h <= 0. || bw <= 0. || bh <= 0. {
        return None;
    }
    let (ax, ay) = (align.0.clamp(0., 1.), align.1.clamp(0., 1.));
    let whole = [0., 0., bw, bh];
    Some(match fit {
        Fit::Stretch => ([x, y, x + w, y + h], whole),
        Fit::Meet => {
            // As large as fits; the room left is split by `align`.
            let s = (w / bw).min(h / bh);
            let (dw, dh) = (bw * s, bh * s);
            let (dx, dy) = (x + (w - dw) * ax, y + (h - dh) * ay);
            ([dx, dy, dx + dw, dy + dh], whole)
        }
        Fit::Slice => {
            // Covering: the part of the bitmap that fits, by `align`.
            let s = (w / bw).max(h / bh);
            let (sw, sh) = (w / s, h / s);
            let (sx, sy) = ((bw - sw) * ax, (bh - sh) * ay);
            ([x, y, x + w, y + h], [sx, sy, sx + sw, sy + sh])
        }
    })
}

/// Where a line of text `width` wide starts, anchored at `x`.
pub fn line_start(anchor: Anchor, x: f32, width: f32) -> f32 {
    match anchor {
        Anchor::Start => x,
        Anchor::Middle => x - width / 2.,
        Anchor::End => x - width,
    }
}

/// How far below a text's `y` its baseline sits, from its first font's
/// ascent, descent and x-height.
pub fn baseline_drop(baseline: Baseline, ascent: f32, descent: f32, x_height: f32) -> f32 {
    match baseline {
        Baseline::Alphabetic => 0.,
        Baseline::Middle => x_height / 2.,
        Baseline::Central => (ascent - descent) / 2.,
        Baseline::Hanging => 0.8 * ascent,
    }
}

/// What a gradient comes to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    /// Nothing: it has no stop.
    None,
    /// One color: a single stop, or a line or circle without extent (SVG
    /// paints the last stop then).
    Flat(Color),
    /// A gradient.
    Gradient,
}

pub fn paint(g: &Gradient) -> Paint {
    let Some(last) = g.stops.last() else {
        return Paint::None;
    };
    let empty = match g.shape {
        GradientShape::Linear { x1, y1, x2, y2 } => x1 == x2 && y1 == y2,
        GradientShape::Radial { r, .. } => r <= 0.,
    };
    if empty || g.stops.len() == 1 {
        Paint::Flat(last.color)
    } else {
        Paint::Gradient
    }
}

/// A radial gradient's focal point as an offset from its center, kept
/// inside the circle (SVG moves a focal point outside onto the edge;
/// Direct2D would draw a cone).
pub fn focal_offset(cx: f32, cy: f32, r: f32, fx: f32, fy: f32) -> (f32, f32) {
    let (dx, dy) = (fx - cx, fy - cy);
    let d = dx.hypot(dy);
    let most = r * 0.99;
    if d > most && d > 0. {
        (dx * most / d, dy * most / d)
    } else {
        (dx, dy)
    }
}

// --- Pixels ------------------------------------------------------------------

/// Blurs `px` (`w` × `h` premultiplied BGRA) with a Gaussian of standard
/// deviation `sigma` pixels; what lies outside is transparent.
pub fn gaussian(px: &mut [u8], w: u32, h: u32, sigma: f32) {
    let (w, h) = (w as usize, h as usize);
    if sigma <= 0. || px.len() < w * h * 4 {
        return;
    }
    let Some(area) = inked(px, w, h, reach(sigma)) else {
        return;
    };
    for channel in 0..4 {
        let mut plane = take_plane(px, w, area, channel);
        blur_plane(&mut plane, area.width(), area.height(), sigma);
        put_plane(px, w, area, channel, &plane, 1.);
    }
}

/// The shadow `px` (`w` × `h` premultiplied BGRA) casts: its alpha blurred
/// by `sigma` pixels (0: a sharp copy), in `rgba` (straight, 0 to 1). Not
/// moved: it is drawn at its offset.
pub fn shadow(px: &[u8], w: u32, h: u32, sigma: f32, rgba: [f32; 4]) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0; w * h * 4];
    if px.len() < out.len() {
        return out;
    }
    let Some(area) = inked(px, w, h, reach(sigma)) else {
        return out;
    };
    let mut plane = take_plane(px, w, area, 3);
    blur_plane(&mut plane, area.width(), area.height(), sigma);
    // Premultiplied: every channel is the color times the shadow's alpha.
    let [r, g, b, a] = rgba.map(|v| v.clamp(0., 1.));
    for (channel, k) in [(0, b * a), (1, g * a), (2, r * a), (3, a)] {
        put_plane(&mut out, w, area, channel, &plane, k);
    }
    out
}

/// A luminance mask from what was drawn (`px`, premultiplied BGRA): each
/// pixel's alpha becomes its brightness times its alpha (0.2126 R +
/// 0.7152 G + 0.0722 B; of premultiplied channels, that is the product),
/// which is what an opacity brush reads.
pub fn luminance_to_alpha(px: &mut [u8]) {
    for p in px.as_chunks_mut::<4>().0 {
        let l = 0.0722 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.2126 * p[2] as f32;
        *p = [l.round().clamp(0., 255.) as u8; 4];
    }
}

/// How many pixels a blur of `sigma` spreads what it blurs.
fn reach(sigma: f32) -> usize {
    if sigma <= 0. {
        0
    } else {
        (3. * sigma).ceil() as usize + 1
    }
}

/// A rectangle of pixels, `x0..x1` by `y0..y1`: it may reach outside the
/// picture, where everything is transparent.
#[derive(Clone, Copy)]
struct Area {
    x0: isize,
    y0: isize,
    x1: isize,
    y1: isize,
}

impl Area {
    fn width(&self) -> usize {
        (self.x1 - self.x0) as usize
    }

    fn height(&self) -> usize {
        (self.y1 - self.y0) as usize
    }
}

/// The rectangle of `px` that holds something, grown by `grow` pixels,
/// past the picture's edges too: a blur in several passes needs the room
/// (what one pass pushes out, the next brings back in part).
fn inked(px: &[u8], w: usize, h: usize, grow: usize) -> Option<Area> {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        let row = &px[y * w * 4..(y + 1) * w * 4];
        let Some(first) = row.iter().skip(3).step_by(4).position(|a| *a != 0) else {
            continue;
        };
        let last = row
            .iter()
            .skip(3)
            .step_by(4)
            .rposition(|a| *a != 0)
            .unwrap_or(first);
        x0 = x0.min(first);
        x1 = x1.max(last + 1);
        y0 = y0.min(y);
        y1 = y + 1;
    }
    let grow = grow as isize;
    (x1 > x0).then(|| Area {
        x0: x0 as isize - grow,
        y0: y0 as isize - grow,
        x1: x1 as isize + grow,
        y1: y1 as isize + grow,
    })
}

/// The part of `area` that is in a `w` × `h` picture: its columns and
/// its rows.
fn inside(area: Area, w: usize, h: usize) -> (std::ops::Range<isize>, std::ops::Range<isize>) {
    (
        area.x0.max(0)..area.x1.min(w as isize),
        area.y0.max(0)..area.y1.min(h as isize),
    )
}

/// One channel of `px` over `area`.
fn take_plane(px: &[u8], w: usize, area: Area, channel: usize) -> Vec<f32> {
    let mut plane = vec![0.; area.width() * area.height()];
    let (columns, rows) = inside(area, w, px.len() / (w * 4));
    for y in rows {
        for x in columns.clone() {
            let at = (y - area.y0) as usize * area.width() + (x - area.x0) as usize;
            plane[at] = px[(y as usize * w + x as usize) * 4 + channel] as f32;
        }
    }
    plane
}

/// Writes `plane` (over `area`) times `k` back as one channel of `px`.
fn put_plane(px: &mut [u8], w: usize, area: Area, channel: usize, plane: &[f32], k: f32) {
    let (columns, rows) = inside(area, w, px.len() / (w * 4));
    for y in rows {
        for x in columns.clone() {
            let at = (y - area.y0) as usize * area.width() + (x - area.x0) as usize;
            px[(y as usize * w + x as usize) * 4 + channel] =
                (plane[at] * k).round().clamp(0., 255.) as u8;
        }
    }
}

/// A Gaussian blur of a `w` × `h` plane: its rows, then its columns.
fn blur_plane(plane: &mut Vec<f32>, w: usize, h: usize, sigma: f32) {
    if sigma <= 0. || w == 0 || h == 0 {
        return;
    }
    blur_rows(plane, w, sigma);
    let mut turned = transpose(plane, w, h);
    blur_rows(&mut turned, h, sigma);
    *plane = transpose(&turned, h, w);
}

fn transpose(plane: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.; plane.len()];
    for y in 0..h {
        for x in 0..w {
            out[x * h + y] = plane[y * w + x];
        }
    }
    out
}

/// Blurs each `w`-long row. A small blur weighs its neighbors by the
/// Gaussian itself; a larger one is three box blurs in a row, which come
/// to the same within a few percent whatever its size (the sizes are the
/// SVG filter specification's).
fn blur_rows(plane: &mut [f32], w: usize, sigma: f32) {
    let mut out = vec![0.; w];
    if sigma < 2. {
        let r = (3. * sigma).ceil() as isize;
        let weights: Vec<f32> = (-r..=r)
            .map(|i| (-((i * i) as f32) / (2. * sigma * sigma)).exp())
            .collect();
        let total: f32 = weights.iter().sum();
        for row in plane.chunks_mut(w) {
            for (i, o) in out.iter_mut().enumerate() {
                let mut sum = 0.;
                for (k, weight) in weights.iter().enumerate() {
                    let j = i as isize + k as isize - r;
                    if j >= 0 && (j as usize) < w {
                        sum += row[j as usize] * weight;
                    }
                }
                *o = sum / total;
            }
            row.copy_from_slice(&out);
        }
        return;
    }
    let d = ((sigma * 3. * (2. * std::f32::consts::PI).sqrt() / 4. + 0.5).floor() as usize).max(1);
    // (size, how many values before each one): an even size cannot be
    // centered, so it leans left once and right once.
    let boxes = if d % 2 == 1 {
        [(d, d / 2); 3]
    } else {
        [(d, d / 2), (d, d / 2 - 1), (d + 1, d / 2)]
    };
    for row in plane.chunks_mut(w) {
        for (size, before) in boxes {
            box_blur(row, &mut out, size, before);
            row.copy_from_slice(&out);
        }
    }
}

/// `out[i]` is the mean of the `size` values of `row` from `before` ahead
/// of `i` on (what is outside counts as 0).
fn box_blur(row: &[f32], out: &mut [f32], size: usize, before: usize) {
    let n = row.len() as isize;
    let at = |j: isize| {
        if j >= 0 && j < n { row[j as usize] } else { 0. }
    };
    let (size_i, before) = (size as isize, before as isize);
    let mut sum: f32 = (-before..size_i - before).map(at).sum();
    for (i, o) in out.iter_mut().enumerate() {
        *o = sum / size as f32;
        let i = i as isize;
        sum += at(i - before + size_i) - at(i - before);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::shape::{Group, Path, Shadow, Text};

    #[test]
    fn flat_paths_are_xaml_paths_and_the_rest_a_picture() {
        let flat = Item::Path(Path::rect(0., 0., 1., 1.).fill(Color::BLACK));
        assert!(is_flat(std::slice::from_ref(&flat)));
        assert!(is_flat(&[]));
        let gradient = Gradient::linear((0., 0.), (1., 0.), [(0., Color::BLACK)]);
        let filled = Item::Path(Path::rect(0., 0., 1., 1.).fill_gradient(gradient.clone()));
        assert!(!is_flat(&[flat.clone(), filled]));
        let stroked = Item::Path(Path::rect(0., 0., 1., 1.).stroke_gradient(gradient, 2.));
        assert!(!is_flat(&[stroked]));
        assert!(!is_flat(&[Item::Text(Text::default())]));
        assert!(!is_flat(&[Item::Group(Group::default())]));
    }

    #[test]
    fn margin_is_the_reach_of_blurs_and_shadows() {
        assert_eq!(margin(&[]), 0.);
        let shadow = |dx, dy, blur| Shadow {
            dx,
            dy,
            blur,
            color: Color::BLACK,
        };
        let inner = Group {
            blur: 2.,
            ..Group::default()
        };
        let outer = Group {
            shadows: vec![shadow(0., 4., 8.), shadow(-30., 0., 0.)],
            items: vec![Item::Group(inner)],
            ..Group::default()
        };
        // The widest shadow (30 away) and, inside, a blur of 2.
        assert_eq!(margin(&[Item::Group(outer)]), 30. + 6.);
    }

    #[test]
    fn an_image_fills_fits_or_covers_its_rectangle() {
        let rect = (10., 20., 100., 50.);
        // Stretched: all of it over the rectangle.
        assert_eq!(
            fit(Fit::Stretch, (0.5, 0.5), rect, (8., 8.)),
            Some(([10., 20., 110., 70.], [0., 0., 8., 8.]))
        );
        // Whole: 50 × 50, centered, at the left, at the right.
        let (dest, src) = fit(Fit::Meet, (0.5, 0.5), rect, (8., 8.)).unwrap();
        assert_eq!((dest, src), ([35., 20., 85., 70.], [0., 0., 8., 8.]));
        let (dest, _) = fit(Fit::Meet, (0., 0.), rect, (8., 8.)).unwrap();
        assert_eq!(dest, [10., 20., 60., 70.]);
        let (dest, _) = fit(Fit::Meet, (1., 1.), rect, (8., 8.)).unwrap();
        assert_eq!(dest, [60., 20., 110., 70.]);
        // Covering: the rectangle shows the bitmap's middle half, its top
        // or its bottom.
        let (dest, src) = fit(Fit::Slice, (0.5, 0.5), rect, (8., 8.)).unwrap();
        assert_eq!((dest, src), ([10., 20., 110., 70.], [0., 2., 8., 6.]));
        let (_, src) = fit(Fit::Slice, (0.5, 0.), rect, (8., 8.)).unwrap();
        assert_eq!(src, [0., 0., 8., 4.]);
        let (_, src) = fit(Fit::Slice, (0.5, 1.), rect, (8., 8.)).unwrap();
        assert_eq!(src, [0., 4., 8., 8.]);
        // Nothing to show.
        assert_eq!(fit(Fit::Meet, (0.5, 0.5), (0., 0., 0., 5.), (8., 8.)), None);
        assert_eq!(fit(Fit::Meet, (0.5, 0.5), rect, (0., 8.)), None);
    }

    #[test]
    fn a_line_sits_around_its_anchor_and_on_its_baseline() {
        assert_eq!(line_start(Anchor::Start, 50., 20.), 50.);
        assert_eq!(line_start(Anchor::Middle, 50., 20.), 40.);
        assert_eq!(line_start(Anchor::End, 50., 20.), 30.);
        assert_eq!(baseline_drop(Baseline::Alphabetic, 10., 3., 5.), 0.);
        assert_eq!(baseline_drop(Baseline::Middle, 10., 3., 5.), 2.5);
        assert_eq!(baseline_drop(Baseline::Central, 10., 3., 5.), 3.5);
        assert_eq!(baseline_drop(Baseline::Hanging, 10., 3., 5.), 8.);
    }

    #[test]
    fn a_gradient_without_extent_is_its_last_color() {
        let (red, blue) = (prism_core::rgb(0xff0000), prism_core::rgb(0x0000ff));
        let stops = [(0., red), (1., blue)];
        assert_eq!(
            paint(&Gradient::linear((0., 0.), (1., 0.), stops)),
            Paint::Gradient
        );
        assert_eq!(
            paint(&Gradient::linear((0.5, 0.5), (0.5, 0.5), stops)),
            Paint::Flat(blue)
        );
        assert_eq!(
            paint(&Gradient::radial((0.5, 0.5), 0., stops)),
            Paint::Flat(blue)
        );
        assert_eq!(
            paint(&Gradient::linear((0., 0.), (1., 0.), [(0.3, red)])),
            Paint::Flat(red)
        );
        assert_eq!(
            paint(&Gradient::linear((0., 0.), (1., 0.), [])),
            Paint::None
        );
    }

    #[test]
    fn a_focal_point_stays_in_its_circle() {
        assert_eq!(focal_offset(50., 50., 10., 47., 54.), (-3., 4.));
        let (dx, dy) = focal_offset(50., 50., 10., 80., 90.);
        assert!((dx.hypot(dy) - 9.9).abs() < 1e-3, "{dx} {dy}");
        assert!((dx / dy - 0.75).abs() < 1e-3);
    }

    /// A `w` × `h` transparent picture with an opaque white square of
    /// `side` pixels at `(x, y)`: enough ink for whole numbers to hold
    /// its blur.
    fn square(w: usize, h: usize, (x, y): (usize, usize), side: usize) -> Vec<u8> {
        let mut px = vec![0; w * h * 4];
        for row in y..y + side {
            px[(row * w + x) * 4..(row * w + x + side) * 4].fill(255);
        }
        px
    }

    /// The total and the standard deviation along x of a picture's alpha.
    fn spread(px: &[u8], w: usize) -> (f32, f32) {
        let alpha = |i: usize| px[i * 4 + 3] as f32;
        let n = px.len() / 4;
        let total: f32 = (0..n).map(alpha).sum();
        let mean: f32 = (0..n).map(|i| (i % w) as f32 * alpha(i)).sum::<f32>() / total;
        let var: f32 = (0..n)
            .map(|i| ((i % w) as f32 - mean).powi(2) * alpha(i))
            .sum::<f32>()
            / total;
        (total, var.sqrt())
    }

    #[test]
    fn a_blur_spreads_by_its_standard_deviation() {
        // A small one (the Gaussian's own weights), an even and an odd box.
        for sigma in [1.2, 4., 6.5] {
            let (w, h) = (96, 96);
            let mut px = square(w, h, (44, 44), 9);
            let (before, wide) = spread(&px, w);
            gaussian(&mut px, w as u32, h as u32, sigma);
            let (after, wider) = spread(&px, w);
            // Nothing is lost, and variances add.
            assert!((after / before - 1.).abs() < 0.02, "{sigma}: {after}");
            let got = (wider * wider - wide * wide).sqrt();
            assert!((got / sigma - 1.).abs() < 0.08, "{sigma}: {got}");
            // Around its center: left and right, up and down alike.
            let a = |x: usize, y: usize| px[(y * w + x) * 4 + 3] as i32;
            for d in 1..20 {
                assert!((a(48 - d, 48) - a(48 + d, 48)).abs() <= 1, "{sigma} {d}");
                assert!((a(48, 48 - d) - a(48 + d, 48)).abs() <= 1, "{sigma} {d}");
            }
            // The colors go with the alpha (premultiplied white).
            assert_eq!(px[(48 * w + 40) * 4], px[(48 * w + 40) * 4 + 3]);
        }
    }

    #[test]
    fn a_blur_leaves_an_empty_picture_and_no_blur_alone() {
        let mut px = vec![0; 8 * 8 * 4];
        gaussian(&mut px, 8, 8, 3.);
        assert!(px.iter().all(|v| *v == 0));
        let mut px = square(8, 8, (3, 3), 1);
        gaussian(&mut px, 8, 8, 0.);
        assert_eq!(px, square(8, 8, (3, 3), 1));
    }

    #[test]
    fn a_blur_at_the_edge_loses_what_leaves_the_picture() {
        // A column against the left edge: about half of it goes out.
        let mut px = vec![0; 32 * 32 * 4];
        for y in 8..24 {
            px[y * 32 * 4..y * 32 * 4 + 4].fill(255);
        }
        gaussian(&mut px, 32, 32, 3.);
        let (total, _) = spread(&px, 32);
        let kept = total / (16. * 255.);
        assert!(kept > 0.5 && kept < 0.62, "{kept}");
    }

    #[test]
    fn a_shadow_is_the_alpha_in_its_color() {
        // Half transparent red ink casts the shadow's color at half its
        // alpha, whatever the ink's color.
        let mut px = vec![0; 4 * 4 * 4];
        px[(4 + 1) * 4..(4 + 1) * 4 + 4].copy_from_slice(&[0, 0, 128, 128]);
        let s = shadow(&px, 4, 4, 0., [0., 0.5, 1., 0.5]);
        // Premultiplied BGRA: alpha 128 × 0.5, blue 1, green 0.5, red 0.
        assert_eq!(&s[(4 + 1) * 4..(4 + 1) * 4 + 4], &[64, 32, 0, 64]);
        assert!(s[..16].iter().all(|v| *v == 0));
        // Blurred: it spreads, and keeps its total.
        let px = square(48, 48, (20, 20), 8);
        let (ink, narrow) = spread(&px, 48);
        let s = shadow(&px, 48, 48, 2.5, [0., 0., 0., 1.]);
        let (total, wide) = spread(&s, 48);
        assert!((total / ink - 1.).abs() < 0.02, "{total}");
        let got = (wide * wide - narrow * narrow).sqrt();
        assert!((got / 2.5 - 1.).abs() < 0.1, "{got}");
        assert!(s.as_chunks::<4>().0.iter().all(|p| p[..3] == [0, 0, 0]));
        // Nothing casts nothing.
        assert!(
            shadow(&[0; 64], 4, 4, 2., [0., 0., 0., 1.])
                .iter()
                .all(|v| *v == 0)
        );
    }

    #[test]
    fn a_luminance_mask_shows_as_much_as_it_is_bright() {
        // Premultiplied BGRA: white, black, nothing, half white, red.
        let mut px = vec![
            255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 0, 128, 128, 128, 128, 0, 0, 255, 255,
        ];
        luminance_to_alpha(&mut px);
        let alphas: Vec<u8> = px.iter().skip(3).step_by(4).copied().collect();
        assert_eq!(alphas, [255, 0, 0, 128, 54]);
    }
}
