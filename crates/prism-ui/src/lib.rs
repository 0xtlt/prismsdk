//! # prism-ui
//!
//! A component library on top of Prism's primitives. Three levels of control:
//!
//! 1. **Theme** — colors, radius, type scale, app-wide: `cx.set_global(Theme::dark())`.
//! 2. **Props & refinements** — per instance: `ui::button("Save").danger().large()`,
//!    and `.style(|s| s.corner_radius = 0.)` for anything else.
//! 3. **Replace the component** — app-wide, keeping its props and behaviour:
//!    `Theme::light().override_component(|b: ui::Button, cx| my_button(b, cx))`,
//!    or write your own [`Component`] from scratch.
//!
//! Each control can also defer to the platform's native widget: `.native()` per
//! instance, or `Theme::prefer_native(true)` everywhere (UIButton/UISwitch,
//! NSButton/NSSwitch, `<button>`/`<input role=switch>`).
//!
//! Every component is accessible by construction: correct role, label, state
//! (checked, disabled), keyboard activation on the web, VoiceOver on Apple.
//!
//! ```ignore
//! use prism_ui as ui;
//! ui::card()
//!     .child(ui::heading("Settings"))
//!     .child(ui::switch("Wi-Fi", self.wifi).on_change(cx.listener(|this, e, _| this.wifi = e.checked())))
//!     .child(ui::button("Save").on_click(cx.listener(|this, _, _| this.save())))
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]

mod theme;

use std::rc::Rc;

use prism_core::{
    Accessible, App, ChangeEvent, ClickEvent, Div, Element, FluentBuilder, Handler, InputEvent,
    IntoElement, Length, ParentElement, Role, Style, Styled, div, lazy, px, text, text_input,
    toggle,
};

pub use theme::{Theme, set_theme, update_theme};

/// A stateless, themeable UI component. Implement it for your own components,
/// then `prism_ui::component!(MyComponent);` to use them as elements.
pub trait Component: Sized + 'static {
    /// The default rendering, used unless the theme overrides this component.
    fn render(self, cx: &mut App) -> Element;
}

/// Renders `component`, honouring a theme override.
pub fn render<C: Component>(component: C, cx: &mut App) -> Element {
    match Theme::of(cx).override_for::<C>() {
        Some(custom) => custom(component, cx),
        None => component.render(cx),
    }
}

/// Makes a [`Component`] usable as an element (`.child(my_component)`).
#[macro_export]
macro_rules! component {
    ($ty:ty) => {
        impl $crate::__IntoElement for $ty {
            fn into_element(self) -> $crate::__Element {
                $crate::__lazy(move |cx| $crate::render(self, cx))
            }
        }
    };
}

#[doc(hidden)]
pub use prism_core::{Element as __Element, IntoElement as __IntoElement, lazy as __lazy};

type StyleFn = Box<dyn FnOnce(&mut Style)>;

/// Per-instance style refinements, applied after the theme.
#[derive(Default)]
struct Refine(Vec<StyleFn>);

impl Refine {
    fn push(&mut self, f: impl FnOnce(&mut Style) + 'static) {
        self.0.push(Box::new(f));
    }

    fn apply<S: Styled>(self, mut el: S) -> S {
        for f in self.0 {
            f(el.style_mut());
        }
        el
    }
}

macro_rules! refinable {
    ($ty:ty) => {
        impl prism_core::FluentBuilder for $ty {}

        impl $ty {
            /// Adjusts the rendered root's style after theming.
            pub fn style(mut self, f: impl FnOnce(&mut prism_core::Style) + 'static) -> Self {
                self.refine.push(f);
                self
            }
            /// Appears from `pose` (see `prism_core::animation`).
            pub fn enter(self, pose: prism_core::Pose) -> Self {
                self.style(move |s| s.animations.enter = Some(pose))
            }
            /// Animates to `pose` when removed.
            pub fn exit(self, pose: prism_core::Pose) -> Self {
                self.style(move |s| s.animations.exit = Some(pose))
            }
            /// Animates its changes with `t`.
            pub fn transition(self, t: prism_core::Transition) -> Self {
                self.style(move |s| s.transition = Some(t))
            }
        }
    };
}

mod forms;
pub use forms::{
    DateField, Select, Slider, Spinner, TextArea, Toast, Toaster, date_field, date_field_opt,
    dismiss_toasts, select, show_toast, slider, spinner, text_area, toast, toaster,
};
pub use forms::{FileInput, FilesHandler, ProgressHandler, file_input};

mod charts;
pub mod qr;
mod qr_code;
pub use charts::{
    Band, BarChart, LineChart, PieChart, RefLine, Ring, Series, Sparkline, Stat, Ticks, bar_chart,
    line_chart, pie_chart, ring, sparkline, stat,
};
pub use qr::QrEcc;
pub use qr_code::{Qr, qr_code};

// --- Button -------------------------------------------------------------------

/// The look of a [`Button`] (and the tone of a [`Badge`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Variant {
    /// Filled with the theme's accent: the main action.
    #[default]
    Primary,
    /// Outlined on the alternate surface: a secondary action.
    Secondary,
    /// Accent-colored text without a background.
    Ghost,
    /// Filled with the theme's danger color: a destructive action.
    Danger,
}

/// The size of a [`Button`]: padding and font size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Size {
    /// Tighter padding, text 2 pt below the theme's font size.
    Small,
    /// The theme's font size (default).
    #[default]
    Medium,
    /// Larger padding, text 2 pt above the theme's font size.
    Large,
}

/// A themed button. Props are public so theme overrides can re-render them.
pub struct Button {
    /// Text shown on the button, also its accessibility label.
    pub label: String,
    /// Look: primary, secondary, ghost or danger.
    pub variant: Variant,
    /// Padding and font size.
    pub size: Size,
    /// Dimmed, ignores clicks, reported as disabled to assistive technologies.
    pub disabled: bool,
    /// Stretches to its container's width instead of its content's.
    pub full_width: bool,
    /// `Some(true)`: native control, `Some(false)`: custom-drawn, `None`: follow
    /// [`Theme::prefer_native`].
    pub native: Option<bool>,
    /// Called on click (not while disabled).
    pub on_click: Option<Handler<ClickEvent>>,
    /// Accessibility hint read after the label.
    pub hint: Option<String>,
    refine: Refine,
}

/// A themed button with `label` (primary, medium size).
pub fn button(label: impl Into<String>) -> Button {
    Button {
        label: label.into(),
        variant: Variant::Primary,
        size: Size::Medium,
        disabled: false,
        full_width: false,
        native: None,
        on_click: None,
        hint: None,
        refine: Refine::default(),
    }
}

impl Button {
    /// Sets the look.
    pub fn variant(mut self, v: Variant) -> Self {
        self.variant = v;
        self
    }
    /// Filled with the accent color (default).
    pub fn primary(self) -> Self {
        self.variant(Variant::Primary)
    }
    /// Outlined, for secondary actions.
    pub fn secondary(self) -> Self {
        self.variant(Variant::Secondary)
    }
    /// Text only, no background.
    pub fn ghost(self) -> Self {
        self.variant(Variant::Ghost)
    }
    /// Filled with the danger color, for destructive actions.
    pub fn danger(self) -> Self {
        self.variant(Variant::Danger)
    }
    /// Smaller padding and text.
    pub fn small(mut self) -> Self {
        self.size = Size::Small;
        self
    }
    /// Larger padding and text.
    pub fn large(mut self) -> Self {
        self.size = Size::Large;
        self
    }
    /// Dims the button and ignores clicks while `disabled` is true.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    /// Stretches to the container's width.
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }
    /// Use the platform's native button for this instance.
    pub fn native(mut self) -> Self {
        self.native = Some(true);
        self
    }
    /// Force the custom-drawn button even if the theme prefers native.
    pub fn custom(mut self) -> Self {
        self.native = Some(false);
        self
    }
    /// Accessibility hint read after the label.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
    /// Called on click.
    pub fn on_click(mut self, handler: Handler<ClickEvent>) -> Self {
        self.on_click = Some(handler);
        self
    }
}

refinable!(Button);

impl Component for Button {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        if self.native.unwrap_or(t.prefer_native) {
            let mut b = prism_core::button(self.label)
                .accessibility_disabled(self.disabled)
                .when_some(self.hint, |b, h| b.accessibility_hint(h));
            if let (Some(h), false) = (self.on_click, self.disabled) {
                b = b.on_click(h);
            }
            return self.refine.apply(b).into_element();
        }
        let (bg, fg, border) = match self.variant {
            Variant::Primary => (Some(t.accent), t.on_accent, None),
            Variant::Secondary => (Some(t.surface_alt), t.text, Some(t.border)),
            Variant::Ghost => (None, t.accent, None),
            Variant::Danger => (Some(t.danger), rgb_white(), None),
        };
        let (px_, py_, font) = match self.size {
            Size::Small => (10., 5., t.font_size - 2.),
            Size::Medium => (16., 9., t.font_size),
            Size::Large => (22., 13., t.font_size + 2.),
        };
        let mut el = div()
            .flex_row()
            .items_center()
            .justify_center()
            .px(px_)
            .py(py_)
            .rounded(t.radius)
            .accessibility_role(Role::Button)
            .accessibility_label(self.label.clone())
            .accessibility_disabled(self.disabled)
            .when_some(self.hint, |d, h| d.accessibility_hint(h))
            .when_some(bg, |d, c| d.bg(c))
            .when_some(border, |d, c| d.border(1.).border_color(c))
            .when(self.disabled, |d| d.opacity(0.45))
            .when(!self.full_width, |d| d.self_start())
            .child(
                text(self.label)
                    .text_color(fg)
                    .text_size(font)
                    .semibold()
                    .accessibility_hidden(),
            );
        if let (Some(h), false) = (self.on_click, self.disabled) {
            el = el.on_click(h);
            // Feedback under the pointer and while pressed.
            el = match self.variant {
                Variant::Ghost => {
                    let accent = t.accent;
                    el.hover(move |s| s.bg(accent.with_alpha(0.08)))
                        .active(move |s| s.bg(accent.with_alpha(0.16)))
                }
                _ => el.hover(|s| s.opacity(0.9)).active(|s| s.opacity(0.75)),
            };
        }
        self.refine.apply(el).into_element()
    }
}

component!(Button);

fn rgb_white() -> prism_core::Color {
    prism_core::Color::WHITE
}

// --- Switch ---------------------------------------------------------------------

/// An on/off switch with a label.
pub struct Switch {
    /// Text next to the switch, also its accessibility label.
    pub label: String,
    /// Whether the switch is on.
    pub on: bool,
    /// Dimmed, ignores changes, reported as disabled.
    pub disabled: bool,
    /// `Some(true)`: native control, `Some(false)`: custom-drawn, `None`: follow
    /// [`Theme::prefer_native`].
    pub native: Option<bool>,
    /// Called with the new state (`e.checked()`).
    pub on_change: Option<Handler<ChangeEvent>>,
    refine: Refine,
}

/// A switch labelled `label`, currently `on`.
pub fn switch(label: impl Into<String>, on: bool) -> Switch {
    Switch {
        label: label.into(),
        on,
        disabled: false,
        native: None,
        on_change: None,
        refine: Refine::default(),
    }
}

impl Switch {
    /// Called with the new state when toggled (`e.checked()`).
    pub fn on_change(mut self, handler: Handler<ChangeEvent>) -> Self {
        self.on_change = Some(handler);
        self
    }
    /// Dims the switch and ignores changes while `disabled` is true.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    /// Use the platform's native switch for this instance.
    pub fn native(mut self) -> Self {
        self.native = Some(true);
        self
    }
    /// Force the custom-drawn switch even if the theme prefers native.
    pub fn custom(mut self) -> Self {
        self.native = Some(false);
        self
    }
}

refinable!(Switch);

impl Component for Switch {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let row = |control: Element| {
            div()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(12.)
                .child(
                    text(self.label.clone())
                        .text_color(t.text)
                        .flex_1()
                        .accessibility_hidden(),
                )
                .child(control)
        };
        if self.native.unwrap_or(t.prefer_native) {
            let mut native = toggle(self.on)
                .accessibility_label(self.label.clone())
                .accessibility_disabled(self.disabled);
            if let (Some(h), false) = (self.on_change.clone(), self.disabled) {
                native = native.on_change(h);
            }
            return self.refine.apply(row(native.into_element())).into_element();
        }
        // Custom: a track with a thumb, the whole row is the control.
        let track = div()
            .w(px(44.))
            .h(px(26.))
            .p(2.)
            .rounded(13.)
            .flex_row()
            .bg(if self.on { t.accent } else { t.border })
            .when(self.on, |d| d.justify_end())
            .accessibility_hidden()
            .child(
                div()
                    .size(px(22.))
                    .rounded(11.)
                    .bg(prism_core::Color::WHITE),
            );
        let mut el = row(track.into_element())
            .accessibility_role(Role::Switch)
            .accessibility_label(self.label.clone())
            .accessibility_checked(self.on)
            .accessibility_disabled(self.disabled)
            .when(self.disabled, |d| d.opacity(0.45));
        if let (Some(h), false) = (self.on_change, self.disabled) {
            let next = if self.on { 0. } else { 1. };
            el = el.on_click(Rc::new(move |_: &ClickEvent, app: &mut App| {
                h(&ChangeEvent { value: next }, app)
            }));
        }
        self.refine.apply(el).into_element()
    }
}

component!(Switch);

// --- Checkbox -------------------------------------------------------------------

/// A check box with a label; the whole row toggles it.
pub struct Checkbox {
    /// Text next to the box, also its accessibility label.
    pub label: String,
    /// Whether the box is checked.
    pub checked: bool,
    /// Called with the new state when toggled (`e.checked()`).
    pub on_change: Option<Handler<ChangeEvent>>,
    refine: Refine,
}

/// A check box labelled `label`, currently `checked`.
pub fn checkbox(label: impl Into<String>, checked: bool) -> Checkbox {
    Checkbox {
        label: label.into(),
        checked,
        on_change: None,
        refine: Refine::default(),
    }
}

impl Checkbox {
    /// Called with the new state when toggled (`e.checked()`).
    pub fn on_change(mut self, handler: Handler<ChangeEvent>) -> Self {
        self.on_change = Some(handler);
        self
    }
}

refinable!(Checkbox);

impl Component for Checkbox {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let mut el = div()
            .flex_row()
            .items_center()
            .gap(10.)
            .accessibility_role(Role::Checkbox)
            .accessibility_label(self.label.clone())
            .accessibility_checked(self.checked)
            .child(
                div()
                    .size(px(22.))
                    .rounded(6.)
                    .items_center()
                    .justify_center()
                    .when(self.checked, |d| d.bg(t.accent))
                    .when(!self.checked, |d| d.border(2.).border_color(t.border))
                    .accessibility_hidden()
                    .when(self.checked, |d| {
                        d.child(text("✓").text_color(t.on_accent).bold().text_size(14.))
                    }),
            )
            .child(text(self.label).text_color(t.text).accessibility_hidden());
        if let Some(h) = self.on_change {
            let next = if self.checked { 0. } else { 1. };
            el = el.on_click(Rc::new(move |_: &ClickEvent, app: &mut App| {
                h(&ChangeEvent { value: next }, app)
            }));
        }
        self.refine.apply(el).into_element()
    }
}

component!(Checkbox);

// --- TextField ------------------------------------------------------------------

/// A labelled native text field with optional helper or error text.
pub struct TextField {
    /// Shown above the field, also its accessibility label.
    pub label: String,
    /// Current text.
    pub value: String,
    /// Shown while the field is empty.
    pub placeholder: String,
    /// Note under the field (hidden while `error` is set).
    pub helper: Option<String>,
    /// Error under the field, in the danger color, with a danger border.
    pub error: Option<String>,
    /// Password, keyboard, return key, capitalization, autocorrection.
    pub config: prism_core::controls::InputConfig,
    /// Called on every edit with the new text.
    pub on_input: Option<Handler<InputEvent>>,
    /// Called when the return key is pressed.
    pub on_submit: Option<Handler<ClickEvent>>,
    refine: Refine,
}

/// A text field labelled `label`, holding `value`.
pub fn text_field(label: impl Into<String>, value: impl Into<String>) -> TextField {
    TextField {
        label: label.into(),
        value: value.into(),
        placeholder: String::new(),
        helper: None,
        error: None,
        config: Default::default(),
        on_input: None,
        on_submit: None,
        refine: Refine::default(),
    }
}

impl TextField {
    /// Shown while the field is empty.
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = p.into();
        self
    }
    /// A note under the field.
    pub fn helper(mut self, h: impl Into<String>) -> Self {
        self.helper = Some(h.into());
        self
    }
    /// An error under the field (`None` clears it); it replaces the helper text.
    pub fn error(mut self, e: Option<impl Into<String>>) -> Self {
        self.error = e.map(Into::into);
        self
    }
    /// A password field (characters hidden, no autocorrection).
    pub fn secure(mut self) -> Self {
        self.config.secure = true;
        self
    }
    /// The on-screen keyboard layout.
    pub fn keyboard(mut self, k: prism_core::Keyboard) -> Self {
        self.config.keyboard = k;
        self
    }
    /// The label and action of the return key.
    pub fn return_key(mut self, k: prism_core::ReturnKey) -> Self {
        self.config.return_key = k;
        self
    }
    /// Automatic capitalization.
    pub fn capitalize(mut self, c: prism_core::Capitalize) -> Self {
        self.config.capitalize = c;
        self
    }
    /// Turns the platform's spelling correction on or off.
    pub fn autocorrect(mut self, on: bool) -> Self {
        self.config.autocorrect = on;
        self
    }
    /// The return key was pressed.
    pub fn on_submit(mut self, handler: Handler<ClickEvent>) -> Self {
        self.on_submit = Some(handler);
        self
    }
    /// Called on every edit with the new text.
    pub fn on_input(mut self, handler: Handler<InputEvent>) -> Self {
        self.on_input = Some(handler);
        self
    }
}

refinable!(TextField);

impl Component for TextField {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let note = self.error.clone().or(self.helper.clone());
        let mut input = text_input()
            .value(self.value)
            .placeholder(self.placeholder)
            .accessibility_label(self.label.clone())
            .when_some(note.clone(), |i, n| i.accessibility_hint(n))
            .when(self.error.is_some(), |i| {
                i.border(1.).border_color(t.danger).rounded(6.)
            });
        let c = self.config;
        input = input
            .secure(c.secure)
            .keyboard(c.keyboard)
            .return_key(c.return_key)
            .capitalize(c.capitalize)
            .autocorrect(c.autocorrect);
        if let Some(h) = self.on_submit {
            input = input.on_submit(h);
        }
        if let Some(h) = self.on_input {
            input = input.on_input(h);
        }
        let el = div()
            .gap(6.)
            .child(
                text(self.label)
                    .text_size(t.font_size - 2.)
                    .semibold()
                    .text_color(t.text)
                    .accessibility_hidden(),
            )
            .child(input)
            .when_some(note, |d, n| {
                d.child(
                    text(n)
                        .text_size(t.font_size - 3.)
                        .text_color(if self.error.is_some() {
                            t.danger
                        } else {
                            t.muted
                        })
                        .accessibility_hidden(),
                )
            });
        self.refine.apply(el).into_element()
    }
}

component!(TextField);

// --- Card, Heading, Badge, Divider, ListRow ---------------------------------------

/// A surface grouping related content.
pub struct Card {
    /// The card's content, stacked vertically.
    pub children: Vec<Element>,
    refine: Refine,
}

/// An empty card; add content with `.child(…)`.
pub fn card() -> Card {
    Card {
        children: Vec::new(),
        refine: Refine::default(),
    }
}

impl ParentElement for Card {
    fn children_mut(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

refinable!(Card);

impl Component for Card {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let el = div()
            .bg(t.surface)
            .rounded(t.radius + 4.)
            .p(t.spacing)
            .gap(12.)
            .border(1.)
            .border_color(t.border)
            // A faint elevation.
            .shadow(prism_core::Shadow::new(
                0.,
                1.,
                3.,
                prism_core::Color::BLACK.with_alpha(0.06),
            ))
            .children(self.children);
        self.refine.apply(el).into_element()
    }
}

component!(Card);

/// A bold heading, exposed as a heading to assistive technologies.
pub struct Heading {
    /// The heading's text.
    pub text: String,
    /// Heading level, 1 (largest) to 6.
    pub level: u8,
    refine: Refine,
}

/// A section heading (level 2). `.level(n)` for others.
pub fn heading(text: impl Into<String>) -> Heading {
    Heading {
        text: text.into(),
        level: 2,
        refine: Refine::default(),
    }
}

impl Heading {
    /// Sets the level (clamped to 1–6): 28, 20, 17 pt, then 15 pt from level 4.
    pub fn level(mut self, level: u8) -> Self {
        self.level = level.clamp(1, 6);
        self
    }
}

refinable!(Heading);

impl Component for Heading {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let size = match self.level {
            1 => 28.,
            2 => 20.,
            3 => 17.,
            _ => 15.,
        };
        let el = text(self.text)
            .text_size(size)
            .bold()
            .text_color(t.text)
            .heading(self.level);
        self.refine.apply(el).into_element()
    }
}

component!(Heading);

/// A small rounded label (a status, a count).
pub struct Badge {
    /// The badge's text.
    pub label: String,
    /// Colors: `Primary` uses the accent, `Danger` the danger color, others
    /// are muted.
    pub tone: Variant,
    refine: Refine,
}

/// A small status pill.
pub fn badge(label: impl Into<String>) -> Badge {
    Badge {
        label: label.into(),
        tone: Variant::Secondary,
        refine: Refine::default(),
    }
}

impl Badge {
    /// Sets the colors (see [`Badge::tone`](Badge#structfield.tone)).
    pub fn tone(mut self, tone: Variant) -> Self {
        self.tone = tone;
        self
    }
}

refinable!(Badge);

impl Component for Badge {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let (bg, fg) = match self.tone {
            Variant::Primary => (t.accent, t.on_accent),
            Variant::Danger => (t.danger, prism_core::Color::WHITE),
            _ => (t.surface_alt, t.muted),
        };
        let el = div()
            .self_start()
            .px(8.)
            .py(3.)
            .rounded(999.)
            .bg(bg)
            .child(text(self.label).text_size(12.).semibold().text_color(fg));
        self.refine.apply(el).into_element()
    }
}

component!(Badge);

/// A hairline separator, hidden from assistive technologies.
pub fn divider() -> Element {
    lazy(|cx| {
        let t = Theme::of(cx);
        div()
            .h(px(1.))
            .bg(t.border)
            .accessibility_hidden()
            .into_element()
    })
}

/// A tappable list row: title, optional subtitle and trailing detail, chevron.
pub struct ListRow {
    /// Opens on long press or right click.
    pub context_menu: Option<prism_core::Menu>,
    /// Called when the row is swiped left.
    pub swipe_left: Option<Handler<ClickEvent>>,
    /// Main text.
    pub title: String,
    /// Smaller, muted text under the title.
    pub subtitle: Option<String>,
    /// Muted text at the trailing edge.
    pub detail: Option<String>,
    /// A decorative glyph at the leading edge, in the accent color.
    pub leading: Option<String>,
    /// Called on tap; the row then shows a chevron and acts as a button.
    pub on_click: Option<Handler<ClickEvent>>,
    refine: Refine,
}

/// A list row titled `title`.
pub fn list_row(title: impl Into<String>) -> ListRow {
    ListRow {
        context_menu: None,
        swipe_left: None,
        title: title.into(),
        subtitle: None,
        detail: None,
        leading: None,
        on_click: None,
        refine: Refine::default(),
    }
}

impl ListRow {
    /// Smaller, muted text under the title.
    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = Some(s.into());
        self
    }
    /// Muted text at the trailing edge (a value, a count).
    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }
    /// A leading glyph (decorative).
    pub fn leading(mut self, glyph: impl Into<String>) -> Self {
        self.leading = Some(glyph.into());
        self
    }
    /// Called on tap; adds a chevron and makes the row a button.
    pub fn on_click(mut self, handler: Handler<ClickEvent>) -> Self {
        self.on_click = Some(handler);
        self
    }
    /// Swipe left (e.g. delete). Also offer it in the context menu.
    pub fn on_swipe_left(mut self, handler: Handler<ClickEvent>) -> Self {
        self.swipe_left = Some(handler);
        self
    }

    /// Long press / right-click menu.
    pub fn context_menu(mut self, menu: prism_core::Menu) -> Self {
        self.context_menu = Some(menu);
        self
    }
}

refinable!(ListRow);

impl Component for ListRow {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let label = [
            Some(self.title.clone()),
            self.subtitle.clone(),
            self.detail.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
        let clickable = self.on_click.is_some();
        let mut el = div()
            .flex_row()
            .items_center()
            .gap(12.)
            .py(10.)
            .min_h(Length::Px(44.))
            .accessibility_label(label)
            .when_some(self.leading, |d, g| {
                d.child(
                    text(g)
                        .text_color(t.accent)
                        .text_size(18.)
                        .accessibility_hidden(),
                )
            })
            .child(
                div()
                    .flex_1()
                    .gap(2.)
                    .child(text(self.title).text_color(t.text))
                    .when_some(self.subtitle, |d, s| {
                        d.child(text(s).text_size(t.font_size - 3.).text_color(t.muted))
                    }),
            )
            .when_some(self.detail, |d, s| d.child(text(s).text_color(t.muted)))
            .when(clickable, |d| {
                d.child(
                    text(if cx.direction() == prism_core::Direction::Rtl {
                        "‹"
                    } else {
                        "›"
                    })
                    .text_color(t.muted)
                    .text_size(20.)
                    .accessibility_hidden(),
                )
            });
        if let Some(h) = self.on_click {
            el = el.on_click(h);
        } else {
            el = el.accessibility_role(Role::Group);
        }
        if let Some(menu) = self.context_menu {
            el = el.context_menu(menu);
        }
        if let Some(h) = self.swipe_left {
            el = el.on_swipe(prism_core::SwipeDirection::Left, h);
        }
        self.refine.apply(el).into_element()
    }
}

component!(ListRow);

/// Re-export for custom components built on `Div`.
pub type Root = Div;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use prism_core::{
        AppInit, Context, IntoElement, NodeKind, Patch, Render, Runtime, UiEvent, rgb,
    };

    use super::*;

    struct Page {
        on: bool,
    }

    impl Render for Page {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(button("Save").on_click(cx.listener(|_, _, _| {})))
                .child(
                    switch("Wi-Fi", self.on)
                        .on_change(cx.listener(|this, e: &ChangeEvent, _| this.on = e.checked())),
                )
        }
    }

    /// Themes hold closures (not `Send`), so each session builds its own.
    fn run(theme: fn() -> Theme) -> (Runtime, Vec<Patch>) {
        let init: AppInit = Arc::new(move |cx: &mut App| {
            set_theme(cx, theme());
            let root = cx.new_entity(|_| Page { on: false });
            cx.set_root(root);
        });
        let mut rt = Runtime::new(&init);
        let first = rt.render();
        (rt, first)
    }

    fn a11y_with_label(patches: &[Patch], label: &str) -> Option<(u64, prism_core::Accessibility)> {
        patches.iter().find_map(|p| match p {
            Patch::SetAccessibility { id, a11y } if a11y.label.as_deref() == Some(label) => {
                Some((*id, a11y.clone()))
            }
            _ => None,
        })
    }

    fn count(patches: &[Patch], kind: NodeKind) -> usize {
        patches
            .iter()
            .filter(|p| matches!(p, Patch::Create { kind: k, .. } if *k == kind))
            .count()
    }

    #[test]
    fn custom_components_are_accessible_and_work() {
        let (mut rt, first) = run(Theme::light);
        assert_eq!(count(&first, NodeKind::Button), 0, "custom, not native");
        let (_, save) = a11y_with_label(&first, "Save").unwrap();
        assert_eq!(save.role, Some(Role::Button));
        let (switch_id, wifi) = a11y_with_label(&first, "Wi-Fi").unwrap();
        assert_eq!((wifi.role, wifi.checked), (Some(Role::Switch), Some(false)));
        let patches = rt.dispatch(UiEvent::Click(switch_id));
        assert_eq!(
            a11y_with_label(&patches, "Wi-Fi").unwrap().1.checked,
            Some(true)
        );
    }

    #[test]
    fn prefer_native_uses_platform_controls() {
        let (mut rt, first) = run(|| Theme::light().prefer_native(true));
        assert_eq!(count(&first, NodeKind::Button), 1);
        assert_eq!(count(&first, NodeKind::Toggle), 1);
        let toggle = first
            .iter()
            .find_map(|p| match p {
                Patch::Create {
                    id,
                    kind: NodeKind::Toggle,
                } => Some(*id),
                _ => None,
            })
            .unwrap();
        let patches = rt.dispatch(UiEvent::Change(toggle, 1.));
        assert!(patches.contains(&Patch::SetValue {
            id: toggle,
            value: 1.
        }));
    }

    #[test]
    fn theme_overrides_replace_a_component_everywhere() {
        let (_, first) = run(|| {
            Theme::light().override_component(|b: Button, _cx: &mut App| {
                text(format!("[{}]", b.label)).into_element()
            })
        });
        assert!(
            first
                .iter()
                .any(|p| matches!(p, Patch::SetText { text, .. } if text == "[Save]"))
        );
    }

    #[test]
    fn theme_changes_rerender_and_set_native_appearance() {
        let (mut rt, _) = run(Theme::light);
        update_theme(&mut rt.app, |t| {
            *t = Theme::dark().accent(rgb(0x00FF00));
        });
        let patches = rt.render();
        assert!(patches.contains(&Patch::SetAppearance {
            appearance: prism_core::Appearance::Dark
        }));
        assert!(
            patches
                .iter()
                .any(|p| matches!(p, Patch::SetStyle { style, .. }
            if style.background == Some(rgb(0x00FF00))))
        );
    }
}

// --- Dual pane --------------------------------------------------------------------------

/// Two panes (list and detail, map and controls) placed for the device:
///
/// - a foldable spanning its hinge: one pane on each side, nothing under it
///   (side by side for a book, top and bottom on a table);
/// - tablets and wide windows: side by side;
/// - phones: one above the other.
///
/// ```ignore
/// ui::dual_pane(list, detail)
/// ```
pub struct DualPane {
    /// The first pane (a list, a map).
    pub primary: Element,
    /// The second pane (the detail, the controls).
    pub secondary: Element,
    refine: Refine,
}

/// Places `primary` and `secondary` for the device (see [`DualPane`]).
pub fn dual_pane(primary: impl IntoElement, secondary: impl IntoElement) -> DualPane {
    DualPane {
        primary: primary.into_element(),
        secondary: secondary.into_element(),
        refine: Refine::default(),
    }
}

refinable!(DualPane);

impl Component for DualPane {
    fn render(self, cx: &mut App) -> Element {
        let device = cx.device_info();
        let el = match device.fold {
            Some(fold) if fold.vertical => div()
                .flex_row()
                .flex_1()
                .child(div().w(px(fold.x)).child(self.primary))
                .child(div().w(px(fold.width)).accessibility_hidden())
                .child(div().flex_1().child(self.secondary)),
            Some(fold) => div()
                .flex_col()
                .flex_1()
                .child(div().h(px(fold.y)).child(self.primary))
                .child(div().h(px(fold.height)).accessibility_hidden())
                .child(div().flex_1().child(self.secondary)),
            None => div()
                .flex_col()
                .flex_1()
                .gap(16.)
                .md(|s| s.flex_row().gap(24.))
                .child(div().flex_1().child(self.primary))
                .child(div().flex_1().child(self.secondary)),
        };
        self.refine.apply(el).into_element()
    }
}

component!(DualPane);
