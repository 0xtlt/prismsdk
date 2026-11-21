//! HTTP for Prism apps. Requests run on a background thread (rustls, no
//! system TLS dependency) and resolve on the UI thread:
//!
//! ```ignore
//! use prism::http::{Fetch, Request};
//! cx.spawn(async move |this, cx| {
//!     let res = cx.fetch(Request::get("https://api.example.com/todos")).await;
//!     cx.update_entity(&this, |t, _| t.status = res.map(|r| r.text()).ok());
//! });
//! ```

pub mod oauth;

use std::io::Read;
use std::time::Duration;

use prism_core::{AsyncApp, Background};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// Which redirects a [`Request`] follows (#250).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redirects {
    /// None: a redirect is the response.
    None,
    /// Only to the same origin (scheme, host, port); a redirect elsewhere
    /// is the response.
    SameOrigin,
    /// Up to this many (10 by default), dropping secret headers when one
    /// leaves the origin.
    Follow(u8),
}

impl Default for Redirects {
    fn default() -> Self {
        Redirects::Follow(10)
    }
}

/// An HTTP request. `Debug` shows no credential: header values only for
/// harmless headers (`Content-Type`, `Accept`…), the body's size, the URL
/// without its query (S34).
#[derive(Clone)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
    /// Which redirects it follows (#250).
    pub redirects: Redirects,
    /// Headers (lowercase names) never sent to another origin than the
    /// request's own, besides `Authorization`, `Proxy-Authorization` and
    /// `Cookie` (#250).
    pub secret_headers: Vec<String>,
    /// The largest response body accepted, in bytes (#250).
    pub max_body: Option<u64>,
    /// Connect only to publicly routable addresses ([`Request::public_only`]).
    pub public_only: bool,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("url", &shown_url(&self.url))
            .field("headers", &ShownHeaders(&self.headers))
            .field("body", &self.body.as_ref().map(|b| Size(b.len())))
            .field("timeout", &self.timeout)
            .field("redirects", &self.redirects)
            .field("secret_headers", &self.secret_headers)
            .field("max_body", &self.max_body)
            .field("public_only", &self.public_only)
            .finish()
    }
}

// --- Debug output without credentials (S34) -----------------------------------------------

/// Headers whose values `Debug` shows: they carry no credential. The
/// others show as `<redacted>`.
const SHOWN_HEADERS: &[&str] = &[
    "accept",
    "accept-encoding",
    "accept-language",
    "accept-ranges",
    "age",
    "cache-control",
    "connection",
    "content-encoding",
    "content-language",
    "content-length",
    "content-range",
    "content-type",
    "date",
    "etag",
    "expires",
    "host",
    "last-modified",
    "range",
    "retry-after",
    "server",
    "transfer-encoding",
    "user-agent",
    "vary",
];

/// Headers as `Debug` shows them: every name, the values of
/// [`SHOWN_HEADERS`].
struct ShownHeaders<'a>(&'a [(String, String)]);

impl std::fmt::Debug for ShownHeaders<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|(name, value)| {
                let shown = SHOWN_HEADERS.iter().any(|h| h.eq_ignore_ascii_case(name));
                (
                    name,
                    shown
                        .then_some(value.as_str())
                        .map_or(HeaderValue::Hidden, HeaderValue::Shown),
                )
            }))
            .finish()
    }
}

enum HeaderValue<'a> {
    Shown(&'a str),
    Hidden,
}

impl std::fmt::Debug for HeaderValue<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeaderValue::Shown(v) => v.fmt(f),
            HeaderValue::Hidden => f.write_str("<redacted>"),
        }
    }
}

/// A body as `Debug` shows it: its size.
struct Size(usize);

impl std::fmt::Debug for Size {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{} bytes>", self.0)
    }
}

/// How `Debug` shows a secret: whether there is one, not what (S34).
pub(crate) struct Hidden(&'static str);

impl Hidden {
    pub(crate) fn text(s: &str) -> Self {
        Hidden(if s.is_empty() { "\"\"" } else { "<redacted>" })
    }

    pub(crate) fn opt(s: Option<&str>) -> Self {
        Hidden(if s.is_some() {
            "Some(<redacted>)"
        } else {
            "None"
        })
    }
}

impl std::fmt::Debug for Hidden {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// `url` as `Debug` and error messages show it: without user info, query
/// or fragment (a signed URL's signature, a token).
pub(crate) fn shown_url(url: &str) -> String {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    let (base, rest) = url.split_at(end);
    let base = match base.split_once("://") {
        Some((scheme, after)) => {
            let authority = after.find('/').unwrap_or(after.len());
            match after[..authority].rfind('@') {
                Some(at) => format!("{scheme}://<redacted>@{}", &after[at + 1..]),
                None => base.to_string(),
            }
        }
        None => base.to_string(),
    };
    if rest.is_empty() {
        base
    } else {
        format!("{base}?<redacted>")
    }
}

/// An error message about `url`, without the URL's credentials.
fn scrubbed(message: String, url: &str) -> String {
    let message = message.replace(url, &shown_url(url));
    match url.find(['?', '#']) {
        Some(i) if i + 1 < url.len() => message.replace(&url[i..], "?<redacted>"),
        _ => message,
    }
}

/// Headers a redirect to another origin always drops (#250), like browsers.
const ALWAYS_SECRET: [&str; 3] = ["authorization", "proxy-authorization", "cookie"];

impl Request {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Request {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout: Duration::from_secs(30),
            redirects: Redirects::default(),
            secret_headers: Vec::new(),
            max_body: None,
            public_only: false,
        }
    }
    pub fn get(url: impl Into<String>) -> Self {
        Self::new(Method::Get, url)
    }
    pub fn post(url: impl Into<String>) -> Self {
        Self::new(Method::Post, url)
    }
    pub fn put(url: impl Into<String>) -> Self {
        Self::new(Method::Put, url)
    }
    pub fn patch(url: impl Into<String>) -> Self {
        Self::new(Method::Patch, url)
    }
    pub fn delete(url: impl Into<String>) -> Self {
        Self::new(Method::Delete, url)
    }
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
    /// A header that holds a secret (an API token such as `X-Api-Key`)
    /// (#250): sent to the request's origin only, dropped when a redirect
    /// leads elsewhere, like `Authorization` and `Cookie` always are.
    pub fn secret_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let name = name.into();
        self.secret_headers.push(name.to_ascii_lowercase());
        self.header(name, value)
    }
    /// Connects only to publicly routable addresses: every address the host
    /// resolves to (at each redirect too) must be public, not loopback,
    /// private, link-local or reserved, IPv4 written as IPv6 included.
    /// Proxy environment variables are ignored, since the check needs the
    /// destination's address. For URLs that come from users.
    pub fn public_only(mut self) -> Self {
        self.public_only = true;
        self
    }

    /// Which redirects to follow (#250): [`Redirects::Follow`]`(10)` by
    /// default.
    pub fn redirects(mut self, redirects: Redirects) -> Self {
        self.redirects = redirects;
        self
    }
    /// Fails past `bytes` of response body (#250), before reading more
    /// (from the `Content-Length` when the server sends one).
    pub fn max_body(mut self, bytes: u64) -> Self {
        self.max_body = Some(bytes);
        self
    }
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }
    /// A JSON body (already serialized) with its content type.
    pub fn json(self, json: impl Into<String>) -> Self {
        self.header("content-type", "application/json")
            .body(json.into())
    }
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// Performs the request on the current thread.
    pub fn send_blocking(self) -> Result<Response, Error> {
        let mut body = Vec::new();
        let head = self.send_stream_blocking(|chunk, _| {
            body.extend_from_slice(chunk);
            true
        })?;
        Ok(Response {
            status: head.status,
            headers: head.headers,
            body,
        })
    }

    /// Performs the request on the current thread, handing the response
    /// body to `on_chunk` as it arrives (#250), with its [`Progress`]:
    /// write a large download to disk without holding it in memory.
    /// `on_chunk` returning `false` stops the download (an error).
    pub fn send_stream_blocking(
        self,
        mut on_chunk: impl FnMut(&[u8], Progress) -> bool,
    ) -> Result<ResponseHead, Error> {
        let limit = self.max_body;
        let (status, headers, mut res) = self.open()?;
        let total = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.trim().parse::<u64>().ok());
        if let (Some(max), Some(total)) = (limit, total)
            && total > max
        {
            return Err(too_large(max));
        }
        let mut reader = res.body_mut().as_reader();
        let mut buf = vec![0u8; 64 * 1024];
        let mut received = 0u64;
        loop {
            let n = reader.read(&mut buf).map_err(|e| Error(e.to_string()))?;
            if n == 0 {
                break;
            }
            received += n as u64;
            if let Some(max) = limit
                && received > max
            {
                return Err(too_large(max));
            }
            if !on_chunk(&buf[..n], Progress { received, total }) {
                return Err(Error("stopped".into()));
            }
        }
        Ok(ResponseHead { status, headers })
    }

    /// Sends the request, following redirects by hand (#250): ureq follows
    /// none, so secret headers are dropped when one leaves the origin.
    #[allow(clippy::type_complexity)]
    fn open(self) -> Result<(u16, Vec<(String, String)>, ureq::http::Response<ureq::Body>), Error> {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(self.timeout))
            .http_status_as_error(false)
            .max_redirects(0);
        let agent: ureq::Agent = if self.public_only {
            // No proxy: the resolver must see the destination.
            ureq::Agent::with_parts(
                config.proxy(None).build(),
                ureq::unversioned::transport::DefaultConnector::new(),
                PublicOnly::default(),
            )
        } else {
            config.build().into()
        };
        let origin = origin_of(&self.url);
        let (mut method, mut url, mut headers, mut body) =
            (self.method, self.url, self.headers, self.body);
        let mut left = match self.redirects {
            Redirects::None => 0,
            Redirects::SameOrigin => 10,
            Redirects::Follow(n) => n,
        };
        loop {
            let res = match (&method, body.clone()) {
                (Method::Get, _) => with_headers(agent.get(&url), &headers).call(),
                (Method::Delete, _) => with_headers(agent.delete(&url), &headers).call(),
                (Method::Post, b) => {
                    with_headers(agent.post(&url), &headers).send(b.unwrap_or_default())
                }
                (Method::Put, b) => {
                    with_headers(agent.put(&url), &headers).send(b.unwrap_or_default())
                }
                (Method::Patch, b) => {
                    with_headers(agent.patch(&url), &headers).send(b.unwrap_or_default())
                }
            }
            .map_err(|e| Error(scrubbed(e.to_string(), &url)))?;
            let status = res.status().as_u16();
            let list: Vec<(String, String)> = res
                .headers()
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
                .collect();
            let location = list
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("location"))
                .map(|(_, v)| v.clone());
            let next = match (matches!(status, 301 | 302 | 303 | 307 | 308), location) {
                (true, Some(l)) if left > 0 => resolve(&url, &l),
                _ => None,
            };
            let Some(next) = next else {
                return Ok((status, list, res));
            };
            let elsewhere = origin_of(&next) != origin;
            if elsewhere && self.redirects == Redirects::SameOrigin {
                return Ok((status, list, res));
            }
            left -= 1;
            // Secrets stay with their origin.
            if elsewhere {
                headers.retain(|(k, _)| {
                    let k = k.to_ascii_lowercase();
                    !ALWAYS_SECRET.contains(&k.as_str()) && !self.secret_headers.contains(&k)
                });
            }
            // 303, and 301/302 after a POST, become a GET without a body.
            if status == 303 || (matches!(status, 301 | 302) && method == Method::Post) {
                method = Method::Get;
                body = None;
                headers.retain(|(k, _)| {
                    !k.eq_ignore_ascii_case("content-type")
                        && !k.eq_ignore_ascii_case("content-length")
                });
            }
            url = next;
        }
    }
}

/// How much of a response body arrived (#250).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    /// Bytes so far.
    pub received: u64,
    /// The whole body's size, when the server says it.
    pub total: Option<u64>,
}

impl Progress {
    /// From 0 to 1, when the size is known.
    pub fn fraction(&self) -> Option<f64> {
        self.total
            .filter(|t| *t > 0)
            .map(|t| (self.received as f64 / t as f64).min(1.))
    }
}

/// A response's status and headers, its body streamed (#250). `Debug`
/// shows header values only for harmless headers (`Content-Type`…).
#[derive(Clone)]
pub struct ResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

impl std::fmt::Debug for ResponseHead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseHead")
            .field("status", &self.status)
            .field("headers", &ShownHeaders(&self.headers))
            .finish()
    }
}

fn too_large(max: u64) -> Error {
    Error(format!("the response is larger than the {max}-byte limit"))
}

/// `https://example.com:8443` for any URL on that origin (lowercase, the
/// default port left out).
fn origin_of(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let scheme = scheme.to_ascii_lowercase();
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let host = match (scheme.as_str(), host.rsplit_once(':')) {
        ("https", Some((h, "443"))) | ("http", Some((h, "80"))) => h.to_string(),
        _ => host,
    };
    format!("{scheme}://{host}")
}

/// A redirect's `Location` resolved against the URL it came from.
fn resolve(base: &str, location: &str) -> Option<String> {
    let location = location.trim();
    if location.is_empty() {
        return None;
    }
    if location.contains("://") {
        return Some(location.to_string());
    }
    let (scheme, rest) = base.split_once("://")?;
    if let Some(net) = location.strip_prefix("//") {
        return Some(format!("{scheme}://{net}"));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if location.starts_with('/') {
        return Some(format!("{scheme}://{authority}{location}"));
    }
    // Relative to the base's folder.
    let path = rest[authority.len()..]
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    let folder = path.rsplit_once('/').map_or("", |(f, _)| f);
    Some(format!("{scheme}://{authority}{folder}/{location}"))
}

fn with_headers<B>(
    mut req: ureq::RequestBuilder<B>,
    headers: &[(String, String)],
) -> ureq::RequestBuilder<B> {
    for (k, v) in headers {
        req = req.header(k, v);
    }
    req
}

/// An HTTP response (any status; check [`Response::ok`]). `Debug` shows
/// header values only for harmless headers (`Content-Type`…), and the
/// body's size (a token endpoint's answer holds tokens).
#[derive(Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("headers", &ShownHeaders(&self.headers))
            .field("body", &Size(self.body.len()))
            .finish()
    }
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A network-level failure (DNS, TLS, timeout…).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// `cx.fetch(request).await` on an [`AsyncApp`].
pub trait Fetch {
    fn fetch(&self, request: Request) -> Background<Result<Response, Error>>;
}

impl Fetch for AsyncApp {
    fn fetch(&self, request: Request) -> Background<Result<Response, Error>> {
        self.background(move || request.send_blocking())
    }
}

/// `cx.fetch_stream(request, on_chunk).await` on an [`AsyncApp`] (#250):
/// the body goes to `on_chunk` as it arrives, on a background thread (write
/// it to a file); send the [`Progress`] to the UI through a
/// `prism_core::channel` for a progress bar.
pub trait FetchStream {
    fn fetch_stream(
        &self,
        request: Request,
        on_chunk: impl FnMut(&[u8], Progress) -> bool + Send + 'static,
    ) -> Background<Result<ResponseHead, Error>>;
}

impl FetchStream for AsyncApp {
    fn fetch_stream(
        &self,
        request: Request,
        on_chunk: impl FnMut(&[u8], Progress) -> bool + Send + 'static,
    ) -> Background<Result<ResponseHead, Error>> {
        self.background(move || request.send_stream_blocking(on_chunk))
    }
}

/// The resolver of [`Request::public_only`]: the system's answer, refused
/// when any address in it is not public (a name can mix them).
#[derive(Debug, Default)]
struct PublicOnly(ureq::unversioned::resolver::DefaultResolver);

impl ureq::unversioned::resolver::Resolver for PublicOnly {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let addrs = self.0.resolve(uri, config, timeout)?;
        screen(uri.host().unwrap_or(""), &addrs)?;
        Ok(addrs)
    }
}

/// An error unless every address of `host` is public.
fn screen(host: &str, addrs: &[std::net::SocketAddr]) -> Result<(), ureq::Error> {
    if addrs.is_empty() || addrs.iter().any(|a| !is_public(a.ip())) {
        return Err(ureq::Error::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{host}: not a public address"),
        )));
    }
    Ok(())
}

/// Whether `ip` is a publicly routable unicast address (the rules of
/// `prism-push`'s endpoint check): not loopback, private, shared,
/// link-local, multicast, documentation or reserved, in IPv4 or IPv6,
/// including IPv4 written as IPv6 (mapped, NAT64, 6to4).
fn is_public(ip: std::net::IpAddr) -> bool {
    use std::net::{IpAddr, Ipv4Addr};
    fn v4(ip: Ipv4Addr) -> bool {
        let [a, b, c, _] = ip.octets();
        !(a == 0
            || a == 10
            || a == 127
            || (a == 100 && (64..128).contains(&b))
            || (a == 169 && b == 254)
            || (a == 172 && (16..32).contains(&b))
            || (a == 192 && b == 0 && (c == 0 || c == 2))
            || (a == 192 && b == 88 && c == 99)
            || (a == 192 && b == 168)
            || (a == 198 && (b == 18 || b == 19))
            || (a == 198 && b == 51 && c == 100)
            || (a == 203 && b == 0 && c == 113)
            || a >= 224)
    }
    match ip {
        IpAddr::V4(ip) => v4(ip),
        IpAddr::V6(ip) => {
            let s = ip.segments();
            let embedded = |hi: u16, lo: u16| v4(Ipv4Addr::from(((hi as u32) << 16) | lo as u32));
            if s[..5] == [0; 5] && s[5] == 0xffff {
                return embedded(s[6], s[7]);
            }
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                return embedded(s[6], s[7]);
            }
            if s[0] == 0x2002 {
                return embedded(s[1], s[2]);
            }
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && s[1] < 0x0200)
                && !(s[0] == 0x2001 && s[1] == 0x0db8)
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S34: printing a request or a response (`{:?}`, in a log) shows no
    /// credential: header values but harmless ones, bodies, the URL's
    /// query; nor does a failure's message.
    #[test]
    fn s34_requests_and_responses_print_no_credentials() {
        let request =
            Request::post("https://api.example/v1/items?sig=S34-MARK-QUERY&x=1#S34-MARK-F")
                .header("Authorization", "Bearer S34-MARK-AUTH")
                .header("Cookie", "sid=S34-MARK-COOKIE")
                .header("X-Api-Key", "S34-MARK-KEY")
                .secret_header("x-custom", "S34-MARK-SECRET")
                .json(r#"{"password":"S34-MARK-BODY"}"#);
        let response = Response {
            status: 200,
            headers: vec![
                ("set-cookie".into(), "sid=S34-MARK-SETCOOKIE".into()),
                ("content-type".into(), "application/json".into()),
            ],
            body: br#"{"access_token":"S34-MARK-TOKEN"}"#.to_vec(),
        };
        let head = ResponseHead {
            status: 200,
            headers: response.headers.clone(),
        };
        for shown in [
            format!("{request:?}"),
            format!("{request:#?}"),
            format!("{response:?}"),
            format!("{response:#?}"),
            format!("{head:?}"),
        ] {
            assert!(!shown.contains("S34-MARK"), "{shown}");
        }
        // What helps to debug stays.
        let shown = format!("{request:?}");
        for kept in [
            "https://api.example/v1/items",
            "application/json",
            "Authorization",
            "Post",
        ] {
            assert!(shown.contains(kept), "{kept}: {shown}");
        }
        assert!(format!("{response:?}").contains("application/json"));
        // A URL the client cannot use: the error does not repeat its query
        // (the client's message names the URL).
        let err = Request::get("/x?sig=S34-MARK-QUERY")
            .timeout(Duration::from_secs(2))
            .send_blocking()
            .unwrap_err();
        assert!(!err.0.contains("S34-MARK"), "{}", err.0);
    }
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Reads one whole request: the headers, then a `Content-Length` body. A
    /// server that closes the connection with bytes left unread resets it,
    /// and on Windows the client then loses the response it was reading.
    fn read_request(s: &mut impl Read) -> String {
        let mut req = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let text = String::from_utf8_lossy(&req);
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|line| {
                        let line = line.to_ascii_lowercase();
                        line.strip_prefix("content-length:")?.trim().parse().ok()
                    })
                    .unwrap_or(0);
                if req.len() >= end + 4 + length {
                    break;
                }
            }
            match s.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => req.extend_from_slice(&buf[..n]),
            }
        }
        String::from_utf8_lossy(&req).into_owned()
    }

    #[test]
    fn fetches_from_a_local_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            read_request(&mut s);
            let _ = s.write_all(
                b"HTTP/1.1 201 Created\r\nX-Test: yes\r\nContent-Length: 5\r\n\r\nhello",
            );
        });
        let res = Request::post(format!("http://{addr}/x"))
            .json("{}")
            .send_blocking()
            .unwrap();
        assert_eq!(
            (res.status, res.text().as_str(), res.header("x-test")),
            (201, "hello", Some("yes"))
        );
        assert!(res.ok());
    }

    /// A server answering each request with `answer(request text)`; the
    /// requests it got, in order.
    fn serve(
        answer: impl Fn(&str) -> String + Send + 'static,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                let req = read_request(&mut s);
                let reply = answer(&req);
                log.lock().unwrap().push(req);
                let _ = s.write_all(reply.as_bytes());
            }
        });
        (format!("{port}"), seen)
    }

    fn has(req: &str, header: &str) -> bool {
        req.lines()
            .any(|l| l.to_ascii_lowercase().starts_with(&format!("{header}:")))
    }

    /// #250: a redirect to another origin drops `Authorization`, cookies
    /// and secret headers; the same origin keeps them.
    #[test]
    fn secrets_do_not_follow_a_redirect_elsewhere() {
        let (other, got) = serve(|_| "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".into());
        let elsewhere = format!("http://localhost:{other}/landing");
        let (port, first) = serve(move |req| {
            if req.starts_with("GET /same ") {
                "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nsame".into()
            } else if req.starts_with("GET /local ") {
                "HTTP/1.1 302 Found\r\nLocation: /same\r\nContent-Length: 0\r\n\r\n".into()
            } else {
                format!("HTTP/1.1 302 Found\r\nLocation: {elsewhere}\r\nContent-Length: 0\r\n\r\n")
            }
        });
        let request = |path: &str| {
            Request::get(format!("http://127.0.0.1:{port}{path}"))
                .header("Authorization", "Bearer s3cret")
                .header("Cookie", "sid=1")
                .secret_header("X-Api-Key", "k3y")
                .header("X-Trace", "t1")
        };
        let res = request("/away").send_blocking().unwrap();
        assert_eq!((res.status, res.text().as_str()), (200, "ok"));
        let landed = got.lock().unwrap()[0].clone();
        assert!(has(&landed, "x-trace"), "{landed}");
        for secret in ["authorization", "cookie", "x-api-key"] {
            assert!(!has(&landed, secret), "{secret} leaked: {landed}");
        }
        // Same origin: everything follows.
        let res = request("/local").send_blocking().unwrap();
        assert_eq!(res.text(), "same");
        let same = first.lock().unwrap().last().unwrap().clone();
        assert!(
            has(&same, "authorization") && has(&same, "x-api-key"),
            "{same}"
        );
        // No redirects, or the same origin only: the redirect is the answer.
        assert_eq!(
            request("/away")
                .redirects(Redirects::None)
                .send_blocking()
                .unwrap()
                .status,
            302
        );
        let stay = request("/away")
            .redirects(Redirects::SameOrigin)
            .send_blocking()
            .unwrap();
        assert_eq!(stay.status, 302);
        assert_eq!(
            request("/local")
                .redirects(Redirects::SameOrigin)
                .send_blocking()
                .unwrap()
                .text(),
            "same"
        );
    }

    /// #250: a size limit, from the header or as the body arrives, and
    /// the body handed over in chunks.
    #[test]
    fn bodies_are_bounded_and_streamed() {
        let big = "x".repeat(200_000);
        let body = big.clone();
        let (port, _) = serve(move |req| {
            if req.starts_with("GET /close ") {
                // No length: read until the connection closes.
                format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{body}")
            } else {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
            }
        });
        let url = |p: &str| format!("http://127.0.0.1:{port}{p}");
        let e = Request::get(url("/len"))
            .max_body(1000)
            .send_blocking()
            .unwrap_err();
        assert!(e.0.contains("1000-byte limit"), "{e}");
        let e = Request::get(url("/close"))
            .max_body(1000)
            .send_blocking()
            .unwrap_err();
        assert!(e.0.contains("1000-byte limit"), "{e}");
        assert_eq!(
            Request::get(url("/len"))
                .max_body(300_000)
                .send_blocking()
                .unwrap()
                .body
                .len(),
            200_000
        );
        let mut chunks = 0;
        let mut last = None;
        let head = Request::get(url("/len"))
            .send_stream_blocking(|c, p| {
                chunks += 1;
                assert!(!c.is_empty());
                last = Some(p);
                true
            })
            .unwrap();
        assert_eq!(head.status, 200);
        assert!(chunks > 1, "streamed in pieces");
        let p = last.unwrap();
        assert_eq!(
            (p.received, p.total, p.fraction()),
            (200_000, Some(200_000), Some(1.))
        );
        // Stopping the download is an error.
        assert!(
            Request::get(url("/len"))
                .send_stream_blocking(|_, _| false)
                .is_err()
        );
    }

    #[test]
    fn redirect_targets_and_origins() {
        assert_eq!(
            origin_of("HTTPS://Example.com:443/a?b"),
            "https://example.com"
        );
        assert_eq!(
            origin_of("https://example.com:8443/a"),
            "https://example.com:8443"
        );
        assert_ne!(
            origin_of("http://example.com/"),
            origin_of("https://example.com/")
        );
        let base = "https://api.example.com/v1/files/a?x=1";
        assert_eq!(resolve(base, "/b").unwrap(), "https://api.example.com/b");
        assert_eq!(
            resolve(base, "c").unwrap(),
            "https://api.example.com/v1/files/c"
        );
        assert_eq!(
            resolve(base, "//cdn.example.com/d").unwrap(),
            "https://cdn.example.com/d"
        );
        assert_eq!(resolve(base, "http://x.org/e").unwrap(), "http://x.org/e");
        assert_eq!(resolve(base, " "), None);
    }

    /// S6 (security review): a public-only request never connects to a
    /// private address, whatever the name or the redirect.
    #[test]
    fn s6_public_only_requests_reach_public_addresses_only() {
        let addr = |s: &str| std::net::SocketAddr::new(s.parse().unwrap(), 443);
        assert!(screen("p.example", &[addr("8.8.8.8"), addr("2a00:1450::1")]).is_ok());
        for bad in [
            vec![addr("127.0.0.1")],
            vec![addr("10.1.2.3")],
            vec![addr("169.254.169.254")],
            vec![addr("::1")],
            vec![addr("fd00::1")],
            vec![addr("::ffff:192.168.1.1")],
            vec![addr("64:ff9b::a00:1")],
            // A changed answer that adds a private address.
            vec![addr("8.8.8.8"), addr("192.168.1.1")],
            vec![],
        ] {
            assert!(screen("p.example", &bad).is_err(), "{bad:?}");
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        for url in [
            format!("http://127.0.0.1:{port}/"),
            format!("http://localhost:{port}/"),
            format!("http://[::ffff:127.0.0.1]:{port}/"),
        ] {
            let err = Request::get(&url)
                .public_only()
                .send_blocking()
                .unwrap_err();
            assert!(err.0.contains("not a public address"), "{url}: {}", err.0);
        }
        assert!(
            listener.accept().is_err(),
            "a public-only request connected"
        );
    }
}
