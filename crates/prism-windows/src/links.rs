//! What a web view may open outside the app (S41): the decisions of
//! `web.rs`, without WebView2, tested on every host.
//!
//! WebView2 has no popup blocker: a page's `window.open` reaches
//! `NewWindowRequested` with or without a user gesture. A new window goes to
//! the default browser only when the user asked for it (a click or a key on
//! a link) and the sandbox allows popups; a link in the page itself leaves
//! the allowed origins the same way. Only web, mail and phone links leave
//! the app: another scheme could start any program registered for it.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

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

/// A new window the page asked for (`window.open`, a `target=_blank`
/// link): `Ok` to open `url` in its app, else why not. Never a window of
/// the web view's own.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_opens_only_after_a_gesture() {
        assert_eq!(new_window("https://example.com/", true, true), Ok(()));
        assert_eq!(
            new_window("https://example.com/", true, false),
            Err(Refused::NoGesture)
        );
    }

    #[test]
    fn a_sandbox_without_popups_opens_nothing() {
        assert_eq!(
            new_window("https://example.com/", false, true),
            Err(Refused::Sandbox)
        );
        assert_eq!(
            new_window("https://example.com/", false, false),
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
            "file:///C:/Windows/System32/calc.exe",
            "ms-settings:privacy",
            "javascript:alert(1)",
            "intent://x#Intent;end",
            "",
        ] {
            assert_eq!(new_window(url, true, true), Err(Refused::Scheme), "{url}");
        }
    }
}
