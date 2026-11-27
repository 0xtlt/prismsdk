//! Pictures from memory (`picture_data`) and SVG markup (`svg`).

use prism_core::prelude::*;
use prism_core::surfaces::snapshot_one;
use prism_core::{Drawing, Length, NodeKind};

#[test]
fn picture_data_is_a_base64_data_url() {
    let mut app = App::new();
    let node = snapshot_one(&mut app, picture_data(b"Man", "image/png"));
    assert_eq!(node.kind, NodeKind::Picture);
    assert_eq!(node.text, "data:image/png;base64,TWFu");
    for (bytes, b64) in [
        (&b"Ma"[..], "TWE="),
        (b"M", "TQ=="),
        (b"", ""),
        (&[0xff, 0xfe, 0xfd], "//79"),
    ] {
        let node = snapshot_one(&mut app, picture_data(bytes, "image/gif"));
        assert_eq!(node.text, format!("data:image/gif;base64,{b64}"));
    }
}

#[test]
fn svg_is_a_vector_shape_with_its_own_size_and_proportions() {
    let markup = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" viewBox="0 0 20 10">
        <path d="M0 0H20V10H0Z" fill="#336699"/>
        <circle cx="5" cy="5" r="2" fill="white"/>
    </svg>"##;
    let mut app = App::new();
    let node = snapshot_one(&mut app, svg(markup).alt("Logo"));
    assert_eq!(node.kind, NodeKind::Shape);
    assert_eq!(node.label.as_deref(), Some("Logo"));
    assert!(!node.hidden);
    // 40 wide; the height follows the 2:1 viewBox (filled in for widgets).
    assert_eq!(node.style.width, Length::Px(40.));
    assert_eq!(node.style.height, Length::Px(20.));
    assert_eq!(node.style.aspect_ratio, Some(2.));
    let d = Drawing::decode(&node.text);
    assert_eq!(d.paths.len(), 2);
    assert_eq!(d.paths[0].fill, Some(rgb(0x336699)));
    // An SVG given to picture_data takes the same path.
    let same = snapshot_one(&mut app, picture_data(markup.as_bytes(), "image/svg+xml"));
    assert_eq!(same.kind, NodeKind::Shape);
    assert_eq!(same.text, node.text);
    // Unreadable markup: an empty drawing, no panic.
    assert_eq!(snapshot_one(&mut app, svg("not svg")).text, "i0,0\n");
}

#[test]
fn even_odd_paths_survive_the_text_form() {
    let p = Path::rect(0., 0., 1., 1.).fill(rgb(0)).even_odd();
    let d = Drawing {
        inset: (0., 0.),
        paths: vec![p.clone(), Path::rect(0., 0., 1., 1.).fill(rgb(0))],
        ..Default::default()
    };
    let back = Drawing::decode(&d.encode());
    assert!(back.paths[0].even_odd);
    assert!(!back.paths[1].even_odd);
}

#[test]
fn headers_travel_with_a_remote_picture() {
    let mut app = App::new();
    let node = snapshot_one(
        &mut app,
        picture("https://api.example/me.png")
            .header("Authorization", "Bearer abc")
            .header("X-Evil\r\n", "a\nb"),
    );
    assert_eq!(
        node.text,
        "https://api.example/me.png\nAuthorization: Bearer abc\nX-Evil: ab"
    );
    let (url, headers) = prism_core::picture_source(&node.text);
    assert_eq!(url, "https://api.example/me.png");
    assert_eq!(
        headers,
        vec![("Authorization", "Bearer abc"), ("X-Evil", "ab")]
    );
    // Assets, data: URLs and SVG ignore headers.
    let asset = snapshot_one(&mut app, picture("logo.png").header("A", "b"));
    assert_eq!(asset.text, "logo.png");
    assert_eq!(
        prism_core::picture_source("logo.png"),
        ("logo.png", Vec::new())
    );
}

/// S6 (security review): line breaks in a plain URL (content an app shows
/// as is) never become headers; only `header` adds them, as tokens.
#[test]
fn a_plain_url_never_carries_headers() {
    let mut app = App::new();
    let url = "https://evil.example/x.png\nAuthorization: Bearer stolen\r\nX-A:\tb";
    let node = snapshot_one(&mut app, picture(url));
    assert_eq!(prism_core::picture_source(&node.text).1, Vec::new());
    assert!(!node.text.contains(['\n', '\r', '\t']), "{:?}", node.text);
    let node = snapshot_one(&mut app, video(url));
    assert_eq!(prism_core::picture_source(&node.text).1, Vec::new());
    // A header name is a token: no separator, space or colon.
    let node = snapshot_one(
        &mut app,
        picture("https://api.example/me.png")
            .header("X-A: b", "c")
            .header("Bad Name", "c")
            .header("X-Ok", "v"),
    );
    assert_eq!(
        prism_core::picture_source(&node.text).1,
        vec![("X-Ok", "v")]
    );
}
