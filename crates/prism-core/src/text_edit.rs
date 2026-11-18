//! Editing hooks of a [`rich_input`](crate::rich_input) (#217): where the
//! selection is on screen, what copy, cut and paste carry, and
//! modifier-clicks on its spans.
//!
//! - [`RichInput::on_selection_rect`](crate::RichInput::on_selection_rect)
//!   (and [`App::selection_rect`](crate::App::selection_rect)): the
//!   rectangle of the caret or the selection, to place a formatting toolbar
//!   or a link popover above the selected words.
//! - [`on_copy`](crate::RichInput::on_copy), [`on_cut`](crate::RichInput::on_cut)
//!   and [`on_paste`](crate::RichInput::on_paste): ⌘C, ⌘X and ⌘V (the Edit
//!   menu, the context menu, the keyboard) reach the app, which writes its
//!   own content ([`ClipboardContent`]: plain text with HTML, Markdown or
//!   its own types) or takes a paste over.
//! - [`on_span_click`](crate::RichInput::on_span_click): a click with ⌘,
//!   Ctrl or ⌥ on a span while the field is being edited (open a link).
//!
//! Handlers made with `cx.listener` return nothing, so a copy handler
//! answers with [`CopyEvent::write`] and a paste handler with
//! [`PasteEvent::prevent_default`], like the DOM's events.

use std::cell::{Cell, RefCell};

use crate::SelectionEvent;
use crate::events::{Modifiers, Rect};
use crate::spans::Span;

/// What a copy or a cut writes to the clipboard (#217): plain text, and
/// optionally HTML, Markdown and the app's own types, all of one item.
/// Other apps take the richest type they read; the app reads its own type
/// back in [`PasteEvent::data`].
///
/// ```
/// # use prism_core::ClipboardContent;
/// let content = ClipboardContent::text("Hello world")
///     .html("<p>Hello <b>world</b></p>")
///     .markdown("Hello **world**")
///     .custom("application/x-notes-blocks", r#"[{"p":"Hello world"}]"#);
/// assert_eq!(
///     content.types(),
///     ["text/plain", "text/html", "text/markdown", "application/x-notes-blocks"]
/// );
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipboardContent {
    text: String,
    html: Option<String>,
    markdown: Option<String>,
    custom: Vec<(String, String)>,
}

/// The MIME type of plain text.
pub const TEXT_PLAIN: &str = "text/plain";
/// The MIME type of HTML.
pub const TEXT_HTML: &str = "text/html";
/// The MIME type of Markdown.
pub const TEXT_MARKDOWN: &str = "text/markdown";

impl ClipboardContent {
    /// Plain text (every app reads it).
    pub fn text(text: impl Into<String>) -> Self {
        ClipboardContent {
            text: text.into(),
            ..Default::default()
        }
    }

    /// Adds the same content as HTML (`text/html`): rich text editors,
    /// mail and browsers paste it with its formatting.
    pub fn html(mut self, html: impl Into<String>) -> Self {
        self.html = Some(html.into());
        self
    }

    /// Adds the same content as Markdown (`text/markdown`;
    /// `net.daringfireball.markdown` on Apple platforms).
    pub fn markdown(mut self, markdown: impl Into<String>) -> Self {
        self.markdown = Some(markdown.into());
        self
    }

    /// Adds a type of the app's own (`application/x-notes-blocks`), as
    /// text (JSON, say): read back by the app when it pastes. Natively the
    /// type is used as it is (a pasteboard type on Apple platforms); the
    /// web writes it as a custom format (`web application/x-…`) where the
    /// browser has them. `text/plain`, `text/html` and `text/markdown` set
    /// those types instead.
    pub fn custom(mut self, mime: impl Into<String>, data: impl Into<String>) -> Self {
        let mime = mime.into();
        let data = data.into();
        match mime.as_str() {
            TEXT_PLAIN => self.text = data,
            TEXT_HTML => self.html = Some(data),
            TEXT_MARKDOWN => self.markdown = Some(data),
            _ => {
                self.custom.retain(|(m, _)| *m != mime);
                self.custom.push((mime, data));
            }
        }
        self
    }

    /// The plain text.
    pub fn plain_text(&self) -> &str {
        &self.text
    }

    /// The HTML, if any.
    pub fn html_text(&self) -> Option<&str> {
        self.html.as_deref()
    }

    /// The Markdown, if any.
    pub fn markdown_text(&self) -> Option<&str> {
        self.markdown.as_deref()
    }

    /// The data of type `mime` (`text/plain` included), if any.
    pub fn get(&self, mime: &str) -> Option<&str> {
        match mime {
            TEXT_PLAIN => Some(&self.text),
            TEXT_HTML => self.html.as_deref(),
            TEXT_MARKDOWN => self.markdown.as_deref(),
            _ => self
                .custom
                .iter()
                .find(|(m, _)| m == mime)
                .map(|(_, d)| d.as_str()),
        }
    }

    /// Every (type, data) pair: plain text first, then HTML, Markdown and
    /// the custom types in the order they were added.
    pub fn entries(&self) -> Vec<(&str, &str)> {
        let mut out = vec![(TEXT_PLAIN, self.text.as_str())];
        if let Some(h) = &self.html {
            out.push((TEXT_HTML, h));
        }
        if let Some(m) = &self.markdown {
            out.push((TEXT_MARKDOWN, m));
        }
        out.extend(self.custom.iter().map(|(m, d)| (m.as_str(), d.as_str())));
        out
    }

    /// The types it holds, in [`entries`](Self::entries) order.
    pub fn types(&self) -> Vec<&str> {
        self.entries().into_iter().map(|(m, _)| m).collect()
    }

    /// The JSON form, for hosts that receive it as text:
    /// `[["text/plain","…"],["text/html","…"]]`.
    pub fn to_json(&self) -> String {
        let pairs: Vec<[&str; 2]> = self.entries().into_iter().map(|(m, d)| [m, d]).collect();
        serde_json::to_string(&pairs).unwrap_or_else(|_| "[]".into())
    }
}

/// The user copies (⌘C) or cuts (⌘X) in a rich input that has
/// [`on_copy`](crate::RichInput::on_copy) or
/// [`on_cut`](crate::RichInput::on_cut) (#217). Call
/// [`write`](Self::write) to put the app's own content on the clipboard;
/// without it the selection's plain text is copied, as usual. A cut then
/// deletes the selection either way (the field reports it with `on_input`).
#[derive(Debug)]
#[non_exhaustive]
pub struct CopyEvent {
    /// What is selected, in characters of the value. A copy with nothing
    /// selected is not reported.
    pub selection: SelectionEvent,
    /// The selection's plain text, as the field would copy it (a chip as
    /// its label).
    pub text: String,
    /// The runs of the selection, cut out of the spans last rendered: turn
    /// them into the HTML or Markdown to write.
    pub spans: Vec<Span>,
    /// A cut (⌘X) rather than a copy.
    pub cut: bool,
    /// The selection spans several fields of a
    /// [`selection_group`](crate::Div::selection_group) (#231): every
    /// field's part, in order. [`selection`](Self::selection) and
    /// [`spans`](Self::spans) are then the part of the field this handler
    /// belongs to (where the selection started), and [`text`](Self::text)
    /// the whole range's text, its fields joined by line breaks. A cut
    /// deletes nothing itself: the app does it in
    /// [`on_edit_across`](crate::Div::on_edit_across).
    pub across: Option<crate::selection_across::SelectionAcross>,
    content: RefCell<Option<ClipboardContent>>,
}

impl CopyEvent {
    /// The event for `selection` of a field showing `spans`
    /// (`text`: the selection's plain text).
    pub fn new(
        selection: SelectionEvent,
        text: impl Into<String>,
        spans: &[Span],
        cut: bool,
    ) -> Self {
        let (_, rest) = crate::spans::split_spans(spans, selection.start);
        let (mid, _) = crate::spans::split_spans(&rest, selection.end - selection.start);
        let mut text = text.into();
        // A host that copies the value's chips (U+FFFC) as they are: their
        // labels instead.
        if text.contains(crate::OBJECT_CHAR) && crate::spans::spans_text(&mid) == text {
            text = mid
                .iter()
                .map(|s| s.chip_label().unwrap_or(s.text()))
                .collect();
        }
        CopyEvent {
            selection,
            text,
            spans: mid,
            cut,
            across: None,
            content: RefCell::new(None),
        }
    }

    /// The same event for a selection across fields (#231): `text` becomes
    /// the whole range's.
    pub fn with_across(mut self, across: crate::selection_across::SelectionAcross) -> Self {
        self.text = across.text();
        self.across = Some(across);
        self
    }

    /// Writes `content` to the clipboard instead of the selection's plain
    /// text. The last call wins.
    pub fn write(&self, content: ClipboardContent) {
        *self.content.borrow_mut() = Some(content);
    }

    /// What a handler wrote ([`write`](Self::write)), if anything.
    pub fn written(&self) -> Option<ClipboardContent> {
        self.content.borrow().clone()
    }
}

/// The user pastes (⌘V) in a rich input that has
/// [`on_paste`](crate::RichInput::on_paste) (#217): what the clipboard
/// holds. Call [`prevent_default`](Self::prevent_default) to take the paste
/// over (insert blocks of your own, upload an image); otherwise the plain
/// text is inserted over the selection as usual.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct PasteEvent {
    /// The selection the paste replaces, in characters (a caret: where it
    /// lands).
    pub selection: SelectionEvent,
    /// The clipboard's plain text (empty when it holds none).
    pub text: String,
    /// Its HTML, where the platform gives it.
    pub html: Option<String>,
    /// The types the clipboard holds (MIME types, `text/plain` and
    /// `text/html` included; Apple pasteboard types as they are, besides
    /// the MIME types of the text ones), for an app choosing what to read.
    pub types: Vec<String>,
    /// Text data of the types the app may want besides these: Markdown and
    /// custom types written by [`ClipboardContent::custom`], where the
    /// platform hands them over (macOS, iOS, the web).
    pub data: Vec<(String, String)>,
    prevented: Cell<bool>,
}

impl PasteEvent {
    /// A paste of `text` over `selection`.
    pub fn new(selection: SelectionEvent, text: impl Into<String>) -> Self {
        PasteEvent {
            selection,
            text: text.into(),
            types: vec![TEXT_PLAIN.to_string()],
            ..Default::default()
        }
    }

    /// With its HTML.
    pub fn with_html(mut self, html: Option<String>) -> Self {
        if html.is_some() && !self.types.iter().any(|t| t == TEXT_HTML) {
            self.types.push(TEXT_HTML.to_string());
        }
        self.html = html;
        self
    }

    /// With the clipboard's types.
    pub fn with_types(mut self, types: Vec<String>) -> Self {
        self.types = types;
        self
    }

    /// With text data of other types (Markdown, the app's own).
    pub fn with_data(mut self, data: Vec<(String, String)>) -> Self {
        self.data = data;
        self
    }

    /// The paste a host sent as JSON: `{"start","end","text","html",
    /// "types":[…],"data":[[type, text]…]}` (the web client, Android).
    pub fn from_json(v: &serde_json::Value) -> Option<Self> {
        use serde_json::Value;
        let at = |k: &str| v.get(k).and_then(Value::as_u64).map(|n| n as usize);
        let types = v
            .get("types")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let data = v
            .get("data")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|pair| {
                        let pair = pair.as_array()?;
                        Some((
                            pair.first()?.as_str()?.to_string(),
                            pair.get(1)?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(
            PasteEvent::new(
                SelectionEvent::new(at("start")?, at("end")?),
                v.get("text").and_then(Value::as_str).unwrap_or(""),
            )
            .with_html(v.get("html").and_then(Value::as_str).map(str::to_string))
            .with_types(types)
            .with_data(data),
        )
    }

    /// The text data of type `mime`: `text/plain`, `text/html` or one of
    /// [`data`](Self::data).
    pub fn get(&self, mime: &str) -> Option<&str> {
        match mime {
            TEXT_PLAIN => Some(&self.text),
            TEXT_HTML => self.html.as_deref(),
            _ => self
                .data
                .iter()
                .find(|(m, _)| m == mime)
                .map(|(_, d)| d.as_str()),
        }
    }

    /// Whether the clipboard holds type `mime`.
    pub fn has(&self, mime: &str) -> bool {
        self.types.iter().any(|t| t == mime) || self.get(mime).is_some_and(|d| !d.is_empty())
    }

    /// The app handles the paste: nothing is inserted.
    pub fn prevent_default(&self) {
        self.prevented.set(true);
    }

    /// Whether a handler called [`prevent_default`](Self::prevent_default).
    pub fn is_default_prevented(&self) -> bool {
        self.prevented.get()
    }
}

/// A click with ⌘, Ctrl or ⌥ on a span of a rich input being edited
/// (#217): open a link, follow a mention. See
/// [`on_span_click`](crate::RichInput::on_span_click).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SpanClickEvent {
    /// The span clicked: its index in the spans given to
    /// [`rich_input`](crate::rich_input) at the last render.
    pub span: usize,
    /// The character clicked, in the value.
    pub offset: usize,
    /// The keys held.
    pub modifiers: Modifiers,
}

impl SpanClickEvent {
    /// A click on character `offset` of span `span`.
    pub fn new(span: usize, offset: usize, modifiers: Modifiers) -> Self {
        SpanClickEvent {
            span,
            offset,
            modifiers,
        }
    }
}

/// Where the caret or the selection of a rich input is (#217), to place a
/// formatting toolbar or a link popover next to the selected words. See
/// [`on_selection_rect`](crate::RichInput::on_selection_rect).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct SelectionRectEvent {
    /// The selection it bounds, in characters.
    pub selection: SelectionEvent,
    /// The rectangle in the field's own coordinates (its top-left corner
    /// is `(0, 0)`, scrolling inside the field included): the caret, one
    /// point wide, or the box around the selected text (on several lines,
    /// the box around all of them). Inside a container laid out at the
    /// field's origin, an absolutely positioned element at this rectangle
    /// is an anchor for a [`popover`](crate::popover).
    pub rect: Rect,
    /// The same rectangle in the window's layout coordinates, like
    /// [`App::frame_of`](crate::App::frame_of): the field's frame plus
    /// [`rect`](Self::rect). `None` until the field has been laid out.
    pub frame: Option<Rect>,
}

impl SelectionRectEvent {
    /// The rectangle `rect` (in the field) of `selection`.
    pub fn new(selection: SelectionEvent, rect: Rect) -> Self {
        SelectionRectEvent {
            selection,
            rect,
            frame: None,
        }
    }
}

/// Which editing events a rich input reports (`Patch::SetEditEvents`):
/// bits of a mask.
pub mod edit_events {
    /// Its selection's rectangle (`UiEvent::SelectionRect`).
    pub const SELECTION_RECT: u8 = 1;
    /// Copies (`UiEvent::Copy` with `cut == false`): the backend writes
    /// nothing itself.
    pub const COPY: u8 = 2;
    /// Cuts (`UiEvent::Copy` with `cut == true`): the backend writes
    /// nothing, then deletes the selection.
    pub const CUT: u8 = 4;
    /// Pastes (`UiEvent::Paste`): the backend inserts nothing itself; the
    /// runtime answers with `Patch::InsertText` unless the app takes it.
    pub const PASTE: u8 = 8;
    /// Clicks with ⌘, Ctrl or ⌥ (`UiEvent::SpanClick`).
    pub const SPAN_CLICK: u8 = 16;
    /// Every click, with the character under it (`UiEvent::FieldClick`,
    /// #266); the field still places its caret.
    pub const CLICK: u8 = 32;
}

/// The handlers of a rich input's editing events (#217).
#[derive(Clone, Default)]
pub struct EditHandlers {
    /// [`RichInput::on_selection_rect`](crate::RichInput::on_selection_rect).
    pub on_selection_rect: Option<crate::Handler<SelectionRectEvent>>,
    /// [`RichInput::track_selection_rect`](crate::RichInput::track_selection_rect).
    pub track_selection_rect: bool,
    /// [`RichInput::on_copy`](crate::RichInput::on_copy).
    pub on_copy: Option<crate::Handler<CopyEvent>>,
    /// [`RichInput::on_cut`](crate::RichInput::on_cut).
    pub on_cut: Option<crate::Handler<CopyEvent>>,
    /// [`RichInput::on_paste`](crate::RichInput::on_paste).
    pub on_paste: Option<crate::Handler<PasteEvent>>,
    /// [`RichInput::on_span_click`](crate::RichInput::on_span_click).
    pub on_span_click: Option<crate::Handler<SpanClickEvent>>,
    /// [`RichInput::on_click`](crate::RichInput::on_click) (#266).
    pub on_click: Option<crate::Handler<crate::ClickEvent>>,
}

impl EditHandlers {
    /// The [`edit_events`] mask backends need.
    pub fn mask(&self) -> u8 {
        use edit_events::*;
        let bit = |on: bool, b: u8| if on { b } else { 0 };
        bit(
            self.on_selection_rect.is_some() || self.track_selection_rect,
            SELECTION_RECT,
        ) | bit(self.on_copy.is_some(), COPY)
            | bit(self.on_cut.is_some(), CUT)
            | bit(self.on_paste.is_some(), PASTE)
            | bit(self.on_span_click.is_some(), SPAN_CLICK)
            | bit(self.on_click.is_some(), CLICK)
    }

    /// Whether no handler is set.
    pub fn is_empty(&self) -> bool {
        self.mask() == 0
    }
}

impl std::fmt::Debug for EditHandlers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EditHandlers({:#x})", self.mask())
    }
}

/// The index of the span holding character `offset` of `spans` (the last
/// one for an offset at the end), or `None` without spans.
pub fn span_at(spans: &[Span], offset: usize) -> Option<usize> {
    let mut at = 0;
    let mut last = None;
    for (i, s) in spans.iter().enumerate() {
        let len = s.len();
        if len == 0 {
            continue;
        }
        if offset < at + len {
            return Some(i);
        }
        at += len;
        last = Some(i);
    }
    last
}
