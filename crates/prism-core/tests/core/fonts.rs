//! Fonts and stylesheets added while the app runs (#81).

use std::sync::Arc;

use prism_core::fonts::{FontFileInfo, font_file_info, font_format};
use prism_core::prelude::*;
use prism_core::{AppInit, FontSource, PlatformRequest, Runtime};

struct Screen;

impl Render for Screen {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        cx.load_font(FontFace::url("Lora", "https://fonts.example/lora.woff2").weights(700, 400));
        cx.load_font(FontFace::bytes("Brand", &b"OTTO"[..]).italic());
        text("Lora").font_family("Lora")
    }
}

fn runtime() -> Runtime {
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Screen);
        cx.set_root(root);
    });
    Runtime::new(&init)
}

fn fonts(requests: Vec<PlatformRequest>) -> Vec<FontFace> {
    requests
        .into_iter()
        .filter_map(|r| match r {
            PlatformRequest::LoadFont(f) => Some(f),
            _ => None,
        })
        .collect()
}

#[test]
fn a_face_is_asked_for_once() {
    let mut rt = runtime();
    rt.render();
    let first = fonts(rt.take_requests());
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].family, "Lora");
    assert_eq!(
        first[0].source,
        FontSource::Url("https://fonts.example/lora.woff2".into())
    );
    assert_eq!(first[0].weight, Some((400, 700)), "a range is ordered");
    assert_eq!(first[0].css_weight().as_deref(), Some("400 700"));
    assert!(first[1].italic);
    assert_eq!(first[1].css_weight(), None);
    // Rendering again asks for nothing new.
    rt.render();
    assert!(fonts(rt.take_requests()).is_empty());
    // The same face with its downloaded bytes is another request.
    rt.app.load_font(first[0].with_bytes(&b"\0\x01\0\0"[..]));
    let again = fonts(rt.take_requests());
    assert_eq!(again.len(), 1);
    assert!(matches!(&again[0].source, FontSource::Bytes(b) if b.len() == 4));
    assert_eq!(
        FontFace::bytes("X", &b"ab"[..]).weight(2000).weight,
        Some((1000, 1000))
    );
}

#[test]
fn stylesheets_live_as_long_as_their_handles() {
    let mut rt = runtime();
    rt.render();
    rt.take_requests();
    let a = rt.app.add_stylesheet("/a.css");
    let a2 = rt.app.add_stylesheet("/a.css");
    let b = rt.app.add_stylesheet("/b.css");
    assert_eq!(b.url(), "/b.css");
    assert_eq!(rt.app.stylesheets(), ["/a.css", "/b.css"]);
    let sheets = |rt: &mut Runtime| {
        rt.take_requests()
            .into_iter()
            .filter(|r| {
                matches!(
                    r,
                    PlatformRequest::AddStylesheet(_) | PlatformRequest::RemoveStylesheet(_)
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        sheets(&mut rt),
        [
            PlatformRequest::AddStylesheet("/a.css".into()),
            PlatformRequest::AddStylesheet("/b.css".into()),
        ]
    );
    drop(a);
    assert!(sheets(&mut rt).is_empty(), "another handle keeps /a.css");
    drop(a2);
    drop(b);
    assert_eq!(
        sheets(&mut rt),
        [
            PlatformRequest::RemoveStylesheet("/a.css".into()),
            PlatformRequest::RemoveStylesheet("/b.css".into()),
        ]
    );
    assert!(rt.app.stylesheets().is_empty());
}

/// A minimal OpenType font: `name` (one family record) and `OS/2` tables,
/// plus `fvar` with a weight axis when `variable`.
fn tiny_font(family: &str, weight: u16, italic: bool, variable: bool) -> Vec<u8> {
    let utf16: Vec<u8> = family.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut name = vec![];
    for v in [0u16, 1, 18, 3, 1, 0x409, 16, utf16.len() as u16, 0] {
        name.extend(v.to_be_bytes());
    }
    name.extend(utf16);
    let mut os2 = vec![0u8; 78];
    os2[4..6].copy_from_slice(&weight.to_be_bytes());
    os2[62..64].copy_from_slice(&u16::from(italic).to_be_bytes());
    // fvar: header (16 bytes, axes at 16, one axis of 20 bytes), `wght`.
    let mut fvar = vec![0u8; 36];
    fvar[4..6].copy_from_slice(&16u16.to_be_bytes());
    fvar[8..10].copy_from_slice(&1u16.to_be_bytes());
    fvar[10..12].copy_from_slice(&20u16.to_be_bytes());
    fvar[16..20].copy_from_slice(b"wght");
    let mut tables: Vec<(&[u8; 4], &Vec<u8>)> = vec![(b"OS/2", &os2), (b"name", &name)];
    if variable {
        tables.push((b"fvar", &fvar));
    }
    let mut font = vec![0, 1, 0, 0];
    font.extend((tables.len() as u16).to_be_bytes());
    font.extend([0u8; 6]);
    let mut offset = 12 + tables.len() * 16;
    for (tag, data) in &tables {
        font.extend(*tag);
        font.extend([0u8; 4]);
        font.extend((offset as u32).to_be_bytes());
        font.extend((data.len() as u32).to_be_bytes());
        offset += data.len();
    }
    for (_, data) in &tables {
        font.extend(data.iter());
    }
    font
}

#[test]
fn font_files_describe_themselves() {
    assert_eq!(
        font_file_info(&tiny_font("Lora Display", 650, true, false)),
        Some(FontFileInfo {
            family: "Lora Display".into(),
            weight: 650,
            variable: false,
            italic: true,
        })
    );
    let variable = font_file_info(&tiny_font("Inter", 400, false, true)).unwrap();
    assert!(variable.variable);
    // A collection: its first font, whose tables are found from the file's start.
    let member = tiny_font("Members", 700, false, false);
    let mut ttc = b"ttcf\0\x01\0\0\0\0\0\x01\0\0\0\x10".to_vec();
    let shift = ttc.len() as u32;
    let mut shifted = member.clone();
    for i in 0..2 {
        let at = 12 + i * 16 + 8;
        let off = u32::from_be_bytes(member[at..at + 4].try_into().unwrap()) + shift;
        shifted[at..at + 4].copy_from_slice(&off.to_be_bytes());
    }
    ttc.extend(shifted);
    assert_eq!(font_file_info(&ttc).unwrap().family, "Members");
    assert_eq!(font_format(&ttc), Some("collection"));
    assert_eq!(font_file_info(b"wOF2...."), None);
    assert_eq!(font_format(b"wOF2...."), Some("woff2"));
    assert_eq!(font_format(b"OTTO"), Some("opentype"));
    assert_eq!(font_format(b"<svg"), None);
}
