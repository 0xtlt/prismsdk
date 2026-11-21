//! Pure helpers of the Windows backend (no Win32 types, so they are
//! tested on every host): virtual-key codes as the web's
//! `KeyboardEvent.key` names, a text field caret's edges, and SF Symbol
//! names as Segoe Fluent Icons.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

/// The web key name (`"ArrowUp"`, `"Enter"`, `"a"`) of a virtual key, from
/// its code (`VK_*`) and the character it types with the current layout
/// and modifiers (`ToUnicode`), if any. `None` for keys Prism does not
/// report alone (Shift, Control, dead keys…).
pub fn key_name(vk: u16, typed: Option<char>) -> Option<String> {
    let named = match vk {
        0x1B => "Escape",
        0x0D => "Enter",
        0x09 => "Tab",
        0x08 => "Backspace",
        0x2E => "Delete",
        0x26 => "ArrowUp",
        0x28 => "ArrowDown",
        0x25 => "ArrowLeft",
        0x27 => "ArrowRight",
        0x24 => "Home",
        0x23 => "End",
        0x21 => "PageUp",
        0x22 => "PageDown",
        0x2D => "Insert",
        0x5D => "ContextMenu",
        0xB3 => "MediaPlayPause",
        0x20 => " ",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_string());
    }
    // F1–F24.
    if (0x70..=0x87).contains(&vk) {
        return Some(format!("F{}", vk - 0x6F));
    }
    if let Some(c) = typed.filter(|c| !c.is_control()) {
        return Some(c.to_string());
    }
    // Ctrl+letter types a control character: name the letter (Ctrl+S → "s").
    match vk {
        0x41..=0x5A => Some(((vk as u8 - 0x41 + b'a') as char).to_string()),
        0x30..=0x39 => Some(((vk as u8) as char).to_string()),
        _ => None,
    }
}

/// Whether a key press would type text into a field: a printed character
/// without Control, Alt or the Windows key (AltGr is Control + Alt: it
/// types too).
pub fn types_text(key: &str, control: bool, alt: bool, meta: bool) -> bool {
    key.chars().count() == 1 && (control == alt) && !meta
}

/// Keys a focusable view keeps from the backend's own handling (focus
/// moves with arrows, activation with Enter / Space) when it listens to
/// keys itself.
pub fn claims(key: &str) -> bool {
    matches!(
        key,
        "ArrowUp"
            | "ArrowDown"
            | "ArrowLeft"
            | "ArrowRight"
            | "Enter"
            | " "
            | "Home"
            | "End"
            | "PageUp"
            | "PageDown"
    )
}

/// The Segoe Fluent Icons (Windows 11) / Segoe MDL2 Assets (Windows 10)
/// glyph for a common SF Symbol name. Unknown symbols have none: the
/// caller falls back to `prism_core::glyphs::glyph`.
pub fn fluent_icon(symbol: &str) -> Option<char> {
    let base = symbol.trim_end_matches(".fill");
    let code = match base {
        "house" => 0xE80F,
        "gear" | "gearshape" | "gearshape.2" => 0xE713,
        "checklist" | "list.bullet" | "list.bullet.rectangle" => 0xE9D5,
        "checkmark" => 0xE73E,
        "checkmark.circle" => 0xE930,
        "circle" => 0xEA3A,
        "star" => 0xE734,
        "heart" => 0xEB51,
        "magnifyingglass" => 0xE721,
        "bell" | "app.badge" => 0xEA8F,
        "square.grid.2x2" | "rectangle.grid.2x2" => 0xF0E2,
        "timer" | "stopwatch" => 0xE916,
        "clock" => 0xE823,
        "xmark" | "xmark.circle" => 0xE711,
        "plus" | "plus.circle" => 0xE710,
        "minus" => 0xE738,
        "chevron.right" => 0xE76C,
        "chevron.left" => 0xE76B,
        "chevron.down" => 0xE70D,
        "chevron.up" => 0xE70E,
        "arrow.left" => 0xE72B,
        "arrow.right" => 0xE72A,
        "arrow.clockwise" => 0xE72C,
        "person" | "person.crop.circle" | "person.circle" => 0xE77B,
        "pencil" | "square.and.pencil" => 0xE70F,
        "trash" => 0xE74D,
        "ellipsis" | "ellipsis.circle" => 0xE712,
        "square.and.arrow.up" => 0xE72D,
        "paintbrush" | "paintpalette" => 0xE790,
        "bolt" => 0xE945,
        "flame" => 0xECAD,
        "sparkles" => 0xE9F5,
        "photo" => 0xEB9F,
        "folder" => 0xE8B7,
        "doc" | "doc.text" => 0xE8A5,
        "envelope" => 0xE715,
        "lock" | "lock.shield" => 0xE72E,
        "key" => 0xE8D7,
        "globe" | "network" => 0xE774,
        "calendar" => 0xE787,
        "link" => 0xE71B,
        "mappin" | "mappin.and.ellipse" | "location" => 0xE707,
        "info.circle" | "info" => 0xE946,
        "exclamationmark.triangle" => 0xE7BA,
        "play" => 0xE768,
        "pause" => 0xE769,
        "stop" => 0xE71A,
        "cart" => 0xE7BF,
        "tag" => 0xE8EC,
        "phone" => 0xE717,
        "camera" => 0xE722,
        "mic" => 0xE720,
        "speaker.wave.2" => 0xE767,
        "moon" => 0xE708,
        "sun.max" => 0xE706,
        "rectangle.stack" => 0xE8F1,
        "square.and.arrow.down" => 0xE896,
        "arrow.up.arrow.down" => 0xE8CB,
        "line.3.horizontal" => 0xE700,
        "cup.and.saucer" => 0xEC32,
        "brain.head.profile" => 0xE9D9,
        "hand.raised" => 0xE7C9,
        "cube" => 0xF158,
        "wifi" => 0xE701,
        "doc.on.doc" => 0xE8C8,
        "eye" => 0xE7B3,
        "eye.slash" => 0xED1A,
        _ => return None,
    };
    char::from_u32(code)
}

/// The keys whose press was reported and that are still down (#159),
/// each with its name and the node it went to: a release is reported to
/// that node under that name (the name a key types can change while it is
/// down, with Shift), only if its press was; the window turning inactive
/// releases them all (Windows sends their releases to the window that is
/// active then).
#[derive(Clone, Debug, Default)]
pub struct Held {
    keys: Vec<(u32, String, Option<prism_core::NodeId>)>,
}

impl Held {
    /// The press of key `code` (its virtual key and scan code) was
    /// reported as `name` to `target`.
    pub fn press(&mut self, code: u32, name: &str, target: Option<prism_core::NodeId>) {
        if let Some(k) = self.keys.iter().find(|k| k.0 == code)
            && k.1 == name
            && k.2 == target
        {
            // A repeat: it keeps its place.
            return;
        }
        self.keys.retain(|k| k.0 != code);
        self.keys.push((code, name.to_string(), target));
    }

    /// Key `code` went up: its name and node, if its press was reported.
    pub fn release(&mut self, code: u32) -> Option<(String, Option<prism_core::NodeId>)> {
        let i = self.keys.iter().position(|k| k.0 == code)?;
        let (_, name, target) = self.keys.remove(i);
        Some((name, target))
    }

    /// Every key still down, in the order they went down.
    pub fn take_all(&mut self) -> Vec<(String, Option<prism_core::NodeId>)> {
        std::mem::take(&mut self.keys)
            .into_iter()
            .map(|(_, name, target)| (name, target))
            .collect()
    }
}

/// Where a multi-line field's lines are, measured in the field: the
/// middles of the caret's line, of the first and of the last line, and
/// the caret's height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lines {
    pub caret: f32,
    pub first: f32,
    pub last: f32,
    pub height: f32,
}

/// Where a text field's caret is (#211), with nothing selected: before
/// character `caret` of `len`; on the first or last visual line from
/// `lines` (a multi-line field's, wrapping included), always for one line
/// (`None`).
pub fn caret_edges(caret: usize, len: usize, lines: Option<Lines>) -> prism_core::CaretEdges {
    let same = |y: f32| {
        lines
            .as_ref()
            .is_none_or(|l| (y - l.caret).abs() < l.height.max(1.) / 2.)
    };
    prism_core::CaretEdges {
        start: caret == 0,
        end: caret >= len,
        first_line: caret == 0 || same(lines.map_or(0., |l| l.first)),
        last_line: caret >= len || same(lines.map_or(0., |l| l.last)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_match_their_reported_press() {
        let mut held = Held::default();
        held.press(0x41_001E, "A", Some(4));
        held.press(0x26_0048, "ArrowUp", None);
        // A repeat keeps the press.
        held.press(0x41_001E, "A", Some(4));
        // Released after Shift: still the name and node it went down with.
        assert_eq!(held.release(0x41_001E), Some(("A".into(), Some(4))));
        // A release whose press was not reported is not reported.
        assert_eq!(held.release(0x41_001E), None);
        assert_eq!(held.release(0x42_0030), None);
        held.press(0x20_0039, " ", Some(9));
        // The window turns inactive: the rest go up, in order.
        assert_eq!(
            held.take_all(),
            vec![("ArrowUp".into(), None), (" ".into(), Some(9))]
        );
        assert_eq!(held.take_all(), vec![]);
    }

    #[test]
    fn caret_edges_from_the_lines() {
        // One line: always on the first and last line.
        let one = caret_edges(3, 8, None);
        assert!(!one.start && !one.end && one.first_line && one.last_line);
        assert!(caret_edges(0, 8, None).start && caret_edges(8, 8, None).end);
        // Three lines 20 high, the caret on the middle one.
        let lines = |caret| Lines {
            caret,
            first: 10.,
            last: 50.,
            height: 20.,
        };
        let mid = caret_edges(12, 30, Some(lines(30.)));
        assert!(!mid.first_line && !mid.last_line && !mid.start && !mid.end);
        let top = caret_edges(4, 30, Some(lines(10.5)));
        assert!(top.first_line && !top.last_line);
        let bottom = caret_edges(25, 30, Some(lines(49.)));
        assert!(!bottom.first_line && bottom.last_line);
        // The very start and end are on their lines, whatever was measured.
        assert!(caret_edges(0, 30, Some(lines(30.))).first_line);
        assert!(caret_edges(30, 30, Some(lines(30.))).last_line);
        // An empty field: every edge.
        let empty = caret_edges(0, 0, Some(lines(10.)));
        assert!(empty.start && empty.end && empty.first_line && empty.last_line);
    }

    #[test]
    fn virtual_keys_map_to_web_names() {
        assert_eq!(key_name(0x1B, None).as_deref(), Some("Escape"));
        assert_eq!(key_name(0x0D, Some('\r')).as_deref(), Some("Enter"));
        assert_eq!(key_name(0x09, Some('\t')).as_deref(), Some("Tab"));
        assert_eq!(key_name(0x08, Some('\u{8}')).as_deref(), Some("Backspace"));
        assert_eq!(key_name(0x26, None).as_deref(), Some("ArrowUp"));
        assert_eq!(key_name(0x22, None).as_deref(), Some("PageDown"));
        assert_eq!(key_name(0x7B, None).as_deref(), Some("F12"));
        assert_eq!(key_name(0x20, Some(' ')).as_deref(), Some(" "));
        assert_eq!(key_name(0x41, Some('a')).as_deref(), Some("a"));
        assert_eq!(key_name(0x41, Some('A')).as_deref(), Some("A"));
        // Ctrl+S types U+0013: the letter is reported.
        assert_eq!(key_name(0x53, Some('\u{13}')).as_deref(), Some("s"));
        assert_eq!(key_name(0x32, None).as_deref(), Some("2"));
        assert_eq!(key_name(0xC0, Some('é')).as_deref(), Some("é"));
        // Shift, Control.
        assert_eq!(key_name(0x10, None), None);
        assert_eq!(key_name(0x11, None), None);
    }

    #[test]
    fn typing_and_claimed_keys() {
        assert!(types_text("a", false, false, false));
        assert!(!types_text("a", true, false, false));
        // AltGr (Control + Alt) types.
        assert!(types_text("@", true, true, false));
        assert!(!types_text("Escape", false, false, false));
        assert!(claims("ArrowDown") && claims(" ") && !claims("Escape") && !claims("Tab"));
    }

    #[test]
    fn sf_symbols_map_to_fluent_icons() {
        assert_eq!(fluent_icon("gearshape"), Some('\u{E713}'));
        assert_eq!(fluent_icon("gearshape.fill"), Some('\u{E713}'));
        assert_eq!(fluent_icon("checklist"), Some('\u{E9D5}'));
        assert_eq!(fluent_icon("sparkles.tv"), None);
    }
}
