//! Configuration of form controls (sliders, pickers, date pickers, text
//! inputs), sent to backends as one short string per node
//! ([`Patch::SetConfig`](crate::Patch::SetConfig)). Each backend decodes it
//! with the matching `decode` and maps it to its native control.
//!
//! | Kind | Config | Value |
//! |---|---|---|
//! | `Slider` | [`SliderConfig`]: `min max step` | the current value |
//! | `Picker` | [`PickerConfig`]: options, one per line | the selected index (-1: none) |
//! | `DatePicker` | [`DateConfig`]: `mode min max`, then `optional` | Unix seconds (UTC instant); NaN: empty (optional pickers) |
//! | `TextInput`, `TextArea` | [`InputConfig`]: `secure keyboard=email return=done capitalize=none autocorrect=off max=40`, then one suggestion per line ([`input_suggestions`]) | – |
//! | `Checkbox` | – | 0 or 1 |
//! | `RadioGroup` | [`RadioConfig`]: `row` or `column`, then options one per line | the selected index (-1: none) |
//! | `Overlay` | [`OverlayConfig`]: `sheet`/`popover`/`dialog`, `fixed`, `medium`, `manual`, `keepfocus`, `side=top`/`trailing`/`leading`, `align=center`/`end`, `placed`, `backdrop=clear`, `size=320x400` | 1 open, 0 closed |

/// A slider's range. `step == 0` is continuous.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderConfig {
    /// Lowest value.
    pub min: f64,
    /// Highest value.
    pub max: f64,
    /// Increment values snap to; `0` is continuous.
    pub step: f64,
}

impl Default for SliderConfig {
    fn default() -> Self {
        SliderConfig {
            min: 0.,
            max: 1.,
            step: 0.,
        }
    }
}

impl SliderConfig {
    /// `min max step`, space-separated.
    pub fn encode(&self) -> String {
        format!("{} {} {}", self.min, self.max, self.step)
    }

    /// Parses [`encode`](Self::encode)'s output; missing or invalid parts fall back to the defaults.
    pub fn decode(s: &str) -> Self {
        let mut it = s.split_whitespace().map(|v| v.parse::<f64>().ok());
        let d = SliderConfig::default();
        let min = it.next().flatten().unwrap_or(d.min);
        let max = it.next().flatten().unwrap_or(d.max).max(min);
        let step = it.next().flatten().unwrap_or(0.).max(0.);
        SliderConfig { min, max, step }
    }

    /// `value` clamped to the range and rounded to the nearest step.
    pub fn snap(&self, value: f64) -> f64 {
        let v = value.clamp(self.min, self.max);
        if self.step > 0. {
            let n = ((v - self.min) / self.step).round();
            (self.min + n * self.step).clamp(self.min, self.max)
        } else {
            v
        }
    }

    /// Where `value` sits in the range, `0.0..=1.0`.
    pub fn fraction(&self, value: f64) -> f64 {
        if self.max > self.min {
            ((value - self.min) / (self.max - self.min)).clamp(0., 1.)
        } else {
            0.
        }
    }

    /// The step assistive technologies use (increment / decrement): the step,
    /// or a tenth of the range for continuous sliders.
    pub fn a11y_step(&self) -> f64 {
        if self.step > 0. {
            self.step
        } else {
            (self.max - self.min) / 10.
        }
    }
}

/// A picker's options, in order.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PickerConfig {
    /// The option labels, in display order.
    pub options: Vec<String>,
}

impl PickerConfig {
    /// One option per line (line breaks inside options become spaces).
    pub fn encode(&self) -> String {
        self.options
            .iter()
            .map(|o| o.replace(['\n', '\r'], " "))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Parses [`encode`](Self::encode)'s output; the empty string is no options.
    pub fn decode(s: &str) -> Self {
        PickerConfig {
            options: if s.is_empty() {
                Vec::new()
            } else {
                s.split('\n').map(str::to_string).collect()
            },
        }
    }
}

/// What a date picker edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DateMode {
    /// A day (the time of the value is kept).
    #[default]
    Date,
    /// A time of day (the day of the value is kept).
    Time,
    /// Both.
    DateTime,
    /// A time of day on its own, the same in every time zone: the value is
    /// seconds since midnight (`07:30` is `27000`). Backends show it in UTC.
    TimeOfDay,
}

impl DateMode {
    /// The name used in the encoded config.
    pub fn name(self) -> &'static str {
        match self {
            DateMode::Date => "date",
            DateMode::Time => "time",
            DateMode::DateTime => "datetime",
            DateMode::TimeOfDay => "timeofday",
        }
    }
}

/// A date picker's mode and bounds (Unix seconds), and whether it may be
/// empty.
///
/// An optional picker's value is `NaN` while it is empty (#89): backends
/// show its placeholder, and offer a way back to empty once it has a value
/// (the browser's own clear, a clear button, a "Clear" button in the
/// dialog). Starting to edit an empty picker fills it with
/// [`start`](Self::start).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct DateConfig {
    /// What the picker edits.
    pub mode: DateMode,
    /// Earliest selectable moment (Unix seconds), if bounded.
    pub min: Option<f64>,
    /// Latest selectable moment (Unix seconds), if bounded.
    pub max: Option<f64>,
    /// The picker may be empty (its value `NaN`) and offers to clear it.
    pub optional: bool,
}

impl DateConfig {
    /// `mode min max`, `-` for no bound, then `optional` if it may be empty.
    pub fn encode(&self) -> String {
        let b = |v: Option<f64>| v.map_or("-".to_string(), |v| v.to_string());
        let optional = if self.optional { " optional" } else { "" };
        format!(
            "{} {} {}{optional}",
            self.mode.name(),
            b(self.min),
            b(self.max)
        )
    }

    /// Parses [`encode`](Self::encode)'s output; unknown modes become `Date`, missing or `-` bounds are unbounded.
    pub fn decode(s: &str) -> Self {
        let mut it = s.split_whitespace();
        let mode = match it.next() {
            Some("time") => DateMode::Time,
            Some("datetime") => DateMode::DateTime,
            Some("timeofday") => DateMode::TimeOfDay,
            _ => DateMode::Date,
        };
        let mut bound = || it.next().and_then(|v| v.parse::<f64>().ok());
        let min = bound();
        let max = bound();
        let optional = it.any(|t| t == "optional");
        DateConfig {
            mode,
            min,
            max,
            optional,
        }
    }

    /// `value` within the bounds (an empty value, `NaN`, stays empty).
    pub fn clamp(&self, value: f64) -> f64 {
        if value.is_nan() {
            return value;
        }
        let v = self.min.map_or(value, |m| value.max(m));
        self.max.map_or(v, |m| v.min(m))
    }

    /// The value an empty picker takes when the user starts to fill it:
    /// `now` (Unix seconds) to the minute, within the bounds. A time of day
    /// is the local time, `now` shifted by `utc_offset` seconds.
    pub fn start(&self, now: f64, utc_offset: f64) -> f64 {
        let minute = |t: f64| (t / 60.).floor() * 60.;
        if self.mode == DateMode::TimeOfDay {
            return minute(now + utc_offset).rem_euclid(86_400.);
        }
        self.clamp(minute(now))
    }
}

/// The on-screen keyboard a text field asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Keyboard {
    /// The platform's standard keyboard.
    #[default]
    Default,
    /// Email address layout.
    Email,
    /// Digits only.
    Number,
    /// Digits and a decimal separator.
    Decimal,
    /// Phone number pad.
    Phone,
    /// URL layout.
    Url,
}

/// The label of the keyboard's return key (and what it means).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReturnKey {
    /// The platform's standard return key.
    #[default]
    Default,
    /// "Done": ends editing.
    Done,
    /// "Go": submits, e.g. to open an address.
    Go,
    /// "Next": moves on to the next field.
    Next,
    /// "Search": submits a search.
    Search,
    /// "Send": submits a message.
    Send,
}

/// Automatic capitalization while typing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Capitalize {
    /// The platform default (sentences on phones).
    #[default]
    Sentences,
    /// Never capitalize automatically.
    None,
    /// Capitalize the first letter of every word.
    Words,
    /// Capitalize every letter.
    All,
}

/// Text input behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputConfig {
    /// Hidden characters (passwords). Single-line inputs only.
    pub secure: bool,
    /// The on-screen keyboard layout.
    pub keyboard: Keyboard,
    /// The label and action of the return key.
    pub return_key: ReturnKey,
    /// Automatic capitalization.
    pub capitalize: Capitalize,
    /// Whether the platform corrects spelling as the user types.
    pub autocorrect: bool,
    /// The most characters the field accepts (`None`: no limit).
    pub max_length: Option<u32>,
    /// A text area's minimum number of lines (#235); `None`: three. A
    /// rich input's is one.
    pub rows: Option<u32>,
}

impl Default for InputConfig {
    fn default() -> Self {
        InputConfig {
            secure: false,
            keyboard: Keyboard::Default,
            return_key: ReturnKey::Default,
            capitalize: Capitalize::Sentences,
            autocorrect: true,
            max_length: None,
            rows: None,
        }
    }
}

impl InputConfig {
    /// A text area's minimum number of lines (#235): three unless set.
    pub fn min_rows(&self) -> u32 {
        self.rows.unwrap_or(3)
    }

    /// Space-separated tokens; the default config is the empty string.
    pub fn encode(&self) -> String {
        let mut t: Vec<&str> = Vec::new();
        if self.secure {
            t.push("secure");
        }
        t.push(match self.keyboard {
            Keyboard::Default => "",
            Keyboard::Email => "keyboard=email",
            Keyboard::Number => "keyboard=number",
            Keyboard::Decimal => "keyboard=decimal",
            Keyboard::Phone => "keyboard=phone",
            Keyboard::Url => "keyboard=url",
        });
        t.push(match self.return_key {
            ReturnKey::Default => "",
            ReturnKey::Done => "return=done",
            ReturnKey::Go => "return=go",
            ReturnKey::Next => "return=next",
            ReturnKey::Search => "return=search",
            ReturnKey::Send => "return=send",
        });
        t.push(match self.capitalize {
            Capitalize::Sentences => "",
            Capitalize::None => "capitalize=none",
            Capitalize::Words => "capitalize=words",
            Capitalize::All => "capitalize=all",
        });
        if !self.autocorrect {
            t.push("autocorrect=off");
        }
        let max = self.max_length.map(|m| format!("max={m}"));
        if let Some(m) = &max {
            t.push(m);
        }
        let rows = self.rows.map(|r| format!("rows={r}"));
        if let Some(r) = &rows {
            t.push(r);
        }
        t.retain(|s| !s.is_empty());
        t.join(" ")
    }

    /// Parses [`encode`](Self::encode)'s output (its first line: the next
    /// ones are [`input_suggestions`]); unknown tokens are ignored.
    pub fn decode(s: &str) -> Self {
        let mut c = InputConfig::default();
        for token in s.lines().next().unwrap_or("").split_whitespace() {
            if let Some(m) = token.strip_prefix("max=") {
                c.max_length = m.parse().ok();
                continue;
            }
            if let Some(r) = token.strip_prefix("rows=") {
                c.rows = r.parse().ok().filter(|r| *r > 0);
                continue;
            }
            match token {
                "secure" => c.secure = true,
                "keyboard=email" => c.keyboard = Keyboard::Email,
                "keyboard=number" => c.keyboard = Keyboard::Number,
                "keyboard=decimal" => c.keyboard = Keyboard::Decimal,
                "keyboard=phone" => c.keyboard = Keyboard::Phone,
                "keyboard=url" => c.keyboard = Keyboard::Url,
                "return=done" => c.return_key = ReturnKey::Done,
                "return=go" => c.return_key = ReturnKey::Go,
                "return=next" => c.return_key = ReturnKey::Next,
                "return=search" => c.return_key = ReturnKey::Search,
                "return=send" => c.return_key = ReturnKey::Send,
                "capitalize=none" => c.capitalize = Capitalize::None,
                "capitalize=words" => c.capitalize = Capitalize::Words,
                "capitalize=all" => c.capitalize = Capitalize::All,
                "autocorrect=off" => c.autocorrect = false,
                _ => {}
            }
        }
        c
    }
}

/// The suggestions of a text input's config: every line after the first
/// (web `<datalist>`, native completion lists).
pub fn input_suggestions(config: &str) -> Vec<String> {
    config.lines().skip(1).map(str::to_string).collect()
}

/// `config` (an [`InputConfig::encode`] line) followed by `suggestions`, one
/// per line (line breaks inside them become spaces).
pub fn with_suggestions(config: &str, suggestions: &[String]) -> String {
    let mut out = config.lines().next().unwrap_or("").to_string();
    for s in suggestions {
        out.push('\n');
        out.push_str(&s.replace(['\n', '\r'], " "));
    }
    out
}

/// A radio group's options and orientation.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RadioConfig {
    /// The option labels, in display order.
    pub options: Vec<String>,
    /// Options side by side (`row`) instead of stacked (`column`).
    pub horizontal: bool,
}

impl RadioConfig {
    /// `row` or `column`, then one option per line.
    pub fn encode(&self) -> String {
        let mut out = String::from(if self.horizontal { "row" } else { "column" });
        for o in &self.options {
            out.push('\n');
            out.push_str(&o.replace(['\n', '\r'], " "));
        }
        out
    }

    /// Parses [`encode`](Self::encode)'s output.
    pub fn decode(s: &str) -> Self {
        let mut lines = s.split('\n');
        let horizontal = lines.next() == Some("row");
        RadioConfig {
            options: lines.map(str::to_string).collect(),
            horizontal,
        }
    }
}

/// How an [`Overlay`](crate::Overlay) is presented.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OverlayKind {
    /// A sheet: from the bottom edge on phones, a page sheet / window sheet
    /// on larger screens (`UISheetPresentationController`, `NSWindow`
    /// sheet, a modal `<dialog>`, a GTK modal window, a bottom sheet).
    #[default]
    Sheet,
    /// A bubble anchored to the overlay's parent element (`NSPopover`,
    /// `UIPopoverPresentationController`, the web popover API,
    /// `GtkPopover`, `PopupWindow`). Phones may show it as a sheet.
    Popover,
    /// A centered modal dialog (`<dialog>`, a centered window or panel, a
    /// form sheet, `Dialog`).
    Dialog,
    /// A bar above the on-screen keyboard while a text field it serves has
    /// the focus (#242): see [`keyboard_toolbar`](crate::keyboard_toolbar).
    /// The field's `inputAccessoryView` on iOS and iPadOS, a bar above the
    /// IME on Android, a bar on the visual viewport's bottom edge in mobile
    /// browsers. Other platforms leave it out of their tree.
    Keyboard,
}

/// The side of its anchor a popover opens on (#113). It flips to the
/// opposite side when the window has no room there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PopoverSide {
    /// Below the anchor (the default).
    #[default]
    Bottom,
    /// Above the anchor.
    Top,
    /// After the anchor: to its right in left-to-right layouts, to its left
    /// in right-to-left ones. Submenus open there.
    Trailing,
    /// Before the anchor: to its left in left-to-right layouts.
    Leading,
}

impl PopoverSide {
    /// Beside the anchor (lined up along its height) rather than below or
    /// above it.
    pub fn beside(self) -> bool {
        matches!(self, PopoverSide::Trailing | PopoverSide::Leading)
    }

    /// The opposite side, where a popover goes when there is no room.
    pub fn opposite(self) -> Self {
        match self {
            PopoverSide::Bottom => PopoverSide::Top,
            PopoverSide::Top => PopoverSide::Bottom,
            PopoverSide::Trailing => PopoverSide::Leading,
            PopoverSide::Leading => PopoverSide::Trailing,
        }
    }

    /// Whether it puts the overlay to the right of its anchor, given the
    /// layout direction (`None` below and above).
    pub fn is_right(self, rtl: bool) -> Option<bool> {
        match self {
            PopoverSide::Leading => Some(rtl),
            PopoverSide::Trailing => Some(!rtl),
            _ => None,
        }
    }
}

/// How a popover lines up with its anchor along the side it opens on
/// (#113).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PopoverAlign {
    /// Leading edges lined up for a popover below or above its anchor, top
    /// edges for one beside it (the default).
    #[default]
    Start,
    /// Centered on the anchor.
    Center,
    /// Trailing edges lined up below or above, bottom edges beside.
    End,
}

impl PopoverAlign {
    /// For popovers the platform centers on a rectangle it points its arrow
    /// at (`NSPopover`, `UIPopoverPresentationController`, `GtkPopover`):
    /// the span `(offset, length)` of an anchor `anchor` long to point at,
    /// so that a popover `popover` long lines up with the anchor's start,
    /// center or end. `start_at_min`: the start is the anchor's low
    /// coordinate (its left in left-to-right layouts, its top in y-down
    /// coordinates). A popover longer than the anchor is centered on it,
    /// its arrow still on the anchor; one not measured yet (`0`) points at
    /// the whole anchor.
    pub fn arrow_span(self, anchor: f64, popover: f64, start_at_min: bool) -> (f64, f64) {
        if popover <= 0. {
            return (0., anchor);
        }
        let part = popover.clamp(0., anchor.max(0.));
        let offset = match self {
            PopoverAlign::Center => (anchor - part) / 2.,
            PopoverAlign::Start if start_at_min => 0.,
            PopoverAlign::End if !start_at_min => 0.,
            PopoverAlign::Start | PopoverAlign::End => anchor - part,
        };
        (offset, part)
    }
}

/// What a sheet or dialog shows over the page behind it (#79). See
/// [`Overlay::backdrop`](crate::Overlay::backdrop).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backdrop {
    /// Nothing: the page stays as it is (still out of reach while the
    /// overlay is open, and a click on it still closes the overlay).
    Clear,
    /// A plain dim.
    Dim,
    /// A dim with the page blurred under it (where the platform can).
    Blur,
}

impl Backdrop {
    fn token(self) -> &'static str {
        match self {
            Backdrop::Clear => "clear",
            Backdrop::Dim => "dim",
            Backdrop::Blur => "blur",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "clear" => Backdrop::Clear,
            "dim" => Backdrop::Dim,
            "blur" => Backdrop::Blur,
            _ => return None,
        })
    }
}

/// What a popover or a placed dialog is anchored to (#328). Placement
/// follows the usual rules against it: its [`side`](crate::Overlay::side)
/// (flipped without room), its [`align`](crate::Overlay::align), kept
/// inside the window. See [`Overlay::anchor`](crate::Overlay::anchor).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OverlayAnchor {
    /// The element that contains the overlay (the default).
    #[default]
    Parent,
    /// The caret of a text field (`text_input`, `text_area`, `rich_input`)
    /// inside the overlay's parent: the one with the keyboard focus, else
    /// the first one in tree order. A field without the focus has no
    /// caret: the overlay goes against the whole field; with no field
    /// there, against the parent. It follows the caret as the user types,
    /// moves it, scrolls the field or the page, and as the field moves.
    Caret,
}

impl OverlayAnchor {
    /// Its `anchor=` token (`None` for the default).
    fn token(self) -> Option<&'static str> {
        match self {
            OverlayAnchor::Parent => None,
            OverlayAnchor::Caret => Some("caret"),
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "parent" => OverlayAnchor::Parent,
            "caret" => OverlayAnchor::Caret,
            _ => return None,
        })
    }
}

/// The caret rectangle `caret` kept inside the visible part of its field
/// `field` (rectangles are `(x, y, width, height)`, in one coordinate
/// space): a caret scrolled out of a field's view is pinned to the field's
/// nearest edge, so an overlay anchored at it (#328) stays beside the
/// field. At least one point wide.
pub fn caret_in_field(
    caret: (f32, f32, f32, f32),
    field: (f32, f32, f32, f32),
) -> (f32, f32, f32, f32) {
    let (fx, fy, fw, fh) = field;
    let (cx, cy, cw, ch) = caret;
    let w = cw.max(1.).min(fw.max(1.));
    let h = ch.max(0.).min(fh.max(0.));
    let x = cx.clamp(fx, (fx + fw - w).max(fx));
    let y = cy.clamp(fy, (fy + fh - h).max(fy));
    (x, y, w, h)
}

/// Where an overlay of `size` goes on `side` of `anchor`, inside `area`
/// (rectangles are `(x, y, width, height)`, y down), `gap` away: on its
/// side, or on the opposite one when its side has no room, lined up with
/// the anchor as `align` says and kept inside `area`. `None` when neither
/// side has room: a placed dialog is then centered (#79).
pub fn place_beside(
    anchor: (f32, f32, f32, f32),
    size: (f32, f32),
    area: (f32, f32, f32, f32),
    side: PopoverSide,
    align: PopoverAlign,
    rtl: bool,
    gap: f32,
) -> Option<(f32, f32)> {
    let (ax, ay, aw, ah) = anchor;
    let (w, h) = size;
    let (bx, by, bw, bh) = area;
    let clamp = |v: f32, lo: f32, len: f32, max: f32| v.min(lo + max - len).max(lo);
    let fits = |s: PopoverSide| match s.is_right(rtl) {
        Some(true) => ax + aw + gap + w <= bx + bw,
        Some(false) => ax - gap - w >= bx,
        None if s == PopoverSide::Bottom => ay + ah + gap + h <= by + bh,
        None => ay - gap - h >= by,
    };
    let side = [side, side.opposite()].into_iter().find(|&s| fits(s))?;
    Some(match side.is_right(rtl) {
        Some(right) => {
            let y = match align {
                PopoverAlign::Start => ay,
                PopoverAlign::Center => ay + (ah - h) / 2.,
                PopoverAlign::End => ay + ah - h,
            };
            (
                if right { ax + aw + gap } else { ax - gap - w },
                clamp(y, by, h, bh),
            )
        }
        None => {
            let (start, end) = if rtl {
                (ax + aw - w, ax)
            } else {
                (ax, ax + aw - w)
            };
            let x = match align {
                PopoverAlign::Start => start,
                PopoverAlign::Center => ax + (aw - w) / 2.,
                PopoverAlign::End => end,
            };
            (
                clamp(x, bx, w, bw),
                if side == PopoverSide::Bottom {
                    ay + ah + gap
                } else {
                    ay - gap - h
                },
            )
        }
    })
}

impl OverlayConfig {
    /// A popover that leaves the focus to its anchor and closes on a press
    /// outside it, which still reaches what it landed on (#182): backends
    /// watch presses around it instead of the platform's light dismiss
    /// (which would take the focus or swallow the press).
    pub fn light_dismiss_beside_focus(&self) -> bool {
        self.kind == OverlayKind::Popover && self.keep_focus && !self.manual && !self.fixed
    }
}

/// An overlay's presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct OverlayConfig {
    /// Sheet, popover or dialog.
    pub kind: OverlayKind,
    /// Escape, Back, a tap outside or a swipe down do not close it.
    pub fixed: bool,
    /// A sheet starts at half height (it can be pulled up where the
    /// platform allows).
    pub medium: bool,
    /// A popover that a click or tap outside does not close
    /// (`popover="manual"` on the web): the app closes it, or Escape while
    /// the focus is inside it. See [`Overlay::light_dismiss`](crate::Overlay::light_dismiss).
    pub manual: bool,
    /// A popover that leaves the keyboard focus where it is (the anchor's
    /// text field keeps receiving keys) and does not take it on a click
    /// inside: inline suggestion menus. A press outside still closes it
    /// unless `manual` (#182); the press then reaches what it landed on.
    /// See [`Overlay::take_focus`](crate::Overlay::take_focus).
    pub keep_focus: bool,
    /// The side of its anchor a popover opens on. See
    /// [`Overlay::side`](crate::Overlay::side).
    pub side: PopoverSide,
    /// How a popover lines up with its anchor. See
    /// [`Overlay::align`](crate::Overlay::align).
    pub align: PopoverAlign,
    /// A dialog placed on [`side`](Self::side) of its anchor instead of
    /// centered (#79). Set by [`Overlay::side`](crate::Overlay::side) on a
    /// dialog; popovers are always placed.
    pub placed: bool,
    /// What a sheet or dialog shows over the page (`None`: the platform's
    /// default). See [`Overlay::backdrop`](crate::Overlay::backdrop).
    pub backdrop: Option<Backdrop>,
    /// The content area's width in points (`0`: the platform's default).
    /// See [`Overlay::size`](crate::Overlay::size).
    pub width: u32,
    /// The content area's height in points (`0`: fit the content).
    pub height: u32,
    /// A dialog or popover that draws no box of its own: no background,
    /// border, corners, arrow, shadow or entry animation; its content is
    /// the panel. See
    /// [`Overlay::bare`](crate::Overlay::bare).
    pub bare: bool,
    /// A keyboard bar that serves only the text field just before it (a
    /// field's own [`keyboard_toolbar`](crate::TextInput::keyboard_toolbar)),
    /// not every field in its parent (#242).
    pub field_only: bool,
    /// What a popover or a placed dialog is anchored to (#328). See
    /// [`Overlay::anchor`](crate::Overlay::anchor).
    pub anchor: OverlayAnchor,
}

impl OverlayConfig {
    /// `sheet` / `popover` / `dialog`, then `fixed`, `medium`, `manual`,
    /// `keepfocus`, `side=…` and `align=…` (left out when they are the
    /// defaults), `placed`, `backdrop=<clear|dim|blur>`,
    /// `size=<width>x<height>`, `bare`, `field`, `anchor=caret`. A keyboard bar is
    /// `keyboard`, with `field` when it serves only the field before it.
    pub fn encode(&self) -> String {
        let mut out = String::from(match self.kind {
            OverlayKind::Sheet => "sheet",
            OverlayKind::Popover => "popover",
            OverlayKind::Dialog => "dialog",
            OverlayKind::Keyboard => "keyboard",
        });
        if self.fixed {
            out.push_str(" fixed");
        }
        if self.medium {
            out.push_str(" medium");
        }
        if self.manual {
            out.push_str(" manual");
        }
        if self.keep_focus {
            out.push_str(" keepfocus");
        }
        match self.side {
            PopoverSide::Bottom => {}
            PopoverSide::Top => out.push_str(" side=top"),
            PopoverSide::Trailing => out.push_str(" side=trailing"),
            PopoverSide::Leading => out.push_str(" side=leading"),
        }
        match self.align {
            PopoverAlign::Start => {}
            PopoverAlign::Center => out.push_str(" align=center"),
            PopoverAlign::End => out.push_str(" align=end"),
        }
        if self.placed {
            out.push_str(" placed");
        }
        if let Some(b) = self.backdrop {
            out.push_str(" backdrop=");
            out.push_str(b.token());
        }
        if self.width > 0 || self.height > 0 {
            out.push_str(&format!(" size={}x{}", self.width, self.height));
        }
        if self.bare {
            out.push_str(" bare");
        }
        if self.field_only {
            out.push_str(" field");
        }
        if let Some(a) = self.anchor.token() {
            out.push_str(" anchor=");
            out.push_str(a);
        }
        out
    }

    /// The side of its anchor the overlay opens on: a popover's
    /// [`side`](Self::side), a placed dialog's, `None` for a centered
    /// dialog and for sheets (#79).
    pub fn anchored_side(&self) -> Option<PopoverSide> {
        match self.kind {
            OverlayKind::Popover => Some(self.side),
            OverlayKind::Dialog if self.placed => Some(self.side),
            OverlayKind::Dialog | OverlayKind::Sheet | OverlayKind::Keyboard => None,
        }
    }

    /// Parses [`encode`](Self::encode)'s output.
    pub fn decode(s: &str) -> Self {
        let mut c = OverlayConfig::default();
        for t in s.split_whitespace() {
            match t {
                "popover" => c.kind = OverlayKind::Popover,
                "dialog" => c.kind = OverlayKind::Dialog,
                "keyboard" => c.kind = OverlayKind::Keyboard,
                "field" => c.field_only = true,
                "fixed" => c.fixed = true,
                "medium" => c.medium = true,
                "manual" => c.manual = true,
                "keepfocus" => c.keep_focus = true,
                "side=top" => c.side = PopoverSide::Top,
                "side=trailing" => c.side = PopoverSide::Trailing,
                "side=leading" => c.side = PopoverSide::Leading,
                "align=center" => c.align = PopoverAlign::Center,
                "align=end" => c.align = PopoverAlign::End,
                "placed" => c.placed = true,
                "bare" => c.bare = true,
                _ => {
                    if let Some(b) = t.strip_prefix("backdrop=") {
                        c.backdrop = Backdrop::parse(b);
                    } else if let Some(a) = t.strip_prefix("anchor=") {
                        c.anchor = OverlayAnchor::parse(a).unwrap_or_default();
                    } else if let Some((w, h)) = t
                        .strip_prefix("size=")
                        .and_then(|s| s.split_once('x'))
                        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    {
                        (c.width, c.height) = (w, h);
                    }
                }
            }
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configs_round_trip() {
        let s = SliderConfig {
            min: -5.,
            max: 10.5,
            step: 0.5,
        };
        assert_eq!(SliderConfig::decode(&s.encode()), s);
        assert_eq!(s.snap(3.3), 3.5);
        assert_eq!(s.snap(99.), 10.5);
        let p = PickerConfig {
            options: vec!["Paris".into(), "New\nYork".into(), String::new()],
        };
        assert_eq!(
            PickerConfig::decode(&p.encode()).options,
            ["Paris", "New York", ""]
        );
        assert!(PickerConfig::decode("").options.is_empty());
        let d = DateConfig {
            mode: DateMode::DateTime,
            min: Some(0.),
            ..Default::default()
        };
        assert_eq!(DateConfig::decode(&d.encode()), d);
        assert_eq!(d.encode(), "datetime 0 -");
        assert!(!d.optional);
        let i = InputConfig {
            secure: true,
            keyboard: Keyboard::Email,
            return_key: ReturnKey::Go,
            capitalize: Capitalize::None,
            autocorrect: false,
            max_length: Some(12),
            rows: Some(1),
        };
        assert_eq!(InputConfig::decode(&i.encode()), i);
        assert_eq!(InputConfig::default().encode(), "");
        // A text area's minimum lines (#235): three unless set.
        assert_eq!(InputConfig::default().min_rows(), 3);
        assert_eq!(InputConfig::decode("rows=1").min_rows(), 1);
        assert_eq!(InputConfig::decode("rows=0").min_rows(), 3);
    }

    #[test]
    fn new_configs_round_trip() {
        let i = InputConfig {
            max_length: Some(5),
            ..Default::default()
        };
        let full = with_suggestions(&i.encode(), &["Paris".into(), "New\nYork".into()]);
        assert_eq!(full, "max=5\nParis\nNew York");
        assert_eq!(InputConfig::decode(&full), i);
        assert_eq!(input_suggestions(&full), ["Paris", "New York"]);
        assert!(input_suggestions("max=5").is_empty());
        // Suggestions are not read as tokens.
        let tricky = with_suggestions("", &["secure".into()]);
        assert!(!InputConfig::decode(&tricky).secure);
        let r = RadioConfig {
            options: vec!["Daily".into(), "Weekly".into()],
            horizontal: true,
        };
        assert_eq!(r.encode(), "row\nDaily\nWeekly");
        assert_eq!(RadioConfig::decode(&r.encode()), r);
        assert!(RadioConfig::decode("column").options.is_empty());
        let o = OverlayConfig {
            kind: OverlayKind::Popover,
            fixed: true,
            ..Default::default()
        };
        assert_eq!(o.encode(), "popover fixed");
        assert_eq!(OverlayConfig::decode(&o.encode()), o);
        assert_eq!(OverlayConfig::decode(""), OverlayConfig::default());
        // An inline suggestion menu (#70).
        let inline = OverlayConfig {
            kind: OverlayKind::Popover,
            manual: true,
            keep_focus: true,
            ..Default::default()
        };
        assert_eq!(inline.encode(), "popover manual keepfocus");
        // #182: a popover beside the focus closes on presses around it
        // unless manual or fixed; other overlays never watch them.
        assert!(!inline.light_dismiss_beside_focus());
        let light = OverlayConfig {
            manual: false,
            ..inline
        };
        assert!(light.light_dismiss_beside_focus());
        assert!(
            !OverlayConfig {
                fixed: true,
                ..light
            }
            .light_dismiss_beside_focus()
        );
        assert!(
            !OverlayConfig {
                keep_focus: false,
                ..light
            }
            .light_dismiss_beside_focus()
        );
        assert!(
            !OverlayConfig {
                kind: OverlayKind::Dialog,
                ..light
            }
            .light_dismiss_beside_focus()
        );
        assert_eq!(OverlayConfig::decode(&inline.encode()), inline);
        let all = OverlayConfig {
            kind: OverlayKind::Sheet,
            fixed: true,
            medium: true,
            manual: true,
            keep_focus: true,
            ..Default::default()
        };
        assert_eq!(all.encode(), "sheet fixed medium manual keepfocus");
        assert_eq!(OverlayConfig::decode(&all.encode()), all);
        // Placed popovers (#113): the defaults are left out.
        for side in [
            PopoverSide::Bottom,
            PopoverSide::Top,
            PopoverSide::Trailing,
            PopoverSide::Leading,
        ] {
            for align in [PopoverAlign::Start, PopoverAlign::Center, PopoverAlign::End] {
                let placed = OverlayConfig {
                    kind: OverlayKind::Popover,
                    side,
                    align,
                    ..Default::default()
                };
                assert_eq!(OverlayConfig::decode(&placed.encode()), placed);
            }
        }
        let submenu = OverlayConfig {
            kind: OverlayKind::Popover,
            manual: true,
            keep_focus: true,
            side: PopoverSide::Trailing,
            align: PopoverAlign::End,
            ..Default::default()
        };
        assert_eq!(
            submenu.encode(),
            "popover manual keepfocus side=trailing align=end"
        );
        assert_eq!(PopoverSide::Leading.opposite(), PopoverSide::Trailing);
        // Arrowed popovers point at the part of the anchor they line up with.
        assert_eq!(PopoverAlign::Start.arrow_span(300., 200., true), (0., 200.));
        assert_eq!(
            PopoverAlign::Start.arrow_span(300., 200., false),
            (100., 200.)
        );
        assert_eq!(PopoverAlign::End.arrow_span(300., 200., true), (100., 200.));
        assert_eq!(
            PopoverAlign::Center.arrow_span(300., 200., true),
            (50., 200.)
        );
        assert_eq!(PopoverAlign::End.arrow_span(24., 100., true), (0., 24.));
        assert_eq!(PopoverAlign::End.arrow_span(24., 0., true), (0., 24.));
        assert!(PopoverSide::Trailing.beside() && !PopoverSide::Top.beside());
        // A choice panel beside its column, the page left undimmed (#79).
        let beside = OverlayConfig {
            kind: OverlayKind::Dialog,
            side: PopoverSide::Trailing,
            placed: true,
            backdrop: Some(Backdrop::Clear),
            width: 320,
            height: 0,
            ..Default::default()
        };
        assert_eq!(
            beside.encode(),
            "dialog side=trailing placed backdrop=clear size=320x0"
        );
        assert_eq!(OverlayConfig::decode(&beside.encode()), beside);
        assert_eq!(beside.anchored_side(), Some(PopoverSide::Trailing));
        // The app draws the panel itself.
        let bare = OverlayConfig {
            bare: true,
            ..beside
        };
        assert_eq!(
            bare.encode(),
            "dialog side=trailing placed backdrop=clear size=320x0 bare"
        );
        assert_eq!(OverlayConfig::decode(&bare.encode()), bare);
        for b in [Backdrop::Clear, Backdrop::Dim, Backdrop::Blur] {
            let c = OverlayConfig {
                backdrop: Some(b),
                height: 200,
                ..Default::default()
            };
            assert_eq!(OverlayConfig::decode(&c.encode()), c);
        }
        // A dialog without a side stays centered; a popover is always placed.
        let centered = OverlayConfig::decode("dialog side=trailing");
        assert_eq!(centered.anchored_side(), None);
        assert_eq!(
            OverlayConfig::decode("popover").anchored_side(),
            Some(PopoverSide::Bottom)
        );
        assert_eq!(OverlayConfig::decode("sheet placed").anchored_side(), None);
        // At a text field's caret (#328): popovers and placed dialogs.
        let at_caret = OverlayConfig {
            kind: OverlayKind::Popover,
            keep_focus: true,
            anchor: OverlayAnchor::Caret,
            ..Default::default()
        };
        assert_eq!(at_caret.encode(), "popover keepfocus anchor=caret");
        assert_eq!(OverlayConfig::decode(&at_caret.encode()), at_caret);
        assert_eq!(
            OverlayConfig::decode("dialog placed anchor=caret").anchor,
            OverlayAnchor::Caret
        );
        assert_eq!(
            OverlayConfig::decode("popover anchor=parent").encode(),
            "popover"
        );
        assert_eq!(
            OverlayConfig::decode("popover anchor=elsewhere").anchor,
            OverlayAnchor::Parent
        );
        // Unknown values are ignored.
        assert_eq!(
            OverlayConfig::decode("dialog backdrop=fog size=ax2"),
            OverlayConfig {
                kind: OverlayKind::Dialog,
                ..Default::default()
            }
        );
        let d = DateConfig {
            mode: DateMode::TimeOfDay,
            ..Default::default()
        };
        assert_eq!(DateConfig::decode(&d.encode()), d);
    }

    #[test]
    fn optional_dates() {
        let d = DateConfig {
            mode: DateMode::Date,
            min: Some(100.),
            max: None,
            optional: true,
        };
        assert_eq!(d.encode(), "date 100 - optional");
        assert_eq!(DateConfig::decode(&d.encode()), d);
        assert!(!DateConfig::decode("time - -").optional);
        // An empty value stays empty.
        assert!(d.clamp(f64::NAN).is_nan());
        assert_eq!(d.clamp(5.), 100.);
        // Filling an empty picker: now, to the minute, within the bounds.
        assert_eq!(d.start(1_000_059., 3600.), 1_000_020.);
        assert_eq!(d.start(50., 0.), 100.);
        let of_day = DateConfig {
            mode: DateMode::TimeOfDay,
            optional: true,
            ..Default::default()
        };
        // 23:30 UTC in UTC+2 is 01:30.
        assert_eq!(
            of_day.start(86_400. * 3. + 23.5 * 3600. + 12., 7200.),
            5400.
        );
    }

    #[test]
    fn carets_stay_inside_their_field() {
        let field = (100., 50., 200., 30.);
        // Inside: kept, one point wide at least.
        assert_eq!(
            caret_in_field((150., 55., 0., 20.), field),
            (150., 55., 1., 20.)
        );
        // Scrolled out past the end, or before the start: on the edge.
        assert_eq!(
            caret_in_field((420., 55., 2., 20.), field),
            (298., 55., 2., 20.)
        );
        assert_eq!(
            caret_in_field((20., 10., 1., 20.), field),
            (100., 50., 1., 20.)
        );
        // Taller than the field (a scrolled text area): no taller than it.
        assert_eq!(
            caret_in_field((150., 40., 1., 60.), field),
            (150., 50., 1., 30.)
        );
    }

    #[test]
    fn overlays_are_placed_beside_their_anchor() {
        let area = (0., 0., 1000., 800.);
        // A column from x 0 to 300, a row at y 100.
        let column = (0., 100., 300., 40.);
        let size = (320., 400.);
        // Trailing: right of it, aligned with its top.
        assert_eq!(
            place_beside(
                column,
                size,
                area,
                PopoverSide::Trailing,
                PopoverAlign::Start,
                false,
                8.
            ),
            Some((308., 100.))
        );
        // A column on the right: no room after it, so before it, in both
        // directions.
        let right_column = (700., 100., 300., 40.);
        assert_eq!(
            place_beside(
                right_column,
                size,
                area,
                PopoverSide::Trailing,
                PopoverAlign::Start,
                true,
                8.
            ),
            Some((372., 100.))
        );
        assert_eq!(
            place_beside(
                right_column,
                size,
                area,
                PopoverSide::Trailing,
                PopoverAlign::Start,
                false,
                8.
            ),
            Some((372., 100.))
        );
        // Kept inside vertically.
        let low = (0., 700., 300., 40.);
        assert_eq!(
            place_beside(
                low,
                size,
                area,
                PopoverSide::Trailing,
                PopoverAlign::Start,
                false,
                8.
            ),
            Some((308., 400.))
        );
        // Below, flipped above without room below.
        let field = (100., 600., 200., 30.);
        assert_eq!(
            place_beside(
                field,
                (200., 180.),
                area,
                PopoverSide::Bottom,
                PopoverAlign::Start,
                false,
                6.
            ),
            Some((100., 414.))
        );
        assert_eq!(
            place_beside(
                field,
                (200., 100.),
                area,
                PopoverSide::Bottom,
                PopoverAlign::Start,
                false,
                6.
            ),
            Some((100., 636.))
        );
        // Lined up with the anchor's end or center (#113's alignment).
        assert_eq!(
            place_beside(
                column,
                (320., 100.),
                area,
                PopoverSide::Trailing,
                PopoverAlign::End,
                false,
                8.
            ),
            Some((308., 40.))
        );
        assert_eq!(
            place_beside(
                field,
                (100., 50.),
                area,
                PopoverSide::Top,
                PopoverAlign::Center,
                false,
                6.
            ),
            Some((150., 544.))
        );
        assert_eq!(
            place_beside(
                field,
                (100., 50.),
                area,
                PopoverSide::Top,
                PopoverAlign::End,
                true,
                6.
            ),
            Some((100., 544.))
        );
        // A phone: room on neither side, the caller centers it.
        let phone = (0., 0., 390., 800.);
        assert_eq!(
            place_beside(
                (0., 100., 390., 40.),
                size,
                phone,
                PopoverSide::Trailing,
                PopoverAlign::Start,
                false,
                8.
            ),
            None
        );
    }
}
