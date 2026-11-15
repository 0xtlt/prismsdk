//! [`WebApp`]: the app as a rendering service, independent of any HTTP server.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri, header};
use prism_core::{AppConfig, AppInit, Backend, PlatformServices, Runtime};

use crate::assets::{Assets, StaticFile};
use crate::html::{self, AssetUrls, HtmlBackend};

/// A Prism app served on the web. Cheap to clone (shared).
///
/// Use it with the built-in server ([`run`](crate::run)), mount it in Axum
/// (`server` feature: [`WebApp::router`]), or call it from any other server:
///
/// - [`handle`](WebApp::handle) answers every GET except the socket: pages
///   (rendered on the server), `/_prism/*` and `/assets/*` (hashed, immutable),
///   `/files/*` (a browser's uploads);
/// - [`connect`](WebApp::connect) runs a live session for the socket at
///   `/ws`, over whatever WebSocket implementation you use.
///
/// Paths are relative to where the app is mounted: set
/// [`base_path`](WebApp::base_path) when it is not at the root.
#[derive(Clone)]
pub struct WebApp {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub config: AppConfig,
    pub init: AppInit,
    /// `""` at the root, `/app` when nested.
    pub base: String,
    /// Where `/_prism` and `/assets` are served from (a CDN), if not this server.
    pub asset_url: Option<String>,
    pub data_root: PathBuf,
    pub assets: Assets,
    /// The VAPID public key (base64url) browsers subscribe with: Web Push is
    /// available when set.
    pub vapid_public_key: Option<String>,

    // --- Embedding in a host server: the page's head, origins, session limits ---
    /// Tags and CSS the app adds to its pages.
    pub head: crate::host::Head,
    /// Socket origins accepted besides the page's own (`*`: any).
    pub allowed_origins: Vec<String>,
    /// At most this many live sessions at once.
    pub max_sessions: Option<usize>,
    /// A session that hears nothing from its page for this long ends.
    pub idle_timeout: Option<std::time::Duration>,
    /// Live sessions now.
    pub sessions: Arc<std::sync::atomic::AtomicUsize>,
}

impl WebApp {
    /// The app with its settings from the environment: `PRISM_BASE_PATH`,
    /// `PRISM_ASSET_URL` (a CDN), `PRISM_DATA_DIR` (default `.prism-data`),
    /// `PRISM_ASSETS_DIR` (default: the configured assets, else `assets`).
    pub fn new(config: AppConfig, init: AppInit) -> Self {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let assets_dir = env("PRISM_ASSETS_DIR")
            .or_else(|| config.assets.clone())
            .unwrap_or_else(|| "assets".into());
        let client = crate::CLIENT_JS.replace("__GLYPHS__", &prism_core::glyphs::js_table());
        let assets = Assets::new(
            client,
            crate::PRISM_CSS.into(),
            Some(PathBuf::from(assets_dir)),
        );
        let app = WebApp {
            inner: Arc::new(Inner {
                config,
                init,
                base: String::new(),
                asset_url: None,
                data_root: env("PRISM_DATA_DIR")
                    .unwrap_or_else(|| ".prism-data".into())
                    .into(),
                assets,
                vapid_public_key: env("PRISM_VAPID_PUBLIC_KEY"),
                head: Default::default(),
                allowed_origins: Vec::new(),
                max_sessions: None,
                idle_timeout: None,
                sessions: Default::default(),
            }),
        };
        let app = match env("PRISM_BASE_PATH") {
            Some(base) => app.base_path(&base),
            None => app,
        };
        match env("PRISM_ASSET_URL") {
            Some(url) => app.asset_url(&url),
            None => app,
        }
    }

    fn configure(mut self, f: impl FnOnce(&mut Inner)) -> Self {
        let inner = Arc::get_mut(&mut self.inner).expect("configure a WebApp before cloning it");
        f(inner);
        self
    }

    /// Mount point (`/app`) when the app is nested in another server's routes.
    pub fn base_path(self, base: &str) -> Self {
        let base = format!("/{}", base.trim_matches('/'));
        self.configure(|i| i.base = if base == "/" { String::new() } else { base })
    }

    /// Serve `/_prism/*` and `/assets/*` from here instead (a CDN in front of
    /// [`export_static`](WebApp::export_static)'s output, or of this server).
    pub fn asset_url(self, url: &str) -> Self {
        let url = url.trim_end_matches('/').to_string();
        self.configure(|i| i.asset_url = Some(url))
    }

    /// Enables Web Push: the public half of your server's VAPID key
    /// (`prism_push::webpush::VapidKey::public_key`; or `PRISM_VAPID_PUBLIC_KEY`).
    pub fn vapid_public_key(self, key: &str) -> Self {
        let key = key.trim().to_string();
        self.configure(|i| i.vapid_public_key = Some(key))
    }

    /// Where each browser's data lives (`cx.storage()`, uploads).
    pub fn data_dir(self, dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        self.configure(|i| i.data_root = dir)
    }

    pub fn config(&self) -> &AppConfig {
        &self.inner.config
    }

    /// The mount point (`""` at the root).
    pub fn base(&self) -> &str {
        &self.inner.base
    }

    /// Writes the hashed static files (`_prism/…`, `assets/…`,
    /// `manifest.json`) under `dir`, to upload to a CDN. Returns the count.
    pub fn export_static(&self, dir: &Path) -> std::io::Result<usize> {
        self.inner.assets.export(dir)
    }

    /// URL prefix of static files.
    fn static_prefix(&self) -> &str {
        self.inner.asset_url.as_deref().unwrap_or(&self.inner.base)
    }

    /// The URL of a picture source: remote and absolute URLs as they are,
    /// app assets hashed (`/assets/logo.<hash>.png`, maybe on the CDN).
    pub fn picture_url(&self, src: &str) -> String {
        if src.starts_with("http://")
            || src.starts_with("https://")
            || src.starts_with("data:")
            || src.starts_with('/')
        {
            return src.to_string();
        }
        format!(
            "{}/{}",
            self.static_prefix(),
            self.inner.assets.app_path(src)
        )
    }

    pub(crate) fn asset_urls(&self) -> AssetUrls {
        let app = self.clone();
        Arc::new(move |src: &str| app.picture_url(src))
    }

    /// The path inside the app for a request path (`/app/todos` → `/todos`).
    fn app_path<'a>(&self, path: &'a str) -> &'a str {
        let base = &self.inner.base;
        match path.strip_prefix(base.as_str()) {
            Some("") if !base.is_empty() => "/",
            Some(rest) if !base.is_empty() && rest.starts_with('/') => rest,
            _ => path,
        }
    }

    /// Whether `path` is the live session socket (upgrade it, then
    /// [`connect`](WebApp::connect)).
    pub fn is_socket(&self, path: &str) -> bool {
        self.app_path(path) == "/ws"
    }

    /// Answers a GET or HEAD: a page, a static file or an upload.
    ///
    /// The request's extensions may carry a [`RequestContext`] (the app reads
    /// it with `cx.request_context::<T>()`) and a [`CspNonce`] (put on the
    /// page's `<style>`, `<link>` and `<script>` tags). An unknown path
    /// renders the router's not-found screen with status `404`.
    ///
    /// [`RequestContext`]: crate::RequestContext
    /// [`CspNonce`]: crate::CspNonce
    pub fn handle<B>(&self, req: &Request<B>) -> Response<Vec<u8>> {
        let context = req.extensions().get::<crate::RequestContext>().cloned();
        self.respond(req, context)
    }

    fn respond<B>(
        &self,
        req: &Request<B>,
        context: Option<crate::RequestContext>,
    ) -> Response<Vec<u8>> {
        if req.method() != Method::GET && req.method() != Method::HEAD {
            return status(StatusCode::METHOD_NOT_ALLOWED);
        }
        let path = percent_decode(self.app_path(req.uri().path()));
        let headers = req.headers();
        let mut response = if path == "/_prism/auth/callback" {
            // A sign-in provider's redirect: hand it to the session that asked.
            let mut r = Response::new(
                crate::security::redirect_page(
                    req.uri().query().unwrap_or(""),
                    req.extensions().get::<crate::CspNonce>(),
                )
                .into_bytes(),
            );
            set(&mut r, header::CONTENT_TYPE, "text/html; charset=utf-8");
            set(&mut r, header::CACHE_CONTROL, "no-store");
            r
        } else if let Some(file) = path
            .strip_prefix('/')
            .filter(|p| p.starts_with("_prism/") || p.starts_with("assets/"))
        {
            match self.inner.assets.get(file) {
                Some(f) => file_response(f, headers),
                None => status(StatusCode::NOT_FOUND),
            }
        } else if let Some(name) = path.strip_prefix("/files/") {
            self.upload(headers, name)
        } else if path == "/prism-sw.js" {
            // The push service worker: at the app's root (its scope), from
            // this origin (never a CDN), revalidated so updates reach browsers.
            let mut r = Response::new(crate::SW_JS.as_bytes().to_vec());
            set(
                &mut r,
                header::CONTENT_TYPE,
                "text/javascript; charset=utf-8",
            );
            set(&mut r, header::CACHE_CONTROL, "no-cache");
            r
        } else if path == "/favicon.ico" || path == "/ws" {
            status(StatusCode::NOT_FOUND)
        } else {
            // The router keeps the query string (`/analytics?period=7d`),
            // still escaped: it decodes each value itself.
            let page = match req.uri().query().filter(|q| !q.is_empty()) {
                Some(query) => format!("{path}?{query}"),
                None => path,
            };
            self.page_response(&page, req, context)
        };
        if req.method() == Method::HEAD {
            response.body_mut().clear();
        }
        response
    }

    /// A file this browser uploaded (photos…), private to it.
    fn upload(&self, headers: &HeaderMap, name: &str) -> Response<Vec<u8>> {
        let Some(sid) = cookie_header(headers).and_then(session_id) else {
            return status(StatusCode::NOT_FOUND);
        };
        if !crate::assets::safe(name) {
            return status(StatusCode::NOT_FOUND);
        }
        match std::fs::read(self.files_dir(&sid).join(name)) {
            Ok(body) => {
                let mut r = Response::new(body);
                set(
                    &mut r,
                    header::CONTENT_TYPE,
                    crate::assets::content_type(name),
                );
                // Names are random and never reused; never cache it on a CDN.
                set(
                    &mut r,
                    header::CACHE_CONTROL,
                    "private, max-age=31536000, immutable",
                );
                r
            }
            Err(_) => status(StatusCode::NOT_FOUND),
        }
    }

    fn page_response<B>(
        &self,
        path: &str,
        req: &Request<B>,
        context: Option<crate::RequestContext>,
    ) -> Response<Vec<u8>> {
        let headers = req.headers();
        let origin = crate::host::page_origin(headers, Some(req.uri()));
        let request = crate::host::web_request(
            prism_core::RequestKind::Page,
            self.page_url(&origin, self.app_path(req.uri().path()), req.uri().query()),
            origin,
            headers,
            context,
        );
        let nonce = req.extensions().get::<crate::CspNonce>();
        let cookies = cookie_header(headers);
        let (sid, new) = match cookies.and_then(session_id) {
            Some(sid) => (sid, false),
            None => (new_session_id(), true),
        };
        let header = |name: header::HeaderName| headers.get(name).and_then(|v| v.to_str().ok());
        let user_agent = header(header::USER_AGENT).unwrap_or("");
        let viewport = cookies.and_then(viewport_cookie).or_else(|| {
            // First visit: a phone or a desktop, until the page reports.
            let mobile = ["Mobile", "Android", "iPhone"]
                .iter()
                .any(|m| user_agent.contains(m));
            Some(if mobile { (390., 844.) } else { (1280., 800.) })
        });
        let visitor = Visitor {
            sid: &sid,
            language: header(header::ACCEPT_LANGUAGE)
                .and_then(prism_core::Locale::from_accept_language),
            tz: cookies.and_then(time_zone),
            viewport,
            device: Some(crate::device_info::from_request(user_agent, cookies)),
        };
        let (page, not_found) = self.page(path, visitor, request, nonce);
        let mut r = Response::new(page.into_bytes());
        if not_found {
            *r.status_mut() = StatusCode::NOT_FOUND;
        }
        set(&mut r, header::CONTENT_TYPE, "text/html; charset=utf-8");
        // Rendered for this browser: never stored by a CDN or the browser.
        set(&mut r, header::CACHE_CONTROL, "private, no-store");
        if new {
            let path = if self.inner.base.is_empty() {
                "/"
            } else {
                &self.inner.base
            };
            let cookie =
                format!("prism_sid={sid}; Path={path}; Max-Age=31536000; SameSite=Lax; HttpOnly");
            if let Ok(v) = HeaderValue::from_str(&cookie) {
                r.headers_mut().insert(header::SET_COOKIE, v);
            }
        }
        r
    }

    /// Platform services for one browser session.
    pub(crate) fn services(&self) -> crate::session::WebServices {
        crate::session::WebServices::new(self.inner.vapid_public_key.clone())
    }

    pub(crate) fn options(&self, visitor: Visitor, path: String) -> prism_core::RuntimeOptions {
        let sid = visitor.sid;
        prism_core::RuntimeOptions {
            path,
            capabilities: self.services().capabilities(),
            storage: Some(Box::new(prism_core::FileStorage::new(
                self.session_dir(sid),
            ))),
            wake_ui: None,
            environment: environment(visitor.language, visitor.tz),
            formatter: None,
            cloud_storage: Some(Box::new(prism_core::FileStorage::new(
                self.session_dir(sid).join("cloud"),
            ))),
            data_dir: Some(self.session_dir(sid)),
            shared_data_dir: Some(self.inner.data_root.clone()),
            viewport: visitor.viewport,
            platform: Some(prism_core::Platform::Web),
            device_info: visitor.device,
        }
    }

    pub(crate) fn session_dir(&self, sid: &str) -> PathBuf {
        self.inner.data_root.join(sid)
    }

    pub(crate) fn files_dir(&self, sid: &str) -> PathBuf {
        self.session_dir(sid).join("files")
    }

    /// URL of an uploaded file.
    pub(crate) fn file_url(&self, stored: &str) -> String {
        format!("{}/files/{stored}", self.inner.base)
    }

    /// The app's init, with the session's request set first (`cx.request()`).
    pub(crate) fn init_with(&self, request: prism_core::WebRequest) -> AppInit {
        let init = self.inner.init.clone();
        Arc::new(move |app: &mut prism_core::App| {
            app.set_request(request.clone());
            init(app)
        })
    }

    /// The URL the browser shows for `app_path` (inside the app) and `query`.
    pub(crate) fn page_url(&self, origin: &str, app_path: &str, query: Option<&str>) -> String {
        let base = &self.inner.base;
        let path = if app_path == "/" && !base.is_empty() {
            base.clone()
        } else {
            format!("{base}{app_path}")
        };
        match query.map(|q| q.trim_start_matches('?')) {
            Some(q) if !q.is_empty() => format!("{origin}{path}?{q}"),
            _ => format!("{origin}{path}"),
        }
    }

    /// Server-side render of a fresh session on `path`, as a full HTML
    /// document without `style` attributes (a strict CSP allows it), and
    /// whether it shows the not-found screen.
    fn page(
        &self,
        path: &str,
        visitor: Visitor,
        request: prism_core::WebRequest,
        nonce: Option<&crate::CspNonce>,
    ) -> (String, bool) {
        let config = &self.inner.config;
        let mut runtime =
            Runtime::with_options(&self.init_with(request), self.options(visitor, path.into()));
        let mut html = HtmlBackend::default();
        html.asset_urls = Some(self.asset_urls());
        html.apply(&runtime.render());
        let not_found = runtime.app.shows_not_found();
        let title = match html.screen_title() {
            Some(screen) if screen != config.title => format!("{screen} · {}", config.title),
            _ => config.title.clone(),
        };
        let prefix = self.static_prefix();
        let (body, ssr_css) = html.to_html_and_css();
        let head = &self.inner.head;
        let page = format!(
            r#"<!doctype html>
<html lang="{lang}" dir="{dir}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<link rel="stylesheet" href="{prefix}/{css}"{n}>
{app_head}<style id="prism-page"{n}>:root{{color-scheme:{scheme}}}
{app_css}</style>
<style id="prism-ssr"{n}>{ssr_css}</style>
</head>
<body>
<main id="prism-root">{body}</main>
<div id="prism-island" role="status" aria-live="polite"></div>
<div id="prism-toasts" role="status" aria-live="polite"></div>
<div id="prism-connection" role="status" aria-live="polite" hidden>{reconnecting}</div>
<script src="{prefix}/{js}" data-base="{base}"{n}></script>
</body>
</html>"#,
            n = crate::host::nonce_attr(nonce),
            app_head = head.render(nonce, &|src: &str| self.picture_url(src)),
            app_css = crate::host::css_text(&head.css),
            reconnecting = html::escape(&crate::host::reconnecting_text(&runtime.app)),
            title = html::escape(&title),
            scheme = html.color_scheme(),
            lang = html::escape(&html.env.lang),
            dir = if html.env.direction == prism_core::Direction::Rtl {
                "rtl"
            } else {
                "ltr"
            },
            prefix = html::escape(prefix),
            css = self.inner.assets.css(),
            js = self.inner.assets.client(),
            base = html::escape(&self.inner.base),
        );
        (page, not_found)
    }

    /// What a socket request tells about the browser: session cookie,
    /// language, time zone, window size, device, and the page's path.
    pub(crate) fn socket_visitor(
        &self,
        uri: &Uri,
        headers: &HeaderMap,
    ) -> (String, String, VisitorOwned) {
        let query = uri.query().unwrap_or("");
        let param = |k: &str| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
        };
        let num = |k: &str| param(k).and_then(|v| v.parse::<f32>().ok());
        let cookies = cookie_header(headers);
        let header = |name: header::HeaderName| headers.get(name).and_then(|v| v.to_str().ok());
        let sid = cookies.and_then(session_id).unwrap_or_else(new_session_id);
        let path = param("path")
            .map(percent_decode)
            .unwrap_or_else(|| "/".into());
        let visitor = VisitorOwned {
            language: header(header::ACCEPT_LANGUAGE)
                .and_then(prism_core::Locale::from_accept_language),
            tz: param("tz").and_then(|v| v.parse().ok()),
            viewport: num("w").zip(num("h")),
            device: Some(crate::device_info::from_request(
                header(header::USER_AGENT).unwrap_or(""),
                cookies,
            )),
        };
        (sid, path, visitor)
    }
}

/// Embedding in a host server: the page's `<head>`, origins, session limits.
impl WebApp {
    fn add_head(self, item: crate::host::HeadItem) -> Self {
        self.configure(|i| i.head.items.push(item))
    }

    /// Adds raw HTML to every page's `<head>`, as it is (trusted markup).
    /// Under a nonce-based CSP, prefer [`style`](WebApp::style) and
    /// [`inline_script`](WebApp::inline_script), which get the nonce.
    pub fn head(self, html: impl Into<String>) -> Self {
        self.add_head(crate::host::HeadItem::Raw(html.into()))
    }

    /// Adds `<meta name="…" content="…">` (`apple-mobile-web-app-title`,
    /// `description`…).
    pub fn meta(self, name: impl Into<String>, content: impl Into<String>) -> Self {
        self.add_head(crate::host::HeadItem::Meta(name.into(), content.into()))
    }

    /// The browser's UI color (`<meta name="theme-color">`).
    pub fn theme_color(self, color: impl Into<String>) -> Self {
        self.meta("theme-color", color)
    }

    /// The page's icon. A name without a leading `/` or a scheme is an app
    /// asset (`icon.png` → `/assets/icon.<hash>.png`, maybe on the CDN).
    pub fn favicon(self, url: impl Into<String>) -> Self {
        let url = url.into();
        let ty = crate::host::icon_type(&url);
        self.add_head(crate::host::HeadItem::Link("icon", url, ty))
    }

    /// The home-screen icon on iOS (`<link rel="apple-touch-icon">`); app
    /// assets like [`favicon`](WebApp::favicon).
    pub fn apple_touch_icon(self, url: impl Into<String>) -> Self {
        self.add_head(crate::host::HeadItem::Link(
            "apple-touch-icon",
            url.into(),
            "",
        ))
    }

    /// Links a stylesheet after Prism's (its rules win over Prism's base
    /// styles; node styles set in Rust still win). App assets like
    /// [`favicon`](WebApp::favicon): `app.css` is served hashed and immutable.
    pub fn stylesheet(self, url: impl Into<String>) -> Self {
        self.add_head(crate::host::HeadItem::Link("stylesheet", url.into(), ""))
    }

    /// Preloads a web font (`<link rel="preload" as="font" crossorigin>`);
    /// declare its `@font-face` with [`style`](WebApp::style) or a
    /// [`stylesheet`](WebApp::stylesheet).
    pub fn preload_font(self, url: impl Into<String>) -> Self {
        let url = url.into();
        let ty = crate::host::font_type(&url);
        self.add_head(crate::host::HeadItem::Link("preload", url, ty))
    }

    /// Loads a script (`<script src defer>`, with the CSP nonce); app assets
    /// like [`favicon`](WebApp::favicon).
    pub fn script(self, url: impl Into<String>) -> Self {
        self.add_head(crate::host::HeadItem::Script(url.into()))
    }

    /// An inline `<script>` in the head (with the CSP nonce).
    pub fn inline_script(self, js: impl Into<String>) -> Self {
        self.add_head(crate::host::HeadItem::InlineScript(js.into()))
    }

    /// CSS rules for every page (`@font-face`, the app's own classes), in
    /// the page's `<style id="prism-page">` (with the CSP nonce).
    pub fn style(self, css: impl AsRef<str>) -> Self {
        let css = css.as_ref().to_string();
        self.configure(|i| {
            i.head.css.push_str(&css);
            i.head.css.push('\n');
        })
    }

    /// The page's background behind the app (`body`; `Canvas` by default).
    pub fn body_background(self, color: impl AsRef<str>) -> Self {
        let rule = format!("body{{background:{}}}", color.as_ref());
        self.style(rule)
    }

    /// The page's font (`body`; text without a font of its own inherits it).
    pub fn font_family(self, family: impl AsRef<str>) -> Self {
        let rule = format!("body{{font-family:{}}}", family.as_ref());
        self.style(rule)
    }

    /// Socket origins accepted besides the page's own
    /// (`https://admin.example.com`; `*` accepts any). By default a
    /// handshake whose `Origin` is not the host it was sent to is refused
    /// (`403`): browsers send cookies with cross-site handshakes. Behind a
    /// proxy that rewrites `Host` without `X-Forwarded-Host`, list the
    /// public origin here.
    pub fn allow_origins<I, S>(self, origins: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let origins: Vec<String> = origins.into_iter().map(Into::into).collect();
        self.configure(|i| i.allowed_origins.extend(origins))
    }

    /// At most `n` live sessions at once (each is a thread with its own
    /// runtime): further handshakes are refused with `503`, and the pages
    /// retry with back-off.
    pub fn max_sessions(self, n: usize) -> Self {
        self.configure(|i| i.max_sessions = Some(n))
    }

    /// Ends a session that heard nothing from its page (no event, resize or
    /// visibility change) for `timeout`. The page keeps its content and
    /// reconnects (to a fresh session) at the next interaction.
    pub fn idle_timeout(self, timeout: std::time::Duration) -> Self {
        self.configure(|i| i.idle_timeout = Some(timeout))
    }

    /// Live sessions right now.
    pub fn session_count(&self) -> usize {
        self.inner
            .sessions
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Whether a socket handshake may open a session: its `Origin` (`403`
    /// when foreign, see [`allow_origins`](WebApp::allow_origins)) and the
    /// session limit (`503`, see [`max_sessions`](WebApp::max_sessions)).
    /// The Axum [`router`](WebApp::router) checks it before upgrading;
    /// other servers should too (a refused [`connect`](WebApp::connect)
    /// ends at once).
    pub fn accept_socket(&self, headers: &HeaderMap) -> Result<(), StatusCode> {
        if !crate::host::origin_allowed(headers, &self.inner.allowed_origins) {
            return Err(StatusCode::FORBIDDEN);
        }
        if self
            .inner
            .max_sessions
            .is_some_and(|max| self.session_count() >= max)
        {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        Ok(())
    }

    /// [`handle`](WebApp::handle) with the host's context for the app
    /// (`cx.request_context::<T>()`), instead of a request extension.
    pub fn handle_with<B>(
        &self,
        req: &Request<B>,
        context: crate::RequestContext,
    ) -> Response<Vec<u8>> {
        self.respond(req, Some(context))
    }
}

/// Who asked for a page (from the request headers and cookies).
pub(crate) struct Visitor<'a> {
    pub sid: &'a str,
    pub language: Option<prism_core::Locale>,
    /// UTC offset in seconds.
    pub tz: Option<i32>,
    pub viewport: Option<(f32, f32)>,
    pub device: Option<prism_core::DeviceInfo>,
}

pub(crate) struct VisitorOwned {
    pub language: Option<prism_core::Locale>,
    pub tz: Option<i32>,
    pub viewport: Option<(f32, f32)>,
    pub device: Option<prism_core::DeviceInfo>,
}

impl VisitorOwned {
    pub fn with_sid(self, sid: &str) -> Visitor<'_> {
        Visitor {
            sid,
            language: self.language,
            tz: self.tz,
            viewport: self.viewport,
            device: self.device,
        }
    }
}

// --- HTTP helpers -------------------------------------------------------------------------

fn status(code: StatusCode) -> Response<Vec<u8>> {
    let mut r = Response::new(Vec::new());
    *r.status_mut() = code;
    r
}

fn set(r: &mut Response<Vec<u8>>, name: header::HeaderName, value: &'static str) {
    r.headers_mut()
        .insert(name, HeaderValue::from_static(value));
}

/// A static file, or `304 Not Modified` when the browser (or CDN) has it.
fn file_response(f: StaticFile, headers: &HeaderMap) -> Response<Vec<u8>> {
    let fresh = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == f.etag || t.trim() == "*"));
    let mut r = if fresh {
        status(StatusCode::NOT_MODIFIED)
    } else {
        let mut r = Response::new(f.body);
        set(&mut r, header::CONTENT_TYPE, f.content_type);
        r
    };
    set(&mut r, header::CACHE_CONTROL, f.cache_control);
    if let Ok(v) = HeaderValue::from_str(&f.etag) {
        r.headers_mut().insert(header::ETAG, v);
    }
    r
}

fn cookie_header(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::COOKIE).and_then(|v| v.to_str().ok())
}

/// `prism_sid` from a Cookie header, if well-formed.
pub(crate) fn session_id(cookie_header: &str) -> Option<String> {
    cookie_header
        .split(';')
        .filter_map(|kv| kv.trim().strip_prefix("prism_sid="))
        .find(|v| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_string)
}

/// A random 128-bit id (process-random SipHash keys + time + counter).
pub(crate) fn new_session_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = std::collections::hash_map::RandomState::new();
    let part = |salt: u64| {
        let mut h = seed.build_hasher();
        h.write_u64(salt);
        h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        h.finish()
    };
    format!("{:016x}{:016x}", part(1), part(2))
}

/// The browser's UTC offset (seconds east), from the `prism_tz` cookie.
pub(crate) fn time_zone(cookie_header: &str) -> Option<i32> {
    cookie_header
        .split(';')
        .find_map(|kv| kv.trim().strip_prefix("prism_tz="))
        .and_then(|v| v.parse().ok())
        .filter(|v: &i32| v.abs() <= 14 * 3600)
}

/// The last viewport the page reported (`prism_vw=1280x800`).
pub(crate) fn viewport_cookie(cookie_header: &str) -> Option<(f32, f32)> {
    let v = cookie_header
        .split(';')
        .find_map(|kv| kv.trim().strip_prefix("prism_vw="))?;
    let (w, h) = v.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// The browser's environment: `Accept-Language` (`PRISM_LOCALE` overrides it).
fn environment(language: Option<prism_core::Locale>, tz: Option<i32>) -> prism_core::Environment {
    let locale = std::env::var("PRISM_LOCALE")
        .ok()
        .map(|l| prism_core::Locale::new(&l))
        .or(language)
        .unwrap_or_default();
    prism_core::Environment {
        locale,
        utc_offset: tz.unwrap_or(0),
        ..Default::default()
    }
}

/// Decodes `%XX` escapes (URL paths and query values).
pub(crate) fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::prelude::*;

    struct Page;
    impl Render for Page {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(text("Hello"))
                .child(picture("logo.png").alt("Logo"))
        }
    }

    fn app(tag: &str) -> (WebApp, PathBuf) {
        let dir = std::env::temp_dir().join(format!("prism-webapp-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("assets/logo.png"), b"png!").unwrap();
        let config = AppConfig {
            title: "Test".into(),
            assets: Some(dir.join("assets").to_string_lossy().into()),
            ..Default::default()
        };
        let init: AppInit = Arc::new(|cx: &mut App| {
            let root = cx.new_entity(|_| Page);
            cx.set_root(root);
        });
        let app = WebApp::new(config, init).data_dir(dir.join("data"));
        (app, dir)
    }

    fn get(app: &WebApp, path: &str, headers: &[(&str, &str)]) -> Response<Vec<u8>> {
        let mut req = Request::get(path);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        app.handle(&req.body(()).unwrap())
    }

    fn header<'a>(r: &'a Response<Vec<u8>>, name: &str) -> &'a str {
        r.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }

    #[test]
    fn pages_are_private_and_static_files_immutable() {
        let (app, dir) = app("cache");
        let page = get(&app, "/", &[]);
        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!(header(&page, "cache-control"), "private, no-store");
        assert!(header(&page, "set-cookie").starts_with("prism_sid="));
        let html = String::from_utf8(page.body().clone()).unwrap();
        // Client, CSS and pictures all have hashed names.
        let logo = format!("/assets/logo.{}.png", crate::content_hash(b"png!"));
        assert!(html.contains(&logo), "{html}");
        let js = html
            .split("<script src=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert!(js.starts_with("/_prism/client.") && js.ends_with(".js"));

        let script = get(&app, js, &[]);
        assert_eq!(header(&script, "cache-control"), crate::IMMUTABLE);
        assert!(header(&script, "content-type").starts_with("text/javascript"));
        let etag = header(&script, "etag").to_string();
        let again = get(&app, js, &[("if-none-match", &etag)]);
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
        assert!(again.body().is_empty());

        let image = get(&app, &logo, &[]);
        assert_eq!(
            (image.body().as_slice(), header(&image, "cache-control")),
            (&b"png!"[..], crate::IMMUTABLE)
        );
        assert_eq!(
            header(&get(&app, "/assets/logo.png", &[]), "cache-control"),
            crate::REVALIDATE
        );
        assert_eq!(
            get(&app, "/assets/../secret", &[]).status(),
            StatusCode::NOT_FOUND
        );
        // Uploads need the browser's own session.
        assert_eq!(
            get(&app, "/files/x.png", &[]).status(),
            StatusCode::NOT_FOUND
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mounted_under_a_prefix_and_on_a_cdn() {
        let (app, dir) = app("base");
        let app = app.base_path("/app/").asset_url("https://cdn.example.com/");
        let page = get(&app, "/app/todos", &[]);
        assert_eq!(page.status(), StatusCode::OK);
        assert!(header(&page, "set-cookie").contains("Path=/app"));
        let html = String::from_utf8(page.body().clone()).unwrap();
        assert!(html.contains(r#"data-base="/app""#));
        assert!(html.contains(r#"src="https://cdn.example.com/_prism/client."#));
        assert!(html.contains("https://cdn.example.com/assets/logo."));
        // Axum's `nest` strips the prefix: both forms work.
        assert_eq!(get(&app, "/todos", &[]).status(), StatusCode::OK);
        assert!(app.is_socket("/app/ws") && app.is_socket("/ws"));
        let head = app.handle(&Request::head("/app/").body(()).unwrap());
        assert!(head.status().is_success() && head.body().is_empty());
        let post = app.handle(&Request::post("/app/").body(()).unwrap());
        assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
        // The push service worker stays on this origin, at the app's root.
        let sw = get(&app, "/app/prism-sw.js", &[]);
        assert_eq!(sw.status(), StatusCode::OK);
        assert_eq!(header(&sw, "cache-control"), "no-cache");
        assert!(String::from_utf8_lossy(sw.body()).contains("showNotification"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn push_needs_a_vapid_key() {
        let (app, dir) = app("push");
        assert_eq!(app.services().capabilities().push, None);
        let app = app.vapid_public_key("BKVqC4g3");
        assert_eq!(
            app.services().capabilities().push,
            Some(prism_core::PushService::WebPush)
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
