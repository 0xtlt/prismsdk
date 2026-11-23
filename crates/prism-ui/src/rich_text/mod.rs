//! A rich text editor (#294): paragraphs, headings and lists of runs in
//! bold, italic and links, written without seeing any markup.
//!
//! [`RichText`] is the document the app stores (or converts to its own
//! format: [`RichText::to_markdown`], [`RichText::to_html`] and back), and
//! [`RichTextInput`] the editor, an entity the app creates once:
//!
//! ```ignore
//! struct Page { body: Entity<RichTextInput> }
//!
//! // Once, when the page is built:
//! let body = cx.new(|_| {
//!     ui::rich_text_input(RichText::from_markdown(&note.body))
//!         .placeholder("Write something…")
//!         .label("Body")
//! });
//! // Store each edit (and redraw a toolbar on each selection change).
//! cx.observe(&body, |this: &mut Page, body, cx| {
//!     this.note.body = body.read(cx).value().to_markdown();
//! });
//!
//! // In `render`: a toolbar that shows and changes the selection's marks.
//! let f = self.body.read(cx).formatting();
//! let pressed = |on| if on { ui::Variant::Primary } else { ui::Variant::Ghost };
//! div()
//!     .child(ui::button("B").variant(pressed(f.bold)).on_click(cx.listener(|this, _, cx| {
//!         this.body.update(cx, |e, cx| e.toggle_bold(cx))
//!     })))
//!     .child(self.body.clone())
//! ```
//!
//! The editor is made of Prism's own fields: one
//! [`rich_input`](prism_core::rich_input) per block, in a
//! [`text_flow`](prism_core::text_flow) so the selection runs across
//! blocks. So it edits natively everywhere `rich_input` does: a
//! `contenteditable` on the web, `NSTextView` on macOS, `UITextView` on
//! iOS, iPadOS and visionOS, `GtkTextView`, `EditText` and `RichEditBox`
//! on Linux, Android and Windows.

mod editor;
mod html;
mod markdown;
#[cfg(test)]
mod tests;

pub use editor::{
    Formatting, LinkRequest, PasteMode, RICH_TEXT_TYPE, RichTextChange, RichTextInput,
    rich_text_input,
};

use prism_core::{Span, normalize_spans, restyle_spans, span, spans_text, split_spans};

/// What a block of a [`RichText`] is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlockKind {
    /// A paragraph.
    #[default]
    Paragraph,
    /// A heading, level 1 to 3 ([`BlockKind::heading`] clamps it).
    Heading(u8),
    /// An item of a bulleted list.
    Bullet,
    /// An item of a numbered list (numbered from 1 in each run of them).
    Numbered,
}

impl BlockKind {
    /// A heading of `level`, kept within 1 to 3.
    pub fn heading(level: u8) -> Self {
        BlockKind::Heading(level.clamp(1, 3))
    }

    /// Whether it is an item of a list.
    pub fn is_list(self) -> bool {
        matches!(self, BlockKind::Bullet | BlockKind::Numbered)
    }

    /// A heading's level (1 to 3).
    pub fn heading_level(self) -> Option<u8> {
        match self {
            BlockKind::Heading(level) => Some(level.clamp(1, 3)),
            _ => None,
        }
    }

    /// The same kind with a valid heading level.
    fn clamped(self) -> Self {
        match self {
            BlockKind::Heading(level) => BlockKind::heading(level),
            k => k,
        }
    }
}

/// One block of a [`RichText`]: its kind and its runs
/// ([`Span`]s: bold, italic, links; any other style the app gives a run
/// is kept as it is).
#[derive(Clone, Debug, Default)]
pub struct RichBlock {
    /// Paragraph, heading or list item.
    pub kind: BlockKind,
    /// The runs, [normalized](prism_core::normalize_spans).
    pub spans: Vec<Span>,
}

impl RichBlock {
    /// A block of `kind` made of `spans`.
    pub fn new(kind: BlockKind, spans: impl IntoIterator<Item = Span>) -> Self {
        RichBlock {
            kind: kind.clamped(),
            spans: normalize_spans(spans),
        }
    }

    /// A paragraph.
    pub fn paragraph(spans: impl IntoIterator<Item = Span>) -> Self {
        RichBlock::new(BlockKind::Paragraph, spans)
    }

    /// A heading of `level` (1 to 3).
    pub fn heading(level: u8, spans: impl IntoIterator<Item = Span>) -> Self {
        RichBlock::new(BlockKind::heading(level), spans)
    }

    /// An item of a bulleted list.
    pub fn bullet(spans: impl IntoIterator<Item = Span>) -> Self {
        RichBlock::new(BlockKind::Bullet, spans)
    }

    /// An item of a numbered list.
    pub fn numbered(spans: impl IntoIterator<Item = Span>) -> Self {
        RichBlock::new(BlockKind::Numbered, spans)
    }

    /// Its plain text.
    pub fn text(&self) -> String {
        spans_text(&self.spans)
    }

    /// Its length in characters (what selections count).
    pub fn len(&self) -> usize {
        self.spans.iter().map(Span::len).sum()
    }

    /// Whether it holds no text.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl PartialEq for RichBlock {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && same_spans(&self.spans, &other.spans)
    }
}

/// Whether two runs hold the same texts in the same styles.
pub(crate) fn same_spans(a: &[Span], b: &[Span]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.text() == y.text()
                && (x.same_style(y)
                    || (x.is_object()
                        && y.is_object()
                        && x.chip_key() == y.chip_key()
                        && x.icon_name() == y.icon_name()))
        })
}

/// A formatted text: paragraphs, headings and list items (#294), the
/// value of a [`RichTextInput`].
///
/// ```
/// use prism_ui::{BlockKind, RichText};
/// let doc = RichText::from_markdown("## Plan\n\nShip **today**.\n\n- tests\n- docs");
/// assert_eq!(doc.blocks[0].kind, BlockKind::Heading(2));
/// assert!(doc.blocks[1].spans[1].is_bold());
/// assert_eq!(doc.to_markdown(), "## Plan\n\nShip **today**.\n\n- tests\n- docs");
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RichText {
    /// The blocks, in order. An empty document has none (an editor shows
    /// one empty paragraph).
    pub blocks: Vec<RichBlock>,
}

impl RichText {
    /// A document of `blocks`.
    pub fn new(blocks: impl IntoIterator<Item = RichBlock>) -> Self {
        RichText {
            blocks: blocks.into_iter().collect(),
        }
    }

    /// Plain text, one paragraph per line.
    pub fn plain(text: &str) -> Self {
        if text.is_empty() {
            return RichText::default();
        }
        RichText::new(
            text.split('\n')
                .map(|line| RichBlock::paragraph([span(line.trim_end_matches('\r'))])),
        )
    }

    /// Its plain text, one line per block.
    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .map(RichBlock::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Whether it holds no text (no block, or only empty paragraphs).
    pub fn is_empty(&self) -> bool {
        self.blocks
            .iter()
            .all(|b| b.is_empty() && b.kind == BlockKind::Paragraph)
    }

    /// The document as Markdown: `#` headings, `-` and `1.` lists,
    /// `**bold**`, `*italic*`, `[links](…)`; characters Markdown would
    /// read as markup are escaped. [`from_markdown`](Self::from_markdown)
    /// reads it back.
    pub fn to_markdown(&self) -> String {
        markdown::write(self)
    }

    /// Reads Markdown: ATX headings (`#` to `###`; deeper ones become
    /// level 3), `-`, `*`, `+` and `1.` lists, paragraphs (lines joined),
    /// `**bold**` / `__bold__`, `*italic*` / `_italic_`, `[text](url)`
    /// links, backslash escapes. Other syntax is kept as text. A link to a
    /// `javascript:`, `data:` or `vbscript:` URL keeps its text and loses
    /// its target.
    pub fn from_markdown(markdown: &str) -> Self {
        markdown::read(markdown)
    }

    /// The document as HTML: `<p>`, `<h1>`–`<h3>`, `<ul>` / `<ol>` of
    /// `<li>`, `<strong>`, `<em>`, `<a href>`, text escaped.
    pub fn to_html(&self) -> String {
        html::write(self)
    }

    /// Reads HTML into the model's subset (a paste from a browser, a word
    /// processor, a mail): headings, paragraphs, list items, bold
    /// (`<b>`, `<strong>`, `font-weight` 600 and up), italic (`<i>`,
    /// `<em>`, `font-style: italic`) and links; every other tag gives its
    /// text, `<script>`, `<style>` and `<template>` nothing. Links keep
    /// only safe targets (no `javascript:`, `data:` or `vbscript:`).
    pub fn from_html(html: &str) -> Self {
        html::read(html)
    }

    /// The part of the document between `start` and `end`, as a document.
    pub fn slice(&self, start: RichPosition, end: RichPosition) -> RichText {
        let (start, end) = (self.clamp(start.min(end)), self.clamp(start.max(end)));
        if self.blocks.is_empty() {
            return RichText::default();
        }
        let mut out = Vec::new();
        for i in start.block..=end.block {
            let b = &self.blocks[i];
            let from = if i == start.block { start.offset } else { 0 };
            let to = if i == end.block { end.offset } else { b.len() };
            let (_, rest) = split_spans(&b.spans, from);
            let (mid, _) = split_spans(&rest, to.saturating_sub(from));
            out.push(RichBlock::new(b.kind, mid));
        }
        RichText::new(out)
    }

    /// `pos` within the document (an empty one: its start).
    pub(crate) fn clamp(&self, pos: RichPosition) -> RichPosition {
        let Some(last) = self.blocks.len().checked_sub(1) else {
            return RichPosition::default();
        };
        let block = pos.block.min(last);
        RichPosition {
            block,
            offset: pos.offset.min(self.blocks[block].len()),
        }
    }

    /// At least one block (an editor edits one empty paragraph).
    pub(crate) fn ensure_block(&mut self) {
        if self.blocks.is_empty() {
            self.blocks.push(RichBlock::default());
        }
    }

    /// Replaces `start..end` with `fragment`; returns the position right
    /// after what was inserted, and which blocks the new ones replaced
    /// (the first of them keeps its place). A fragment of several blocks
    /// splits the block at `start`: its first block joins the text before,
    /// its last one the text after.
    pub(crate) fn replace(
        &mut self,
        start: RichPosition,
        end: RichPosition,
        fragment: &RichText,
    ) -> (RichPosition, std::ops::RangeInclusive<usize>, usize) {
        self.ensure_block();
        let (start, end) = (self.clamp(start.min(end)), self.clamp(start.max(end)));
        let first = &self.blocks[start.block];
        let (head, _) = split_spans(&first.spans, start.offset);
        let (_, tail) = split_spans(&self.blocks[end.block].spans, end.offset);
        let kind = first.kind;
        let frag = &fragment.blocks;
        let (blocks, caret) = match frag.len() {
            0 | 1 => {
                let insert = frag.first().map(|b| b.spans.clone()).unwrap_or_default();
                let len: usize = insert.iter().map(Span::len).sum();
                // An empty block takes the kind of the one block pasted in.
                let kind = match frag.first() {
                    Some(b)
                        if head.is_empty() && tail.is_empty() && kind == BlockKind::Paragraph =>
                    {
                        b.kind
                    }
                    _ => kind,
                };
                let block = RichBlock::new(kind, head.into_iter().chain(insert).chain(tail));
                (
                    vec![block],
                    RichPosition {
                        block: start.block,
                        offset: start.offset + len,
                    },
                )
            }
            n => {
                let first_kind = if head.is_empty() && kind == BlockKind::Paragraph {
                    frag[0].kind
                } else {
                    kind
                };
                let mut blocks = vec![RichBlock::new(
                    first_kind,
                    head.into_iter().chain(frag[0].spans.iter().cloned()),
                )];
                blocks.extend(frag[1..n - 1].iter().cloned());
                let last = &frag[n - 1];
                blocks.push(RichBlock::new(
                    last.kind,
                    last.spans.iter().cloned().chain(tail),
                ));
                (
                    blocks,
                    RichPosition {
                        block: start.block + n - 1,
                        offset: last.len(),
                    },
                )
            }
        };
        let count = blocks.len();
        self.blocks.splice(start.block..=end.block, blocks);
        (caret, start.block..=end.block, count)
    }

    /// Splits the block at `at` (Enter); returns where the caret goes and
    /// the index of the block inserted. An empty list item leaves the list
    /// instead (it becomes a paragraph); at the start of a block, an empty
    /// block goes above it; the text after a heading's end is a paragraph.
    pub(crate) fn split(&mut self, at: RichPosition) -> (RichPosition, Option<usize>) {
        self.ensure_block();
        let at = self.clamp(at);
        let block = &mut self.blocks[at.block];
        if block.kind.is_list() && block.is_empty() {
            block.kind = BlockKind::Paragraph;
            return (at, None);
        }
        let kind = block.kind;
        let (left, right) = split_spans(&block.spans, at.offset);
        if left.is_empty() && !right.is_empty() {
            let above = if kind.is_list() {
                kind
            } else {
                BlockKind::Paragraph
            };
            self.blocks.insert(at.block, RichBlock::new(above, []));
            return (RichPosition::new(at.block + 1, 0), Some(at.block));
        }
        let next = match kind {
            BlockKind::Heading(_) if right.is_empty() => BlockKind::Paragraph,
            k => k,
        };
        block.spans = left;
        self.blocks
            .insert(at.block + 1, RichBlock::new(next, right));
        (RichPosition::new(at.block + 1, 0), Some(at.block + 1))
    }

    /// Backspace at the start of block `i`: a heading or a list item
    /// becomes a paragraph; a paragraph joins the block before. Returns
    /// where the caret goes and the index of the block removed, or `None`
    /// (the first paragraph).
    pub(crate) fn backspace_at_start(&mut self, i: usize) -> Option<(RichPosition, Option<usize>)> {
        let block = self.blocks.get_mut(i)?;
        if block.kind != BlockKind::Paragraph {
            block.kind = BlockKind::Paragraph;
            return Some((RichPosition::new(i, 0), None));
        }
        if i == 0 {
            return None;
        }
        Some((self.join(i - 1), Some(i)))
    }

    /// Delete at the end of block `i`: the next block joins it (it is the
    /// one removed).
    pub(crate) fn delete_at_end(&mut self, i: usize) -> Option<RichPosition> {
        (i + 1 < self.blocks.len()).then(|| self.join(i))
    }

    /// Joins block `i + 1` to block `i` (an empty block `i` takes the
    /// other's kind); returns the caret at the seam.
    fn join(&mut self, i: usize) -> RichPosition {
        let next = self.blocks.remove(i + 1);
        let block = &mut self.blocks[i];
        let offset = block.len();
        if block.is_empty() {
            block.kind = next.kind;
        }
        block.spans = normalize_spans(block.spans.drain(..).chain(next.spans));
        RichPosition { block: i, offset }
    }

    /// Restyles the runs between `start` and `end` with `f`.
    pub(crate) fn restyle(
        &mut self,
        start: RichPosition,
        end: RichPosition,
        f: impl Fn(Span) -> Span,
    ) {
        if self.blocks.is_empty() {
            return;
        }
        let (start, end) = (self.clamp(start.min(end)), self.clamp(start.max(end)));
        for i in start.block..=end.block {
            let b = &mut self.blocks[i];
            let from = if i == start.block { start.offset } else { 0 };
            let to = if i == end.block { end.offset } else { b.len() };
            if from < to {
                b.spans = restyle_spans(&b.spans, from..to, &f);
            }
        }
    }

    /// The runs between `start` and `end` (each cut at the range's ends).
    fn spans_in(&self, start: RichPosition, end: RichPosition) -> Vec<Span> {
        self.slice(start, end)
            .blocks
            .into_iter()
            .flat_map(|b| b.spans)
            .collect()
    }

    /// The run a caret at `pos` types in: the one before it, or at a
    /// block's start the one after it.
    fn span_at_caret(&self, pos: RichPosition) -> Option<&Span> {
        let block = self.blocks.get(pos.block)?;
        let mut at = 0;
        let mut after = None;
        for s in &block.spans {
            let len = s.len();
            if pos.offset > at && pos.offset <= at + len {
                return Some(s);
            }
            if after.is_none() && pos.offset <= at {
                after = Some(s);
            }
            at += len;
        }
        if pos.offset == 0 { after } else { None }
    }

    /// The links of block `i`: each run of neighbours with the same
    /// target, as `(from, to, href)`.
    fn links_of(&self, i: usize) -> Vec<(usize, usize, &str)> {
        let mut out: Vec<(usize, usize, &str)> = Vec::new();
        let mut at = 0;
        for s in self.blocks.get(i).map_or(&[][..], |b| &b.spans[..]) {
            let len = s.len();
            if let Some(href) = s.href() {
                match out.last_mut() {
                    Some(last) if last.1 == at && last.2 == href => last.1 = at + len,
                    _ => out.push((at, at + len, href)),
                }
            }
            at += len;
        }
        out
    }

    /// The link under a caret at `pos`, as `(from, to, href)`: strictly
    /// inside it (at its ends, typing writes plain text).
    fn link_at_caret(&self, pos: RichPosition) -> Option<(usize, usize, &str)> {
        self.links_of(pos.block)
            .into_iter()
            .find(|(from, to, _)| *from < pos.offset && pos.offset < *to)
    }

    /// The marks, link and block kind of `sel` (a toolbar's state).
    pub(crate) fn formatting(&self, sel: RichSelection) -> (bool, bool, Option<String>, BlockKind) {
        let kind = self
            .blocks
            .get(self.clamp(sel.start()).block)
            .map_or(BlockKind::Paragraph, |b| b.kind);
        if sel.is_caret() {
            let s = self.span_at_caret(sel.head);
            return (
                s.is_some_and(Span::is_bold),
                s.is_some_and(Span::is_italic),
                self.link_at_caret(sel.head).map(|(_, _, h)| h.to_string()),
                kind,
            );
        }
        let spans: Vec<Span> = self
            .spans_in(sel.start(), sel.end())
            .into_iter()
            .filter(|s| !s.text().trim().is_empty() || s.is_object())
            .collect();
        if spans.is_empty() {
            return (false, false, None, kind);
        }
        let link = spans[0].href().map(str::to_string);
        let link = link.filter(|h| spans.iter().all(|s| s.href() == Some(h.as_str())));
        (
            spans.iter().all(Span::is_bold),
            spans.iter().all(Span::is_italic),
            link,
            kind,
        )
    }

    /// The link around a caret at `pos`, as a range.
    pub(crate) fn link_range(&self, pos: RichPosition) -> Option<(RichPosition, RichPosition)> {
        let (from, to, _) = self.link_at_caret(pos)?;
        Some((
            RichPosition::new(pos.block, from),
            RichPosition::new(pos.block, to),
        ))
    }
}

/// A place in a [`RichText`]: a block and a character offset in it (a
/// chip counts as one, like [`SelectionEvent`](prism_core::SelectionEvent)s).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RichPosition {
    /// The block's index.
    pub block: usize,
    /// Characters before it in the block.
    pub offset: usize,
}

impl RichPosition {
    /// Character `offset` of block `block`.
    pub fn new(block: usize, offset: usize) -> Self {
        RichPosition { block, offset }
    }
}

/// The selection of a [`RichTextInput`]: from `anchor` (where it started)
/// to `head` (the end that moves), possibly across blocks; a caret when
/// they are equal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RichSelection {
    /// Where the selection started.
    pub anchor: RichPosition,
    /// The end that moves (the caret).
    pub head: RichPosition,
}

impl RichSelection {
    /// A selection from `anchor` to `head`.
    pub fn new(anchor: RichPosition, head: RichPosition) -> Self {
        RichSelection { anchor, head }
    }

    /// A caret at `at`.
    pub fn caret(at: RichPosition) -> Self {
        RichSelection {
            anchor: at,
            head: at,
        }
    }

    /// The end that comes first.
    pub fn start(&self) -> RichPosition {
        self.anchor.min(self.head)
    }

    /// The end that comes last.
    pub fn end(&self) -> RichPosition {
        self.anchor.max(self.head)
    }

    /// Whether it is a caret (nothing selected).
    pub fn is_caret(&self) -> bool {
        self.anchor == self.head
    }
}
