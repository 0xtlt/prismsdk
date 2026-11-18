//! A rich input's editing hooks (#217): the selection's rectangle, copy,
//! cut and paste handled by the app, modifier-clicks on spans.

use prism_core::prelude::*;
use prism_core::{
    ClipboardContent, CopyEvent, NodeKind, PasteEvent, Patch, SelectionRectEvent, SpanClickEvent,
    UiEvent, edit_events,
};
use prism_testing::{Harness, mount};

/// A document block: what its hooks saw.
#[derive(Default)]
struct Block {
    spans: Vec<Span>,
    rects: Vec<SelectionRectEvent>,
    copies: Vec<(String, usize, bool)>,
    pastes: Vec<PasteEvent>,
    take_pastes: bool,
    clicks: Vec<SpanClickEvent>,
    hooks: bool,
}

impl Render for Block {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let field = rich_input(self.spans.clone()).id("block").on_input(
            cx.listener(|this: &mut Block, e: &RichInputEvent, _| this.spans = e.spans.clone()),
        );
        if !self.hooks {
            return field;
        }
        field
            .on_selection_rect(
                cx.listener(|this: &mut Block, e: &SelectionRectEvent, _| this.rects.push(*e)),
            )
            .on_copy(cx.listener(|this: &mut Block, e: &CopyEvent, _| {
                this.copies.push((e.text.clone(), e.spans.len(), e.cut));
                let bold: String = e
                    .spans
                    .iter()
                    .map(|s| {
                        if s.is_bold() {
                            format!("**{}**", s.text())
                        } else {
                            s.text().to_string()
                        }
                    })
                    .collect();
                e.write(ClipboardContent::text(&e.text).markdown(bold));
            }))
            .on_cut(cx.listener(|this: &mut Block, e: &CopyEvent, _| {
                this.copies.push((e.text.clone(), e.spans.len(), e.cut))
            }))
            .on_paste(cx.listener(|this: &mut Block, e: &PasteEvent, _| {
                this.pastes.push(e.clone());
                if this.take_pastes {
                    e.prevent_default();
                }
            }))
            .on_span_click(
                cx.listener(|this: &mut Block, e: &SpanClickEvent, _| this.clicks.push(*e)),
            )
    }
}

fn block(hooks: bool) -> (Harness, prism_core::NodeId) {
    let h = Harness::new(move |cx| {
        let root = cx.new_entity(|_| Block {
            spans: vec![span("Hello "), span("bold").bold(), span(" world")],
            hooks,
            ..Default::default()
        });
        mount(cx, root);
    });
    let id = h.host.find(|n| n.kind == NodeKind::RichInput).unwrap();
    (h, id)
}

#[test]
fn a_rich_input_reports_only_the_hooks_it_has() {
    let (h, id) = block(false);
    assert_eq!(h.host.get(id).edit_events, 0);
    assert!(!h.host.get(id).reports_layout);
    let (h, id) = block(true);
    let n = h.host.get(id);
    assert_eq!(
        n.edit_events,
        edit_events::SELECTION_RECT
            | edit_events::COPY
            | edit_events::CUT
            | edit_events::PASTE
            | edit_events::SPAN_CLICK
    );
    // Its frame gives the rectangle in the window too.
    assert!(n.reports_layout);
}

#[test]
fn the_selection_rectangle_is_given_in_the_field_and_in_the_window() {
    let (mut h, id) = block(true);
    let rect = Rect::new(40., 2., 30., 18.);
    h.dispatch(UiEvent::SelectionRect(id, 6, 10, rect));
    let b = h.root::<Block>();
    let e = h.rt.app.read(&b).rects[0];
    assert_eq!(e.selection.range(), 6..10);
    assert_eq!(e.rect, rect);
    assert_eq!(e.frame, None, "not laid out yet");

    h.dispatch(UiEvent::Layout(id, Rect::new(100., 200., 300., 24.)));
    h.dispatch(UiEvent::SelectionRect(
        id,
        6,
        6,
        Rect::new(40., 2., 1., 18.),
    ));
    let e = *h.rt.app.read(&b).rects.last().unwrap();
    assert!(e.selection.is_caret());
    assert_eq!(e.frame, Some(Rect::new(140., 202., 1., 18.)));
    assert_eq!(h.rt.app.selection_rect("block"), Some(e));
    assert_eq!(h.rt.app.selection_rect("other"), None);
}

#[test]
fn a_copy_writes_what_the_app_gives_or_the_plain_text() {
    let (mut h, id) = block(true);
    // "lo bold w": three runs, the middle one bold.
    let patches = h
        .dispatch(UiEvent::Copy(id, false, 3, 12, "lo bold w".into()))
        .to_vec();
    let b = h.root::<Block>();
    assert_eq!(
        h.rt.app.read(&b).copies,
        [("lo bold w".to_string(), 3, false)]
    );
    let content = ClipboardContent::text("lo bold w").markdown("lo **bold** w");
    assert!(patches.contains(&Patch::WriteClipboard {
        id,
        content: content.clone()
    }));
    assert_eq!(h.host.clipboard, [(id, content)]);

    // The cut handler writes nothing: the plain text is copied.
    h.dispatch(UiEvent::Copy(id, true, 0, 5, "Hello".into()));
    assert_eq!(h.rt.app.read(&b).copies[1], ("Hello".to_string(), 1, true));
    assert_eq!(h.host.clipboard[1], (id, ClipboardContent::text("Hello")));
}

#[test]
fn a_paste_is_inserted_unless_the_app_takes_it() {
    let (mut h, id) = block(true);
    let paste = PasteEvent::new(SelectionEvent::caret(5), "one\ntwo")
        .with_html(Some("<p>one</p><p>two</p>".into()));
    h.dispatch(UiEvent::Paste(id, paste));
    let b = h.root::<Block>();
    let seen = h.rt.app.read(&b).pastes[0].clone();
    assert_eq!(seen.text, "one\ntwo");
    assert!(seen.has("text/html"));
    assert_eq!(seen.get("text/html"), Some("<p>one</p><p>two</p>"));
    assert_eq!(h.host.inserted, [(id, "one\ntwo".to_string())]);

    h.rt.app.update(&b, |this, _| this.take_pastes = true);
    h.dispatch(UiEvent::Paste(
        id,
        PasteEvent::new(SelectionEvent::caret(0), "x"),
    ));
    assert_eq!(h.rt.app.read(&b).pastes.len(), 2);
    assert_eq!(h.host.inserted.len(), 1, "taken by the app");
}

#[test]
fn a_modifier_click_names_the_span_under_it() {
    let (mut h, id) = block(true);
    let cmd = Modifiers {
        meta: true,
        ..Modifiers::NONE
    };
    h.dispatch(UiEvent::SpanClick(id, 7, cmd));
    h.dispatch(UiEvent::SpanClick(id, 6, cmd));
    h.dispatch(UiEvent::SpanClick(id, 5, cmd));
    h.dispatch(UiEvent::SpanClick(id, 99, cmd));
    let b = h.root::<Block>();
    let spans: Vec<usize> = h.rt.app.read(&b).clicks.iter().map(|c| c.span).collect();
    assert_eq!(spans, [1, 1, 0, 2], "past the end: the last span");
    assert!(h.rt.app.read(&b).clicks[0].modifiers.meta);
}

#[test]
fn clipboard_contents_list_their_types() {
    let c = ClipboardContent::text("a")
        .html("<b>a</b>")
        .custom("application/x-blocks", "[1]")
        .custom("text/markdown", "**a**")
        .custom("application/x-blocks", "[2]");
    assert_eq!(
        c.types(),
        [
            "text/plain",
            "text/html",
            "text/markdown",
            "application/x-blocks"
        ]
    );
    assert_eq!(c.get("application/x-blocks"), Some("[2]"));
    assert_eq!(c.markdown_text(), Some("**a**"));
    assert_eq!(
        c.to_json(),
        r#"[["text/plain","a"],["text/html","<b>a</b>"],["text/markdown","**a**"],["application/x-blocks","[2]"]]"#
    );
}

#[test]
fn a_copied_chip_is_its_label() {
    let spans = [span("See "), prism_core::chip("page:1", "Roadmap")];
    let e = CopyEvent::new(
        SelectionEvent::new(0, 5),
        format!("See {}", prism_core::OBJECT_CHAR),
        &spans,
        false,
    );
    assert_eq!(e.text, "See Roadmap");
    assert_eq!(e.spans.len(), 2);
}
