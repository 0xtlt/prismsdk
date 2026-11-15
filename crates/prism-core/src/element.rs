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
            NodeKind::Picture | NodeKind::Shape => Some(Role::Image),
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

    /// A date picker's new value.
    pub fn date(&self) -> std::time::SystemTime {
        unix_time(self.value)
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
            on_back: None,
            on_navigate: None,
            on_change: None,
            on_scroll: None,
            on_swipe_left: None,
            on_swipe_right: None,
            on_submit: None,
            id: None,
            autofocus: false,
            on_hover: None,
            on_pointer_move: None,
            on_drag: None,
            on_key_down: None,
            on_global_key_down: None,
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
        }
    };
}
text_entry!(TextInput);
text_entry!(TextArea);

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

    /// Called with the new instant (`e.date()`).
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

/// A vertically scrolling container. Give it a bounded height (e.g. `.flex_1()`).
pub fn scroll() -> Scroll {
    let mut n = VNode::new(NodeKind::Scroll);
    n.style.flex_grow = 1.;
    n.style.flex_shrink = 1.;
    n.style.flex_basis = crate::style::Length::Px(0.);
    Scroll(n)
}

impl Scroll {
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
        self.0.value = Some(if closable { 1. } else { 0. });
        self
    }
}

/// An image from a URL (`https://…`) or an app asset (`"logo.png"`, served
/// from `assets/`). Decorative unless given [`Picture::alt`].
pub fn picture(src: impl Into<String>) -> Picture {
    let mut n = VNode::new(NodeKind::Picture);
    n.text = src.into();
    n.value = Some(0.);
    n.a11y.hidden = true;
    Picture(n)
}

impl Picture {
    /// Describes the image for screen readers (and makes it non-decorative).
    pub fn alt(mut self, alt: impl Into<String>) -> Self {
        self.0.a11y.hidden = false;
        self.0.a11y.label = Some(alt.into());
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
    /// Points kept free around the drawing, horizontally and vertically.
    pub fn inset(mut self, x: f32, y: f32) -> Self {
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
    let mut n = VNode::new(NodeKind::DatePicker);
    n.value = Some(f64::from(minutes.min(24 * 60 - 1)) * 60.);
    n.config = crate::controls::DateConfig {
        mode: crate::controls::DateMode::TimeOfDay,
        min: None,
        max: None,
    }
    .encode();
    DatePicker(n)
}

impl ChangeEvent {
    /// A [`time_picker`]'s new value, in minutes since midnight.
    pub fn minutes(&self) -> u32 {
        ((self.value / 60.).round().max(0.) as u32) % (24 * 60)
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
}
