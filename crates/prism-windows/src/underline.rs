//! Underlines Prism draws itself (#221). XAML's own underline is one thin
//! line in the text's color (a `TextBlock`'s) or a RichEdit line style
//! without a color of its own, so a run whose underline has a color, or a
//! style a `TextBlock` lacks, is drawn: a stroked line under each line of
//! its characters, in a canvas over the text, like Android's spans. No
//! Win32: tested on every host.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::{Color, UnderlineStyle};
use prism_native::text::{StyledRun, StyledText};

/// A run's underline Prism draws: its run, characters (UTF-16, in the
/// whole text), color (`None`: the text's) and line.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    /// The run's index in the text.
    pub run: usize,
    pub start: usize,
    pub end: usize,
    pub color: Option<Color>,
    /// The run's text color (`None`: the paragraph's).
    pub ink: Option<Color>,
    pub style: UnderlineStyle,
    /// The run's font size.
    pub size: f32,
}

/// Whether a run's underline is drawn by Prism: it has a color, or (in a
/// `TextBlock`, `rich: false`) a line other than one thin one. An icon is
/// not underlined.
pub fn drawn(r: &StyledRun, rich: bool) -> bool {
    r.underline
        && r.icon.is_none()
        && (r.underline_color.is_some() || (!rich && r.underline_style != UnderlineStyle::Single))
}

/// The underlines of `st` Prism draws.
pub fn marks(st: &StyledText, rich: bool) -> Vec<Mark> {
    st.runs
        .iter()
        .enumerate()
        .filter(|(_, r)| drawn(r, rich) && !r.text.is_empty())
        .map(|(run, r)| Mark {
            run,
            start: r.utf16_start,
            end: r.utf16_start + r.text.encode_utf16().count(),
            color: r.underline_color,
            ink: r.color,
            style: r.underline_style,
            size: r.font.size.max(1.),
        })
        .collect()
}

/// One line's part of an underline: from `x0` to `x1`, the line's top and
/// height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub x0: f32,
    pub x1: f32,
    pub top: f32,
    pub height: f32,
}

/// The characters' boxes (x, y, width, height, in order) as one segment a
/// line: boxes of the same top join.
pub fn segments(boxes: impl IntoIterator<Item = (f32, f32, f32, f32)>) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    for (x, y, w, h) in boxes {
        let (l, r) = (x.min(x + w), x.max(x + w));
        match out.last_mut() {
            Some(s) if (s.top - y).abs() < 0.5 => {
                s.x0 = s.x0.min(l);
                s.x1 = s.x1.max(r);
                s.height = s.height.max(h);
            }
            _ => out.push(Segment {
                x0: l,
                x1: r,
                top: y,
                height: h,
            }),
        }
    }
    out.retain(|s| s.x1 - s.x0 > 0.5);
    out
}

/// How a line is stroked at a font size.
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub thickness: f32,
    /// Each line's center, under the baseline.
    pub offsets: Vec<f32>,
    /// Dash and gap, in thicknesses (XAML's unit).
    pub dash: Option<(f32, f32)>,
    /// Round caps (dots).
    pub round: bool,
}

/// `style` at font `size`: about the font's own underline (5 % of the size,
/// a pixel at least, 9 % under the baseline); thick as the web's (12 %, two
/// pixels at least); double two thin lines a line apart; dotted round dots
/// two thicknesses apart; dashed four on, two off.
pub fn stroke(style: UnderlineStyle, size: f32) -> Stroke {
    let thin = (size * 0.05).round().max(1.);
    let under = (size * 0.09).max(1.);
    let (thickness, dash, round) = match style {
        UnderlineStyle::Thick => ((size * 0.12).round().max(2.), None, false),
        UnderlineStyle::Dotted => (thin, Some((0.001, 2.)), true),
        UnderlineStyle::Dashed => (thin, Some((4., 2.)), false),
        UnderlineStyle::Single | UnderlineStyle::Double => (thin, None, false),
    };
    let first = under + thickness / 2.;
    let offsets = if style == UnderlineStyle::Double {
        vec![first, first + thickness * 2.]
    } else {
        vec![first]
    };
    Stroke {
        thickness,
        offsets,
        dash,
        round,
    }
}

impl Stroke {
    /// How far under the baseline its lowest line reaches: past a text's
    /// last line (a double underline's second line), less than a line.
    pub fn depth(&self) -> f32 {
        self.offsets.last().copied().unwrap_or(0.) + self.thickness / 2.
    }
}

/// A line's center at `y`, set so a whole-pixel line covers whole pixels.
pub fn snap(y: f32, thickness: f32) -> f32 {
    (y - thickness / 2.).round() + thickness / 2.
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::{Style, TextRun, rgb};

    fn run(text: &str, f: impl FnOnce(&mut TextRun)) -> TextRun {
        let mut r = TextRun {
            text: text.into(),
            ..Default::default()
        };
        f(&mut r);
        r
    }

    #[test]
    fn colored_and_styled_underlines_are_drawn() {
        let runs = [
            run("plain ", |_| {}),
            run("thin", |r| r.underline = true),
            run(" é ", |_| {}),
            run("colored", |r| {
                r.underline = true;
                r.underline_color = Some(rgb(0xE8A317));
            }),
            run("dotted", |r| {
                r.underline = true;
                r.underline_style = UnderlineStyle::Dotted;
            }),
        ];
        let st = StyledText::from_runs("", &runs, &Style::default());
        let m = marks(&st, false);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].run, m[1].run), (3, 4));
        // UTF-16 offsets: "plain thin é " is 13 units.
        assert_eq!((m[0].start, m[0].end), (13, 20));
        assert_eq!(m[0].color, Some(rgb(0xE8A317)));
        assert_eq!((m[1].start, m[1].end), (20, 26));
        assert_eq!(m[1].style, UnderlineStyle::Dotted);
        // A rich input draws the styles itself, not the colors.
        let m = marks(&st, true);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].color, Some(rgb(0xE8A317)));
    }

    #[test]
    fn boxes_join_by_line() {
        let s = segments([
            (0., 0., 8., 20.),
            (8., 0., 8., 20.),
            (16., 0., 8., 20.),
            (0., 20., 8., 20.),
            (8., 20., 6., 22.),
        ]);
        assert_eq!(
            s,
            vec![
                Segment {
                    x0: 0.,
                    x1: 24.,
                    top: 0.,
                    height: 20.
                },
                Segment {
                    x0: 0.,
                    x1: 14.,
                    top: 20.,
                    height: 22.
                },
            ]
        );
        // Empty boxes (a line break) draw nothing.
        assert!(segments([(5., 0., 0., 20.)]).is_empty());
        // XAML's are carets (no width): the one after the last character
        // ends the line, the next line's first starts the next.
        let carets = [0., 8., 16., 24.].map(|x| (x, 0., 0., 20.));
        let s = segments(
            carets
                .into_iter()
                .chain([(0., 20., 0., 20.), (6., 20., 0., 20.)]),
        );
        assert_eq!((s[0].x0, s[0].x1), (0., 24.));
        assert_eq!((s[1].x0, s[1].x1), (0., 6.));
    }

    #[test]
    fn styles_stroke_as_their_names_say() {
        let single = stroke(UnderlineStyle::Single, 14.);
        assert_eq!(single.thickness, 1.);
        assert_eq!(single.offsets.len(), 1);
        assert!(single.offsets[0] > 1. && single.offsets[0] < 3.);
        assert_eq!(stroke(UnderlineStyle::Thick, 14.).thickness, 2.);
        assert_eq!(stroke(UnderlineStyle::Thick, 32.).thickness, 4.);
        let double = stroke(UnderlineStyle::Double, 14.);
        assert_eq!(double.offsets.len(), 2);
        assert_eq!(double.offsets[1] - double.offsets[0], 2.);
        // Its second line goes deeper than one, still above the next line.
        assert!(double.depth() > single.depth() && double.depth() < 14. * 0.4);
        let dotted = stroke(UnderlineStyle::Dotted, 14.);
        assert!(dotted.round && dotted.dash.is_some());
        assert_eq!(stroke(UnderlineStyle::Dashed, 14.).dash, Some((4., 2.)));
        assert_eq!(snap(10.3, 1.), 10.5);
        assert_eq!(snap(10.3, 2.), 10.);
    }
}
