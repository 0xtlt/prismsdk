//! [`RichTextInput`]: the editor of a [`RichText`] (#294).

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use prism_core::{
    Accessible, App, CaretEdge, ClipboardContent, Color, Context, CopyEvent, EditAcross, EditCause,
    FluentBuilder, FocusEvent, FontWeight, Handler, IntoElement, KeyEvent, ParentElement,
    PasteEvent, RichInputEvent, Role, SelectionAcross, SelectionEvent, Span, Style, Styled, div,
    normalize_spans, px, restyle_spans, rich_input, span, split_spans, text, text_flow,
};

use super::{BlockKind, RichBlock, RichPosition, RichSelection, RichText};
use crate::Theme;

/// The clipboard type an editor writes its own content as (Markdown), read
/// back first when it pastes.
pub const RICH_TEXT_TYPE: &str = "application/x-prism-rich-text";

/// What a paste inserts in a [`RichTextInput`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PasteMode {
    /// The model's subset of what the clipboard holds: an editor's own
    /// content, else its HTML (headings, paragraphs, lists, bold, italic,
    /// safe links), else its Markdown, else its plain text.
    #[default]
    Rich,
    /// Plain text only, in the style of where it lands (a line per
    /// paragraph).
    Plain,
}

/// The state a toolbar shows for the selection of a [`RichTextInput`]:
/// which marks it has, its block's kind, the link it is in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Formatting {
    /// The selection is bold (all of it), or text typed at the caret will be.
    pub bold: bool,
    /// The selection is italic (all of it), or text typed at the caret will be.
    pub italic: bool,
    /// The link the caret is in, or that the whole selection is.
    pub link: Option<String>,
    /// The kind of the block where the selection starts.
    pub block: BlockKind,
}

impl Formatting {
    /// Whether the selection is in a link.
    pub fn in_link(&self) -> bool {
        self.link.is_some()
    }
}

/// The value of a [`RichTextInput`] changed: the user edited it, or a
/// command did ([`RichTextInput::on_change`]).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RichTextChange {
    /// The new value.
    pub value: RichText,
}

/// The user asked for a link (⌘K, Ctrl+K, or
/// [`request_link`](RichTextInput::request_link) from a toolbar): open
/// the app's own link picker, then call
/// [`set_link`](RichTextInput::set_link) or
/// [`remove_link`](RichTextInput::remove_link)
/// ([`RichTextInput::on_link_request`]).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LinkRequest {
    /// The link the selection is in, to edit it.
    pub href: Option<String>,
    /// The selected text (the link's text, for a caret in a link).
    pub text: String,
    /// What the link will cover.
    pub selection: RichSelection,
}

/// Marks for the text typed next at a caret (⌘B with nothing selected).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pending {
    at: RichPosition,
    bold: bool,
    italic: bool,
}

/// What the editor reports to the app's handlers, after its own update.
enum Report {
    Change,
    Link(LinkRequest),
}

static NEXT_EDITOR: AtomicU64 = AtomicU64::new(1);

/// The editor of a [`RichText`] (#294): paragraphs, headings and lists of
/// bold, italic and linked runs, edited without markup. Create it once
/// with [`rich_text_input`] in an entity (`cx.new`), show it as a child,
/// read its [`value`](Self::value) and [`formatting`](Self::formatting),
/// call its commands from a toolbar. See the guide's
/// [rich text editor](https://prismsdk.dev/guide/forms.html#rich-text-editor).
///
/// - **Typing**: Enter splits the block (an empty list item leaves the
///   list), Backspace at a block's start turns a heading or a list item
///   into a paragraph, then joins it to the block before; Delete at its
///   end joins the next one. ↑ ↓ ← → cross from block to block, ⇧ with
///   them selects across blocks.
/// - **Shortcuts**: ⌘B / Ctrl+B bold, ⌘I / Ctrl+I italic, ⌘K / Ctrl+K a
///   [`LinkRequest`]; the platform (a browser's bookmarks, its search
///   bar) does not get them. Typing `- `, `* `, `1. `, `# `, `## ` or
///   `### ` at the start of a paragraph makes it a list item or a heading
///   ([`markdown_shortcuts`](Self::markdown_shortcuts)).
/// - **Inline mode** ([`inline`](Self::inline)): one line, bold, italic
///   and links; Enter does nothing, pastes are joined on one line.
/// - **Accessibility**: each block is a text field (`role=textbox`,
///   `aria-multiline` on the web) labelled with the editor's
///   [`label`](Self::label), a heading's or a list item's kind as its hint;
///   the blocks are one group with that label.
pub struct RichTextInput {
    value: RichText,
    keys: Vec<u64>,
    next_key: u64,
    name: String,
    selection: RichSelection,
    pending: Option<Pending>,
    /// What a [`LinkRequest`] covers, until the app answers it.
    link_selection: Option<RichSelection>,
    /// After the editor moved the caret to another block: that block's
    /// key. Selection reports from the others are late (they were the
    /// focused field's) and dropped until it reports.
    focus_target: Option<u64>,
    focused: Option<u64>,
    inline: bool,
    placeholder: String,
    label: String,
    paste: PasteMode,
    shortcuts: bool,
    revision: u64,
    on_change: Option<Handler<RichTextChange>>,
    on_link_request: Option<Handler<LinkRequest>>,
    style: Option<Rc<dyn Fn(Style) -> Style>>,
    reports: Vec<Report>,
    hooked: bool,
}

/// An editor of `value` (#294): `cx.new(|_| ui::rich_text_input(doc))`.
/// See [`RichTextInput`].
pub fn rich_text_input(value: RichText) -> RichTextInput {
    let mut editor = RichTextInput {
        value: RichText::default(),
        keys: Vec::new(),
        next_key: 0,
        name: format!(
            "prism-rich-text-{}",
            NEXT_EDITOR.fetch_add(1, Ordering::Relaxed)
        ),
        selection: RichSelection::default(),
        pending: None,
        link_selection: None,
        focus_target: None,
        focused: None,
        inline: false,
        placeholder: String::new(),
        label: String::new(),
        paste: PasteMode::Rich,
        shortcuts: true,
        revision: 0,
        on_change: None,
        on_link_request: None,
        style: None,
        reports: Vec::new(),
        hooked: false,
    };
    editor.load(value);
    editor
}

impl RichTextInput {
    // --- Options ---------------------------------------------------------------

    /// Text shown while the editor is empty.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Its accessibility label (also its group's): what screen readers
    /// call the field. Defaults to the placeholder.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// One line (a title, a caption): bold, italic and links, no blocks.
    /// Enter does nothing; line breaks pasted or typed become spaces.
    pub fn inline(mut self) -> Self {
        self.inline = true;
        let doc = std::mem::take(&mut self.value);
        self.next_key = 0;
        self.load(doc);
        self
    }

    /// What a paste inserts: [`PasteMode::Rich`] (the default) or plain text.
    pub fn paste(mut self, mode: PasteMode) -> Self {
        self.paste = mode;
        self
    }

    /// Whether typing `- `, `* `, `1. `, `# `, `## ` or `### ` at the start
    /// of a paragraph makes it a list item or a heading (on by default).
    pub fn markdown_shortcuts(mut self, on: bool) -> Self {
        self.shortcuts = on;
        self
    }

    /// Called after each change of the value, by the user or a command,
    /// once the event is handled (so it may update the entity that holds
    /// the editor): store it, convert it to the app's own format.
    /// [`cx.observe`](prism_core::Context::observe) of the editor works
    /// too (it also runs when only the selection moves).
    pub fn on_change(mut self, handler: Handler<RichTextChange>) -> Self {
        self.on_change = Some(handler);
        self
    }

    /// Called on ⌘K / Ctrl+K and [`request_link`](Self::request_link):
    /// open the app's link picker, then answer with
    /// [`set_link`](Self::set_link) or [`remove_link`](Self::remove_link).
    /// Without it, ⌘K does nothing.
    pub fn on_link_request(mut self, handler: Handler<LinkRequest>) -> Self {
        self.on_link_request = Some(handler);
        self
    }

    /// Refines the style of the editor's frame (a field's look by default:
    /// the theme's surface, border and radius): `.style(|s| s.border(0.).p(0.))`
    /// for a document without a frame.
    pub fn style(mut self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.style = Some(Rc::new(f));
        self
    }

    /// The name of the editor's fields (`<id>-<n>`), unique in the
    /// window; one is made up by default.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.name = id.into();
        self
    }

    // --- State -------------------------------------------------------------------

    /// The document.
    pub fn value(&self) -> &RichText {
        &self.value
    }

    /// The selection (a caret when nothing is selected).
    pub fn selection(&self) -> RichSelection {
        self.selection
    }

    /// The selection's marks, block kind and link, for a toolbar.
    pub fn formatting(&self) -> Formatting {
        let (bold, italic, link, block) = self.value.formatting(self.selection);
        match self.pending {
            Some(p) if self.selection.is_caret() && p.at == self.selection.head => Formatting {
                bold: p.bold,
                italic: p.italic,
                link,
                block,
            },
            _ => Formatting {
                bold,
                italic,
                link,
                block,
            },
        }
    }

    /// Whether one of its fields has the keyboard focus.
    pub fn is_focused(&self) -> bool {
        self.focused.is_some()
    }

    /// A number that grows with each change of the value (not of the
    /// selection): an observer can tell the two apart.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    // --- Commands ----------------------------------------------------------------

    /// Replaces the document (the app loaded another one). The caret goes
    /// to its start. Not reported to [`on_change`](Self::on_change).
    pub fn set_value(&mut self, value: RichText, cx: &mut Context<Self>) {
        self.load(value);
        self.revision += 1;
        cx.notify();
    }

    /// Selects `selection` (clamped to the document) and focuses the editor.
    pub fn select(&mut self, selection: RichSelection, cx: &mut Context<Self>) {
        self.selection = RichSelection::new(
            self.value.clamp(selection.anchor),
            self.value.clamp(selection.head),
        );
        self.pending = None;
        self.refocus(cx);
        cx.notify();
    }

    /// Gives the editor the keyboard focus, its selection as it was.
    pub fn focus(&mut self, cx: &mut Context<Self>) {
        self.refocus(cx);
    }

    /// Bold on or off: on the selection (on unless all of it is bold), or
    /// for the text typed next at the caret.
    pub fn toggle_bold(&mut self, cx: &mut Context<Self>) {
        let f = self.formatting();
        self.mark(!f.bold, f.italic, true, cx);
    }

    /// Italic on or off, like [`toggle_bold`](Self::toggle_bold).
    pub fn toggle_italic(&mut self, cx: &mut Context<Self>) {
        let f = self.formatting();
        self.mark(f.bold, !f.italic, false, cx);
    }

    fn mark(&mut self, bold: bool, italic: bool, is_bold: bool, cx: &mut Context<Self>) {
        let sel = self.selection;
        if sel.is_caret() {
            self.pending = Some(Pending {
                at: sel.head,
                bold,
                italic,
            });
            self.refocus(cx);
            cx.notify();
            return;
        }
        self.value
            .restyle(sel.start(), sel.end(), |s| match (is_bold, bold, italic) {
                (true, true, _) => s.bold(),
                (true, false, _) => s.no_bold(),
                (false, _, true) => s.italic(),
                (false, _, false) => s.no_italic(),
            });
        self.refocus(cx);
        self.changed(cx);
    }

    /// Makes the blocks of the selection `kind` (in block mode).
    pub fn set_block(&mut self, kind: BlockKind, cx: &mut Context<Self>) {
        if self.inline {
            return;
        }
        let range = self.selected_blocks();
        for b in &mut self.value.blocks[range] {
            b.kind = kind.clamped();
        }
        self.refocus(cx);
        self.changed(cx);
    }

    /// Makes the blocks of the selection `kind`, or paragraphs if they all
    /// are already (a toolbar's heading or list button).
    pub fn toggle_block(&mut self, kind: BlockKind, cx: &mut Context<Self>) {
        let kind = kind.clamped();
        let all = self.value.blocks[self.selected_blocks()]
            .iter()
            .all(|b| b.kind == kind);
        self.set_block(if all { BlockKind::Paragraph } else { kind }, cx);
    }

    /// Asks the app for a link over the selection ([`on_link_request`](Self::on_link_request)).
    pub fn request_link(&mut self, cx: &mut Context<Self>) {
        let sel = self.link_target();
        self.link_selection = Some(sel);
        let request = LinkRequest {
            href: self.formatting().link,
            text: self.value.slice(sel.start(), sel.end()).text(),
            selection: sel,
        };
        self.reports.push(Report::Link(request));
        cx.notify();
    }

    /// Links the selection (the one of the last [`LinkRequest`] if the app
    /// answers one) to `href`; a caret in a link changes its target, a
    /// caret elsewhere inserts `href` as a linked text.
    pub fn set_link(&mut self, href: impl Into<String>, cx: &mut Context<Self>) {
        let href = href.into();
        let sel = self
            .link_selection
            .take()
            .unwrap_or_else(|| self.link_target());
        if href.is_empty() {
            return self.remove_link_in(sel, cx);
        }
        if sel.is_caret() {
            let frag = RichText::new([RichBlock::paragraph([span(href.clone()).link_to(href)])]);
            let (caret, _, _) = self.value.replace(sel.head, sel.head, &frag);
            self.selection = RichSelection::caret(caret);
        } else {
            self.value
                .restyle(sel.start(), sel.end(), |s| s.link_to(href.clone()));
            self.selection = sel;
        }
        self.refocus(cx);
        self.changed(cx);
    }

    /// Removes the link the caret is in, or the links of the selection.
    pub fn remove_link(&mut self, cx: &mut Context<Self>) {
        let sel = self
            .link_selection
            .take()
            .unwrap_or_else(|| self.link_target());
        self.remove_link_in(sel, cx);
    }

    fn remove_link_in(&mut self, sel: RichSelection, cx: &mut Context<Self>) {
        self.value.restyle(sel.start(), sel.end(), Span::no_link);
        self.refocus(cx);
        self.changed(cx);
    }

    /// What a link command covers: the selection, or the link around the caret.
    fn link_target(&self) -> RichSelection {
        let sel = self.selection;
        if sel.is_caret()
            && let Some((a, b)) = self.value.link_range(sel.head)
        {
            return RichSelection::new(a, b);
        }
        sel
    }

    // --- Internals -----------------------------------------------------------------

    fn load(&mut self, value: RichText) {
        let mut value = value;
        if self.inline {
            value = flatten(&value);
        }
        // A block holds no line break: each starts a new block.
        let mut blocks = Vec::new();
        for b in value.blocks {
            if b.text().contains('\n') {
                let mut rest = b.spans;
                let mut kind = b.kind;
                while let Some(at) = spans_char(&rest, '\n') {
                    let (head, tail) = split_spans(&rest, at);
                    let (_, tail) = split_spans(&tail, 1);
                    blocks.push(RichBlock::new(kind, head));
                    rest = tail;
                    if !kind.is_list() {
                        kind = BlockKind::Paragraph;
                    }
                }
                blocks.push(RichBlock::new(kind, rest));
            } else {
                blocks.push(b);
            }
        }
        self.value = RichText::new(blocks);
        self.value.ensure_block();
        self.keys = (0..self.value.blocks.len())
            .map(|_| self.new_key())
            .collect();
        self.selection = RichSelection::default();
        self.pending = None;
        self.link_selection = None;
        self.focus_target = None;
    }

    fn new_key(&mut self) -> u64 {
        self.next_key += 1;
        self.next_key
    }

    /// The blocks the selection touches.
    fn selected_blocks(&self) -> std::ops::RangeInclusive<usize> {
        let (a, b) = (
            self.value.clamp(self.selection.start()),
            self.value.clamp(self.selection.end()),
        );
        a.block..=b.block
    }

    fn index_of(&self, key: u64) -> Option<usize> {
        self.keys.iter().position(|k| *k == key)
    }

    fn field_id(&self, key: u64) -> String {
        format!("{}-{key}", self.name)
    }

    fn key_of_field(&self, field: &str) -> Option<usize> {
        let key = field
            .strip_prefix(&self.name)?
            .strip_prefix('-')?
            .parse()
            .ok()?;
        self.index_of(key)
    }

    /// The value changed: re-render, and report it once the event is handled.
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        self.reports.push(Report::Change);
        cx.notify();
    }

    /// Runs the app's handlers for what happened, outside the editor's
    /// update (they may update the entity holding it).
    fn hook(&mut self, cx: &mut Context<Self>) {
        if self.hooked {
            return;
        }
        self.hooked = true;
        let this = cx.entity();
        cx.on_notify(&this, move |app: &mut App| {
            // A handler may change the editor again (a link picker that
            // answers at once): its reports are delivered here too, as the
            // editor's notification is the one being handled.
            loop {
                if !app.contains(this.id()) {
                    return;
                }
                let (reports, value, on_change, on_link) = app.update(&this, |e, _| {
                    (
                        std::mem::take(&mut e.reports),
                        e.value.clone(),
                        e.on_change.clone(),
                        e.on_link_request.clone(),
                    )
                });
                if reports.is_empty() {
                    return;
                }
                let mut changed = false;
                for r in reports {
                    match r {
                        Report::Change => changed = true,
                        Report::Link(request) => {
                            if let Some(h) = &on_link {
                                h(&request, app);
                            }
                        }
                    }
                }
                if changed && let Some(h) = on_change {
                    h(&RichTextChange { value }, app);
                }
            }
        });
    }

    /// Puts the focus (and the selection) back in the editor's fields,
    /// after the next render.
    fn refocus(&mut self, cx: &mut Context<Self>) {
        let sel = RichSelection::new(
            self.value.clamp(self.selection.anchor),
            self.value.clamp(self.selection.head),
        );
        self.selection = sel;
        let (Some(&a), Some(&h)) = (
            self.keys.get(sel.anchor.block),
            self.keys.get(sel.head.block),
        ) else {
            return;
        };
        self.focus_target = Some(h);
        if self.inline {
            cx.focus(self.field_id(h));
        } else {
            cx.set_selection_across(
                (self.field_id(a), sel.anchor.offset),
                (self.field_id(h), sel.head.offset),
            );
        }
    }

    /// Moves the caret to `pos` (another block, after an edit).
    fn caret_to(&mut self, pos: RichPosition, cx: &mut Context<Self>) {
        self.selection = RichSelection::caret(self.value.clamp(pos));
        self.pending = None;
        self.refocus(cx);
    }

    /// Keeps a key per block after an edit that replaced `range` with
    /// `count` blocks (the first keeps its key).
    fn rekey(&mut self, range: std::ops::RangeInclusive<usize>, count: usize) {
        let (start, end) = (
            *range.start(),
            (*range.end()).min(self.keys.len().saturating_sub(1)),
        );
        let first = self.keys.get(start).copied();
        let mut new: Vec<u64> = Vec::with_capacity(count);
        for i in 0..count {
            match (i, first) {
                (0, Some(k)) => new.push(k),
                _ => new.push(self.new_key()),
            }
        }
        if self.keys.is_empty() {
            self.keys = new;
        } else {
            self.keys.splice(start..=end, new);
        }
        self.sync_keys();
    }

    /// One key per block, whatever happened.
    fn sync_keys(&mut self) {
        while self.keys.len() < self.value.blocks.len() {
            let k = self.new_key();
            self.keys.push(k);
        }
        self.keys.truncate(self.value.blocks.len());
    }

    /// Replaces `start..end` with `fragment`, the caret after it.
    fn insert(
        &mut self,
        start: RichPosition,
        end: RichPosition,
        fragment: &RichText,
        cx: &mut Context<Self>,
    ) {
        let (caret, range, count) = self.value.replace(start, end, fragment);
        self.rekey(range, count);
        self.caret_to(caret, cx);
        self.changed(cx);
    }

    /// Splits at `at` (Enter).
    fn split_at(&mut self, at: RichPosition, cx: &mut Context<Self>) {
        let (caret, inserted) = self.value.split(at);
        if let Some(i) = inserted {
            let k = self.new_key();
            self.keys.insert(i.min(self.keys.len()), k);
        }
        self.sync_keys();
        self.caret_to(caret, cx);
        self.changed(cx);
    }

    // --- Field events ----------------------------------------------------------------

    fn input(&mut self, key: u64, e: &RichInputEvent, cx: &mut Context<Self>) {
        let Some(i) = self.index_of(key) else {
            return;
        };
        let old_len = self.value.blocks[i].len();
        let old_text = self.value.blocks[i].text();
        let mut spans = e.spans.clone();
        let new_len: usize = spans.iter().map(Span::len).sum();
        // Marks chosen at the caret (⌘B with nothing selected) style what is typed there.
        if let Some(p) = self.pending.filter(|p| p.at.block == i)
            && new_len > old_len
        {
            let n = new_len - old_len;
            let at = p.at.offset;
            spans = restyle_spans(&spans, at..at + n, |s| {
                let s = if p.bold { s.bold() } else { s.no_bold() };
                if p.italic { s.italic() } else { s.no_italic() }
            });
            self.pending = Some(Pending {
                at: RichPosition::new(i, at + n),
                ..p
            });
        }
        let has_break = spans.iter().any(|s| s.text().contains('\n'));
        if self.inline && has_break {
            spans = spans
                .into_iter()
                .map(|s| {
                    let t = s.text().replace('\n', " ");
                    s.with_text(t)
                })
                .collect();
        }
        self.value.blocks[i].spans = normalize_spans(spans);
        if !self.inline && has_break {
            // Shift+Enter, or a line break the platform typed: new blocks.
            let mut caret = RichPosition::new(i, 0);
            let mut block = i;
            while let Some(at) = spans_char(&self.value.blocks[block].spans, '\n') {
                let spans = &self.value.blocks[block].spans;
                let (head, tail) = split_spans(spans, at);
                let (_, tail) = split_spans(&tail, 1);
                self.value.blocks[block].spans = normalize_spans(head.into_iter().chain(tail));
                let (c, inserted) = self.value.split(RichPosition::new(block, at));
                if let Some(j) = inserted {
                    let k = self.new_key();
                    self.keys.insert(j.min(self.keys.len()), k);
                }
                self.sync_keys();
                caret = c;
                block = c.block;
            }
            self.caret_to(caret, cx);
        } else if self.shortcuts
            && !self.inline
            && self.value.blocks[i].kind == BlockKind::Paragraph
        {
            self.shortcut(i, &old_text, cx);
        }
        self.link_selection = None;
        self.changed(cx);
    }

    /// `- ` and the others typed at the start of a paragraph.
    fn shortcut(&mut self, i: usize, old_text: &str, cx: &mut Context<Self>) {
        let text = self.value.blocks[i].text();
        const PREFIXES: [(&str, BlockKind); 6] = [
            ("- ", BlockKind::Bullet),
            ("* ", BlockKind::Bullet),
            ("1. ", BlockKind::Numbered),
            ("# ", BlockKind::Heading(1)),
            ("## ", BlockKind::Heading(2)),
            ("### ", BlockKind::Heading(3)),
        ];
        for (prefix, kind) in PREFIXES {
            let typed = text.starts_with(prefix)
                && !old_text.starts_with(prefix)
                && text.chars().count() == old_text.chars().count() + 1
                && old_text.starts_with(&prefix[..prefix.len() - 1]);
            if typed {
                let n = prefix.chars().count();
                let (_, rest) = split_spans(&self.value.blocks[i].spans, n);
                let block = &mut self.value.blocks[i];
                block.spans = rest;
                block.kind = kind;
                self.caret_to(RichPosition::new(i, 0), cx);
                return;
            }
        }
    }

    fn select_in(&mut self, key: u64, e: SelectionEvent) {
        let Some(i) = self.index_of(key) else {
            return;
        };
        match self.focus_target {
            Some(t) if t != key => return,
            Some(_) => self.focus_target = None,
            None => {}
        }
        let len = self.value.blocks[i].len();
        self.selection = RichSelection::new(
            RichPosition::new(i, e.start.min(len)),
            RichPosition::new(i, e.end.min(len)),
        );
        self.focused = Some(key);
        if self
            .pending
            .is_some_and(|p| !(self.selection.is_caret() && p.at == self.selection.head))
        {
            self.pending = None;
        }
    }

    fn across(&mut self, e: &SelectionAcross) {
        if !e.is_across() {
            return;
        }
        let point = |p: &prism_core::SelectionPoint| {
            self.key_of_field(&p.field)
                .map(|b| RichPosition::new(b, p.offset))
        };
        if let (Some(a), Some(h)) = (point(&e.anchor), point(&e.head)) {
            self.selection = RichSelection::new(self.value.clamp(a), self.value.clamp(h));
            self.pending = None;
            self.focus_target = None;
        }
    }

    fn edit_across(&mut self, e: &EditAcross, cx: &mut Context<Self>) {
        let point = |p: &prism_core::SelectionPoint| {
            self.key_of_field(&p.field)
                .map(|b| RichPosition::new(b, p.offset))
        };
        let (Some(a), Some(b)) = (point(e.selection.start()), point(e.selection.end())) else {
            return;
        };
        let (start, end) = (a.min(b), a.max(b));
        match e.cause {
            EditCause::Enter => {
                let (caret, range, count) = self.value.replace(start, end, &RichText::default());
                self.rekey(range, count);
                self.split_at(caret, cx);
            }
            EditCause::Typing | EditCause::Paste if !e.text.is_empty() => {
                let frag = self.plain_fragment(&e.text, start);
                self.insert(start, end, &frag, cx);
            }
            _ => self.insert(start, end, &RichText::default(), cx),
        }
    }

    fn key(&mut self, key: u64, e: &KeyEvent, cx: &mut Context<Self>) {
        let Some(i) = self.index_of(key) else {
            return;
        };
        if let Some(sel) = e.selection {
            let len = self.value.blocks[i].len();
            self.selection = RichSelection::new(
                RichPosition::new(i, sel.start.min(len)),
                RichPosition::new(i, sel.end.min(len)),
            );
        }
        self.focus_target = None;
        let m = e.modifiers;
        if m.command() && !m.alt && !m.shift {
            if e.is("b") {
                self.toggle_bold(cx);
            } else if e.is("i") {
                self.toggle_italic(cx);
            } else if e.is("k") {
                self.request_link(cx);
            }
            return;
        }
        if self.inline || !m.is_empty() {
            return;
        }
        let len = self.value.blocks[i].len();
        let caret = e.selection.filter(|s| s.start == s.end).map(|s| s.start);
        let n = self.value.blocks.len();
        match e.key.as_str() {
            "Backspace" if caret == Some(0) => {
                if let Some((pos, removed)) = self.value.backspace_at_start(i) {
                    if let Some(r) = removed {
                        self.keys.remove(r.min(self.keys.len() - 1));
                    }
                    self.sync_keys();
                    self.caret_to(pos, cx);
                    self.changed(cx);
                }
            }
            "Delete" if caret == Some(len) => {
                if let Some(pos) = self.value.delete_at_end(i) {
                    self.keys.remove((i + 1).min(self.keys.len() - 1));
                    self.sync_keys();
                    self.caret_to(pos, cx);
                    self.changed(cx);
                }
            }
            "ArrowUp" if i > 0 && e.edges.is_some() && e.at(CaretEdge::FirstLine) => {
                let prev = self.value.blocks[i - 1].len();
                self.caret_to(RichPosition::new(i - 1, prev), cx);
            }
            "ArrowDown" if i + 1 < n && e.edges.is_some() && e.at(CaretEdge::LastLine) => {
                self.caret_to(RichPosition::new(i + 1, 0), cx);
            }
            "ArrowLeft" if i > 0 && caret == Some(0) => {
                let prev = self.value.blocks[i - 1].len();
                self.caret_to(RichPosition::new(i - 1, prev), cx);
            }
            "ArrowRight" if i + 1 < n && caret == Some(len) => {
                self.caret_to(RichPosition::new(i + 1, 0), cx);
            }
            _ => {}
        }
    }

    fn submit(&mut self, key: u64, cx: &mut Context<Self>) {
        if self.inline {
            return;
        }
        let Some(i) = self.index_of(key) else {
            return;
        };
        let sel = self.selection;
        let (start, end) = if sel.start().block == i || sel.end().block == i {
            (sel.start(), sel.end())
        } else {
            let at = RichPosition::new(i, self.value.blocks[i].len());
            (at, at)
        };
        let at = if start != end {
            let (caret, range, count) = self.value.replace(start, end, &RichText::default());
            self.rekey(range, count);
            caret
        } else {
            start
        };
        self.split_at(at, cx);
    }

    fn focus_change(&mut self, key: u64, e: &FocusEvent) {
        if e.focused {
            self.focused = Some(key);
        } else if self.focused == Some(key) {
            self.focused = None;
        }
    }

    /// What a paste over `sel` of block `i` inserts.
    fn pasted(&self, e: &PasteEvent, at: RichPosition) -> RichText {
        let rich = match self.paste {
            PasteMode::Plain => None,
            PasteMode::Rich => e
                .get(RICH_TEXT_TYPE)
                .map(RichText::from_markdown)
                .or_else(|| {
                    e.html
                        .as_deref()
                        .map(RichText::from_html)
                        .filter(|d| !d.blocks.is_empty())
                })
                .or_else(|| {
                    e.get(prism_core::text_edit::TEXT_MARKDOWN)
                        .map(RichText::from_markdown)
                }),
        };
        let frag = rich.unwrap_or_else(|| self.plain_fragment(&e.text, at));
        if self.inline { flatten(&frag) } else { frag }
    }

    /// Plain text pasted or typed at `at`: a paragraph per line, in the
    /// marks of where it lands (not its link).
    fn plain_fragment(&self, text: &str, at: RichPosition) -> RichText {
        let (bold, italic, _, _) = self.value.formatting(RichSelection::caret(at));
        let text: String = text
            .chars()
            .filter(|c| *c != prism_core::OBJECT_CHAR)
            .collect();
        let style = |line: &str| {
            let mut s = span(line.trim_end_matches('\r'));
            if bold {
                s = s.bold();
            }
            if italic {
                s = s.italic();
            }
            s
        };
        let doc = RichText::new(text.split('\n').map(|l| RichBlock::paragraph([style(l)])));
        if self.inline { flatten(&doc) } else { doc }
    }

    fn paste_in(&mut self, key: u64, e: &PasteEvent, cx: &mut Context<Self>) {
        let Some(i) = self.index_of(key) else {
            return;
        };
        e.prevent_default();
        let len = self.value.blocks[i].len();
        let (start, end) = (
            RichPosition::new(i, e.selection.start.min(len)),
            RichPosition::new(i, e.selection.end.min(len)),
        );
        let frag = self.pasted(e, start);
        self.insert(start, end, &frag, cx);
    }

    fn copy(&self, key: u64, e: &CopyEvent) {
        let Some(i) = self.index_of(key) else {
            return;
        };
        let (start, end) = match &e.across {
            Some(across) => {
                let point = |p: &prism_core::SelectionPoint| {
                    self.key_of_field(&p.field)
                        .map(|b| RichPosition::new(b, p.offset))
                };
                match (point(across.start()), point(across.end())) {
                    (Some(a), Some(b)) => (a, b),
                    _ => return,
                }
            }
            None => (
                RichPosition::new(i, e.selection.start),
                RichPosition::new(i, e.selection.end),
            ),
        };
        let frag = self.value.slice(start, end);
        let markdown = frag.to_markdown();
        e.write(
            ClipboardContent::text(frag.text())
                .html(frag.to_html())
                .markdown(markdown.clone())
                .custom(RICH_TEXT_TYPE, markdown),
        );
    }
}

/// The first `c` in `spans`, in characters.
fn spans_char(spans: &[Span], c: char) -> Option<usize> {
    let mut at = 0;
    for s in spans {
        if !s.is_object()
            && let Some(i) = s.text().chars().position(|x| x == c)
        {
            return Some(at + i);
        }
        at += s.len();
    }
    None
}

/// The document on one line: its blocks' runs joined by spaces.
fn flatten(doc: &RichText) -> RichText {
    let mut spans: Vec<Span> = Vec::new();
    for (i, b) in doc.blocks.iter().enumerate() {
        if i > 0 && !b.is_empty() && !spans.is_empty() {
            spans.push(span(" "));
        }
        spans.extend(b.spans.iter().map(|s| {
            let t = s.text().replace('\n', " ");
            s.clone().with_text(t)
        }));
    }
    RichText::new([RichBlock::paragraph(spans)])
}

impl prism_core::Render for RichTextInput {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.hook(cx);
        self.sync_keys();
        let t = Theme::of(cx);
        let label = if self.label.is_empty() {
            self.placeholder.clone()
        } else {
            self.label.clone()
        };
        let empty = self.value.is_empty();
        let single = (self.selection.anchor.block == self.selection.head.block)
            .then_some(self.selection.head.block);
        let mut number = 0;
        let blocks = self.value.blocks.len();
        let mut rows = Vec::with_capacity(blocks);
        for (i, block) in self.value.blocks.iter().enumerate() {
            let key = self.keys[i];
            number = if block.kind == BlockKind::Numbered {
                number + 1
            } else {
                0
            };
            let size = match block.kind {
                BlockKind::Heading(1) => (t.font_size * 1.6).round(),
                BlockKind::Heading(2) => (t.font_size * 1.35).round(),
                BlockKind::Heading(_) => (t.font_size * 1.15).round(),
                _ => t.font_size,
            };
            let hint = match block.kind {
                BlockKind::Heading(level) => {
                    cx.chrome_text("heading").replace("{n}", &level.to_string())
                }
                BlockKind::Bullet => cx.chrome_text("list_item"),
                BlockKind::Numbered => cx
                    .chrome_text("numbered_item")
                    .replace("{n}", &number.to_string()),
                BlockKind::Paragraph => String::new(),
            };
            let mut field = rich_input(block.spans.clone())
                .id(self.field_id(key))
                .text_size(size)
                .text_color(t.text)
                .p(0.)
                .border(0.)
                .bg(Color::TRANSPARENT)
                .flex_1()
                .accessibility_label(label.clone())
                .when(!hint.is_empty(), |f| f.accessibility_hint(hint))
                .when(empty && i == 0, |f| f.placeholder(self.placeholder.clone()))
                .on_input(cx.listener(move |this, e: &RichInputEvent, cx| this.input(key, e, cx)))
                .on_select(cx.listener(move |this, e: &SelectionEvent, _| this.select_in(key, *e)))
                .on_focus_change(
                    cx.listener(move |this, e: &FocusEvent, _| this.focus_change(key, e)),
                )
                .on_key_down(cx.listener(move |this, e: &KeyEvent, cx| this.key(key, e, cx)))
                .consume_keys(&["cmd+b", "cmd+i", "cmd+k"])
                .on_submit(cx.listener(move |this, _, cx| this.submit(key, cx)))
                .on_paste(cx.listener(move |this, e: &PasteEvent, cx| this.paste_in(key, e, cx)))
                .on_copy(cx.listener(move |this, e: &CopyEvent, _| this.copy(key, e)))
                .on_cut(cx.listener(move |this, e: &CopyEvent, _| this.copy(key, e)));
            // At a block's edges the arrows move to the next one: the field
            // does not also move its caret.
            if i > 0 {
                field = field
                    .consume_key_at("ArrowUp", CaretEdge::FirstLine)
                    .consume_key_at("ArrowLeft", CaretEdge::Start);
            }
            if i + 1 < blocks {
                field = field
                    .consume_key_at("ArrowDown", CaretEdge::LastLine)
                    .consume_key_at("ArrowRight", CaretEdge::End);
            }
            if matches!(block.kind, BlockKind::Heading(_)) {
                field = field.font_weight(FontWeight::Bold);
            }
            if single == Some(i) {
                let (a, b) = (self.selection.anchor.offset, self.selection.head.offset);
                field = field.selection(SelectionEvent::new(a.min(b), a.max(b)));
            }
            let marker = match block.kind {
                BlockKind::Bullet => Some("•".to_string()),
                BlockKind::Numbered => Some(format!("{number}.")),
                _ => None,
            };
            let row = match marker {
                Some(m) => div()
                    .flex_row()
                    .items_start()
                    .gap(4.)
                    .child(
                        text(m)
                            .text_size(size)
                            .text_color(t.muted)
                            .min_w(px((size * 1.4).round()))
                            .accessibility_hidden(),
                    )
                    .child(field)
                    .into_element(),
                None => field.into_element(),
            };
            rows.push(row);
        }
        let frame = |s: Style| -> Style {
            s.bg(t.surface)
                .border(1.)
                .border_color(t.border)
                .rounded(t.radius)
                .px(10.)
                .py(8.)
                .focus_within(move |s| s.border_color(t.accent))
        };
        let container = if self.inline {
            div().children(rows)
        } else {
            text_flow()
                .gap((t.font_size * 0.5).round())
                .children(rows)
                .on_selection_across(cx.listener(|this, e: &SelectionAcross, _| this.across(e)))
                .on_edit_across(cx.listener(|this, e: &EditAcross, cx| this.edit_across(e, cx)))
        };
        let refine = self.style.clone();
        container
            .with_style(move |style| {
                let framed = frame(std::mem::take(style));
                *style = match &refine {
                    Some(f) => f(framed),
                    None => framed,
                };
            })
            .accessibility_role(Role::Group)
            .accessibility_label(label)
    }
}
