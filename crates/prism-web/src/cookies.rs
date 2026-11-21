//! `cx.set_cookie` on the web (#91): a socket cannot set a cookie, an HTTP
//! response can.
//!
//! 1. The session gives the cookie a one-time ticket (128 random bits, 60 s,
//!    bound to the browser's `prism_sid`) and sends it to the page.
//! 2. The page posts it to `<base>/_prism/cookie` in an `X-Prism-Cookie`
//!    header (never in the URL: it stays out of access logs and history).
//! 3. [`redeem`] checks the request comes from the page itself (same origin,
//!    the same browser), answers `204` with the `Set-Cookie`, and tells the
//!    session, which answers the app's call and updates its request.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use http::{HeaderMap, HeaderValue, Response, StatusCode, header};
use prism_core::Cookie;

use crate::session::Inbound;

/// How long a ticket can be redeemed.
pub(crate) const TICKET_TTL: Duration = Duration::from_secs(60);

/// The header the page sends the ticket in.
pub(crate) const TICKET_HEADER: &str = "x-prism-cookie";

/// A cookie waiting for its page to fetch it.
struct Pending {
    /// The browser it was issued to (`prism_sid`).
    sid: String,
    cookie: Cookie,
    expires: Instant,
    /// The session, and its call.
    session: mpsc::Sender<Inbound>,
    id: u64,
}

fn pending() -> &'static Mutex<HashMap<String, Pending>> {
    static P: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// A ticket for `cookie`, for browser `sid` and call `id` of `session`.
pub(crate) fn issue(sid: &str, cookie: Cookie, session: mpsc::Sender<Inbound>, id: u64) -> String {
    let ticket = crate::security::random_id();
    let now = Instant::now();
    let mut map = pending()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.retain(|_, p| p.expires > now);
    map.insert(
        ticket.clone(),
        Pending {
            sid: sid.to_string(),
            cookie,
            expires: now + TICKET_TTL,
            session,
            id,
        },
    );
    ticket
}

/// Forgets the tickets of call `id` (the page could not fetch it).
pub(crate) fn forget(id: u64, sid: &str) {
    pending()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|_, p| !(p.id == id && p.sid == sid));
}

/// Why the app's cookie cannot reach the browser of a page at `origin`
/// (`https://app.example.com`), before any ticket: a `Secure` cookie on a
/// plain `http://` page (other than `localhost`, which browsers treat as
/// secure) or a `Domain` the page's host is not under, which the browser
/// would drop without a word.
pub(crate) fn refused(cookie: &Cookie, origin: &str) -> Option<String> {
    let (scheme, rest) = origin.split_once("://").unwrap_or(("https", origin));
    let host = host_of(rest);
    if cookie.is_secure() && scheme == "http" && !is_local(&host) {
        return Some(format!(
            "the Secure cookie {:?} needs https: the browser would drop it on {origin}",
            cookie.name()
        ));
    }
    if let Some(domain) = cookie.domain_value() {
        let domain = domain.trim_start_matches('.').to_ascii_lowercase();
        if host != domain && !host.ends_with(&format!(".{domain}")) {
            return Some(format!(
                "the cookie {:?} is for {domain}, not for this page's host {host}",
                cookie.name()
            ));
        }
    }
    None
}

/// `app.example.com:8080` → `app.example.com`; `[::1]:3000` → `::1`.
fn host_of(authority: &str) -> String {
    let authority = authority.split('/').next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    host.to_ascii_lowercase()
}

fn is_local(host: &str) -> bool {
    host == "localhost" || host.ends_with(".localhost") || host == "::1" || host.starts_with("127.")
}

/// `POST <base>/_prism/cookie` with the ticket in `X-Prism-Cookie`.
pub(crate) fn redeem(headers: &HeaderMap) -> Response<Vec<u8>> {
    let answer = |code: StatusCode| {
        let mut r = Response::new(Vec::new());
        *r.status_mut() = code;
        r.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        r
    };
    let text = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    // The page's own fetch: same origin, never a form or a link from
    // another site (the ticket is not guessable either: this is depth).
    let same_site = text("sec-fetch-site").is_none_or(|s| s == "same-origin");
    let has_origin = text("origin").is_some_and(|o| o != "null");
    if !same_site || !has_origin || !crate::host::origin_allowed(headers, &[]) {
        return answer(StatusCode::FORBIDDEN);
    }
    let Some(ticket) = text(TICKET_HEADER) else {
        return answer(StatusCode::BAD_REQUEST);
    };
    // Taken out at once: a ticket works once, whatever happens next.
    let found = pending()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(ticket);
    let sid = crate::app::browser_id(headers);
    let Some(p) = found.filter(|p| p.expires > Instant::now() && sid.as_ref() == Some(&p.sid))
    else {
        return answer(StatusCode::FORBIDDEN);
    };
    let Ok(value) = HeaderValue::from_str(&p.cookie.to_header()) else {
        return answer(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let mut r = answer(StatusCode::NO_CONTENT);
    r.headers_mut().insert(header::SET_COOKIE, value);
    let _ = p.session.send(Inbound::CookieSet(p.id, p.cookie));
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_cookies_need_https_except_on_localhost() {
        let c = Cookie::new("s", "1");
        assert!(refused(&c, "https://app.example.com").is_none());
        assert!(refused(&c, "http://localhost:3000").is_none());
        assert!(refused(&c, "http://127.0.0.1:3000").is_none());
        assert!(refused(&c, "http://[::1]:3000").is_none());
        let err = refused(&c, "http://app.example.com").unwrap();
        assert!(err.contains("needs https"), "{err}");
        assert!(refused(&c.clone().secure(false), "http://app.example.com").is_none());
    }

    #[test]
    fn a_domain_must_cover_the_page_host() {
        let c = Cookie::new("s", "1");
        let at = "https://app.example.com";
        assert!(refused(&c.clone().domain("example.com"), at).is_none());
        assert!(refused(&c.clone().domain(".example.com"), at).is_none());
        assert!(refused(&c.clone().domain("app.example.com"), at).is_none());
        assert!(refused(&c.clone().domain("other.com"), at).is_some());
        assert!(refused(&c.clone().domain("ample.com"), at).is_some());
    }
}
