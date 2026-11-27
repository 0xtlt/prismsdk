//! `ui::qr_code(text)`: a QR code drawn as one vector shape (the encoder is
//! [`crate::qr`]), so it is native on every backend, widgets included.

use std::collections::HashMap;

use prism_core::{App, Color, Element, IntoElement, Path, Styled, px, rgb, shape};

use crate::qr::{QrCode, QrEcc};
use crate::{Component, Refine};

/// A QR code for some text (a URL, a pairing payload).
pub struct Qr {
    /// What the code holds (UTF-8, byte mode).
    pub text: String,
    /// Error correction (default [`QrEcc::Medium`]).
    pub ecc: QrEcc,
    /// Side in points, quiet zone included (default 160).
    pub size: f32,
    /// Light margin around the modules, in modules (default 4, the
    /// standard; scanners need some).
    pub quiet_zone: usize,
    /// Module color (default black).
    pub dark: Color,
    /// Background color (default white, in dark mode too: scanners read
    /// dark on light).
    pub light: Color,
    /// Spoken description (default "QR code").
    pub alt: Option<String>,
    refine: Refine,
}

/// A QR code holding `text`, smallest version that fits at the chosen
/// error correction. Text longer than a version 40 code holds draws an empty
/// square.
///
/// ```ignore
/// ui::qr_code(pairing_url).size(180.).ecc(QrEcc::Quartile).alt("Pairing code")
/// ```
pub fn qr_code(text: impl Into<String>) -> Qr {
    Qr {
        text: text.into(),
        ecc: QrEcc::Medium,
        size: 160.,
        quiet_zone: 4,
        dark: rgb(0x000000),
        light: rgb(0xffffff),
        alt: None,
        refine: Refine::default(),
    }
}

impl Qr {
    /// Error correction: `Low` holds the most data, `High` survives the
    /// most damage (a logo over it).
    pub fn ecc(mut self, ecc: QrEcc) -> Self {
        self.ecc = ecc;
        self
    }
    /// Side in points, quiet zone included (default 160).
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
    /// Margin in modules (default 4).
    pub fn quiet_zone(mut self, modules: usize) -> Self {
        self.quiet_zone = modules;
        self
    }
    /// Module and background colors (keep strong contrast, dark on light).
    pub fn colors(mut self, dark: Color, light: Color) -> Self {
        self.dark = dark;
        self.light = light;
        self
    }
    /// Spoken description ("Pairing code").
    pub fn alt(mut self, alt: impl Into<String>) -> Self {
        self.alt = Some(alt.into());
        self
    }

    /// The paths: the background, then every dark module as rectangles
    /// (runs merged across rows), in fractions of the square.
    pub fn paths(&self) -> Vec<Path> {
        let mut paths = vec![Path::rect(0., 0., 1., 1.).fill(self.light)];
        if let Ok(code) = QrCode::encode_text(&self.text, self.ecc) {
            paths.push(modules(&code, self.quiet_zone).fill(self.dark));
        }
        paths
    }
}

/// Dark modules as rectangles: identical runs on consecutive rows become one.
fn modules(code: &QrCode, quiet: usize) -> Path {
    let total = (code.size() + 2 * quiet) as f32;
    let f = |v: usize| (v + quiet) as f32 / total;
    // (x, len) → (first row, rows) of rectangles still growing downwards.
    let mut open: HashMap<(usize, usize), (usize, usize)> = HashMap::new();
    let mut rects: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut row = usize::MAX;
    let mut seen: Vec<(usize, usize)> = Vec::new();
    let close_unseen = |open: &mut HashMap<(usize, usize), (usize, usize)>,
                        seen: &[(usize, usize)],
                        rects: &mut Vec<(usize, usize, usize, usize)>| {
        let gone: Vec<(usize, usize)> =
            open.keys().filter(|k| !seen.contains(k)).copied().collect();
        for k in gone {
            let (y, h) = open.remove(&k).expect("open run");
            rects.push((k.0, y, k.1, h));
        }
    };
    for (x, y, len) in code.dark_runs() {
        if y != row {
            if row != usize::MAX {
                close_unseen(&mut open, &seen, &mut rects);
            }
            seen.clear();
            row = y;
        }
        seen.push((x, len));
        match open.get_mut(&(x, len)) {
            // It ended on the row above: it grows.
            Some((top, h)) if *top + *h == y => *h += 1,
            Some(&mut (top, h)) => {
                rects.push((x, top, len, h));
                open.insert((x, len), (y, 1));
            }
            None => {
                open.insert((x, len), (y, 1));
            }
        }
    }
    seen.clear();
    close_unseen(&mut open, &seen, &mut rects);
    rects.sort();
    let mut p = Path::new();
    for (x, y, w, h) in rects {
        let (x0, y0, x1, y1) = (f(x), f(y), f(x + w), f(y + h));
        p = p
            .move_to(x0, y0)
            .line_to(x1, y0)
            .line_to(x1, y1)
            .line_to(x0, y1)
            .close();
    }
    p
}

refinable!(Qr);

impl Component for Qr {
    fn render(self, _cx: &mut App) -> Element {
        let paths = self.paths();
        let label = self.alt.clone().unwrap_or_else(|| "QR code".into());
        let el = shape()
            .paths(paths)
            .size(px(self.size))
            .flex_shrink_0()
            .label(label);
        self.refine.apply(el).into_element()
    }
}

component!(Qr);

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::Segment;

    /// The modules back from the drawn rectangles, sampled at module centers.
    fn sample(p: &Path, n: usize, quiet: usize) -> Vec<bool> {
        let total = (n + 2 * quiet) as f32;
        let mut rects = Vec::new();
        let mut cur = Vec::new();
        for s in &p.segments {
            match *s {
                Segment::Move(x, y) | Segment::Line(x, y) => cur.push((x, y)),
                Segment::Close => {
                    let (x0, y0) = cur[0];
                    let (x1, y1) = cur[2];
                    rects.push((x0, y0, x1, y1));
                    cur.clear();
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let (cx, cy) = (
                    (x + quiet) as f32 / total + 0.5 / total,
                    (y + quiet) as f32 / total + 0.5 / total,
                );
                out.push(
                    rects
                        .iter()
                        .any(|r| cx > r.0 && cx < r.2 && cy > r.1 && cy < r.3),
                );
            }
        }
        out
    }

    #[test]
    fn merged_rectangles_cover_exactly_the_dark_modules() {
        for text in [
            "HELLO",
            "https://example.com/pair?code=8F3K-22QZ",
            &"x".repeat(300),
        ] {
            let q = qr_code(text);
            let code = QrCode::encode_text(text, q.ecc).unwrap();
            let paths = q.paths();
            assert_eq!(paths.len(), 2);
            assert_eq!(paths[0].fill, Some(q.light));
            let got = sample(&paths[1], code.size(), q.quiet_zone);
            let n = code.size();
            let want: Vec<bool> = (0..n * n).map(|i| code.dark(i % n, i / n)).collect();
            assert_eq!(got, want, "{text}");
            // Fewer rectangles than horizontal runs (rows merged).
            let rects = paths[1]
                .segments
                .iter()
                .filter(|s| **s == Segment::Close)
                .count();
            assert!(rects < code.dark_runs().len(), "{text}");
        }
    }

    #[test]
    fn the_drawing_rasterized_at_odd_sizes_decodes() {
        // Pixels at a size that is not a multiple of the module count, filled
        // where a pixel's center falls in a drawn rectangle: what a renderer does.
        for (text, side) in [
            ("https://prism.dev/pair/42", 173usize),
            ("Wi-Fi: hôtel ☕", 241),
        ] {
            let q = qr_code(text).ecc(QrEcc::High);
            // Through the text form backends get (coordinates rounded to 0.001).
            let shape = prism_core::Drawing {
                inset: (0., 0.),
                paths: q.paths(),
                ..Default::default()
            };
            let paths = prism_core::Drawing::decode(&shape.encode()).paths;
            let mut rects = Vec::new();
            let mut cur = Vec::new();
            for s in &paths[1].segments {
                match *s {
                    Segment::Move(x, y) | Segment::Line(x, y) => cur.push((x, y)),
                    Segment::Close => {
                        rects.push((cur[0].0, cur[0].1, cur[2].0, cur[2].1));
                        cur.clear();
                    }
                    _ => {}
                }
            }
            let mut img = rqrr::PreparedImage::prepare_from_greyscale(side, side, |x, y| {
                let (fx, fy) = (
                    (x as f32 + 0.5) / side as f32,
                    (y as f32 + 0.5) / side as f32,
                );
                let dark = rects
                    .iter()
                    .any(|r| fx >= r.0 && fx < r.2 && fy >= r.1 && fy < r.3);
                if dark { 0 } else { 255 }
            });
            let grids = img.detect_grids();
            assert_eq!(grids.len(), 1, "{text}");
            let (_, content) = grids[0].decode().expect("decodes");
            assert_eq!(content, text);
        }
    }

    #[test]
    fn too_long_text_draws_only_the_background() {
        assert_eq!(qr_code("x".repeat(4000)).paths().len(), 1);
    }
}
