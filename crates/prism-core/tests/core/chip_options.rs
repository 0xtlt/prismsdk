//! A chip's options (#329): tooltip, box (ring, radius, height, largest
//! width), styled label runs, vector icon, state, test id and attributes,
//! encoded for every backend and kept through edits.

use std::cell::Cell;
use std::rc::Rc;

use prism_core::prelude::*;
use prism_core::{ChipIcon, NodeKind, OBJECT_CHAR, TextRun, UiEvent, decode_runs, encode_runs};
use prism_testing::{Harness, mount};

const GREY: u32 = 0x8e8e93;

fn styled() -> Span {
    chip("user:7", "")
        .chip_label_runs([
            span("Ada").bold(),
            span(" · 3 tasks").text_color(rgb(GREY)).italic(),
        ])
        // A third: written with three decimals, read back a little off.
        .chip_icon(Path::circle(0.5, 0.5, 1. / 3.).fill(Color::current()))
        .tooltip("Ada Lovelace · 3 open tasks")
        .chip_ring(1.5, adaptive(rgb(0x0a84ff), rgb(0x64d2ff)))
        .chip_radius(9.)
        .chip_height(22.)
        .chip_max_width(120.)
        .chip_state("open")
        .chip_test_id("mention-ada")
        .chip_attr("data-person", "person")
}

thread_local! {
    static SHOWN: std::cell::RefCell<Vec<Span>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The runs a rich input of `spans` sends its host.
fn mounted(spans: Vec<Span>) -> Vec<TextRun> {
    SHOWN.with(|s| *s.borrow_mut() = spans);
    let h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Note {
            spans: SHOWN.with(|s| s.borrow().clone()),
        });
        mount(cx, root);
    });
    let id = h.host.find(|n| n.kind == NodeKind::RichInput).unwrap();
    decode_runs(&h.host.get(id).runs)
}

/// The run of one span, as a host receives it.
fn run(span: Span) -> TextRun {
    mounted(vec![span]).remove(0)
}

#[test]
fn every_option_is_encoded_and_read_back() {
    let runs = vec![run(styled())];
    let back = decode_runs(&encode_runs(&runs));
    assert_eq!(back.len(), 1);
    let r = &back[0];
    assert_eq!(r.text, OBJECT_CHAR.to_string());
    assert_eq!(r.chip.as_deref(), Some("user:7"));
    assert_eq!(r.chip_label.as_deref(), Some("Ada · 3 tasks"));
    assert_eq!(r.tooltip.as_deref(), Some("Ada Lovelace · 3 open tasks"));
    let (w, c) = r.chip_ring.expect("a ring");
    assert_eq!(w, 1.5);
    assert_eq!(c.light_variant().hex(), rgb(0x0a84ff).hex());
    assert_eq!(c.dark_variant().hex(), rgb(0x64d2ff).hex());
    assert_eq!(
        (r.chip_radius, r.chip_height, r.chip_max_width),
        (Some(9.), Some(22.), Some(120.))
    );
    assert_eq!(r.chip_label_runs.len(), 2);
    assert_eq!(r.chip_label_runs[0].text, "Ada");
    assert_eq!(r.chip_label_runs[0].weight, Some(FontWeight::Bold));
    assert_eq!(r.chip_label_runs[1].text, " · 3 tasks");
    assert!(r.chip_label_runs[1].italic);
    assert_eq!(
        r.chip_label_runs[1].color.map(|c| c.hex()),
        Some(rgb(GREY).hex())
    );
    let d = r.chip_drawing.as_ref().expect("a vector icon");
    assert_eq!(d.paths.len(), 1);
    assert!(d.uses_current_color(), "painted in the chip's text color");
    assert_eq!(r.chip_icon, None, "a drawing replaces the symbol");
    assert_eq!(r.chip_state.as_deref(), Some("open"));
    assert_eq!(r.chip_test_id.as_deref(), Some("mention-ada"));
    assert_eq!(
        r.chip_attrs,
        vec![("data-person".to_string(), "person".to_string())]
    );
    // Read back, it encodes the same: what editors report matches.
    assert_eq!(encode_runs(&back), encode_runs(&runs));
}

/// A chip without options encodes as before (#115): the fields are only
/// there when set.
#[test]
fn a_plain_chip_encodes_as_before() {
    let plain = chip("page:42", "Roadmap").chip_icon("doc.text");
    assert_eq!(
        encode_runs(&[run(plain)]),
        "\u{fffc}\u{1f}k=page:42\u{1f}n=Roadmap\u{1f}m=doc.text"
    );
}

#[test]
fn separators_in_texts_are_dropped() {
    let c = chip("k", "")
        .chip_label_runs([span("a\u{1c}b\u{1d}c")])
        .tooltip("t\u{1d}ip")
        .chip_attr("data-x", "1\u{1c}2");
    let r = &decode_runs(&encode_runs(&[run(c.clone())]))[0];
    assert_eq!(r.chip_label_runs[0].text, "abc");
    assert_eq!(r.tooltip.as_deref(), Some("tip"));
    assert_eq!(r.chip_attrs, vec![("data-x".into(), "12".into())]);
}

#[test]
fn an_svg_icon_keeps_its_proportions() {
    let icon = ChipIcon::svg(
        r#"<svg viewBox="0 0 32 16"><rect width="32" height="16" fill="currentColor"/></svg>"#,
    );
    let ChipIcon::Vector { drawing, ratio } = &icon else {
        panic!("a vector icon: {icon:?}");
    };
    assert_eq!(*ratio, 2.);
    assert_eq!(drawing.paths.len(), 1);
    let c = chip("k", "Wide").chip_icon(icon.clone());
    let r = &decode_runs(&encode_runs(&[run(c.clone())]))[0];
    assert_eq!(r.chip_icon_ratio, Some(2.));
    assert_eq!(c.chip_icon_value(), Some(icon));
    // A symbol again replaces it.
    let c = c.chip_icon("star");
    assert_eq!(c.chip_icon_value(), Some(ChipIcon::Symbol("star".into())));
    assert_eq!(run(c).chip_drawing, None);
}

#[test]
fn a_state_is_one_word_and_attributes_are_data_names() {
    let c = chip("k", "x").chip_state("is open!");
    assert_eq!(c.chip_state_value(), Some("isopen"));
    assert_eq!(chip("k", "x").chip_state("  ").chip_state_value(), None);
    let c = chip("k", "x")
        .chip_attr("Data-Order", "1")
        .chip_attr("data-order", "2");
    assert_eq!(run(c).chip_attrs, vec![("data-order".into(), "2".into())]);
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "not an attribute a chip may set")]
fn a_chip_refuses_other_attributes() {
    let _ = chip("k", "x").chip_attr("onclick", "alert(1)");
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "not an attribute a chip may set")]
fn a_chip_leaves_its_state_attribute_to_chip_state() {
    let _ = chip("k", "x").chip_attr("data-state", "open");
}

thread_local! {
    static OPENED: Cell<usize> = const { Cell::new(0) };
}

struct Note {
    spans: Vec<Span>,
}

impl Render for Note {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        rich_input(self.spans.clone()).on_input(
            cx.listener(|this: &mut Note, e: &RichInputEvent, _| this.spans = e.spans.clone()),
        )
    }
}

/// An editor reports the chip with the fields it was sent (the web
/// client): its drawing and colors read back from the encoding, it is
/// still the app's span, its handler included.
#[test]
fn a_styled_chip_keeps_its_handler_through_edits() {
    let h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Note {
            spans: vec![
                span("Hi "),
                styled().on_click(Rc::new(|_: &ClickEvent, _: &mut App| {
                    OPENED.with(|o| o.set(o.get() + 1))
                })),
            ],
        });
        mount(cx, root);
    });
    let mut h = h;
    let id = h.host.find(|n| n.kind == NodeKind::RichInput).unwrap();
    let shown = encode_runs(&decode_runs(&h.host.get(id).runs)).replace("Hi ", "Hi! ");
    let shown: String = shown
        .split('\u{1f}')
        .filter(|f| !f.starts_with("l="))
        .collect::<Vec<_>>()
        .join("\u{1f}");
    h.dispatch(UiEvent::RichInput(id, shown));
    let runs = decode_runs(&h.host.get(id).runs);
    assert_eq!(runs[0].text, "Hi! ");
    assert_eq!(runs[1].chip_test_id.as_deref(), Some("mention-ada"));
    let link = runs[1].link.expect("still clickable");
    h.dispatch(UiEvent::MenuSelect(link));
    assert_eq!(OPENED.with(Cell::get), 1);
}

/// Tests find a chip by its test id in the runs the host received.
#[test]
fn prism_testing_finds_a_chip_by_its_test_id() {
    let h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Note {
            spans: vec![span("To "), styled(), span(" and "), chip("user:8", "Alan")],
        });
        mount(cx, root);
    });
    let id = h.host.find(|n| n.kind == NodeKind::RichInput).unwrap();
    let (index, run) = h
        .host
        .chip(id, "mention-ada")
        .expect("found by its test id");
    assert_eq!(index, 3, "its character in the value");
    assert_eq!(run.chip_state.as_deref(), Some("open"));
    assert!(h.host.chip(id, "nobody").is_none());
}
