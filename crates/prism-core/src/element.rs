//! Element builders: the declarative tree that `Render::render` returns.

use std::rc::Rc;

use crate::a11y::{Accessibility, Accessible, Role};
use crate::app::{AnyEntity, App, Entity, Render};
use crate::style::{Style, Styled};

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
    /// `WKWebView` on Apple platforms, `android.webkit.WebView` on Android
    /// (see [`web_view`](crate::web_view())). Its text is the URL, its
    /// config a [`WebViewConfig`](crate::web_view::WebViewConfig). No
    /// natural size: give it one. Reports `UiEvent::WebMessage` and
    /// `UiEvent::WebLoad`.
    WebView,

    // --- Editable rich text (#115) ---
    /// An editable paragraph of styled runs and inline objects
    /// ([`rich_input`]): a `contenteditable` on the web, an `NSTextView`
    /// with attributes and attachments on macOS. Its text is the plain
    /// value (a chip is [`OBJECT_CHAR`](crate::OBJECT_CHAR)), its runs come
    /// in [`Patch::SetRichValue`](crate::Patch::SetRichValue), its config is
    /// an [`InputConfig`](crate::controls::InputConfig). Backends without
    /// rich editing show a text area of the plain value instead (see
    /// `prism_native::Widgets::rich_input`).
    RichInput,
}

impl NodeKind {
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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClickEvent {
    /// Where the pointer was, in the element's coordinates.
    pub position: Option<crate::events::Point>,
}

/// A click without a position (keyboard, assistive technologies), so
/// `ClickEvent` reads as a value too: `handler(&ClickEvent, cx)`.
#[allow(non_upper_case_globals)]
pub const ClickEvent: ClickEvent = ClickEvent { position: None };

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

/// Reported by a stack when the user went back natively (iOS swipe, browser
/// back button, macOS back button): only the first `depth` screens remain.
#[derive(Clone, Debug)]
pub struct BackEvent {
    /// How many screens remain on the stack.
    pub depth: usize,
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

/// `secs` since 1970 as a [`SystemTime`](std::time::SystemTime) (negative: before).
pub fn unix_time(secs: f64) -> std::time::SystemTime {
    let d = std::time::Duration::from_secs_f64(secs.abs());
    if secs >= 0. {
        std::time::UNIX_EPOCH + d
    } else {
        std::time::UNIX_EPOCH - d
    }
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
    /// Placeholder of a text input.
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
    /// Long press (iOS), right-click (macOS, web) menu.
    pub context_menu: Option<crate::menu::Menu>,
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

    // --- Drop targets (#111) ---
    /// Files dropped on the node: the types it accepts, and the handler.
    pub on_drop_files: Option<(
        crate::device::PickFiles,
        Handler<crate::events::FileDropEvent>,
    )>,
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
            key: None,
            children: Vec::new(),
            on_click: None,
            on_input: None,
            on_rich_input: None,
            on_select: None,
            selection: None,
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
            island: None,
            widget_action: None,
            on_scene_tap: None,
            scene: None,
            attributes: Vec::new(),
            on_web_message: None,
            on_web_load: None,
            on_drop_files: None,
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

            /// Called for every key pressed in the window while this element
            /// is mounted, whatever has the focus (Escape to close, arrows
            /// for a remote pad). While a text field has the focus, only
            /// keys that do not type (Escape, arrows, Enter, Tab, function
            /// keys) or come with Control / Command are reported.
            pub fn on_global_key_down(mut self, handler: Handler<crate::events::KeyEvent>) -> Self {
                self.0.on_global_key_down = Some(handler);
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
    /// Makes the container a link to an app path. Works everywhere: in the app
    /// (navigates), in widgets and Live Activities (opens the app there).
    pub fn link(mut self, path: impl Into<String>) -> Self {
        self.0.link = Some(path.into());
        self
    }

    /// A context menu: long press on iOS, right-click on macOS and the web
    /// (plus the Menu key / Shift+F10). Exposed to assistive technologies.
    pub fn context_menu(mut self, menu: crate::menu::Menu) -> Self {
        self.0.context_menu = Some(menu);
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
}

impl TextInput {
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
/// It wraps like a paragraph and grows with its content (cap it with
/// `max_h`). Enter calls [`on_submit`](RichInput::on_submit) and inserts
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
/// pasted text arrives plain. Rich editing runs on the web and macOS;
/// iOS, GTK, Android and watchOS show a plain multi-line field of the
/// value instead (styles not drawn, a chip shown as `U+FFFC`), whose edits
/// still come back as spans that keep the styles of the text around them.
pub fn rich_input(spans: impl IntoIterator<Item = crate::spans::Span>) -> RichInput {
    RichInput(VNode::new(NodeKind::RichInput)).spans(spans)
}

impl RichInput {
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
}

/// A multi-line text field. It grows with its text, then scrolls once it
/// reaches its `max_h`: `text_area().value(&self.notes).max_h(px(160.))`.
pub fn text_area() -> TextArea {
    TextArea(VNode::new(NodeKind::TextArea))
}

impl TextArea {
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

    /// Pull to refresh (iOS refresh control; pull gesture and a Refresh
    /// button on the web). Show progress with [`Scroll::refreshing`].
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

/// An image from a URL (`https://…`) or an app asset (`"logo.png"`, served
/// from `assets/`). Decorative unless given [`Picture::alt`].
pub fn picture(src: impl Into<String>) -> Picture {
    let mut n = VNode::new(NodeKind::Picture);
    n.text = src.into();
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
fn base64(bytes: &[u8]) -> String {
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
    /// `data:` URLs and SVG. Line breaks in `name` or `value` are dropped.
    pub fn header(mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        let clean = |s: &str| s.replace(['\r', '\n'], "");
        let (name, value) = (clean(name.as_ref()), clean(value.as_ref()));
        let remote = self.0.text.starts_with("http://") || self.0.text.starts_with("https://");
        if self.0.kind == NodeKind::Picture && remote && !name.trim().is_empty() {
            // The source is the URL, then one `name: value` line per header
            // (URLs and header values never contain line breaks).
            self.0.text.push('\n');
            self.0.text.push_str(name.trim());
            self.0.text.push_str(": ");
            self.0.text.push_str(value.trim());
        }
        self
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
    /// outside touches. Sheets and dialogs ignore it (see
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
    ///         popover(menu).open(true).light_dismiss(false).take_focus(false)
    ///     }))
    /// ```
    ///
    /// It implies [`light_dismiss(false)`](Overlay::light_dismiss): a click
    /// in the field would otherwise close it. Keys stay with the field:
    /// handle Escape and the arrows there. Web: no focus move, presses
    /// inside keep the field focused; macOS: a non-activating child panel
    /// instead of an `NSPopover`; iOS: the field stays first responder (the
    /// keyboard stays up); Linux: a `GtkPopover` without autohide that
    /// cannot take the focus; Android: a non-focusable `PopupWindow`.
    /// Sheets and dialogs ignore it.
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
    /// Android: the `PopupWindow` is placed beside or above the anchor.
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
    /// a window; Android: the dialog's window beside the anchor.
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
    /// dialog's dim, the blur from Android 12; macOS and Linux show no
    /// backdrop (sheets are windows). Popovers have none.
    pub fn backdrop(self, backdrop: crate::controls::Backdrop) -> Self {
        self.overlay_config(|c| c.backdrop = Some(backdrop))
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
