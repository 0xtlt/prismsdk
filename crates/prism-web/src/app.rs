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
    pub fn handle<B>(&self, req: &Request<B>) -> Response<Vec<u8>> {
        if req.method() != Method::GET && req.method() != Method::HEAD {
            return status(StatusCode::METHOD_NOT_ALLOWED);
        }
        let path = percent_decode(self.app_path(req.uri().path()));
        let headers = req.headers();
        let mut response = if let Some(file) = path
            .strip_prefix('/')
            .filter(|p| p.starts_with("_prism/") || p.starts_with("assets/"))
        {
            match self.inner.assets.get(file) {
                Some(f) => file_response(f, headers),
                None => status(StatusCode::NOT_FOUND),
            }
        } else if let Some(name) = path.strip_prefix("/files/") {
            self.upload(headers, name)
        } else if path == "/favicon.ico" || path == "/ws" {
            status(StatusCode::NOT_FOUND)
        } else {
            self.page_response(&path, headers)
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

    fn page_response(&self, path: &str, headers: &HeaderMap) -> Response<Vec<u8>> {
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
        let mut r = Response::new(self.page(path, visitor).into_bytes());
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

    pub(crate) fn options(&self, visitor: Visitor, path: String) -> prism_core::RuntimeOptions {
        let sid = visitor.sid;
        prism_core::RuntimeOptions {
            path,
            capabilities: crate::session::WebServices::default().capabilities(),
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

    /// Server-side render of a fresh session on `path`, as a full HTML document.
    fn page(&self, path: &str, visitor: Visitor) -> String {
        let config = &self.inner.config;
        let mut runtime =
            Runtime::with_options(&self.inner.init, self.options(visitor, path.into()));
        let mut html = HtmlBackend::default();
        html.asset_urls = Some(self.asset_urls());
        html.apply(&runtime.render());
        let title = match html.screen_title() {
            Some(screen) if screen != config.title => format!("{screen} · {}", config.title),
            _ => config.title.clone(),
        };
        let prefix = self.static_prefix();
        format!(
            r#"<!doctype html>
<html lang="{lang}" dir="{dir}" style="color-scheme: {scheme}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<link rel="stylesheet" href="{prefix}/{css}">
</head>
<body>
<main id="prism-root">{body}</main>
<div id="prism-island" role="status" aria-live="polite"></div>
<div id="prism-toasts" role="status" aria-live="polite"></div>
<script src="{prefix}/{js}" data-base="{base}"></script>
</body>
</html>"#,
            title = html::escape(&title),
            scheme = html.color_scheme(),
            lang = html::escape(&html.env.lang),
            dir = if html.env.direction == prism_core::Direction::Rtl {
                "rtl"
            } else {
                "ltr"
            },
            body = html.to_html(),
            prefix = html::escape(prefix),
            css = self.inner.assets.css(),
            js = self.inner.assets.client(),
            base = html::escape(&self.inner.base),
        )
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
fn session_id(cookie_header: &str) -> Option<String> {
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
fn time_zone(cookie_header: &str) -> Option<i32> {
    cookie_header
        .split(';')
        .find_map(|kv| kv.trim().strip_prefix("prism_tz="))
        .and_then(|v| v.parse().ok())
        .filter(|v: &i32| v.abs() <= 14 * 3600)
}

/// The last viewport the page reported (`prism_vw=1280x800`).
fn viewport_cookie(cookie_header: &str) -> Option<(f32, f32)> {
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
        let _ = std::fs::remove_dir_all(dir);
    }
}
