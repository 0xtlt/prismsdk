//! The web request a session was opened with: its URL, origin, headers and
//! cookies, plus whatever the host server attached to it (the signed-in user).
//!
//! On the web, the first paint renders with the page request; the live
//! session renders with the socket handshake (re-read at every reconnect).
//! Native apps have no request: [`App::request`] and [`App::page_origin`]
//! are `None` there.

use std::any::Any;
use std::sync::Arc;

use crate::app::App;

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

    /// Sets the request (web backends call it before the app's init).
    pub fn set_request(&mut self, request: WebRequest) {
        self.set_global(RequestSlot(request));
    }
}
