//! Typography CSS (#9), rich-text runs and clickable texts in server-rendered
//! HTML (#10), bundled fonts as `@font-face`.

use prism_core::{
    Accessibility, Backend, Color, FontFamily, FontWeight, Length, NodeKind, Patch, Role, Style,
    TextRun, TextTransform, encode_runs, rgb,
};

use crate::css::{css, run_css};
use crate::html::{HtmlBackend, runs_html};

#[test]
fn typography_becomes_css_only_when_set() {
    let plain = css(NodeKind::Text, &Style::default());
    for prop in [
        "font-family",
        "font-style",
        "line-height",
        "letter-spacing",
        "text-transform",
        "text-decoration",
        "font-variant-numeric",
        "line-clamp",
        "text-overflow",
    ] {
        assert!(!plain.contains(prop), "{prop} in {plain}");
    }
    let s = Style {
        font_family: FontFamily::Monospace,
        font_weight: FontWeight::Custom(650),
        italic: true,
        line_height: Length::Fraction(1.25),
        letter_spacing: 1.6,
        text_transform: TextTransform::Uppercase,
        underline: true,
        strikethrough: true,
        tabular_numbers: true,
        ..Style::default()
    };
    let c = css(NodeKind::Text, &s);
    for part in [
        "font-weight:650;",
        "font-family:ui-monospace,",
        "font-style:italic;",
        "line-height:1.25;",
        "letter-spacing:0.1rem;",
        "text-transform:uppercase;",
        "text-decoration-line:underline line-through;",
        "font-variant-numeric:tabular-nums;",
    ] {
        assert!(c.contains(part), "{part} missing in {c}");
    }
    let px = css(
        NodeKind::Text,
        &Style {
            line_height: Length::Px(24.),
            ..Style::default()
        },
    );
    assert!(px.contains("line-height:1.5rem;"), "{px}");
    // Families also apply to buttons and inputs (after their `inherit`).
    let b = css(
        NodeKind::Button,
        &Style {
            font_family: FontFamily::Named("Inter".into()),
            ..Style::default()
        },
    );
    assert!(
        b.rfind("font-family:\"Inter\"") > b.rfind("font-family:inherit"),
        "{b}"
    );
}

#[test]
fn truncation_css() {
    let one = css(
        NodeKind::Text,
        &Style {
            max_lines: Some(1),
            ..Style::default()
        },
    );
    assert!(one.ends_with("white-space:pre;overflow:hidden;text-overflow:ellipsis;"));
    let three = css(
        NodeKind::Text,
        &Style {
            max_lines: Some(3),
            ..Style::default()
        },
    );
    assert!(three.contains("display:-webkit-box;"));
    assert!(three.contains("-webkit-line-clamp:3;line-clamp:3;overflow:hidden;"));
    // The clamp's display comes after the text's own `display:block`.
    assert!(three.rfind("display:-webkit-box") > three.rfind("display:block"));
    let nowrap = css(
        NodeKind::Text,
        &Style {
            no_wrap: true,
            ..Style::default()
        },
    );
    assert!(nowrap.ends_with("white-space:pre;overflow:hidden;"));
}

fn runs() -> String {
    encode_runs(&[
        TextRun {
            text: "Signed in as ".into(),
            ..TextRun::default()
        },
        TextRun {
            text: "Ada <3".into(),
            weight: Some(FontWeight::Bold),
            color: Some(Color::adaptive(rgb(0x000000), rgb(0xffffff))),
            size: Some(1.5),
            ..TextRun::default()
        },
        TextRun {
            text: "settings".into(),
            underline: true,
            link: Some(42),
            href: Some("/settings?a=1&b=2".into()),
            ..TextRun::default()
        },
        TextRun {
            text: "★".into(),
            icon: Some("star.fill".into()),
            ..TextRun::default()
        },
        TextRun {
            text: "tap".into(),
            link: Some(43),
            family: Some(FontFamily::Monospace),
            ..TextRun::default()
        },
    ])
}

#[test]
fn runs_become_inline_spans_and_links() {
    let r = &prism_core::decode_runs(&runs())[1];
    assert_eq!(
        run_css(r),
        "font-weight:700;color:light-dark(rgba(0,0,0,1), rgba(255,255,255,1));font-size:1.5em;"
    );
    assert_eq!(
        runs_html(&runs()),
        concat!(
            "Signed in as ",
            r#"<span style="font-weight:700;color:light-dark(rgba(0,0,0,1), rgba(255,255,255,1));font-size:1.5em;">Ada &lt;3</span>"#,
            r#"<a class="prism-link" href="/settings?a=1&amp;b=2" data-link="42" style="text-decoration-line:underline;">settings</a>"#,
            r#"<span class="prism-symbol" aria-hidden="true">★</span>"#,
            r#"<span class="prism-link" role="link" tabindex="0" data-link="43" style="font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,&quot;Liberation Mono&quot;,monospace;">tap</span>"#,
        )
    );
}

#[test]
fn ssr_rich_text_and_clickable_text() {
    let mut b = HtmlBackend::default();
    b.apply(&[
        Patch::Create {
            id: 1,
            kind: NodeKind::View,
        },
        Patch::Create {
            id: 2,
            kind: NodeKind::Text,
        },
        Patch::SetText {
            id: 2,
            text: "Signed in as Ada <3settings★tap".into(),
        },
        Patch::SetConfig {
            id: 2,
            config: runs(),
        },
        Patch::Create {
            id: 3,
            kind: NodeKind::Text,
        },
        Patch::SetText {
            id: 3,
            text: "Tap me".into(),
        },
        Patch::SetClickable {
            id: 3,
            clickable: true,
        },
        Patch::SetAccessibility {
            id: 3,
            a11y: Accessibility {
                role: Some(Role::Button),
                ..Accessibility::default()
            },
        },
        Patch::SetChildren {
            id: 1,
            children: vec![2, 3],
        },
        Patch::SetRoot { id: 1 },
    ]);
    let html = b.to_html();
    assert!(html.contains(&format!(
        r#"data-kind="text" style="">{}</div>"#,
        runs_html(&runs())
    )));
    assert!(
        html.contains(r#"data-kind="text" style="" role="button" tabindex="0">Tap me</div>"#),
        "{html}"
    );
}

#[test]
fn bundled_fonts_are_font_faces_of_the_base_stylesheet() {
    let dir = std::env::temp_dir().join(format!("prism-web-fonts-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("fonts")).unwrap();
    std::fs::write(dir.join("fonts/Brand-BoldItalic.woff2"), b"wOF2").unwrap();
    std::fs::write(dir.join("logo.png"), b"png").unwrap();
    let assets = crate::assets::Assets::new("js".into(), "body{}".into(), Some(dir.clone()));
    let css_path = assets.css().to_string();
    let css = String::from_utf8(assets.get(&css_path).expect("the stylesheet").body).unwrap();
    assert!(css.starts_with("body{}"));
    let hashed = assets.app_path("fonts/Brand-BoldItalic.woff2");
    assert!(
        css.contains(&format!(
            "@font-face{{font-family:\"Brand\";src:url(\"../{hashed}\") format(\"woff2\");font-weight:700;font-style:italic;font-display:swap}}"
        )),
        "{css}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
