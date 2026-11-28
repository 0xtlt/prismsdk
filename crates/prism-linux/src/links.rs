//! What a web view lets through (#343): the decisions of `web.rs`, without
//! WebKitGTK, tested without a display. The same rules as the Windows backend's
//! `links.rs` and the Apple bridge (`PrismWebView.swift`):
//!
//! - a navigation the user started (a click on a link, a form) towards an
//!   origin the view does not allow opens in the system browser instead;
//! - a new window (`target="_blank"`, `window.open`) opens in the system
//!   browser, only when the sandbox allows popups and the user asked for
//!   it, never in a window of the view's own;
//! - only web, mail and phone links leave the app: another scheme could
//!   start any program registered for it;
//! - a message from the page comes with the origin of the page that sent
//!   it, and is heard only from an allowed one and when it is JSON.

use prism_core::web_view::origin_of;

/// Why a page's request to open something outside the app was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// No click or key from the user: a script asked on its own.
    NoGesture,
    /// The web view's sandbox does not allow popups.
    Sandbox,
    /// Not a web, mail or phone link.
    Scheme,
}

impl Refused {
    /// For the log: why nothing opened.
    pub fn reason(self) -> &'static str {
        match self {
            Refused::NoGesture => "no user gesture",
            Refused::Sandbox => "the sandbox has no popups",
            Refused::Scheme => "not a web, mail or phone link",
        }
    }
}

/// Whether `url`'s scheme may leave the app: http, https, mailto, tel.
pub fn external_scheme(url: &str) -> bool {
    let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
    matches!(scheme.as_str(), "http" | "https" | "mailto" | "tel")
}

/// A new window the page asked for: `Ok` to open `url` in its app, else
/// why not.
pub fn new_window(url: &str, popups: bool, user_initiated: bool) -> Result<(), Refused> {
    if !popups {
        Err(Refused::Sandbox)
    } else if !user_initiated {
        Err(Refused::NoGesture)
    } else if !external_scheme(url) {
        Err(Refused::Scheme)
    } else {
        Ok(())
    }
}

/// The origin of the page at `url`: `file://` for a local file (as WebKit
/// reports it), none for `about:blank` or `data:`.
pub fn page_origin(url: &str) -> Option<String> {
    origin_of(url).or_else(|| {
        url.get(..5)
            .is_some_and(|s| s.eq_ignore_ascii_case("file:"))
            .then(|| "file://".to_string())
    })
}

/// Whether a navigation to `url` leaves the view for the system browser:
/// the user started it, it goes to a web page, and `allows` (the view's
/// allowed origins) refuses that page's origin.
pub fn leaves(url: &str, user_initiated: bool, allows: impl Fn(Option<&str>) -> bool) -> bool {
    let web = matches!(
        url.split(':')
            .next()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("http" | "https")
    );
    user_initiated && web && !allows(page_origin(url).as_deref())
}

/// What the bridge's isolated script posts (`origin`, a newline, then the
/// page's JSON): the origin and the JSON, if the JSON parses.
pub fn parse_message(body: &str) -> Option<(&str, &str)> {
    let (origin, json) = body.split_once('\n')?;
    serde_json::from_str::<serde_json::Value>(json).ok()?;
    Some((origin, json))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_opens_only_after_a_gesture_and_with_popups() {
        assert_eq!(new_window("https://example.com/", true, true), Ok(()));
        assert_eq!(
            new_window("https://example.com/", true, false),
            Err(Refused::NoGesture)
        );
        assert_eq!(
            new_window("https://example.com/", false, true),
            Err(Refused::Sandbox)
        );
    }

    #[test]
    fn only_web_mail_and_phone_links_leave() {
        for url in [
            "http://a.test/",
            "HTTPS://a.test/",
            "mailto:a@a.test",
            "tel:5550100",
        ] {
            assert_eq!(new_window(url, true, true), Ok(()), "{url}");
        }
        for url in [
            "file:///usr/bin/xterm",
            "apt:sl",
            "javascript:alert(1)",
            "steam://run/1",
            "",
        ] {
            assert_eq!(new_window(url, true, true), Err(Refused::Scheme), "{url}");
        }
    }

    #[test]
    fn a_click_to_another_origin_leaves() {
        let allows = |o: Option<&str>| o == Some("https://app.example");
        assert!(leaves("https://other.example/x", true, allows));
        assert!(!leaves("https://app.example/next", true, allows));
        // Scripts and redirects stay (their messages are filtered).
        assert!(!leaves("https://other.example/x", false, allows));
        // Not a web page: WebKit handles it (or refuses it).
        assert!(!leaves("about:blank", true, allows));
        assert!(!leaves("file:///etc/passwd", true, allows));
    }

    #[test]
    fn local_files_have_the_file_origin() {
        assert_eq!(
            page_origin("file:///tmp/a.html").as_deref(),
            Some("file://")
        );
        assert_eq!(
            page_origin("https://A.example:443/x").as_deref(),
            Some("https://a.example")
        );
        assert_eq!(page_origin("about:blank"), None);
    }

    #[test]
    fn messages_carry_their_origin_and_must_be_json() {
        assert_eq!(
            parse_message("https://app.example\n{\"a\":1}"),
            Some(("https://app.example", "{\"a\":1}"))
        );
        // JSON text may hold newlines.
        assert_eq!(
            parse_message("file://\n[1,\n2]"),
            Some(("file://", "[1,\n2]"))
        );
        assert_eq!(parse_message("https://app.example\nnot json"), None);
        assert_eq!(parse_message("{\"a\":1}"), None);
    }
}
