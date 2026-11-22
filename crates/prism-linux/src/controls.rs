//! Form controls: `GtkScale` (slider), `GtkDropDown` (picker), a
//! `GtkMenuButton` with a `GtkCalendar` and spin buttons (date / time
//! picker), `GtkTextView` (text area), `GtkSpinner`, and the text input
//! traits of `GtkEntry` / `GtkTextView`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use prism_core::controls::{
    Capitalize, DateConfig, DateMode, InputConfig, Keyboard, PickerConfig, SliderConfig,
};
use prism_core::{BasicFormatter, DateStyle, Formatter, Locale, NodeId, UiEvent};

use crate::imp::dispatch;

// --- Slider -----------------------------------------------------------------

/// A horizontal `GtkScale` without its value label. User changes snap to
/// the step (`change-value`), then report `Change` (`value-changed`).
/// `GtkScale` is flippable: it fills from the right in right-to-left.
pub fn slider(id: NodeId) -> (gtk::Scale, Rc<Cell<SliderConfig>>) {
    let config = Rc::new(Cell::new(SliderConfig::default()));
    let scale = gtk::Scale::new(gtk::Orientation::Horizontal, None::<&gtk::Adjustment>);
    scale.set_draw_value(false);
    scale.set_valign(gtk::Align::Center);
    configure_slider(&scale, SliderConfig::default());
    let c = config.clone();
    scale.connect_change_value(move |range, _, value| {
        range.set_value(c.get().snap(value));
        glib::Propagation::Stop
    });
    scale.connect_value_changed(move |range| dispatch(UiEvent::Change(id, range.value())));
    (scale, config)
}

pub fn configure_slider(scale: &gtk::Scale, c: SliderConfig) {
    let adj = scale.adjustment();
    let step = c.a11y_step().max(f64::EPSILON);
    adj.configure(
        c.snap(adj.value()),
        c.min,
        c.max,
        step,
        step.max((c.max - c.min) / 10.),
        0.,
    );
}

// --- Picker -----------------------------------------------------------------

/// A `GtkDropDown` over a `GtkStringList`. With nothing selected
/// (`GTK_INVALID_LIST_POSITION`) the drop-down's button shows its "empty"
/// page (gtkdropdown.ui: a label reading "(None)"): that label shows the
/// placeholder, dimmed. The button's stack is homogeneous, so the
/// placeholder counts in its width.
pub struct Picker {
    pub dropdown: gtk::DropDown,
    /// The label of the button's "empty" page (absent in a future GTK
    /// without it: then nothing shows).
    empty: Option<gtk::Label>,
    /// The selected index from Prism (-1: none), kept across new options.
    selected: Cell<f64>,
}

/// The label of the drop-down button's "empty" stack page.
fn empty_label(w: &gtk::Widget) -> Option<gtk::Label> {
    if let Some(stack) = w.downcast_ref::<gtk::Stack>()
        && let Some(label) = stack
            .child_by_name("empty")
            .and_then(|c| c.downcast::<gtk::Label>().ok())
    {
        return Some(label);
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        if let Some(l) = empty_label(&c) {
            return Some(l);
        }
        child = c.next_sibling();
    }
    None
}

pub fn picker(id: NodeId) -> Rc<Picker> {
    let dropdown = gtk::DropDown::from_strings(&[]);
    dropdown.set_valign(gtk::Align::Center);
    let empty = empty_label(dropdown.upcast_ref());
    // The list's rows are created when it opens: they follow the
    // drop-down's direction, and get their names on GTK before 4.12
    // (`names.rs`).
    let mut child = dropdown.first_child();
    while let Some(c) = child {
        if c.is::<gtk::Popover>() {
            let d = dropdown.downgrade();
            c.connect_map(move |popover| {
                let Some(d) = d.upgrade() else { return };
                let (popover, rtl) = (popover.clone(), d.direction() == gtk::TextDirection::Rtl);
                glib::idle_add_local_once(move || set_direction_deep(&popover, rtl));
                crate::names::schedule();
            });
        }
        child = c.next_sibling();
    }
    if let Some(l) = &empty {
        l.set_text("");
        l.add_css_class("dim-label");
        l.set_xalign(0.);
    }
    dropdown.connect_selected_notify(move |d| {
        if d.selected() != gtk::INVALID_LIST_POSITION {
            dispatch(UiEvent::Change(id, d.selected() as f64));
        }
    });
    let p = Rc::new(Picker {
        dropdown,
        empty,
        selected: Cell::new(-1.),
    });
    p.select(-1.);
    p
}

impl Picker {
    pub fn set_options(&self, config: &str) {
        let options = PickerConfig::decode(config).options;
        let refs: Vec<&str> = options.iter().map(String::as_str).collect();
        self.dropdown.set_model(Some(&gtk::StringList::new(&refs)));
        // A new model selects its first item: show Prism's selection again.
        self.select(self.selected.get());
    }

    pub fn select(&self, index: f64) {
        self.selected.set(index);
        let n = self.dropdown.model().map_or(0, |m| m.n_items());
        let position = if index >= 0. && (index as u32) < n {
            index as u32
        } else {
            gtk::INVALID_LIST_POSITION
        };
        self.dropdown.set_selected(position);
    }

    pub fn set_placeholder(&self, text: &str) {
        if let Some(l) = &self.empty {
            l.set_text(text);
        }
    }
}

// --- Date picker ------------------------------------------------------------

/// A button showing the formatted date and/or time; it opens a popover
/// with a `GtkCalendar` (date) and hour / minute spin buttons (time).
/// `GtkCalendar` has no minimum or maximum date: choices outside the
/// bounds are clamped (and the calendar moves back to the clamped day).
///
/// An optional picker (#89) shows its placeholder, dimmed, while empty
/// (`NaN`); opening it fills it with the current moment, and a "Clear"
/// button under the calendar empties it.
pub struct DatePicker {
    pub button: gtk::MenuButton,
    label: gtk::Label,
    calendar: gtk::Calendar,
    time_row: gtk::Box,
    hour: gtk::SpinButton,
    minute: gtk::SpinButton,
    clear: gtk::Button,
    popover: gtk::Popover,
    placeholder: RefCell<String>,
    value: Cell<f64>,
    config: Cell<DateConfig>,
    /// Set while the popover follows the value (its signals are ignored).
    syncing: Cell<bool>,
    locale: RefCell<Locale>,
}

fn local(unix: f64) -> Option<glib::DateTime> {
    glib::DateTime::from_unix_local(unix.floor() as i64).ok()
}

/// A time of day is the same in every time zone: shown in UTC.
fn utc(unix: f64) -> Option<glib::DateTime> {
    glib::DateTime::from_unix_utc(unix.floor() as i64).ok()
}

fn two_digits(spin: &gtk::SpinButton) {
    spin.connect_output(|s| {
        s.set_text(&format!("{:02}", s.value_as_int()));
        glib::Propagation::Stop
    });
}

pub fn date_picker(id: NodeId, locale: Locale) -> Rc<DatePicker> {
    let label = gtk::Label::new(None);
    let button = gtk::MenuButton::new();
    button.set_child(Some(&label));
    button.set_always_show_arrow(true);
    button.set_valign(gtk::Align::Center);
    let calendar = gtk::Calendar::new();
    let hour = gtk::SpinButton::with_range(0., 23., 1.);
    let minute = gtk::SpinButton::with_range(0., 59., 1.);
    for spin in [&hour, &minute] {
        spin.set_orientation(gtk::Orientation::Vertical);
        spin.set_wrap(true);
        spin.set_numeric(true);
        spin.set_width_chars(2);
        two_digits(spin);
    }
    let time_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    time_row.set_halign(gtk::Align::Center);
    time_row.append(&hour);
    time_row.append(&gtk::Label::new(Some(":")));
    time_row.append(&minute);
    let clear = gtk::Button::with_label("Clear");
    clear.set_halign(gtk::Align::End);
    clear.set_visible(false);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.append(&calendar);
    content.append(&time_row);
    content.append(&clear);
    let popover = gtk::Popover::new();
    popover.set_child(Some(&content));
    button.set_popover(Some(&popover));
    let p = Rc::new(DatePicker {
        button,
        label,
        calendar,
        time_row,
        hour,
        minute,
        clear,
        popover: popover.clone(),
        placeholder: RefCell::new(String::new()),
        value: Cell::new(0.),
        config: Cell::new(DateConfig::default()),
        syncing: Cell::new(false),
        locale: RefCell::new(locale),
    });
    let weak = Rc::downgrade(&p);
    p.calendar.connect_day_selected(move |_| {
        if let Some(p) = weak.upgrade() {
            p.edited(id);
        }
    });
    for spin in [&p.hour, &p.minute] {
        let weak = Rc::downgrade(&p);
        spin.connect_value_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                p.edited(id);
            }
        });
    }
    // Opening shows the current value; an empty one starts from now.
    let weak = Rc::downgrade(&p);
    popover.connect_show(move |_| {
        if let Some(p) = weak.upgrade() {
            if p.value.get().is_nan() {
                p.fill(id);
            }
            p.sync();
        }
    });
    let weak = Rc::downgrade(&p);
    p.clear.connect_clicked(move |_| {
        if let Some(p) = weak.upgrade() {
            p.popover.popdown();
            p.value.set(f64::NAN);
            p.sync();
            dispatch(UiEvent::Change(id, f64::NAN));
        }
    });
    p.set_config("");
    p
}

impl DatePicker {
    pub fn set_config(&self, config: &str) {
        let c = DateConfig::decode(config);
        self.config.set(c);
        self.calendar
            .set_visible(!matches!(c.mode, DateMode::Time | DateMode::TimeOfDay));
        self.time_row.set_visible(c.mode != DateMode::Date);
        self.sync();
    }

    pub fn set_value(&self, value: f64) {
        self.value.set(value);
        self.sync();
    }

    pub fn set_locale(&self, locale: Locale) {
        *self.locale.borrow_mut() = locale;
        self.sync();
    }

    /// Shown, dimmed, while an optional picker is empty.
    pub fn set_placeholder(&self, text: &str) {
        *self.placeholder.borrow_mut() = text.to_string();
        self.sync();
    }

    /// The label of the button that empties an optional picker.
    pub fn set_clear_label(&self, text: &str) {
        self.clear.set_label(text);
    }

    /// An empty picker being opened: the current moment, within the bounds.
    fn fill(&self, id: NodeId) {
        let Ok(now) = glib::DateTime::now_local() else {
            return;
        };
        let offset = now.utc_offset().as_seconds() as f64;
        let v = self.config.get().start(now.to_unix() as f64, offset);
        self.value.set(v);
        dispatch(UiEvent::Change(id, v));
    }

    /// The value in the time zone it is shown in.
    fn at(&self, unix: f64) -> Option<glib::DateTime> {
        if self.config.get().mode == DateMode::TimeOfDay {
            utc(unix)
        } else {
            local(unix)
        }
    }

    /// The value as shown, in the device's time zone (UTC for a time of
    /// day) and the app's locale.
    fn text(&self) -> String {
        let v = self.value.get();
        let offset = self.at(v).map_or(0, |d| d.utc_offset().as_seconds() as i32);
        let f = BasicFormatter { utc_offset: offset };
        let locale = self.locale.borrow();
        match self.config.get().mode {
            DateMode::Date => f.date(&locale, v, DateStyle::Medium),
            DateMode::Time | DateMode::TimeOfDay => f.time(&locale, v),
            DateMode::DateTime => format!(
                "{}, {}",
                f.date(&locale, v, DateStyle::Medium),
                f.time(&locale, v)
            ),
        }
    }

    fn sync(&self) {
        let empty = self.value.get().is_nan();
        self.clear.set_visible(self.config.get().optional && !empty);
        if empty {
            self.label.set_text(&self.placeholder.borrow());
            self.label.add_css_class("dim-label");
            return;
        }
        self.label.remove_css_class("dim-label");
        self.label.set_text(&self.text());
        let Some(d) = self.at(self.value.get()) else {
            return;
        };
        self.syncing.set(true);
        self.calendar.select_day(&d);
        self.hour.set_value(d.hour() as f64);
        self.minute.set_value(d.minute() as f64);
        self.syncing.set(false);
    }

    /// The user picked a day, an hour or a minute.
    fn edited(&self, id: NodeId) {
        if self.syncing.get() {
            return;
        }
        let c = self.config.get();
        if c.mode == DateMode::TimeOfDay {
            let minutes = self.hour.value_as_int() * 60 + self.minute.value_as_int();
            let v = f64::from(minutes * 60);
            self.value.set(v);
            self.sync();
            dispatch(UiEvent::Change(id, v));
            return;
        }
        let Some(cur) = local(self.value.get()) else {
            return;
        };
        let day = self.calendar.date();
        let (y, m, d) = if c.mode == DateMode::Time {
            (cur.year(), cur.month(), cur.day_of_month())
        } else {
            (day.year(), day.month(), day.day_of_month())
        };
        let (h, min) = if c.mode == DateMode::Date {
            (cur.hour(), cur.minute())
        } else {
            (self.hour.value_as_int(), self.minute.value_as_int())
        };
        let Ok(t) = glib::DateTime::from_local(y, m, d, h, min, cur.second() as f64) else {
            return;
        };
        let v = c.clamp(t.to_unix() as f64);
        self.value.set(v);
        self.sync();
        dispatch(UiEvent::Change(id, v));
    }
}

// --- Text area ----------------------------------------------------------------

/// Insets of the text inside a text area (px).
const AREA_PAD_X: i32 = 10;
const AREA_PAD_Y: i32 = 8;

/// A word-wrapping `GtkTextView` in a `GtkScrolledWindow` (it scrolls once
/// Prism caps its height), with a dim label as placeholder: `GtkTextView`
/// has none.
pub struct TextArea {
    pub root: gtk::Overlay,
    pub scroll: gtk::ScrolledWindow,
    pub view: gtk::TextView,
    placeholder: gtk::Label,
    /// The text's insets: left, right, top, bottom (its padding, #14).
    pad: Cell<(i32, i32, i32, i32)>,
    /// Its minimum lines (#235).
    pub rows: Cell<u32>,
}

pub fn text_area(id: NodeId) -> Rc<TextArea> {
    let view = gtk::TextView::new();
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_top_margin(AREA_PAD_Y);
    view.set_bottom_margin(AREA_PAD_Y);
    view.set_left_margin(AREA_PAD_X);
    view.set_right_margin(AREA_PAD_X);
    view.set_accepts_tab(false);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_child(Some(&view));
    let placeholder = gtk::Label::new(None);
    placeholder.add_css_class("dim-label");
    placeholder.set_halign(gtk::Align::Start);
    placeholder.set_valign(gtk::Align::Start);
    placeholder.set_margin_start(AREA_PAD_X);
    placeholder.set_margin_end(AREA_PAD_X);
    placeholder.set_margin_top(AREA_PAD_Y);
    placeholder.set_wrap(true);
    placeholder.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    placeholder.set_xalign(0.);
    placeholder.set_can_target(false);
    let root = gtk::Overlay::new();
    root.set_child(Some(&scroll));
    root.add_overlay(&placeholder);
    // A mouse drag into the other fields of a selection group (#231).
    crate::across::attach_drag(id, &view);
    let label = placeholder.clone();
    let weak = view.downgrade();
    view.buffer().connect_changed(move |b| {
        // A rich input's chips are characters of its value (#115).
        let text = crate::rich::slice(b);
        label.set_visible(text.is_empty());
        crate::imp::dispatch_typed(id, text);
        crate::imp::report_selection(id, crate::imp::buffer_selection(b));
        // The text under the selection moved (#217).
        if let Some(view) = weak.upgrade() {
            crate::edit::rect_changed(id, &view);
        }
    });
    // The caret and the selection's other end moved (#86).
    let weak = view.downgrade();
    view.buffer().connect_mark_set(move |b, _, mark| {
        if matches!(mark.name().as_deref(), Some("insert" | "selection_bound")) {
            crate::imp::report_selection(id, crate::imp::buffer_selection(b));
            if let Some(view) = weak.upgrade() {
                crate::edit::rect_changed(id, &view);
            }
        }
    });
    // A rich input's copy, cut, paste and modifier-clicks (#217).
    crate::edit::connect(id, &view);
    Rc::new(TextArea {
        root,
        scroll,
        view,
        placeholder,
        pad: Cell::new((AREA_PAD_X, AREA_PAD_X, AREA_PAD_Y, AREA_PAD_Y)),
        rows: Cell::new(3),
    })
}

impl TextArea {
    pub fn text(&self) -> String {
        crate::rich::slice(&self.view.buffer())
    }

    /// Draws `start..end` (characters) like a selection without selecting
    /// it (#231): a tag of the buffer, `start == end` removing it.
    pub fn highlight(&self, start: usize, end: usize) {
        let b = self.view.buffer();
        let table = b.tag_table();
        let tag = table.lookup("prism-across").unwrap_or_else(|| {
            let t = gtk::TextTag::new(Some("prism-across"));
            // The theme's selection colour, as GTK 4 draws it.
            t.set_background(Some("rgba(53, 132, 228, 0.35)"));
            table.add(&t);
            t
        });
        b.remove_tag(&tag, &b.start_iter(), &b.end_iter());
        let len = b.char_count().max(0) as usize;
        if start < end.min(len) {
            let at = |c: usize| b.iter_at_offset(c.min(len) as i32);
            b.apply_tag(&tag, &at(start), &at(end));
        }
    }

    /// Selects `start..end` (characters, clamped), the caret at `end` (#86).
    pub fn select(&self, start: usize, end: usize) {
        let b = self.view.buffer();
        let len = b.char_count().max(0) as usize;
        let at = |c: usize| b.iter_at_offset(c.min(len) as i32);
        b.select_range(&at(end.max(start)), &at(start));
    }

    /// Prism's value, written when `takes` (the app's, not an echo of the
    /// typing, #73), the cursor keeping its distance from the end.
    pub fn set_text(&self, text: &str, takes: bool) {
        if takes {
            let b = self.view.buffer();
            let caret = prism_native::fields::caret_from_end(
                b.char_count().max(0) as usize,
                b.cursor_position().max(0) as usize,
                text.chars().count(),
            );
            b.set_text(text);
            b.place_cursor(&b.iter_at_offset(caret as i32));
        }
        self.placeholder.set_visible(self.text().is_empty());
    }

    pub fn set_placeholder(&self, text: &str) {
        self.placeholder.set_text(text);
    }

    /// Its padding replaces the text's own insets (10 × 8), left and right
    /// as drawn (already mirrored right to left); `None`: none set (an
    /// explicit zero is a real one, #69).
    pub fn set_padding(&self, p: Option<prism_core::Edges>) {
        let pad = match p {
            None => (AREA_PAD_X, AREA_PAD_X, AREA_PAD_Y, AREA_PAD_Y),
            Some(p) => {
                let r = |v: f32| v.round() as i32;
                (r(p.left), r(p.right), r(p.top), r(p.bottom))
            }
        };
        if self.pad.replace(pad) == pad {
            return;
        }
        let (l, r, t, b) = pad;
        self.view.set_left_margin(l);
        self.view.set_right_margin(r);
        self.view.set_top_margin(t);
        self.view.set_bottom_margin(b);
        let rtl = self.root.direction() == gtk::TextDirection::Rtl;
        self.placeholder.set_margin_start(if rtl { r } else { l });
        self.placeholder.set_margin_end(if rtl { l } else { r });
        self.placeholder.set_margin_top(t);
    }

    /// Its height at `width`, insets included: at least its minimum lines
    /// (#235; one for a rich input). `attrs`: a rich input's fonts and chips.
    pub fn measure_height(&self, width: f32, attrs: Option<gtk::pango::AttrList>) -> f32 {
        let (l, r, t, b) = self.pad.get();
        let inner = (width - (l + r) as f32).max(1.);
        let layout = self.view.create_pango_layout(Some(&self.text()));
        layout.set_attributes(attrs.as_ref());
        layout.set_width((inner * gtk::pango::SCALE as f32) as i32);
        layout.set_wrap(gtk::pango::WrapMode::WordChar);
        let text_h = layout.pixel_size().1 as f32;
        let line = self.view.create_pango_layout(Some("Ag")).pixel_size().1 as f32;
        text_h.max(line * self.rows.get().max(1) as f32) + (t + b) as f32
    }
}

// --- Spinner ------------------------------------------------------------------

/// GTK has one spinner size (16 px in the default theme); "large" is 32.
pub fn spinner_size(spinner: &gtk::Spinner, large: bool) {
    let s = if large { 32 } else { 16 };
    spinner.set_size_request(s, s);
}

// --- Text input traits ----------------------------------------------------------

/// `InputConfig` as an input purpose (on-screen keyboards) and hints
/// (spell checking, capitalization). GTK has no return-key label.
pub fn input_traits(c: &InputConfig) -> (gtk::InputPurpose, gtk::InputHints) {
    use gtk::InputHints as H;
    let purpose = match (c.secure, c.keyboard) {
        (true, Keyboard::Number) => gtk::InputPurpose::Pin,
        (true, _) => gtk::InputPurpose::Password,
        (_, Keyboard::Default) => gtk::InputPurpose::FreeForm,
        (_, Keyboard::Email) => gtk::InputPurpose::Email,
        (_, Keyboard::Number) => gtk::InputPurpose::Digits,
        (_, Keyboard::Decimal) => gtk::InputPurpose::Number,
        (_, Keyboard::Phone) => gtk::InputPurpose::Phone,
        (_, Keyboard::Url) => gtk::InputPurpose::Url,
    };
    let text = !c.secure && c.keyboard == Keyboard::Default;
    let mut hints = if text && c.autocorrect {
        H::SPELLCHECK | H::WORD_COMPLETION
    } else {
        H::NO_SPELLCHECK
    };
    if c.secure {
        hints |= H::PRIVATE;
    }
    if text {
        hints |= match c.capitalize {
            Capitalize::Sentences => H::UPPERCASE_SENTENCES,
            Capitalize::Words => H::UPPERCASE_WORDS,
            Capitalize::All => H::UPPERCASE_CHARS,
            Capitalize::None => H::NONE,
        };
    }
    (purpose, hints)
}

/// Secure entries hide their characters; a maximum length; suggestions
/// in a `GtkEntryCompletion` popup (prefix matches, as you type).
pub fn configure_entry(entry: &gtk::Entry, config: &str) {
    let c = InputConfig::decode(config);
    let (purpose, hints) = input_traits(&c);
    entry.set_visibility(!c.secure);
    entry.set_input_purpose(purpose);
    entry.set_input_hints(hints);
    entry.set_max_length(c.max_length.map_or(0, |m| m.min(i32::MAX as u32) as i32));
    set_suggestions(entry, &prism_core::controls::input_suggestions(config));
}

/// `GtkEntryCompletion` (deprecated in GTK 4.10 for a future replacement,
/// still the toolkit's completion popup; Prism targets 4.8).
#[allow(deprecated)]
fn set_suggestions(entry: &gtk::Entry, suggestions: &[String]) {
    if suggestions.is_empty() {
        entry.set_completion(None);
        return;
    }
    let store = gtk::ListStore::new(&[glib::Type::STRING]);
    for s in suggestions {
        store.insert_with_values(None, &[(0, s)]);
    }
    let completion = entry.completion().unwrap_or_else(|| {
        let c = gtk::EntryCompletion::new();
        c.set_text_column(0);
        c.set_minimum_key_length(1);
        c.set_popup_completion(true);
        c.set_inline_completion(false);
        entry.set_completion(Some(&c));
        c
    });
    completion.set_model(Some(&store));
}

// --- Checkbox and radio group ---------------------------------------------------

/// A `GtkCheckButton` (its label is the node's text) reporting `Change`.
pub fn checkbox(id: NodeId) -> gtk::CheckButton {
    let b = gtk::CheckButton::new();
    b.set_valign(gtk::Align::Center);
    b.connect_toggled(move |b| dispatch(UiEvent::Change(id, if b.is_active() { 1. } else { 0. })));
    b
}

/// Grouped `GtkCheckButton`s (radio buttons: arrow keys move between them,
/// screen readers get the radio role) in a box with the radio-group role.
pub struct RadioGroup {
    pub root: gtk::Box,
    buttons: RefCell<Vec<gtk::CheckButton>>,
    selected: Cell<i32>,
}

pub fn radio_group(id: NodeId) -> Rc<RadioGroup> {
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .accessible_role(gtk::AccessibleRole::RadioGroup)
        .build();
    root.set_widget_name(&format!("radio{id}"));
    Rc::new(RadioGroup {
        root,
        buttons: RefCell::new(Vec::new()),
        selected: Cell::new(-1),
    })
}

impl RadioGroup {
    /// Rebuilds the buttons for `config` (`RadioConfig`).
    pub fn set_options(self: &Rc<Self>, id: NodeId, config: &str) {
        let c = prism_core::controls::RadioConfig::decode(config);
        let (orientation, spacing) = if c.horizontal {
            (gtk::Orientation::Horizontal, 16)
        } else {
            (gtk::Orientation::Vertical, 6)
        };
        self.root.set_orientation(orientation);
        self.root.set_spacing(spacing);
        for b in self.buttons.borrow_mut().drain(..) {
            self.root.remove(&b);
        }
        let mut buttons = Vec::new();
        for (i, label) in c.options.iter().enumerate() {
            let b = gtk::CheckButton::with_label(label);
            if let Some(first) = buttons.first() {
                b.set_group(Some(first));
            }
            b.connect_toggled(move |b| {
                if b.is_active() {
                    dispatch(UiEvent::Change(id, i as f64));
                }
            });
            self.root.append(&b);
            buttons.push(b);
        }
        *self.buttons.borrow_mut() = buttons;
        self.select(self.selected.get() as f64);
    }

    pub fn select(&self, index: f64) {
        let index = if index >= 0. { index as i32 } else { -1 };
        self.selected.set(index);
        for (i, b) in self.buttons.borrow().iter().enumerate() {
            b.set_active(i as i32 == index);
        }
    }

    pub fn set_sensitive(&self, on: bool) {
        self.root.set_sensitive(on);
    }
}

pub fn configure_text_view(view: &gtk::TextView, config: &str) {
    let (purpose, hints) = input_traits(&InputConfig::decode(config));
    view.set_input_purpose(purpose);
    view.set_input_hints(hints);
}

// --- Direction ----------------------------------------------------------------

/// `w` and everything inside it (popovers included) lay out right to left
/// or left to right. Only form controls follow it: Prism already mirrors
/// frames and text alignment, and GTK would flip labels' alignment again.
pub fn set_direction_deep(w: &gtk::Widget, rtl: bool) {
    w.set_direction(if rtl {
        gtk::TextDirection::Rtl
    } else {
        gtk::TextDirection::Ltr
    });
    let mut child = w.first_child();
    while let Some(c) = child {
        set_direction_deep(&c, rtl);
        child = c.next_sibling();
    }
}

// --- Announcements --------------------------------------------------------------

unsafe extern "C" {
    /// dlfcn.h (glibc: `RTLD_DEFAULT` is the null handle).
    fn dlsym(
        handle: *mut std::ffi::c_void,
        symbol: *const std::ffi::c_char,
    ) -> *mut std::ffi::c_void;
}

/// `gtk_accessible_announce (GtkAccessible*, const char*, GtkAccessibleAnnouncementPriority)`,
/// GTK 4.14+.
type Announce =
    unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char, std::ffi::c_int);

/// `GTK_ACCESSIBLE_ANNOUNCEMENT_PRIORITY_MEDIUM` (polite).
const PRIORITY_MEDIUM: std::ffi::c_int = 1;

/// Asks screen readers to read `message` out. GTK 4.14+ has
/// `gtk_accessible_announce`, looked up when the app runs (Prism builds
/// against older GTKs too); before 4.14 the status region's description
/// changes, which assistive technologies see as an update.
pub fn announce(w: &gtk::Widget, message: &str) {
    static FN: std::sync::OnceLock<Option<Announce>> = std::sync::OnceLock::new();
    let f = FN.get_or_init(|| {
        // SAFETY: looks a symbol up in the loaded libraries.
        let p = unsafe { dlsym(std::ptr::null_mut(), c"gtk_accessible_announce".as_ptr()) };
        // SAFETY: when present, it has the signature documented above.
        (!p.is_null()).then(|| unsafe { std::mem::transmute::<*mut std::ffi::c_void, Announce>(p) })
    });
    match (f, std::ffi::CString::new(message)) {
        (Some(f), Ok(text)) => {
            use gtk::glib::translate::ToGlibPtr;
            let widget: *mut gtk::ffi::GtkWidget = w.to_glib_none().0;
            // SAFETY: every GtkWidget is a GtkAccessible; the string outlives the call.
            unsafe { f(widget.cast(), text.as_ptr(), PRIORITY_MEDIUM) };
        }
        _ => crate::imp::set_accessible_string(w, crate::imp::a11y_property::DESCRIPTION, message),
    }
}
