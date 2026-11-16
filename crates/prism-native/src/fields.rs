//! Text fields: telling the echo of what the user typed from a value the
//! app set (#73), and converting selections between the platforms' UTF-16
//! offsets and Prism's characters (#86).
//!
//! A text input or area sends each value the user types; the app stores
//! it (or changes it: a Markdown shortcut strips "# ", a command clears the
//! field, input is normalised) and renders it back. An echo must leave the
//! field alone (caret, selection, IME composition); any other value is the
//! app's and must reach the field, focused or not, the caret keeping its
//! distance from the end. Backends whose events reach the app synchronously
//! (AppKit, UIKit, Android) only compare with the field's text; those that
//! queue them (GTK) can be answered after the user typed further and
//! remember what they sent with [`Typed`]. The web client does the same in
//! `client.js`.

use std::collections::VecDeque;

/// The values typed in one field and sent, not yet rendered back.
#[derive(Clone, Debug, Default)]
pub struct Typed {
    sent: VecDeque<String>,
}

/// At most this many values wait for their echo (older ones are dropped).
const MAX_PENDING: usize = 64;

impl Typed {
    /// The user typed `text`, about to be sent to the app.
    pub fn sent(&mut self, text: &str) {
        if self.sent.len() == MAX_PENDING {
            self.sent.pop_front();
        }
        self.sent.push_back(text.to_string());
    }

    /// The app renders `text` for the field showing `current`: whether to
    /// write it into the field. An echo of a value typed (the field shows it,
    /// or has moved on since) is forgotten with what was typed before it and
    /// not written; any other value is the app's, written unless the field
    /// already shows it.
    pub fn should_apply(&mut self, text: &str, current: &str) -> bool {
        if let Some(i) = self.sent.iter().position(|s| s == text) {
            self.sent.drain(..=i);
            return false;
        }
        // The app answered the typing in flight with its own value.
        self.sent.clear();
        text != current
    }
}

/// Where the caret goes when a field's text of `old_len` units, caret at
/// `caret`, becomes `new_len` units long: the same distance from the end
/// (at the end if it was there, at the start if the text got shorter than
/// that distance). Units are the platform's (UTF-16 or characters).
pub fn caret_from_end(old_len: usize, caret: usize, new_len: usize) -> usize {
    new_len.saturating_sub(old_len.saturating_sub(caret))
}

/// The character offset of UTF-16 offset `utf16` in `text` (clamped; a
/// unit inside a surrogate pair counts its character as before it): what
/// AppKit, UIKit and Android selections are in, to Prism's characters
/// (#86).
pub fn utf16_to_chars(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (i, c) in text.chars().enumerate() {
        if units >= utf16 {
            return i;
        }
        units += c.len_utf16();
    }
    text.chars().count()
}

/// The UTF-16 offset of character `chars` in `text` (clamped to its end).
pub fn chars_to_utf16(text: &str, chars: usize) -> usize {
    text.chars().take(chars).map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets_to_characters_and_back() {
        let t = "a😀b";
        // a = 1 unit, 😀 = 2 units, b = 1 unit.
        assert_eq!(utf16_to_chars(t, 0), 0);
        assert_eq!(utf16_to_chars(t, 1), 1);
        assert_eq!(utf16_to_chars(t, 3), 2);
        assert_eq!(utf16_to_chars(t, 4), 3);
        assert_eq!(utf16_to_chars(t, 99), 3);
        assert_eq!(chars_to_utf16(t, 2), 3);
        assert_eq!(chars_to_utf16(t, 3), 4);
        assert_eq!(chars_to_utf16(t, 9), 4);
    }

    #[test]
    fn echoes_are_skipped_app_values_applied() {
        let mut t = Typed::default();
        // Typing "ab": both echoes, even late (the field shows "ab").
        t.sent("a");
        t.sent("ab");
        assert!(!t.should_apply("a", "ab"), "a late echo");
        assert!(!t.should_apply("ab", "ab"));
        // Nothing in flight: an app value applies, the field's own does not.
        assert!(t.should_apply("", "ab"));
        assert!(!t.should_apply("ab", "ab"));
    }

    #[test]
    fn markdown_shortcut_while_typing_ahead() {
        let mut t = Typed::default();
        t.sent("# ");
        t.sent("# T");
        // The app answers "# " by stripping it, then "# T" with "T".
        assert!(t.should_apply("", "# T"));
        assert!(t.should_apply("T", ""));
        // Typing goes on from the app's value.
        t.sent("Ti");
        assert!(!t.should_apply("Ti", "Ti"));
    }

    #[test]
    fn a_value_typed_twice_is_matched_in_order() {
        let mut t = Typed::default();
        for v in ["a", "", "a"] {
            t.sent(v);
        }
        assert!(!t.should_apply("a", "a"));
        assert!(
            !t.should_apply("", "a"),
            "the echo of the deletion, not an app clear"
        );
        assert!(!t.should_apply("a", "a"));
        // All echoed: clearing now is the app's.
        assert!(t.should_apply("", "a"));
    }

    #[test]
    fn pending_values_are_bounded() {
        let mut t = Typed::default();
        for i in 0..200 {
            t.sent(&i.to_string());
        }
        assert_eq!(t.sent.len(), MAX_PENDING);
        assert!(!t.should_apply("199", "199"));
    }

    #[test]
    fn caret_keeps_its_distance_from_the_end() {
        // "# Title|" → "Title|".
        assert_eq!(caret_from_end(7, 7, 5), 5);
        // "ab|cd" → "AB|CD".
        assert_eq!(caret_from_end(4, 2, 4), 2);
        // Cleared.
        assert_eq!(caret_from_end(4, 1, 0), 0);
        // Shorter than the distance: at the start.
        assert_eq!(caret_from_end(10, 2, 3), 0);
    }
}
