//! Configuration of form controls (sliders, pickers, date pickers, text
//! inputs), sent to backends as one short string per node
//! ([`Patch::SetConfig`](crate::Patch::SetConfig)). Each backend decodes it
//! with the matching `decode` and maps it to its native control.
//!
//! | Kind | Config | Value |
//! |---|---|---|
//! | `Slider` | [`SliderConfig`]: `min max step` | the current value |
//! | `Picker` | [`PickerConfig`]: options, one per line | the selected index (-1: none) |
//! | `DatePicker` | [`DateConfig`]: `mode min max` | Unix seconds (UTC instant) |
//! | `TextInput`, `TextArea` | [`InputConfig`]: `secure keyboard=email return=done capitalize=none autocorrect=off max=40`, then one suggestion per line ([`input_suggestions`]) | – |
//! | `Checkbox` | – | 0 or 1 |
//! | `RadioGroup` | [`RadioConfig`]: `row` or `column`, then options one per line | the selected index (-1: none) |
//! | `Overlay` | [`OverlayConfig`]: `sheet`/`popover`/`dialog`, `fixed`, `medium` | 1 open, 0 closed |

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

/// A date picker's mode and bounds (Unix seconds).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct DateConfig {
    /// What the picker edits.
    pub mode: DateMode,
    /// Earliest selectable moment (Unix seconds), if bounded.
    pub min: Option<f64>,
    /// Latest selectable moment (Unix seconds), if bounded.
    pub max: Option<f64>,
}

impl DateConfig {
    /// `mode min max`, `-` for no bound.
    pub fn encode(&self) -> String {
        let b = |v: Option<f64>| v.map_or("-".to_string(), |v| v.to_string());
        format!("{} {} {}", self.mode.name(), b(self.min), b(self.max))
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
        DateConfig { mode, min, max }
    }

    /// `value` within the bounds.
    pub fn clamp(&self, value: f64) -> f64 {
        let v = self.min.map_or(value, |m| value.max(m));
        self.max.map_or(v, |m| v.min(m))
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
        }
    }
}

impl InputConfig {
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
}

impl OverlayConfig {
    /// `sheet` / `popover` / `dialog`, then `fixed`, `medium`.
    pub fn encode(&self) -> String {
        let mut out = String::from(match self.kind {
            OverlayKind::Sheet => "sheet",
            OverlayKind::Popover => "popover",
            OverlayKind::Dialog => "dialog",
        });
        if self.fixed {
            out.push_str(" fixed");
        }
        if self.medium {
            out.push_str(" medium");
        }
        out
    }

    /// Parses [`encode`](Self::encode)'s output.
    pub fn decode(s: &str) -> Self {
        let mut c = OverlayConfig::default();
        for t in s.split_whitespace() {
            match t {
                "popover" => c.kind = OverlayKind::Popover,
                "dialog" => c.kind = OverlayKind::Dialog,
                "fixed" => c.fixed = true,
                "medium" => c.medium = true,
                _ => {}
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
            max: None,
        };
        assert_eq!(DateConfig::decode(&d.encode()), d);
        assert_eq!(d.encode(), "datetime 0 -");
        let i = InputConfig {
            secure: true,
            keyboard: Keyboard::Email,
            return_key: ReturnKey::Go,
            capitalize: Capitalize::None,
            autocorrect: false,
            max_length: Some(12),
        };
        assert_eq!(InputConfig::decode(&i.encode()), i);
        assert_eq!(InputConfig::default().encode(), "");
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
            medium: false,
        };
        assert_eq!(o.encode(), "popover fixed");
        assert_eq!(OverlayConfig::decode(&o.encode()), o);
        assert_eq!(OverlayConfig::decode(""), OverlayConfig::default());
        let d = DateConfig {
            mode: DateMode::TimeOfDay,
            min: None,
            max: None,
        };
        assert_eq!(DateConfig::decode(&d.encode()), d);
    }
}
