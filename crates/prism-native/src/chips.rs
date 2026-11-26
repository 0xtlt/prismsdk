//! A rich input's chip as native backends draw it (#115, #329): the box
//! (padding, icon, gap, label, its height and largest width, radius,
//! ring), shared by AppKit, UIKit, GTK and Windows so a chip measures the
//! same everywhere. Android (Java) follows the same numbers.

use prism_core::{Color, Path, TextRun};

/// The default radius of a chip's corners, points.
pub const RADIUS: f32 = 4.;

/// Where a chip's parts go, points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChipBox {
    /// Free on the left and on the right.
    pub pad: f32,
    /// The icon's width (0: no icon) and height.
    pub icon_w: f32,
    pub icon_h: f32,
    /// Between the icon and the label.
    pub gap: f32,
    /// The room for the label: its width, or less when the chip is cut
    /// at its largest width (the label then ends with an ellipsis).
    pub label_w: f32,
    /// Whether the label is cut.
    pub cut: bool,
    /// The whole box.
    pub width: f32,
    pub height: f32,
    /// Its corners' radius.
    pub radius: f32,
}

/// How wide `run`'s icon is for a font of `em` points: a symbol is one
/// em, a drawing `ratio` em, none 0.
pub fn icon_width(run: &TextRun, em: f32) -> f32 {
    if run.chip_drawing.is_some() {
        em * run.chip_icon_ratio.filter(|r| *r > 0.).unwrap_or(1.)
    } else if run.chip_icon.is_some() {
        em
    } else {
        0.
    }
}

/// The box of chip `run` drawn in a font of `em` points whose text is
/// `text_h` tall, its label `label_w` wide (all its runs).
pub fn chip_box(run: &TextRun, em: f32, text_h: f32, label_w: f32) -> ChipBox {
    let em = em.max(1.);
    let pad = (em * 0.35).round();
    let icon_w = icon_width(run, em);
    let gap = if icon_w > 0. && label_w > 0. {
        (em * 0.25).round()
    } else {
        0.
    };
    let natural = (pad * 2. + icon_w + gap + label_w).ceil();
    let fixed = pad * 2. + icon_w + gap;
    let width = match run.chip_max_width {
        Some(max) if max < natural => max.max(fixed).ceil(),
        _ => natural,
    };
    let room = (width - fixed).max(0.);
    ChipBox {
        pad,
        icon_w,
        icon_h: if run.chip_drawing.is_some() {
            em
        } else {
            em * 0.9
        },
        gap,
        label_w: room.min(label_w),
        cut: room + 0.5 < label_w,
        width,
        height: run.chip_height.unwrap_or(0.).max(text_h).ceil(),
        radius: run.chip_radius.unwrap_or(RADIUS),
    }
}

/// The paths of `run`'s vector icon in a `w` × `h` box, its
/// [`Color::current`] painted in `text` (the chip's text color).
pub fn icon_paths(run: &TextRun, w: f32, h: f32, text: Color) -> Vec<Path> {
    run.chip_drawing
        .as_ref()
        .map(|d| d.with_current_color(text).resolve(w, h, 1.))
        .unwrap_or_default()
}

/// `paths` moved by (`dx`, `dy`): an icon drawn at its place in the chip.
pub fn offset(paths: &mut [Path], dx: f32, dy: f32) {
    use prism_core::Segment as S;
    for p in paths {
        for s in &mut p.segments {
            match s {
                S::Move(x, y) | S::Line(x, y) => {
                    *x += dx;
                    *y += dy;
                }
                S::Cubic(a, b, c, d, e, f) => {
                    *a += dx;
                    *b += dy;
                    *c += dx;
                    *d += dy;
                    *e += dx;
                    *f += dy;
                }
                S::Arc { cx, cy, .. } => {
                    *cx += dx;
                    *cy += dy;
                }
                S::Close => {}
            }
        }
    }
}

/// What stands for chip `run` (one character in the field) when a
/// backend measures a rich input as plain text in a font of `em` points:
/// as wide as its box. Em and en spaces stand for its padding and icon,
/// joined to its label (spaces made non-breaking, cut at its largest
/// width at about 0.55 em a character) so they never break from it.
pub fn stand_in(run: &TextRun, em: f32) -> String {
    const JOIN: char = '\u{2060}';
    let em = em.max(1.);
    let mut label: String = run
        .label_runs()
        .iter()
        .map(|r| r.text.as_str())
        .collect::<String>()
        .replace(' ', "\u{a0}");
    if let Some(max) = run.chip_max_width {
        let fixed = chip_box(run, em, 0., 0.).width;
        let fits = ((max - fixed).max(0.) / (em * 0.55)).floor() as usize;
        if label.chars().count() > fits {
            label = label
                .chars()
                .take(fits.saturating_sub(1))
                .collect::<String>()
                + "…";
        }
    }
    // A vector icon as wide as its ratio: one or two em spaces.
    let room = if run.chip_icon.is_some() || run.chip_drawing.is_some() {
        let extra = if icon_width(run, em) > em * 1.5 {
            "\u{2003}"
        } else {
            ""
        };
        format!("\u{2003}{JOIN}\u{2003}{extra}")
    } else {
        format!("\u{2002}{JOIN}\u{2009}")
    };
    format!("{JOIN}{room}{JOIN}{label}{JOIN}")
}

/// How much taller than `lines` lines `lh` points tall the chips of
/// `runs` (their [`chip_height`](TextRun::chip_height)) make a rich
/// input's text: the tallest ones, at most one a line.
pub fn extra_height(runs: &[TextRun], lh: f32, lines: f32) -> f32 {
    let mut extra: Vec<f32> = runs
        .iter()
        .filter(|r| r.chip.is_some())
        .filter_map(|r| r.chip_height)
        .map(|h| (h - lh).max(0.))
        .filter(|e| *e > 0.)
        .collect();
    extra.sort_by(|a, b| b.total_cmp(a));
    extra.iter().take(lines.max(1.) as usize).sum()
}

/// `text` cut to fit `fits` with an ellipsis: the longest start of it
/// (by characters) that `fits` with "…" after it. For backends that cut
/// the label themselves.
pub fn ellipsize(text: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(text) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let s: String = chars[..mid].iter().collect::<String>() + "…";
        if fits(&s) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo]
        .iter()
        .collect::<String>()
        .trim_end()
        .to_string()
        + "…"
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::Drawing;

    fn run() -> TextRun {
        TextRun {
            chip: Some("k".into()),
            chip_label: Some("Label".into()),
            ..TextRun::default()
        }
    }

    #[test]
    fn a_plain_chip_measures_as_before() {
        let b = chip_box(&run(), 13., 16., 40.);
        assert_eq!((b.pad, b.icon_w, b.gap), (5., 0., 0.));
        assert_eq!((b.width, b.height, b.radius), (50., 16., 4.));
        assert!(!b.cut);
        let b = chip_box(
            &TextRun {
                chip_icon: Some("star".into()),
                ..run()
            },
            13.,
            16.,
            40.,
        );
        assert_eq!((b.icon_w, b.gap, b.width), (13., 3., 66.));
    }

    #[test]
    fn options_shape_the_box() {
        let r = TextRun {
            chip_drawing: Some(Drawing::default()),
            chip_icon_ratio: Some(1.5),
            chip_max_width: Some(80.),
            chip_height: Some(22.),
            chip_radius: Some(11.),
            ..run()
        };
        let b = chip_box(&r, 14., 17., 200.);
        assert_eq!(b.icon_w, 21.);
        assert_eq!(b.width, 80.);
        assert!(b.cut);
        assert_eq!(b.label_w, 80. - 10. - 21. - 4.);
        assert_eq!((b.height, b.radius), (22., 11.));
        // Never shorter than its text.
        let b = chip_box(&r, 14., 30., 10.);
        assert_eq!(b.height, 30.);
        assert!(!b.cut);
    }

    #[test]
    fn ellipsize_cuts_by_characters() {
        let fits = |s: &str| s.chars().count() <= 6;
        assert_eq!(ellipsize("short", fits), "short");
        assert_eq!(ellipsize("a longer one", fits), "a lon…");
        assert_eq!(ellipsize("abcdefghij", |_| false), "…");
    }
}
