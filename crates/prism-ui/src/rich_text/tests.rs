//! The rich text model, its Markdown and HTML, and the editor driven like
//! a backend would (#294).

use prism_core::prelude::*;
use prism_core::{KeyEvent, Modifiers, PasteEvent, Patch, UiEvent};
use prism_testing::{Harness, mount};

use super::*;

fn plain(text: &str) -> Span {
    span(text)
}

fn texts(doc: &RichText) -> Vec<(BlockKind, String)> {
    doc.blocks.iter().map(|b| (b.kind, b.text())).collect()
}

// --- Markdown ---------------------------------------------------------------------

#[test]
fn markdown_round_trips_the_model() {
    let doc = RichText::new([
        RichBlock::heading(2, [plain("Plan")]),
        RichBlock::paragraph([
            plain("Ship "),
            plain("today").bold(),
            plain(", "),
            plain("maybe").italic(),
            plain(" "),
            plain("both").bold().italic(),
            plain(" — see "),
            plain("the doc").link_to("https://example.com/a_b"),
            plain("."),
        ]),
        RichBlock::bullet([plain("tests")]),
        RichBlock::bullet([plain("docs")]),
        RichBlock::numbered([plain("one")]),
        RichBlock::numbered([plain("two")]),
        RichBlock::heading(3, [plain("Notes")]),
        RichBlock::paragraph([plain("2 * 3 = 6, a_b_c, [x], `y`, <z>")]),
    ]);
    let md = doc.to_markdown();
    assert_eq!(
        md,
        "## Plan\n\nShip **today**, *maybe* ***both*** — see [the doc](https://example.com/a_b).\n\n\
         - tests\n- docs\n\n1. one\n2. two\n\n### Notes\n\n\
         2 \\* 3 = 6, a\\_b\\_c, \\[x\\], \\`y\\`, \\<z>"
    );
    assert_eq!(RichText::from_markdown(&md), doc);
}

#[test]
fn markdown_escapes_what_would_start_a_block() {
    for text in [
        "# not a heading",
        "- not a list",
        "1. not a list",
        "> no quote",
        "+ plus",
    ] {
        let doc = RichText::new([RichBlock::paragraph([plain(text)])]);
        let md = doc.to_markdown();
        assert_ne!(md, text);
        assert_eq!(
            texts(&RichText::from_markdown(&md)),
            [(BlockKind::Paragraph, text.into())]
        );
    }
}

#[test]
fn markdown_marks_never_touch_whitespace() {
    let doc = RichText::new([RichBlock::paragraph([
        plain("a"),
        plain(" bold ").bold(),
        plain("b"),
        plain("it").italic(),
        plain("c"),
    ])]);
    let md = doc.to_markdown();
    assert_eq!(md, "a **bold** b*it*c");
    let back = RichText::from_markdown(&md);
    assert_eq!(back.text(), doc.text());
    assert_eq!(back.to_markdown(), md);
}

#[test]
fn markdown_reads_common_syntax() {
    let doc = RichText::from_markdown(
        "Title\n=====\n\nSub\n---\n\n#### Deep ##\n\nOne\nparagraph  \nbroken\n\n\
         * star\n+ plus\n  lazy line\n3) three\n\n> quoted\n\n***\n\n__bold__ _it_ **_both_**",
    );
    assert_eq!(
        texts(&doc),
        [
            (BlockKind::Heading(1), "Title".into()),
            (BlockKind::Heading(2), "Sub".into()),
            (BlockKind::Heading(3), "Deep".into()),
            (BlockKind::Paragraph, "One paragraph".into()),
            (BlockKind::Paragraph, "broken".into()),
            (BlockKind::Bullet, "star".into()),
            (BlockKind::Bullet, "plus lazy line".into()),
            (BlockKind::Numbered, "three".into()),
            (BlockKind::Paragraph, "quoted".into()),
            (BlockKind::Paragraph, "bold it both".into()),
        ]
    );
    let last = &doc.blocks[9].spans;
    assert!(last[0].is_bold() && !last[0].is_italic());
    assert!(last[2].is_italic() && !last[2].is_bold());
    assert!(last[4].is_bold() && last[4].is_italic());
}

#[test]
fn markdown_keeps_unmatched_stars_and_unsafe_links_as_text() {
    let doc = RichText::from_markdown(
        "2 * 3 * 4, *a, **b\n\n[x](javascript:alert(1)) [y](<https://e.com/a b>) <https://auto.link> [r][ref]",
    );
    assert_eq!(doc.blocks[0].text(), "2 * 3 * 4, *a, **b");
    assert!(
        doc.blocks[0]
            .spans
            .iter()
            .all(|s| !s.is_bold() && !s.is_italic())
    );
    let links: Vec<(String, Option<&str>)> = doc.blocks[1]
        .spans
        .iter()
        .map(|s| (s.text().to_string(), s.href()))
        .collect();
    assert_eq!(
        links,
        [
            ("x ".into(), None),
            ("y".into(), Some("https://e.com/a b")),
            (" ".into(), None),
            ("https://auto.link".into(), Some("https://auto.link")),
            (" [r][ref]".into(), None),
        ]
    );
}

// --- HTML -------------------------------------------------------------------------

#[test]
fn html_round_trips_the_model() {
    let doc = RichText::new([
        RichBlock::heading(1, [plain("A & B")]),
        RichBlock::paragraph([
            plain("x "),
            plain("<b>").bold(),
            plain(" y").link_to("/p?a=1&b=2"),
        ]),
        RichBlock::bullet([plain("one")]),
        RichBlock::numbered([plain("two").italic()]),
    ]);
    let html = doc.to_html();
    assert_eq!(
        html,
        "<h1>A &amp; B</h1><p>x <strong>&lt;b&gt;</strong><a href=\"/p?a=1&amp;b=2\"> y</a></p>\
         <ul><li>one</li></ul><ol><li><em>two</em></li></ol>"
    );
    assert_eq!(RichText::from_html(&html), doc);
}

#[test]
fn html_pastes_keep_the_models_subset_only() {
    // A word processor's paste: a bold wrapper set back to normal, styles
    // on spans, nested lists, a script, entities, an unsafe link.
    let html = r#"<html><head><style>p{}</style></head><body>
        <b style="font-weight:normal" id="docs-internal-guid-1">
          <h2>Heading&nbsp;two</h2>
          <p>plain <span style="font-weight:700">heavy</span> <span style="font-style:italic">slanted</span>
             <a href="https://ok.example">ok</a> <a href=" javascript:alert(1)">bad</a></p>
          <ul><li>first<ol><li>inner</li></ol></li><li>second</li></ul>
          <script>alert("no")</script>
          <p>line<br>break &amp; &#x263A; &#9731; &unknown;</p>
          <div><img src="x.png" alt="pic">end</div>
        </b></body></html>"#;
    let doc = RichText::from_html(html);
    assert_eq!(
        texts(&doc),
        [
            (BlockKind::Heading(2), "Heading two".into()),
            (BlockKind::Paragraph, "plain heavy slanted ok bad".into()),
            (BlockKind::Bullet, "first".into()),
            (BlockKind::Numbered, "inner".into()),
            (BlockKind::Bullet, "second".into()),
            (BlockKind::Paragraph, "line".into()),
            (BlockKind::Paragraph, "break & ☺ ☃ &unknown;".into()),
            (BlockKind::Paragraph, "end".into()),
        ]
    );
    let p = &doc.blocks[1].spans;
    let styled = |t: &str| p.iter().find(|s| s.text() == t).unwrap();
    assert!(!styled("plain ").is_bold());
    assert!(styled("heavy").is_bold());
    assert!(styled("slanted").is_italic());
    assert_eq!(styled("ok").href(), Some("https://ok.example"));
    assert_eq!(styled(" bad").href(), None);
}

// --- Editing the model --------------------------------------------------------------

#[test]
fn enter_splits_and_leaves_lists() {
    let mut doc = RichText::new([
        RichBlock::heading(2, [plain("Title")]),
        RichBlock::bullet([plain("item")]),
        RichBlock::bullet([]),
    ]);
    // At a heading's end: a paragraph follows.
    let (caret, inserted) = doc.split(RichPosition::new(0, 5));
    assert_eq!((caret, inserted), (RichPosition::new(1, 0), Some(1)));
    assert_eq!(doc.blocks[1].kind, BlockKind::Paragraph);
    // In a list item: another item.
    let (caret, _) = doc.split(RichPosition::new(2, 2));
    assert_eq!(caret, RichPosition::new(3, 0));
    assert_eq!(
        texts(&doc)[2..4],
        [
            (BlockKind::Bullet, "it".into()),
            (BlockKind::Bullet, "em".into())
        ]
    );
    // An empty item leaves the list.
    let (caret, inserted) = doc.split(RichPosition::new(4, 0));
    assert_eq!((caret, inserted), (RichPosition::new(4, 0), None));
    assert_eq!(doc.blocks[4].kind, BlockKind::Paragraph);
    // At a block's start: an empty block above, the caret stays with the text.
    let (caret, inserted) = doc.split(RichPosition::new(0, 0));
    assert_eq!((caret, inserted), (RichPosition::new(1, 0), Some(0)));
    assert_eq!(
        texts(&doc)[0..2],
        [
            (BlockKind::Paragraph, "".into()),
            (BlockKind::Heading(2), "Title".into())
        ]
    );
}

#[test]
fn backspace_and_delete_join_blocks() {
    let mut doc = RichText::new([
        RichBlock::paragraph([plain("one")]),
        RichBlock::bullet([plain("two").bold()]),
        RichBlock::paragraph([plain("three")]),
    ]);
    // A list item first becomes a paragraph…
    assert_eq!(
        doc.backspace_at_start(1),
        Some((RichPosition::new(1, 0), None))
    );
    assert_eq!(doc.blocks[1].kind, BlockKind::Paragraph);
    // … then joins the block before, its marks kept.
    assert_eq!(
        doc.backspace_at_start(1),
        Some((RichPosition::new(0, 3), Some(1)))
    );
    assert_eq!(doc.blocks[0].text(), "onetwo");
    assert!(doc.blocks[0].spans[1].is_bold());
    assert_eq!(doc.backspace_at_start(0), None);
    assert_eq!(doc.delete_at_end(0), Some(RichPosition::new(0, 6)));
    assert_eq!(texts(&doc), [(BlockKind::Paragraph, "onetwothree".into())]);
    assert_eq!(doc.delete_at_end(0), None);
}

#[test]
fn a_fragment_of_blocks_splits_the_block_it_lands_in() {
    let mut doc = RichText::new([RichBlock::bullet([plain("headtail")])]);
    let frag = RichText::from_markdown("A\n\n## B\n\nC");
    let (caret, range, count) =
        doc.replace(RichPosition::new(0, 4), RichPosition::new(0, 4), &frag);
    assert_eq!((caret, range, count), (RichPosition::new(2, 1), 0..=0, 3));
    assert_eq!(
        texts(&doc),
        [
            (BlockKind::Bullet, "headA".into()),
            (BlockKind::Heading(2), "B".into()),
            (BlockKind::Paragraph, "Ctail".into()),
        ]
    );
    // Across blocks: what is between goes.
    let (caret, range, count) = doc.replace(
        RichPosition::new(0, 2),
        RichPosition::new(2, 1),
        &RichText::default(),
    );
    assert_eq!((caret, range, count), (RichPosition::new(0, 2), 0..=2, 1));
    assert_eq!(texts(&doc), [(BlockKind::Bullet, "hetail".into())]);
}

#[test]
fn formatting_reads_marks_links_and_kinds() {
    let doc = RichText::new([
        RichBlock::heading(2, [plain("ab").bold(), plain("cd").link_to("/x").bold()]),
        RichBlock::paragraph([plain("ef")]),
    ]);
    let at = |b, o| RichSelection::caret(RichPosition::new(b, o));
    assert_eq!(
        doc.formatting(at(0, 1)),
        (true, false, None, BlockKind::Heading(2))
    );
    assert_eq!(
        doc.formatting(at(0, 3)),
        (true, false, Some("/x".into()), BlockKind::Heading(2))
    );
    // At a link's end, typing writes plain text: not in it.
    assert_eq!(doc.formatting(at(0, 4)).2, None);
    let all = RichSelection::new(RichPosition::new(0, 0), RichPosition::new(0, 4));
    assert_eq!(
        doc.formatting(all),
        (true, false, None, BlockKind::Heading(2))
    );
    let across = RichSelection::new(RichPosition::new(0, 1), RichPosition::new(1, 1));
    assert!(!doc.formatting(across).0);
    assert_eq!(
        doc.link_range(RichPosition::new(0, 3)),
        Some((RichPosition::new(0, 2), RichPosition::new(0, 4)))
    );
}

// --- The editor, driven like a backend ------------------------------------------------

/// A page holding an editor, recording its changes and link requests.
struct Page {
    editor: Entity<RichTextInput>,
    changes: Vec<RichText>,
    links: Vec<LinkRequest>,
}

impl Render for Page {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().child(self.editor)
    }
}

fn harness(md: &'static str, inline: bool) -> Harness {
    Harness::new(move |cx| {
        let doc = RichText::from_markdown(md);
        let page = cx.new_entity(|cx: &mut Context<Page>| {
            let on_change = cx.listener(|this: &mut Page, e: &RichTextChange, _| {
                this.changes.push(e.value.clone())
            });
            let on_link =
                cx.listener(|this: &mut Page, e: &LinkRequest, _| this.links.push(e.clone()));
            let editor = cx.new(move |_| {
                let e = rich_text_input(doc)
                    .id("ed")
                    .label("Body")
                    .placeholder("Write")
                    .on_change(on_change)
                    .on_link_request(on_link);
                if inline { e.inline() } else { e }
            });
            Page {
                editor,
                changes: Vec::new(),
                links: Vec::new(),
            }
        });
        mount(cx, page);
    })
}

fn editor(h: &Harness) -> &RichTextInput {
    let page = h.root::<Page>();
    let e = h.rt.app.read(&page).editor;
    h.rt.app.read(&e)
}

fn page(h: &Harness) -> &Page {
    h.rt.app.read(&h.root::<Page>())
}

fn command() -> Modifiers {
    Modifiers {
        meta: cfg!(target_vendor = "apple"),
        control: !cfg!(target_vendor = "apple"),
        ..Modifiers::default()
    }
}

fn key(name: &str, sel: (usize, usize), mods: Modifiers) -> KeyEvent {
    let mut k = KeyEvent::new(name).with_selection(SelectionEvent::new(sel.0, sel.1));
    k.modifiers = mods;
    k
}

/// Types `text` into field `name` (its whole new plain value), the caret after it.
fn type_in(h: &mut Harness, name: &str, value: &str, caret: usize) {
    let id = h.by_name(name);
    h.dispatch(UiEvent::Input(id, value.into()));
    h.dispatch(UiEvent::Select(id, caret, caret));
}

#[test]
fn typing_is_reported_after_the_event() {
    let mut h = harness("", false);
    // One empty paragraph, with the placeholder, in a labelled group.
    let field = h.by_name("ed-1");
    assert_eq!(h.host.get(field).placeholder, "Write");
    assert_eq!(h.host.get(field).a11y.label.as_deref(), Some("Body"));
    type_in(&mut h, "ed-1", "Hello", 5);
    assert_eq!(editor(&h).value().text(), "Hello");
    assert_eq!(
        page(&h).changes.last().map(RichText::text).as_deref(),
        Some("Hello")
    );
    // The placeholder goes once there is text.
    assert_eq!(h.host.get(h.by_name("ed-1")).placeholder, "");
}

#[test]
fn enter_backspace_and_arrows_move_between_blocks() {
    let mut h = harness("Hello world", false);
    let first = h.by_name("ed-1");
    h.dispatch(UiEvent::Select(first, 5, 5));
    h.dispatch(UiEvent::Submit(first));
    assert_eq!(
        texts(editor(&h).value()),
        [
            (BlockKind::Paragraph, "Hello".into()),
            (BlockKind::Paragraph, " world".into())
        ]
    );
    // The caret went to the new block, which took the focus.
    let second = h.by_name("ed-2");
    assert_eq!(
        editor(&h).selection(),
        RichSelection::caret(RichPosition::new(1, 0))
    );
    assert!(
        h.last
            .iter()
            .any(|p| matches!(p, Patch::Focus { id } if *id == second)),
        "{:?}",
        h.last
    );
    // A late selection report from the first field is dropped.
    h.dispatch(UiEvent::Select(first, 6, 6));
    assert_eq!(editor(&h).selection().head, RichPosition::new(1, 0));
    // Backspace at the start joins the blocks again.
    h.dispatch(UiEvent::Key(
        Some(second),
        key("Backspace", (0, 0), Modifiers::default()),
    ));
    assert_eq!(
        texts(editor(&h).value()),
        [(BlockKind::Paragraph, "Hello world".into())]
    );
    assert_eq!(editor(&h).selection().head, RichPosition::new(0, 5));
    assert_eq!(page(&h).changes.len(), 2);
}

#[test]
fn shortcuts_format_the_selection_and_the_caret() {
    let mut h = harness("one two", false);
    let field = h.by_name("ed-1");
    // ⌘B / Ctrl+B are the editor's: the platform does not get them.
    let bold = key("b", (0, 3), command());
    assert!(h.rt.consumes_key_event(Some(field), &bold));
    assert!(
        !h.rt
            .consumes_key_event(Some(field), &key("b", (0, 3), Modifiers::default()))
    );
    h.dispatch(UiEvent::Select(field, 0, 3));
    h.dispatch(UiEvent::Key(Some(field), bold));
    assert_eq!(editor(&h).value().to_markdown(), "**one** two");
    assert!(editor(&h).formatting().bold);
    // Again: off.
    h.dispatch(UiEvent::Key(Some(field), key("b", (0, 3), command())));
    assert_eq!(editor(&h).value().to_markdown(), "one two");
    // At a caret: what is typed next is italic.
    h.dispatch(UiEvent::Select(field, 7, 7));
    h.dispatch(UiEvent::Key(Some(field), key("i", (7, 7), command())));
    assert!(editor(&h).formatting().italic);
    type_in(&mut h, "ed-1", "one two!", 8);
    assert_eq!(editor(&h).value().to_markdown(), "one two*!*");
    // Moving away forgets it.
    h.dispatch(UiEvent::Select(field, 2, 2));
    assert!(!editor(&h).formatting().italic);
}

#[test]
fn links_are_asked_from_the_app() {
    let mut h = harness("see the docs", false);
    let field = h.by_name("ed-1");
    h.dispatch(UiEvent::Select(field, 4, 12));
    h.dispatch(UiEvent::Key(Some(field), key("k", (4, 12), command())));
    let req = page(&h).links.last().cloned().expect("a link request");
    assert_eq!((req.href, req.text.as_str()), (None, "the docs"));
    // The app's picker answers; the focus went elsewhere meanwhile.
    let e = editor_entity(&h);
    h.rt.app
        .update(&e, |ed, cx| ed.set_link("https://docs.example", cx));
    h.render();
    assert_eq!(
        editor(&h).value().to_markdown(),
        "see [the docs](https://docs.example)"
    );
    // A caret in the link: its request carries the target; remove it.
    h.dispatch(UiEvent::Select(field, 6, 6));
    assert_eq!(
        editor(&h).formatting().link.as_deref(),
        Some("https://docs.example")
    );
    h.dispatch(UiEvent::Key(Some(field), key("k", (6, 6), command())));
    let req = page(&h).links.last().cloned().unwrap();
    assert_eq!(
        (req.href.as_deref(), req.text.as_str()),
        (Some("https://docs.example"), "the docs")
    );
    h.rt.app.update(&e, |ed, cx| ed.remove_link(cx));
    h.render();
    assert_eq!(editor(&h).value().to_markdown(), "see the docs");
}

fn editor_entity(h: &Harness) -> Entity<RichTextInput> {
    h.rt.app.read(&h.root::<Page>()).editor
}

#[test]
fn toolbar_commands_set_blocks() {
    let mut h = harness("a\n\nb\n\nc", false);
    let e = editor_entity(&h);
    h.rt.app.update(&e, |ed, cx| {
        ed.select(
            RichSelection::new(RichPosition::new(0, 0), RichPosition::new(1, 1)),
            cx,
        );
        ed.toggle_block(BlockKind::Bullet, cx);
    });
    h.render();
    assert_eq!(editor(&h).value().to_markdown(), "- a\n- b\n\nc");
    assert_eq!(editor(&h).formatting().block, BlockKind::Bullet);
    h.rt.app
        .update(&e, |ed, cx| ed.toggle_block(BlockKind::Bullet, cx));
    h.render();
    assert_eq!(editor(&h).value().to_markdown(), "a\n\nb\n\nc");
    // The markers are drawn, hidden from screen readers.
    h.rt.app
        .update(&e, |ed, cx| ed.set_block(BlockKind::Numbered, cx));
    h.render();
    assert!(
        h.host.find(|n| n.text == "2." && n.a11y.hidden).is_some(),
        "{}",
        h.describe()
    );
}

#[test]
fn markdown_shortcuts_make_lists_and_headings() {
    let mut h = harness("", false);
    type_in(&mut h, "ed-1", "-", 1);
    type_in(&mut h, "ed-1", "- ", 2);
    assert_eq!(texts(editor(&h).value()), [(BlockKind::Bullet, "".into())]);
    assert_eq!(editor(&h).selection().head, RichPosition::new(0, 0));
    let mut h = harness("", false);
    type_in(&mut h, "ed-1", "##", 2);
    type_in(&mut h, "ed-1", "## ", 3);
    assert_eq!(
        texts(editor(&h).value()),
        [(BlockKind::Heading(2), "".into())]
    );
}

#[test]
fn pastes_insert_blocks_from_the_richest_type() {
    let mut h = harness("startend", false);
    let field = h.by_name("ed-1");
    let paste = PasteEvent::new(SelectionEvent::caret(5), "plain one\nplain two")
        .with_html(Some("<h2>Title</h2><p>body <b>bold</b></p>".into()));
    h.dispatch(UiEvent::Paste(field, paste));
    assert_eq!(
        texts(editor(&h).value()),
        [
            (BlockKind::Paragraph, "startTitle".into()),
            (BlockKind::Paragraph, "body boldend".into())
        ]
    );
    // Its own content first (copied from an editor).
    let mut h = harness("", false);
    let field = h.by_name("ed-1");
    let paste = PasteEvent::new(SelectionEvent::caret(0), "x")
        .with_data(vec![(RICH_TEXT_TYPE.into(), "- *x*".into())])
        .with_html(Some("<p>other</p>".into()));
    h.dispatch(UiEvent::Paste(field, paste));
    assert_eq!(editor(&h).value().to_markdown(), "- *x*");
}

#[test]
fn copies_write_text_html_and_markdown() {
    let mut h = harness("a **bold** move", false);
    let field = h.by_name("ed-1");
    h.dispatch(UiEvent::Select(field, 2, 6));
    let out = h
        .dispatch(UiEvent::Copy(field, false, 2, 6, "bold".into()))
        .to_vec();
    let content = out
        .iter()
        .find_map(|p| match p {
            Patch::WriteClipboard { content, .. } => Some(content.clone()),
            _ => None,
        })
        .expect("written");
    assert_eq!(content.plain_text(), "bold");
    assert_eq!(content.markdown_text(), Some("**bold**"));
    assert_eq!(content.html_text(), Some("<p><strong>bold</strong></p>"));
    assert_eq!(content.get(RICH_TEXT_TYPE), Some("**bold**"));
}

#[test]
fn inline_mode_stays_on_one_line() {
    let mut h = harness("one\n\ntwo", true);
    assert_eq!(
        texts(editor(&h).value()),
        [(BlockKind::Paragraph, "one two".into())]
    );
    let field = h.by_name("ed-1");
    h.dispatch(UiEvent::Select(field, 7, 7));
    h.dispatch(UiEvent::Submit(field));
    assert_eq!(editor(&h).value().blocks.len(), 1);
    let paste = PasteEvent::new(SelectionEvent::caret(7), "a\nb");
    h.dispatch(UiEvent::Paste(field, paste));
    assert_eq!(editor(&h).value().text(), "one twoa b");
    // ⌘B still works.
    h.dispatch(UiEvent::Select(field, 0, 3));
    h.dispatch(UiEvent::Key(Some(field), key("b", (0, 3), command())));
    assert_eq!(editor(&h).value().to_markdown(), "**one** twoa b");
}

/// A page whose link handler answers at once, from the listener.
struct Instant {
    editor: Option<Entity<RichTextInput>>,
    changes: Vec<String>,
}

impl Render for Instant {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().children(self.editor)
    }
}

#[test]
fn a_link_request_can_be_answered_from_its_handler() {
    let mut h = Harness::new(|cx| {
        let page = cx.new_entity(|cx: &mut Context<Instant>| {
            let on_link = cx.listener(|this: &mut Instant, _: &LinkRequest, cx| {
                let Some(e) = this.editor else { return };
                e.update(cx, |ed, cx| ed.set_link("https://prismsdk.dev", cx));
            });
            let on_change = cx.listener(|this: &mut Instant, e: &RichTextChange, _| {
                this.changes.push(e.value.to_markdown())
            });
            let editor = cx.new(move |_| {
                rich_text_input(RichText::from_markdown("one two"))
                    .id("ed")
                    .on_link_request(on_link)
                    .on_change(on_change)
            });
            Instant {
                editor: Some(editor),
                changes: Vec::new(),
            }
        });
        mount(cx, page);
    });
    let field = h.by_name("ed-1");
    h.dispatch(UiEvent::Select(field, 0, 3));
    h.dispatch(UiEvent::Key(Some(field), key("k", (0, 3), command())));
    let e = h.rt.app.read(&h.root::<Instant>()).editor.unwrap();
    assert_eq!(
        h.rt.app.read(&e).value().to_markdown(),
        "[one](https://prismsdk.dev) two"
    );
    // The change it made is reported too.
    let changes = &h.rt.app.read(&h.root::<Instant>()).changes;
    assert_eq!(
        changes.last().map(String::as_str),
        Some("[one](https://prismsdk.dev) two")
    );
}

#[test]
fn arrows_at_a_blocks_edges_are_the_editors() {
    use prism_core::CaretEdges;
    let mut h = harness("one\n\ntwo", false);
    let (first, second) = (h.by_name("ed-1"), h.by_name("ed-2"));
    let at = |name: &str, sel: (usize, usize), edges: CaretEdges| {
        let mut k = key(name, sel, Modifiers::default());
        k.edges = Some(edges);
        k
    };
    let start = CaretEdges {
        start: true,
        end: false,
        first_line: true,
        last_line: true,
    };
    let end = CaretEdges {
        start: false,
        end: true,
        first_line: true,
        last_line: true,
    };
    // Out of the document's ends, the field keeps them.
    assert!(
        !h.rt
            .consumes_key_event(Some(first), &at("ArrowUp", (0, 0), start))
    );
    assert!(
        !h.rt
            .consumes_key_event(Some(second), &at("ArrowRight", (3, 3), end))
    );
    // Between blocks, they move the caret to the next one.
    assert!(h.rt.consumes_key_event(Some(first), &at("ArrowRight", (3, 3), end)));
    h.dispatch(UiEvent::Key(Some(first), at("ArrowRight", (3, 3), end)));
    assert_eq!(
        editor(&h).selection(),
        RichSelection::caret(RichPosition::new(1, 0))
    );
    assert!(h.rt.consumes_key_event(Some(second), &at("ArrowUp", (0, 0), start)));
    h.dispatch(UiEvent::Key(Some(second), at("ArrowUp", (0, 0), start)));
    assert_eq!(
        editor(&h).selection(),
        RichSelection::caret(RichPosition::new(0, 3))
    );
}
