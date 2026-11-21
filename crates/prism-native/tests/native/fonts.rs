//! Fonts loaded at run time (#81): aliases, URLs loaded in the background,
//! texts drawn again.

use std::time::{Duration, Instant};

use prism_core::{
    Context, FontFace, FontFamily, FontSource, IntoElement, ParentElement, PlatformRequest, Render,
    Styled, div, text,
};
use prism_native::fonts::{prepare, resolve, resolve_name, save};
use prism_native::images::ImageSources;

use crate::common::{Call, Harness};

/// A minimal TrueType font: `name` (one family record) and `OS/2` tables.
fn tiny_font(family: &str) -> Vec<u8> {
    let utf16: Vec<u8> = family.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut name = vec![];
    for v in [0u16, 1, 18, 3, 1, 0x409, 1, utf16.len() as u16, 0] {
        name.extend(v.to_be_bytes());
    }
    name.extend(utf16);
    let mut os2 = vec![0u8; 78];
    os2[4..6].copy_from_slice(&400u16.to_be_bytes());
    let mut font = vec![0, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0];
    let mut offset = 12 + 2 * 16;
    for (tag, data) in [(b"OS/2", &os2), (b"name", &name)] {
        font.extend(tag);
        font.extend([0u8; 4]);
        font.extend((offset as u32).to_be_bytes());
        font.extend((data.len() as u32).to_be_bytes());
        offset += data.len();
    }
    font.extend(&os2);
    font.extend(&name);
    font
}

struct Label;
impl Render for Label {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().child(text("Aa").font_family("Uploaded 7"))
    }
}

#[test]
fn the_app_s_name_becomes_an_alias_of_the_file_s_family() {
    let mut h = Harness::root(|| Label);
    let bytes = tiny_font("Tiny Sans Seven");
    let ready = prepare(
        &mut h.rt.app,
        FontFace::bytes("Uploaded 7", bytes.clone()),
        &ImageSources::default(),
    )
    .expect("a TrueType file");
    assert_eq!(ready.family, "Tiny Sans Seven");
    assert_eq!(&ready.bytes[..], &bytes[..]);
    assert_eq!(resolve_name("uploaded 7"), "Tiny Sans Seven");
    assert_eq!(resolve_name("Inter"), "Inter", "other names are left alone");
    assert_eq!(
        resolve(&FontFamily::Named("Uploaded 7".into())),
        FontFamily::Named("Tiny Sans Seven".into())
    );
    assert_eq!(resolve(&FontFamily::Monospace), FontFamily::Monospace);
    // Remote hosts (Android, watchOS) get the file's family.
    let style = prism_core::Style {
        font_family: "Uploaded 7".into(),
        ..Default::default()
    };
    assert_eq!(
        prism_native::remote::typography(&style)["ff"],
        "Tiny Sans Seven"
    );
    // Saved once, by content, for the platforms that take a file.
    let dir = crate::temp_dir::TempDir::new("native-fonts");
    let path = save(&ready, &dir).unwrap();
    assert_eq!(path.extension().unwrap(), "ttf");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(save(&ready, &dir).unwrap(), path);
    // WOFF2 is a web format.
    assert!(
        prepare(
            &mut h.rt.app,
            FontFace::bytes("Web only", &b"wOF2\0\0\0\0"[..]),
            &ImageSources::default(),
        )
        .is_none()
    );
}

#[test]
fn a_url_comes_back_as_its_bytes() {
    let mut h = Harness::root(|| Label);
    h.rt.take_requests();
    // A `data:` URL loads like a picture, without the network.
    let face = FontFace::url("From url", "data:font/ttf;base64,AAEAAA==").weight(500);
    assert!(prepare(&mut h.rt.app, face, &ImageSources::default()).is_none());
    let deadline = Instant::now() + Duration::from_secs(5);
    let font = loop {
        h.rt.poll();
        let found = h.rt.take_requests().into_iter().find_map(|r| match r {
            PlatformRequest::LoadFont(f) => Some(f),
            _ => None,
        });
        if let Some(f) = found {
            break f;
        }
        assert!(Instant::now() < deadline, "the font never came back");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(
        (font.family.as_str(), font.weight),
        ("From url", Some((500, 500)))
    );
    assert_eq!(font.source, FontSource::Bytes(vec![0, 1, 0, 0].into()));
}

#[test]
fn restyling_hands_every_node_its_style_again() {
    let mut h = Harness::root(|| Label);
    h.layout(200., 100.);
    h.r.widgets.calls.clear();
    h.r.restyle_all();
    let styled = h
        .w()
        .calls
        .iter()
        .filter(|c| matches!(c, Call::Style(..)))
        .count();
    assert_eq!(styled, 2, "the div and its text");
    // Measured again at the next layout.
    h.r.widgets.measured.clear();
    h.layout(200., 100.);
    assert!(!h.w().measured.is_empty());
}
