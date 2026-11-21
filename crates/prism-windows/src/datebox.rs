//! A date picker node (#89): a `CalendarDatePicker` for the day, a
//! `TimePicker` for the time, or both side by side (`date_time()`), then,
//! while an optional picker has a value, a clear button (Fluent's dismiss
//! glyph, named and tipped "Clear" in the app's language, like the macOS
//! backend's) that empties it. Picking a day keeps the time and the other
//! way round; the first pick in an empty picker starts from now
//! (`dates.rs`).

use std::cell::RefCell;

use prism_core::NodeId;
use prism_core::controls::{DateConfig, DateMode};
use windows::Foundation::{DateTime, IReference, PropertyValue, TimeSpan, TypedEventHandler};
use windows_core::{Interface, Ref, Result};

use crate::dates;
use crate::imp::{changed, with_state};
use crate::scene::Node;
use crate::text::{ICON_FONT, font};
use crate::winui::xaml::Automation::AutomationProperties;
use crate::winui::xaml::Controls::{
    Button, CalendarDatePicker, CalendarDatePickerDateChangedEventArgs, ColumnDefinition, Control,
    FontIcon, Grid, TimePicker, TimePickerSelectedValueChangedEventArgs, ToolTipService,
};
use crate::winui::xaml::{
    FocusState, GridLength, GridUnitType, HorizontalAlignment, RoutedEventHandler, UIElement,
    VerticalAlignment, Visibility,
};
use crate::winui::{boxed, h, transparent, uniform};

/// The clear button's width; the gap between the parts.
pub const CLEAR_WIDTH: f32 = 32.;
pub const GAP: f32 = 4.;

thread_local! {
    /// The clear button's name, in the app's language
    /// (`RenderEnvironment::clear`).
    static CLEAR: RefCell<String> = RefCell::new("Clear".into());
}

/// The clear buttons' name from now on (`Widgets::set_environment`).
pub fn set_clear_label(text: &str) {
    let text = if text.is_empty() { "Clear" } else { text };
    CLEAR.with(|c| *c.borrow_mut() = text.to_string());
}

pub struct DateBox {
    pub root: Grid,
    pub mode: DateMode,
    date: Option<CalendarDatePicker>,
    time: Option<TimePicker>,
    clear: Button,
}

/// The value after a pick, from the node's value and config (`f`).
fn pick(id: NodeId, f: impl FnOnce(&DateConfig, f64, f64, f64) -> f64) {
    let Some((config, value)) = with_state(|s| {
        let n = s.renderer.widgets.scene.get(id)?;
        Some((DateConfig::decode(&n.config), n.value))
    })
    .flatten() else {
        return;
    };
    let now = crate::imp::now();
    changed(id, f(&config, value, now, crate::imp::local_offset()));
}

fn column(width: GridLength) -> Result<ColumnDefinition> {
    let c = ColumnDefinition::new()?;
    c.SetWidth(width)?;
    Ok(c)
}

fn star() -> GridLength {
    GridLength {
        Value: 1.,
        GridUnitType: GridUnitType::Star,
    }
}

fn auto() -> GridLength {
    GridLength {
        Value: 1.,
        GridUnitType: GridUnitType::Auto,
    }
}

impl DateBox {
    pub fn new(id: NodeId, mode: DateMode) -> Result<DateBox> {
        let root = Grid::new()?;
        root.SetColumnSpacing(GAP as f64)?;
        let (day, clock) = dates::parts(mode);
        let columns = root.ColumnDefinitions()?;
        let kids = root.Children()?;
        let mut col = 0;
        let date = if day {
            let d = CalendarDatePicker::new()?;
            d.SetMinWidth(0.)?;
            d.SetIsTextScaleFactorEnabled(false)?;
            d.SetHorizontalAlignment(HorizontalAlignment::Stretch)?;
            d.SetVerticalAlignment(VerticalAlignment::Stretch)?;
            d.DateChanged(&TypedEventHandler::<
                CalendarDatePicker,
                CalendarDatePickerDateChangedEventArgs,
            >::new(
                move |_, a: Ref<CalendarDatePickerDateChangedEventArgs>| {
                    if let Some(a) = a.as_ref()
                        && let Ok(v) = a.NewDate()
                        && let Ok(dt) = v.Value()
                    {
                        let day = crate::controls::unix_seconds(dt);
                        pick(id, |c, value, now, offset| {
                            dates::pick_day(c, value, day, now, offset)
                        });
                    }
                    Ok(())
                },
            ))?;
            columns.Append(&column(star())?)?;
            Grid::SetColumn(&d, col)?;
            kids.Append(&d)?;
            col += 1;
            Some(d)
        } else {
            None
        };
        let time = if clock {
            let t = TimePicker::new()?;
            t.SetMinWidth(0.)?;
            t.SetIsTextScaleFactorEnabled(false)?;
            t.SetHorizontalAlignment(HorizontalAlignment::Stretch)?;
            t.SetVerticalAlignment(VerticalAlignment::Stretch)?;
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
                        pick(id, |c, value, now, offset| {
                            dates::pick_time(c, value, secs, now, offset)
                        });
                    }
                    Ok(())
                },
            ))?;
            columns.Append(&column(if day { auto() } else { star() })?)?;
            Grid::SetColumn(&t, col)?;
            kids.Append(&t)?;
            col += 1;
            Some(t)
        } else {
            None
        };
        let clear = Button::new()?;
        clear.SetBackground(&transparent()?)?;
        clear.SetBorderThickness(uniform(0.))?;
        clear.SetPadding(uniform(0.))?;
        clear.SetMinWidth(0.)?;
        clear.SetWidth(CLEAR_WIDTH as f64)?;
        clear.SetVerticalAlignment(VerticalAlignment::Stretch)?;
        clear.SetVisibility(Visibility::Collapsed)?;
        let icon = FontIcon::new()?;
        icon.SetFontFamily(&font(ICON_FONT)?)?;
        // Cancel.
        icon.SetGlyph(&h("\u{E711}"))?;
        icon.SetFontSize(12.)?;
        clear.SetContent(&icon)?;
        columns.Append(&column(auto())?)?;
        Grid::SetColumn(&clear, col)?;
        kids.Append(&clear)?;
        let b = DateBox {
            root,
            mode,
            date,
            time,
            clear,
        };
        b.label_clear(&CLEAR.with(|c| c.borrow().clone()))?;
        // Back to empty; the keyboard focus goes back to the picker, the
        // button leaving.
        let focus = b.control();
        b.clear.Click(&RoutedEventHandler::new(move |_, _| {
            changed(id, f64::NAN);
            if let Some(c) = &focus {
                let _ = c.Focus(FocusState::Programmatic);
            }
            Ok(())
        }))?;
        Ok(b)
    }

    /// The element placed in the node's box.
    pub fn element(&self) -> Result<UIElement> {
        self.root.cast()
    }

    /// The control that takes the keyboard focus and the node's
    /// accessibility (name, help, tooltip): the day's, else the time's.
    pub fn control(&self) -> Option<Control> {
        match (&self.date, &self.time) {
            (Some(d), _) => d.cast().ok(),
            (None, Some(t)) => t.cast().ok(),
            _ => None,
        }
    }

    /// The clear button's name and tooltip.
    pub fn label_clear(&self, text: &str) -> Result<()> {
        AutomationProperties::SetName(&self.clear, &h(text))?;
        ToolTipService::SetToolTip(&self.clear, &boxed(text)?)
    }

    /// The time picker next to a day's takes the node's name and help too:
    /// a screen reader reads "Due, date picker", then "Due, time picker".
    pub fn name_time(&self, name: &str, help: &str) -> Result<()> {
        if let (Some(_), Some(t)) = (&self.date, &self.time) {
            AutomationProperties::SetName(t, &h(name))?;
            AutomationProperties::SetHelpText(t, &h(help))?;
        }
        Ok(())
    }

    pub fn set_enabled(&self, on: bool) -> Result<()> {
        if let Some(d) = &self.date {
            d.SetIsEnabled(on)?;
        }
        if let Some(t) = &self.time {
            t.SetIsEnabled(on)?;
        }
        self.clear.SetIsEnabled(on)
    }

    /// The bounds (the calendar's; a time keeps any value, clamped when
    /// picked).
    pub fn configure(&self, c: &DateConfig) -> Result<()> {
        if let Some(d) = &self.date {
            if let Some(min) = c.min {
                d.SetMinDate(crate::controls::date_time(min))?;
            }
            if let Some(max) = c.max {
                d.SetMaxDate(crate::controls::date_time(max))?;
            }
        }
        Ok(())
    }

    /// Shows `value` (`NaN`: empty, the placeholder showing).
    pub fn set_value(&self, n: &Node, value: f64) -> Result<()> {
        let c = DateConfig::decode(&n.config);
        let empty = !value.is_finite();
        if let Some(d) = &self.date {
            if empty {
                d.SetDate(None::<&IReference<DateTime>>)?;
            } else {
                let dt = crate::controls::date_time(value);
                let shown = d.Date().ok().and_then(|v| v.Value().ok());
                if shown.map(|s| s.UniversalTime / 10_000_000)
                    != Some(dt.UniversalTime / 10_000_000)
                {
                    d.SetDate(&PropertyValue::CreateDateTime(dt)?.cast::<IReference<DateTime>>()?)?;
                }
            }
            d.SetPlaceholderText(&h(&n.placeholder))?;
        }
        if let Some(t) = &self.time {
            if empty {
                t.SetSelectedTime(None::<&IReference<TimeSpan>>)?;
            } else {
                let secs = dates::shown_time(c.mode, value, crate::imp::local_offset());
                let span = TimeSpan {
                    Duration: (secs * 1e7) as i64,
                };
                t.SetSelectedTime(
                    &PropertyValue::CreateTimeSpan(span)?.cast::<IReference<TimeSpan>>()?,
                )?;
            }
        }
        self.clear.SetVisibility(if c.optional && !empty {
            Visibility::Visible
        } else {
            Visibility::Collapsed
        })
    }
}
