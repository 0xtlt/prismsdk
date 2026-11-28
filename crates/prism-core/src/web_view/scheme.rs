//! App schemes (#343): a web view's page, its fetches, stylesheets, fonts
//! and pictures served by the app itself, from Rust, without a local
//! server.
//!
//! The app registers a scheme with its handler
//! (`Application::web_scheme("app", |req| …)`, which calls
//! [`register_web_scheme`]); `web_view("app://main/index.html")` then loads
//! from it, and so does every URL of that page: relative ones, `fetch()`,
//! XHR, CSS, fonts and images. Each request runs the handler on a
//! background thread (never the UI's); a handler that panics answers 500.
//!
//! How each platform serves it:
//!
//! | Platform | How |
//! |---|---|
//! | macOS, iOS, iPadOS, visionOS | a `WKURLSchemeHandler`: the page is at `app://main/…` |
//! | Windows | a WebView2 custom scheme (`CoreWebView2CustomSchemeRegistration`): the page is at `app://main/…` |
//! | Linux (prismsdk's `web-view` feature) | a URI scheme of WebKitGTK's web context (`webkit_web_context_register_uri_scheme`), secure and CORS-enabled: the page is at `app://main/…` |
//! | Android | `shouldInterceptRequest`: Chromium's fetch only knows web schemes, so the page is at `https://main.app.prism.invalid/…` ([`engine_url`]), a host that never exists (RFC 6761) and that the web view always answers itself |
//! | Web | not by the handler: `WebApp::web_scheme_path("app", "/canvas")` loads `/canvas/main/…`, which the app serves with its own routes |
//! | tvOS, watchOS, Linux without the feature | no web view |
//!
//! The app always sees the URL as it wrote it (`app://main/…`): in
//! [`SchemeRequest::url`], [`WebLoadEvent::url`](super::WebLoadEvent::url) and
//! [`WebMessageEvent::origin`](super::WebMessageEvent::origin) (`app://main`),
//! on every platform. The default allowed origin of a web view at
//! `app://main/…` is `app://main`, so its page talks to the app with
//! `prismHost` as any other.
//!
//! **Who asks**: every frame inside the web view can request the scheme,
//! an embedded third-party frame too. [`SchemeRequest::origin`] says which
//! page asked when the engine tells (`Origin`, else `Referer`): a handler
//! that hands out private data checks it. No CORS header is added: a page
//! of another origin cannot read the answers unless the handler allows it
//! (`Access-Control-Allow-Origin`).

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};

use super::origin_of;

/// A request a web view's page made to an app scheme ([`register_web_scheme`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SchemeRequest {
    /// `GET`, `POST`… (upper-case).
    pub method: String,
    /// The URL as the app writes it: `app://main/data.json?page=2`, on
    /// every platform.
    pub url: String,
    /// The request's headers, as the engine sent them.
    pub headers: Vec<(String, String)>,
    /// The body (a `fetch` POST's). Empty on Android: its web view does
    /// not give request bodies to the app.
    pub body: Vec<u8>,
}

impl SchemeRequest {
    /// A request for `url` (tests, and backends building one).
    pub fn new(method: impl Into<String>, url: impl Into<String>) -> Self {
        SchemeRequest {
            method: method.into().to_ascii_uppercase(),
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Adds a header.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Sets the body.
    pub fn with_body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    /// The scheme, lower-case (`app`).
    pub fn scheme(&self) -> String {
        split(&self.url).map_or_else(String::new, |p| p.scheme.to_ascii_lowercase())
    }

    /// The host, lower-case (`main` in `app://main/index.html`).
    pub fn host(&self) -> String {
        split(&self.url).map_or_else(String::new, |p| p.authority.to_ascii_lowercase())
    }

    /// The path, decoded (`/fonts/My Font.woff2`; `/` when the URL has
    /// none), never above `/`: its `.` and `..` segments are resolved here
    /// (escaped too: `%2E%2E`), whatever the engine did, and an escaped
    /// `/`, `\` or NUL stays escaped (`%2F`), so decoding never makes a new
    /// segment. `app://main/a/../b` is `/b`, `app://main/..%2Fsecret` is
    /// `/..%2Fsecret`. To map a path to files, take
    /// [`path_segments`](Self::path_segments).
    pub fn path(&self) -> String {
        let raw = split(&self.url).map_or("", |p| p.path);
        if raw.is_empty() {
            return "/".to_string();
        }
        let parts: Vec<&str> = raw.split('/').skip(1).collect();
        let mut kept: Vec<&str> = Vec::new();
        for (i, seg) in parts.iter().enumerate() {
            let last = i + 1 == parts.len();
            match dot_segment(seg) {
                0 => kept.push(seg),
                dots => {
                    if dots == 2 {
                        kept.pop();
                    }
                    // `/a/..` is the directory `/`, with its slash.
                    if last {
                        kept.push("");
                    }
                }
            }
        }
        percent_decode_path(&format!("/{}", kept.join("/")))
    }

    /// The path's segments, decoded, for a lookup in a folder or a map:
    /// `app://main/fonts/My%20Font.woff2` gives `["fonts", "My Font.woff2"]`
    /// (empty segments left out). `None` if a segment is `.` or `..`
    /// (escaped or not), or decodes to text with a `/`, a `\` or a NUL:
    /// such a path could leave the folder, so answer it 404.
    pub fn path_segments(&self) -> Option<Vec<String>> {
        let raw = split(&self.url).map_or("", |p| p.path);
        raw.split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                let d = percent_decode(s, false);
                (d != "." && d != ".." && !d.contains(['/', '\\', '\0'])).then_some(d)
            })
            .collect()
    }

    /// The query, still encoded (`page=2&q=a%20b`), if any.
    pub fn query(&self) -> Option<&str> {
        split(&self.url).and_then(|p| p.query)
    }

    /// The first value of query parameter `name`, decoded (`+` is a space).
    pub fn query_param(&self, name: &str) -> Option<String> {
        self.query()?.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (percent_decode(k, true) == name).then(|| percent_decode(v, true))
        })
    }

    /// The first header named `name` (any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The origin of the page that asked, when the engine tells: its
    /// `Origin` header (`app://main`, `https://ads.example`, `null` for a
    /// sandboxed frame), else the origin of its `Referer`. `None`: not
    /// told (a page the user typed, a navigation without a referrer).
    ///
    /// Any frame inside the web view can request the scheme, a
    /// third-party one included: before answering with private data,
    /// check that it is the app's own page (`Some("app://main")`).
    pub fn origin(&self) -> Option<String> {
        if let Some(o) = self
            .header("origin")
            .map(str::trim)
            .filter(|o| !o.is_empty())
        {
            return Some(origin_of(o).unwrap_or_else(|| o.to_ascii_lowercase()));
        }
        origin_of(self.header("referer")?.trim())
    }
}

/// The app's answer to a [`SchemeRequest`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SchemeResponse {
    /// The HTTP status (`200`, `404`…).
    pub status: u16,
    /// The `Content-Type` (`text/html; charset=utf-8`); empty: none.
    pub content_type: String,
    /// More headers (`Cache-Control`…). Names and values with line breaks
    /// are dropped; `Content-Length` is the platform's.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

impl Default for SchemeResponse {
    fn default() -> Self {
        SchemeResponse::new(200)
    }
}

impl SchemeResponse {
    /// An empty answer with `status`.
    pub fn new(status: u16) -> Self {
        SchemeResponse {
            status,
            content_type: String::new(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// `200` with `body` of type `content_type`.
    pub fn ok(content_type: impl Into<String>, body: impl Into<Vec<u8>>) -> Self {
        SchemeResponse {
            content_type: content_type.into(),
            body: body.into(),
            ..SchemeResponse::new(200)
        }
    }

    /// `200`, an HTML page.
    pub fn html(body: impl Into<String>) -> Self {
        Self::ok("text/html; charset=utf-8", body.into())
    }

    /// `200`, plain text.
    pub fn text(body: impl Into<String>) -> Self {
        Self::ok("text/plain; charset=utf-8", body.into())
    }

    /// `200`, `value` as JSON (`500` if it does not serialize).
    pub fn json<T: serde::Serialize + ?Sized>(value: &T) -> Self {
        match serde_json::to_vec(value) {
            Ok(body) => Self::ok("application/json", body),
            Err(_) => Self::new(500),
        }
    }

    /// `200`, a file's bytes, typed from its name's extension
    /// ([`content_type_of`]): `SchemeResponse::file("fonts/Inter.woff2", bytes)`.
    pub fn file(name: &str, body: impl Into<Vec<u8>>) -> Self {
        Self::ok(content_type_of(name), body)
    }

    /// `404 Not Found`.
    pub fn not_found() -> Self {
        SchemeResponse {
            content_type: "text/plain; charset=utf-8".into(),
            body: b"Not Found".to_vec(),
            ..SchemeResponse::new(404)
        }
    }

    /// The same answer with another status.
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    /// Adds a header (`Cache-Control: no-store`).
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// The status's reason phrase (`Not Found`; `OK` for one it does not
    /// know).
    pub fn reason(&self) -> &'static str {
        reason_phrase(self.status)
    }

    /// The headers a platform sends: `Content-Type` first (from
    /// [`content_type`](Self::content_type), else the handler's own), then
    /// the others; without `Content-Length` (the platform's), and without
    /// any name or value that is not a valid header.
    pub fn header_list(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let valid_name = |n: &str| {
            !n.is_empty()
                && n.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        };
        let valid_value = |v: &str| !v.chars().any(|c| matches!(c, '\r' | '\n' | '\0'));
        let typed = !self.content_type.is_empty() && valid_value(&self.content_type);
        if typed {
            out.push(("Content-Type".to_string(), self.content_type.clone()));
        }
        for (name, value) in &self.headers {
            let lower = name.to_ascii_lowercase();
            if !valid_name(name)
                || !valid_value(value)
                || lower == "content-length"
                || (typed && lower == "content-type")
            {
                continue;
            }
            out.push((name.clone(), value.trim().to_string()));
        }
        out
    }

    /// The head as one block for a native bridge: the status, then one
    /// `Name: value` line per header ([`header_list`](Self::header_list)).
    pub fn encode_head(&self) -> String {
        let mut out = self.status.to_string();
        for (name, value) in self.header_list() {
            out.push('\n');
            out.push_str(&name);
            out.push_str(": ");
            out.push_str(&value);
        }
        out
    }
}

/// The reason phrase of an HTTP status (`OK` for one it does not know).
pub fn reason_phrase(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "OK",
    }
}

/// The content type of a file named `name`, from its extension
/// (`application/octet-stream` for one it does not know).
pub fn content_type_of(name: &str) -> &'static str {
    let ext = name
        .rsplit('/')
        .next()
        .and_then(|n| n.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

type Handler = Arc<dyn Fn(&SchemeRequest) -> SchemeResponse + Send + Sync>;

fn registry() -> &'static RwLock<BTreeMap<String, Handler>> {
    static SCHEMES: OnceLock<RwLock<BTreeMap<String, Handler>>> = OnceLock::new();
    SCHEMES.get_or_init(Default::default)
}

/// Schemes the engines (or the systems around them) handle themselves:
/// never an app's.
const ENGINE_SCHEMES: &[&str] = &[
    "about",
    "android-app",
    "applewebdata",
    "blob",
    "chrome",
    "chrome-extension",
    "chrome-untrusted",
    "content",
    "data",
    "devtools",
    "edge",
    "file",
    "filesystem",
    "ftp",
    "ftps",
    "http",
    "https",
    "intent",
    "javascript",
    "mailto",
    "sms",
    "tel",
    "view-source",
    "webkit",
    "ws",
    "wss",
];

/// Whether `scheme` may be an app scheme: a lower-case letter, then
/// lower-case letters, digits or `-` (RFC 3986 schemes, lower-case,
/// without `+` and `.` so they also name a host on Android), at most 63
/// characters, not ending with `-`, and none the engines own (`http`,
/// `https`, `file`, `data`, `blob`, `about`, `javascript`, `ws`, `wss`,
/// `ftp`, `mailto`…). The error says why.
pub fn check_web_scheme(scheme: &str) -> Result<(), String> {
    let first = scheme.chars().next();
    if !first.is_some_and(|c| c.is_ascii_lowercase()) {
        return Err(format!(
            "\"{scheme}\" is not a scheme name: it starts with a lower-case letter (a-z)"
        ));
    }
    if let Some(c) = scheme
        .chars()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-'))
    {
        return Err(format!(
            "\"{scheme}\" is not a scheme name an app may serve: {c:?} (only a-z, 0-9 and -)"
        ));
    }
    if scheme.len() > 63 || scheme.ends_with('-') {
        return Err(format!(
            "\"{scheme}\" is not a scheme name an app may serve: at most 63 characters, not \
             ending with -"
        ));
    }
    if ENGINE_SCHEMES.contains(&scheme) || scheme.starts_with("x-apple") {
        return Err(format!(
            "\"{scheme}\" is a scheme the web engines handle themselves: pick the app's own \
             (\"app\")"
        ));
    }
    Ok(())
}

/// Serves `scheme` with `handler` in every web view of the app (see the
/// [module docs](self)); a handler registered before for it is replaced.
/// Apps call it through `Application::web_scheme`, before the first web
/// view.
///
/// # Panics
///
/// If [`check_web_scheme`] refuses the name: a programmer error, found at
/// startup.
pub fn register_web_scheme(
    scheme: &str,
    handler: impl Fn(&SchemeRequest) -> SchemeResponse + Send + Sync + 'static,
) {
    if let Err(why) = check_web_scheme(scheme) {
        panic!("prism: web_scheme: {why}");
    }
    registry()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(scheme.to_string(), Arc::new(handler));
}

/// The app's schemes, sorted (`["app"]`).
pub fn web_schemes() -> Vec<String> {
    registry()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .cloned()
        .collect()
}

/// Whether the app serves `scheme` (lower-case).
pub fn is_web_scheme(scheme: &str) -> bool {
    registry()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(scheme)
}

fn handler_of(scheme: &str) -> Option<Handler> {
    registry()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(scheme)
        .cloned()
}

/// Answers `request` on the calling thread with its scheme's handler:
/// `404` if the app serves no such scheme, `500` if the handler panics
/// (the app goes on). Backends call it off the UI thread.
pub fn respond_web_request(request: &SchemeRequest) -> SchemeResponse {
    let scheme = request.scheme();
    let Some(handler) = handler_of(&scheme) else {
        return SchemeResponse::not_found();
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(request))) {
        Ok(response) => response,
        Err(_) => {
            eprintln!(
                "prism: the {scheme}: handler panicked on {} {}: answered 500",
                request.method, request.url
            );
            SchemeResponse {
                content_type: "text/plain; charset=utf-8".into(),
                body: b"Internal Server Error".to_vec(),
                ..SchemeResponse::new(500)
            }
        }
    }
}

/// [`respond_web_request`] on a background thread (the pool of
/// `cx.background`), then `done` with the answer, on that thread.
pub fn serve_web_request(
    request: SchemeRequest,
    done: impl FnOnce(SchemeResponse) + Send + 'static,
) {
    crate::task::run_in_background(move || done(respond_web_request(&request)));
}

/// Header lines (`Name: value`, one per line) as pairs: how native
/// bridges hand a request's headers over.
pub fn decode_header_lines(lines: &str) -> Vec<(String, String)> {
    lines
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            let k = k.trim();
            (!k.is_empty()).then(|| (k.to_string(), v.trim().to_string()))
        })
        .collect()
}

/// The domain under which app schemes are served where the engine only
/// fetches web schemes (Android): `prism.invalid`. `.invalid` is reserved
/// (RFC 6761): no such host ever exists, and the web view answers it
/// itself.
pub const ENGINE_DOMAIN: &str = "prism.invalid";

/// The `https:` URL standing for an app scheme's URL where the engine only
/// fetches web schemes (Android, see the [module docs](self)):
/// `app://main/a.css?v=1` → `https://main.app.prism.invalid/a.css?v=1`.
/// `None` for a URL of another scheme (`https:`), or one this cannot
/// stand for: no host, a port, user info, or a host that is not letters,
/// digits, `-` and `.`.
pub fn engine_url(url: &str) -> Option<String> {
    let p = split(url)?;
    let scheme = p.scheme.to_ascii_lowercase();
    check_web_scheme(&scheme).ok()?;
    let host = p.authority.to_ascii_lowercase();
    let label_ok = |l: &str| {
        !l.is_empty() && l.len() <= 63 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if !host.split('.').all(label_ok) || host.len() + scheme.len() + ENGINE_DOMAIN.len() > 250 {
        return None;
    }
    Some(format!(
        "https://{host}.{scheme}.{ENGINE_DOMAIN}{}",
        &url[p.rest_start..]
    ))
}

/// The app scheme's URL an [`engine_url`] stands for:
/// `https://main.app.prism.invalid/a.css` → `app://main/a.css`, and an
/// origin as well (`https://main.app.prism.invalid` → `app://main`).
/// `None` for any other URL.
pub fn app_url(url: &str) -> Option<String> {
    let p = split(url)?;
    if !p.scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = p.authority.to_ascii_lowercase();
    let rest = authority.strip_suffix(ENGINE_DOMAIN)?.strip_suffix('.')?;
    let (host, scheme) = rest.rsplit_once('.')?;
    if host.is_empty() || check_web_scheme(scheme).is_err() {
        return None;
    }
    Some(format!("{scheme}://{host}{}", &url[p.rest_start..]))
}

/// A URL's parts: `scheme://authority` then the path, query and fragment.
struct Parts<'a> {
    scheme: &'a str,
    authority: &'a str,
    path: &'a str,
    query: Option<&'a str>,
    /// Where the path starts in the URL.
    rest_start: usize,
}

fn split(url: &str) -> Option<Parts<'_>> {
    let (scheme, after) = url.split_once("://")?;
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = &after[..end];
    if authority.is_empty() || authority.contains(['@', ':']) {
        return None;
    }
    let rest_start = scheme.len() + 3 + end;
    let rest = &url[rest_start..];
    let rest = rest.split('#').next().unwrap_or("");
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    Some(Parts {
        scheme,
        authority,
        path,
        query,
        rest_start,
    })
}

/// Whether a raw path segment is `.` (1) or `..` (2), escaped or not, as
/// URL parsers read them; 0 otherwise.
fn dot_segment(seg: &str) -> u8 {
    match seg.to_ascii_lowercase().as_str() {
        "." | "%2e" => 1,
        ".." | ".%2e" | "%2e." | "%2e%2e" => 2,
        _ => 0,
    }
}

/// Decodes a path's `%XX` escapes, but those of `/`, `\\` and NUL, which
/// stay escaped: decoding never adds a segment or ends a string.
fn percent_decode_path(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (
                bytes.get(i + 1).and_then(|&b| hex(b)),
                bytes.get(i + 2).and_then(|&b| hex(b)),
            )
        {
            let byte = (h * 16 + l) as u8;
            if matches!(byte, b'/' | b'\\' | 0) {
                out.extend_from_slice(&bytes[i..i + 3]);
            } else {
                out.push(byte);
            }
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Decodes `%XX` escapes (and `+` as a space in a query).
fn percent_decode(s: &str, plus: bool) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b'+' if plus => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_names_are_lower_case_and_never_an_engine_s() {
        for ok in ["app", "my-app", "x2", "prism-docs"] {
            assert_eq!(check_web_scheme(ok), Ok(()), "{ok}");
        }
        for bad in [
            "",
            "App",
            "APP",
            "1app",
            "-app",
            "app-",
            "my.app",
            "my+app",
            "my_app",
            "a b",
            "é",
            "http",
            "https",
            "file",
            "data",
            "blob",
            "about",
            "javascript",
            "ws",
            "wss",
            "ftp",
            "mailto",
            "intent",
            "content",
            "chrome",
            "edge",
            "x-apple-data-detectors",
        ] {
            assert!(check_web_scheme(bad).is_err(), "{bad:?}");
        }
        assert!(check_web_scheme(&"a".repeat(64)).is_err());
        assert!(check_web_scheme(&"a".repeat(63)).is_ok());
    }

    #[test]
    #[should_panic(expected = "prism: web_scheme: \"https\" is a scheme the web engines handle")]
    fn registering_an_engine_scheme_fails_loudly() {
        register_web_scheme("https", |_| SchemeResponse::not_found());
    }

    #[test]
    #[should_panic(expected = "starts with a lower-case letter")]
    fn registering_an_upper_case_scheme_fails_loudly() {
        register_web_scheme("App", |_| SchemeResponse::not_found());
    }

    #[test]
    fn a_request_reads_its_url_headers_and_origin() {
        let r = SchemeRequest::new("post", "app://Main/fonts/My%20Font.woff2?v=1&q=a+b%26c#top")
            .with_header("Origin", "app://main")
            .with_header("Content-Type", "application/json")
            .with_body(b"{}".to_vec());
        assert_eq!(r.method, "POST");
        assert_eq!(r.scheme(), "app");
        assert_eq!(r.host(), "main");
        assert_eq!(r.path(), "/fonts/My Font.woff2");
        assert_eq!(r.query(), Some("v=1&q=a+b%26c"));
        assert_eq!(r.query_param("q").as_deref(), Some("a b&c"));
        assert_eq!(r.query_param("v").as_deref(), Some("1"));
        assert_eq!(r.query_param("none"), None);
        assert_eq!(r.header("content-type"), Some("application/json"));
        assert_eq!(r.origin().as_deref(), Some("app://main"));
        assert_eq!(SchemeRequest::new("GET", "app://main").path(), "/");
        assert_eq!(
            r.path_segments(),
            Some(vec!["fonts".to_string(), "My Font.woff2".to_string()])
        );
        assert_eq!(SchemeRequest::new("GET", "app://main?x").path(), "/");
        // The Referer's origin when there is no Origin; `null` stays.
        let referred = SchemeRequest::new("GET", "app://main/a.png")
            .with_header("Referer", "https://ads.example:443/frame.html?id=1");
        assert_eq!(referred.origin().as_deref(), Some("https://ads.example"));
        let sandboxed = SchemeRequest::new("GET", "app://main/a").with_header("Origin", "null");
        assert_eq!(sandboxed.origin().as_deref(), Some("null"));
        assert_eq!(SchemeRequest::new("GET", "app://main/").origin(), None);
    }

    #[test]
    fn a_path_never_leaves_the_root() {
        let path = |u: &str| SchemeRequest::new("GET", u).path();
        let segments = |u: &str| SchemeRequest::new("GET", u).path_segments();
        // Dot segments, escaped or not, resolve inside `/`.
        assert_eq!(path("app://main/a/../b"), "/b");
        assert_eq!(path("app://main/../../secret"), "/secret");
        assert_eq!(path("app://main/%2e%2E/%2E./.%2e/secret"), "/secret");
        assert_eq!(path("app://main/a/./b/."), "/a/b/");
        assert_eq!(path("app://main/a/.."), "/");
        assert_eq!(path("app://main/a/"), "/a/");
        assert_eq!(path("app://main/"), "/");
        // An escaped separator or NUL stays escaped: no new segment.
        assert_eq!(path("app://main/..%2F..%2Fsecret"), "/..%2F..%2Fsecret");
        assert_eq!(path("app://main/a%5Cb%00c"), "/a%5Cb%00c");
        assert_eq!(path("app://main/caf%C3%A9%2fx%20y"), "/café%2fx y");
        assert_eq!(path("app://main/100%"), "/100%");
        // Segments refuse anything that could leave a folder.
        assert_eq!(
            segments("app://main//a/b%20c/"),
            Some(vec!["a".to_string(), "b c".to_string()])
        );
        assert_eq!(segments("app://main/"), Some(vec![]));
        for bad in [
            "app://main/../secret",
            "app://main/a/./b",
            "app://main/%2e%2e/secret",
            "app://main/..%2Fsecret",
            "app://main/a%5Cb",
            "app://main/a%00b",
        ] {
            assert_eq!(segments(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_response_sends_valid_headers_only() {
        let r = SchemeResponse::html("<p>hi</p>")
            .with_header("Cache-Control", "no-store")
            .with_header("Content-Type", "text/plain")
            .with_header("Content-Length", "1")
            .with_header("X-Bad", "a\r\nSet-Cookie: x=1")
            .with_header("Bad Name", "v");
        assert_eq!(
            r.header_list(),
            [
                ("Content-Type".into(), "text/html; charset=utf-8".into()),
                ("Cache-Control".into(), "no-store".into()),
            ]
        );
        assert_eq!(
            r.encode_head(),
            "200\nContent-Type: text/html; charset=utf-8\nCache-Control: no-store"
        );
        // Without a content type, the handler's own.
        let own = SchemeResponse::new(201).with_header("content-type", "text/csv");
        assert_eq!(own.encode_head(), "201\ncontent-type: text/csv");
        assert_eq!(own.reason(), "Created");
        assert_eq!(SchemeResponse::not_found().reason(), "Not Found");
        assert_eq!(SchemeResponse::json(&[1, 2]).body, b"[1,2]");
        assert_eq!(
            SchemeResponse::file("fonts/Inter.WOFF2", vec![0]).content_type,
            "font/woff2"
        );
        assert_eq!(content_type_of("a/b"), "application/octet-stream");
        assert_eq!(content_type_of("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            decode_header_lines("Accept: */*\nOrigin: app://main\nbad\n: x\n"),
            [
                ("Accept".to_string(), "*/*".to_string()),
                ("Origin".to_string(), "app://main".to_string())
            ]
        );
    }

    #[test]
    fn a_handler_answers_off_the_calling_thread_and_a_panic_is_a_500() {
        register_web_scheme("test-serve", |r| {
            if r.path() == "/boom" {
                panic!("boom (expected by the test)");
            }
            SchemeResponse::text(format!("{} {}", r.method, r.path()))
        });
        assert!(is_web_scheme("test-serve"));
        assert!(web_schemes().contains(&"test-serve".to_string()));
        let ok = respond_web_request(&SchemeRequest::new("GET", "test-serve://main/a"));
        assert_eq!((ok.status, ok.body.as_slice()), (200, &b"GET /a"[..]));
        let boom = respond_web_request(&SchemeRequest::new("GET", "test-serve://main/boom"));
        assert_eq!(boom.status, 500);
        let none = respond_web_request(&SchemeRequest::new("GET", "test-none://main/a"));
        assert_eq!(none.status, 404);
        // On a background thread, then `done` there.
        let (tx, rx) = std::sync::mpsc::channel();
        let caller = std::thread::current().id();
        serve_web_request(
            SchemeRequest::new("GET", "test-serve://main/boom"),
            move |r| {
                let _ = tx.send((r.status, std::thread::current().id()));
            },
        );
        let (status, thread) = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert_eq!(status, 500);
        assert_ne!(thread, caller);
    }

    #[test]
    fn engine_urls_map_both_ways() {
        let pairs = [
            (
                "app://main/index.html",
                "https://main.app.prism.invalid/index.html",
            ),
            ("app://main", "https://main.app.prism.invalid"),
            (
                "app://Main/a/b.css?v=1#x",
                "https://main.app.prism.invalid/a/b.css?v=1#x",
            ),
            ("my-app://docs.v2/", "https://docs.v2.my-app.prism.invalid/"),
            ("app://main?q", "https://main.app.prism.invalid?q"),
        ];
        for (app, engine) in pairs {
            assert_eq!(engine_url(app).as_deref(), Some(engine), "{app}");
            assert_eq!(
                app_url(engine).as_deref(),
                Some(app.replace("Main", "main").as_str()),
                "{engine}"
            );
        }
        // Origins too.
        assert_eq!(
            app_url("https://main.app.prism.invalid").as_deref(),
            Some("app://main")
        );
        assert_eq!(
            origin_of(&engine_url("app://main/x").unwrap()).as_deref(),
            Some("https://main.app.prism.invalid")
        );
        // Nothing else maps.
        for url in [
            "https://example.com/",
            "http://main.app.prism.invalid/",
            "https://app.prism.invalid/",
            "https://main.http.prism.invalid/",
            "https://prism.invalid/",
            "https://evil.example/main.app.prism.invalid",
            "file:///tmp/a.html",
        ] {
            assert_eq!(app_url(url), None, "{url}");
        }
        for url in [
            "https://example.com/",
            "file:///tmp/a.html",
            "app:///index.html",
            "app://main:8080/",
            "app://user@main/",
            "app://ma_in/",
            "app://a..b/",
            "App+x://main/",
            "javascript://main/",
        ] {
            assert_eq!(engine_url(url), None, "{url}");
        }
    }

    #[test]
    fn percent_decoding_keeps_broken_escapes() {
        assert_eq!(percent_decode("a%2", false), "a%2");
        assert_eq!(percent_decode("a%zz", false), "a%zz");
        assert_eq!(percent_decode("%41%", false), "A%");
        assert_eq!(percent_decode("a+b", false), "a+b");
        assert_eq!(percent_decode("a+b", true), "a b");
    }
}
