//! The web request a session was opened with: its URL, origin, headers and
//! cookies, plus whatever the host server attached to it (the signed-in user).
//!
//! On the web, the first paint renders with the page request; the live
//! session renders with the socket handshake (re-read at every reconnect).
//! The first live session after a page load also gets that page's request
//! back ([`App::page_request`]), for what the host hands out once (a flash
//! message after a redirect, a one-time token). Native apps have no request:
//! [`App::request`] and [`App::page_origin`] are `None` there.
//!
//! A handler that signs a user in sets the session cookie with
//! [`App::set_cookie`] (or [`AsyncApp::set_cookie`](crate::AsyncApp::set_cookie)
//! to learn the outcome), then [`App::reconnect`]s so the host's middleware
//! sees it on the next handshake.

use std::any::Any;
use std::sync::Arc;

use crate::app::App;
use crate::device::{DeviceCall, DeviceError, DeviceValue};

/// Which request a [`WebRequest`] describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RequestKind {
    /// The page request (server-side render of the first paint).
    #[default]
    Page,
    /// The live session's socket handshake.
    Socket,
}

/// A web request, as the app sees it (`cx.request()`).
#[derive(Clone, Default)]
pub struct WebRequest {
    /// The page or the handshake.
    pub kind: RequestKind,
    /// The page's URL as the browser shows it
    /// (`https://example.com/app/todos?tab=2`), also during the live session.
    pub url: String,
    /// The page's origin: scheme, host and port (`https://example.com`).
    pub origin: String,
    /// The request's headers, names in lower case, in order.
    pub headers: Vec<(String, String)>,
    /// What the host server attached to the request (see
    /// [`App::request_context`]).
    pub context: Option<Arc<dyn Any + Send + Sync>>,
}

impl std::fmt::Debug for WebRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebRequest")
            .field("kind", &self.kind)
            .field("url", &self.url)
            .field("origin", &self.origin)
            .field("headers", &self.headers)
            .field("context", &self.context.as_ref().map(|_| ".."))
            .finish()
    }
}

impl WebRequest {
    /// The first value of header `name` (any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The value of cookie `name`, from the `Cookie` header.
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("cookie"))
            .flat_map(|(_, v)| v.split(';'))
            .find_map(|kv| {
                let (k, v) = kv.trim().split_once('=')?;
                (k == name).then_some(v)
            })
    }

    /// The path of the page's URL (`/app/todos`).
    pub fn path(&self) -> &str {
        let rest = self.url.strip_prefix(&self.origin).unwrap_or(&self.url);
        let end = rest.find(['?', '#']).unwrap_or(rest.len());
        match &rest[..end] {
            "" => "/",
            path => path,
        }
    }

    /// The host's context, if it is a `T`.
    pub fn context<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.context.clone()?.downcast::<T>().ok()
    }
}

/// Where [`App::open_url_in`] opens a URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum UrlTarget {
    /// The web: this tab for a URL of the page's origin, a new tab otherwise.
    /// Natively: the system (browser, mail, another app).
    #[default]
    Auto,
    /// The web: this tab (the page is left). Natively: the system.
    SameTab,
    /// The web: a new tab. Natively: the system.
    NewTab,
}

impl UrlTarget {
    /// `auto`, `same` or `new` (the web client's names).
    pub fn name(self) -> &'static str {
        match self {
            UrlTarget::Auto => "auto",
            UrlTarget::SameTab => "same",
            UrlTarget::NewTab => "new",
        }
    }
}

/// The slot that holds the session's request (a global).
struct RequestSlot(WebRequest);

/// The slot that holds the page request a live session took over (a global).
struct PageRequestSlot(WebRequest);

impl App {
    /// The web request this session renders for: the page request during the
    /// server-side render, the socket handshake in the live session. `None`
    /// natively.
    pub fn request(&self) -> Option<&WebRequest> {
        self.try_global::<RequestSlot>().map(|s| &s.0)
    }

    /// What the host server attached to the request, if it is a `T`: the
    /// signed-in user, a tenant… (see `prism_web::RequestContext`). `None`
    /// natively.
    pub fn request_context<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.request()?.context::<T>()
    }

    /// The page request that drew the first paint. During that render it is
    /// [`request`](App::request); in the live session, where `request` is
    /// the socket handshake, it is the page's own request, with the context
    /// the host attached to it then: for what the host hands out once (a
    /// flash message read and cleared on the page request, a one-time
    /// token), which the handshake no longer carries.
    ///
    /// One-shot: only the first live session after a page load gets it,
    /// within a minute of the page, from the browser that loaded it. A
    /// reconnect gets `None`. `None` natively.
    pub fn page_request(&self) -> Option<&WebRequest> {
        match self.try_global::<PageRequestSlot>() {
            Some(slot) => Some(&slot.0),
            None => self.request().filter(|r| r.kind == RequestKind::Page),
        }
    }

    /// What the host attached to the [page request](App::page_request), if
    /// it is a `T`. One-shot in the live session, like `page_request`.
    /// `None` natively.
    pub fn page_request_context<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.page_request()?.context::<T>()
    }

    /// The page's origin (`https://example.com`). `None` natively: an app has
    /// no address there.
    pub fn page_origin(&self) -> Option<&str> {
        self.request().map(|r| r.origin.as_str())
    }

    /// The page's full URL (`https://example.com/app/todos?tab=2`), as it was
    /// opened. `None` natively.
    pub fn page_url(&self) -> Option<&str> {
        self.request().map(|r| r.url.as_str())
    }

    /// Answers the page request with HTTP status `code`, for a route that
    /// matches but whose data is missing (`/orders/:id` with an unknown or
    /// someone else's id: `cx.set_page_status(404)`), gone (`410`) or
    /// forbidden (`403`). Call it while the first paint renders: in the
    /// screen's build or `render`. The last call wins.
    ///
    /// Only a success (`200`–`299`) or an error (`400`–`599`) is kept; other
    /// codes are ignored (a redirect needs a `Location`: use
    /// [`navigate`](App::navigate)). The web's server-side render honours it;
    /// the live session (the response is already sent) and native apps
    /// accept it and ignore it.
    pub fn set_page_status(&mut self, code: u16) {
        if matches!(code, 200..=299 | 400..=599) {
            // Not a global: nothing re-renders, so `render` may call it.
            self.page_status = Some(code);
        }
    }

    /// The HTTP status the page request answers with: the app's
    /// [`set_page_status`](App::set_page_status), else `404` when the router
    /// shows its not-found screen, else `200`.
    pub fn page_status(&self) -> u16 {
        match self.page_status {
            Some(code) => code,
            None if self.shows_not_found() => 404,
            None => 200,
        }
    }

    /// Sets the request (web backends call it before the app's init).
    pub fn set_request(&mut self, request: WebRequest) {
        self.set_global(RequestSlot(request));
    }

    /// Sets the page request a live session took over (web backends call it
    /// before the app's init; see [`page_request`](App::page_request)).
    pub fn set_page_request(&mut self, request: WebRequest) {
        self.set_global(PageRequestSlot(request));
    }

    /// Sets (or, with [`Cookie::remove`], deletes) a cookie in the browser,
    /// from a handler running over the live socket: signing in or out, a
    /// remembered choice the host's middleware reads.
    ///
    /// The web client fetches a one-time URL of the app (same origin) whose
    /// answer carries the `Set-Cookie`, one cookie after the other; the
    /// session's [`request`](App::request) then shows the new value. The
    /// host's middleware sees it on the next handshake: call
    /// [`reconnect`](App::reconnect) after it. Natively there are no cookies
    /// (keep secrets in [`crate::security`]): the call is ignored.
    ///
    /// An invalid cookie (see [`Cookie::check`]) is ignored and fails a debug
    /// assertion; [`AsyncApp::set_cookie`](crate::AsyncApp::set_cookie)
    /// answers the outcome instead.
    pub fn set_cookie(&mut self, cookie: Cookie) {
        if let Err(e) = cookie.check() {
            debug_assert!(false, "prism: cookie {:?} refused: {e}", cookie.name());
            return;
        }
        self.device_call(DeviceCall::SetCookie(cookie));
    }

    /// Ends the live session and opens a new one at once, on the same page,
    /// without the « Reconnecting… » banner: the host's middleware sees the
    /// cookies set before it ([`set_cookie`](App::set_cookie), which the web
    /// client waits for) and the app starts again with the new handshake's
    /// [`request`](App::request) and context. The app's state is not kept:
    /// save what must survive. Native apps ignore it.
    pub fn reconnect(&mut self) {
        self.device_call(DeviceCall::Reconnect);
    }
}

impl crate::AsyncApp {
    /// [`App::set_cookie`], answered once the browser has the cookie: `Ok`
    /// then (the session's [`request`](App::request) shows it), else why
    /// not: [`DeviceError::Failed`] for an invalid cookie or a `Secure` one
    /// on a plain `http://` page (other than `localhost`, which the browser
    /// would drop), [`DeviceError::Unavailable`] natively.
    pub async fn set_cookie(&self, cookie: Cookie) -> Result<(), DeviceError> {
        cookie.check().map_err(DeviceError::Failed)?;
        match self.device(DeviceCall::SetCookie(cookie)).await? {
            DeviceValue::Done => Ok(()),
            other => Err(DeviceError::Failed(format!("unexpected answer {other:?}"))),
        }
    }

    /// Gives this browser a new id once your app signed the user in by
    /// itself, and moves its data to the new id: the old one opens nothing
    /// any more. Prism's own sign-ins ([`crate::security`]: OAuth and
    /// OpenID Connect through [`web_auth`](crate::AsyncApp::web_auth),
    /// passkeys of your relying party) do it for you.
    ///
    /// Someone who planted their id in the browser before the user signed
    /// in (session fixation) is left with an empty session: the browser's
    /// storage, cloud storage, secrets, uploads and
    /// [`data_dir`](App::data_dir) follow the new id, and the browser's
    /// other live pages reconnect with it. Call it after checking the
    /// user's credentials and before storing anything that proves the
    /// sign-in (a token, a session cookie). The browser gets the new id
    /// before any cookie set after it ([`set_cookie`](App::set_cookie)) and
    /// before a [`reconnect`](App::reconnect).
    ///
    /// # Platforms
    ///
    /// The web backend only: native apps have no browser id, so the call
    /// answers `Ok(())` at once. On the web it fails with
    /// [`DeviceError::Unavailable`] while the first paint renders (no live
    /// session yet) and with [`DeviceError::Failed`] when the data cannot
    /// move (the old id then stays, with its data). The browser's pages
    /// that another process of the server runs are not told: they show an
    /// empty session until they reload.
    pub async fn renew_browser_id(&self) -> Result<(), DeviceError> {
        if self.update(|app| app.platform()) != crate::Platform::Web {
            return Ok(());
        }
        match self.device(DeviceCall::RenewBrowserId).await? {
            DeviceValue::Done => Ok(()),
            other => Err(DeviceError::Failed(format!("unexpected answer {other:?}"))),
        }
    }
}

/// A cookie's `SameSite` attribute: when the browser sends it with requests
/// started by another site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SameSite {
    /// Only with requests from the site itself.
    Strict,
    /// Also when following a link from another site (the default).
    #[default]
    Lax,
    /// With every request, cross-site ones included (needs `Secure`).
    None,
}

impl SameSite {
    /// `Strict`, `Lax` or `None`, as written in `Set-Cookie`.
    pub fn name(self) -> &'static str {
        match self {
            SameSite::Strict => "Strict",
            SameSite::Lax => "Lax",
            SameSite::None => "None",
        }
    }
}

/// A cookie for [`App::set_cookie`]. Safe by default: `HttpOnly`, `Secure`,
/// `SameSite=Lax`, `Path=/`, and kept until the browser closes unless
/// [`max_age`](Cookie::max_age) says otherwise.
///
/// ```
/// # use prism_core::{Cookie, SameSite};
/// # use std::time::Duration;
/// let session = Cookie::new("session", "f3a9c2")
///     .max_age(Duration::from_secs(30 * 24 * 3600))
///     .same_site(SameSite::Strict);
/// assert_eq!(
///     session.to_header(),
///     "session=f3a9c2; Path=/; Max-Age=2592000; HttpOnly; Secure; SameSite=Strict"
/// );
/// let signed_out = Cookie::remove("session");
/// assert_eq!(
///     signed_out.to_header(),
///     "session=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Lax"
/// );
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cookie {
    name: String,
    value: String,
    path: String,
    domain: Option<String>,
    max_age: Option<u64>,
    http_only: bool,
    secure: bool,
    same_site: SameSite,
}

/// The size browsers keep at least, name, value and attributes together.
const MAX_COOKIE: usize = 4096;

impl Cookie {
    /// Cookie `name` with `value`, and the safe defaults.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Cookie {
            name: name.into(),
            value: value.into(),
            path: "/".into(),
            domain: None,
            max_age: None,
            http_only: true,
            secure: true,
            same_site: SameSite::Lax,
        }
    }

    /// Deletes cookie `name`: an empty value that expires at once. Give it
    /// the [`path`](Cookie::path) and [`domain`](Cookie::domain) the cookie
    /// was set with.
    pub fn remove(name: impl Into<String>) -> Self {
        Cookie {
            max_age: Some(0),
            ..Cookie::new(name, "")
        }
    }

    /// The paths it is sent with (`/` by default: the whole site).
    #[must_use]
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// Also sent to the subdomains of `domain` (by default, only to the
    /// page's own host).
    #[must_use]
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Kept for `age` (whole seconds), across browser restarts.
    #[must_use]
    pub fn max_age(mut self, age: std::time::Duration) -> Self {
        self.max_age = Some(age.as_secs());
        self
    }

    /// Hidden from the page's scripts (`true` by default).
    #[must_use]
    pub fn http_only(mut self, on: bool) -> Self {
        self.http_only = on;
        self
    }

    /// Sent over `https` only (`true` by default; browsers also accept it on
    /// `http://localhost`).
    #[must_use]
    pub fn secure(mut self, on: bool) -> Self {
        self.secure = on;
        self
    }

    /// When cross-site requests carry it ([`SameSite::Lax`] by default).
    #[must_use]
    pub fn same_site(mut self, same_site: SameSite) -> Self {
        self.same_site = same_site;
        self
    }

    /// Its name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its value (empty for [`remove`](Cookie::remove)).
    pub fn value(&self) -> &str {
        &self.value
    }

    /// The domain it was given with [`domain`](Cookie::domain), if any.
    pub fn domain_value(&self) -> Option<&str> {
        self.domain.as_deref()
    }

    /// Whether it deletes the cookie (expires at once).
    pub fn is_removal(&self) -> bool {
        self.max_age == Some(0)
    }

    /// Whether it is `Secure`.
    pub fn is_secure(&self) -> bool {
        self.secure
    }

    /// Why a browser would refuse it, or a header could not carry it: a
    /// name that is not a token (RFC 6265), a value with spaces, quotes,
    /// commas, semicolons or backslashes (encode it: base64url, hex), a path
    /// or domain with `;` or control characters, `SameSite=None` without
    /// `Secure`, a `__Secure-` name without `Secure`, a `__Host-` name
    /// without `Secure`, with a domain or another path than `/`, or more
    /// than 4096 bytes.
    pub fn check(&self) -> Result<(), String> {
        let token = |c: char| c.is_ascii_graphic() && !"()<>@,;:\\\"/[]?={}".contains(c);
        if self.name.is_empty() || !self.name.chars().all(token) {
            return Err(format!("{:?} is not a cookie name", self.name));
        }
        let octet = |c: char| c.is_ascii_graphic() && !matches!(c, '"' | ',' | ';' | '\\');
        if !self.value.chars().all(octet) {
            return Err(format!(
                "the value of {:?} has a character a cookie cannot hold (encode it)",
                self.name
            ));
        }
        let attribute = |s: &str| !s.chars().any(|c| c == ';' || c.is_ascii_control());
        if !self.path.starts_with('/') || !attribute(&self.path) {
            return Err(format!("{:?} is not a cookie path", self.path));
        }
        if let Some(domain) = &self.domain {
            let ok = !domain.is_empty()
                && domain
                    .trim_start_matches('.')
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
            if !ok {
                return Err(format!("{domain:?} is not a cookie domain"));
            }
        }
        if self.same_site == SameSite::None && !self.secure {
            return Err("SameSite=None needs Secure".into());
        }
        let lower = self.name.to_ascii_lowercase();
        if lower.starts_with("__secure-") && !self.secure {
            return Err(format!("{:?} needs Secure", self.name));
        }
        if lower.starts_with("__host-")
            && (!self.secure || self.domain.is_some() || self.path != "/")
        {
            return Err(format!(
                "{:?} needs Secure, Path=/ and no Domain",
                self.name
            ));
        }
        if self.to_header().len() > MAX_COOKIE {
            return Err(format!("{:?} is larger than {MAX_COOKIE} bytes", self.name));
        }
        Ok(())
    }

    /// The `Set-Cookie` header's value (check it first with
    /// [`check`](Cookie::check)).
    pub fn to_header(&self) -> String {
        use std::fmt::Write;
        let mut h = format!("{}={}; Path={}", self.name, self.value, self.path);
        if let Some(domain) = &self.domain {
            let _ = write!(h, "; Domain={domain}");
        }
        if let Some(age) = self.max_age {
            let _ = write!(h, "; Max-Age={age}");
        }
        if self.http_only {
            h.push_str("; HttpOnly");
        }
        if self.secure {
            h.push_str("; Secure");
        }
        let _ = write!(h, "; SameSite={}", self.same_site.name());
        h
    }
}

impl WebRequest {
    /// Applies a cookie the browser now has to this request's `Cookie`
    /// header (web backends, once [`App::set_cookie`] succeeded): replaced,
    /// added at the end, or removed.
    pub fn apply_cookie(&mut self, cookie: &Cookie) {
        let mut pairs: Vec<String> = self
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("cookie"))
            .flat_map(|(_, v)| v.split(';'))
            .map(str::trim)
            .filter(|kv| !kv.is_empty() && kv.split('=').next() != Some(cookie.name()))
            .map(str::to_string)
            .collect();
        if !cookie.is_removal() {
            pairs.push(format!("{}={}", cookie.name(), cookie.value()));
        }
        let at = self
            .headers
            .iter()
            .position(|(k, _)| k.eq_ignore_ascii_case("cookie"));
        self.headers
            .retain(|(k, _)| !k.eq_ignore_ascii_case("cookie"));
        if !pairs.is_empty() {
            let at = at.unwrap_or(self.headers.len()).min(self.headers.len());
            self.headers.insert(at, ("cookie".into(), pairs.join("; ")));
        }
    }
}
