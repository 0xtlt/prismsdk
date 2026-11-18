//! Editable rich text (#115): `rich_input(spans)` sends its plain text and
//! its runs, rebuilds the edits backends report as spans (styles, links,
//! chips and their handlers kept), and the span editing helpers.

use std::cell::Cell;
use std::rc::Rc;

use prism_core::prelude::*;
use prism_core::{
    NodeKind, OBJECT_CHAR, Patch, UiEvent, decode_runs, normalize_spans, restyle_spans, spans_text,
    splice_spans, split_spans,
};
use prism_testing::{Harness, mount};

thread_local! {
    /// Clicks on the mention chip.
    static OPENED: Cell<usize> = const { Cell::new(0) };
}

/// A note editor: its runs, what it was given, Enter presses.
struct Note {
    spans: Vec<Span>,
    events: Vec<RichInputEvent>,
    submits: usize,
    sel: SelectionEvent,
}

impl Render for Note {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        rich_input(self.spans.clone())
            .placeholder("Write…")
            .on_input(cx.listener(|this: &mut Note, e: &RichInputEvent, _| {
                this.spans = e.spans.clone();
                this.events.push(e.clone());
            }))
            .on_submit(cx.listener(|this: &mut Note, _, _| this.submits += 1))
            .on_select(cx.listener(|this: &mut Note, e: &SelectionEvent, _| this.sel = *e))
            .selection(self.sel)
    }
}

fn note(spans: fn() -> Vec<Span>) -> (Harness, prism_core::NodeId) {
    OPENED.with(|o| o.set(0));
    let h = Harness::new(move |cx| {
        let root = cx.new_entity(|_| Note {
            spans: spans(),
            events: Vec::new(),
            submits: 0,
            sel: SelectionEvent::default(),
        });
        mount(cx, root);
    });
    let id = h.host.find(|n| n.kind == NodeKind::RichInput).unwrap();
    (h, id)
}

fn mention() -> Span {
    chip("page:42", "Roadmap")
        .chip_icon("doc.text")
        .on_click(Rc::new(|_: &ClickEvent, _: &mut App| {
            OPENED.with(|o| o.set(o.get() + 1))
        }))
}

fn last_event(h: &Harness) -> RichInputEvent {
    let n = h.root::<Note>();
    h.rt.app.read(&n).events.last().cloned().expect("an edit")
}

#[test]
fn a_rich_input_sends_its_plain_text_then_its_runs() {
    let (h, id) = note(|| vec![span("See "), span("this").bold(), span(" "), mention()]);
    let n = h.host.get(id);
    assert_eq!(
        n.text,
        format!("See this {OBJECT_CHAR}"),
        "a chip is one character"
    );
    assert_eq!(n.placeholder, "Write…");
    assert_eq!(n.a11y.label.as_deref(), Some("Write…"));
    let runs = decode_runs(&n.runs);
    assert_eq!(runs.len(), 4);
    assert_eq!(runs[1].weight, Some(FontWeight::Bold));
    let chip = &runs[3];
    assert_eq!(chip.text, OBJECT_CHAR.to_string());
    assert_eq!(chip.chip.as_deref(), Some("page:42"));
    assert_eq!(chip.chip_label.as_deref(), Some("Roadmap"));
    assert_eq!(chip.chip_icon.as_deref(), Some("doc.text"));
    assert!(chip.link.is_some(), "a clickable chip gets a link id");
}

#[test]
fn edits_reported_as_runs_keep_styles_links_and_chip_handlers() {
    let (mut h, id) = note(|| vec![span("Hi "), span("there").bold(), mention()]);
    // The web client: what it shows, each run with the fields it was sent
    // with (no link ids). The user typed "!" in the bold run.
    let shown =
        prism_core::encode_runs(&decode_runs(&h.host.get(id).runs)).replace("there", "there!");
    let shown: String = shown
        .split('\u{1f}')
        .filter(|f| !f.starts_with("l="))
        .collect::<Vec<_>>()
        .join("\u{1f}");
    let patches = h.dispatch(UiEvent::RichInput(id, shown)).to_vec();
    let e = last_event(&h);
    assert_eq!(e.value, format!("Hi there!{OBJECT_CHAR}"));
    assert_eq!(e.spans.len(), 3);
    assert!(e.spans[1].is_bold() && e.spans[1].text() == "there!");
    assert_eq!(e.spans[2].chip_key(), Some("page:42"));
    // Rendered back: the new runs reach the field, after its text.
    let text = patches
        .iter()
        .position(|p| matches!(p, Patch::SetText { .. }));
    let runs = patches
        .iter()
        .position(|p| matches!(p, Patch::SetRichValue { .. }));
    assert!(text.unwrap() < runs.unwrap());
    // The chip's handler came back with it: its new link id opens it.
    let link = decode_runs(&h.host.get(id).runs)[2].link.unwrap();
    h.dispatch(UiEvent::MenuSelect(link));
    assert_eq!(OPENED.with(Cell::get), 1);
    // Rendering the same spans again sends nothing.
    let again = h.update::<Note>(|_| {}).to_vec();
    assert!(
        !again
            .iter()
            .any(|p| matches!(p, Patch::SetRichValue { .. } | Patch::SetText { .. })),
        "{again:?}"
    );
}

#[test]
fn plain_text_edits_keep_the_styles_around_them() {
    // GTK, Android, iOS: the field edits the plain text.
    let (mut h, id) = note(|| {
        vec![
            span("Hello "),
            span("world").bold(),
            span(" "),
            link("docs", "/docs"),
        ]
    });
    // Typed at the end of the bold run: bold.
    h.dispatch(UiEvent::Input(id, "Hello world! docs".into()));
    let e = last_event(&h);
    assert_eq!(e.spans[1].text(), "world!");
    assert!(e.spans[1].is_bold());
    // Typed after the link: plain, the link unchanged.
    h.dispatch(UiEvent::Input(id, "Hello world! docs?".into()));
    let e = last_event(&h);
    assert_eq!(e.spans.len(), 5);
    assert_eq!(e.spans[3].href(), Some("/docs"));
    assert_eq!(e.spans[3].text(), "docs");
    assert_eq!(e.spans[4].text(), "?");
    assert!(!e.spans[4].is_clickable() && !e.spans[4].is_bold());
    // A selection replaced: the style of its first character.
    h.dispatch(UiEvent::Input(id, "Hello W docs?".into()));
    let e = last_event(&h);
    assert_eq!(spans_text(&e.spans), "Hello W docs?");
    assert!(e.spans[1].is_bold() && e.spans[1].text() == "W");

    // A chip deleted with Backspace is gone; a pasted object character is
    // dropped (only the app makes chips).
    let (mut h, id) = note(|| vec![span("a"), mention(), span("b")]);
    h.dispatch(UiEvent::Input(id, "ab".into()));
    let e = last_event(&h);
    assert_eq!(e.value, "ab");
    assert!(e.spans.iter().all(|s| !s.is_chip()));
    h.dispatch(UiEvent::Input(id, format!("a{OBJECT_CHAR}xb")));
    assert_eq!(last_event(&h).value, "axb");
}

#[test]
fn enter_in_a_plain_field_submits_and_takes_the_text_back() {
    let (mut h, id) = note(|| vec![span("line")]);
    let patches = h.dispatch(UiEvent::Input(id, "li\nne".into())).to_vec();
    let n = h.root::<Note>();
    assert_eq!(h.rt.app.read(&n).submits, 1);
    assert!(h.rt.app.read(&n).events.is_empty(), "not an edit");
    // The field shows a line break the app does not have: it gets the
    // app's text and runs again.
    assert!(patches.contains(&Patch::SetText {
        id,
        text: "line".into()
    }));
    assert!(
        patches
            .iter()
            .any(|p| matches!(p, Patch::SetRichValue { .. }))
    );
    // Pasting several lines is an edit.
    h.dispatch(UiEvent::Input(id, "li\nn\ne".into()));
    assert_eq!(last_event(&h).value, "li\nn\ne");
}

#[test]
fn selections_count_a_chip_as_one_character() {
    let (mut h, id) = note(|| vec![span("a😀"), mention(), span("b")]);
    assert!(h.host.get(id).select_events);
    h.dispatch(UiEvent::Select(id, 4, 3));
    let n = h.root::<Note>();
    let sel = h.rt.app.read(&n).sel;
    assert_eq!(sel.range(), 3..4);
    let value = spans_text(&h.rt.app.read(&n).spans);
    assert_eq!(&value[sel.byte_range(&value)], "b");
}

#[test]
fn spans_are_split_restyled_and_spliced_by_characters() {
    let spans = vec![
        span("Hello "),
        span("wörld").bold(),
        chip("p", "Page"),
        span("!"),
    ];
    let (a, b) = split_spans(&spans, 8);
    assert_eq!(spans_text(&a), "Hello wö");
    assert_eq!(spans_text(&b), format!("rld{OBJECT_CHAR}!"));
    assert!(a[1].is_bold() && b[0].is_bold());
    // A chip is never cut.
    let (a, b) = split_spans(&spans, 11);
    assert_eq!(spans_text(&a), "Hello wörld");
    assert!(b[0].is_chip());
    let (a, _) = split_spans(&spans, 12);
    assert!(a.last().unwrap().is_chip());

    // Bold over "lo w": the runs merge where they meet.
    let bold = restyle_spans(&spans, 3..7, Span::bold);
    assert_eq!(bold.len(), 4);
    assert_eq!(bold[0].text(), "Hel");
    assert_eq!(bold[1].text(), "lo wörld");
    assert!(bold[1].is_bold());
    let italic = restyle_spans(&spans, 0..2, Span::italic);
    assert!(italic[0].is_italic() && italic[0].text() == "He");

    // "@pa" replaced by a mention.
    let typed = vec![span("Ask @pa")];
    let done = splice_spans(&typed, 4..7, [chip("page:7", "Paris"), span(" ")]);
    assert_eq!(spans_text(&done), format!("Ask {OBJECT_CHAR} "));
    assert_eq!(done[1].chip_label(), Some("Paris"));
    assert_eq!(
        done.len(),
        3,
        "the space merged into nothing: it follows the chip"
    );

    // Normalized: empty runs dropped, equal neighbours merged, chips apart.
    let n = normalize_spans([
        span("a"),
        span(""),
        span("b"),
        chip("x", "X"),
        chip("x", "X"),
        span("c").italic(),
    ]);
    assert_eq!(n.len(), 4);
    assert_eq!(n[0].text(), "ab");
    assert!(n[1].is_chip() && n[2].is_chip());
    // with_text keeps the style; a chip keeps its one character.
    assert!(span("x").bold().with_text("y").is_bold());
    assert_eq!(chip("k", "K").with_text("long").len(), 1);
}

#[test]
fn a_read_only_rich_text_shows_a_chip_as_its_label() {
    struct Label;
    impl Render for Label {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            rich_text([span("See "), chip("page:1", "Roadmap")])
        }
    }
    let h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Label);
        mount(cx, root);
    });
    let id = h.host.find(|n| n.kind == NodeKind::Text).unwrap();
    assert_eq!(h.host.get(id).text, "See Roadmap");
    let runs = decode_runs(&h.host.get(id).config);
    assert_eq!(runs[1].text, "Roadmap");
    assert_eq!(runs[1].chip.as_deref(), Some("page:1"));
}

#[test]
fn an_icon_is_one_object_character_with_its_underline_kept() {
    // A date mention with a reminder bell (#221).
    let (mut h, id) = note(|| {
        vec![
            span("Due "),
            Span::icon("bell", 12.).text_color(rgb(0xe03131)),
            span("Friday")
                .underline_color(rgb(0xe8a317))
                .underline_style(UnderlineStyle::Thick),
        ]
    });
    let n = h.host.get(id);
    assert_eq!(
        n.text,
        format!("Due {OBJECT_CHAR}Friday"),
        "an icon is one character"
    );
    let runs = decode_runs(&n.runs);
    assert_eq!(runs.len(), 3);
    assert_eq!(runs[1].text, OBJECT_CHAR.to_string());
    assert_eq!(runs[1].icon.as_deref(), Some("bell"));
    assert!(runs[2].underline);
    assert_eq!(runs[2].underline_color, Some(rgb(0xe8a317)));
    assert_eq!(runs[2].underline_style, UnderlineStyle::Thick);
    // Typed right after the icon: the style of the run after it (an icon,
    // like a chip, never extends), the icon untouched.
    h.dispatch(UiEvent::Input(id, format!("Due {OBJECT_CHAR}xFriday")));
    let e = last_event(&h);
    assert_eq!(e.spans.len(), 3);
    assert_eq!(e.spans[1].icon_name(), Some("bell"));
    assert_eq!(e.spans[2].text(), "xFriday");
    assert_eq!(
        e.spans[2].underline_style_value(),
        UnderlineStyle::Thick,
        "the underline kept its style"
    );
    // One Backspace deletes it.
    h.dispatch(UiEvent::Input(id, "Due xFriday".into()));
    let e = last_event(&h);
    assert!(e.spans.iter().all(|s| !s.is_icon()));
    assert_eq!(e.value, "Due xFriday");
}
