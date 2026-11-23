//! Element builders: the declarative tree that `Render::render` returns.

use std::rc::Rc;

use crate::a11y::{Accessibility, Accessible, Role};
use crate::app::{AnyEntity, App, Entity, Render};
use crate::style::{Color, Style, Styled};

/// The kinds of host nodes every backend must implement.
///
/// Adding a kind here is the single extension point for new primitives
/// (image, scroll view, ...): each backend then maps it to a native widget.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeKind {
    /// A flex container (`<div>`, `NSView`, `UIView`).
    View,
    /// A run of text (`<div>`, `NSTextField` label, `UILabel`).
    Text,
    /// A native push button.
    Button,
    /// A single-line editable text field.
    TextInput,
    /// A navigation stack (`UINavigationController`, a header bar + top screen
    /// on macOS/web). Its children are screens; the last one is visible.
    /// Its text is the current path (URL on the web).
    Stack,
    /// A screen inside a [`Stack`](NodeKind::Stack). Its text is the title.
    Screen,
    /// A native on/off switch (`UISwitch`, `NSSwitch`, `<input role=switch>`).
    /// Its value is 0 or 1.
    Toggle,
    /// A scrollable container (`UIScrollView`, `NSScrollView`, `overflow:auto`).
    Scroll,
    /// A native tab bar (`UITabBarController`, segmented bar, ARIA tablist).
    /// Its children are [`Tab`](NodeKind::Tab)s; its value is the selected index.
    Tabs,
    /// One tab: its text is the title, its icon an SF Symbol name.
    Tab,
    /// A symbol image: its icon is an SF Symbol name (a glyph on the web).
    /// Size from `text_size`, tint from `text_color`.
    Image,
    /// A determinate progress bar, value in `0.0..=1.0`.
    Progress,
    /// A countdown to a moment (value: Unix seconds), ticking by itself —
    /// even in widgets and Live Activities, where the app is not running.
    Timer,
    /// A modal presentation (sheet) of its child stack, attached to a root
    /// stack or tab bar. Reports `UiEvent::Back(id, 0)` when dismissed by the
    /// user (swipe down, Escape).
    Modal,
    /// A bitmap image from a URL or an app asset (text = source; value 0 =
    /// fill/cover, 1 = fit/contain). Loaded asynchronously and cached.
    Picture,
    /// Vector drawing (text = [`Drawing::encode`](crate::shape::Drawing)):
    /// paths in fractions of the frame, drawn natively. No natural size.
    Shape,
    /// A native slider (`UISlider`, `NSSlider`, `<input type=range>`,
    /// `GtkScale`, `SeekBar`). Value: the current value; config:
    /// [`SliderConfig`](crate::controls::SliderConfig).
    Slider,
    /// A native menu-style picker (`UIButton` menu, `NSPopUpButton`,
    /// `<select>`, `GtkDropDown`, `Spinner`, SwiftUI `Picker`). Value: the
    /// selected index; config: [`PickerConfig`](crate::controls::PickerConfig).
    Picker,
    /// A native date and/or time picker. Value: Unix seconds; config:
    /// [`DateConfig`](crate::controls::DateConfig).
    DatePicker,
    /// A multi-line editable text (`UITextView`, `NSTextView`, `<textarea>`,
    /// `GtkTextView`, multi-line `EditText`). Grows with its content, then
    /// scrolls (cap it with `max_h`).
    TextArea,
    /// An indeterminate activity indicator. Value 1: large.
    Spinner,

    // --- Overlays and choice controls (#18, #19) ---
    /// Content presented over the app without a route: a sheet, a popover
    /// anchored to the overlay's parent, or a dialog (config:
    /// [`OverlayConfig`](crate::controls::OverlayConfig)). Value 1 while
    /// open. Takes no room in its parent; its one child is the content, laid
    /// out on its own. Reports `UiEvent::Back(id, 0)` when the user closes
    /// it (Escape, Back, a tap outside, a swipe down).
    Overlay,
    /// A native checkbox with an optional label (text). Value 0 or 1.
    Checkbox,
    /// A group of native radio buttons (config:
    /// [`RadioConfig`](crate::controls::RadioConfig)). Value: the selected
    /// index (-1: none).
    RadioGroup,

    // --- 3D ---
    /// A 3D scene drawn by the GPU (its [`VNode::scene`], sent as
    /// [`Patch::SetScene`](crate::Patch::SetScene)): SceneKit, WebGL 2,
    /// OpenGL ES 3, OpenGL. No natural size. Reports
    /// [`UiEvent::SceneTap`](crate::UiEvent::SceneTap).
    Scene3d,

    // --- Web views (#62) ---
    /// Another web page shown inside the app: an `<iframe>` on the web,
    /// `WKWebView` on Apple platforms, `android.webkit.WebView` on Android,
    /// WinUI's `WebView2` on Windows (see [`web_view`](crate::web_view())). Its text is the URL, its
    /// config a [`WebViewConfig`](crate::web_view::WebViewConfig). No
    /// natural size: give it one. Reports `UiEvent::WebMessage` and
    /// `UiEvent::WebLoad`.
    WebView,

    // --- Editable rich text (#115) ---
    /// An editable paragraph of styled runs and inline objects
    /// ([`rich_input`]): a `contenteditable` on the web, an `NSTextView`
    /// with attributes and attachments on macOS, a `UITextView` on iOS,
    /// iPadOS and visionOS, a `GtkTextView` with tags and child anchors on
    /// Linux, an `EditText` with spans on Android, a `RichEditBox` on
    /// Windows. Its text is the plain
    /// value (a chip is [`OBJECT_CHAR`](crate::OBJECT_CHAR)), its runs come
    /// in [`Patch::SetRichValue`](crate::Patch::SetRichValue), its config is
    /// an [`InputConfig`](crate::controls::InputConfig). Backends without
    /// rich editing show a text area of the plain value instead (see
    /// `prism_native::Widgets::rich_input`).
    RichInput,

    // --- Media (#207) ---
    /// A video with the platform's controls ([`video`](crate::video())):
    /// its text is the source (then one `name: value` header per line), its
    /// config a [`VideoConfig`](crate::video::VideoConfig). Its natural size
    /// is the video's. Reports `UiEvent::Ended`.
    Video,

    // --- Tables (#284) ---
    /// A table ([`table`](crate::table())): a `<table>` on the web; on
    /// native platforms a container whose children are
    /// [`TableRow`](NodeKind::TableRow)s, the first one the column headers.
    /// prism-native sizes its columns with the CSS automatic table layout
    /// (see `prism_native::table`).
    Table,
    /// A row of a table (`<tr>`): its children are cells.
    TableRow,
    /// The header cell of a column (`<th scope="col">`), in the first row.
    ColumnHeader,
    /// A cell of a table row (`<td>`): holds any element.
    TableCell,
}

impl NodeKind {
    /// Whether the node is one of a table's (#284): the table, a row or a
    /// cell. They lay out and draw as containers.
    pub fn is_table_part(self) -> bool {
        matches!(
            self,
            NodeKind::Table | NodeKind::TableRow | NodeKind::ColumnHeader | NodeKind::TableCell
        )
    }

    /// Whether the node is a table cell: a [`ColumnHeader`](Self::ColumnHeader)
    /// or a [`TableCell`](Self::TableCell).
    pub fn is_table_cell(self) -> bool {
        matches!(self, NodeKind::ColumnHeader | NodeKind::TableCell)
    }

    /// Snake-case name (`"view"`, `"text_input"`…), used in patches and by the web client.
    pub fn name(self) -> &'static str {
        match self {
            NodeKind::View => "view",
            NodeKind::Text => "text",
            NodeKind::Button => "button",
            NodeKind::TextInput => "text_input",
            NodeKind::Stack => "stack",
            NodeKind::Screen => "screen",
            NodeKind::Toggle => "toggle",
            NodeKind::Scroll => "scroll",
            NodeKind::Tabs => "tabs",
            NodeKind::Tab => "tab",
            NodeKind::Image => "image",
            NodeKind::Progress => "progress",
            NodeKind::Timer => "timer",
            NodeKind::Modal => "modal",
            NodeKind::Picture => "picture",
            NodeKind::Shape => "shape",
            NodeKind::Slider => "slider",
            NodeKind::Picker => "picker",
            NodeKind::DatePicker => "date_picker",
            NodeKind::TextArea => "text_area",
            NodeKind::Spinner => "spinner",
            NodeKind::Overlay => "overlay",
            NodeKind::Checkbox => "checkbox",
            NodeKind::RadioGroup => "radio_group",
            NodeKind::Scene3d => "scene3d",
            NodeKind::WebView => "web_view",
            NodeKind::RichInput => "rich_input",
            NodeKind::Video => "video",
            NodeKind::Table => "table",
            NodeKind::TableRow => "table_row",
            NodeKind::ColumnHeader => "column_header",
            NodeKind::TableCell => "table_cell",
        }
    }

    /// The role a node of this kind has unless overridden.
    pub fn default_role(self) -> Option<Role> {
        match self {
            NodeKind::Button => Some(Role::Button),
            NodeKind::Toggle => Some(Role::Switch),
            NodeKind::Tabs => Some(Role::TabList),
            NodeKind::Tab => Some(Role::TabPanel),
            NodeKind::Text | NodeKind::Timer => Some(Role::Text),
            NodeKind::Progress | NodeKind::Spinner => Some(Role::ProgressBar),
            NodeKind::Slider => Some(Role::Slider),
            NodeKind::Picker => Some(Role::ComboBox),
            NodeKind::Picture | NodeKind::Shape | NodeKind::Scene3d => Some(Role::Image),
            NodeKind::Checkbox => Some(Role::Checkbox),
            NodeKind::RadioGroup => Some(Role::RadioGroup),
            NodeKind::Table => Some(Role::Table),
            NodeKind::TableRow => Some(Role::Row),
            NodeKind::ColumnHeader => Some(Role::ColumnHeader),
            NodeKind::TableCell => Some(Role::Cell),
            _ => None,
        }
    }
}

/// Fired when a clickable view or a button is activated.
///
/// `position` is where it was clicked or tapped, in the element (see
/// [`Point`](crate::Point)); `None` when it was activated from the keyboard
/// or an assistive technology. `ClickEvent` alone is the event without a
/// position.
#[derive(Clone, Debug, PartialEq)]
pub struct ClickEvent {
    /// Where the pointer was, in the element's coordinates.
    pub position: Option<crate::events::Point>,
    /// Keys held (#159): ⌘ / Ctrl-click opens in a new tab, ⇧-click extends
    /// a selection, ⌥-click picks another action. See
    /// [`Modifiers::command`](crate::Modifiers::command).
    pub modifiers: crate::events::Modifiers,
    /// The click's number in a quick series, as the system counts them
    /// with the user's double-click speed (#159): 2 on a double click, 3 on
    /// a triple click. 1 for a single click and from the keyboard.
    pub count: u32,
    /// The button (#215): [`Primary`](crate::PointerButton::Primary) for
    /// `on_click` (and from the keyboard), the middle, back or forward
    /// button for [`on_aux_click`](Div::on_aux_click).
    pub button: crate::events::PointerButton,
    /// In a text field (#266): the character under the pointer, so ⇧-click
    /// can extend a selection to it from elsewhere (another field of a
    /// [`text_flow`](crate::text_flow)). `None` elsewhere.
    pub text_offset: Option<usize>,
}

impl Default for ClickEvent {
    fn default() -> Self {
        ClickEvent
    }
}

impl ClickEvent {
    /// A double click (or more): the second click of a quick series.
    pub fn is_double(&self) -> bool {
        self.count >= 2
    }
}

/// A click without a position (keyboard, assistive technologies), so
/// `ClickEvent` reads as a value too: `handler(&ClickEvent, cx)`.
#[allow(non_upper_case_globals)]
pub const ClickEvent: ClickEvent = ClickEvent {
    position: None,
    modifiers: crate::events::Modifiers::NONE,
    count: 1,
    button: crate::events::PointerButton::Primary,
    text_offset: None,
};

/// Fired on every edit of a text input, with its new content.
#[derive(Clone, Debug)]
pub struct InputEvent {
    /// The field's text after the edit.
    pub value: String,
}

/// The selection of a text input or area moved (#86): the user selected
/// text, moved the caret, or typed. `start..end` in **characters** (Rust
/// `char`s, Unicode scalar values) of the field's text, not UTF-8 bytes
/// nor UTF-16 units; `start == end` is a caret, and `start <= end` always
/// (a selection made backwards is reported in order).
///
/// Use [`byte_range`](SelectionEvent::byte_range) to slice the text:
///
/// ```
/// # use prism_core::SelectionEvent;
/// let text = "héllo wörld";
/// let e = SelectionEvent { start: 6, end: 11 };
/// assert_eq!(&text[e.byte_range(text)], "wörld");
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SelectionEvent {
    /// Where the selection starts, in characters.
    pub start: usize,
    /// Where it ends (excluded), in characters: equal to `start` for a caret.
    pub end: usize,
}

impl SelectionEvent {
    /// A selection from `start` to `end` characters (swapped if reversed).
    pub fn new(start: usize, end: usize) -> Self {
        SelectionEvent {
            start: start.min(end),
            end: start.max(end),
        }
    }

    /// A caret (empty selection) at character `at`.
    pub fn caret(at: usize) -> Self {
        SelectionEvent { start: at, end: at }
    }

    /// Whether nothing is selected (`start == end`): a caret.
    pub fn is_caret(&self) -> bool {
        self.start == self.end
    }

    /// `start..end` in characters.
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start..self.end
    }

    /// The same range in UTF-8 bytes of `text` (the field's value), to
    /// slice or edit it: `&text[e.byte_range(text)]`. Offsets past the end
    /// of `text` are clamped to its length.
    pub fn byte_range(&self, text: &str) -> std::ops::Range<usize> {
        char_to_byte(text, self.start)..char_to_byte(text, self.end)
    }
}

impl From<std::ops::Range<usize>> for SelectionEvent {
    fn from(r: std::ops::Range<usize>) -> Self {
        SelectionEvent::new(r.start, r.end)
    }
}

/// Another person's caret or selection in a text field (#240), for
/// collaborative editing: [`remote_carets`](TextInput::remote_carets).
/// The field draws it inside its own text (wrapping and scrolling
/// included), above the text: a thin caret in `color` at `head`, the
/// characters between `anchor` and `head` tinted with it, and `label`
/// (a name) in a small tag at the caret. It is never editable nor hit:
/// the local caret, selection and IME composition are left alone.
///
/// Offsets are in characters, like [`SelectionEvent`]; a chip or an
/// icon of a rich input counts as one.
///
/// ```
/// # use prism_core::{RemoteCaret, rgb};
/// let camille = RemoteCaret::caret(12, rgb(0xE8590C)).label("Camille");
/// let selection = RemoteCaret::new(3, 9, rgb(0x1C7ED6));
/// assert!(camille.is_caret() && !selection.is_caret());
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteCaret {
    /// Where the selection started, in characters.
    pub anchor: usize,
    /// Where the caret is, in characters (`anchor` for a caret alone).
    pub head: usize,
    /// The person's color: the caret, the label's background, and the
    /// selection's tint (drawn at a low opacity).
    pub color: Color,
    /// A name shown at the caret, if any.
    pub label: Option<String>,
    /// The label's text color (#267); `None`: white, as before.
    pub label_text_color: Option<Color>,
    /// The person stopped typing: the label fades out (the caret and the
    /// selection stay). The app decides when, from its own presence data,
    /// such as a few seconds without an update.
    pub idle: bool,
}

impl RemoteCaret {
    /// A selection from `anchor` to `head` (the caret's side) in `color`.
    pub fn new(anchor: usize, head: usize, color: Color) -> Self {
        RemoteCaret {
            anchor,
            head,
            color,
            label: None,
            label_text_color: None,
            idle: false,
        }
    }

    /// A caret alone at character `at`.
    pub fn caret(at: usize, color: Color) -> Self {
        RemoteCaret::new(at, at, color)
    }

    /// Shows `name` in a tag at the caret.
    pub fn label(mut self, name: impl Into<String>) -> Self {
        self.label = Some(name.into());
        self
    }

    /// The label's text color (#267), for a light person color that needs
    /// dark text (white by default).
    pub fn label_text_color(mut self, color: Color) -> Self {
        self.label_text_color = Some(color);
        self
    }

    /// Fades the label out (`true`) or shows it (`false`).
    pub fn idle(mut self, idle: bool) -> Self {
        self.idle = idle;
        self
    }

    /// Whether nothing is selected: a caret alone.
    pub fn is_caret(&self) -> bool {
        self.anchor == self.head
    }

    /// The selected characters, in order (`head..head` for a caret).
    pub fn range(&self) -> std::ops::Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
}

/// The UTF-8 byte offset of character `chars` in `text` (clamped to its
/// length): turns a [`SelectionEvent`] offset into one `str` methods take.
pub fn char_to_byte(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(b, _)| b)
}

/// The character offset of UTF-8 byte `byte` in `text` (clamped; a byte
/// inside a character counts as after it): the reverse of
/// [`char_to_byte`], to give [`TextInput::selection`] a position found
/// with `str::find`.
pub fn byte_to_char(text: &str, byte: usize) -> usize {
    text.char_indices().take_while(|(b, _)| *b < byte).count()
}

/// A [`rich_input`] was edited (#115): its runs as the user left them.
/// Each run keeps the style, link, handler and key of the run it came
/// from (text typed next to a run takes its style, except after a chip or
/// a link); a deleted [`chip`](crate::chip) is gone; pasted text arrives
/// plain, in the style of where it lands. Store `spans` and render them
/// back (`rich_input(self.spans.clone())`), or map them to your own model
/// with the [`Span`](crate::Span) getters (`text()`, `is_bold()`,
/// `chip_key()`…).
#[derive(Clone, Debug)]
pub struct RichInputEvent {
    /// The plain text: the runs' texts, each chip being
    /// [`OBJECT_CHAR`](crate::OBJECT_CHAR) (so selection offsets from
    /// [`on_select`](RichInput::on_select) index it, chips as one character).
    pub value: String,
    /// The runs, [normalized](crate::normalize_spans) (no empty run, equal
    /// neighbours merged).
    pub spans: Vec<crate::spans::Span>,
}

impl RichInputEvent {
    /// The event for `spans` (normalized).
    pub fn new(spans: impl IntoIterator<Item = crate::spans::Span>) -> Self {
        let spans = crate::spans::normalize_spans(spans);
        RichInputEvent {
            value: crate::spans::spans_text(&spans),
            spans,
        }
    }
}

/// A node took or lost the keyboard focus (#224): a field the user tabbed
/// into or clicked out of. See [`Div::on_focus_change`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusEvent {
    /// `true` when it took the focus, `false` when it lost it.
    pub focused: bool,
}

/// Reported by a stack when the user went back natively (iOS swipe, browser
/// back button, macOS back button): only the first `depth` screens remain.
#[derive(Clone, Debug)]
pub struct BackEvent {
    /// How many screens remain on the stack.
    pub depth: usize,
    /// How the user closed an overlay (`on_dismiss`, #156): Escape, a tap
    /// outside, a swipe… [`DismissReason::Back`] for a stack going back.
    pub reason: DismissReason,
}

/// How the user closed a sheet, popover or dialog (`on_dismiss`, #156):
/// a color picker cancels on Escape and keeps the choice on a click outside.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DismissReason {
    /// Escape (a hardware keyboard), or the TV remote's Menu button.
    Escape,
    /// A click or tap outside it (its backdrop, the page around a popover).
    Outside,
    /// A swipe down (a sheet on a phone or tablet).
    Swipe,
    /// A Back or Close button: the system's back (Android), the close
    /// button of a sheet's stack, a stack going back.
    #[default]
    Back,
    /// The platform did not say.
    Unknown,
}

impl DismissReason {
    /// `escape`, `outside`, `swipe`, `back` or `unknown`, as backends report it.
    pub fn name(self) -> &'static str {
        match self {
            DismissReason::Escape => "escape",
            DismissReason::Outside => "outside",
            DismissReason::Swipe => "swipe",
            DismissReason::Back => "back",
            DismissReason::Unknown => "unknown",
        }
    }

    /// Parses [`name`](Self::name)'s output (anything else: `Unknown`).
    pub fn from_name(name: &str) -> Self {
        match name {
            "escape" => DismissReason::Escape,
            "outside" => DismissReason::Outside,
            "swipe" => DismissReason::Swipe,
            "back" => DismissReason::Back,
            _ => DismissReason::Unknown,
        }
    }
}

/// Reported by a stack when the host asks for a path (browser forward button).
#[derive(Clone, Debug)]
pub struct NavigateEvent {
    /// The requested path.
    pub path: String,
}

/// Fired when a control's value changes: a toggle (0/1), a tab bar (index).
#[derive(Clone, Debug)]
pub struct ChangeEvent {
    /// The new value: 0 or 1 for a toggle, the selected index for a tab bar.
    pub value: f64,
}

impl ChangeEvent {
    /// A toggle's new state.
    pub fn checked(&self) -> bool {
        self.value != 0.
    }

    /// A tab bar's newly selected tab.
    pub fn index(&self) -> usize {
        self.value.max(0.) as usize
    }

    /// A date picker's new value (the Unix epoch when an optional picker
    /// was cleared: read those with [`date_opt`](Self::date_opt)).
    pub fn date(&self) -> std::time::SystemTime {
        self.date_opt().unwrap_or(std::time::UNIX_EPOCH)
    }

    /// An optional date picker's new value ([`date_picker_opt`]): `None`
    /// when the user cleared it.
    pub fn date_opt(&self) -> Option<std::time::SystemTime> {
        self.value.is_finite().then(|| unix_time(self.value))
    }
}

/// `secs` since 1970 as a [`SystemTime`](std::time::SystemTime) (negative:
/// before). The Unix epoch when `secs` is not a number or no `SystemTime`
/// can hold it.
pub fn unix_time(secs: f64) -> std::time::SystemTime {
    let Ok(d) = std::time::Duration::try_from_secs_f64(secs.abs()) else {
        return std::time::UNIX_EPOCH;
    };
    if secs >= 0. {
        std::time::UNIX_EPOCH.checked_add(d)
    } else {
        std::time::UNIX_EPOCH.checked_sub(d)
    }
    .unwrap_or(std::time::UNIX_EPOCH)
}

/// Seconds since 1970 (negative: before).
pub fn unix_seconds(t: std::time::SystemTime) -> f64 {
    match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

/// A scroll view moved: offset of the content and size of the viewport (pt).
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollEvent {
    /// How far the content is scrolled, in points.
    pub offset: f32,
    /// Height of the visible area, in points.
    pub viewport: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// Direction of a horizontal swipe ([`Div::on_swipe`]).
pub enum SwipeDirection {
    /// Finger moving to the left.
    Left,
    /// Finger moving to the right.
    Right,
}

/// An event handler. Handlers receive the whole `App` so they can update any entity.
pub type Handler<E> = Rc<dyn Fn(&E, &mut App)>;

/// An unresolved element: host nodes plus references to entities, which are
/// rendered lazily when the tree is resolved.
pub enum Element {
    /// A host node (view, text, button…) and its children.
    Node(Box<VNode>),
    /// A child entity, rendered by itself.
    Entity(AnyEntity),
    /// Several elements in a row, without a wrapper.
    Fragment(Vec<Element>),
    /// Built when the tree is resolved, with access to the app (themes,
    /// globals). This is how stateless components are implemented.
    Lazy(Box<dyn FnOnce(&mut App) -> Element>),
    /// Nothing (an absent optional child).
    Empty,
}

/// An element built at resolve time with access to the [`App`]:
/// `lazy(|cx| text(cx.global::<Theme>().name))`.
pub fn lazy<E: IntoElement>(f: impl FnOnce(&mut App) -> E + 'static) -> Element {
    Element::Lazy(Box::new(move |app| f(app).into_element()))
}

/// A host node under construction.
pub struct VNode {
    /// What host view this node becomes.
    pub kind: NodeKind,
    /// Layout and appearance.
    pub style: Style,
    /// Text content: a label, a button title, a field's value, a screen title, a picture source.
    pub text: String,
    /// Placeholder of a text input; a timer's text once it has ended
    /// ([`Timer::after`]).
    pub placeholder: String,
    /// Icon name (SF Symbol on Apple platforms), used by tabs.
    pub icon: String,
    /// Numeric value: toggle state (0/1), selected tab index.
    pub value: Option<f64>,
    /// Control configuration (see [`crate::controls`]).
    pub config: String,
    /// Accessibility overrides (role, label, hint…).
    pub a11y: Accessibility,
    /// Deep link (`/todos/1`): navigates in the app, opens it from widgets,
    /// Live Activities and notifications.
    pub link: Option<String>,
    /// Long press (iOS, Android), right-click (macOS, Linux, Windows, web)
    /// menu.
    pub context_menu: Option<crate::menu::Menu>,
    /// A menu opened below the element when it is clicked (#247), in
    /// place of `on_click`.
    pub click_menu: Option<crate::menu::Menu>,
    /// Identity among siblings, so reordered children are moved rather than rebuilt.
    pub key: Option<String>,
    /// Child elements.
    pub children: Vec<Element>,
    /// Click or tap (also pull to refresh on scroll views).
    pub on_click: Option<Handler<ClickEvent>>,
    /// Every edit of a text input.
    pub on_input: Option<Handler<InputEvent>>,
    /// Every edit of a rich input (#115).
    pub on_rich_input: Option<Handler<RichInputEvent>>,
    /// The selection or caret of a text input or area moved (#86).
    pub on_select: Option<Handler<SelectionEvent>>,
    /// The selection the app sets on a text input or area, in characters
    /// (applied when it changes, #86).
    pub selection: Option<SelectionEvent>,
    /// Other people's carets and selections drawn in a text field (#240).
    pub remote_carets: Vec<RemoteCaret>,
    /// The user went back natively on a stack.
    pub on_back: Option<Handler<BackEvent>>,
    /// The host asked a stack for a path.
    pub on_navigate: Option<Handler<NavigateEvent>>,
    /// A toggle or tab bar changed value.
    pub on_change: Option<Handler<ChangeEvent>>,
    /// A scroll view moved.
    pub on_scroll: Option<Handler<ScrollEvent>>,
    /// A swipe to the left.
    pub on_swipe_left: Option<Handler<ClickEvent>>,
    /// A swipe to the right.
    pub on_swipe_right: Option<Handler<ClickEvent>>,
    /// The return key of a text input.
    pub on_submit: Option<Handler<ClickEvent>>,
    /// Runs of a rich text (`rich_text`): its text and config are derived
    /// from them when mounted.
    pub spans: Vec<crate::spans::Span>,

    // --- Pointer, keys, focus (#17) ---
    /// A name to address the element from code: `cx.focus(name)`,
    /// `cx.scroll_into_view(name)`.
    pub id: Option<String>,
    /// Takes the keyboard focus when it appears.
    pub autofocus: bool,
    /// The pointer came over the element or left it.
    pub on_hover: Option<Handler<crate::events::HoverEvent>>,
    /// The pointer moved over the element.
    pub on_pointer_move: Option<Handler<crate::events::PointerEvent>>,
    /// A drag started on the element.
    pub on_drag: Option<Handler<crate::events::DragEvent>>,
    /// A key pressed while the element, or something in it, has the focus.
    pub on_key_down: Option<Handler<crate::events::KeyEvent>>,
    /// A key pressed anywhere in the window while the element is mounted.
    pub on_global_key_down: Option<Handler<crate::events::KeyEvent>>,
    /// A key released while the element, or something in it, has the
    /// focus (#159).
    pub on_key_up: Option<Handler<crate::events::KeyEvent>>,
    /// A key released anywhere in the window while the element is mounted
    /// (#159).
    pub on_global_key_up: Option<Handler<crate::events::KeyEvent>>,

    // --- Islands: client-side Web Components (WASM or JS) ---
    /// Makes this view an island (see [`island`](crate::island())): its
    /// custom element, props and events. Its children are the fallback.
    pub island: Option<Box<crate::island::IslandSpec>>,

    // --- Widget buttons (#47) ---
    /// In a widget or Live Activity: a tap runs this action of the app
    /// ([`App::on_widget_action`](crate::App::on_widget_action)) without
    /// opening it: `(action, payload)`.
    pub widget_action: Option<(String, String)>,

    // --- 3D ---
    /// A tap on a 3D scene ([`scene3d`](crate::scene3d::scene3d)).
    pub on_scene_tap: Option<Handler<crate::scene3d::SceneTapEvent>>,
    /// A 3D scene's contents (the reconciler sends what changed).
    pub scene: Option<Box<crate::scene3d::SceneData>>,
    // --- App-defined attributes (#85) ---
    /// Attributes of the element on the web (`data-*` and a few safe ones,
    /// see [`crate::attributes`]), in the order first set. Native backends
    /// ignore them.
    pub attributes: Vec<(String, String)>,

    // --- Web views (#62) ---
    /// A web view's page posted a message.
    pub on_web_message: Option<Handler<crate::web_view::WebMessageEvent>>,
    /// A web view finished loading a page.
    pub on_web_load: Option<Handler<crate::web_view::WebLoadEvent>>,

    /// The `pointer_mask` drag bits of the buttons that start a drag
    /// (#136), the primary one by default.
    pub drag_buttons: u8,
    /// On touch screens, how long a press must be held still before an
    /// [`on_drag`](Div::on_drag) starts, in milliseconds (#243); 0 starts
    /// it on movement.
    pub drag_delay_ms: u32,
    /// Keys the element's `on_key_down` takes from the focused control in
    /// it (#181): a text field does not also act on them.
    pub consume_keys: Vec<String>,

    /// Its frame after each layout that moved or resized it (#192).
    pub on_layout: Option<Handler<crate::events::LayoutEvent>>,
    /// Its frame is kept for [`App::frame_of`](crate::App::frame_of) (#192).
    pub track_layout: bool,

    // --- Wheel and pinch (#133) ---
    /// Wheel and trackpad scrolling over the node.
    pub on_wheel: Option<Handler<crate::events::WheelEvent>>,
    /// Clicks with the middle, back or forward button (#215).
    pub on_aux_click: Option<Handler<ClickEvent>>,
    /// The node took or lost the keyboard focus (#224).
    pub on_focus_change: Option<Handler<FocusEvent>>,
    /// Pinches over the node.
    pub on_magnify: Option<Handler<crate::events::MagnifyEvent>>,

    // --- Drop targets (#111) ---
    /// Files dropped on the node: the types it accepts, and the handler.
    pub on_drop_files: Option<(
        crate::device::PickFiles,
        Handler<crate::events::FileDropEvent>,
    )>,
    /// The upload of what was dropped on the node (#199).
    pub on_drop_progress: Option<Handler<crate::events::DropProgress>>,
    /// A rich input's editing hooks (#217): its selection's rectangle,
    /// copy, cut and paste, modifier-clicks on its spans.
    pub edit: crate::text_edit::EditHandlers,
    /// Its text fields share one selection (#231,
    /// [`selection_group`](crate::Div::selection_group)).
    pub selection_group: bool,
    /// A selection group's handlers (#231).
    pub across: crate::selection_across::GroupHandlers,

    // --- Drag and drop between elements (#251) ---
    /// What the node carries when dragged.
    pub drag_payload: Option<crate::events::DragPayload>,
    /// Payloads dropped on the node, by kind.
    pub on_drop_payload: Vec<(String, Handler<crate::events::DropEvent>)>,
    /// Payloads dragged over the node, by kind.
    pub on_drag_over: Vec<(String, Handler<crate::events::DragOverEvent>)>,
    /// A video's playback reached the end (#207).
    pub on_ended: Option<Handler<crate::video::EndedEvent>>,
    /// A text field's own bar above the on-screen keyboard (#242): placed
    /// right after the field, as a [`keyboard_toolbar`] serving only it.
    pub keyboard_toolbar: Option<Box<Element>>,
}

impl VNode {
    pub(crate) fn new(kind: NodeKind) -> Self {
        VNode {
            kind,
            style: Style::default(),
            text: String::new(),
            placeholder: String::new(),
            icon: String::new(),
            value: None,
            config: String::new(),
            a11y: Accessibility::default(),
            link: None,
            context_menu: None,
            click_menu: None,
            key: None,
            children: Vec::new(),
            on_click: None,
            on_input: None,
            on_rich_input: None,
            on_select: None,
            selection: None,
            remote_carets: Vec::new(),
            on_back: None,
            on_navigate: None,
            on_change: None,
            on_scroll: None,
            on_swipe_left: None,
            on_swipe_right: None,
            on_submit: None,
            spans: Vec::new(),
            id: None,
            autofocus: false,
            on_hover: None,
            on_pointer_move: None,
            on_drag: None,
            on_key_down: None,
            on_global_key_down: None,
            on_key_up: None,
            on_global_key_up: None,
            island: None,
            widget_action: None,
            on_scene_tap: None,
            scene: None,
            attributes: Vec::new(),
            on_web_message: None,
            on_web_load: None,
            on_wheel: None,
            on_aux_click: None,
            on_focus_change: None,
            on_magnify: None,
            on_layout: None,
            track_layout: false,
            drag_buttons: crate::events::pointer_mask::DRAG,
            drag_delay_ms: 0,
            consume_keys: Vec::new(),
            on_drop_files: None,
            on_drop_progress: None,
            edit: Default::default(),
            selection_group: false,
            across: Default::default(),
            drag_payload: None,
            on_drop_payload: Vec::new(),
            on_drag_over: Vec::new(),
            on_ended: None,
            keyboard_toolbar: None,
        }
    }
}

/// Anything that can be placed in the tree.
pub trait IntoElement {
    /// Converts into an [`Element`].
    fn into_element(self) -> Element;
}

impl IntoElement for Element {
    fn into_element(self) -> Element {
        self
    }
}

impl<E: IntoElement> IntoElement for Option<E> {
    fn into_element(self) -> Element {
        self.map_or(Element::Empty, IntoElement::into_element)
    }
}

impl<E: IntoElement> IntoElement for Vec<E> {
    fn into_element(self) -> Element {
        Element::Fragment(self.into_iter().map(IntoElement::into_element).collect())
    }
}

impl IntoElement for &str {
    fn into_element(self) -> Element {
        text(self).into_element()
    }
}

impl IntoElement for String {
    fn into_element(self) -> Element {
        text(self).into_element()
    }
}

impl<T: Render> IntoElement for Entity<T> {
    fn into_element(self) -> Element {
        Element::Entity(self.into_any())
    }
}

/// Builders that accept children.
pub trait ParentElement: Sized {
    /// The builder's children, for the provided methods.
    fn children_mut(&mut self) -> &mut Vec<Element>;

    /// Appends a child.
    fn child(mut self, child: impl IntoElement) -> Self {
        self.children_mut().push(child.into_element());
        self
    }

    /// Appends several children.
    fn children<E: IntoElement>(mut self, children: impl IntoIterator<Item = E>) -> Self {
        self.children_mut()
            .extend(children.into_iter().map(IntoElement::into_element));
        self
    }
}

/// Conditional builder helpers (`.when(cond, |this| ...)`).
pub trait FluentBuilder: Sized {
    /// Applies `f` only if `cond` holds.
    fn when(self, cond: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if cond { f(self) } else { self }
    }

    /// Applies `f` with the value, if there is one.
    fn when_some<T>(self, value: Option<T>, f: impl FnOnce(Self, T) -> Self) -> Self {
        match value {
            Some(v) => f(self, v),
            None => self,
        }
    }

    /// Applies `f` (keeps long builder chains readable).
    fn map(self, f: impl FnOnce(Self) -> Self) -> Self {
        f(self)
    }
}

macro_rules! builder {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        pub struct $name(VNode);

        impl Styled for $name {
            fn style_mut(&mut self) -> &mut Style {
                &mut self.0.style
            }
        }

        impl FluentBuilder for $name {}

        impl Accessible for $name {
            fn a11y_mut(&mut self) -> &mut Accessibility {
                &mut self.0.a11y
            }
        }

        impl IntoElement for $name {
            fn into_element(self) -> Element {
                Element::Node(Box::new(self.0))
            }
        }

        impl $name {
            /// Stable identity among siblings, so reordered lists keep their native views.
            pub fn key(mut self, key: impl Into<String>) -> Self {
                self.0.key = Some(key.into());
                self
            }

            /// A name to address the element from code: [`App::focus`],
            /// [`App::scroll_into_view`]. Unique in the window (the last
            /// element given a name wins).
            pub fn id(mut self, id: impl Into<String>) -> Self {
                self.0.id = Some(id.into());
                self
            }

            /// Sets an attribute of the element on the web, for browser
            /// integrations and end-to-end tests: `data-*`
            /// (`.attr("data-1p-ignore", "")`, `.attr("data-testid", "save")`)
            /// and the few names of [`SAFE_ATTRIBUTES`]
            /// (`.attr("autocomplete", "one-time-code")`). Set in the first
            /// paint and kept in sync live; setting a name again replaces its
            /// value. Native backends ignore it.
            ///
            /// The name is lower-cased. Any other name (event handlers,
            /// `style`, the `data-*` names Prism uses itself: see
            /// [`is_app_attribute`]) is ignored, and fails a debug assertion.
            ///
            /// [`SAFE_ATTRIBUTES`]: crate::attributes::SAFE_ATTRIBUTES
            /// [`is_app_attribute`]: crate::attributes::is_app_attribute
            pub fn attr(mut self, name: impl AsRef<str>, value: impl Into<String>) -> Self {
                let name = name.as_ref().to_ascii_lowercase();
                let allowed = crate::attributes::is_app_attribute(&name);
                debug_assert!(
                    allowed,
                    "`{name}` is not an attribute an app may set (see prism_core::attributes)"
                );
                if allowed {
                    let value = value.into();
                    match self.0.attributes.iter_mut().find(|(n, _)| *n == name) {
                        Some(slot) => slot.1 = value,
                        None => self.0.attributes.push((name, value)),
                    }
                }
                self
            }

            /// Takes the keyboard focus when it appears (a text field in a
            /// form or a dialog, a pad driven by arrow keys).
            pub fn autofocus(mut self) -> Self {
                self.0.autofocus = true;
                self
            }

            /// Called when a key is pressed while this element, or
            /// something in it, has the focus. A view with this handler can
            /// take the focus (Tab, a click, [`App::focus`], the tvOS focus
            /// engine, an Android D-pad).
            pub fn on_key_down(mut self, handler: Handler<crate::events::KeyEvent>) -> Self {
                self.0.on_key_down = Some(handler);
                self
            }

            /// Keys (`"ArrowDown"`, `"Enter"`, as [`KeyEvent::key`](crate::KeyEvent::key)
            /// names them) that go to the `on_key_down` handlers here and no
            /// further (#181): while the focus is in this element, a text
            /// field in it does not also act on them (↑ does not move its
            /// caret, Enter does not submit), nor does the page or the
            /// window. Declared at render, so it works the same on every
            /// platform, the web included: set them while a suggestion
            /// menu is open, `&[]` when it closes.
            ///
            /// ```ignore
            /// div()
            ///     .child(rich_input(runs))
            ///     .on_key_down(cx.listener(|this, e: &KeyEvent, _| this.menu_key(e)))
            ///     .consume_keys(if menu_open { &["ArrowUp", "ArrowDown", "Enter", "Escape"][..] } else { &[] })
            /// ```
            pub fn consume_keys(mut self, keys: &[&str]) -> Self {
                self.0.consume_keys = keys.iter().map(|k| k.to_string()).collect();
                self
            }

            /// Takes `key` from the focused text field in this element only
            /// while its caret is at `edge` (#211), decided by the platform
            /// when the key is pressed: ← at the start or → at the end moves
            /// to the previous or next cell of a grid, ↑ on the first visual
            /// line or ↓ on the last one (wrapping included) to the previous
            /// or next block of a document. Elsewhere the field moves its
            /// caret. [`KeyEvent::at`](crate::KeyEvent::at) tells the handler
            /// the same. Adds to [`consume_keys`](Self::consume_keys) (call
            /// it after).
            ///
            /// ```ignore
            /// div()
            ///     .child(text_area(block.text.clone()))
            ///     .consume_key_at("ArrowUp", CaretEdge::FirstLine)
            ///     .consume_key_at("ArrowDown", CaretEdge::LastLine)
            ///     .on_key_down(cx.listener(move |this, e: &KeyEvent, cx| {
            ///         if e.is("ArrowUp") && e.at(CaretEdge::FirstLine) { this.focus_block(i - 1, cx) }
            ///         if e.is("ArrowDown") && e.at(CaretEdge::LastLine) { this.focus_block(i + 1, cx) }
            ///     }))
            /// ```
            pub fn consume_key_at(mut self, key: &str, edge: crate::events::CaretEdge) -> Self {
                self.0.consume_keys.push(format!("{key}@{}", edge.name()));
                self
            }

            /// Called for every key pressed in the window while this element
            /// is mounted, whatever has the focus (Escape to close, arrows
            /// for a remote pad). While a text field has the focus, only
            /// keys that do not type (Escape, arrows, Enter, Tab, function
            /// keys) or come with Control / Command are reported.
            pub fn on_global_key_down(mut self, handler: Handler<crate::events::KeyEvent>) -> Self {
                self.0.on_global_key_down = Some(handler);
                self
            }

            /// Called when a key is released while this element, or
            /// something in it, has the focus (#159): the end of « hold a
            /// key » interactions. Like [`on_key_down`](Self::on_key_down),
            /// it makes the element focusable.
            pub fn on_key_up(mut self, handler: Handler<crate::events::KeyEvent>) -> Self {
                self.0.on_key_up = Some(handler);
                self
            }

            /// Called for every key released in the window while this
            /// element is mounted (#159), with the same rules as
            /// [`on_global_key_down`](Self::on_global_key_down): hold Space
            /// to pan a canvas, release it to stop.
            pub fn on_global_key_up(mut self, handler: Handler<crate::events::KeyEvent>) -> Self {
                self.0.on_global_key_up = Some(handler);
                self
            }
        }
    };
}

builder!(
    /// A flex container. See [`div`].
    Div
);
builder!(
    /// A text run. See [`text`].
    Text
);
builder!(
    /// A native button. See [`button`].
    Button
);
builder!(
    /// A native single-line text field. See [`text_input`].
    TextInput
);
builder!(
    /// An editable paragraph of styled runs and inline objects. See
    /// [`rich_input`].
    RichInput
);
builder!(
    /// A screen of a navigation stack. See [`screen`].
    Screen
);
builder!(
    /// A native navigation stack. Usually created by [`Router`](crate::Router).
    Stack
);
builder!(
    /// A native on/off switch. See [`toggle`].
    Toggle
);
builder!(
    /// A scrollable container. See [`scroll`].
    Scroll
);
builder!(
    /// A native tab bar. Usually created by [`Router`](crate::Router) tabs.
    Tabs
);
builder!(
    /// One tab of a [`Tabs`] bar. See [`tab`].
    Tab
);
builder!(
    /// A symbol image. See [`image`].
    Image
);
builder!(
    /// A progress bar. See [`progress`].
    Progress
);
builder!(
    /// A self-updating countdown. See [`timer`].
    Timer
);
builder!(
    /// A sheet. Usually created by [`Router`](crate::Router) (`cx.present`).
    Modal
);
builder!(
    /// A bitmap image. See [`picture`].
    Picture
);
builder!(
    /// A vector drawing. See [`shape`].
    Shape
);
builder!(
    /// A native slider. See [`slider`].
    Slider
);
builder!(
    /// A native menu picker. See [`picker`].
    Picker
);
builder!(
    /// A native date / time picker. See [`date_picker`].
    DatePicker
);
builder!(
    /// A native multi-line text field. See [`text_area`].
    TextArea
);
builder!(
    /// An activity indicator. See [`spinner`].
    Spinner
);

/// A flex container, laid out as a column by default.
pub fn div() -> Div {
    Div(VNode::new(NodeKind::View))
}

/// A text label.
pub fn text(content: impl Into<String>) -> Text {
    let mut n = VNode::new(NodeKind::Text);
    n.text = content.into();
    Text(n)
}

/// A paragraph of runs with their own style, links and icons:
/// `rich_text([span("Signed in as "), span("Ada").bold(), link("settings", "/settings")])`.
/// See [`spans`](crate::spans).
pub fn rich_text(spans: impl IntoIterator<Item = crate::spans::Span>) -> Text {
    text("").spans(spans)
}

impl Text {
    /// Replaces the text with runs (see [`rich_text`]).
    pub fn spans(mut self, spans: impl IntoIterator<Item = crate::spans::Span>) -> Self {
        self.0.spans = spans.into_iter().collect();
        self.0.text = self.0.spans.iter().map(|s| s.display_text()).collect();
        self
    }

    /// Makes the whole text clickable (a button for assistive technologies,
    /// reachable from the keyboard).
    pub fn on_click(mut self, handler: Handler<ClickEvent>) -> Self {
        self.0.on_click = Some(handler);
        self
    }

    /// Makes the whole text a link to an app path (or a URL): it navigates
    /// when activated, and is announced as a link.
    pub fn link(mut self, path: impl Into<String>) -> Self {
        self.0.link = Some(path.into());
        self
    }
}

/// A native push button with a title.
pub fn button(label: impl Into<String>) -> Button {
    let mut n = VNode::new(NodeKind::Button);
    n.text = label.into();
    Button(n)
}

/// A single-line text field. Controlled: set its value with [`TextInput::value`] from [`TextInput::on_input`].
pub fn text_input() -> TextInput {
    TextInput(VNode::new(NodeKind::TextInput))
}

impl ParentElement for Div {
    fn children_mut(&mut self) -> &mut Vec<Element> {
        &mut self.0.children
    }
}

impl Div {
    /// Groups the named text fields inside (`.id(…)`: rich inputs, text
    /// areas, text inputs) into one selection (#231): ⇧+arrows at a
    /// field's edge, and mouse drags from one field into another, extend
    /// the selection into the next or previous field. See
    /// [`selection_across`](crate::selection_across) and [`text_flow`](crate::text_flow).
    pub fn selection_group(mut self) -> Self {
        self.0.selection_group = true;
        self
    }

    /// Called when the selection of this group changes while it spans
    /// several fields (#231), and once more when it comes back inside one
    /// field ([`SelectionAcross::is_across`](crate::SelectionAcross::is_across)
    /// then `false`). Makes the container a
    /// [`selection_group`](Self::selection_group).
    pub fn on_selection_across(
        mut self,
        handler: Handler<crate::selection_across::SelectionAcross>,
    ) -> Self {
        self.0.selection_group = true;
        self.0.across.on_selection = Some(handler);
        self
    }

    /// Typing, Backspace, Delete, Enter, a cut or a paste over a selection
    /// that spans several fields of this group (#231): no field applies
    /// it, the app replaces the range in its model. Without it, such a key
    /// first brings the selection back into the field where it started.
    /// Makes the container a [`selection_group`](Self::selection_group).
    pub fn on_edit_across(mut self, handler: Handler<crate::selection_across::EditAcross>) -> Self {
        self.0.selection_group = true;
        self.0.across.on_edit = Some(handler);
        self
    }

    /// Makes the container a link to an app path. Works everywhere: in the app
    /// (navigates), in widgets and Live Activities (opens the app there).
    pub fn link(mut self, path: impl Into<String>) -> Self {
        self.0.link = Some(path.into());
        self
    }

    /// A bar above the on-screen keyboard shared by every text field
    /// inside the container (#242): it shows while any of them has the
    /// focus (a field's own [`keyboard_toolbar`](TextInput::keyboard_toolbar)
    /// and a nearer container's bar win). Shorthand for
    /// `.child(keyboard_toolbar(bar))`.
    pub fn keyboard_toolbar(self, bar: impl IntoElement) -> Self {
        self.child(crate::keyboard_toolbar(bar))
    }

    /// A context menu: long press on iOS and Android, right-click on macOS,
    /// Linux, Windows and the web (plus the Menu key / Shift+F10 on the web
    /// and Windows). Exposed to assistive technologies.
    pub fn context_menu(mut self, menu: crate::menu::Menu) -> Self {
        self.0.context_menu = Some(menu);
        self
    }

    /// Opens `menu`, the platform's own pop-up menu, below the element when
    /// it is clicked or tapped (#247): a "+" button, a "…" button. It takes
    /// the place of [`on_click`](Self::on_click). See
    /// [`App::show_menu_on`](crate::App::show_menu_on) to open one from a
    /// handler, at a point.
    pub fn on_click_menu(mut self, menu: crate::menu::Menu) -> Self {
        self.0.click_menu = Some(menu);
        self
    }

    /// A horizontal swipe (touch, trackpad). Not reachable by every user:
    /// also offer the action elsewhere (context menu, button).
    pub fn on_swipe(mut self, direction: SwipeDirection, handler: Handler<ClickEvent>) -> Self {
        match direction {
            SwipeDirection::Left => self.0.on_swipe_left = Some(handler),
            SwipeDirection::Right => self.0.on_swipe_right = Some(handler),
        }
        self
    }

    /// Makes the whole container clickable.
    pub fn on_click(mut self, handler: Handler<ClickEvent>) -> Self {
        self.0.on_click = Some(handler);
        self
    }
}

impl Button {
    /// Called when the button is pressed.
    pub fn on_click(mut self, handler: Handler<ClickEvent>) -> Self {
        self.0.on_click = Some(handler);
        self
    }

    /// Makes the button a link to an app path (or a URL) (#230), like
    /// [`Div::link`]: pressing it navigates, and on the web it is an `<a
    /// href>` with the button's look, so a middle click or « Open in new
    /// tab » works, and it works before the page's script runs. Announced
    /// as a link. An `on_click` handler, if any, runs instead of the
    /// navigation in the app.
    pub fn link(mut self, path: impl Into<String>) -> Self {
        self.0.link = Some(path.into());
        self
    }

    /// Opens `menu` below the button when it is pressed (#247), in place of
    /// [`on_click`](Self::on_click): a "+" or "…" button with the
    /// platform's own pop-up menu. See [`Div::on_click_menu`].
    pub fn on_click_menu(mut self, menu: crate::menu::Menu) -> Self {
        self.0.click_menu = Some(menu);
        self
    }
}

impl TextInput {
    /// Called when this node takes or loses the keyboard focus (#224):
    /// commit a field's value when the user tabs away or clicks elsewhere,
    /// not on every keystroke. `e.focused` says which. For the node itself
    /// (a field, or a view focusable through `on_key_down`); use
    /// `focus_within` styles for a group.
    pub fn on_focus_change(mut self, handler: Handler<FocusEvent>) -> Self {
        self.0.on_focus_change = Some(handler);
        self
    }

    /// The controlled value of the field.
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.0.text = value.into();
        self
    }

    /// Text shown while the field is empty (also its default accessibility label).
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.0.placeholder = placeholder.into();
        self
    }

    /// Called on every edit, with the new text.
    pub fn on_input(mut self, handler: Handler<InputEvent>) -> Self {
        self.0.on_input = Some(handler);
        self
    }

    /// Hides the characters (passwords): `isSecureTextEntry`,
    /// `NSSecureTextField`, `type=password`, an invisible `GtkEntry`,
    /// `textPassword`. Autocorrection and capitalization are off.
    pub fn secure(self, secure: bool) -> Self {
        self.input_config(|c| c.secure = secure)
    }

    /// Called when the user presses the return key (Enter).
    pub fn on_submit(mut self, handler: Handler<ClickEvent>) -> Self {
        self.0.on_submit = Some(handler);
        self
    }

    /// The return key's label: Done, Go, Next, Search, Send.
    pub fn return_key(self, key: crate::controls::ReturnKey) -> Self {
        self.input_config(|c| c.return_key = key)
    }
}

macro_rules! text_entry {
    ($ty:ident) => {
        impl $ty {
            fn input_config(mut self, f: impl FnOnce(&mut crate::controls::InputConfig)) -> Self {
                let mut c = crate::controls::InputConfig::decode(&self.0.config);
                f(&mut c);
                // Keep the suggestions (the lines after the first).
                let suggestions = crate::controls::input_suggestions(&self.0.config);
                self.0.config = crate::controls::with_suggestions(&c.encode(), &suggestions);
                self
            }

            /// The on-screen keyboard: email, number, decimal, phone, URL.
            pub fn keyboard(self, keyboard: crate::controls::Keyboard) -> Self {
                self.input_config(|c| c.keyboard = keyboard)
            }

            /// Automatic capitalization (`Capitalize::None` for emails, codes).
            pub fn capitalize(self, capitalize: crate::controls::Capitalize) -> Self {
                self.input_config(|c| c.capitalize = capitalize)
            }

            /// Autocorrection and spell checking (on by default).
            pub fn autocorrect(self, on: bool) -> Self {
                self.input_config(|c| c.autocorrect = on)
            }

            /// Called when the selection or the caret moves (#86): the user
            /// selects text, clicks or taps in it, moves with the arrow
            /// keys, and after each edit (after its `on_input`). Offsets are
            /// in characters ([`SelectionEvent`]). The web sends at most one
            /// per animation frame. A key handler that needs the caret
            /// before the key acts reads [`KeyEvent::selection`](crate::KeyEvent::selection).
            pub fn on_select(mut self, handler: Handler<SelectionEvent>) -> Self {
                self.0.on_select = Some(handler);
                self
            }

            /// Selects `start..end` (characters; `n..n` places the caret)
            /// in the field (#86): after a command, to restore or move the
            /// caret. Like `value`, it is applied when it changes, focused
            /// or not, after the value. A selection the user made and the
            /// app renders back (stored from [`on_select`](Self::on_select))
            /// leaves the field alone, so keeping
            /// `.selection(self.sel.clone())` in sync never fights the user;
            /// a value that did not change is not applied again either.
            /// Offsets past the end are clamped to the text.
            pub fn selection(mut self, range: impl Into<SelectionEvent>) -> Self {
                self.0.selection = Some(range.into());
                self
            }

            /// A bar above the on-screen keyboard while this field has the
            /// focus (#242): formatting buttons, « Done ». See
            /// [`keyboard_toolbar`](crate::keyboard_toolbar) for where it
            /// shows; tapping its buttons keeps the field focused and the
            /// keyboard up. It takes precedence over a container's shared
            /// bar ([`Div::keyboard_toolbar`]).
            pub fn keyboard_toolbar(mut self, bar: impl IntoElement) -> Self {
                self.0.keyboard_toolbar = Some(Box::new(bar.into_element()));
                self
            }

            /// Draws other people's carets and selections in the field
            /// (#240), for collaborative editing: each [`RemoteCaret`] is a
            /// thin caret in its color with an optional name, and a tinted
            /// selection, laid out with the field's own text (wrapping and
            /// scrolling included) and above it. They are never editable
            /// and updating them leaves the user's caret, selection and IME
            /// composition alone. An empty list removes them. The web,
            /// macOS, iOS, iPadOS, visionOS, Android, Linux and Windows;
            /// tvOS and watchOS fields show none.
            pub fn remote_carets(mut self, carets: impl IntoIterator<Item = RemoteCaret>) -> Self {
                self.0.remote_carets = carets.into_iter().collect();
                self
            }
        }
    };
}
text_entry!(TextInput);
text_entry!(TextArea);
text_entry!(RichInput);

/// An editable paragraph of styled runs and inline objects (#115): the
/// [`Span`](crate::Span)s of [`rich_text`], edited in place. Bold, italic,
/// underline, strikethrough, inline code (`font_mono`, `bg`) and links stay
/// styled while the user types; a [`chip`](crate::chip) (a mention of a
/// page, a person, a date) is one character the caret jumps over and one
/// Backspace deletes. Controlled like [`text_input`]: render the spans you
/// store, store what [`on_input`](RichInput::on_input) gives back.
///
/// ```
/// # use prism_core::prelude::*;
/// # use prism_core::{restyle_spans, splice_spans};
/// struct Note { spans: Vec<Span>, sel: SelectionEvent }
///
/// impl Render for Note {
///     fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
///         rich_input(self.spans.clone())
///             .placeholder("Write something…")
///             .on_input(cx.listener(|this, e: &RichInputEvent, _| this.spans = e.spans.clone()))
///             .on_select(cx.listener(|this, e: &SelectionEvent, _| this.sel = *e))
///             .selection(self.sel)
///             .on_key_down(cx.listener(|this, e: &KeyEvent, _| {
///                 // ⌘B: bold the selection.
///                 if e.is("b") && e.modifiers.command() {
///                     this.spans = restyle_spans(&this.spans, this.sel.range(), Span::bold);
///                 }
///                 // "@": a mention picker would insert a chip at the caret.
///                 if e.is("F2") {
///                     let at = this.sel.start;
///                     this.spans = splice_spans(&this.spans, at..at, [chip("page:42", "Roadmap")]);
///                     this.sel = SelectionEvent::caret(at + 1);
///                 }
///             }))
///     }
/// }
/// ```
///
/// It wraps like a paragraph and grows with its content: one line of text
/// tall when it holds one (cap it with `max_h`). Enter calls [`on_submit`](RichInput::on_submit) and inserts
/// nothing (a block editor splits the block there, with
/// [`split_spans`](crate::split_spans) at [`KeyEvent::selection`](crate::KeyEvent::selection));
/// Shift+Enter inserts a line break (`'\n'`). Selections
/// ([`on_select`](RichInput::on_select), [`selection`](RichInput::selection))
/// count characters of the value, a chip being one, and follow the rules
/// of text fields: a value or selection the app sets while the field is
/// focused is applied, echoes of the user's own typing are not (#73, #86).
///
/// Styles come from the app: the field has no formatting commands of its
/// own (⌘B is yours, with [`restyle_spans`](crate::restyle_spans)), and
/// pasted text arrives plain. Rich editing runs on the web, macOS,
/// Windows, iOS, iPadOS, visionOS, Linux and Android; tvOS and watchOS show
/// a plain multi-line field of the value instead (styles not drawn, a chip
/// shown as `U+FFFC`), whose edits still come back as spans that keep the
/// styles of the text around them.
pub fn rich_input(spans: impl IntoIterator<Item = crate::spans::Span>) -> RichInput {
    // A paragraph: one line at least, where backends show it as a text
    // area (whose minimum is three lines, #235).
    RichInput(VNode::new(NodeKind::RichInput))
        .input_config(|c| c.rows = Some(1))
        .spans(spans)
}

impl RichInput {
    /// Called when this node takes or loses the keyboard focus (#224):
    /// commit a field's value when the user tabs away or clicks elsewhere,
    /// not on every keystroke. `e.focused` says which. For the node itself
    /// (a field, or a view focusable through `on_key_down`); use
    /// `focus_within` styles for a group.
    pub fn on_focus_change(mut self, handler: Handler<FocusEvent>) -> Self {
        self.0.on_focus_change = Some(handler);
        self
    }

    /// Replaces the runs (see [`rich_input`]).
    pub fn spans(mut self, spans: impl IntoIterator<Item = crate::spans::Span>) -> Self {
        self.0.spans = spans.into_iter().collect();
        self.0.text = crate::spans::spans_text(&self.0.spans);
        self
    }

    /// Text shown while the field is empty (also its default accessibility label).
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.0.placeholder = placeholder.into();
        self
    }

    /// Called on every edit with the new runs and plain value
    /// ([`RichInputEvent`]).
    pub fn on_input(mut self, handler: Handler<RichInputEvent>) -> Self {
        self.0.on_rich_input = Some(handler);
        self
    }

    /// Called when the user presses Enter (Shift+Enter inserts a line break).
    pub fn on_submit(mut self, handler: Handler<ClickEvent>) -> Self {
        self.0.on_submit = Some(handler);
        self
    }

    /// The return key's label on touch keyboards: Done, Go, Next, Search, Send.
    pub fn return_key(self, key: crate::controls::ReturnKey) -> Self {
        self.input_config(|c| c.return_key = key)
    }

    /// Called with the rectangle of the caret or the selection (#217)
    /// when it moves or the text under it changes: place a formatting
    /// toolbar or a link popover above the selected words.
    /// [`rect`](crate::SelectionRectEvent::rect) is in the field's
    /// coordinates; [`frame`](crate::SelectionRectEvent::frame) in the
    /// window's, like [`App::frame_of`](crate::App::frame_of). To anchor a
    /// popover, wrap the field in a container (laid out at the field's
    /// origin: no padding) and put the popover in an absolutely positioned
    /// element at `rect`:
    ///
    /// ```text
    /// div()
    ///     .child(rich_input(runs).on_selection_rect(cx.listener(|this, e: &SelectionRectEvent, _| {
    ///         this.toolbar = (!e.selection.is_caret()).then_some(e.rect)
    ///     })))
    ///     .children(self.toolbar.map(|r| {
    ///         div().absolute().left(r.x).top(r.y).w(r.width).h(r.height)
    ///             .child(popover(toolbar).open(true).side(PopoverSide::Top).take_focus(false))
    ///     }))
    /// ```
    ///
    /// Web: the selection's client rectangles; macOS: the layout manager's
    /// glyph box, the caret's `firstRectForCharacterRange`; iOS, iPadOS,
    /// visionOS: `UITextView`'s `caretRect` and `selectionRects`; Linux:
    /// the text view's iter locations; Android: the `Layout`'s line bounds;
    /// Windows: RichEdit's `ITextRange::GetRect`. A selection over several lines is the box around them. tvOS and
    /// watchOS report none.
    pub fn on_selection_rect(mut self, handler: Handler<crate::SelectionRectEvent>) -> Self {
        self.0.edit.on_selection_rect = Some(handler);
        self
    }

    /// Keeps the selection's rectangle for
    /// [`App::selection_rect`](crate::App::selection_rect) (with the
    /// field's `.id()`), without a handler (#217).
    pub fn track_selection_rect(mut self) -> Self {
        self.0.edit.track_selection_rect = true;
        self
    }

    /// Called when the user copies (⌘C, Ctrl+C, the Edit or context menu)
    /// with text selected (#217). [`CopyEvent::write`](crate::CopyEvent::write)
    /// puts the app's own content on the clipboard (Markdown, HTML, a type
    /// of its own, built from [`spans`](crate::CopyEvent::spans)); without
    /// it the selection's plain text is copied.
    ///
    /// ```text
    /// rich_input(runs).on_copy(cx.listener(|_, e: &CopyEvent, _| {
    ///     e.write(ClipboardContent::text(&e.text).markdown(to_markdown(&e.spans)))
    /// }))
    /// ```
    ///
    /// Web: the `copy` event (the plain text is set at once; the app's
    /// content follows through the async Clipboard API, which Safari may
    /// refuse outside the key press); macOS: `copy:` of the text view
    /// (`NSPasteboard`, every type); iOS, iPadOS, visionOS: `copy:`
    /// (`UIPasteboard`, every type); Linux: the text view's `copy-clipboard`
    /// (every type); Android: the text field's copy action (every type: the
    /// clip's text and HTML, the others in its description and extras);
    /// Windows: the `RichEditBox`'s `CopyingToClipboard` (every
    /// type). tvOS and watchOS have no clipboard.
    pub fn on_copy(mut self, handler: Handler<crate::CopyEvent>) -> Self {
        self.0.edit.on_copy = Some(handler);
        self
    }

    /// Called when the user cuts (⌘X) with text selected (#217): like
    /// [`on_copy`](Self::on_copy) (`e.cut` is true); the selection is then
    /// deleted, which [`on_input`](Self::on_input) reports.
    pub fn on_cut(mut self, handler: Handler<crate::CopyEvent>) -> Self {
        self.0.edit.on_cut = Some(handler);
        self
    }

    /// Called when the user pastes (⌘V, the Edit or context menu) with
    /// what the clipboard holds (#217): its plain text, its HTML and its
    /// types ([`PasteEvent`](crate::PasteEvent)).
    /// [`prevent_default`](crate::PasteEvent::prevent_default) takes the
    /// paste over (split a multi-line text into blocks, upload an image);
    /// otherwise the plain text is inserted over the selection.
    ///
    /// Web: the `paste` event's `clipboardData`; macOS: `paste:` and the
    /// general pasteboard (every type's text data); iOS, iPadOS, visionOS:
    /// `paste:` and `UIPasteboard`; Linux: the text view's `paste-clipboard`
    /// (plain text, HTML and the clipboard's types); Android: the text
    /// field's paste action (plain text, HTML, types); Windows: the
    /// `RichEditBox`'s `Paste` (text, HTML, types). tvOS and watchOS have
    /// no clipboard.
    pub fn on_paste(mut self, handler: Handler<crate::PasteEvent>) -> Self {
        self.0.edit.on_paste = Some(handler);
        self
    }

    /// Called on a click with a modifier key held on a span while the
    /// field is being edited (#217): open a link with ⌘-click, follow a
    /// mention. [`span`](crate::SpanClickEvent::span) is its index in the
    /// spans given to [`rich_input`]. A plain click still places the caret;
    /// with this handler a modifier-click is the app's, instead of moving
    /// the caret or running a link's own handler. Web: ⌘, Ctrl or ⌥;
    /// macOS: ⌘ or ⌥ (⌃-click stays the context menu); Linux and Windows:
    /// Ctrl or Alt.
    /// Touch screens have no modifier-click: iOS, iPadOS, Android, tvOS and
    /// watchOS report none.
    pub fn on_span_click(mut self, handler: Handler<crate::SpanClickEvent>) -> Self {
        self.0.edit.on_span_click = Some(handler);
        self
    }

    /// Called for each click in the field (#266), with the character under
    /// the pointer (`e.text_offset`), the keys held and the click count;
    /// the field still places its caret. A ⇧-click on a field that is not
    /// focused can extend a selection to that point from another field of a
    /// [`text_flow`](crate::text_flow). The web, macOS, Linux and Windows;
    /// touch screens report none.
    pub fn on_click(mut self, handler: Handler<ClickEvent>) -> Self {
        self.0.edit.on_click = Some(handler);
        self
    }
}

/// A multi-line text field. It grows with its text, then scrolls once it
/// reaches its `max_h`: `text_area().value(&self.notes).max_h(px(160.))`.
pub fn text_area() -> TextArea {
    TextArea(VNode::new(NodeKind::TextArea))
}

impl TextArea {
    /// Its minimum height, in lines (#235): 3 by default. `rows(1)` starts
    /// like a one-line field and grows with its text (up to its `max_h`):
    /// a title, a table cell, a label that may be long.
    pub fn rows(self, rows: u32) -> Self {
        self.input_config(|c| c.rows = Some(rows.max(1)))
    }

    /// Called when this node takes or loses the keyboard focus (#224):
    /// commit a field's value when the user tabs away or clicks elsewhere,
    /// not on every keystroke. `e.focused` says which. For the node itself
    /// (a field, or a view focusable through `on_key_down`); use
    /// `focus_within` styles for a group.
    pub fn on_focus_change(mut self, handler: Handler<FocusEvent>) -> Self {
        self.0.on_focus_change = Some(handler);
        self
    }

    /// The controlled value of the field.
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.0.text = value.into();
        self
    }

    /// Text shown while the field is empty.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.0.placeholder = placeholder.into();
        self
    }

    /// Called with the new text on every edit.
    pub fn on_input(mut self, handler: Handler<InputEvent>) -> Self {
        self.0.on_input = Some(handler);
        self
    }
}

/// A native slider, `0.0..=1.0` unless given a [`range`](Slider::range):
/// `slider(self.volume).range(0., 100.).step(1.).on_change(cx.listener(|this, e, _| this.volume = e.value))`.
/// Adjustable with assistive technologies (swipe up/down, arrow keys).
/// Give it a label: `.accessibility_label("Volume")`.
pub fn slider(value: f64) -> Slider {
    let mut n = VNode::new(NodeKind::Slider);
    n.value = Some(value);
    n.config = crate::controls::SliderConfig::default().encode();
    Slider(n)
}

impl Slider {
    fn slider_config(mut self, f: impl FnOnce(&mut crate::controls::SliderConfig)) -> Self {
        let mut c = crate::controls::SliderConfig::decode(&self.0.config);
        f(&mut c);
        self.0.config = c.encode();
        self
    }

    /// The lowest and highest values (swapped if given in the wrong order).
    pub fn range(self, min: f64, max: f64) -> Self {
        self.slider_config(|c| {
            c.min = min.min(max);
            c.max = max.max(min);
        })
    }

    /// Values snap to multiples of `step` from the minimum (0: continuous).
    pub fn step(self, step: f64) -> Self {
        self.slider_config(|c| c.step = step.max(0.))
    }

    /// Called while the value changes (dragging, keys, assistive technologies).
    pub fn on_change(mut self, handler: Handler<ChangeEvent>) -> Self {
        self.0.on_change = Some(handler);
        self
    }
}

/// A native menu picker showing the selected option:
/// `picker(["Small", "Medium", "Large"], Some(self.size)).on_change(cx.listener(|this, e, _| this.size = e.index()))`.
/// Give it a label: `.accessibility_label("Size")`.
pub fn picker<S: Into<String>>(
    options: impl IntoIterator<Item = S>,
    selected: Option<usize>,
) -> Picker {
    let mut n = VNode::new(NodeKind::Picker);
    let c = crate::controls::PickerConfig {
        options: options.into_iter().map(Into::into).collect(),
    };
    n.config = c.encode();
    n.value = Some(selected.map_or(-1., |i| i as f64));
    Picker(n)
}

impl Picker {
    /// Shown while nothing is selected.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.0.placeholder = placeholder.into();
        self
    }

    /// Called with the chosen index (`e.index()`).
    pub fn on_change(mut self, handler: Handler<ChangeEvent>) -> Self {
        self.0.on_change = Some(handler);
        self
    }
}

/// A native date picker (`.time()` / `.date_time()` for the others):
/// `date_picker(self.due).on_change(cx.listener(|this, e, _| this.due = e.date()))`.
/// Values are instants; the picker shows them in the device's time zone.
pub fn date_picker(value: std::time::SystemTime) -> DatePicker {
    let mut n = VNode::new(NodeKind::DatePicker);
    n.value = Some(unix_seconds(value).round());
    n.config = crate::controls::DateConfig::default().encode();
    DatePicker(n)
}

/// A date picker that may be empty (#89): `None` shows its
/// [`placeholder`](DatePicker::placeholder), and the user can clear a date
/// (the browser's own clear, a clear button, "Clear" in the dialog).
/// Starting to edit an empty picker fills it with the current moment
/// (within the bounds):
/// `date_picker_opt(self.published).placeholder("Not published").on_change(cx.listener(|this, e, _| this.published = e.date_opt()))`.
pub fn date_picker_opt(value: Option<std::time::SystemTime>) -> DatePicker {
    let mut n = VNode::new(NodeKind::DatePicker);
    n.value = Some(value.map_or(f64::NAN, |v| unix_seconds(v).round()));
    n.config = crate::controls::DateConfig {
        optional: true,
        ..Default::default()
    }
    .encode();
    DatePicker(n)
}

impl DatePicker {
    fn date_config(mut self, f: impl FnOnce(&mut crate::controls::DateConfig)) -> Self {
        let mut c = crate::controls::DateConfig::decode(&self.0.config);
        f(&mut c);
        self.0.config = c.encode();
        self
    }

    /// What the picker edits: a day, a time of day or both.
    pub fn mode(self, mode: crate::controls::DateMode) -> Self {
        self.date_config(|c| c.mode = mode)
    }

    /// A time of day.
    pub fn time(self) -> Self {
        self.mode(crate::controls::DateMode::Time)
    }

    /// A day and a time.
    pub fn date_time(self) -> Self {
        self.mode(crate::controls::DateMode::DateTime)
    }

    /// The earliest selectable moment.
    pub fn min(self, t: std::time::SystemTime) -> Self {
        self.date_config(|c| c.min = Some(unix_seconds(t).round()))
    }

    /// The latest selectable moment.
    pub fn max(self, t: std::time::SystemTime) -> Self {
        self.date_config(|c| c.max = Some(unix_seconds(t).round()))
    }

    /// Text shown while an optional picker ([`date_picker_opt`],
    /// [`time_picker_opt`]) is empty; also its default accessibility label.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.0.placeholder = placeholder.into();
        self
    }

    /// Called with the new instant (`e.date()`; `e.date_opt()` for an
    /// optional picker).
    pub fn on_change(mut self, handler: Handler<ChangeEvent>) -> Self {
        self.0.on_change = Some(handler);
        self
    }
}

/// An indeterminate activity indicator (`UIActivityIndicatorView`, spinning
/// `NSProgressIndicator`, `GtkSpinner`, indeterminate `ProgressBar`,
/// SwiftUI `ProgressView`, a CSS spinner). Tinted by `text_color`. Give it
/// a label (`.accessibility_label("Loading")`) unless text next to it says so.
pub fn spinner() -> Spinner {
    let mut n = VNode::new(NodeKind::Spinner);
    n.value = Some(0.);
    Spinner(n)
}

impl Spinner {
    /// The large size.
    pub fn large(mut self) -> Self {
        self.0.value = Some(1.);
        self
    }
}

/// A screen with a title, shown in the native navigation bar.
/// A route's view should return one as its root.
pub fn screen(title: impl Into<String>) -> Screen {
    let mut n = VNode::new(NodeKind::Screen);
    n.text = title.into();
    n.style.flex_grow = 1.;
    // 1: bare (no header bar while it is on top), see `Screen::bare`.
    n.value = Some(0.);
    Screen(n)
}

impl ParentElement for Screen {
    fn children_mut(&mut self) -> &mut Vec<Element> {
        &mut self.0.children
    }
}

impl Screen {
    /// Sets the title shown in the navigation bar.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.0.text = title.into();
        self
    }

    /// Hides the stack's header bar while this screen is on top (a screen
    /// with its own header, a full-bleed photo, a sign-in page). Its title
    /// still names the window and the page for assistive technologies.
    /// Every screen at once: [`RouterBuilder::header`](crate::RouterBuilder::header).
    pub fn bare(mut self) -> Self {
        self.0.value = Some(1.);
        self
    }
}

/// A navigation stack. Prefer [`Router`](crate::Router), which manages one for you.
pub fn stack() -> Stack {
    let mut n = VNode::new(NodeKind::Stack);
    n.style.flex_grow = 1.;
    Stack(n)
}

impl ParentElement for Stack {
    fn children_mut(&mut self) -> &mut Vec<Element> {
        &mut self.0.children
    }
}

impl Stack {
    /// The current path, mirrored to the URL bar on the web.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.0.text = path.into();
        self
    }

    /// Called when the user goes back natively (swipe, back button); [`BackEvent::depth`] screens remain.
    pub fn on_back(mut self, handler: Handler<BackEvent>) -> Self {
        self.0.on_back = Some(handler);
        self
    }

    /// Called when the host asks for a path (browser forward button).
    pub fn on_navigate(mut self, handler: Handler<NavigateEvent>) -> Self {
        self.0.on_navigate = Some(handler);
        self
    }
}

/// A native on/off switch: `toggle(self.wifi).on_change(cx.listener(|this, e, _| this.wifi = e.checked()))`.
/// Give it a label: `.accessibility_label("Wi-Fi")`.
pub fn toggle(on: bool) -> Toggle {
    let mut n = VNode::new(NodeKind::Toggle);
    n.value = Some(if on { 1. } else { 0. });
    Toggle(n)
}

impl Toggle {
    /// Called when the user flips the switch ([`ChangeEvent::checked`]).
    pub fn on_change(mut self, handler: Handler<ChangeEvent>) -> Self {
        self.0.on_change = Some(handler);
        self
    }
}

/// A vertically scrolling container. By default it fills the space left in
/// its parent (`flex: 1 1 0`) and scrolls what does not fit; use
/// [`Scroll::fit`] for one as tall as its content up to a `max_h`.
pub fn scroll() -> Scroll {
    let mut n = VNode::new(NodeKind::Scroll);
    n.style.flex_grow = 1.;
    n.style.flex_shrink = 1.;
    n.style.flex_basis = crate::style::Length::Px(0.);
    Scroll(n)
}

/// A horizontally scrolling row (chips, a wide table on a phone): as tall
/// as its content; its children keep their size instead of shrinking to
/// the view. Shorthand for `scroll().horizontal()`.
pub fn scroll_x() -> Scroll {
    scroll().horizontal()
}

impl Scroll {
    /// Scrolls leading to trailing instead of top to bottom: the children
    /// form a row, keep their width (they never shrink) and the view is as
    /// tall as they are.
    pub fn horizontal(mut self) -> Self {
        let s = &mut self.0.style;
        s.scroll_axis = crate::layout::ScrollAxis::Horizontal;
        s.direction = crate::style::FlexDirection::Row;
        s.flex_grow = 0.;
        s.flex_basis = crate::style::Length::Auto;
        self
    }

    /// How the scroll bars show (#132): [`Scrollbar::Overlay`] keeps
    /// them thin and over the content, [`Scrollbar::Hidden`] removes them
    /// (the view still scrolls), for areas whose design has none: a
    /// sidebar, a canvas, a row of tabs. Keep them where users need to
    /// see how long the content is.
    ///
    /// [`Scrollbar::Overlay`]: crate::Scrollbar::Overlay
    /// [`Scrollbar::Hidden`]: crate::Scrollbar::Hidden
    pub fn scrollbar(mut self, bar: crate::layout::Scrollbar) -> Self {
        self.0.style.scrollbar = bar;
        self
    }

    /// Called as the content scrolls (virtualized lists, parallax…).
    pub fn on_scroll(mut self, handler: Handler<ScrollEvent>) -> Self {
        self.0.on_scroll = Some(handler);
        self
    }

    /// A drop target for dragged payloads of `kind` (#251), like
    /// [`Div::on_drop_payload`]: the position is in the content, scrolling
    /// included (a column of cards finds the slot from it).
    pub fn on_drop_payload(
        self,
        kind: impl Into<String>,
        handler: Handler<crate::events::DropEvent>,
    ) -> Self {
        Scroll(Div(self.0).on_drop_payload(kind, handler).0)
    }

    /// Hears payloads of `kind` dragged over it (#251), like
    /// [`Div::on_drag_over`], the position in the content.
    pub fn on_drag_over(
        self,
        kind: impl Into<String>,
        handler: Handler<crate::events::DragOverEvent>,
    ) -> Self {
        Scroll(Div(self.0).on_drag_over(kind, handler).0)
    }

    /// Pull to refresh (iOS refresh control; pull gesture and a Refresh
    /// button on the web; SwiftUI's `refreshable` on watchOS; a trackpad
    /// pull past the top or ⌘R on macOS; a pull at the top, F5 or Ctrl+R on
    /// Android; F5 or Ctrl+R on Linux; WinUI's `RefreshContainer`, a touch
    /// pull, and F5 or Ctrl+R on Windows). Show progress with
    /// [`Scroll::refreshing`].
    pub fn on_refresh(mut self, handler: Handler<ClickEvent>) -> Self {
        self.0.on_click = Some(handler);
        self
    }

    /// Whether a refresh is in progress (spinner visible).
    pub fn refreshing(mut self, on: bool) -> Self {
        self.0.value = Some(if on { 1. } else { 0. });
        self
    }

    /// Sizes the scroll view to its content instead of filling its parent:
    /// as tall as what it holds, up to its `max_h`, then it scrolls
    /// (`flex: 0 1 auto`). For lists in sheets, cards and popovers:
    /// `scroll().fit().max_h(px(320.))`. It still shrinks when its parent
    /// is shorter than its content.
    pub fn fit(mut self) -> Self {
        self.0.style.flex_grow = 0.;
        self.0.style.flex_shrink = 1.;
        self.0.style.flex_basis = crate::style::Length::Auto;
        self
    }
}

impl ParentElement for Scroll {
    fn children_mut(&mut self) -> &mut Vec<Element> {
        &mut self.0.children
    }
}

/// A tab bar. Prefer `Router::new().tab(...)`, which manages one for you.
pub fn tabs(selected: usize) -> Tabs {
    let mut n = VNode::new(NodeKind::Tabs);
    n.value = Some(selected as f64);
    n.style.flex_grow = 1.;
    Tabs(n)
}

impl ParentElement for Tabs {
    fn children_mut(&mut self) -> &mut Vec<Element> {
        &mut self.0.children
    }
}

impl Tabs {
    /// Called when the user picks a tab ([`ChangeEvent::index`]).
    pub fn on_change(mut self, handler: Handler<ChangeEvent>) -> Self {
        self.0.on_change = Some(handler);
        self
    }

    /// The current path, mirrored to the URL bar on the web.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.0.text = path.into();
        self
    }
}

/// One tab: a title and an icon (SF Symbol name, e.g. "checklist").
pub fn tab(title: impl Into<String>, icon: impl Into<String>) -> Tab {
    let mut n = VNode::new(NodeKind::Tab);
    n.text = title.into();
    n.icon = icon.into();
    n.style.flex_grow = 1.;
    Tab(n)
}

impl ParentElement for Tab {
    fn children_mut(&mut self) -> &mut Vec<Element> {
        &mut self.0.children
    }
}

/// An SF Symbol (`"checkmark.circle.fill"`). Decorative unless given an
/// `accessibility_label`. On the web, common symbols map to glyphs.
pub fn image(symbol: impl Into<String>) -> Image {
    let mut n = VNode::new(NodeKind::Image);
    n.icon = symbol.into();
    n.a11y.hidden = true;
    Image(n)
}

impl Image {
    /// Announces the image (it is decorative by default).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.0.a11y.hidden = false;
        self.0.a11y.role = Some(Role::Image);
        self.0.a11y.label = Some(label.into());
        self
    }
}

/// A progress bar, `fraction` in `0.0..=1.0`.
pub fn progress(fraction: f64) -> Progress {
    let mut n = VNode::new(NodeKind::Progress);
    n.value = Some(fraction.clamp(0., 1.));
    n.a11y.value = Some(format!("{}%", (fraction.clamp(0., 1.) * 100.).round()));
    Progress(n)
}

/// A countdown to `until` that keeps ticking on its own (`12:34`).
pub fn timer(until: std::time::SystemTime) -> Timer {
    let secs = until
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0., |d| d.as_secs_f64());
    let mut n = VNode::new(NodeKind::Timer);
    n.value = Some(secs);
    n.text = format_remaining(secs);
    Timer(n)
}

impl Timer {
    /// What the timer shows once its end has passed, instead of `0:00`
    /// (`"Departed"`, `"—"`). Every backend and static surface (widgets,
    /// Live Activities) switches to it on its own when the countdown ends.
    pub fn after(mut self, text: impl Into<String>) -> Self {
        self.0.placeholder = text.into();
        if let Some(end) = self.0.value {
            self.0.text = timer_text(end, &self.0.placeholder);
        }
        self
    }
}

/// A timer's text: the time left until the Unix time `end`
/// ([`format_remaining`]), or `after` once it shows `0:00` (when `after` is
/// not empty, see [`Timer::after`]).
pub fn timer_text(end: f64, after: &str) -> String {
    let text = format_remaining(end);
    if !after.is_empty() && text == "0:00" {
        after.to_string()
    } else {
        text
    }
}

/// `m:ss` (or `h:mm:ss`) until the Unix time `end`.
pub fn format_remaining(end: f64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0., |d| d.as_secs_f64());
    let left = (end - now).max(0.).round() as u64;
    let (h, m, s) = (left / 3600, (left / 60) % 60, left % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A modal sheet presenting `content` (usually a stack).
pub fn modal() -> Modal {
    let mut n = VNode::new(NodeKind::Modal);
    n.style.flex_grow = 1.;
    Modal(n)
}

impl ParentElement for Modal {
    fn children_mut(&mut self) -> &mut Vec<Element> {
        &mut self.0.children
    }
}

impl Modal {
    /// Called when the user dismisses the sheet natively.
    pub fn on_dismiss(mut self, handler: Handler<BackEvent>) -> Self {
        self.0.on_back = Some(handler);
        self
    }
}

impl Stack {
    /// Shows a Close button on the first screen (stacks inside sheets).
    pub fn closable(mut self, closable: bool) -> Self {
        self.set_flag(STACK_CLOSABLE, closable);
        self
    }

    /// Shows or hides the header bar (title, Back/Close) above every screen
    /// (default: shown). Screens then get the whole area; Back stays
    /// available natively and through `cx.back()`.
    pub fn header(mut self, shown: bool) -> Self {
        self.set_flag(STACK_NO_HEADER, !shown);
        self
    }

    fn set_flag(&mut self, flag: u32, on: bool) {
        let bits = self.0.value.unwrap_or(0.) as u32;
        let bits = if on { bits | flag } else { bits & !flag };
        self.0.value = Some(bits as f64);
    }
}

/// A stack's value is a set of flags: bit 1, a Close button on the first
/// screen ([`Stack::closable`]).
pub const STACK_CLOSABLE: u32 = 1;
/// Bit 2 of a stack's value: no header bar ([`Stack::header`]).
pub const STACK_NO_HEADER: u32 = 2;

/// A picture's [`object_position`](Picture::object_position) from its
/// config (#218): `(x, y)` in fractions, the center when unset.
pub fn picture_position(config: &str) -> (f32, f32) {
    let mut it = config.split_whitespace().map(|v| v.parse::<f32>().ok());
    match (it.next().flatten(), it.next().flatten()) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => (x.clamp(0., 1.), y.clamp(0., 1.)),
        _ => (0.5, 0.5),
    }
}

/// The part of an image of `image` size that covers a frame of `frame`
/// size around the focal point `at` (#218): `(x, y, width, height)` in
/// fractions of the image, from its top-left corner. What
/// [`fill`](Picture::fill) shows with an
/// [`object_position`](Picture::object_position).
pub fn cover_crop(image: (f32, f32), frame: (f32, f32), at: (f32, f32)) -> (f32, f32, f32, f32) {
    let (iw, ih) = image;
    let (fw, fh) = frame;
    if iw <= 0. || ih <= 0. || fw <= 0. || fh <= 0. {
        return (0., 0., 1., 1.);
    }
    // The image scaled to cover: the share of each side the frame shows.
    let scale = (fw / iw).max(fh / ih);
    let w = (fw / (iw * scale)).min(1.);
    let h = (fh / (ih * scale)).min(1.);
    ((1. - w) * at.0, (1. - h) * at.1, w, h)
}

/// An image from a URL (`https://…`) or an app asset (`"logo.png"`, served
/// from `assets/`). Decorative unless given [`Picture::alt`].
pub fn picture(src: impl Into<String>) -> Picture {
    let mut n = VNode::new(NodeKind::Picture);
    n.text = media_url(src.into());
    n.value = Some(0.);
    n.a11y.hidden = true;
    Picture(n)
}

/// An image from bytes in memory (`image/png`, `image/jpeg`, `image/gif`,
/// `image/webp`…: what the platform decodes), sent as a `data:` URL. For
/// SVG (`image/svg+xml`) this is [`svg`]: drawn as vectors everywhere.
///
/// ```ignore
/// picture_data(png_bytes, "image/png").alt("Avatar").size(px(48.))
/// ```
pub fn picture_data(bytes: impl AsRef<[u8]>, mime: &str) -> Picture {
    let bytes = bytes.as_ref();
    if mime.trim().eq_ignore_ascii_case("image/svg+xml") {
        return svg(String::from_utf8_lossy(bytes));
    }
    picture(format!("data:{};base64,{}", mime.trim(), base64(bytes)))
}

/// An SVG document (a server-made QR code, an icon), drawn natively as
/// vector paths on every backend, widgets included (see
/// [`crate::svg`](mod@crate::svg) for what is supported). It is as wide as
/// the SVG's `width` (or `viewBox`) and keeps its proportions: change the
/// width (`.w(px(64.))`) and the height follows. Decorative unless given
/// [`Picture::alt`]. Unreadable markup draws nothing.
pub fn svg(markup: impl AsRef<str>) -> Picture {
    let mut n = VNode::new(NodeKind::Shape);
    n.a11y.hidden = true;
    match crate::svg::parse(markup.as_ref()) {
        Some(s) => {
            n.text = s.drawing.encode();
            n.style.width = crate::style::Length::Px(s.width);
            if s.width > 0. && s.height > 0. {
                n.style.aspect_ratio = Some(s.width / s.height);
            }
        }
        None => n.text = "i0,0\n".into(),
    }
    Picture(n)
}

/// Standard base64 with padding (`data:` URLs).
pub(crate) fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl Picture {
    /// Describes the image for screen readers (and makes it non-decorative).
    pub fn alt(mut self, alt: impl Into<String>) -> Self {
        self.0.a11y.hidden = false;
        self.0.a11y.label = Some(alt.into());
        if self.0.kind == NodeKind::Shape {
            // An SVG picture: announced as an image.
            self.0.a11y.role = Some(Role::Image);
        }
        self
    }
    /// Paints every path of an [`svg`] picture in `color`, each keeping its
    /// own opacity: a monochrome icon that hard-codes its color, tinted.
    /// [`Color::current()`](crate::Color::current) follows the node's
    /// [`text_color`](crate::Styled::text_color); an adaptive color follows
    /// dark mode. (An SVG painted in `currentColor` needs no tint:
    /// `.text_color(…)` is enough.) No effect on bitmap pictures.
    pub fn tint(mut self, color: crate::style::Color) -> Self {
        if self.0.kind == NodeKind::Shape {
            self.0.text = crate::shape::Drawing::decode(&self.0.text)
                .tinted(color)
                .encode();
        }
        self
    }
    /// Scales to fit inside the frame (letterboxed).
    pub fn fit(mut self) -> Self {
        self.0.value = Some(1.);
        self
    }
    /// Scales to fill the frame, cropping (default).
    pub fn fill(mut self) -> Self {
        self.0.value = Some(0.);
        self
    }
    /// The point a [`fill`](Self::fill)ed picture is cropped around (#218),
    /// like CSS `object-position`: `x` and `y` from 0 (left, top) to 1
    /// (right, bottom), the center (0.5, 0.5) by default. A cover's focal
    /// point, which the user can drag. No effect with [`fit`](Self::fit).
    pub fn object_position(mut self, x: f32, y: f32) -> Self {
        let (x, y) = (x.clamp(0., 1.), y.clamp(0., 1.));
        self.0.config = if (x, y) == (0.5, 0.5) {
            String::new()
        } else {
            format!("{x} {y}")
        };
        self
    }
}

// --- Pictures behind authentication (#32) ---------------------------------------------------

impl Picture {
    /// Sends an HTTP header with the request for a remote picture
    /// (`https://…`), such as a bearer token for a per-user image:
    ///
    /// ```ignore
    /// picture(format!("{api}/me/avatar.png"))
    ///     .header("Authorization", format!("Bearer {access_token}"))
    /// ```
    ///
    /// Native backends fetch it themselves with the header (and keep it out
    /// of the on-disk image cache). On the web the server fetches it: the
    /// page gets a same-origin URL that only this browser's session can load,
    /// and the header never reaches the browser. Ignored for assets, files,
    /// `data:` URLs and SVG. `name` must be an HTTP token (else the header is
    /// dropped); control characters leave `value`.
    pub fn header(mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        if self.0.kind == NodeKind::Picture {
            push_header(&mut self.0.text, name.as_ref(), value.as_ref());
        }
        self
    }
}

/// A picture or video URL as its node keeps it: without control characters
/// (browsers drop line breaks and tabs from URLs too), so only a header
/// added with `header` starts a new line.
pub(crate) fn media_url(src: String) -> String {
    if src.contains(char::is_control) {
        src.replace(char::is_control, "")
    } else {
        src
    }
}

/// Adds header `name: value` to a remote media source: the source is the
/// URL, then one `name: value` line per header ([`picture_source`]). The
/// name must be an HTTP token; control characters leave the value.
pub(crate) fn push_header(text: &mut String, name: &str, value: &str) {
    let name = name.trim();
    let token = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b));
    let remote = text.starts_with("http://") || text.starts_with("https://");
    if token && remote {
        text.push('\n');
        text.push_str(name);
        text.push_str(": ");
        text.push_str(media_url(value.to_string()).trim());
    }
}

/// A picture's source split into its URL and the headers given with
/// [`Picture::header`] (backends load it with them).
pub fn picture_source(text: &str) -> (&str, Vec<(&str, &str)>) {
    let mut lines = text.split('\n');
    let url = lines.next().unwrap_or_default();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim(), v.trim()))
        .filter(|(k, _)| !k.is_empty())
        .collect();
    (url, headers)
}

/// A vector drawing (lines, curves, arcs; see [`crate::shape`](mod@crate::shape)). It has no
/// natural size: give it one (`.size(…)`, `.flex_grow(1.)`). Decorative
/// unless given [`Shape::label`].
pub fn shape() -> Shape {
    let mut n = VNode::new(NodeKind::Shape);
    n.a11y.hidden = true;
    n.text = "i0,0\n".into();
    Shape(n)
}

impl Shape {
    /// Adds a path, drawn over the previous ones.
    pub fn path(mut self, path: crate::shape::Path) -> Self {
        path.encode(&mut self.0.text);
        self
    }
    /// Adds several paths, in order.
    pub fn paths(self, paths: impl IntoIterator<Item = crate::shape::Path>) -> Self {
        paths.into_iter().fold(self, Shape::path)
    }
    /// Points kept free around the drawing, horizontally and vertically, so
    /// thick strokes are not clipped at the frame's edges. (Named apart from
    /// [`Styled::inset`], which positions an absolute element.)
    pub fn stroke_inset(mut self, x: f32, y: f32) -> Self {
        let rest = self
            .0
            .text
            .split_once('\n')
            .map_or("", |(_, r)| r)
            .to_string();
        self.0.text = format!("i{x},{y}\n{rest}");
        self
    }
    /// Announces the drawing (a chart summary: "Sales, 3 to 12").
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.0.a11y.hidden = false;
        self.0.a11y.role = Some(Role::Image);
        self.0.a11y.label = Some(label.into());
        self
    }
}

// --- Pointer and drag (#17) --------------------------------------------------

impl Div {
    /// Called when the pointer (mouse, trackpad, pen; Apple Vision Pro
    /// gaze) comes over the container or leaves it. Touch screens have no
    /// hover: do not hide anything essential behind it.
    pub fn on_hover(mut self, handler: Handler<crate::events::HoverEvent>) -> Self {
        self.0.on_hover = Some(handler);
        self
    }

    /// Called as the pointer moves over the container, with its position
    /// in the container (a chart read-out that follows the pointer). On
    /// touch screens it is reported while a finger touches it.
    pub fn on_pointer_move(mut self, handler: Handler<crate::events::PointerEvent>) -> Self {
        self.0.on_pointer_move = Some(handler);
        self
    }

    /// Called as the container is dragged (mouse, finger, pen): `Start`
    /// once the pointer moved past the platform's slop, `Move`s, then `End`
    /// or `Cancel`. Enough for drag to reorder or a custom slider (with
    /// [`on_click`](Div::on_click)'s position for taps). A drag does not
    /// also click. Also offer the action another way (buttons, keys): not
    /// every user can drag.
    pub fn on_drag(mut self, handler: Handler<crate::events::DragEvent>) -> Self {
        self.0.on_drag = Some(handler);
        self
    }

    /// Which mouse buttons start an [`on_drag`](Div::on_drag) (#136): by
    /// default the primary one. `&[PointerButton::Middle]` pans a canvas
    /// with the wheel pressed while the left button selects; the platform's
    /// own use of that button (autoscroll, pasting, opening a link in a new
    /// tab) is then left out over the element. Fingers and pens are the
    /// primary button. [`DragEvent::button`](crate::DragEvent::button) says
    /// which one drags.
    pub fn drag_buttons(mut self, buttons: &[crate::events::PointerButton]) -> Self {
        self.0.drag_buttons = buttons.iter().fold(0, |bits, b| bits | b.drag_bit());
        self
    }

    /// On touch screens (fingers and pens), the [`on_drag`](Div::on_drag)
    /// starts only once the press is held still for `delay` (#243): « long
    /// press to pick up, then drag », with the platform's lift haptic. Until
    /// then a swipe scrolls what is around it (a card in a horizontally
    /// scrolling board, a row in a list); once lifted, the drag takes over
    /// from any scroll view around it. A mouse or a trackpad still drags on
    /// movement. iOS, iPadOS and visionOS, Android, the web on touch
    /// screens, Linux and Windows touch screens; macOS has no touch screen.
    ///
    /// ```ignore
    /// card.on_drag(move_card).drag_after_long_press(Duration::from_millis(400))
    /// ```
    pub fn drag_after_long_press(mut self, delay: std::time::Duration) -> Self {
        self.0.drag_delay_ms = delay.as_millis().clamp(1, u32::MAX as u128) as u32;
        self
    }

    /// Called after a layout that moved or resized the container, with its
    /// frame in the window's layout coordinates before scrolling (#192, see
    /// [`Rect`](crate::Rect)): hit-test a rubber-band selection against
    /// rows, find the drop slot under the pointer. Also keeps the frame for
    /// [`App::frame_of`](crate::App::frame_of) when it has an `.id()`. The
    /// web reports it from the page's layout, native backends from Prism's.
    pub fn on_layout(mut self, handler: Handler<crate::events::LayoutEvent>) -> Self {
        self.0.on_layout = Some(handler);
        self
    }

    /// Keeps the container's frame for [`App::frame_of`](crate::App::frame_of)
    /// (with its `.id()`), without a handler (#192).
    pub fn track_layout(mut self) -> Self {
        self.0.track_layout = true;
        self
    }

    /// Called as the mouse wheel turns or two fingers scroll on a trackpad
    /// over the container (#133): pan an infinite canvas, a map, an image.
    /// The container takes the events: what is behind it does not scroll.
    /// Touch screens have no wheel (their panning is
    /// [`on_drag`](Div::on_drag)); an iPad's trackpad or mouse scrolls here.
    pub fn on_wheel(mut self, handler: Handler<crate::events::WheelEvent>) -> Self {
        self.0.on_wheel = Some(handler);
        self
    }

    /// Called for a click with the middle button (pressing the wheel), or
    /// the back or forward button of a mouse (#215): close a tab with the
    /// middle button, open a row in a new tab. `e.button` says which.
    /// `on_click` stays for the primary button, the context menu for the
    /// secondary one. On the web the middle button's autoscroll is kept
    /// from starting over the element. Desktop and the web: touch screens
    /// have no such buttons.
    ///
    /// ```ignore
    /// tab.on_aux_click(cx.listener(move |this, e: &ClickEvent, cx| {
    ///     if e.button == PointerButton::Middle { this.close_tab(i, cx) }
    /// }))
    /// ```
    pub fn on_aux_click(mut self, handler: Handler<ClickEvent>) -> Self {
        self.0.on_aux_click = Some(handler);
        self
    }

    /// Called when this node takes or loses the keyboard focus (#224):
    /// commit a field's value when the user tabs away or clicks elsewhere,
    /// not on every keystroke. `e.focused` says which. For the node itself
    /// (a field, or a view focusable through `on_key_down`); use
    /// `focus_within` styles for a group.
    pub fn on_focus_change(mut self, handler: Handler<FocusEvent>) -> Self {
        self.0.on_focus_change = Some(handler);
        self
    }

    /// Called as two fingers pinch over the container (#133): zoom a canvas
    /// around [`MagnifyEvent::position`](crate::MagnifyEvent::position). The
    /// container takes the pinch (the page does not zoom). On a touch screen
    /// (#172) it runs alongside [`on_drag`](Div::on_drag): one finger drags,
    /// and during a pinch the drag follows the fingers' center.
    pub fn on_magnify(mut self, handler: Handler<crate::events::MagnifyEvent>) -> Self {
        self.0.on_magnify = Some(handler);
        self
    }

    /// Files dragged from the system (the desktop, the Files app, another
    /// app) and dropped on the container (#111): `types` are MIME types
    /// (`"image/*"`) or extensions (`".pdf"`), empty for any file, like
    /// [`PickFiles::types`](crate::PickFiles::types). The handler gets the
    /// dropped files it accepts (it is not called when none is); style
    /// the highlight while files hover with
    /// [`drop_hover`](crate::Styled::drop_hover). On the web the browser
    /// uploads the files over the app's connection first, like
    /// [`App::pick_files`](crate::AsyncApp::pick_files). Keep a way to
    /// choose files too: phones rarely drag them, and
    /// [`Capabilities::drop_files`](crate::Capabilities::drop_files) is
    /// `false` where nothing can be dropped (tvOS, watchOS).
    pub fn on_drop_files<S: Into<String>>(
        mut self,
        types: impl IntoIterator<Item = S>,
        handler: Handler<crate::events::FileDropEvent>,
    ) -> Self {
        self.0.on_drop_files = Some((crate::device::PickFiles::new().types(types), handler));
        self
    }

    /// Called while files dropped on the container
    /// ([`on_drop_files`](Div::on_drop_files)) reach the app (#199): on the
    /// web the browser sends them over the app's connection, which takes a
    /// while for a video or a batch of photos. The first call (`sent == 0`)
    /// comes as the drop starts uploading, the last one when every byte
    /// arrived, just before `on_drop_files`. Natively the files are on the
    /// device already: it is not called.
    pub fn on_drop_progress(mut self, handler: Handler<crate::events::DropProgress>) -> Self {
        self.0.on_drop_progress = Some(handler);
        self
    }

    /// Lets the container be dragged onto drop targets inside the app
    /// (#251): a card into another column, a library item onto a canvas, a
    /// row into a tree. While it is dragged a snapshot follows the pointer
    /// (unless [`DragPreview::None`](crate::DragPreview::None)), the target
    /// under it that accepts `payload.kind` gets its
    /// [`on_drag_over`](Div::on_drag_over) and
    /// [`drop_hover`](crate::Styled::drop_hover) style, a scroll view
    /// scrolls when the pointer stays near its edge, and the release calls
    /// the target's [`on_drop_payload`](Div::on_drop_payload). Positions
    /// come in the target's coordinates, scrolling included: no geometry on
    /// the app's side. It starts like [`on_drag`](Div::on_drag) (past the
    /// platform's slop, with the buttons of
    /// [`drag_buttons`](Div::drag_buttons)), which still runs alongside.
    /// Also offer the move another way (a menu, keys): not every user can
    /// drag.
    pub fn draggable(mut self, payload: crate::events::DragPayload) -> Self {
        self.0.drag_payload = Some(payload);
        self
    }

    /// Called when a [`draggable`](Div::draggable) payload of `kind` is
    /// released over the container (#251), with where, in the container
    /// (in its content for a scroll view). The innermost target under the
    /// pointer that accepts the kind takes it. Call it again for other
    /// kinds.
    pub fn on_drop_payload(
        mut self,
        kind: impl Into<String>,
        handler: Handler<crate::events::DropEvent>,
    ) -> Self {
        let kind = kind.into();
        self.0.on_drop_payload.retain(|(k, _)| *k != kind);
        self.0.on_drop_payload.push((kind, handler));
        self
    }

    /// Called as a [`draggable`](Div::draggable) payload of `kind` comes
    /// over the container, moves over it and leaves it (#251): the live
    /// position for an insertion line, in the container (in its content
    /// for a scroll view). A container with this handler is a drop target
    /// for `kind` even without [`on_drop_payload`](Div::on_drop_payload).
    pub fn on_drag_over(
        mut self,
        kind: impl Into<String>,
        handler: Handler<crate::events::DragOverEvent>,
    ) -> Self {
        let kind = kind.into();
        self.0.on_drag_over.retain(|(k, _)| *k != kind);
        self.0.on_drag_over.push((kind, handler));
        self
    }
}

// --- Text input extras (#18) --------------------------------------------------

impl TextInput {
    /// The most characters the field accepts (`maxlength`, a length
    /// filter natively).
    pub fn max_length(self, max: u32) -> Self {
        self.input_config(|c| c.max_length = Some(max))
    }

    /// An icon inside the field, before the text (an SF Symbol name such as
    /// `"magnifyingglass"`, mapped to the platform's icons): `leftView`,
    /// `GtkEntry` primary icon, a compound drawable, a glyph on the web.
    pub fn leading_icon(mut self, symbol: impl Into<String>) -> Self {
        self.0.icon = symbol.into();
        self
    }

    /// Values offered as the user types (web `<datalist>`, the platform's
    /// completion list natively). The user can still type anything else.
    pub fn suggestions<S: Into<String>>(
        mut self,
        suggestions: impl IntoIterator<Item = S>,
    ) -> Self {
        let list: Vec<String> = suggestions.into_iter().map(Into::into).collect();
        self.0.config = crate::controls::with_suggestions(&self.0.config, &list);
        self
    }
}

// --- Checkbox and radio group (#18) ------------------------------------------

builder!(
    /// A native checkbox. See [`checkbox`].
    Checkbox
);
builder!(
    /// A group of native radio buttons. See [`radio_group`].
    RadioGroup
);

/// A native checkbox (`NSButton` checkbox, `GtkCheckButton`, `CheckBox`,
/// `<input type=checkbox>`; drawn on iOS, tvOS and visionOS, which have
/// none, with the toggle-button trait):
/// `checkbox(self.remember).label("Remember me").on_change(cx.listener(|this, e, _| this.remember = e.checked()))`.
pub fn checkbox(checked: bool) -> Checkbox {
    let mut n = VNode::new(NodeKind::Checkbox);
    n.value = Some(if checked { 1. } else { 0. });
    Checkbox(n)
}

impl Checkbox {
    /// The text next to the box (also its accessibility label); clicking it
    /// toggles the box.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.0.text = label.into();
        self
    }

    /// Called when the user checks or unchecks it ([`ChangeEvent::checked`]).
    pub fn on_change(mut self, handler: Handler<ChangeEvent>) -> Self {
        self.0.on_change = Some(handler);
        self
    }
}

/// Native radio buttons, one per option, as one group (arrow keys move
/// between them; screen readers announce "1 of 3"):
/// `radio_group(["Daily", "Weekly"], Some(self.every)).on_change(cx.listener(|this, e, _| this.every = e.index()))`.
/// Give it a label: `.accessibility_label("Frequency")`.
pub fn radio_group<S: Into<String>>(
    options: impl IntoIterator<Item = S>,
    selected: Option<usize>,
) -> RadioGroup {
    let mut n = VNode::new(NodeKind::RadioGroup);
    n.config = crate::controls::RadioConfig {
        options: options.into_iter().map(Into::into).collect(),
        horizontal: false,
    }
    .encode();
    n.value = Some(selected.map_or(-1., |i| i as f64));
    RadioGroup(n)
}

impl RadioGroup {
    /// Options side by side instead of stacked.
    pub fn horizontal(mut self) -> Self {
        let mut c = crate::controls::RadioConfig::decode(&self.0.config);
        c.horizontal = true;
        self.0.config = c.encode();
        self
    }

    /// Called with the chosen index ([`ChangeEvent::index`]).
    pub fn on_change(mut self, handler: Handler<ChangeEvent>) -> Self {
        self.0.on_change = Some(handler);
        self
    }
}

// --- Time of day (#18) -------------------------------------------------------

/// A native picker for a time of day that does not depend on the time
/// zone (an alarm, opening hours): `minutes` since midnight
/// (`7 * 60 + 30` is 07:30). Read the new value with
/// [`ChangeEvent::minutes`].
pub fn time_picker(minutes: u32) -> DatePicker {
    time_of_day(Some(minutes), false)
}

/// A [`time_picker`] that may be empty (#89): `None` shows its
/// [`placeholder`](DatePicker::placeholder), and the user can clear the
/// time. Read the new value with [`ChangeEvent::minutes_opt`].
pub fn time_picker_opt(minutes: Option<u32>) -> DatePicker {
    time_of_day(minutes, true)
}

fn time_of_day(minutes: Option<u32>, optional: bool) -> DatePicker {
    let mut n = VNode::new(NodeKind::DatePicker);
    n.value = Some(minutes.map_or(f64::NAN, |m| f64::from(m.min(24 * 60 - 1)) * 60.));
    n.config = crate::controls::DateConfig {
        mode: crate::controls::DateMode::TimeOfDay,
        optional,
        ..Default::default()
    }
    .encode();
    DatePicker(n)
}

impl ChangeEvent {
    /// A [`time_picker`]'s new value, in minutes since midnight (0 when an
    /// optional picker was cleared: read those with
    /// [`minutes_opt`](Self::minutes_opt)).
    pub fn minutes(&self) -> u32 {
        self.minutes_opt().unwrap_or(0)
    }

    /// A [`time_picker_opt`]'s new value, in minutes since midnight: `None`
    /// when the user cleared it.
    pub fn minutes_opt(&self) -> Option<u32> {
        self.value
            .is_finite()
            .then(|| ((self.value / 60.).round().max(0.) as u32) % (24 * 60))
    }
}

// --- Overlays (#19) ----------------------------------------------------------

builder!(
    /// A sheet, popover or dialog. See [`sheet`], [`popover`], [`dialog`].
    Overlay
);

fn overlay(kind: crate::controls::OverlayKind, content: Element) -> Overlay {
    let mut n = VNode::new(NodeKind::Overlay);
    n.config = crate::controls::OverlayConfig {
        kind,
        ..Default::default()
    }
    .encode();
    n.value = Some(0.);
    n.children.push(content);
    Overlay(n)
}

/// `content` in a sheet over the app, shown while [`open`](Overlay::open)
/// is true: `sheet(gallery).open(self.picking).on_dismiss(cx.listener(|this, _, _| this.picking = false))`.
/// Place it anywhere in the screen: it covers the whole window (bars and
/// tabs too), keeps the keyboard focus inside, and closes on Escape, Back
/// or a swipe down (then [`on_dismiss`](Overlay::on_dismiss) runs).
pub fn sheet(content: impl IntoElement) -> Overlay {
    overlay(crate::controls::OverlayKind::Sheet, content.into_element())
}

/// `content` in a popover anchored to the element that contains the
/// popover: `div().child(button("Filters")).child(popover(filters).open(self.filters))`.
/// It closes on Escape or a click outside. Phones may present it as a
/// sheet.
pub fn popover(content: impl IntoElement) -> Overlay {
    overlay(
        crate::controls::OverlayKind::Popover,
        content.into_element(),
    )
}

/// `content` in a centered modal dialog over a dimmed window.
pub fn dialog(content: impl IntoElement) -> Overlay {
    overlay(crate::controls::OverlayKind::Dialog, content.into_element())
}

/// `content` in a bar above the on-screen keyboard, shown while a text
/// field in the element containing it has the focus (#242):
/// `div().child(editor).child(keyboard_toolbar(format_bar))`, or
/// [`Div::keyboard_toolbar`]. A field's own bar
/// ([`TextInput::keyboard_toolbar`]) wins over its container's, and a
/// nearer container's over a farther one's.
///
/// The bar is laid out at the window's width; give it its own height,
/// padding and background. Taps on its buttons keep the field focused and
/// the keyboard up.
///
/// - iOS, iPadOS: the field's `inputAccessoryView` (above a hardware
///   keyboard's shortcut bar too).
/// - Android: a bar docked above the IME (window insets), while the field
///   has the focus.
/// - Web: on touch screens (`pointer: coarse`), a bar fixed to the bottom
///   of the visual viewport, which the on-screen keyboard shrinks
///   (`VisualViewport`); presses on it do not take the focus.
/// - macOS, Linux, Windows, tvOS, watchOS, visionOS, desktop browsers: no
///   on-screen keyboard bar; it is left out (its content is not built on
///   native desktops).
pub fn keyboard_toolbar(content: impl IntoElement) -> Overlay {
    let mut o = overlay(
        crate::controls::OverlayKind::Keyboard,
        content.into_element(),
    );
    o.0.value = Some(1.);
    o.0.style.platforms =
        Some(crate::Platform::Ios | crate::Platform::Android | crate::Platform::Web);
    o
}

impl Overlay {
    fn overlay_config(mut self, f: impl FnOnce(&mut crate::controls::OverlayConfig)) -> Self {
        let mut c = crate::controls::OverlayConfig::decode(&self.0.config);
        f(&mut c);
        self.0.config = c.encode();
        self
    }

    /// Shows the overlay (`true`) or closes it. Its content is only built
    /// while it is open.
    pub fn open(mut self, open: bool) -> Self {
        self.0.value = Some(if open { 1. } else { 0. });
        if !open {
            self.0.children.clear();
        }
        self
    }

    /// Called when the user closes it (Escape, Back, a tap outside, a
    /// swipe down): set your `open` state to false there.
    pub fn on_dismiss(mut self, handler: Handler<BackEvent>) -> Self {
        self.0.on_back = Some(handler);
        self
    }

    /// Whether Escape, Back, a tap outside or a swipe down close it (yes by
    /// default). A fixed overlay must offer its own way out.
    pub fn dismissible(self, dismissible: bool) -> Self {
        self.overlay_config(|c| c.fixed = !dismissible)
    }

    /// A sheet that starts at half height (a medium detent); it can be
    /// pulled up where the platform allows.
    pub fn half(self) -> Self {
        self.overlay_config(|c| c.medium = true)
    }

    /// `false`: a popover that a click or tap outside does not close (yes
    /// by default): only the app closes it (`open(false)`), or Escape
    /// while the focus is inside it, which calls
    /// [`on_dismiss`](Overlay::on_dismiss). Web: `popover="manual"`;
    /// macOS: an `NSPopover` with the application-defined behavior; iOS:
    /// the window stays interactive around it (passthrough); Linux: a
    /// `GtkPopover` without autohide; Android: a `PopupWindow` that ignores
    /// outside touches; Windows: a `Flyout` that lets presses through to the
    /// page (`OverlayInputPassThroughElement`). Sheets and dialogs ignore it (see
    /// [`dismissible`](Overlay::dismissible)).
    pub fn light_dismiss(self, light_dismiss: bool) -> Self {
        self.overlay_config(|c| c.manual = !light_dismiss)
    }

    /// `false`: a popover that leaves the keyboard focus where it is, so
    /// the text field it is anchored to keeps receiving keys, and that does
    /// not take the focus when its items are clicked (they still run their
    /// handlers). For inline suggestion menus (slash commands, mentions,
    /// autocomplete):
    ///
    /// ```text
    /// div()
    ///     .child(text_input().value(text).on_input(..))
    ///     .children(text.starts_with('/').then(|| {
    ///         popover(menu).open(true).take_focus(false).on_dismiss(close)
    ///     }))
    /// ```
    ///
    /// A press outside it, the anchor's field included, closes it
    /// (`on_dismiss` with [`DismissReason::Outside`](crate::DismissReason::Outside))
    /// and still reaches what it landed on: the caret moves, a button runs
    /// (#182). Typing in the field does not close it.
    /// [`light_dismiss(false)`](Overlay::light_dismiss) keeps it open until
    /// the app closes it. Keys stay with the field: handle Escape and the
    /// arrows there, with [`consume_keys`](crate::Div::consume_keys) (#181).
    /// Web: no focus move, presses inside keep the field focused; macOS: a
    /// non-activating child panel instead of an `NSPopover`; iOS: the field
    /// stays first responder (the keyboard stays up); Linux: a `GtkPopover`
    /// without autohide that cannot take the focus; Android: a
    /// non-focusable `PopupWindow`; Windows: a transient `Flyout` that takes
    /// no focus. Sheets and dialogs ignore it.
    pub fn take_focus(self, take_focus: bool) -> Self {
        self.overlay_config(|c| c.keep_focus = !take_focus)
    }

    /// The side of its anchor a popover opens on: below by default, above,
    /// or beside it ([`Trailing`](crate::PopoverSide::Trailing) is to the
    /// right in left-to-right layouts, to the left in right-to-left ones).
    /// It flips to the opposite side when the window has no room there. A
    /// submenu opens beside the item that opens it, tops lined up:
    ///
    /// ```ignore
    /// div()
    ///     .child(button("Turn into"))
    ///     .child(popover(kinds).open(self.turn_into).side(PopoverSide::Trailing))
    /// ```
    ///
    /// Web: placed by the client, like the default; macOS: an `NSPopover`
    /// on that edge, its arrow on the anchor (a `take_focus(false)` panel is
    /// placed exactly); iPad: the matching
    /// `permittedArrowDirections`; Linux: `GtkPopover::set_position`;
    /// Android: the `PopupWindow` is placed beside or above the anchor;
    /// Windows: the `Flyout`'s placement (WinUI flips it).
    /// Phones that present popovers as sheets ignore it, as do sheets.
    ///
    /// On a dialog it places the dialog beside its anchor instead of
    /// centering it (#79), lined up as [`align`](Overlay::align) says, and
    /// the dialog stays modal (focus kept inside, the rest out of reach,
    /// Escape and a click outside close it): a choice panel beside the
    /// column that asked for it, with
    /// [`backdrop(Backdrop::Clear)`](Overlay::backdrop) to keep the page in
    /// sight:
    ///
    /// ```text
    /// div()
    ///     .child(row("Colour", value).on_click(…))
    ///     .child(dialog(picker).open(self.picking)
    ///         .side(PopoverSide::Trailing)
    ///         .backdrop(Backdrop::Clear)
    ///         .size(320., 0.)
    ///         .on_dismiss(cx.listener(|this, _, _| this.picking = false)))
    /// ```
    ///
    /// A dialog with room on neither side (a phone) is centered as usual.
    /// Web: positioned beside the anchor, still `showModal`ed; macOS: an
    /// `NSPopover` on that edge instead of a sheet; iOS, iPadOS, visionOS:
    /// the dialog's card beside the anchor; Linux: a `GtkPopover` instead of
    /// a window; Android: the dialog's window beside the anchor; Windows: a
    /// `Flyout` on that side.
    pub fn side(self, side: crate::PopoverSide) -> Self {
        self.overlay_config(|c| {
            c.side = side;
            c.placed = c.kind == crate::OverlayKind::Dialog;
        })
    }

    /// How a popover lines up with its anchor along its
    /// [`side`](Overlay::side): leading edges (top edges beside the anchor)
    /// by default, centered, or trailing edges (bottom edges beside). A
    /// panel opened from a button at the end of a toolbar lines up with
    /// the button's trailing edge and grows towards the window's middle:
    /// `popover(panel).open(true).align(PopoverAlign::End)`. The popover
    /// still stays inside the window. Arrowed popovers (macOS, iPad) point
    /// at the anchor: they line up exactly when they are no longer than the
    /// anchor along that edge, and are centered on it otherwise.
    pub fn align(self, align: crate::PopoverAlign) -> Self {
        self.overlay_config(|c| c.align = align)
    }

    /// What a sheet or dialog shows over the page behind it: nothing
    /// ([`Backdrop::Clear`](crate::Backdrop::Clear), the page stays in
    /// sight, still out of reach while it is open), a plain dim, or a dim
    /// with a blur. Defaults: a blur on the web and tvOS, a dim elsewhere
    /// (macOS window sheets have none). Web: the `::backdrop`; iOS: the
    /// dialog's backdrop, and no dim under a clear sheet; Android: the
    /// dialog's dim, the blur from Android 12; Windows: the smoke (clear, a
    /// dim or an acrylic blur); macOS and Linux show no backdrop (sheets
    /// are windows). Popovers have none.
    pub fn backdrop(self, backdrop: crate::controls::Backdrop) -> Self {
        self.overlay_config(|c| c.backdrop = Some(backdrop))
    }

    /// A dialog or popover that draws no box of its own (#166, #287): no
    /// background, border, corners, arrow, shadow or entry animation, so
    /// its content is the whole panel, drawn by the app (a menu's own
    /// card). With [`side`](Overlay::side) and
    /// [`backdrop(Backdrop::Clear)`](Overlay::backdrop) a picker the app
    /// styles itself opens beside its anchor; the dialog stays modal
    /// (focus kept inside, Escape and a click outside close it).
    ///
    /// ```text
    /// dialog(div().bg(rgb(0x1c1c1e)).rounded(12.).shadow_lg().child(swatches))
    ///     .open(self.picking)
    ///     .side(PopoverSide::Trailing)
    ///     .backdrop(Backdrop::Clear)
    ///     .bare()
    /// ```
    ///
    /// Web: the `<dialog>` or popover without Prism's styles; macOS: a
    /// borderless, transparent panel instead of a sheet or an `NSPopover`;
    /// iOS, iPadOS, visionOS: a dialog is a clear card, shown without a
    /// cross-fade, a popover keeps its place and its taps without the
    /// popover's background and arrow (tvOS: a clear card); Android: a
    /// transparent window without animation; Linux: a `GtkPopover` without
    /// arrow or background, or an undecorated transparent window; Windows:
    /// the content without the panel or flyout, its edge, shadow or
    /// animation.
    /// Sheets keep their platform's box.
    pub fn bare(self) -> Self {
        self.overlay_config(|c| {
            c.bare = matches!(
                c.kind,
                crate::OverlayKind::Dialog | crate::OverlayKind::Popover
            )
        })
    }

    /// The content area's size in points, instead of the platform's
    /// default width (`0`: keep it) and the content's height (`0`: fit
    /// it). Kept inside the window; on phones a sheet stays full width.
    pub fn size(self, width: f32, height: f32) -> Self {
        let points = |v: f32| {
            if v.is_finite() {
                v.max(0.).round() as u32
            } else {
                0
            }
        };
        self.overlay_config(|c| {
            c.width = points(width);
            c.height = points(height);
        })
    }
}

// --- Navigation bar options (#30) ---

impl Screen {
    /// `false`: a small title in the bar even where stacks show large
    /// titles (iOS, iPadOS, visionOS), for a screen that draws its own hero;
    /// `true`: always large. Other platforms have one title size.
    pub fn large_title(mut self, large: bool) -> Self {
        let mut c = crate::navbar::ScreenConfig::decode(&self.0.config);
        c.large_title = Some(large);
        self.0.config = c.encode();
        self
    }

    /// Adds a button before the title (after Back, which stays):
    /// `UIBarButtonItem`s on Apple platforms, buttons in the header bar
    /// elsewhere. See [`bar_button`](crate::bar_button).
    pub fn leading(self, button: crate::navbar::BarButton) -> Self {
        self.bar_item(button, crate::navbar::BarPlacement::Leading)
    }

    /// Adds a button after the title ("Edit", "+"). See
    /// [`bar_button`](crate::bar_button).
    pub fn trailing(self, button: crate::navbar::BarButton) -> Self {
        self.bar_item(button, crate::navbar::BarPlacement::Trailing)
    }

    fn bar_item(
        mut self,
        button: crate::navbar::BarButton,
        placement: crate::navbar::BarPlacement,
    ) -> Self {
        let mut c = crate::navbar::ScreenConfig::decode(&self.0.config);
        let index = c.items.len();
        c.items.push(crate::navbar::BarItem {
            placement,
            ..button.item
        });
        self.0.config = c.encode();
        // A tap on button `i` is a change event of value `i` on the screen.
        let previous = self.0.on_change.take();
        let on_click = button.on_click;
        self.0.on_change = Some(Rc::new(move |e: &ChangeEvent, app: &mut App| {
            if e.index() == index && e.value >= 0. {
                if let Some(h) = &on_click {
                    h(&ClickEvent, app);
                }
            } else if let Some(p) = &previous {
                p(e, app);
            }
        }));
        self
    }
}

impl Tabs {
    /// The color of the selected tab (`UITabBar.tintColor`, the selected
    /// segment, the web's selected tab). The router sets it from
    /// `RouterBuilder::tint` or the app's accent color.
    pub fn tint(mut self, color: impl Into<Option<crate::style::Color>>) -> Self {
        self.0.style.tint = color.into();
        self
    }
}

impl Stack {
    /// The color of the header bar's buttons, Back included
    /// (`UINavigationBar.tintColor`, the buttons' content tint).
    pub fn tint(mut self, color: impl Into<Option<crate::style::Color>>) -> Self {
        self.0.style.tint = color.into();
        self
    }
}

// --- Widget buttons (#47) ---

macro_rules! widget_action {
    ($($ty:ident),*) => {$(
        impl $ty {
            /// In a widget or a Live Activity, a tap runs `action` with
            /// `payload` in the app ([`App::on_widget_action`](crate::App::on_widget_action))
            /// without opening it, then redraws the widgets: WidgetKit's
            /// `Button(intent:)` on iOS 17+, a broadcast on Android. Where a
            /// widget cannot run app code, the tap opens the app, which runs
            /// the action there (see the widgets chapter). In the app's own
            /// UI it does nothing: use `on_click` there.
            pub fn widget_action(mut self, action: impl Into<String>, payload: impl Into<String>) -> Self {
                self.0.widget_action = Some((action.into(), payload.into()));
                self
            }
        }
    )*};
}

widget_action!(Div, Text, Button);
