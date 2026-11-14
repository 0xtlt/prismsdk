//! Shared by the iOS and macOS backends: Foundation-based formatting and the
//! system environment (locale, time zone).

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{
    NSDate, NSDateFormatter, NSDateFormatterStyle, NSLocale, NSNumber, NSNumberFormatter,
    NSNumberFormatterStyle, NSString, NSTimeZone,
};
use prism_core::{DateStyle, Formatter, Locale};

/// Numbers and dates formatted by Foundation (calendar, digits and time
/// zone of the user).
#[derive(Default)]
pub struct FoundationFormatter;

fn ns_locale(locale: &Locale) -> Retained<NSLocale> {
    NSLocale::initWithLocaleIdentifier(NSLocale::alloc(), &NSString::from_str(locale.tag()))
}

impl Formatter for FoundationFormatter {
    fn number(&self, locale: &Locale, value: f64, decimals: usize) -> String {
        let f = NSNumberFormatter::new();
        f.setLocale(Some(&ns_locale(locale)));
        f.setNumberStyle(NSNumberFormatterStyle::DecimalStyle);
        f.setMinimumFractionDigits(decimals);
        f.setMaximumFractionDigits(decimals);
        f.stringFromNumber(&NSNumber::new_f64(value))
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{value:.decimals$}"))
    }

    fn date(&self, locale: &Locale, unix: f64, style: DateStyle) -> String {
        let f = NSDateFormatter::new();
        let style = match style {
            DateStyle::Short => NSDateFormatterStyle::ShortStyle,
            DateStyle::Medium => NSDateFormatterStyle::MediumStyle,
            DateStyle::Long => NSDateFormatterStyle::LongStyle,
        };
        f.setLocale(Some(&ns_locale(locale)));
        f.setDateStyle(style);
        f.setTimeStyle(NSDateFormatterStyle::NoStyle);
        f.stringFromDate(&NSDate::dateWithTimeIntervalSince1970(unix))
            .to_string()
    }

    fn time(&self, locale: &Locale, unix: f64) -> String {
        let f = NSDateFormatter::new();
        f.setLocale(Some(&ns_locale(locale)));
        f.setDateStyle(NSDateFormatterStyle::NoStyle);
        f.setTimeStyle(NSDateFormatterStyle::ShortStyle);
        f.stringFromDate(&NSDate::dateWithTimeIntervalSince1970(unix))
            .to_string()
    }
}

/// The user's preferred language (`PRISM_LOCALE` overrides it).
pub fn system_locale() -> Locale {
    if let Ok(tag) = std::env::var("PRISM_LOCALE") {
        return Locale::new(&tag);
    }
    NSLocale::preferredLanguages()
        .firstObject()
        .map(|s| Locale::new(&s.to_string()))
        .unwrap_or_default()
}

/// Seconds east of UTC.
pub fn utc_offset() -> i32 {
    NSTimeZone::localTimeZone().secondsFromGMT() as i32
}
