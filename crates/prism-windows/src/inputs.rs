//! A text input's options as Windows sees them (#18): the input scope
//! (the touch keyboard's layout and its keys), spell checking and text
//! prediction; and which suggestions show for what is typed. No Win32:
//! tested on every host.
//!
//! The touch keyboard's automatic capitalization comes with its scope:
//! the default scope capitalizes sentences while spell checking is on,
//! `NameOrPhoneNumber` the first letter of each word. No scope
//! capitalizes every letter.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::controls::{Capitalize, InputConfig, Keyboard, ReturnKey};

/// The input scopes Prism uses (`InputScopeNameValue`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Default,
    /// The Search key in place of Enter.
    Search,
    /// Capitalizes each word.
    NameOrPhoneNumber,
    EmailSmtpAddress,
    /// Digits only.
    Digits,
    /// Digits and a decimal separator.
    Number,
    TelephoneNumber,
    Url,
    /// A password box's own two scopes.
    Password,
    NumericPin,
}

/// How a field takes text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Traits {
    pub scope: Scope,
    /// Spelling marked and corrected (with the default scope, the touch
    /// keyboard's capitalization too).
    pub spell_check: bool,
    /// The touch keyboard's word suggestions.
    pub prediction: bool,
}

/// `InputConfig` as Windows' traits, like GTK's purposes and hints.
pub fn traits(c: &InputConfig) -> Traits {
    let scope = match (c.secure, c.keyboard) {
        (true, Keyboard::Number) => Scope::NumericPin,
        (true, _) => Scope::Password,
        (_, Keyboard::Email) => Scope::EmailSmtpAddress,
        (_, Keyboard::Number) => Scope::Digits,
        (_, Keyboard::Decimal) => Scope::Number,
        (_, Keyboard::Phone) => Scope::TelephoneNumber,
        (_, Keyboard::Url) => Scope::Url,
        (_, Keyboard::Default) if c.return_key == ReturnKey::Search => Scope::Search,
        (_, Keyboard::Default) if c.capitalize == Capitalize::Words => Scope::NameOrPhoneNumber,
        (_, Keyboard::Default) => Scope::Default,
    };
    // Corrections are for prose: off for addresses, numbers and secrets,
    // and with the default scope, off when nothing is to be capitalized.
    let text = !c.secure && c.keyboard == Keyboard::Default;
    let on =
        text && c.autocorrect && !(scope == Scope::Default && c.capitalize == Capitalize::None);
    Traits {
        scope,
        spell_check: on,
        prediction: text && c.autocorrect,
    }
}

/// The most suggestions shown at once.
pub const SHOWN: usize = 8;

/// The suggestions (indexes) offered for `typed`: those that start with
/// it, ignoring case, but not the one it already is; none before the
/// first character. Like GTK's completion popup.
pub fn matches(suggestions: &[String], typed: &str) -> Vec<usize> {
    if typed.is_empty() {
        return Vec::new();
    }
    let key = typed.to_lowercase();
    suggestions
        .iter()
        .enumerate()
        .filter(|(_, s)| s.as_str() != typed && s.to_lowercase().starts_with(&key))
        .map(|(i, _)| i)
        .take(SHOWN)
        .collect()
}

/// The highlighted suggestion after an arrow key (`down`), wrapping, of
/// `count` shown.
pub fn step(current: Option<usize>, count: usize, down: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match (current, down) {
        (None, true) => 0,
        (None, false) => count - 1,
        (Some(i), true) => (i + 1) % count,
        (Some(i), false) => (i + count - 1) % count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(f: impl FnOnce(&mut InputConfig)) -> InputConfig {
        let mut c = InputConfig::default();
        f(&mut c);
        c
    }

    #[test]
    fn keyboards_pick_their_scope() {
        let t = traits(&InputConfig::default());
        assert_eq!(t.scope, Scope::Default);
        assert!(t.spell_check && t.prediction);
        let t = traits(&config(|c| c.keyboard = Keyboard::Email));
        assert_eq!(t.scope, Scope::EmailSmtpAddress);
        assert!(!t.spell_check && !t.prediction);
        assert_eq!(
            traits(&config(|c| c.keyboard = Keyboard::Number)).scope,
            Scope::Digits
        );
        assert_eq!(
            traits(&config(|c| c.keyboard = Keyboard::Decimal)).scope,
            Scope::Number
        );
        assert_eq!(
            traits(&config(|c| c.keyboard = Keyboard::Phone)).scope,
            Scope::TelephoneNumber
        );
        assert_eq!(
            traits(&config(|c| c.keyboard = Keyboard::Url)).scope,
            Scope::Url
        );
        assert_eq!(
            traits(&config(|c| c.return_key = ReturnKey::Search)).scope,
            Scope::Search
        );
    }

    #[test]
    fn secrets_take_the_password_scopes() {
        let t = traits(&config(|c| c.secure = true));
        assert_eq!(t.scope, Scope::Password);
        assert!(!t.spell_check && !t.prediction);
        let t = traits(&config(|c| {
            c.secure = true;
            c.keyboard = Keyboard::Number;
        }));
        assert_eq!(t.scope, Scope::NumericPin);
    }

    #[test]
    fn capitalization_and_corrections() {
        let t = traits(&config(|c| c.capitalize = Capitalize::Words));
        assert_eq!(t.scope, Scope::NameOrPhoneNumber);
        assert!(t.spell_check);
        // The default scope capitalizes with spell checking on.
        let t = traits(&config(|c| c.capitalize = Capitalize::None));
        assert_eq!(t.scope, Scope::Default);
        assert!(!t.spell_check && t.prediction);
        let t = traits(&config(|c| c.autocorrect = false));
        assert!(!t.spell_check && !t.prediction);
    }

    #[test]
    fn suggestions_match_the_start_ignoring_case() {
        let list: Vec<String> = ["Paris", "Parma", "Lyon", "paris"]
            .map(String::from)
            .to_vec();
        assert_eq!(matches(&list, ""), Vec::<usize>::new());
        assert_eq!(matches(&list, "pa"), vec![0, 1, 3]);
        assert_eq!(matches(&list, "Paris"), vec![3]);
        assert_eq!(matches(&list, "x"), Vec::<usize>::new());
        let many: Vec<String> = (0..20).map(|i| format!("a{i}")).collect();
        assert_eq!(matches(&many, "a").len(), SHOWN);
    }

    #[test]
    fn arrows_wrap() {
        assert_eq!(step(None, 3, true), Some(0));
        assert_eq!(step(None, 3, false), Some(2));
        assert_eq!(step(Some(2), 3, true), Some(0));
        assert_eq!(step(Some(0), 3, false), Some(2));
        assert_eq!(step(Some(0), 0, true), None);
    }
}
