//! A Prism notification as an app notification's XML (the toast schema:
//! `ToastGeneric`, buttons, reply fields, sound), and the arguments a click
//! brings back. Pure: tested on every host.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::surfaces::Notification;

/// What a click on a notification or one of its buttons says.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Clicked {
    /// The notification's id.
    pub id: String,
    /// The button's id; empty for the notification itself.
    pub action: String,
    /// The deep link to open in the app.
    pub link: String,
    /// The button brings the app forward (a click on the body does).
    pub foreground: bool,
}

/// `&`-separated `key=value` pairs, values percent-encoded.
pub fn arguments(c: &Clicked) -> String {
    format!(
        "prism=1&id={}&action={}&link={}&fg={}",
        encode(&c.id),
        encode(&c.action),
        encode(&c.link),
        u8::from(c.foreground)
    )
}

/// The click's arguments, unless they are not Prism's.
pub fn parse(arguments: &str) -> Option<Clicked> {
    let mut c = Clicked::default();
    let mut ours = false;
    for pair in arguments.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = decode(v);
        match k {
            "prism" => ours = true,
            "id" => c.id = v,
            "action" => c.action = v,
            "link" => c.link = v,
            "fg" => c.foreground = v == "1",
            _ => {}
        }
    }
    ours.then_some(c)
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/' | b':') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h << 4 | l);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Escapes text for XML.
fn xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && !matches!(c, '\n' | '\t') => {}
            c => out.push(c),
        }
    }
    out
}

/// The text field of a reply button.
pub fn input_id(action: &str) -> String {
    format!("reply-{action}")
}

/// A tag or a group: at most 64 characters (Windows' limit), else a hash
/// of it (the same for the same id, so it can be withdrawn).
pub fn short(id: &str) -> String {
    if id.chars().count() <= 64 {
        return id.to_string();
    }
    // FNV-1a: stable across runs and builds.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("prism-{h:016x}")
}

/// The notification's XML.
pub fn payload(n: &Notification) -> String {
    let launch = arguments(&Clicked {
        id: n.id.clone(),
        action: String::new(),
        link: n.link.clone().unwrap_or_default(),
        foreground: true,
    });
    let mut s = format!(
        "<toast launch=\"{}\" useButtonStyle=\"true\"><visual><binding template=\"ToastGeneric\">",
        xml(&launch)
    );
    s += &format!("<text>{}</text>", xml(&n.title));
    for line in [n.subtitle.as_deref(), Some(n.body.as_str())]
        .into_iter()
        .flatten()
        .filter(|l| !l.is_empty())
    {
        s += &format!("<text>{}</text>", xml(line));
    }
    s += "</binding></visual>";
    if !n.actions.is_empty() {
        s += "<actions>";
        for a in n.actions.iter().filter(|a| a.reply.is_some()) {
            s += &format!(
                "<input id=\"{}\" type=\"text\" placeHolderContent=\"{}\"/>",
                xml(&input_id(&a.id)),
                xml(a.reply.as_deref().unwrap_or_default())
            );
        }
        for a in &n.actions {
            let args = arguments(&Clicked {
                id: n.id.clone(),
                action: a.id.clone(),
                link: a.link.clone().unwrap_or_default(),
                foreground: a.foreground,
            });
            s += &format!(
                "<action content=\"{}\" arguments=\"{}\"",
                xml(&a.title),
                xml(&args)
            );
            if a.reply.is_some() {
                s += &format!(" hint-inputId=\"{}\"", xml(&input_id(&a.id)));
            }
            if a.destructive {
                s += " hint-buttonStyle=\"Critical\"";
            }
            s += "/>";
        }
        s += "</actions>";
    }
    if !n.sound {
        s += "<audio silent=\"true\"/>";
    }
    s += "</toast>";
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::NotificationAction;

    fn note() -> Notification {
        Notification::new("Tea & cake <now>")
            .id("t-1")
            .subtitle("Kitchen")
            .body("Body \"quoted\"")
            .link("/todos/4?x=1&y=2")
            .silent()
            .action(NotificationAction::new("reply", "Reply").reply("Message"))
            .action(NotificationAction::new("del", "Delete").destructive())
    }

    #[test]
    fn payload_escapes_and_lists_the_buttons() {
        let x = payload(&note());
        assert!(x.starts_with("<toast launch=\"prism=1&amp;id=t-1&amp;action=&amp;link=/todos/4%3Fx%3D1%26y%3D2&amp;fg=1\""));
        assert!(x.contains("<text>Tea &amp; cake &lt;now&gt;</text><text>Kitchen</text><text>Body &quot;quoted&quot;</text>"));
        assert!(
            x.contains("<input id=\"reply-reply\" type=\"text\" placeHolderContent=\"Message\"/>")
        );
        assert!(x.contains("hint-inputId=\"reply-reply\""));
        assert!(x.contains("content=\"Delete\""));
        assert!(x.contains("hint-buttonStyle=\"Critical\""));
        assert!(x.ends_with("<audio silent=\"true\"/></toast>"));
    }

    #[test]
    fn arguments_round_trip() {
        let c = Clicked {
            id: "a&b=c%d ✓".into(),
            action: "reply".into(),
            link: "/todos/4?x=1&y=2".into(),
            foreground: true,
        };
        assert_eq!(parse(&arguments(&c)), Some(c));
        assert_eq!(parse("action=reply"), None);
    }

    #[test]
    fn long_tags_are_hashed() {
        assert_eq!(short("todo-1"), "todo-1");
        let long = "x".repeat(80);
        assert_eq!(short(&long), short(&long));
        assert!(short(&long).len() <= 64);
        assert_ne!(short(&long), short(&"y".repeat(80)));
    }
}
