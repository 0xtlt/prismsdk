//! Styled text for native backends: what an attributed string (AppKit,
//! UIKit), a Pango attribute list (GTK) or a spannable (Android) must hold
//! for a text node's style and rich-text runs. Backends map a
//! [`StyledText`] to their own attributes, so the case transform, run
//! merging and link ids are the same everywhere, and measuring the native
//! label measures exactly what is drawn.

use prism_core::{
    Color, FontFamily, FontWeight, Style, TextAlign, TextRun, TextWrap, UnderlineStyle, decode_runs,
};

/// The font of one run.
#[derive(Clone, Debug, PartialEq)]
pub struct FontSpec {
    /// Family.
    pub family: FontFamily,
    /// Size, points (already scaled).
    pub size: f32,
    /// Weight.
    pub weight: FontWeight,
    /// Italic.
    pub italic: bool,
    /// Digits of equal width.
    pub tabular: bool,
}

/// One run of styled text.
#[derive(Clone, Debug, PartialEq)]
pub struct StyledRun {
    /// The text as drawn (case transform applied).
    pub text: String,
    /// Where the run starts in the whole text, in UTF-16 code units
    /// (`NSRange`, Android spans).
    pub utf16_start: usize,
    /// Where the run starts in the whole text, in UTF-8 bytes (Pango).
    pub byte_start: usize,
    /// Its font.
    pub font: FontSpec,
    /// Text color (`None`: the platform's label color).
    pub color: Option<Color>,
    /// Background color.
    pub background: Option<Color>,
    /// Underlined.
    pub underline: bool,
    /// The underline's color (`None`: the text's, #221).
    pub underline_color: Option<Color>,
    /// How the underline is drawn (#221).
    pub underline_style: UnderlineStyle,
    /// Struck through.
    pub strikethrough: bool,
    /// Clickable run: report `UiEvent::MenuSelect(link)`.
    pub link: Option<u64>,
    /// The link's path or URL.
    pub href: Option<String>,
    /// An inline symbol (SF Symbol name); `text` is its fallback glyph
    /// (U+FFFC in a rich input).
    pub icon: Option<String>,
}

impl StyledRun {
    /// Length in UTF-16 code units.
    pub fn utf16_len(&self) -> usize {
        self.text.encode_utf16().count()
    }
}

/// A text node's content as styled runs plus paragraph settings.
#[derive(Clone, Debug, PartialEq)]
pub struct StyledText {
    /// The runs, in order (at least one).
    pub runs: Vec<StyledRun>,
    /// Alignment (already mirrored right to left).
    pub align: TextAlign,
    /// Line height, points.
    pub line_height: Option<f32>,
    /// Extra space after each character, points.
    pub letter_spacing: f32,
    /// At most this many lines, ellipsized.
    pub max_lines: Option<u32>,
    /// One line per paragraph, clipped.
    pub no_wrap: bool,
    /// How wrapping lines are broken (#95).
    pub wrap: TextWrap,
}

impl StyledText {
    /// `text` (a plain node) or `config` (encoded runs, see
    /// `prism_core::spans`) styled with the node's (effective) `style`.
    pub fn new(text: &str, config: &str, style: &Style) -> StyledText {
        StyledText::from_runs(text, &decode_runs(config), style)
    }

    /// Like [`new`](Self::new) with decoded runs (empty: `text` is one run).
    pub fn from_runs(text: &str, runs: &[TextRun], style: &Style) -> StyledText {
        let base = FontSpec {
            family: style.font_family.clone(),
            size: style.font_size,
            weight: style.font_weight,
            italic: style.italic,
            tabular: style.tabular_numbers,
        };
        let plain;
        let runs = if runs.is_empty() {
            plain = [TextRun {
                text: text.to_string(),
                ..TextRun::default()
            }];
            &plain[..]
        } else {
            runs
        };
        let (mut utf16, mut bytes) = (0, 0);
        // The text before each run, so a word split across runs is
        // capitalized once (an icon ends a word).
        let mut before = "";
        let runs = runs
            .iter()
            .map(|r| {
                let text = match &r.icon {
                    Some(_) => {
                        before = "";
                        r.text.clone()
                    }
                    None => {
                        let t = style.text_transform.apply_after(before, &r.text);
                        before = &r.text;
                        t
                    }
                };
                let run = StyledRun {
                    utf16_start: utf16,
                    byte_start: bytes,
                    font: FontSpec {
                        family: r.family.clone().unwrap_or_else(|| base.family.clone()),
                        size: r
                            .size
                            .map_or(base.size, |z| (z * base.size).round().max(1.)),
                        weight: r.weight.unwrap_or(base.weight),
                        italic: base.italic || r.italic,
                        tabular: base.tabular,
                    },
                    color: r.color.or(style.text_color),
                    background: r.background,
                    underline: style.underline || r.underline,
                    underline_color: r.underline_color,
                    underline_style: r.underline_style,
                    strikethrough: style.strikethrough || r.strikethrough,
                    link: r.link,
                    href: r.href.clone(),
                    icon: r.icon.clone(),
                    text,
                };
                utf16 += run.utf16_len();
                bytes += run.text.len();
                run
            })
            .collect();
        StyledText {
            runs,
            align: style.text_align,
            line_height: style.line_height_points(),
            letter_spacing: style.letter_spacing,
            max_lines: style.max_lines,
            no_wrap: style.no_wrap,
            wrap: style.text_wrap,
        }
    }

    /// The whole text as drawn.
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    /// Whether one plain run with no paragraph settings: a label's own font
    /// and color draw it (no attributed string needed).
    pub fn is_simple(&self) -> bool {
        let [r] = &self.runs[..] else { return false };
        r.font.family == FontFamily::System
            && !r.font.italic
            && !r.font.tabular
            && r.background.is_none()
            && !r.underline
            && !r.strikethrough
            && r.link.is_none()
            && r.icon.is_none()
            && self.line_height.is_none()
            && self.letter_spacing == 0.
    }

    /// The clickable runs: `(text, link id)`, in order (accessibility
    /// actions, keyboard focus).
    pub fn links(&self) -> Vec<(String, u64)> {
        self.runs
            .iter()
            .filter_map(|r| r.link.map(|l| (r.text.clone(), l)))
            .collect()
    }

    /// The link at UTF-16 offset `index`, if a clickable run covers it.
    pub fn link_at_utf16(&self, index: usize) -> Option<u64> {
        self.runs
            .iter()
            .find(|r| index >= r.utf16_start && index < r.utf16_start + r.utf16_len())
            .and_then(|r| r.link)
    }

    /// The link at UTF-8 byte offset `index`, if a clickable run covers it.
    pub fn link_at_byte(&self, index: usize) -> Option<u64> {
        self.runs
            .iter()
            .find(|r| index >= r.byte_start && index < r.byte_start + r.text.len())
            .and_then(|r| r.link)
    }

    /// Whether any run is clickable.
    pub fn has_links(&self) -> bool {
        self.runs.iter().any(|r| r.link.is_some())
    }
}

/// The `NSUnderlineStyle` of an underline (#221), for AppKit and UIKit:
/// single, thick or double; dotted and dashed are single lines with a
/// pattern.
pub fn ns_underline_style(style: UnderlineStyle) -> isize {
    // NSUnderlineStyleSingle 0x01, Thick 0x02, Double 0x09; the patterns
    // Dot 0x0100 and Dash 0x0200.
    match style {
        UnderlineStyle::Single => 0x01,
        UnderlineStyle::Thick => 0x02,
        UnderlineStyle::Double => 0x09,
        UnderlineStyle::Dotted => 0x0101,
        UnderlineStyle::Dashed => 0x0201,
    }
}

/// `w` between the platform's fixed weights `steps` (the values at 100,
/// 200 … 900), interpolated for the weights in between (650).
pub fn interpolate_weight(w: FontWeight, steps: [f64; 9]) -> f64 {
    let w = w.css() as f64;
    let i = ((w - 100.) / 100.).floor().clamp(0., 7.) as usize;
    let t = (w - 100. - i as f64 * 100.) / 100.;
    steps[i] + (steps[i + 1] - steps[i]) * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::{Styled, TextTransform, encode_runs, rgb};

    #[test]
    fn a_run_keeps_its_underline_color_and_style() {
        let runs = [TextRun {
            text: "x".into(),
            underline: true,
            underline_color: Some(rgb(0xe8a317)),
            underline_style: UnderlineStyle::Dashed,
            ..Default::default()
        }];
        let t = StyledText::from_runs("", &runs, &Style::default());
        assert!(t.runs[0].underline);
        assert_eq!(t.runs[0].underline_color, Some(rgb(0xe8a317)));
        assert_eq!(t.runs[0].underline_style, UnderlineStyle::Dashed);
        assert!(!t.is_simple());
        assert_eq!(ns_underline_style(UnderlineStyle::Single), 1);
        assert_eq!(ns_underline_style(UnderlineStyle::Dotted), 0x0101);
    }

    #[test]
    fn plain_text_is_one_run_with_the_node_style() {
        let style = Style::default().text_size(20.).italic().uppercase();
        let t = StyledText::new("héllo", "", &style);
        assert_eq!(t.runs.len(), 1);
        assert_eq!(t.text(), "HÉLLO");
        assert_eq!(t.runs[0].font.size, 20.);
        assert!(t.runs[0].font.italic);
        assert!(!t.is_simple());
        assert!(StyledText::new("a", "", &Style::default()).is_simple());
    }

    #[test]
    fn the_paragraph_keeps_its_line_breaking() {
        use prism_core::{Styled, TextWrap};
        let plain = StyledText::new("a b c", "", &Style::default());
        assert_eq!(plain.wrap, TextWrap::Auto);
        let t = StyledText::new("a b c", "", &Style::default().text_balance());
        assert_eq!(t.wrap, TextWrap::Balance);
        // Labels set it themselves: a plain text stays on the fast path.
        assert!(t.is_simple());
    }

    #[test]
    fn runs_inherit_and_override_the_paragraph() {
        let runs = vec![
            TextRun {
                text: "App ".into(),
                ..Default::default()
            },
            TextRun {
                text: "Home".into(),
                weight: Some(FontWeight::Bold),
                color: Some(rgb(0xff0000)),
                size: Some(1.5),
                ..Default::default()
            },
            TextRun {
                text: "😀 go".into(),
                link: Some(7),
                underline: true,
                ..Default::default()
            },
        ];
        let style = Style::default()
            .text_size(10.)
            .text_color(rgb(0x00ff00))
            .text_transform(TextTransform::Lowercase);
        let t = StyledText::new("", &encode_runs(&runs), &style);
        assert_eq!(t.text(), "app home😀 go");
        assert_eq!(t.runs[1].font.weight, FontWeight::Bold);
        assert_eq!(t.runs[1].font.size, 15.);
        assert_eq!(t.runs[0].color, Some(rgb(0x00ff00)));
        assert_eq!(t.runs[1].color, Some(rgb(0xff0000)));
        // UTF-16: the emoji is two code units; UTF-8 four bytes.
        assert_eq!(t.runs[2].utf16_start, 8);
        assert_eq!(t.runs[2].byte_start, 8);
        assert_eq!(t.link_at_utf16(8), Some(7));
        assert_eq!(t.link_at_utf16(12), Some(7));
        assert_eq!(t.link_at_utf16(13), None);
        assert_eq!(t.link_at_utf16(3), None);
        assert_eq!(t.link_at_byte(8 + 4 + 2), Some(7));
        assert_eq!(t.links(), vec![("😀 go".to_string(), 7)]);
    }

    #[test]
    fn capitalize_spans_run_boundaries() {
        let run = |text: &str| TextRun {
            text: text.into(),
            ..Default::default()
        };
        let runs = vec![
            run("hel"),
            run("lo wor"),
            run("ld, it"),
            run("'s "),
            TextRun {
                icon: Some("star".into()),
                ..run("*")
            },
            run("ok"),
        ];
        let style = Style::default().capitalize();
        let t = StyledText::new("", &encode_runs(&runs), &style);
        assert_eq!(t.text(), "Hello World, It's *Ok");
        let t = StyledText::new("l'été à paris", "", &style);
        assert_eq!(t.text(), "L'été À Paris");
    }

    #[test]
    fn weights_interpolate_between_platform_steps() {
        let steps = [-0.8, -0.6, -0.4, 0., 0.23, 0.3, 0.4, 0.56, 0.62];
        assert_eq!(interpolate_weight(FontWeight::Regular, steps), 0.);
        assert_eq!(interpolate_weight(FontWeight::Black, steps), 0.62);
        assert_eq!(interpolate_weight(FontWeight::Thin, steps), -0.8);
        let w = interpolate_weight(FontWeight::Custom(650), steps);
        assert!((w - 0.35).abs() < 1e-9, "{w}");
    }
}
