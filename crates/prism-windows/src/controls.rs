//! Prism's controls as WinUI 3 controls: `Button`, `TextBox` /
//! `PasswordBox`, `ToggleSwitch`, `CheckBox`, `RadioButton`s, `Slider`,
//! `ProgressBar`, `ProgressRing`, `ComboBox`, `CalendarDatePicker` /
//! `TimePicker`, `FontIcon` (Segoe Fluent Icons), `Image`, vector `Path`s.
//! Their events become `UiEvent`s; their natural sizes (`Widgets::measure`)
//! are WinUI's metrics around the measured text.
//!
//! A button or a text field the app styles itself (a background, a
//! border, `.border(0.)`) keeps the real control (focus, keyboard, UI
//! Automation), its own chrome made transparent: the node's `Border`
//! draws Prism's look under it.

use prism_core::controls::{
    DateConfig, DateMode, InputConfig, PickerConfig, RadioConfig, SliderConfig,
};
use prism_core::{Color, NodeId, NodeKind, Style, UiEvent};
use prism_native::text::StyledText;
use windows::Foundation::{DateTime, IReference, PropertyValue, TimeSpan, TypedEventHandler};
use windows::System::VirtualKey;
use windows_core::{Interface, Ref, Result};

use crate::fluent::Palette;
use crate::fluent::metrics::*;
use crate::imp::{changed, dispatch, typed};
use crate::scene::Node;
use crate::text::{Measurer, apply, font, label, weight};
use crate::winui::xaml::Controls::Primitives::RangeBaseValueChangedEventHandler;
use crate::winui::xaml::Controls::{
    Button, CalendarDatePicker, CalendarDatePickerDateChangedEventArgs, Canvas, CheckBox, ComboBox,
    Control, FontIcon, Image, Orientation, PasswordBox, ProgressBar, ProgressRing, RadioButton,
    SelectionChangedEventHandler, Slider, StackPanel, TextBlock, TextBox, TextChangedEventHandler,
    TimePicker, TimePickerSelectedValueChangedEventArgs, ToggleSwitch,
};
use crate::winui::xaml::Input::KeyEventHandler;
use crate::winui::xaml::Media::{
    BezierSegment, Brush, FillRule, LineSegment, PathFigure, PathGeometry, PenLineCap, PenLineJoin,
    Stretch,
};
use crate::winui::xaml::Shapes::Path as XPath;
use crate::winui::xaml::{
    HorizontalAlignment, ResourceDictionary, RoutedEventHandler, TextWrapping, VerticalAlignment,
};
use crate::winui::{boxed, brush, h, thickness, transparent, uniform};

/// What a node shows inside its box.
pub enum Content {
    None,
    Text(TextBlock),
    Button(Button, TextBlock),
    Field(TextBox),
    Secure(PasswordBox),
    Toggle(ToggleSwitch),
    Check(CheckBox, TextBlock),
    Radios(StackPanel, Vec<RadioButton>),
    Slider(Slider),
    Progress(ProgressBar),
    Ring(ProgressRing),
    Combo(ComboBox),
    Date(CalendarDatePicker),
    Time(TimePicker),
    Icon(FontIcon),
    Picture(Image),
    Shape(Canvas),
    /// Embedded content (`web.rs`, `media.rs`, `rich.rs`).
    Web(crate::web::Web),
    Video(crate::media::Video),
    Rich(crate::rich::Rich),
    Scene(crate::scene3d::SceneView),
}

impl Content {
    /// The element placed in the node's box.
    pub fn element(&self) -> Option<crate::winui::xaml::UIElement> {
        let e: Result<crate::winui::xaml::UIElement> = match self {
            Content::None => return None,
            Content::Text(t) => t.cast(),
            Content::Button(b, _) => b.cast(),
            Content::Field(t) => t.cast(),
            Content::Secure(p) => p.cast(),
            Content::Toggle(t) => t.cast(),
            Content::Check(c, _) => c.cast(),
            Content::Radios(p, _) => p.cast(),
            Content::Slider(s) => s.cast(),
            Content::Progress(p) => p.cast(),
            Content::Ring(r) => r.cast(),
            Content::Combo(c) => c.cast(),
            Content::Date(d) => d.cast(),
            Content::Time(t) => t.cast(),
            Content::Icon(i) => i.cast(),
            Content::Picture(i) => i.cast(),
            Content::Shape(c) => c.cast(),
            Content::Web(w) => w.element.cast(),
            Content::Video(v) => v.element.cast(),
            Content::Rich(r) => r.element.cast(),
            Content::Scene(s) => s.element.cast(),
        };
        e.ok()
    }

    /// The control that takes the keyboard focus.
    pub fn control(&self) -> Option<Control> {
        let c: Result<Control> = match self {
            Content::Button(b, _) => b.cast(),
            Content::Field(t) => t.cast(),
            Content::Secure(p) => p.cast(),
            Content::Toggle(t) => t.cast(),
            Content::Check(c, _) => c.cast(),
            Content::Radios(_, r) => r.first()?.cast(),
            Content::Slider(s) => s.cast(),
            Content::Combo(c) => c.cast(),
            Content::Date(d) => d.cast(),
            Content::Time(t) => t.cast(),
            Content::Rich(r) => r.element.cast(),
            Content::Web(w) => w.element.cast(),
            _ => return None,
        };
        c.ok()
    }
}

/// Whether a button keeps the Fluent look: the app gave it no background,
/// gradient or border of its own.
pub fn native_button(s: &Style) -> bool {
    s.background.is_none()
        && s.gradient.is_none()
        && s.border_width <= 0.
        && !s.border_none
        && s.border_sides.is_empty()
}

pub fn is_field(k: NodeKind) -> bool {
    matches!(
        k,
        NodeKind::TextInput | NodeKind::TextArea | NodeKind::RichInput
    )
}

/// The content for a new node of `kind`, its events wired to `id`.
pub fn create(id: NodeId, kind: NodeKind) -> Result<Content> {
    Ok(match kind {
        NodeKind::Text | NodeKind::Timer => {
            let t = TextBlock::new()?;
            t.SetIsHitTestVisible(false)?;
            Content::Text(t)
        }
        NodeKind::Button => {
            let b = Button::new()?;
            let t = TextBlock::new()?;
            t.SetTextWrapping(TextWrapping::NoWrap)?;
            t.SetIsTextScaleFactorEnabled(false)?;
            b.SetContent(&t)?;
            b.SetHorizontalContentAlignment(HorizontalAlignment::Center)?;
            b.SetVerticalContentAlignment(VerticalAlignment::Center)?;
            b.SetMinWidth(0.)?;
            b.SetMinHeight(0.)?;
            b.SetIsTextScaleFactorEnabled(false)?;
            b.Click(&RoutedEventHandler::new(move |_, _| {
                dispatch(UiEvent::Click(id));
                Ok(())
            }))?;
            Content::Button(b, t)
        }
        NodeKind::RichInput => Content::Rich(crate::rich::Rich::new(id)?),
        NodeKind::WebView => Content::Web(crate::web::Web::new(id)?),
        NodeKind::Video => Content::Video(crate::media::Video::new(id)?),
        NodeKind::Scene3d => Content::Scene(crate::scene3d::SceneView::new(id)?),
        k if is_field(k) => field(id, k == NodeKind::TextInput)?,
        NodeKind::Toggle => {
            let t = ToggleSwitch::new()?;
            t.SetOnContent(&boxed("")?)?;
            t.SetOffContent(&boxed("")?)?;
            t.SetMinWidth(0.)?;
            t.SetIsTextScaleFactorEnabled(false)?;
            t.Toggled(&RoutedEventHandler::new(move |s, _| {
                if let Some(t) = s.as_ref().and_then(|s| s.cast::<ToggleSwitch>().ok()) {
                    changed(id, if t.IsOn()? { 1. } else { 0. });
                }
                Ok(())
            }))?;
            Content::Toggle(t)
        }
        NodeKind::Checkbox => {
            let c = CheckBox::new()?;
            let t = TextBlock::new()?;
            t.SetIsTextScaleFactorEnabled(false)?;
            c.SetContent(&t)?;
            c.SetMinWidth(0.)?;
            c.SetMinHeight(0.)?;
            c.SetIsTextScaleFactorEnabled(false)?;
            let on = move |v: f64| {
                RoutedEventHandler::new(move |_, _| {
                    changed(id, v);
                    Ok(())
                })
            };
            c.Checked(&on(1.))?;
            c.Unchecked(&on(0.))?;
            Content::Check(c, t)
        }
        NodeKind::RadioGroup => {
            let p = StackPanel::new()?;
            Content::Radios(p, Vec::new())
        }
        NodeKind::Slider => {
            let s = Slider::new()?;
            s.SetIsTextScaleFactorEnabled(false)?;
            s.ValueChanged(&RangeBaseValueChangedEventHandler::new(move |_, a| {
                if let Some(a) = a.as_ref() {
                    changed(id, a.NewValue()?);
                }
                Ok(())
            }))?;
            Content::Slider(s)
        }
        NodeKind::Progress => {
            let p = ProgressBar::new()?;
            p.SetMinimum(0.)?;
            p.SetMaximum(1.)?;
            p.SetVerticalAlignment(VerticalAlignment::Center)?;
            Content::Progress(p)
        }
        NodeKind::Spinner => {
            let r = ProgressRing::new()?;
            r.SetIsActive(true)?;
            r.SetMinWidth(0.)?;
            r.SetMinHeight(0.)?;
            Content::Ring(r)
        }
        NodeKind::Picker => {
            let c = ComboBox::new()?;
            c.SetMinWidth(0.)?;
            c.SetIsTextScaleFactorEnabled(false)?;
            c.SelectionChanged(&SelectionChangedEventHandler::new(move |s, _| {
                if let Some(c) = s.as_ref().and_then(|s| s.cast::<ComboBox>().ok()) {
                    let i = c.SelectedIndex()?;
                    if i >= 0 {
                        changed(id, i as f64);
                    }
                }
                Ok(())
            }))?;
            Content::Combo(c)
        }
        NodeKind::DatePicker => date_picker(id, DateMode::Date)?,
        NodeKind::Image => {
            let i = FontIcon::new()?;
            i.SetFontFamily(&font(crate::text::ICON_FONT)?)?;
            i.SetIsHitTestVisible(false)?;
            i.SetIsTextScaleFactorEnabled(false)?;
            Content::Icon(i)
        }
        NodeKind::Picture => {
            let i = Image::new()?;
            i.SetIsHitTestVisible(false)?;
            Content::Picture(i)
        }
        NodeKind::Shape => {
            let c = Canvas::new()?;
            c.SetIsHitTestVisible(false)?;
            Content::Shape(c)
        }
        _ => Content::None,
    })
}

/// A text field: `TextBox` (one line, or several for a text area).
fn field(id: NodeId, single: bool) -> Result<Content> {
    let t = TextBox::new()?;
    t.SetMinWidth(0.)?;
    t.SetMinHeight(0.)?;
    t.SetIsTextScaleFactorEnabled(false)?;
    if !single {
        t.SetAcceptsReturn(true)?;
        t.SetTextWrapping(TextWrapping::Wrap)?;
    }
    t.TextChanged(&TextChangedEventHandler::new(move |s, _| {
        if let Some(t) = s.as_ref().and_then(|s| s.cast::<TextBox>().ok()) {
            typed(id, t.Text()?.to_string());
        }
        Ok(())
    }))?;
    t.SelectionChanged(&RoutedEventHandler::new(move |s, _| {
        if let Some(t) = s.as_ref().and_then(|s| s.cast::<TextBox>().ok()) {
            let text = t.Text()?.to_string();
            let a = t.SelectionStart()?.max(0) as usize;
            let b = a + t.SelectionLength()?.max(0) as usize;
            crate::imp::selected(id, &text, a, b);
        }
        Ok(())
    }))?;
    if single {
        t.KeyDown(&KeyEventHandler::new(move |_, a| {
            if let Some(a) = a.as_ref()
                && a.Key()? == VirtualKey::Enter
            {
                dispatch(UiEvent::Submit(id));
            }
            Ok(())
        }))?;
    }
    Ok(Content::Field(t))
}

/// A password field (`InputConfig::secure`).
fn secure(id: NodeId) -> Result<Content> {
    let p = PasswordBox::new()?;
    p.SetMinWidth(0.)?;
    p.SetMinHeight(0.)?;
    p.SetIsTextScaleFactorEnabled(false)?;
    p.PasswordChanged(&RoutedEventHandler::new(move |s, _| {
        if let Some(p) = s.as_ref().and_then(|s| s.cast::<PasswordBox>().ok()) {
            typed(id, p.Password()?.to_string());
        }
        Ok(())
    }))?;
    p.KeyDown(&KeyEventHandler::new(move |_, a| {
        if let Some(a) = a.as_ref()
            && a.Key()? == VirtualKey::Enter
        {
            dispatch(UiEvent::Submit(id));
        }
        Ok(())
    }))?;
    Ok(Content::Secure(p))
}

/// `CalendarDatePicker` for dates, `TimePicker` for times.
fn date_picker(id: NodeId, mode: DateMode) -> Result<Content> {
    if matches!(mode, DateMode::Time | DateMode::TimeOfDay) {
        let t = TimePicker::new()?;
        t.SetMinWidth(0.)?;
        t.SetIsTextScaleFactorEnabled(false)?;
        t.SelectedTimeChanged(&TypedEventHandler::<
            TimePicker,
            TimePickerSelectedValueChangedEventArgs,
        >::new(
            move |_, a: Ref<TimePickerSelectedValueChangedEventArgs>| {
                if let Some(a) = a.as_ref()
                    && let Ok(v) = a.NewTime()
                    && let Ok(span) = v.Value()
                {
                    let secs = span.Duration as f64 / 1e7;
                    changed(
                        id,
                        if mode == DateMode::TimeOfDay {
                            secs
                        } else {
                            crate::imp::time_of_day(id, secs)
                        },
                    );
                }
                Ok(())
            },
        ))?;
        return Ok(Content::Time(t));
    }
    let d = CalendarDatePicker::new()?;
    d.SetMinWidth(0.)?;
    d.SetIsTextScaleFactorEnabled(false)?;
    d.DateChanged(&TypedEventHandler::<
        CalendarDatePicker,
        CalendarDatePickerDateChangedEventArgs,
    >::new(
        move |_, a: Ref<CalendarDatePickerDateChangedEventArgs>| {
            if let Some(a) = a.as_ref()
                && let Ok(v) = a.NewDate()
                && let Ok(dt) = v.Value()
            {
                changed(id, unix_seconds(dt));
            }
            Ok(())
        },
    ))?;
    Ok(Content::Date(d))
}

/// Windows' `DateTime` (100 ns since 1601) to Unix seconds.
pub fn unix_seconds(dt: DateTime) -> f64 {
    dt.UniversalTime as f64 / 1e7 - 11_644_473_600.
}

pub fn date_time(unix: f64) -> DateTime {
    DateTime {
        UniversalTime: ((unix + 11_644_473_600.) * 1e7) as i64,
    }
}

/// The node's text color, resolved (`None` in the style: the theme's).
pub fn text_color(s: &Style, pal: &Palette) -> Color {
    s.text_color.map_or(pal.text, |c| pal.resolve(c))
}

/// Lightweight styling: these theme resources of one control, every state
/// pointing to the same brushes (Prism resolves the states itself).
pub fn override_brushes(res: &ResourceDictionary, keys: &[&str], b: &Brush) -> Result<()> {
    for k in keys {
        res.Insert(&boxed(k)?, b)?;
    }
    Ok(())
}

pub const BUTTON_CHROME: [&str; 8] = [
    "ButtonBackground",
    "ButtonBackgroundPointerOver",
    "ButtonBackgroundPressed",
    "ButtonBackgroundDisabled",
    "ButtonBorderBrush",
    "ButtonBorderBrushPointerOver",
    "ButtonBorderBrushPressed",
    "ButtonBorderBrushDisabled",
];

const FIELD_CHROME: [&str; 8] = [
    "TextControlBackground",
    "TextControlBackgroundPointerOver",
    "TextControlBackgroundFocused",
    "TextControlBackgroundDisabled",
    "TextControlBorderBrush",
    "TextControlBorderBrushPointerOver",
    "TextControlBorderBrushFocused",
    "TextControlBorderBrushDisabled",
];

const FIELD_TEXT: [&str; 4] = [
    "TextControlForeground",
    "TextControlForegroundPointerOver",
    "TextControlForegroundFocused",
    "TextControlForegroundDisabled",
];

/// The node's look on its control: fonts, colors, padding; chrome made
/// transparent where the app draws its own.
pub fn style(c: &Content, n: &Node, pal: &Palette) -> Result<()> {
    let s = &n.style;
    let color = text_color(s, pal);
    match c {
        Content::Text(t) => {
            let st = StyledText::new(&n.text, &n.config, s);
            apply(t, &st, pal, color)?;
            // A text takes pointer input only for its links.
            t.SetIsHitTestVisible(st.has_links())?;
        }
        Content::Button(b, t) => {
            let st = label(&n.text, s);
            apply(t, &st, pal, color)?;
            let native = native_button(s);
            if native && s.text_color.is_none() {
                // The template's foreground (pressed, disabled states).
                t.ClearValue(&TextBlock::ForegroundProperty()?)?;
            }
            if !native {
                let res = b.Resources()?;
                if !res.HasKey(&boxed("ButtonBackground")?)? {
                    override_brushes(&res, &BUTTON_CHROME, &transparent()?.cast()?)?;
                }
                b.SetBorderThickness(uniform(0.))?;
            }
            b.SetPadding(if s.has_padding() {
                let p = s.padding;
                thickness(p.left, p.top, p.right, p.bottom)
            } else {
                let (l, t, r, bt) = BUTTON_PADDING;
                thickness(l, t, r, bt)
            })?;
            b.SetIsEnabled(!n.a11y.disabled)?;
        }
        Content::Field(t) => {
            field_look(t.cast()?, n, color)?;
            t.SetPlaceholderText(&h(&n.placeholder))?;
        }
        Content::Secure(p) => {
            field_look(p.cast()?, n, color)?;
            p.SetPlaceholderText(&h(&n.placeholder))?;
        }
        Content::Check(cb, t) => {
            let st = label(&n.text, s);
            apply(t, &st, pal, color)?;
            if s.text_color.is_none() {
                t.ClearValue(&TextBlock::ForegroundProperty()?)?;
            }
            cb.SetIsEnabled(!n.a11y.disabled)?;
        }
        Content::Radios(_, radios) => {
            for r in radios {
                r.SetFontSize(s.font_size as f64)?;
                r.SetIsEnabled(!n.a11y.disabled)?;
                if let Some(c) = s.text_color {
                    r.SetForeground(&brush(pal.resolve(c))?)?;
                }
            }
        }
        Content::Icon(i) => {
            i.SetFontSize(s.font_size.max(1.) as f64)?;
            i.SetForeground(&brush(color)?)?;
        }
        Content::Toggle(t) => t.SetIsEnabled(!n.a11y.disabled)?,
        Content::Slider(sl) => sl.SetIsEnabled(!n.a11y.disabled)?,
        Content::Combo(cb) => {
            cb.SetFontSize(s.font_size as f64)?;
            cb.SetIsEnabled(!n.a11y.disabled)?;
        }
        Content::Date(d) => d.SetIsEnabled(!n.a11y.disabled)?,
        Content::Time(t) => t.SetIsEnabled(!n.a11y.disabled)?,
        Content::Progress(p) => {
            if let Some(c) = s.text_color {
                p.SetForeground(&brush(pal.resolve(c))?)?;
            }
        }
        Content::Ring(r) => {
            if let Some(c) = s.text_color {
                r.SetForeground(&brush(pal.resolve(c))?)?;
            }
        }
        Content::Rich(r) => {
            field_look(r.element.cast()?, n, color)?;
            r.element.SetPlaceholderText(&h(&n.placeholder))?;
        }
        Content::Picture(_)
        | Content::Shape(_)
        | Content::Web(_)
        | Content::Video(_)
        | Content::Scene(_)
        | Content::None => {}
    }
    Ok(())
}

/// A text field's fonts, colors and chrome.
fn field_look(c: Control, n: &Node, color: Color) -> Result<()> {
    let s = &n.style;
    c.SetFontSize(s.font_size.max(1.) as f64)?;
    c.SetFontFamily(&font(&crate::text::family_name(
        &s.font_family,
        s.font_size,
    ))?)?;
    c.SetFontWeight(weight(s.font_weight))?;
    c.SetIsEnabled(!n.a11y.disabled)?;
    let res = c.Resources()?;
    if s.text_color.is_some() {
        let fg: Brush = brush(color)?.cast()?;
        c.SetForeground(&fg)?;
        override_brushes(&res, &FIELD_TEXT, &fg)?;
    }
    if s.own_field_look() {
        if !res.HasKey(&boxed("TextControlBackground")?)? {
            override_brushes(&res, &FIELD_CHROME, &transparent()?.cast()?)?;
        }
        c.SetBorderThickness(uniform(0.))?;
        let p = s.padding;
        // A leading icon (`fieldicon.rs`) takes the start of the field.
        let lead = crate::fieldicon::leading(n);
        c.SetPadding(if s.has_padding() {
            thickness(lead, p.top, p.right, p.bottom)
        } else {
            thickness(lead, 0., 0., 0.)
        })?;
    } else {
        let (_, t, r, b) = FIELD_PADDING;
        c.SetPadding(thickness(crate::fieldicon::leading(n), t, r, b))?;
    }
    Ok(())
}

/// A real `Button` without chrome, for a clickable view: its children
/// inside, Invoke, keyboard activation, focus visuals and UI Automation
/// from WinUI; Prism draws the look (box, states) under it.
pub fn clickable_host(id: NodeId) -> Result<Button> {
    let b = Button::new()?;
    override_brushes(&b.Resources()?, &BUTTON_CHROME, &transparent()?.cast()?)?;
    b.SetBorderThickness(uniform(0.))?;
    b.SetPadding(uniform(0.))?;
    b.SetMinWidth(0.)?;
    b.SetMinHeight(0.)?;
    b.SetHorizontalContentAlignment(HorizontalAlignment::Left)?;
    b.SetVerticalContentAlignment(VerticalAlignment::Top)?;
    b.SetIsTextScaleFactorEnabled(false)?;
    b.Click(&RoutedEventHandler::new(move |_, _| {
        dispatch(UiEvent::Click(id));
        Ok(())
    }))?;
    Ok(b)
}

/// A field's configuration: secure (a `PasswordBox` in place of the
/// `TextBox`), length limit. Returns a new content when it must change.
pub fn configure_field(c: &Content, id: NodeId, config: &str) -> Result<Option<Content>> {
    let config = InputConfig::decode(config);
    match c {
        Content::Field(t) if config.secure => {
            let _ = t;
            return secure(id).map(Some);
        }
        Content::Field(t) => t.SetMaxLength(config.max_length.unwrap_or(0) as i32)?,
        Content::Secure(p) => p.SetMaxLength(config.max_length.unwrap_or(0) as i32)?,
        _ => {}
    }
    Ok(None)
}

/// A control's options (`set_config`).
pub fn configure(c: &mut Content, id: NodeId, kind: NodeKind, config: &str) -> Result<()> {
    match c {
        Content::Slider(s) => {
            let cfg = SliderConfig::decode(config);
            s.SetMinimum(cfg.min)?;
            s.SetMaximum(cfg.max)?;
            if cfg.step > 0. {
                s.SetStepFrequency(cfg.step)?;
                s.SetSmallChange(cfg.step)?;
            } else {
                s.SetStepFrequency((cfg.max - cfg.min) / 100.)?;
            }
        }
        Content::Combo(cb) => {
            let cfg = PickerConfig::decode(config);
            let items = cb.Items()?;
            items.Clear()?;
            for o in &cfg.options {
                items.Append(&boxed(o)?)?;
            }
        }
        Content::Radios(panel, radios) => {
            let cfg = RadioConfig::decode(config);
            panel.SetOrientation(if cfg.horizontal {
                Orientation::Horizontal
            } else {
                Orientation::Vertical
            })?;
            panel.SetSpacing(if cfg.horizontal { 16. } else { 0. })?;
            let children = panel.Children()?;
            children.Clear()?;
            radios.clear();
            let group = format!("prism-radio-{id}");
            for (i, o) in cfg.options.iter().enumerate() {
                let r = RadioButton::new()?;
                r.SetContent(&boxed(o)?)?;
                r.SetGroupName(&h(&group))?;
                r.SetMinWidth(0.)?;
                r.SetMinHeight(CONTROL_HEIGHT as f64)?;
                r.SetIsTextScaleFactorEnabled(false)?;
                r.Checked(&RoutedEventHandler::new(move |_, _| {
                    changed(id, i as f64);
                    Ok(())
                }))?;
                children.Append(&r)?;
                radios.push(r);
            }
        }
        Content::Web(w) => w.configure(config)?,
        Content::Video(v) => v.configure(config)?,
        Content::Date(_) | Content::Time(_) if kind == NodeKind::DatePicker => {
            let cfg = DateConfig::decode(config);
            let time = matches!(cfg.mode, DateMode::Time | DateMode::TimeOfDay);
            if time != matches!(c, Content::Time(_)) {
                *c = date_picker(id, cfg.mode)?;
            }
            if let Content::Date(d) = c {
                if let Some(min) = cfg.min {
                    d.SetMinDate(date_time(min))?;
                }
                if let Some(max) = cfg.max {
                    d.SetMaxDate(date_time(max))?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// The control shows `value` (`set_value`).
pub fn set_value(c: &Content, n: &Node, value: f64) -> Result<()> {
    match c {
        Content::Toggle(t) => {
            if t.IsOn()? != (value >= 0.5) {
                t.SetIsOn(value >= 0.5)?;
            }
        }
        Content::Check(cb, _) => {
            let on = value >= 0.5;
            let now = cb
                .IsChecked()
                .ok()
                .and_then(|v| v.Value().ok())
                .unwrap_or(false);
            if now != on {
                cb.SetIsChecked(&PropertyValue::CreateBoolean(on)?.cast::<IReference<bool>>()?)?;
            }
        }
        Content::Radios(_, radios) => {
            for (i, r) in radios.iter().enumerate() {
                let on = value >= 0. && i == value as usize;
                let now = r
                    .IsChecked()
                    .ok()
                    .and_then(|v| v.Value().ok())
                    .unwrap_or(false);
                if now != on {
                    r.SetIsChecked(&PropertyValue::CreateBoolean(on)?.cast::<IReference<bool>>()?)?;
                }
            }
        }
        Content::Slider(s) => {
            if (s.Value()? - value).abs() > f64::EPSILON {
                s.SetValue2(value)?;
            }
        }
        Content::Progress(p) => {
            p.SetIsIndeterminate(value < 0.)?;
            p.SetValue2(value.clamp(0., 1.))?;
        }
        Content::Ring(r) => {
            let d = if value == 1. { SPINNER } else { 20. };
            r.SetWidth(d as f64)?;
            r.SetHeight(d as f64)?;
        }
        Content::Combo(cb) => {
            let i = if value >= 0. { value as i32 } else { -1 };
            if cb.SelectedIndex()? != i {
                cb.SetSelectedIndex(i)?;
            }
            cb.SetPlaceholderText(&h(&n.placeholder))?;
        }
        Content::Date(d) => {
            if value.is_nan() {
                d.SetDate(None::<&IReference<DateTime>>)?;
            } else {
                let dt = date_time(value);
                let now = d.Date().ok().and_then(|v| v.Value().ok());
                if now.map(|n| n.UniversalTime / 10_000_000) != Some(dt.UniversalTime / 10_000_000)
                {
                    d.SetDate(&PropertyValue::CreateDateTime(dt)?.cast::<IReference<DateTime>>()?)?;
                }
            }
            d.SetPlaceholderText(&h(&n.placeholder))?;
        }
        Content::Time(t) => {
            if value.is_nan() {
                t.SetSelectedTime(None::<&IReference<TimeSpan>>)?;
            } else {
                // A time of day on its own is seconds since midnight (UTC);
                // a time is the local time of a Unix time.
                let own = DateConfig::decode(&n.config).mode == DateMode::TimeOfDay;
                let secs = if own {
                    value.rem_euclid(86_400.)
                } else {
                    crate::imp::seconds_of_day(value)
                };
                let span = TimeSpan {
                    Duration: (secs * 1e7) as i64,
                };
                t.SetSelectedTime(
                    &PropertyValue::CreateTimeSpan(span)?.cast::<IReference<TimeSpan>>()?,
                )?;
            }
        }
        Content::Picture(i) => {
            i.SetStretch(if value == 1. {
                Stretch::Uniform
            } else {
                Stretch::UniformToFill
            })?;
        }
        _ => {}
    }
    Ok(())
}

/// A symbol image's glyph (`set_icon`).
pub fn set_icon(c: &Content, icon: &str) -> Result<()> {
    if let Content::Icon(i) = c {
        let glyph = crate::keys::fluent_icon(icon)
            .map(String::from)
            .unwrap_or_else(|| prism_core::glyphs::glyph(icon).to_string());
        i.SetGlyph(&h(&glyph))?;
    }
    Ok(())
}

/// A shape's paths, in its own points.
pub fn set_paths(c: &Content, paths: &[prism_core::Path], pal: &Palette) -> Result<()> {
    let Content::Shape(canvas) = c else {
        return Ok(());
    };
    let children = canvas.Children()?;
    children.Clear()?;
    use prism_core::shape::Segment;
    for p in paths {
        let geometry = PathGeometry::new()?;
        if p.even_odd {
            geometry.SetFillRule(FillRule::EvenOdd)?;
        }
        let figures = geometry.Figures()?;
        let mut figure: Option<PathFigure> = None;
        let pt = |x: f32, y: f32| windows::Foundation::Point { X: x, Y: y };
        for seg in &p.segments {
            match *seg {
                Segment::Move(x, y) => {
                    let f = PathFigure::new()?;
                    f.SetStartPoint(pt(x, y))?;
                    f.SetIsFilled(p.fill.is_some())?;
                    figures.Append(&f)?;
                    figure = Some(f);
                }
                Segment::Line(x, y) => {
                    if let Some(f) = &figure {
                        let l = LineSegment::new()?;
                        l.SetPoint(pt(x, y))?;
                        f.Segments()?.Append(&l)?;
                    }
                }
                Segment::Cubic(x1, y1, x2, y2, x, y) => {
                    if let Some(f) = &figure {
                        let b = BezierSegment::new()?;
                        b.SetPoint1(pt(x1, y1))?;
                        b.SetPoint2(pt(x2, y2))?;
                        b.SetPoint3(pt(x, y))?;
                        f.Segments()?.Append(&b)?;
                    }
                }
                Segment::Close => {
                    if let Some(f) = &figure {
                        f.SetIsClosed(true)?;
                    }
                }
                // Arcs come as curves already (`set_drawing`).
                Segment::Arc { .. } => {}
            }
        }
        let path = XPath::new()?;
        path.SetData(&geometry)?;
        if let Some(f) = p.fill {
            path.SetFill(&brush(pal.resolve(f))?)?;
        }
        if let Some(s) = p.stroke {
            path.SetStroke(&brush(pal.resolve(s))?)?;
            path.SetStrokeThickness(p.width as f64)?;
            if p.round {
                path.SetStrokeStartLineCap(PenLineCap::Round)?;
                path.SetStrokeEndLineCap(PenLineCap::Round)?;
                path.SetStrokeLineJoin(PenLineJoin::Round)?;
            }
            if let Some((on, off)) = p.dash {
                let w = p.width.max(0.001);
                let d = crate::winui::xaml::Media::DoubleCollection::new()?;
                d.Append((on / w) as f64)?;
                d.Append((off / w) as f64)?;
                path.SetStrokeDashArray(&d)?;
            }
        }
        children.Append(&path)?;
    }
    Ok(())
}

/// A field's text, set by the app (`set_text`).
pub fn field_text(c: &Content) -> Option<String> {
    match c {
        Content::Field(t) => t.Text().ok().map(|s| s.to_string()),
        Content::Secure(p) => p.Password().ok().map(|s| s.to_string()),
        Content::Rich(r) => r.text(),
        _ => None,
    }
}

pub fn set_field_text(c: &Content, text: &str) -> Result<()> {
    match c {
        Content::Field(t) => {
            // Keeps the caret where it was, as far as the text allows.
            let caret = t.SelectionStart().unwrap_or(0);
            t.SetText(&h(text))?;
            let len = text.encode_utf16().count() as i32;
            t.SetSelectionStart(caret.min(len))?;
        }
        Content::Secure(p) => p.SetPassword(&h(text))?,
        // Shown with its runs (`set_rich_value`).
        Content::Rich(r) => r.set_text(text),
        _ => {}
    }
    Ok(())
}

/// Natural size of a leaf (`Widgets::measure`): WinUI's metrics around
/// the measured text.
pub fn measure(
    m: &mut Measurer,
    images: &crate::images::Images,
    n: &Node,
    id: NodeId,
    max_width: Option<f32>,
    locale: &prism_core::Locale,
) -> (f32, f32) {
    let s = &n.style;
    let text_size = |m: &Measurer, text: &str| m.measure(&label(text, s), None);
    match n.kind {
        NodeKind::Text | NodeKind::Timer => {
            let st = StyledText::new(&n.text, &n.config, s);
            if st.text().is_empty() {
                return (0., line_height(m, s));
            }
            m.node(id, &st, max_width)
        }
        NodeKind::Button => {
            let (w, h) = text_size(m, &n.text);
            if s.has_padding() {
                (w, h)
            } else {
                let (l, t, r, b) = BUTTON_PADDING;
                // The template's 1-pixel border on each side.
                (w + l + r + 2., (h + t + b + 2.).max(CONTROL_HEIGHT))
            }
        }
        k if is_field(k) => {
            let lh = line_height(m, s);
            let rows = InputConfig::decode(&n.config).rows.unwrap_or(1).max(1) as f32;
            let own = s.own_field_look();
            let (l, t, r, b) = if own { (0., 0., 0., 0.) } else { FIELD_PADDING };
            let content_h = if k == NodeKind::TextInput {
                lh
            } else {
                // Grows with its text (then scrolls, capped by max_h).
                let w = max_width.map(|w| (w - l - r - 2.).max(10.));
                let mut st = label(&n.text, s);
                st.no_wrap = false;
                let text_h = if n.text.is_empty() {
                    lh
                } else {
                    m.measure(&st, w).1
                };
                text_h.max(lh * rows)
            };
            // A one-line field is as wide as its placeholder or its text,
            // at least 120 (macOS's `fittingSize`, at least 120): two short
            // fields share a row. Taller fields keep 200.
            let natural = if k == NodeKind::TextInput {
                // A secure field's dots are not its text.
                let secure = InputConfig::decode(&n.config).secure;
                let widest = [&n.placeholder, &n.text]
                    .into_iter()
                    .filter(|t| !t.is_empty() && !(secure && std::ptr::eq(*t, &n.text)))
                    .map(|t| text_size(m, t).0)
                    .fold(0., f32::max);
                widest.ceil().max(120.)
            } else {
                200.
            };
            let w = max_width.map_or(natural, |m| m.min(natural));
            if own || s.has_padding() {
                (w, content_h)
            } else {
                (w + l + r + 2., (content_h + t + b + 2.).max(CONTROL_HEIGHT))
            }
        }
        NodeKind::Toggle => TOGGLE,
        // WinUI's check box sits in a row 32 high.
        NodeKind::Checkbox => {
            if n.text.is_empty() {
                return (CHECKBOX, CONTROL_HEIGHT);
            }
            let (w, h) = text_size(m, &n.text);
            (CHECKBOX + 8. + w, h.max(CONTROL_HEIGHT))
        }
        NodeKind::RadioGroup => {
            let c = RadioConfig::decode(&n.config);
            let sizes: Vec<(f32, f32)> = c.options.iter().map(|o| text_size(m, o)).collect();
            let item = |(w, _h): &(f32, f32)| CHECKBOX + 8. + w;
            if c.horizontal {
                let w: f32 = sizes.iter().map(item).sum::<f32>()
                    + 16. * sizes.len().saturating_sub(1) as f32;
                (w, CONTROL_HEIGHT)
            } else {
                let w = sizes.iter().map(item).fold(0., f32::max);
                (w, CONTROL_HEIGHT * sizes.len() as f32)
            }
        }
        NodeKind::Slider => (max_width.map_or(200., |m| m.min(200.)), CONTROL_HEIGHT),
        NodeKind::Progress => (max_width.map_or(200., |m| m.min(200.)), 4.),
        NodeKind::Spinner => {
            let d = if n.value == 1. { SPINNER } else { 20. };
            (d, d)
        }
        NodeKind::Picker | NodeKind::DatePicker => {
            let text = if n.kind == NodeKind::Picker {
                // As wide as its widest option, so choosing does not move it.
                let c = PickerConfig::decode(&n.config);
                c.options
                    .iter()
                    .chain(std::iter::once(&n.placeholder))
                    .max_by_key(|o| o.chars().count())
                    .cloned()
                    .unwrap_or_default()
            } else {
                date_text(n, locale)
            };
            let (w, h) = text_size(m, &text);
            let time = n.kind == NodeKind::DatePicker
                && matches!(
                    DateConfig::decode(&n.config).mode,
                    DateMode::Time | DateMode::TimeOfDay
                );
            // A time picker shows hours and minutes (and AM/PM) in
            // columns: WinUI's minimum width (`TimePickerThemeMinWidth`).
            let min = if time { 242. } else { 0. };
            ((w + 11. + 38.).max(min), (h + 11.).max(CONTROL_HEIGHT))
        }
        NodeKind::Image => {
            let d = s.font_size.max(1.) * 1.2;
            (d, d)
        }
        NodeKind::Picture => images.size_of(&n.text).unwrap_or((0., 0.)),
        NodeKind::Video => crate::media::measure(id, max_width),
        _ => (0., 0.),
    }
}

/// The natural line height of a node's font.
pub fn line_height(m: &Measurer, s: &Style) -> f32 {
    if let Some(lh) = s.line_height_points() {
        return lh;
    }
    m.measure(&label("Ag", s), None).1
}

fn date_text(n: &Node, locale: &prism_core::Locale) -> String {
    use prism_core::Formatter;
    let c = DateConfig::decode(&n.config);
    if n.value.is_nan() {
        return n.placeholder.clone();
    }
    let f = prism_core::BasicFormatter::default();
    match c.mode {
        DateMode::Date => f.date(locale, n.value, prism_core::DateStyle::Medium),
        DateMode::Time | DateMode::TimeOfDay => f.time(locale, n.value),
        DateMode::DateTime => format!(
            "{} {}",
            f.date(locale, n.value, prism_core::DateStyle::Medium),
            f.time(locale, n.value)
        ),
    }
}

/// Distance from the top of a measured leaf to its first baseline.
pub fn baseline(m: &Measurer, n: &Node, width: f32, height: f32) -> Option<f32> {
    let s = &n.style;
    match n.kind {
        NodeKind::Text | NodeKind::Timer => {
            m.baseline(&StyledText::new(&n.text, &n.config, s), width)
        }
        NodeKind::Button | NodeKind::Picker | NodeKind::DatePicker | NodeKind::Checkbox => {
            let st = label(if n.text.is_empty() { "Ag" } else { &n.text }, s);
            let (_, h) = m.measure(&st, None);
            Some((height - h) / 2. + m.baseline(&st, 1e5)?)
        }
        k if is_field(k) => {
            let st = label("Ag", s);
            let (_, h) = m.measure(&st, None);
            let top = if k == NodeKind::TextInput {
                (height - h) / 2.
            } else if s.own_field_look() {
                s.padding.top
            } else {
                FIELD_PADDING.1 + 1.
            };
            Some(top + m.baseline(&st, 1e5)?)
        }
        _ => None,
    }
}

/// A picture's bitmap, from WIC's decoded pixels (`WriteableBitmap`: no
/// second decode, no async load).
pub fn set_picture(
    c: &Content,
    bitmap: Option<&windows::Win32::Graphics::Imaging::IWICBitmap>,
) -> Result<()> {
    let Content::Picture(img) = c else {
        return Ok(());
    };
    let Some(bitmap) = bitmap else {
        img.SetSource(None::<&crate::winui::xaml::Media::ImageSource>)?;
        return Ok(());
    };
    use crate::winui::xaml::Media::Imaging::WriteableBitmap;
    use windows::Win32::System::WinRT::IBufferByteAccess;
    unsafe {
        let (mut w, mut h) = (0u32, 0u32);
        bitmap.GetSize(&mut w, &mut h)?;
        let wb = WriteableBitmap::CreateInstanceWithDimensions(w as i32, h as i32)?;
        let buffer = wb.PixelBuffer()?;
        let access: IBufferByteAccess = buffer.cast()?;
        let len = buffer.Capacity()?;
        let dst = std::slice::from_raw_parts_mut(access.Buffer()?, len as usize);
        bitmap.CopyPixels(std::ptr::null(), w * 4, dst)?;
        buffer.SetLength(len)?;
        wb.Invalidate()?;
        img.SetSource(&wb)?;
    }
    Ok(())
}
