//! A popover anchored to a chip of a rich input (#327): the runtime gives
//! backends the chip's character in its field (again when the text moves
//! it), the field once the chip is gone, and a chip's click carries its
//! box.

use prism_core::controls::OverlayConfig;
use prism_core::prelude::*;
use prism_core::{NodeKind, UiEvent, decode_runs};
use prism_testing::{Harness, mount};

/// A note with two chips; a click on one opens a menu under it.
struct Note {
    spans: Vec<Span>,
    menu: Option<String>,
    clicked: Option<ClickEvent>,
    field: &'static str,
    dialog: bool,
}

impl Render for Note {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let spans = self.spans.iter().cloned().map(|s| match s.chip_key() {
            Some(key) => {
                let key = key.to_string();
                s.on_click(cx.listener(move |this: &mut Note, e: &ClickEvent, _| {
                    this.menu = Some(key.clone());
                    this.clicked = Some(e.clone());
                }))
            }
            None => s,
        });
        let menu = self.menu.clone().map(|key| {
            if self.dialog {
                dialog(text("Remove"))
                    .open(true)
                    .anchor_chip(self.field, key)
            } else {
                popover(text("Remove"))
                    .open(true)
                    .side(PopoverSide::Bottom)
                    .anchor_chip(self.field, key)
            }
        });
        // The menu comes before the field (keyed, so it stays the same
        // node): its anchor is found once the field is mounted.
        div()
            .children(menu)
            .child(rich_input(spans).id("note").key("note"))
    }
}

fn note() -> Harness {
    Harness::new(|cx| {
        let root = cx.new_entity(|_| Note {
            spans: vec![
                span("Hello "),
                chip("name", "Name"),
                span(" and "),
                chip("date", "Date"),
            ],
            menu: None,
            clicked: None,
            field: "note",
            dialog: false,
        });
        mount(cx, root);
    })
}

fn overlay(h: &Harness) -> OverlayConfig {
    let id = h.host.find(|n| n.kind == NodeKind::Overlay).unwrap();
    OverlayConfig::decode(&h.host.get(id).config)
}

fn field(h: &Harness) -> u64 {
    h.host.find(|n| n.kind == NodeKind::RichInput).unwrap()
}

/// The link id of the chip `n` (0: the first) of the field.
fn chip_link(h: &Harness, n: usize) -> u64 {
    decode_runs(&h.host.get(field(h)).runs)
        .into_iter()
        .filter(|r| r.chip.is_some())
        .nth(n)
        .and_then(|r| r.link)
        .unwrap()
}

#[test]
fn a_chip_click_carries_its_box_in_the_field_and_the_window() {
    let mut h = note();
    let f = field(&h);
    // Clickable chips make the field report its frame.
    assert!(h.host.get(f).reports_layout);
    h.dispatch(UiEvent::Layout(f, Rect::new(20., 100., 300., 40.)));
    let date = chip_link(&h, 1);
    h.dispatch(UiEvent::RunClick(date, Rect::new(90., 4., 40., 18.)));
    let e = h.rt.app.read(&h.root::<Note>()).clicked.clone().unwrap();
    assert_eq!(e.span_rect, Some(Rect::new(90., 4., 40., 18.)));
    assert_eq!(e.span_frame, Some(Rect::new(110., 104., 40., 18.)));
    assert_eq!(
        h.rt.app.read(&h.root::<Note>()).menu.as_deref(),
        Some("date")
    );
    // From the keyboard (or a backend without boxes): no box.
    h.dispatch(UiEvent::MenuSelect(chip_link(&h, 0)));
    let e = h.rt.app.read(&h.root::<Note>()).clicked.clone().unwrap();
    assert_eq!((e.span_rect, e.span_frame), (None, None));
    // A box that is not one is dropped.
    h.dispatch(UiEvent::RunClick(date, Rect::new(f32::NAN, 0., 1., 1.)));
    let e = h.rt.app.read(&h.root::<Note>()).clicked.clone().unwrap();
    assert_eq!((e.span_rect, e.span_frame), (None, None));
}

#[test]
fn a_popover_anchored_to_a_chip_follows_it_then_falls_back_to_the_field() {
    let mut h = note();
    let f = field(&h);
    h.dispatch(UiEvent::MenuSelect(chip_link(&h, 1)));
    // "Hello " (6), the name chip (1), " and " (5): the date chip is 12.
    let c = overlay(&h);
    assert_eq!(
        c.anchor,
        OverlayAnchor::Text {
            field: f,
            start: 12,
            end: 13
        }
    );
    assert_eq!(c.anchored_side(), Some(PopoverSide::Bottom));
    // Text typed before it moves it.
    h.update(|n: &mut Note| n.spans.insert(0, span(">> ")));
    assert_eq!(
        overlay(&h).anchor,
        OverlayAnchor::Text {
            field: f,
            start: 15,
            end: 16
        }
    );
    // An unrelated render sends nothing new.
    let patches = h.update(|n: &mut Note| n.dialog = false).to_vec();
    assert!(
        !patches
            .iter()
            .any(|p| matches!(p, prism_core::Patch::SetConfig { .. }))
    );
    // The chip deleted: anchored to the field, still open.
    h.update(|n: &mut Note| n.spans.retain(|s| s.chip_key() != Some("date")));
    assert_eq!(overlay(&h).anchor, OverlayAnchor::Field(f));
    // No field of that name: its parent, as usual.
    h.update(|n: &mut Note| n.field = "missing");
    assert_eq!(overlay(&h).anchor, OverlayAnchor::Parent);
}

#[test]
fn a_dialog_anchored_to_a_chip_is_placed_beside_it() {
    let mut h = note();
    h.update(|n: &mut Note| n.dialog = true);
    h.dispatch(UiEvent::MenuSelect(chip_link(&h, 0)));
    let c = overlay(&h);
    assert!(c.placed);
    assert_eq!(c.anchored_side(), Some(PopoverSide::Bottom));
    assert_eq!(
        c.anchor,
        OverlayAnchor::Text {
            field: field(&h),
            start: 6,
            end: 7
        }
    );
    // Closed and opened again: the anchor comes back with it.
    h.update(|n: &mut Note| n.menu = None);
    assert!(h.host.find(|n| n.kind == NodeKind::Overlay).is_none());
    h.update(|n: &mut Note| n.menu = Some("name".into()));
    assert!(overlay(&h).anchor.field().is_some());
}

#[test]
fn another_anchor_replaces_the_chip() {
    let o = popover(text("x"))
        .anchor_chip("note", "page:1")
        .anchor(OverlayAnchor::Caret)
        .into_element();
    let prism_core::Element::Node(n) = o else {
        panic!("an overlay is a node")
    };
    assert_eq!(n.chip_anchor, None);
    assert_eq!(n.config, "popover anchor=caret");
    let o = popover(text("x"))
        .anchor_chip("note", "page:1")
        .into_element();
    let prism_core::Element::Node(n) = o else {
        panic!("an overlay is a node")
    };
    assert_eq!(n.chip_anchor, Some(("note".into(), "page:1".into())));
}
