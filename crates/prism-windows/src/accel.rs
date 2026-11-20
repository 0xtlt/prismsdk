//! Menu shortcuts (`"cmd+shift+n"`) as Windows keyboard accelerators: the
//! virtual key, the modifiers (Command is Ctrl here, Option is Alt) and
//! the text menus show (`Ctrl+Shift+N`). Pure: tested on every host.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

/// `VirtualKeyModifiers` flags.
pub const CONTROL: u32 = 1;
pub const ALT: u32 = 2;
pub const SHIFT: u32 = 4;
pub const WINDOWS: u32 = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    /// `VirtualKey` (a Win32 virtual-key code).
    pub key: i32,
    pub modifiers: u32,
    /// As Windows menus write it.
    pub label: String,
}

/// The key's virtual-key code and its name in menus.
fn key(name: &str) -> Option<(i32, String)> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.clone().next()) {
        let c = c.to_ascii_uppercase();
        let vk = match c {
            'A'..='Z' | '0'..='9' => c as i32,
            ';' => 0xBA,
            '=' | '+' => 0xBB,
            ',' => 0xBC,
            '-' => 0xBD,
            '.' => 0xBE,
            '/' => 0xBF,
            '`' => 0xC0,
            '[' => 0xDB,
            '\\' => 0xDC,
            ']' => 0xDD,
            '\'' => 0xDE,
            _ => return None,
        };
        return Some((vk, c.to_string()));
    }
    let lower = name.to_ascii_lowercase();
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<i32>().ok())
        && (1..=24).contains(&n)
    {
        return Some((0x6F + n, format!("F{n}")));
    }
    let (vk, label) = match lower.as_str() {
        "enter" | "return" => (0x0D, "Enter"),
        "escape" | "esc" => (0x1B, "Esc"),
        "space" => (0x20, "Space"),
        "tab" => (0x09, "Tab"),
        "backspace" => (0x08, "Backspace"),
        "delete" | "del" => (0x2E, "Del"),
        "insert" => (0x2D, "Ins"),
        "home" => (0x24, "Home"),
        "end" => (0x23, "End"),
        "pageup" => (0x21, "PgUp"),
        "pagedown" => (0x22, "PgDn"),
        "left" | "arrowleft" => (0x25, "Left"),
        "up" | "arrowup" => (0x26, "Up"),
        "right" | "arrowright" => (0x27, "Right"),
        "down" | "arrowdown" => (0x28, "Down"),
        "plus" => (0xBB, "+"),
        "minus" => (0xBD, "-"),
        "comma" => (0xBC, ","),
        "period" => (0xBE, "."),
        _ => return None,
    };
    Some((vk, label.into()))
}

/// `"cmd+shift+n"` → Ctrl+Shift+N. `None` for keys Windows has no code
/// for.
pub fn parse(keys: &str) -> Option<Shortcut> {
    let mut modifiers = 0;
    let mut found = None;
    for part in keys.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "ctrl" | "control" | "meta" | "primary" => modifiers |= CONTROL,
            "shift" => modifiers |= SHIFT,
            "alt" | "option" | "opt" => modifiers |= ALT,
            "super" | "win" | "windows" => modifiers |= WINDOWS,
            // `cmd++`: the empty part after the last `+` is the plus key.
            "" => found = key("+"),
            other => found = key(other),
        }
    }
    let (key, name) = found?;
    let mut label = String::new();
    for (flag, text) in [
        (WINDOWS, "Win+"),
        (CONTROL, "Ctrl+"),
        (ALT, "Alt+"),
        (SHIFT, "Shift+"),
    ] {
        if modifiers & flag != 0 {
            label += text;
        }
    }
    label += &name;
    Some(Shortcut {
        key,
        modifiers,
        label,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts() {
        let s = parse("cmd+shift+n").unwrap();
        assert_eq!((s.key, s.modifiers), ('N' as i32, CONTROL | SHIFT));
        assert_eq!(s.label, "Ctrl+Shift+N");
        assert_eq!(parse("cmd+,").unwrap().label, "Ctrl+,");
        assert_eq!(parse("cmd+,").unwrap().key, 0xBC);
        assert_eq!(parse("cmd+1").unwrap().key, '1' as i32);
        assert_eq!(parse("alt+f4").unwrap().label, "Alt+F4");
        assert_eq!(parse("F5").unwrap().key, 0x74);
        assert_eq!(parse("cmd+delete").unwrap().label, "Ctrl+Del");
        assert_eq!(parse("cmd++").unwrap().key, 0xBB);
        assert_eq!(parse("cmd+§"), None);
    }
}
