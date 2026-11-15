//! Form and feedback components: sliders, selects, date fields, text areas,
//! spinners and toasts. The controls are the platform's own (dragging,
//! menus, calendars and keyboards behave natively); the theme adds the label,
//! the value, spacing and its accent color. `.native()` renders the bare
//! platform control instead (no label row, platform tint).

use std::rc::Rc;
use std::time::{Duration, SystemTime};

use prism_core::{
    Accessible, App, ChangeEvent, ClickEvent, Element, Entity, FluentBuilder, Handler, InputEvent,
    IntoElement, ParentElement, Pose, Render, Role, Styled, Transition, controls::DateMode,
    date_picker, div, picker, px, text,
};

use crate::{Refine, Theme, Variant};

/// The label above a control (hidden from screen readers: the control
/// carries it).
fn field(t: &Theme, label: &str, trailing: Option<String>, control: Element) -> prism_core::Div {
    div()
        .gap(6.)
        .child(
            div()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(8.)
                .child(
                    text(label.to_string())
                        .text_size(t.font_size - 2.)
                        .semibold()
                        .text_color(t.text)
                        .flex_1()
                        .accessibility_hidden(),
                )
                .when_some(trailing, |d, v| {
                    d.child(
                        text(v)
                            .text_size(t.font_size - 2.)
                            .text_color(t.muted)
                            .accessibility_hidden(),
                    )
                }),
        )
        .child(control)
}

// --- Slider ---------------------------------------------------------------------

/// A labelled slider showing its value: `ui::slider("Volume", self.volume).range(0., 100.).step(1.)`.
pub struct Slider {
    pub label: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub disabled: bool,
    pub native: Option<bool>,
    /// How the value is shown next to the label and read out (`"42 %"`).
    pub format: Option<Rc<dyn Fn(f64) -> String>>,
    pub on_change: Option<Handler<ChangeEvent>>,
    refine: Refine,
}

pub fn slider(label: impl Into<String>, value: f64) -> Slider {
    Slider {
        label: label.into(),
        value,
        min: 0.,
        max: 1.,
        step: 0.,
        disabled: false,
        native: None,
        format: None,
        on_change: None,
        refine: Refine::default(),
    }
}

impl Slider {
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }
    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }
    pub fn format(mut self, f: impl Fn(f64) -> String + 'static) -> Self {
        self.format = Some(Rc::new(f));
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn native(mut self) -> Self {
        self.native = Some(true);
        self
    }
    pub fn on_change(mut self, handler: Handler<ChangeEvent>) -> Self {
        self.on_change = Some(handler);
        self
    }
}

refinable!(Slider);

impl crate::Component for Slider {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let shown = self.format.as_ref().map(|f| f(self.value));
        let mut control = prism_core::slider(self.value)
            .range(self.min, self.max)
            .step(self.step)
            .accessibility_label(self.label.clone())
            .accessibility_disabled(self.disabled)
            .when_some(shown.clone(), |s, v| s.accessibility_value(v))
            .when(self.disabled, |s| s.opacity(0.45));
        if let (Some(h), false) = (self.on_change, self.disabled) {
            control = control.on_change(h);
        }
        if self.native.unwrap_or(t.prefer_native) {
            return self.refine.apply(control).into_element();
        }
        let shown = shown.unwrap_or_else(|| cx.format_number(self.value, decimals(self.step)));
        let el = field(
            &t,
            &self.label,
            Some(shown),
            control.text_color(t.accent).into_element(),
        );
        self.refine.apply(el).into_element()
    }
}

component!(Slider);

/// Decimals worth showing for a step (1 → 0, 0.25 → 2; continuous → 2).
fn decimals(step: f64) -> usize {
    if step <= 0. {
        return 2;
    }
    let mut d = 0;
    let mut s = step;
    while s.fract().abs() > 1e-9 && d < 4 {
        s *= 10.;
        d += 1;
    }
    d
}

// --- Select ---------------------------------------------------------------------

/// A labelled menu of options (the platform's pop-up menu):
/// `ui::select("Size", ["S", "M", "L"], Some(self.size)).on_change(…)`.
pub struct Select {
    pub label: String,
    pub options: Vec<String>,
    pub selected: Option<usize>,
    pub placeholder: String,
    pub disabled: bool,
    pub native: Option<bool>,
    pub on_change: Option<Handler<ChangeEvent>>,
    refine: Refine,
}

pub fn select<S: Into<String>>(
    label: impl Into<String>,
    options: impl IntoIterator<Item = S>,
    selected: Option<usize>,
) -> Select {
    Select {
        label: label.into(),
        options: options.into_iter().map(Into::into).collect(),
        selected,
        placeholder: String::new(),
        disabled: false,
        native: None,
        on_change: None,
        refine: Refine::default(),
    }
}

impl Select {
    /// Shown while nothing is selected.
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = p.into();
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn native(mut self) -> Self {
        self.native = Some(true);
        self
    }
    pub fn on_change(mut self, handler: Handler<ChangeEvent>) -> Self {
        self.on_change = Some(handler);
        self
    }
}

refinable!(Select);

impl crate::Component for Select {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let mut control = picker(self.options, self.selected)
            .placeholder(self.placeholder)
            .accessibility_label(self.label.clone())
            .accessibility_disabled(self.disabled)
            .when(self.disabled, |s| s.opacity(0.45));
        if let (Some(h), false) = (self.on_change, self.disabled) {
            control = control.on_change(h);
        }
        if self.native.unwrap_or(t.prefer_native) {
            return self.refine.apply(control).into_element();
        }
        let el = field(
            &t,
            &self.label,
            None,
            control.text_color(t.accent).self_start().into_element(),
        );
        self.refine.apply(el).into_element()
    }
}

component!(Select);

// --- DateField ------------------------------------------------------------------

/// A labelled date (or time) picker: `ui::date_field("Due", self.due).on_change(…)`.
pub struct DateField {
    pub label: String,
    pub value: SystemTime,
    pub mode: DateMode,
    pub min: Option<SystemTime>,
    pub max: Option<SystemTime>,
    pub disabled: bool,
    pub native: Option<bool>,
    pub on_change: Option<Handler<ChangeEvent>>,
    refine: Refine,
}

pub fn date_field(label: impl Into<String>, value: SystemTime) -> DateField {
    DateField {
        label: label.into(),
        value,
        mode: DateMode::Date,
        min: None,
        max: None,
        disabled: false,
        native: None,
        on_change: None,
        refine: Refine::default(),
    }
}

impl DateField {
    pub fn time(mut self) -> Self {
        self.mode = DateMode::Time;
        self
    }
    pub fn date_time(mut self) -> Self {
        self.mode = DateMode::DateTime;
        self
    }
    pub fn min(mut self, t: SystemTime) -> Self {
        self.min = Some(t);
        self
    }
    pub fn max(mut self, t: SystemTime) -> Self {
        self.max = Some(t);
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn native(mut self) -> Self {
        self.native = Some(true);
        self
    }
    pub fn on_change(mut self, handler: Handler<ChangeEvent>) -> Self {
        self.on_change = Some(handler);
        self
    }
}

refinable!(DateField);

impl crate::Component for DateField {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let mut control = date_picker(self.value)
            .mode(self.mode)
            .when_some(self.min, |d, m| d.min(m))
            .when_some(self.max, |d, m| d.max(m))
            .accessibility_label(self.label.clone())
            .accessibility_disabled(self.disabled)
            .when(self.disabled, |s| s.opacity(0.45));
        if let (Some(h), false) = (self.on_change, self.disabled) {
            control = control.on_change(h);
        }
        if self.native.unwrap_or(t.prefer_native) {
            return self.refine.apply(control).into_element();
        }
        let el = field(
            &t,
            &self.label,
            None,
            control.text_color(t.accent).self_start().into_element(),
        );
        self.refine.apply(el).into_element()
    }
}

component!(DateField);

// --- TextArea -------------------------------------------------------------------

/// A labelled multi-line field that grows with its text up to `max_lines`,
/// then scrolls: `ui::text_area("Notes", &self.notes).on_input(…)`.
pub struct TextArea {
    pub label: String,
    pub value: String,
    pub placeholder: String,
    pub helper: Option<String>,
    /// Visible lines before it scrolls (default 6).
    pub max_lines: u32,
    pub autocorrect: bool,
    pub native: Option<bool>,
    pub on_input: Option<Handler<InputEvent>>,
    refine: Refine,
}

pub fn text_area(label: impl Into<String>, value: impl Into<String>) -> TextArea {
    TextArea {
        label: label.into(),
        value: value.into(),
        placeholder: String::new(),
        helper: None,
        max_lines: 6,
        autocorrect: true,
        native: None,
        on_input: None,
        refine: Refine::default(),
    }
}

impl TextArea {
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = p.into();
        self
    }
    pub fn helper(mut self, h: impl Into<String>) -> Self {
        self.helper = Some(h.into());
        self
    }
    pub fn max_lines(mut self, lines: u32) -> Self {
        self.max_lines = lines.max(1);
        self
    }
    pub fn autocorrect(mut self, on: bool) -> Self {
        self.autocorrect = on;
        self
    }
    pub fn native(mut self) -> Self {
        self.native = Some(true);
        self
    }
    pub fn on_input(mut self, handler: Handler<InputEvent>) -> Self {
        self.on_input = Some(handler);
        self
    }
}

refinable!(TextArea);

impl crate::Component for TextArea {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let line = (t.font_size * 1.3).ceil();
        let mut area = prism_core::text_area()
            .value(self.value)
            .placeholder(self.placeholder)
            .autocorrect(self.autocorrect)
            .accessibility_label(self.label.clone())
            .when_some(self.helper.clone(), |a, h| a.accessibility_hint(h))
            .max_h(px(line * self.max_lines as f32 + 16.));
        if let Some(h) = self.on_input {
            area = area.on_input(h);
        }
        if self.native.unwrap_or(t.prefer_native) {
            return self.refine.apply(area).into_element();
        }
        let area = area
            .text_size(t.font_size)
            .text_color(t.text)
            .bg(t.surface)
            .border(1.)
            .border_color(t.border)
            .rounded(t.radius);
        let el =
            field(&t, &self.label, None, area.into_element()).when_some(self.helper, |d, h| {
                d.child(
                    text(h)
                        .text_size(t.font_size - 3.)
                        .text_color(t.muted)
                        .accessibility_hidden(),
                )
            });
        self.refine.apply(el).into_element()
    }
}

component!(TextArea);

// --- Spinner --------------------------------------------------------------------

/// An activity indicator in the accent color, with an optional caption
/// (`ui::spinner().label("Loading…")`).
pub struct Spinner {
    pub label: Option<String>,
    pub large: bool,
    refine: Refine,
}

pub fn spinner() -> Spinner {
    Spinner {
        label: None,
        large: false,
        refine: Refine::default(),
    }
}

impl Spinner {
    /// Shown next to it and read out.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    pub fn large(mut self) -> Self {
        self.large = true;
        self
    }
}

refinable!(Spinner);

impl crate::Component for Spinner {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let s = prism_core::spinner()
            .text_color(t.accent)
            .when(self.large, |s| s.large())
            .when_some(self.label.clone(), |s, l| s.accessibility_label(l));
        let el = match self.label {
            None => div().items_start().child(s),
            Some(l) => div().flex_row().items_center().gap(10.).child(s).child(
                text(l)
                    .text_color(t.muted)
                    .text_size(t.font_size - 1.)
                    .accessibility_hidden(),
            ),
        };
        self.refine.apply(el).into_element()
    }
}

component!(Spinner);

// --- Toasts ---------------------------------------------------------------------

/// A short message over the screen's bottom edge, dismissed after a few
/// seconds: `ui::show_toast(cx, ui::toast("Saved").action("Undo", undo))`.
/// Screen readers announce it (a polite live region).
pub struct Toast {
    pub message: String,
    pub action: Option<(String, Handler<ClickEvent>)>,
    pub duration: Duration,
    pub tone: Variant,
}

pub fn toast(message: impl Into<String>) -> Toast {
    Toast {
        message: message.into(),
        action: None,
        duration: Duration::from_millis(4000),
        tone: Variant::Secondary,
    }
}

impl Toast {
    /// A button in the toast (Undo, Retry); choosing it dismisses the toast.
    pub fn action(mut self, label: impl Into<String>, handler: Handler<ClickEvent>) -> Self {
        self.action = Some((label.into(), handler));
        self
    }
    /// How long it stays (default 4 s; toasts with an action stay at least 6 s).
    pub fn duration(mut self, d: Duration) -> Self {
        self.duration = d;
        self
    }
    /// `Variant::Danger` for errors, `Variant::Primary` for the accent.
    pub fn tone(mut self, tone: Variant) -> Self {
        self.tone = tone;
        self
    }
}

/// The toasts on screen (see [`toaster`]).
#[derive(Default)]
pub struct Toaster {
    items: Vec<(u64, Toast)>,
    next: u64,
    /// The last message, announced by screen readers.
    announce: String,
}

struct ToasterHandle(Entity<Toaster>);

fn toaster_entity(cx: &mut App) -> Entity<Toaster> {
    if let Some(h) = cx.try_global::<ToasterHandle>() {
        return h.0;
    }
    let e = cx.new_entity(|_| Toaster::default());
    cx.set_global(ToasterHandle(e));
    e
}

/// Where toasts appear: put it once, as the last child of the screen (or
/// root view) they cover. It is absolutely positioned at the bottom.
pub fn toaster() -> Element {
    prism_core::lazy(toaster_entity)
}

/// Shows `toast` (several stack up, newest at the bottom).
pub fn show_toast(cx: &mut App, toast: Toast) {
    let entity = toaster_entity(cx);
    let min = if toast.action.is_some() {
        Duration::from_secs(6)
    } else {
        Duration::ZERO
    };
    let duration = toast.duration.max(min);
    let id = entity.update(cx, |t, cx| {
        t.next += 1;
        let id = t.next;
        t.announce = toast.message.clone();
        t.items.push((id, toast));
        // A handful at most: the oldest goes first.
        if t.items.len() > 3 {
            t.items.remove(0);
        }
        cx.notify();
        id
    });
    let e = entity;
    cx.spawn(async move |cx| {
        cx.sleep(duration).await;
        cx.update_entity(&e, |t, _| t.items.retain(|(i, _)| *i != id));
    });
}

/// Removes every toast at once.
pub fn dismiss_toasts(cx: &mut App) {
    let entity = toaster_entity(cx);
    entity.update(cx, |t, cx| {
        t.items.clear();
        cx.notify();
    });
}

impl Render for Toaster {
    fn render(&mut self, cx: &mut prism_core::Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let this = cx.entity();
        let toasts = self.items.iter().map(|(id, toast)| {
            let (bg, fg) = match toast.tone {
                Variant::Danger => (t.danger, prism_core::Color::WHITE),
                Variant::Primary => (t.accent, t.on_accent),
                _ => (t.text, t.surface),
            };
            let id = *id;
            div()
                .key(format!("toast-{id}"))
                .flex_row()
                .items_center()
                .gap(12.)
                .px(16.)
                .py(12.)
                .rounded(t.radius + 4.)
                .bg(bg)
                .w_full()
                .max_w(px(520.))
                .enter(Pose::slide(0., 16.).opacity(0.))
                .exit(Pose::fade())
                .transition(Transition::spring(350))
                .child(
                    text(toast.message.clone())
                        .text_color(fg)
                        .text_size(t.font_size - 1.)
                        .medium()
                        .flex_1()
                        // Wraps on narrow screens (watches) instead of pushing
                        // the action out.
                        .min_w(px(0.)),
                )
                .when_some(toast.action.clone(), |d, (label, h)| {
                    d.child(
                        div()
                            .px(8.)
                            .py(4.)
                            .rounded(t.radius)
                            .flex_shrink_0()
                            .accessibility_role(Role::Button)
                            .accessibility_label(label.clone())
                            .child(
                                text(label)
                                    .text_color(if toast.tone == Variant::Secondary {
                                        t.accent.dark_variant()
                                    } else {
                                        fg
                                    })
                                    .bold()
                                    .accessibility_hidden(),
                            )
                            .on_click(Rc::new(move |e: &ClickEvent, app: &mut App| {
                                h(e, app);
                                this.update(app, |t, cx| {
                                    t.items.retain(|(i, _)| *i != id);
                                    cx.notify();
                                });
                            })),
                    )
                })
        });
        div()
            .absolute()
            .left(16.)
            .right(16.)
            .bottom(24.)
            .gap(8.)
            .items_center()
            .accessibility_role(Role::Status)
            .accessibility_value(self.announce.clone())
            .children(toasts.collect::<Vec<_>>())
    }
}
