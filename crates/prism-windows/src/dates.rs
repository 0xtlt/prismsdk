//! A date picker's arithmetic (#89): what picking a day or a time makes of
//! its value, in local time. The picker shows a `CalendarDatePicker`, a
//! `TimePicker`, or both (`controls.rs`); each edits its own part and keeps
//! the other. An empty optional picker starts from
//! [`DateConfig::start`]. No Win32: tested on every host.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::controls::{DateConfig, DateMode};

const DAY: f64 = 86_400.;

/// Local midnight of the day of `unix` (`offset`: the local time zone's,
/// seconds east of UTC).
pub fn midnight(unix: f64, offset: f64) -> f64 {
    ((unix + offset) / DAY).floor() * DAY - offset
}

/// Seconds past local midnight of `unix`.
pub fn seconds_of_day(unix: f64, offset: f64) -> f64 {
    (unix + offset).rem_euclid(DAY)
}

/// The value, or where an empty picker starts.
fn base(c: &DateConfig, value: f64, now: f64, offset: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        c.start(now, offset)
    }
}

/// The value once the user picked the day of `day` (any moment of it,
/// local): the value's time of day on that day, within the bounds.
pub fn pick_day(c: &DateConfig, value: f64, day: f64, now: f64, offset: f64) -> f64 {
    let time = seconds_of_day(base(c, value, now, offset), offset);
    c.clamp(midnight(day, offset) + time)
}

/// The value once the user picked `secs` past midnight: a time of day on
/// its own is that; otherwise that time on the value's day, within the
/// bounds.
pub fn pick_time(c: &DateConfig, value: f64, secs: f64, now: f64, offset: f64) -> f64 {
    if c.mode == DateMode::TimeOfDay {
        return secs.rem_euclid(DAY);
    }
    c.clamp(midnight(base(c, value, now, offset), offset) + secs)
}

/// What the picker shows: (a day, a time).
pub fn parts(mode: DateMode) -> (bool, bool) {
    match mode {
        DateMode::Date => (true, false),
        DateMode::Time | DateMode::TimeOfDay => (false, true),
        DateMode::DateTime => (true, true),
    }
}

/// The seconds past midnight a time picker shows for `value`: a time of
/// day on its own in UTC, any other in local time.
pub fn shown_time(mode: DateMode, value: f64, offset: f64) -> f64 {
    if mode == DateMode::TimeOfDay {
        value.rem_euclid(DAY)
    } else {
        seconds_of_day(value, offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2024-03-10 15:45:30 UTC.
    const T: f64 = 1_710_085_530.;
    const PARIS: f64 = 3_600.;

    fn config(mode: DateMode) -> DateConfig {
        DateConfig {
            mode,
            ..Default::default()
        }
    }

    #[test]
    fn days_and_times_are_local() {
        // 16:45:30 in Paris.
        assert_eq!(seconds_of_day(T, PARIS), 16. * 3600. + 45. * 60. + 30.);
        assert_eq!(midnight(T, PARIS), T - seconds_of_day(T, PARIS));
        // 23:30 UTC on the 10th is the 11th in Paris.
        let late = midnight(T, 0.) + 23.5 * 3600.;
        assert_eq!(midnight(late, PARIS), midnight(T, PARIS) + DAY);
    }

    #[test]
    fn picking_a_day_keeps_the_time() {
        let c = config(DateMode::DateTime);
        // Any moment of the 12th.
        let day = T + 2. * DAY - 5. * 3600.;
        assert_eq!(pick_day(&c, T, day, 0., PARIS), T + 2. * DAY);
        // An empty picker takes the start's time (now, to the minute).
        let now = T + 7. * DAY;
        let v = pick_day(&c, f64::NAN, day, now, PARIS);
        assert_eq!(v, midnight(day, PARIS) + seconds_of_day(T - 30., PARIS));
    }

    #[test]
    fn picking_a_time_keeps_the_day() {
        let c = config(DateMode::Time);
        let v = pick_time(&c, T, 9. * 3600., 0., PARIS);
        assert_eq!(v, midnight(T, PARIS) + 9. * 3600.);
        // A time of day on its own is the seconds since midnight.
        let c = config(DateMode::TimeOfDay);
        assert_eq!(pick_time(&c, f64::NAN, 27_000., T, PARIS), 27_000.);
        assert_eq!(shown_time(DateMode::TimeOfDay, 27_000., PARIS), 27_000.);
        assert_eq!(
            shown_time(DateMode::DateTime, T, PARIS),
            seconds_of_day(T, PARIS)
        );
    }

    #[test]
    fn picks_stay_within_the_bounds() {
        let c = DateConfig {
            mode: DateMode::Date,
            min: Some(T),
            max: Some(T + DAY),
            optional: true,
        };
        assert_eq!(pick_day(&c, T, T - 3. * DAY, 0., PARIS), T);
        assert_eq!(pick_day(&c, T, T + 9. * DAY, 0., PARIS), T + DAY);
    }

    #[test]
    fn modes_show_their_parts() {
        assert_eq!(parts(DateMode::Date), (true, false));
        assert_eq!(parts(DateMode::Time), (false, true));
        assert_eq!(parts(DateMode::TimeOfDay), (false, true));
        assert_eq!(parts(DateMode::DateTime), (true, true));
    }
}
