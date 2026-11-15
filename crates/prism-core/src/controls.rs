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
//! | `TextInput`, `TextArea` | [`InputConfig`]: `secure keyboard=email return=done capitalize=none autocorrect=off` | – |

/// A slider's range. `step == 0` is continuous.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderConfig {
    pub min: f64,
    pub max: f64,
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
    pub fn encode(&self) -> String {
        format!("{} {} {}", self.min, self.max, self.step)
    }

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
}

impl DateMode {
    pub fn name(self) -> &'static str {
        match self {
            DateMode::Date => "date",
            DateMode::Time => "time",
            DateMode::DateTime => "datetime",
        }
    }
}

/// A date picker's mode and bounds (Unix seconds).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct DateConfig {
    pub mode: DateMode,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl DateConfig {
    /// `mode min max`, `-` for no bound.
    pub fn encode(&self) -> String {
        let b = |v: Option<f64>| v.map_or("-".to_string(), |v| v.to_string());
        format!("{} {} {}", self.mode.name(), b(self.min), b(self.max))
    }

    pub fn decode(s: &str) -> Self {
        let mut it = s.split_whitespace();
        let mode = match it.next() {
            Some("time") => DateMode::Time,
            Some("datetime") => DateMode::DateTime,
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
    #[default]
    Default,
    Email,
    /// Digits only.
    Number,
    /// Digits and a decimal separator.
    Decimal,
    Phone,
    Url,
}

/// The label of the keyboard's return key (and what it means).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReturnKey {
    #[default]
    Default,
    Done,
    Go,
    Next,
    Search,
    Send,
}

/// Automatic capitalization while typing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Capitalize {
    /// The platform default (sentences on phones).
    #[default]
    Sentences,
    None,
    Words,
    All,
}

/// Text input behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputConfig {
    /// Hidden characters (passwords). Single-line inputs only.
    pub secure: bool,
    pub keyboard: Keyboard,
    pub return_key: ReturnKey,
    pub capitalize: Capitalize,
    pub autocorrect: bool,
}

impl Default for InputConfig {
    fn default() -> Self {
        InputConfig {
            secure: false,
            keyboard: Keyboard::Default,
            return_key: ReturnKey::Default,
            capitalize: Capitalize::Sentences,
            autocorrect: true,
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
        t.retain(|s| !s.is_empty());
        t.join(" ")
    }

    pub fn decode(s: &str) -> Self {
        let mut c = InputConfig::default();
        for token in s.split_whitespace() {
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
        };
        assert_eq!(InputConfig::decode(&i.encode()), i);
        assert_eq!(InputConfig::default().encode(), "");
    }
}
