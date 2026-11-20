//! Pure helpers of the Windows backend (no Win32 types, so they are
//! tested on every host): virtual-key codes as the web's
//! `KeyboardEvent.key` names, and SF Symbol names as Segoe Fluent Icons.

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

#[cfg(test)]
mod tests {
    use super::*;

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
