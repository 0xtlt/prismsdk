//! Prism's controls as WinUI 3 controls: `Button`, `TextBox` /
//! `PasswordBox`, `ToggleSwitch`, `CheckBox`, `RadioButton`s, `Slider`,
//! `ProgressBar`, `ProgressRing`, `ComboBox`, `CalendarDatePicker` and
//! `TimePicker` (`datebox.rs`), `FontIcon` (Segoe Fluent Icons), `Image`,
//! vector `Path`s.
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
use windows::Foundation::{DateTime, IReference, PropertyValue};
use windows::System::VirtualKey;
use windows_core::{Interface, Result};

use crate::fluent::Palette;
use crate::fluent::metrics::*;
use crate::imp::{changed, dispatch, typed};
use crate::scene::Node;
use crate::text::{Measurer, apply, font, label, weight};
use crate::winui::xaml::Controls::Primitives::RangeBaseValueChangedEventHandler;
use crate::winui::xaml::Controls::{
    Button, Canvas, CheckBox, ComboBox, Control, FontIcon, Image, Orientation, PasswordBox,
    ProgressBar, ProgressRing, RadioButton, SelectionChangedEventHandler, Slider, StackPanel,
    TextBlock, TextBox, TextChangedEventHandler, ToggleSwitch,
};
use crate::winui::xaml::Input::{InputScope, InputScopeName, InputScopeNameValue, KeyEventHandler};
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
    Date(crate::datebox::DateBox),
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
            Content::Date(d) => d.element(),
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
            Content::Date(d) => return d.control(),
            Content::Rich(r) => r.element.cast(),
            Content::Web(w) => w.element.cast(),
            _ => return None,
        };
        c.ok()
    }

    /// The element that carries the node's accessibility: a date picker's
    /// day or time control, not its panel.
    pub fn named(&self) -> Option<crate::winui::xaml::UIElement> {
        match self {
            Content::Date(d) => d.control()?.cast().ok(),
            _ => self.element(),
        }
    }

    /// A date picker's mode (what it is made of).
    pub fn date_mode(&self) -> Option<DateMode> {
        match self {
            Content::Date(d) => Some(d.mode),
            _ => None,
        }
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

/// A rich input's runs as its measure lays them out: its bold or larger
/// words, and each chip (one character in the field) as wide as its
/// picture (`chips.rs`): its label, 0.35 em of padding on each side and,
/// with a symbol, the symbol (1 em) and a 0.25 em gap. Em and en spaces
/// stand for those, joined to the label so they never break from it.
fn rich_label(runs: &str, s: &Style) -> StyledText {
    const JOIN: char = '\u{2060}';
    let mut runs = prism_core::decode_runs(runs);
    for r in &mut runs {
        if r.chip.is_none() {
            continue;
        }
        let label = r
            .chip_label
            .as_deref()
            .unwrap_or_default()
            .replace(' ', "\u{a0}");
        let room = if r.chip_icon.is_some() {
            format!("\u{2003}{JOIN}\u{2003}")
        } else {
            format!("\u{2002}{JOIN}\u{2009}")
        };
        r.text = format!("{JOIN}{room}{JOIN}{label}{JOIN}");
        r.icon = None;
    }
    let text: String = runs.iter().map(|r| r.text.as_str()).collect();
    // The value is edited as typed: no case transform.
    let style = Style {
        text_transform: prism_core::TextTransform::None,
        ..s.clone()
    };
    let mut st = StyledText::from_runs(&text, &runs, &style);
    st.align = prism_core::TextAlign::Left;
    st.max_lines = None;
    st
}

/// A text area's or rich input's text insets (left, top, right, bottom)
/// when it draws its own look without a padding: macOS's
/// `textContainerInset` and GTK's margins.
const AREA_INSET: (f32, f32, f32, f32) = (10., 8., 10., 8.);

/// The insets of a field that draws its own look: its padding, an explicit
/// zero included (#69); without one, [`AREA_INSET`] for a text area or a
/// rich input, none for a one-line field.
pub fn own_insets(n: &Node) -> (f32, f32, f32, f32) {
    let s = &n.style;
    if s.has_padding() {
        let p = s.padding;
        (p.left, p.top, p.right, p.bottom)
    } else if n.kind == NodeKind::TextInput {
        (0., 0., 0., 0.)
    } else {
        AREA_INSET
    }
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
                crate::events::button_clicked(id);
                Ok(())
            }))?;
            crate::events::button_presses(&b, id)?;
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
        NodeKind::DatePicker => Content::Date(crate::datebox::DateBox::new(id, DateMode::Date)?),
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
            crate::carets::changed(id);
            crate::overlay::caret_moved();
        }
        Ok(())
    }))?;
    // Drags across a selection group's fields (#264).
    if let Err(e) = crate::across::connect(id, &t.cast()?) {
        crate::win::log!("text field {id}: drags across fields: {e}");
    }
    t.SelectionChanged(&RoutedEventHandler::new(move |s, _| {
        if let Some(t) = s.as_ref().and_then(|s| s.cast::<TextBox>().ok()) {
            let text = t.Text()?.to_string();
            let a = t.SelectionStart()?.max(0) as usize;
            let b = a + t.SelectionLength()?.max(0) as usize;
            crate::imp::selected(id, &text, a, b);
            crate::overlay::caret_moved();
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
        crate::suggest::connect(id, &t)?;
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
    pal.ink(s.text_color.unwrap_or(pal.text))
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
                    r.SetForeground(&brush(pal.ink(c))?)?;
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
        Content::Date(d) => d.set_enabled(!n.a11y.disabled)?,
        Content::Progress(p) => {
            if let Some(c) = s.text_color {
                p.SetForeground(&brush(pal.ink(c))?)?;
            }
        }
        Content::Ring(r) => {
            if let Some(c) = s.text_color {
                r.SetForeground(&brush(pal.ink(c))?)?;
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
        s.font_weight.css(),
        s.italic,
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
        // A leading icon (`fieldicon.rs`) takes the start of the field.
        let lead = crate::fieldicon::leading(n);
        let (_, t, r, b) = own_insets(n);
        c.SetPadding(thickness(lead, t, r, b))?;
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
    // Its custom automation peer gives it the node's role (`peers.rs`).
    let b = crate::peers::button(id)?;
    override_brushes(&b.Resources()?, &BUTTON_CHROME, &transparent()?.cast()?)?;
    b.SetBorderThickness(uniform(0.))?;
    b.SetPadding(uniform(0.))?;
    b.SetMinWidth(0.)?;
    b.SetMinHeight(0.)?;
    b.SetHorizontalContentAlignment(HorizontalAlignment::Left)?;
    b.SetVerticalContentAlignment(VerticalAlignment::Top)?;
    b.SetIsTextScaleFactorEnabled(false)?;
    b.Click(&RoutedEventHandler::new(move |_, _| {
        crate::events::button_clicked(id);
        Ok(())
    }))?;
    crate::events::button_presses(&b, id)?;
    Ok(b)
}

/// A field's configuration: secure (a `PasswordBox` in place of the
/// `TextBox`), length limit. Returns a new content when it must change.
pub fn configure_field(c: &Content, id: NodeId, config: &str) -> Result<Option<Content>> {
    let list = prism_core::controls::input_suggestions(config);
    let config = InputConfig::decode(config);
    let traits = crate::inputs::traits(&config);
    match c {
        Content::Field(_) if config.secure => {
            crate::suggest::set(id, Vec::new())?;
            let p = secure(id)?;
            configure_field(&p, id, &InputConfig::encode(&config))?;
            return Ok(Some(p));
        }
        Content::Field(t) => {
            t.SetMaxLength(config.max_length.unwrap_or(0) as i32)?;
            t.SetInputScope(&input_scope(traits.scope)?)?;
            t.SetIsSpellCheckEnabled(traits.spell_check)?;
            t.SetIsTextPredictionEnabled(traits.prediction)?;
            crate::suggest::set(id, if t.AcceptsReturn()? { Vec::new() } else { list })?;
        }
        Content::Secure(p) => {
            p.SetMaxLength(config.max_length.unwrap_or(0) as i32)?;
            p.SetInputScope(&input_scope(traits.scope)?)?;
        }
        _ => {}
    }
    Ok(None)
}

/// An input scope (the touch keyboard's layout and keys).
fn input_scope(s: crate::inputs::Scope) -> Result<InputScope> {
    use crate::inputs::Scope;
    let value = match s {
        Scope::Default => InputScopeNameValue::Default,
        Scope::Search => InputScopeNameValue::Search,
        Scope::NameOrPhoneNumber => InputScopeNameValue::NameOrPhoneNumber,
        Scope::EmailSmtpAddress => InputScopeNameValue::EmailSmtpAddress,
        Scope::Digits => InputScopeNameValue::Digits,
        Scope::Number => InputScopeNameValue::Number,
        Scope::TelephoneNumber => InputScopeNameValue::TelephoneNumber,
        Scope::Url => InputScopeNameValue::Url,
        Scope::Password => InputScopeNameValue::Password,
        Scope::NumericPin => InputScopeNameValue::NumericPin,
    };
    let name = InputScopeName::new()?;
    name.SetNameValue(value)?;
    let scope = InputScope::new()?;
    scope.Names()?.Append(&name)?;
    Ok(scope)
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
        Content::Date(_) if kind == NodeKind::DatePicker => {
            let cfg = DateConfig::decode(config);
            if c.date_mode() != Some(cfg.mode) {
                // Other parts (a day, a time, both): a new picker.
                *c = Content::Date(crate::datebox::DateBox::new(id, cfg.mode)?);
            }
            if let Content::Date(d) = c {
                d.configure(&cfg)?;
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
        Content::Date(d) => d.set_value(n, value)?,
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
            path.SetFill(&brush(pal.graphic(f))?)?;
        }
        if let Some(s) = p.stroke {
            path.SetStroke(&brush(pal.graphic(s))?)?;
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
    rich: Option<&str>,
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
            // Its minimum lines (#235): three unless set, one for a rich input.
            let rows = InputConfig::decode(&n.config).min_rows().max(1) as f32;
            let own = s.own_field_look();
            let (l, t, r, b) = if own { (0., 0., 0., 0.) } else { FIELD_PADDING };
            // Its own look: no template border, and a padding is taffy's,
            // around this size; a text area without one has its insets.
            let inset = own && !s.has_padding() && k != NodeKind::TextInput;
            let (il, it, ir, ib) = if inset { AREA_INSET } else { (0., 0., 0., 0.) };
            let content_h = if k == NodeKind::TextInput {
                lh
            } else {
                // Grows with its text (then scrolls, capped by max_h).
                let frame = if own { il + ir } else { l + r + 2. };
                let w = max_width.map(|w| (w - frame).max(10.));
                let mut st = match rich {
                    Some(runs) if k == NodeKind::RichInput && !runs.is_empty() => {
                        rich_label(runs, s)
                    }
                    _ => label(&n.text, s),
                };
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
                (w, content_h + it + ib)
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
        NodeKind::Picker => {
            // As wide as its widest option, so choosing does not move it.
            let c = PickerConfig::decode(&n.config);
            let text = c
                .options
                .iter()
                .chain(std::iter::once(&n.placeholder))
                .max_by_key(|o| o.chars().count())
                .cloned()
                .unwrap_or_default();
            let (w, h) = text_size(m, &text);
            (w + 11. + 38., (h + 11.).max(CONTROL_HEIGHT))
        }
        NodeKind::DatePicker => {
            use crate::datebox::{CLEAR_WIDTH, GAP};
            let c = DateConfig::decode(&n.config);
            let (day, time) = crate::dates::parts(c.mode);
            let mut w = 0.;
            let mut h: f32 = 0.;
            if day {
                let (tw, th) = text_size(m, &date_text(n, locale, false));
                w += tw + 11. + 38.;
                h = h.max(th);
            }
            if time {
                let (tw, th) = text_size(m, &date_text(n, locale, true));
                // Hours and minutes (and AM/PM) in columns: WinUI's
                // minimum width (`TimePickerThemeMinWidth`).
                w += if day { GAP } else { 0. } + (tw + 11. + 38.).max(242.);
                h = h.max(th);
            }
            if c.optional {
                w += GAP + CLEAR_WIDTH;
            }
            (w, (h + 11.).max(CONTROL_HEIGHT))
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

/// What a date picker's day part (or its time part) shows.
fn date_text(n: &Node, locale: &prism_core::Locale, time: bool) -> String {
    use prism_core::Formatter;
    if n.value.is_nan() {
        return n.placeholder.clone();
    }
    let f = prism_core::BasicFormatter::default();
    if time {
        f.time(locale, n.value)
    } else {
        f.date(locale, n.value, prism_core::DateStyle::Medium)
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
                own_insets(n).1
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
