//! Pure helpers of the GTK backend (no GTK types, so they are tested on
//! every host): GDK key names as the web's `KeyboardEvent.key` names, and
//! SF Symbol names as freedesktop icon names.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

/// The web key name (`"ArrowUp"`, `"Enter"`, `"a"`) of a GDK key, from its
/// name (`gdk_keyval_name`: `"Up"`, `"Return"`, `"KP_Enter"`) and the
/// character it types, if any. `None` for keys Prism does not report alone
/// (Shift, Control, dead keys…).
pub fn key_name(gdk_name: &str, unicode: Option<char>) -> Option<String> {
    let named = match gdk_name {
        "Escape" => "Escape",
        "Return" | "KP_Enter" | "ISO_Enter" => "Enter",
        "Tab" | "ISO_Left_Tab" | "KP_Tab" => "Tab",
        "BackSpace" => "Backspace",
        "Delete" | "KP_Delete" => "Delete",
        "Up" | "KP_Up" => "ArrowUp",
        "Down" | "KP_Down" => "ArrowDown",
        "Left" | "KP_Left" => "ArrowLeft",
        "Right" | "KP_Right" => "ArrowRight",
        "Home" | "KP_Home" => "Home",
        "End" | "KP_End" => "End",
        "Page_Up" | "KP_Page_Up" | "Prior" => "PageUp",
        "Page_Down" | "KP_Page_Down" | "Next" => "PageDown",
        "Insert" | "KP_Insert" => "Insert",
        "Menu" => "ContextMenu",
        "AudioPlay" | "AudioPause" => "MediaPlayPause",
        "space" | "KP_Space" => " ",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_string());
    }
    if let Some(n) = gdk_name.strip_prefix('F')
        && n.parse::<u8>().is_ok_and(|n| (1..=24).contains(&n))
    {
        return Some(gdk_name.to_string());
    }
    unicode.filter(|c| !c.is_control()).map(|c| c.to_string())
}

/// Whether a key press would type text into a field: a printed character
/// without Control, Alt or Super.
pub fn types_text(key: &str, control: bool, alt: bool, meta: bool) -> bool {
    key.chars().count() == 1 && !control && !alt && !meta
}

/// Keys a focusable view keeps from GTK's own handling (focus moves with
/// arrows, activation with Enter / Space) when it listens to keys itself.
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

/// The freedesktop (Adwaita) icon name for a common SF Symbol, for icons
/// inside text fields. Unknown symbols have none.
pub fn freedesktop_icon(symbol: &str) -> Option<&'static str> {
    let base = symbol
        .trim_end_matches(".fill")
        .trim_end_matches(".circle")
        .trim_end_matches(".fill");
    Some(match base {
        "magnifyingglass" => "system-search-symbolic",
        "envelope" | "at" => "mail-unread-symbolic",
        "lock" | "lock.shield" | "key" | "key.fill" => "dialog-password-symbolic",
        "person" | "person.crop" => "avatar-default-symbolic",
        "globe" | "network" | "safari" => "web-browser-symbolic",
        "phone" => "call-start-symbolic",
        "calendar" => "x-office-calendar-symbolic",
        "clock" => "document-open-recent-symbolic",
        "link" => "insert-link-symbolic",
        "mappin" | "mappin.and.ellipse" | "location" => "mark-location-symbolic",
        "star" => "starred-symbolic",
        "heart" => "emblem-favorite-symbolic",
        "gear" | "gearshape" => "emblem-system-symbolic",
        "house" => "go-home-symbolic",
        "tag" => "tag-symbolic",
        "pencil" | "square.and.pencil" => "document-edit-symbolic",
        "creditcard" => "auth-smartcard-symbolic",
        "number" | "textformat" => "format-text-plain-symbolic",
        "photo" => "image-x-generic-symbolic",
        "folder" => "folder-symbolic",
        "doc" | "doc.text" => "text-x-generic-symbolic",
        "bell" => "preferences-system-notifications-symbolic",
        "cart" => "emblem-shared-symbolic",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gdk_keys_map_to_web_names() {
        let k = |n: &str, c: Option<char>| key_name(n, c);
        assert_eq!(k("Escape", None).as_deref(), Some("Escape"));
        assert_eq!(k("Return", Some('\r')).as_deref(), Some("Enter"));
        assert_eq!(k("KP_Enter", None).as_deref(), Some("Enter"));
        assert_eq!(k("ISO_Left_Tab", None).as_deref(), Some("Tab"));
        assert_eq!(k("BackSpace", Some('\u{8}')).as_deref(), Some("Backspace"));
        assert_eq!(k("Up", None).as_deref(), Some("ArrowUp"));
        assert_eq!(k("KP_Left", None).as_deref(), Some("ArrowLeft"));
        assert_eq!(k("Page_Down", None).as_deref(), Some("PageDown"));
        assert_eq!(k("F12", None).as_deref(), Some("F12"));
        assert_eq!(k("space", Some(' ')).as_deref(), Some(" "));
        assert_eq!(k("a", Some('a')).as_deref(), Some("a"));
        assert_eq!(k("A", Some('A')).as_deref(), Some("A"));
        assert_eq!(k("eacute", Some('é')).as_deref(), Some("é"));
        assert_eq!(k("Shift_L", None), None);
        assert_eq!(k("Fancy", None), None);
    }

    #[test]
    fn typing_and_claimed_keys() {
        assert!(types_text("a", false, false, false));
        assert!(!types_text("a", true, false, false));
        assert!(!types_text("Escape", false, false, false));
        assert!(claims("ArrowDown") && claims(" ") && !claims("Escape") && !claims("Tab"));
    }

    #[test]
    fn sf_symbols_map_to_freedesktop_icons() {
        assert_eq!(
            freedesktop_icon("magnifyingglass"),
            Some("system-search-symbolic")
        );
        assert_eq!(
            freedesktop_icon("envelope.fill"),
            Some("mail-unread-symbolic")
        );
        assert_eq!(
            freedesktop_icon("person.circle.fill"),
            Some("avatar-default-symbolic")
        );
        assert_eq!(freedesktop_icon("sparkles.tv"), None);
    }
}
