//! Pixels and outlines XAML cannot draw by itself: a conic gradient's
//! pixels (an `ImageBrush` shows them), and the shape of each side of a
//! border whose sides differ in color (one `Path` each, split at the
//! corners like CSS). No Win32 types: tested on every host.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

/// A gradient stop: where (0 to 1) and its color (straight RGBA, 0 to 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    pub at: f32,
    pub rgba: [f32; 4],
}

/// The color at `t` (0 to 1) of `stops` (in order), premultiplied.
fn color_at(stops: &[Stop], t: f32) -> [f32; 4] {
    let pre = |c: [f32; 4]| [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]];
    let Some(first) = stops.first() else {
        return [0.; 4];
    };
    if t <= first.at {
        return pre(first.rgba);
    }
    for w in stops.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.at {
            let span = b.at - a.at;
            let p = if span <= 0. { 1. } else { (t - a.at) / span };
            let (ca, cb) = (pre(a.rgba), pre(b.rgba));
            return std::array::from_fn(|i| ca[i] + (cb[i] - ca[i]) * p);
        }
    }
    pre(stops[stops.len() - 1].rgba)
}

/// A CSS `conic-gradient(from <from>deg, …)` over a `w` × `h` pixel box:
/// premultiplied BGRA rows, top first (what a `WriteableBitmap` takes).
/// The colors go clockwise from `from` degrees (0: up), the last one
/// meeting the first after a full turn.
pub fn conic(w: u32, h: u32, from: f32, stops: &[Stop]) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    let (cx, cy) = (w as f32 / 2., h as f32 / 2.);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            // Clockwise from up (y points down).
            let angle = dx.atan2(-dy).to_degrees();
            let t = (angle - from).rem_euclid(360.) / 360.;
            let c = color_at(stops, t);
            let b = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
            out.extend_from_slice(&[b(c[2]), b(c[1]), b(c[0]), b(c[3])]);
        }
    }
    out
}

/// The outline of each side of a `w` × `h` border (top, right, bottom,
/// left), `widths` thick, with outer corner `radii` (top left, top right,
/// bottom right, bottom left): the ring between the outer and the inner
/// edge, cut at each corner where CSS changes color (on the line from the
/// outer to the inner corner; along the corner's arc for rounded ones).
/// A side without width has no outline.
pub fn border_sides(w: f32, h: f32, widths: [f32; 4], radii: [f32; 4]) -> [Vec<(f32, f32)>; 4] {
    std::array::from_fn(|side| {
        if widths[side] <= 0. {
            return Vec::new();
        }
        // The side goes from the corner it leaves (`side`) to the one it
        // reaches (`side + 1`), clockwise.
        let (start, end) = (side, (side + 1) % 4);
        let a = Corner::new(start, w, h, widths, radii);
        let b = Corner::new(end, w, h, widths, radii);
        let mut pts = Vec::new();
        // Outer edge: the start corner's outgoing part, the end corner's
        // incoming part; then back along the inner edge.
        a.arc(&mut pts, a.split, 1., false);
        b.arc(&mut pts, 0., b.split, false);
        b.arc(&mut pts, b.split, 0., true);
        a.arc(&mut pts, 1., a.split, true);
        pts.dedup_by(|p, q| (p.0 - q.0).abs() < 1e-3 && (p.1 - q.1).abs() < 1e-3);
        pts
    })
}

/// A corner seen from inside its side pair: the side coming in (clockwise)
/// runs up the local left edge, the side going out along the local top
/// edge; `map` takes local points to the box.
struct Corner {
    index: usize,
    w: f32,
    h: f32,
    r: f32,
    width_in: f32,
    width_out: f32,
    /// Where the color changes, as a fraction of the quarter turn from the
    /// incoming side's end.
    split: f32,
}

impl Corner {
    fn new(index: usize, w: f32, h: f32, widths: [f32; 4], radii: [f32; 4]) -> Corner {
        // Corner i sits between side i - 1 (in) and side i (out).
        let width_in = widths[(index + 3) % 4].max(0.);
        let width_out = widths[index].max(0.);
        let split = 1. - width_out.atan2(width_in) / std::f32::consts::FRAC_PI_2;
        Corner {
            index,
            w,
            h,
            r: radii[index].max(0.),
            width_in,
            width_out,
            split: if width_in == 0. && width_out == 0. {
                0.5
            } else {
                split
            },
        }
    }

    fn map(&self, (x, y): (f32, f32)) -> (f32, f32) {
        match self.index {
            0 => (x, y),
            1 => (self.w - y, x),
            2 => (self.w - x, self.h - y),
            _ => (y, self.h - x),
        }
    }

    /// The outer (or inner) arc from fraction `from` to `to` of its quarter
    /// turn.
    fn arc(&self, pts: &mut Vec<(f32, f32)>, from: f32, to: f32, inner: bool) {
        let n = ((to - from).abs() * 12.).ceil().max(1.) as usize;
        for i in 0..=n {
            let f = from + (to - from) * i as f32 / n as f32;
            let a = f * std::f32::consts::FRAC_PI_2;
            let p = if inner {
                let rx = (self.r - self.width_in).max(0.);
                let ry = (self.r - self.width_out).max(0.);
                let (cx, cy) = (self.width_in + rx, self.width_out + ry);
                (cx - rx * a.cos(), cy - ry * a.sin())
            } else {
                (self.r - self.r * a.cos(), self.r - self.r * a.sin())
            };
            pts.push(self.map(p));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
    }

    #[test]
    fn square_sides_meet_on_the_diagonals() {
        let [top, right, bottom, left] = border_sides(10., 8., [1., 2., 3., 0.], [0.; 4]);
        let expect = |got: &[(f32, f32)], want: &[(f32, f32)]| {
            assert_eq!(got.len(), want.len(), "{got:?}");
            for (g, w) in got.iter().zip(want) {
                assert!(near(*g, *w), "{got:?} vs {want:?}");
            }
        };
        // No left side: the top runs from the outer corner to the inner one.
        expect(&top, &[(0., 0.), (10., 0.), (8., 1.), (0., 1.)]);
        expect(&right, &[(10., 0.), (10., 8.), (8., 5.), (8., 1.)]);
        expect(&bottom, &[(10., 8.), (0., 8.), (0., 5.), (8., 5.)]);
        assert!(left.is_empty());
    }

    #[test]
    fn rounded_sides_follow_the_corners() {
        let sides = border_sides(40., 20., [2.; 4], [8.; 4]);
        for s in &sides {
            assert!(s.len() > 8, "{s:?}");
            // Every point is on the box.
            assert!(
                s.iter()
                    .all(|p| p.0 >= -1e-3 && p.0 <= 40.001 && p.1 >= -1e-3 && p.1 <= 20.001)
            );
        }
        // The top's outer edge starts halfway round the top left corner
        // (equal widths): on the 45° point of its arc.
        let k = 8. - 8. * std::f32::consts::FRAC_1_SQRT_2;
        assert!(near(sides[0][0], (k, k)), "{:?}", sides[0][0]);
        // And reaches the top edge.
        assert!(sides[0].iter().any(|p| near(*p, (8., 0.))));
        assert!(sides[0].iter().any(|p| near(*p, (32., 0.))));
    }

    #[test]
    fn conic_colors_go_clockwise_from_the_start_angle() {
        let red = [1., 0., 0., 1.];
        let blue = [0., 0., 1., 1.];
        let stops = [Stop { at: 0., rgba: red }, Stop { at: 1., rgba: blue }];
        let px = conic(4, 4, 0., &stops);
        let at = |x: usize, y: usize| {
            let i = (y * 4 + x) * 4;
            (px[i + 2], px[i], px[i + 3])
        };
        // Just right of up: mostly red; just left of up: mostly blue.
        let (r, b, a) = at(2, 0);
        assert!(r > 200 && b < 60 && a == 255, "{r} {b}");
        let (r, b, _) = at(1, 0);
        assert!(b > 200 && r < 60, "{r} {b}");
        // Down is halfway.
        let (r, b, _) = at(2, 3);
        assert!(r.abs_diff(b) < 80, "{r} {b}");
        // From 180°: down is where it starts.
        let px = conic(4, 4, 180., &stops);
        assert!(px[(3 * 4 + 1) * 4 + 2] > 200);
    }

    #[test]
    fn conic_colors_are_premultiplied() {
        let stops = [Stop {
            at: 0.,
            rgba: [1., 1., 1., 0.5],
        }];
        let px = conic(1, 1, 0., &stops);
        assert_eq!(px, vec![128, 128, 128, 128]);
    }
}
