//! What a host server adds to Prism's pages and sessions: tags in the
//! page's `<head>`, a per-request context for the app, a CSP nonce, and the
//! origin checks of the socket.

use std::any::Any;
use std::sync::Arc;

use http::{HeaderMap, Uri};

use crate::html::escape;

/// A value the host attaches to a request for the app to read with
/// `cx.request_context::<T>()`: the signed-in user, a tenant…
///
/// Insert it as a request extension in a middleware (Prism's Axum router
/// reads it from the page request and from every socket handshake, so it is
/// evaluated again when the page reconnects; the page request's own value
/// stays readable once in the live session, with
/// `cx.page_request_context::<T>()`), or pass it to
/// [`WebApp::handle_with`](crate::WebApp::handle_with) /
/// [`WebApp::connect_with`](crate::WebApp::connect_with):
///
/// ```ignore
/// async fn sign_in(mut req: Request, next: Next) -> Response {
///     let user = users::from_cookie(req.headers()).await?;
///     req.extensions_mut().insert(prism_web::RequestContext::new(user));
///     next.run(req).await
/// }
/// ```
#[derive(Clone)]
pub struct RequestContext(pub Arc<dyn Any + Send + Sync>);

impl RequestContext {
    /// Wraps `value`.
    pub fn new<T: Any + Send + Sync>(value: T) -> Self {
        RequestContext(Arc::new(value))
    }
}

impl std::fmt::Debug for RequestContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RequestContext(..)")
    }
}

/// The nonce of the host's `Content-Security-Policy` for this page
/// (`style-src 'self' 'nonce-…'; script-src 'self' 'nonce-…'`). Insert it as
/// a request extension: Prism puts it on its `<style>`, `<link>` and
/// `<script>` tags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CspNonce(pub String);

impl CspNonce {
    /// A random nonce (128 bits, hex).
    pub fn random() -> Self {
        CspNonce(crate::app::new_session_id())
    }
}

/// One thing the app adds to its pages' `<head>`.
#[derive(Clone, Debug)]
pub(crate) enum HeadItem {
    /// Raw HTML, as it is.
    Raw(String),
    /// `<meta name content>`.
    Meta(String, String),
    /// `<link rel href>` plus extra attributes (already escaped).
    Link(&'static str, String, &'static str),
    /// `<script src defer>`.
    Script(String),
    /// An inline `<script>` (gets the nonce).
    InlineScript(String),
}

/// The app's additions to the page, set on [`WebApp`](crate::WebApp).
#[derive(Clone, Debug, Default)]
pub(crate) struct Head {
    pub items: Vec<HeadItem>,
    /// CSS of the page's `<style id="prism-page">` (body background, fonts,
    /// the app's own rules).
    pub css: String,
}

/// ` nonce="…"` or nothing.
pub(crate) fn nonce_attr(nonce: Option<&CspNonce>) -> String {
    match nonce {
        Some(n) => format!(r#" nonce="{}""#, escape(&n.0)),
        None => String::new(),
    }
}

/// Keeps CSS text inside its `<style>` element (`<` cannot close it).
pub(crate) fn css_text(css: &str) -> String {
    css.replace('<', "\\3c ")
}

impl Head {
    /// The tags, with app asset names resolved by `url` (`app.css` →
    /// `/assets/app.<hash>.css`).
    pub fn render(&self, nonce: Option<&CspNonce>, url: &dyn Fn(&str) -> String) -> String {
        let n = nonce_attr(nonce);
        let mut out = String::new();
        for item in &self.items {
            match item {
                HeadItem::Raw(html) => out.push_str(html),
                HeadItem::Meta(name, content) => out.push_str(&format!(
                    r#"<meta name="{}" content="{}">"#,
                    escape(name),
                    escape(content)
                )),
                HeadItem::Link(rel, href, extra) => out.push_str(&format!(
                    r#"<link rel="{rel}" href="{}"{extra}{n}>"#,
                    escape(&url(href))
                )),
                HeadItem::Script(src) => out.push_str(&format!(
                    r#"<script src="{}" defer{n}></script>"#,
                    escape(&url(src))
                )),
                HeadItem::InlineScript(js) => {
                    out.push_str(&format!("<script{n}>{}</script>", js.replace("</", "<\\/")))
                }
            }
            out.push('\n');
        }
        out
    }
}

/// The type of an icon from its name.
pub(crate) fn icon_type(url: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    match path
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("svg") => r#" type="image/svg+xml""#,
        Some("png") => r#" type="image/png""#,
        Some("ico") => r#" type="image/x-icon""#,
        _ => "",
    }
}

/// The font's type for a preload, from its name.
pub(crate) fn font_type(url: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    match path
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("woff2") => r#" as="font" type="font/woff2" crossorigin"#,
        Some("woff") => r#" as="font" type="font/woff" crossorigin"#,
        Some("ttf") => r#" as="font" type="font/ttf" crossorigin"#,
        Some("otf") => r#" as="font" type="font/otf" crossorigin"#,
        _ => r#" as="font" crossorigin"#,
    }
}

// --- Origins -------------------------------------------------------------------------------

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// A `Forwarded` header's `name=` value (first hop).
fn forwarded(headers: &HeaderMap, name: &str) -> Option<String> {
    let first = header(headers, "forwarded")?.split(',').next()?;
    first.split(';').find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        k.eq_ignore_ascii_case(name)
            .then(|| v.trim_matches('"').to_string())
    })
}

/// The host the browser asked for: `X-Forwarded-Host`, `Forwarded: host=`,
/// then `Host` (a proxy in front may rewrite the last one).
pub(crate) fn request_host(headers: &HeaderMap, uri: Option<&Uri>) -> Option<String> {
    header(headers, "x-forwarded-host")
        .map(|h| h.split(',').next().unwrap_or(h).trim().to_string())
        .or_else(|| forwarded(headers, "host"))
        .or_else(|| header(headers, "host").map(str::to_string))
        .or_else(|| uri.and_then(|u| u.authority()).map(|a| a.to_string()))
}

/// The scheme the browser used (`X-Forwarded-Proto`, `Forwarded: proto=`,
/// the URI's; `http` otherwise). A socket's `wss` / `ws` (Traefik forwards
/// those for WebSocket upgrades, #286) is its page's `https` / `http`.
pub(crate) fn request_scheme(headers: &HeaderMap, uri: Option<&Uri>) -> String {
    let scheme = header(headers, "x-forwarded-proto")
        .map(|p| p.split(',').next().unwrap_or(p).trim().to_string())
        .or_else(|| forwarded(headers, "proto"))
        .or_else(|| uri.and_then(|u| u.scheme_str()).map(str::to_string))
        .unwrap_or_else(|| "http".into())
        .to_ascii_lowercase();
    match scheme.as_str() {
        "wss" => "https".into(),
        "ws" => "http".into(),
        _ => scheme,
    }
}

/// The page's origin (`https://app.example.com`): the app's public origin
/// if it has one, else the `Origin` header (sent with every socket
/// handshake), else rebuilt from the forwarded headers and `Host`.
pub(crate) fn page_origin(headers: &HeaderMap, uri: Option<&Uri>, public: Option<&str>) -> String {
    if let Some(public) = public {
        return public.to_string();
    }
    if let Some(o) = header(headers, "origin").filter(|o| *o != "null") {
        return o.trim_end_matches('/').to_string();
    }
    format!(
        "{}://{}",
        request_scheme(headers, uri),
        request_host(headers, uri).unwrap_or_else(|| "localhost".into())
    )
}

/// An origin as compared: scheme (`http` or `https`), host (lower case),
/// port (the scheme's default written out). `None` for anything else: a
/// path, credentials, another scheme, `null`.
pub(crate) fn origin_parts(origin: &str) -> Option<(String, String, u16)> {
    let (scheme, rest) = origin.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let default = match scheme.as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let (host, port) = authority_parts(rest, default)?;
    Some((scheme, host, port))
}

/// `host[:port]`: the host (lower case, an IPv6 address in brackets) and
/// the port (`default` if none). `None` if anything else is in it.
fn authority_parts(a: &str, default: u16) -> Option<(String, u16)> {
    if a.is_empty()
        || a.contains(['/', '?', '#', '@', '\\'])
        || a.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let (host, port) = match a.strip_prefix('[') {
        Some(v6) => {
            let (h, after) = v6.split_once(']')?;
            let port = match after {
                "" => None,
                p => Some(p.strip_prefix(':')?),
            };
            (format!("[{}]", h.to_ascii_lowercase()), port)
        }
        None => match a.rsplit_once(':') {
            Some((h, p)) => (h.to_ascii_lowercase(), Some(p)),
            None => (a.to_ascii_lowercase(), None),
        },
    };
    if host.is_empty() || host == "[]" || (host.contains(':') && !host.starts_with('[')) {
        return None;
    }
    let port = match port {
        None => default,
        Some(p) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => p.parse().ok()?,
        Some(_) => return None,
    };
    Some((host, port))
}

/// Whether the proxy in front said which scheme the browser used.
fn forwarded_scheme(headers: &HeaderMap) -> bool {
    header(headers, "x-forwarded-proto").is_some() || forwarded(headers, "proto").is_some()
}

/// Whether a socket handshake may open a session: no `Origin` (not a
/// browser, so no page's cookies), an origin of `allowed` (`*`: any), or
/// the page's own origin, compared whole (scheme, host, port): the app's
/// public origin if it has one, else the request's. One `Origin` field
/// only.
pub(crate) fn origin_allowed(
    headers: &HeaderMap,
    allowed: &[String],
    public: Option<&str>,
) -> bool {
    let mut fields = headers.get_all("origin").iter();
    let Some(origin) = fields.next() else {
        return true;
    };
    if fields.next().is_some() {
        return false;
    }
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let origin = origin.trim_end_matches('/');
    if allowed
        .iter()
        .any(|a| a == "*" || a.trim_end_matches('/').eq_ignore_ascii_case(origin))
    {
        return true;
    }
    let Some(theirs) = origin_parts(origin) else {
        return false;
    };
    if let Some(public) = public {
        return origin_parts(public).is_some_and(|ours| ours == theirs);
    }
    let Some(ours) = request_host(headers, None)
        .and_then(|host| origin_parts(&format!("{}://{host}", request_scheme(headers, None))))
    else {
        return false;
    };
    theirs == ours
        // TLS ended in front of the server, which was not told: an HTTPS
        // page of this host (never an HTTP page of an HTTPS one).
        || (!forwarded_scheme(headers)
            && ours.0 == "http"
            && theirs.0 == "https"
            && theirs.1 == ours.1
            && (theirs.2 == ours.2 || (ours.2 == 80 && theirs.2 == 443)))
}

/// Whether a request may reach a server listening on a loopback address
/// only: its `Host` is a loopback name or address, or the host of the
/// app's public origin, or a proxy on this machine forwarded it
/// (`X-Forwarded-*`, `Forwarded`, `X-Real-IP`). Anything else is a page
/// whose name was pointed at 127.0.0.1 (DNS rebinding): a navigation or a
/// WebSocket cannot add those headers.
pub(crate) fn loopback_host_allowed(headers: &HeaderMap, public: Option<&str>) -> bool {
    let Some(host) = header(headers, "host") else {
        return true;
    };
    let Some((host, _)) = authority_parts(host, 0) else {
        return false;
    };
    let loopback = host == "localhost"
        || host.ends_with(".localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    loopback
        || public
            .and_then(origin_parts)
            .is_some_and(|(_, public, _)| public == host)
        || [
            "x-forwarded-for",
            "x-forwarded-host",
            "x-forwarded-proto",
            "forwarded",
            "x-real-ip",
        ]
        .iter()
        .any(|h| headers.contains_key(*h))
}

/// « Reconnecting… » in the app's language (`prism.reconnecting` in its
/// translations overrides it).
pub(crate) fn reconnecting_text(app: &prism_core::App) -> String {
    let locale = app.locale();
    if let Some(t) = app
        .try_global::<prism_core::Translations>()
        .and_then(|t| t.lookup(locale, "prism.reconnecting"))
    {
        return t.to_string();
    }
    const TEXTS: &[(&str, &str)] = &[
        ("en", "Reconnecting…"),
        ("fr", "Reconnexion…"),
        ("de", "Verbindung wird wiederhergestellt…"),
        ("es", "Reconectando…"),
        ("it", "Riconnessione…"),
        ("pt", "Reconectando…"),
        ("nl", "Opnieuw verbinden…"),
        ("ar", "جارٍ إعادة الاتصال…"),
        ("he", "מתחבר מחדש…"),
        ("ja", "再接続しています…"),
        ("zh", "正在重新连接…"),
        ("ko", "다시 연결하는 중…"),
    ];
    TEXTS
        .iter()
        .find(|(l, _)| *l == locale.language())
        .unwrap_or(&TEXTS[0])
        .1
        .to_string()
}

/// The request as the app sees it (`cx.request()`).
pub(crate) fn web_request(
    kind: prism_core::RequestKind,
    url: String,
    origin: String,
    headers: &HeaderMap,
    context: Option<RequestContext>,
) -> prism_core::WebRequest {
    prism_core::WebRequest {
        kind,
        url,
        origin,
        headers: headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect(),
        context: context.map(|c| c.0),
    }
}
