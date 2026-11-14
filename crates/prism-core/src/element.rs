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
}

impl NodeKind {
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
        }
    }

    /// The role a node of this kind has unless overridden.
    pub fn default_role(self) -> Option<Role> {
        match self {
            NodeKind::Button => Some(Role::Button),
            NodeKind::Toggle => Some(Role::Switch),
            NodeKind::Tabs => Some(Role::TabList),
            NodeKind::Tab => Some(Role::TabPanel),
            NodeKind::Text => Some(Role::Text),
            _ => None,
        }
    }
}

/// Fired when a clickable view or a button is activated.
#[derive(Clone, Debug, Default)]
pub struct ClickEvent;

/// Fired on every edit of a text input, with its new content.
#[derive(Clone, Debug)]
pub struct InputEvent {
    pub value: String,
}

/// Reported by a stack when the user went back natively (iOS swipe, browser
/// back button, macOS back button): only the first `depth` screens remain.
#[derive(Clone, Debug)]
pub struct BackEvent {
    pub depth: usize,
}

/// Reported by a stack when the host asks for a path (browser forward button).
#[derive(Clone, Debug)]
pub struct NavigateEvent {
    pub path: String,
}

/// Fired when a control's value changes: a toggle (0/1), a tab bar (index).
#[derive(Clone, Debug)]
pub struct ChangeEvent {
    pub value: f64,
}

impl ChangeEvent {
    pub fn checked(&self) -> bool {
        self.value != 0.
    }

    pub fn index(&self) -> usize {
        self.value.max(0.) as usize
    }
}

/// An event handler. Handlers receive the whole `App` so they can update any entity.
pub type Handler<E> = Rc<dyn Fn(&E, &mut App)>;

/// An unresolved element: host nodes plus references to entities, which are
/// rendered lazily when the tree is resolved.
pub enum Element {
    Node(Box<VNode>),
    Entity(AnyEntity),
    Fragment(Vec<Element>),
    /// Built when the tree is resolved, with access to the app (themes,
    /// globals). This is how stateless components are implemented.
    Lazy(Box<dyn FnOnce(&mut App) -> Element>),
    Empty,
}

/// An element built at resolve time with access to the [`App`]:
/// `lazy(|cx| text(cx.global::<Theme>().name))`.
pub fn lazy<E: IntoElement>(f: impl FnOnce(&mut App) -> E + 'static) -> Element {
    Element::Lazy(Box::new(move |app| f(app).into_element()))
}

/// A host node under construction.
pub struct VNode {
    pub kind: NodeKind,
    pub style: Style,
    pub text: String,
    pub placeholder: String,
    /// Icon name (SF Symbol on Apple platforms), used by tabs.
    pub icon: String,
    /// Numeric value: toggle state (0/1), selected tab index.
    pub value: Option<f64>,
    pub a11y: Accessibility,
    pub key: Option<String>,
    pub children: Vec<Element>,
    pub on_click: Option<Handler<ClickEvent>>,
    pub on_input: Option<Handler<InputEvent>>,
    pub on_back: Option<Handler<BackEvent>>,
    pub on_navigate: Option<Handler<NavigateEvent>>,
    pub on_change: Option<Handler<ChangeEvent>>,
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
            a11y: Accessibility::default(),
            key: None,
            children: Vec::new(),
            on_click: None,
            on_input: None,
            on_back: None,
            on_navigate: None,
            on_change: None,
        }
    }
}

/// Anything that can be placed in the tree.
pub trait IntoElement {
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
    fn children_mut(&mut self) -> &mut Vec<Element>;

    fn child(mut self, child: impl IntoElement) -> Self {
        self.children_mut().push(child.into_element());
        self
    }

    fn children<E: IntoElement>(mut self, children: impl IntoIterator<Item = E>) -> Self {
        self.children_mut()
            .extend(children.into_iter().map(IntoElement::into_element));
        self
    }
}

/// Conditional builder helpers (`.when(cond, |this| ...)`).
pub trait FluentBuilder: Sized {
    fn when(self, cond: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if cond { f(self) } else { self }
    }

    fn when_some<T>(self, value: Option<T>, f: impl FnOnce(Self, T) -> Self) -> Self {
        match value {
            Some(v) => f(self, v),
            None => self,
        }
    }

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

/// A flex container, laid out as a column by default.
pub fn div() -> Div {
    Div(VNode::new(NodeKind::View))
}

pub fn text(content: impl Into<String>) -> Text {
    let mut n = VNode::new(NodeKind::Text);
    n.text = content.into();
    Text(n)
}

pub fn button(label: impl Into<String>) -> Button {
    let mut n = VNode::new(NodeKind::Button);
    n.text = label.into();
    Button(n)
}

pub fn text_input() -> TextInput {
    TextInput(VNode::new(NodeKind::TextInput))
}

impl ParentElement for Div {
    fn children_mut(&mut self) -> &mut Vec<Element> {
        &mut self.0.children
    }
}

impl Div {
    /// Makes the whole container clickable.
    pub fn on_click(mut self, handler: Handler<ClickEvent>) -> Self {
        self.0.on_click = Some(handler);
        self
    }
}

impl Button {
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

    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.0.placeholder = placeholder.into();
        self
    }

    pub fn on_input(mut self, handler: Handler<InputEvent>) -> Self {
        self.0.on_input = Some(handler);
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

    pub fn on_back(mut self, handler: Handler<BackEvent>) -> Self {
        self.0.on_back = Some(handler);
        self
    }

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
