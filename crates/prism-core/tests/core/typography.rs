//! Typography styles (#9), rich text runs and clickable texts (#10).

use prism_core::prelude::*;
use prism_core::{
    DeviceCall, Length, NodeKind, OBJECT_CHAR, PlatformRequest, Style, TextRun, UiEvent,
    UnderlineStyle, decode_runs, encode_runs,
};
use prism_testing::{Harness, mount};

#[test]
fn weights_cover_the_css_scale() {
    assert_eq!(FontWeight::Thin.css(), 100);
    assert_eq!(FontWeight::Black.css(), 900);
    assert_eq!(FontWeight::Custom(650).css(), 650);
    assert_eq!(FontWeight::Custom(5000).css(), 900);
    assert_eq!(FontWeight::from_css(700), FontWeight::Bold);
    assert_eq!(FontWeight::from_css(650), FontWeight::Custom(650));
    assert_eq!(FontWeight::Custom(650).nearest_hundred(), 700);
    assert_eq!(FontWeight::Custom(640).nearest_hundred(), 600);
    assert_eq!(div().weight(300).style_mut().font_weight, FontWeight::Light);
}

#[test]
fn builders_set_the_typography_fields() {
    let mut t = text("x")
        .font_mono()
        .italic()
        .leading(1.5)
        .tracking(0.5)
        .uppercase()
        .underline()
        .line_through()
        .tabular_nums()
        .line_clamp(3);
    let s = t.style_mut().clone();
    assert_eq!(s.font_family, FontFamily::Monospace);
    assert!(s.italic && s.underline && s.strikethrough && s.tabular_numbers);
    assert_eq!(s.line_height, Length::Fraction(1.5));
    assert_eq!(s.letter_spacing, 0.5);
    assert_eq!(s.text_transform, TextTransform::Uppercase);
    assert_eq!(s.max_lines, Some(3));
    assert!(s.truncates());
    assert_eq!(text("x").truncate().style_mut().max_lines, Some(1));
    assert_eq!(text("x").line_clamp(0).style_mut().max_lines, None);
    assert!(text("x").nowrap().style_mut().no_wrap);
    assert_eq!(
        text("x").font_family("Inter").style_mut().font_family,
        FontFamily::Named("Inter".into())
    );
    // Line height in points or as a multiple of the font size.
    let s = Style {
        font_size: 20.,
        line_height: Length::Fraction(1.5),
        ..Style::default()
    };
    assert_eq!(s.line_height_points(), Some(30.));
    let s = Style {
        line_height: Length::Px(18.),
        letter_spacing: 1.,
        ..Style::default()
    };
    let z = s.scaled(2.);
    assert_eq!(z.line_height, Length::Px(36.));
    assert_eq!(z.letter_spacing, 2.);
}

#[test]
fn families_and_transforms() {
    for f in [
        FontFamily::System,
        FontFamily::Monospace,
        FontFamily::Serif,
        FontFamily::Rounded,
        FontFamily::Named("Inter Display".into()),
    ] {
        assert_eq!(FontFamily::decode(f.encode()), f);
    }
    assert!(
        FontFamily::Named("A\"B".into())
            .css()
            .starts_with("\"AB\",")
    );
    assert_eq!(TextTransform::Uppercase.apply("straße"), "STRASSE");
    assert_eq!(TextTransform::Lowercase.apply("ÉTÉ"), "été");
    assert_eq!(
        TextTransform::Capitalize.apply("hello wide-world, it's l'été"),
        "Hello Wide-World, It's L'été"
    );
    // A run continues the word the previous one ended in.
    let c = TextTransform::Capitalize;
    assert_eq!(c.apply_after("Hel", "lo world"), "lo World");
    assert_eq!(c.apply_after("it", "'s"), "'s");
    assert_eq!(c.apply_after("one ", "two"), "Two");
    assert_eq!(TextTransform::Uppercase.apply_after("x", "ab"), "AB");
    assert_eq!(
        text("x").capitalize().style_mut().text_transform,
        TextTransform::Capitalize
    );
}

#[test]
fn runs_round_trip_through_their_encoding() {
    let runs = vec![
        TextRun {
            text: "a\u{1e}b".into(),
            weight: Some(FontWeight::Custom(650)),
            italic: true,
            underline: true,
            underline_color: Some(adaptive(rgb(0xe8a317), rgb(0xffd43b))),
            underline_style: UnderlineStyle::Dotted,
            strikethrough: true,
            color: Some(rgb(0x112233)),
            background: Some(adaptive(rgb(0xffffff), rgb(0x000000))),
            family: Some(FontFamily::Named("Fira Code".into())),
            size: Some(1.25),
            link: Some(7 << 16 | 0x8000),
            href: Some("https://example.com/a b".into()),
            chip: Some("page:1".into()),
            chip_label: Some("Road map".into()),
            chip_icon: Some("doc.text".into()),
            icon: None,
            // A chip's options (#329).
            chip_drawing: Some(prism_core::Drawing {
                inset: (0., 0.),
                paths: vec![Path::circle(0.5, 0.5, 0.25).fill(rgb(0x334455))],
                ..Default::default()
            }),
            chip_icon_ratio: Some(1.5),
            tooltip: Some("Road map, the whole name".into()),
            chip_ring: Some((1.5, adaptive(rgb(0x0a84ff), rgb(0x64d2ff)))),
            chip_radius: Some(8.),
            chip_height: Some(22.5),
            chip_max_width: Some(140.),
            chip_label_runs: vec![
                TextRun {
                    text: "Road".into(),
                    weight: Some(FontWeight::Bold),
                    ..TextRun::default()
                },
                TextRun {
                    text: " map".into(),
                    italic: true,
                    color: Some(rgb(0x8e8e93)),
                    ..TextRun::default()
                },
            ],
            chip_state: Some("open".into()),
            chip_test_id: Some("roadmap".into()),
            chip_attrs: vec![("data-order".into(), "3".into())],
        },
        TextRun {
            text: "★".into(),
            icon: Some("star.fill".into()),
            ..TextRun::default()
        },
        TextRun::default(),
    ];
    let decoded = decode_runs(&encode_runs(&runs));
    assert_eq!(decoded.len(), 3);
    assert_eq!(decoded[0].text, "ab", "separators are dropped from texts");
    assert_eq!(
        TextRun {
            text: "a\u{1e}b".into(),
            ..decoded[0].clone()
        },
        runs[0]
    );
    assert_eq!(decoded[1], runs[1]);
    assert_eq!(decoded[2], runs[2]);
    assert!(decode_runs("").is_empty());
}

/// A rich text with a bold run, a link, an icon and a clickable run, and a
/// clickable plain text.
#[derive(Default)]
struct Profile {
    taps: u32,
    text_taps: u32,
}

impl Render for Profile {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(
                rich_text([
                    span("Signed in as "),
                    span("Ada").bold().text_size(20.),
                    span(" · "),
                    link("settings", "/settings"),
                    icon("star.fill"),
                    span(format!(" {}", self.taps))
                        .on_click(cx.listener(|this, _, _| this.taps += 1)),
                    link("docs", "https://example.com"),
                ])
                .text_size(10.),
            )
            .child(
                text(format!("tapped {}", self.text_taps))
                    .on_click(cx.listener(|this, _, _| this.text_taps += 1)),
            )
            .child(text("Home").link("/"))
    }
}

fn profile() -> Harness {
    Harness::new(|cx| {
        let root = cx.new_entity(|_| Profile::default());
        mount(cx, root);
    })
}

#[test]
fn rich_text_is_one_text_node_with_runs() {
    let h = profile();
    let id = h.host.by_text("Signed in as Ada · settings★ 0docs");
    let node = h.host.get(id);
    assert_eq!(node.kind, NodeKind::Text);
    let runs = decode_runs(&node.config);
    assert_eq!(runs.len(), 7);
    assert_eq!(runs[1].weight, Some(FontWeight::Bold));
    // Sizes are relative to the paragraph's (they follow Dynamic Type).
    assert_eq!(runs[1].size, Some(2.));
    assert!(runs[3].underline, "links are underlined by default");
    assert_eq!(runs[3].href.as_deref(), Some("/settings"));
    assert!(runs[3].link.is_some());
    assert!(runs[5].link.is_some() && runs[5].href.is_none());
    assert_eq!(runs[4].icon.as_deref(), Some("star.fill"));
    assert_eq!(
        runs[4].text, "★",
        "read-only, an icon is its fallback glyph"
    );
    assert!(runs[0].link.is_none());
    // Icons have no label: the paragraph is read without them.
    assert_eq!(
        node.a11y.label.as_deref(),
        Some("Signed in as Ada · settings 0docs")
    );
    assert_eq!(node.a11y.role, Some(Role::Text));
    assert!(!node.clickable);
}

#[test]
fn clickable_runs_report_their_id_as_a_menu_selection() {
    let mut h = profile();
    let id = h.host.by_text("Signed in as Ada · settings★ 0docs");
    let runs = decode_runs(&h.host.get(id).config);
    h.dispatch(UiEvent::MenuSelect(runs[5].link.unwrap()));
    let id = h.host.by_text("Signed in as Ada · settings★ 1docs");
    // The same ids after the re-render.
    let again = decode_runs(&h.host.get(id).config);
    assert_eq!(again[5].link, runs[5].link);
    // External links open the URL.
    h.dispatch(UiEvent::MenuSelect(runs[6].link.unwrap()));
    let opened = h.rt.take_requests().into_iter().any(|r| {
        matches!(r, PlatformRequest::Device { call: DeviceCall::OpenUrl(u), .. } if u == "https://example.com")
    });
    assert!(opened, "an external link opens its URL");
}

#[test]
fn texts_can_be_clicked_and_be_links() {
    let mut h = profile();
    let id = h.host.by_text("tapped 0");
    assert!(h.host.get(id).clickable);
    assert_eq!(h.host.get(id).a11y.role, Some(Role::Button));
    h.click("tapped 0");
    h.host.by_text("tapped 1");
    let home = h.host.by_text("Home");
    assert!(h.host.get(home).clickable);
    assert_eq!(h.host.get(home).a11y.role, Some(Role::Link));
}

#[test]
fn spans_have_their_own_underline_color_and_style() {
    let s = span("passage")
        .underline_color(rgb(0xe8a317))
        .underline_style(UnderlineStyle::Dashed);
    assert!(s.is_underlined(), "a color or a style underlines");
    assert_eq!(s.underline_color_value(), Some(rgb(0xe8a317)));
    assert_eq!(s.underline_style_value(), UnderlineStyle::Dashed);
    assert!(!s.clone().no_underline().is_underlined());
    // Every style survives the encoding; single is the default, unwritten.
    for style in [
        UnderlineStyle::Single,
        UnderlineStyle::Thick,
        UnderlineStyle::Double,
        UnderlineStyle::Dotted,
        UnderlineStyle::Dashed,
    ] {
        let run = TextRun {
            text: "x".into(),
            underline: true,
            underline_style: style,
            ..TextRun::default()
        };
        assert_eq!(decode_runs(&encode_runs(std::slice::from_ref(&run))), [run]);
        assert_eq!(UnderlineStyle::from_name(style.name()), style);
    }
    assert!(
        !encode_runs(&[TextRun {
            underline: true,
            ..TextRun::default()
        }])
        .contains("us=")
    );
    // Two runs differing only by their underline are two runs.
    let a = span("a").underline();
    assert!(!a.same_style(&span("b").underline_style(UnderlineStyle::Double)));
    assert!(a.same_style(&span("b").underline()));
}

#[test]
fn a_sized_icon_reads_as_its_glyph_in_a_read_only_text() {
    let s = Span::icon("bell", 12.);
    assert!(s.is_icon() && s.is_object());
    assert_eq!(s.len(), 1);
    assert_eq!(s.text(), OBJECT_CHAR.to_string());
}
