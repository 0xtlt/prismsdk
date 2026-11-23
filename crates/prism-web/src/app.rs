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

/// Live sessions at once unless [`WebApp::max_sessions`] says otherwise.
pub const DEFAULT_MAX_SESSIONS: usize = 1024;
/// The longest message from a page unless [`WebApp::max_message_size`]
/// says otherwise.
pub const DEFAULT_MAX_MESSAGE: usize = 8 << 20;
/// Bytes waiting in each direction of a session unless
/// [`WebApp::max_queued_bytes`] says otherwise.
pub const DEFAULT_MAX_QUEUED: usize = 32 << 20;
/// The largest picture with headers the server fetches for a page, in
/// bytes, unless [`WebApp::max_picture_bytes`] says otherwise.
pub const DEFAULT_MAX_PICTURE: u64 = 16 << 20;
/// The most files one pick, drop or clipboard read uploads unless
/// [`WebApp::max_upload_files`] says otherwise.
pub const DEFAULT_MAX_UPLOAD_FILES: usize = 256;
/// The bytes of uploaded files one browser keeps on the server unless
/// [`WebApp::max_upload_bytes`] says otherwise (1 GiB).
pub const DEFAULT_MAX_UPLOAD_BYTES: u64 = 1 << 30;
/// The bytes all sessions upload at once unless
/// [`WebApp::max_uploading_bytes`] says otherwise (4 GiB).
pub const DEFAULT_MAX_UPLOADING_BYTES: u64 = 4 << 30;
/// How long uploaded files stay on the server unless
/// [`WebApp::keep_uploads`] says otherwise (24 hours).
pub const DEFAULT_KEEP_UPLOADS: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// What the browsers may upload (S16).
#[derive(Clone, Copy, Debug)]
pub(crate) struct UploadLimits {
    /// Files in one pick, drop or clipboard read.
    pub files: usize,
    /// Bytes of files one browser keeps, those arriving included.
    pub bytes: u64,
    /// Bytes all sessions receive at once.
    pub uploading: u64,
    /// How long a received file stays.
    pub keep: std::time::Duration,
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
    /// Where browsers reach the app (`https://app.example.com`), if set:
    /// `Host` and forwarded headers are then ignored.
    pub public_origin: Option<String>,
    /// Origins (`scheme://host:port`) whose pictures and videos with
    /// headers the server may fetch on a private address.
    pub media_origins: Vec<String>,
    /// The largest picture with headers the server fetches, in bytes.
    pub max_picture: u64,
    /// At most this many live sessions at once.
    pub max_sessions: Option<usize>,
    /// The longest message a page may send, in bytes.
    pub max_message: usize,
    /// The most bytes a session lets wait in either direction: messages the
    /// session has not handled yet, frames the page has not read yet.
    pub max_queued: usize,
    /// What the browsers may upload.
    pub upload_limits: UploadLimits,
    /// Bytes being uploaded now, all sessions together.
    pub uploading: Arc<std::sync::atomic::AtomicU64>,
    /// When old uploads were last deleted.
    pub swept: std::sync::Mutex<Option<std::time::Instant>>,
    /// A session that hears nothing from its page for this long ends.
    pub idle_timeout: Option<std::time::Duration>,
    /// Live sessions now.
    pub sessions: Arc<std::sync::atomic::AtomicUsize>,
    /// How long a first render waits for the background work it started
    /// (`first_paint_wait`).
    pub first_paint_wait: std::time::Duration,
    /// 3D scene blobs go to anyone, and shared caches keep them
    /// (`public_scenes`).
    pub public_scenes: bool,

    // --- Islands: client-side Web Components ---
    /// The WASM islands `prism` built (`PRISM_ISLANDS_DIR`).
    pub islands: crate::islands::Islands,
    /// Send a strict CSP from the built-in router (`strict_csp`).
    pub strict_csp: bool,
    /// Page requests waiting for the live session that takes the page over
    /// (`cx.page_request()`).
    pub handoffs: crate::handoff::Handoffs,
    /// Origins web views may show under the strict CSP (`frame_origin`).
    pub frame_origins: Vec<String>,
    /// Sites that may show the app's pages in a frame, besides the app's
    /// own origin (`frame_ancestor`).
    pub frame_ancestors: Vec<String>,
}

/// `client.js` with what it takes from prism-core: the glyph table and the
/// 3D scenes' GLSL (`scene3d::shaders`, one copy for every backend).
pub(crate) fn client_js() -> String {
    use prism_core::scene3d::shaders;
    let glsl = |source: &str| serde_json::to_string(source).expect("a string is JSON");
    crate::CLIENT_JS
        .replace("__GLYPHS__", &prism_core::glyphs::js_table())
        .replace("__SCENE3D_VERTEX__", &glsl(shaders::VERTEX))
        .replace("__SCENE3D_FRAGMENT__", &glsl(shaders::FRAGMENT))
        .replace("__SCENE3D_SKY_VERTEX__", &glsl(shaders::SKY_VERTEX))
        .replace("__SCENE3D_SKY_FRAGMENT__", &glsl(shaders::SKY_FRAGMENT))
}

#[cfg(test)]
mod client_js_tests {
    /// The client draws 3D scenes with prism-core's GLSL, the same text as
    /// every OpenGL backend: `client.js` keeps no copy that could drift.
    #[test]
    fn the_client_carries_prism_cores_shaders() {
        use prism_core::scene3d::shaders;
        let client = super::client_js();
        for placeholder in ["__GLYPHS__", "__SCENE3D_"] {
            assert!(
                !client.contains(placeholder),
                "{placeholder} left in client.js"
            );
        }
        for glsl in [
            shaders::VERTEX,
            shaders::FRAGMENT,
            shaders::SKY_VERTEX,
            shaders::SKY_FRAGMENT,
        ] {
            assert!(client.contains(&serde_json::to_string(glsl).unwrap()));
        }
        assert!(
            !crate::CLIENT_JS.contains("precision highp"),
            "a GLSL copy in client.js"
        );
    }
}

/// Prism's client, its base stylesheet, the app's assets and its islands.
fn build_assets(
    dir: Option<PathBuf>,
    embedded: prism_core::EmbeddedAssets,
    islands: &crate::islands::Islands,
) -> Assets {
    let client = crate::assets::compact_js(&client_js());
    let css = crate::assets::compact_css(crate::PRISM_CSS);
    let mut assets = Assets::new(client, css, dir, embedded);
    assets.add_islands(islands);
    assets
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
        let islands = env("PRISM_ISLANDS_DIR")
            .map(|dir| crate::islands::Islands::load(Path::new(&dir)))
            .unwrap_or_default();
        let assets = build_assets(
            Some(PathBuf::from(assets_dir)),
            config.embedded_assets.clone(),
            &islands,
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
                public_origin: None,
                media_origins: Vec::new(),
                max_picture: DEFAULT_MAX_PICTURE,
                max_sessions: Some(DEFAULT_MAX_SESSIONS),
                max_message: DEFAULT_MAX_MESSAGE,
                max_queued: DEFAULT_MAX_QUEUED,
                upload_limits: UploadLimits {
                    files: DEFAULT_MAX_UPLOAD_FILES,
                    bytes: DEFAULT_MAX_UPLOAD_BYTES,
                    uploading: DEFAULT_MAX_UPLOADING_BYTES,
                    keep: DEFAULT_KEEP_UPLOADS,
                },
                uploading: Default::default(),
                swept: Default::default(),
                idle_timeout: None,
                sessions: Default::default(),
                first_paint_wait: std::time::Duration::ZERO,
                public_scenes: false,
                islands,
                strict_csp: env("PRISM_STRICT_CSP").is_some_and(|v| v != "0"),
                handoffs: Default::default(),
                frame_origins: Vec::new(),
                frame_ancestors: Vec::new(),
            }),
        };
        let app = match env("PRISM_BASE_PATH") {
            Some(base) => app.base_path(&base),
            None => app,
        };
        let app = match env("PRISM_PUBLIC_ORIGIN") {
            Some(origin) => app.public_origin(origin),
            None => app,
        };
        match env("PRISM_ASSET_URL") {
            Some(url) => app.asset_url(&url),
            None => app,
        }
    }

    pub(crate) fn configure(mut self, f: impl FnOnce(&mut Inner)) -> Self {
        let inner = Arc::get_mut(&mut self.inner).expect("configure a WebApp before cloning it");
        f(inner);
        self
    }

    /// The app's language (#229), instead of the browser's
    /// `Accept-Language`, for an app mounted in a host server (see
    /// `prism::Application::locale`). `PRISM_LOCALE` still wins.
    pub fn locale(self, tag: &str) -> Self {
        self.app_locale(Some(prism_core::Locale::new(tag)), Vec::new())
    }

    /// The languages the app is written in (#229): the browser's when it is
    /// one of them, else the closest, else the first one.
    pub fn supported_locales<S: AsRef<str>>(self, tags: impl IntoIterator<Item = S>) -> Self {
        let tags = tags
            .into_iter()
            .map(|t| prism_core::Locale::new(t.as_ref()))
            .collect();
        self.app_locale(None, tags)
    }

    fn app_locale(
        self,
        locale: Option<prism_core::Locale>,
        supported: Vec<prism_core::Locale>,
    ) -> Self {
        self.configure(move |i| {
            let init = i.init.clone();
            i.init = Arc::new(move |app: &mut prism_core::App| {
                app.apply_app_locale(locale.as_ref(), &supported);
                init(app)
            });
        })
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

    /// Adds app assets compiled into the binary, for a server shipped as one
    /// file: hashed and served like the assets directory's files (which they
    /// win over), exported by [`export_static`](Self::export_static), and
    /// those under `fonts/` declared as `@font-face` rules. Also
    /// `Application::embedded_assets`.
    ///
    /// ```ignore
    /// let assets = EmbeddedAssets::new()
    ///     .file("logo.png", include_bytes!("../assets/logo.png"))
    ///     .file("fonts/Inter.woff2", include_bytes!("../assets/fonts/Inter.woff2"));
    /// let app = WebApp::new(config, init).embedded_assets(assets);
    /// ```
    pub fn embedded_assets(self, assets: prism_core::EmbeddedAssets) -> Self {
        self.configure(|i| {
            let (dir, mut embedded) = i.assets.sources();
            embedded.merge(&assets);
            i.config.embedded_assets = embedded.clone();
            i.assets = build_assets(dir, embedded, &i.islands);
        })
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
    pub(crate) fn static_prefix(&self) -> &str {
        self.inner.asset_url.as_deref().unwrap_or(&self.inner.base)
    }

    /// The URL of a picture source: remote and absolute URLs as they are,
    /// app assets hashed (`/assets/logo.<hash>.png`, maybe on the CDN). A
    /// remote picture with headers (`Picture::header`) is fetched by this
    /// server: its URL is `<base>/_prism/picture/<id>` (the headers stay
    /// here). Pages bind it to their session; this one to none.
    pub fn picture_url(&self, src: &str) -> String {
        self.session_picture_url(None, src)
    }

    /// [`picture_url`](Self::picture_url) for the session `sid`: a picture
    /// with headers only loads for that browser.
    fn session_picture_url(&self, sid: Option<&str>, src: &str) -> String {
        if let Some(url) = crate::pictures::proxied(&self.inner.base, sid, src) {
            return url;
        }
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

    /// Picture URLs ([`picture_url`](Self::picture_url)) for the pages of
    /// session `sid`.
    pub(crate) fn session_asset_urls(&self, sid: &str) -> AssetUrls {
        let (app, sid) = (self.clone(), sid.to_string());
        Arc::new(move |src: &str| app.session_picture_url(Some(&sid), src))
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
    /// renders the router's not-found screen with status `404`; a screen sets
    /// another with `cx.set_page_status(code)`.
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
        // `cx.set_cookie`: the page redeems its ticket (#91).
        if req.method() == Method::POST && self.app_path(req.uri().path()) == "/_prism/cookie" {
            return crate::cookies::redeem(
                req.headers(),
                self.browser(req.headers()),
                self.public(),
                |sid| self.browser_cookie(req.headers(), sid),
            );
        }
        if req.method() != Method::GET && req.method() != Method::HEAD {
            return status(StatusCode::METHOD_NOT_ALLOWED);
        }
        let path = percent_decode(self.app_path(req.uri().path()));
        let headers = req.headers();
        let mut response = if let Some(id) = path.strip_prefix("/_prism/picture/") {
            // A picture with headers, fetched here for its session.
            crate::pictures::serve(
                id,
                self.browser(headers).as_deref(),
                &self.inner.media_origins,
                self.inner.max_picture,
            )
        } else if let Some(id) = path.strip_prefix("/_prism/media/") {
            // A video with headers (#207), in ranges.
            let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
            crate::pictures::serve_media(
                id,
                self.browser(headers).as_deref(),
                range,
                &self.inner.media_origins,
            )
        } else if let Some(id) = path.strip_prefix("/_prism/download/") {
            // A file the app handed to this session (#210).
            crate::downloads::serve(id, self.browser(headers).as_deref())
        } else if let Some(id) = path.strip_prefix("/_prism/3d/") {
            // A 3D scene's blob, for the browsers it was sent to.
            crate::scene_blobs::serve(
                id,
                self.browser(headers).as_deref(),
                self.inner.public_scenes,
            )
        } else if path == "/_prism/auth/callback" {
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
        self.protect(&mut response);
        // Pages and other answers made now: gzipped when the browser accepts it.
        crate::compress::encode_response(headers, &mut response);
        if req.method() == Method::HEAD {
            response.body_mut().clear();
        }
        response
    }

    /// What every answer says to the browser: never sniff it as another
    /// type and, for an HTML document (a page, the sign-in callback), who
    /// may show it in a frame: the app's own origin and its
    /// [`frame_ancestor`](Self::frame_ancestor)s, so another site cannot
    /// overlay a signed-in page to steer the visitor's clicks.
    fn protect(&self, r: &mut Response<Vec<u8>>) {
        let html = r
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|t| t.trim_start().to_ascii_lowercase().starts_with("text/html"));
        let h = r.headers_mut();
        h.entry(header::X_CONTENT_TYPE_OPTIONS)
            .or_insert(HeaderValue::from_static("nosniff"));
        if !html {
            return;
        }
        // A policy of its own: browsers enforce it with any other (the
        // strict CSP, a sandbox).
        let policy = format!("frame-ancestors {}", self.frame_ancestors());
        if let Ok(v) = HeaderValue::from_str(&policy) {
            h.append(header::CONTENT_SECURITY_POLICY, v);
        }
        // For browsers without `frame-ancestors`; it cannot name other sites.
        if self.inner.frame_ancestors.is_empty() {
            h.insert(
                header::X_FRAME_OPTIONS,
                HeaderValue::from_static("SAMEORIGIN"),
            );
        }
    }

    /// A file this browser uploaded (photos…), private to it.
    fn upload(&self, headers: &HeaderMap, name: &str) -> Response<Vec<u8>> {
        let Some(sid) = self.browser(headers) else {
            return status(StatusCode::NOT_FOUND);
        };
        if !crate::uploads::stored_name(name) {
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
                // Someone else's file, served from the app's origin: opened
                // as a document it runs nothing (an SVG's or HTML's
                // scripts), is never sniffed as another type, and only
                // pictures and videos show inline; anything else downloads.
                set(&mut r, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
                set(&mut r, header::CONTENT_SECURITY_POLICY, SANDBOX);
                if !inert(crate::assets::content_type(name)) {
                    set(&mut r, header::CONTENT_DISPOSITION, "attachment");
                }
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
        let origin = crate::host::page_origin(headers, Some(req.uri()), self.public());
        let request = crate::host::web_request(
            prism_core::RequestKind::Page,
            self.page_url(&origin, self.app_path(req.uri().path()), req.uri().query()),
            origin,
            headers,
            context,
        );
        let nonce = req.extensions().get::<crate::CspNonce>();
        let cookies = cookie_header(headers);
        let (sid, new) = match self.browser(headers) {
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
        // The live session of a GET takes this request over (`cx.page_request()`).
        let page_id =
            (req.method() == Method::GET).then(|| self.inner.handoffs.put(&sid, request.clone()));
        let (page, code) = self.page(path, visitor, request, page_id.as_deref(), nonce);
        let mut r = Response::new(page.into_bytes());
        *r.status_mut() = StatusCode::from_u16(code).unwrap_or(StatusCode::OK);
        set(&mut r, header::CONTENT_TYPE, "text/html; charset=utf-8");
        // Rendered for this browser: never stored by a CDN or the browser.
        set(&mut r, header::CACHE_CONTROL, "private, no-store");
        if new && let Ok(v) = HeaderValue::from_str(&self.browser_cookie(headers, &sid)) {
            r.headers_mut().insert(header::SET_COOKIE, v);
        }
        r
    }

    /// The `Set-Cookie` that gives the browser the id `sid`.
    pub(crate) fn browser_cookie(&self, headers: &HeaderMap, sid: &str) -> String {
        // HTTPS: host-only and `Secure` (`__Host-` needs `Path=/`).
        if self.https(headers) {
            format!("{SECURE_SID}={sid}; Path=/; Max-Age=31536000; Secure; SameSite=Lax; HttpOnly")
        } else {
            let path = if self.inner.base.is_empty() {
                "/"
            } else {
                &self.inner.base
            };
            format!("{PLAIN_SID}={sid}; Path={path}; Max-Age=31536000; SameSite=Lax; HttpOnly")
        }
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

    /// Whether the data directory takes files: created if missing, a probe
    /// written and removed (#290).
    pub(crate) fn data_dir_writable(&self) -> std::io::Result<()> {
        let root = &self.inner.data_root;
        std::fs::create_dir_all(root)?;
        let probe = root.join(format!(".prism-probe-{}", std::process::id()));
        std::fs::write(&probe, b"")?;
        std::fs::remove_file(&probe)
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

    /// The app's init, with the session's request set first (`cx.request()`),
    /// and the page request a live session took over (`cx.page_request()`).
    pub(crate) fn init_with(
        &self,
        request: prism_core::WebRequest,
        page_request: Option<prism_core::WebRequest>,
    ) -> AppInit {
        let init = self.inner.init.clone();
        Arc::new(move |app: &mut prism_core::App| {
            app.set_request(request.clone());
            if let Some(page) = &page_request {
                app.set_page_request(page.clone());
            }
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
    /// its HTTP status (`cx.page_status()`: the app's, or `404` on the
    /// not-found screen). `page_id` names the request its live session takes
    /// over (`data-page`, sent back on the socket).
    fn page(
        &self,
        path: &str,
        visitor: Visitor,
        request: prism_core::WebRequest,
        page_id: Option<&str>,
        nonce: Option<&crate::CspNonce>,
    ) -> (String, u16) {
        let config = &self.inner.config;
        let sid = visitor.sid;
        let asset_urls = self.session_asset_urls(sid);
        let mut runtime = Runtime::with_options(
            &self.init_with(request, None),
            self.options(visitor, path.into()),
        );
        let mut html = HtmlBackend::default();
        html.asset_urls = Some(asset_urls);
        html.base.clone_from(&self.inner.base);
        html.apply(&runtime.render_settled(self.inner.first_paint_wait));
        // Run-time fonts (by URL) and stylesheets of the first paint; the
        // live page registers them again (`client.js` keeps these).
        let (mut fonts, mut sheets) = (Vec::new(), String::new());
        for request in runtime.take_requests() {
            if let prism_core::PlatformRequest::LoadFont(font) = request {
                fonts.push(crate::session::font_at(font, &|src| self.picture_url(src)));
            }
        }
        for url in runtime.app.stylesheets() {
            sheets.push_str(&format!(
                "<link rel=\"stylesheet\" href=\"{}\" data-prism-sheet{}>\n",
                html::escape(&url),
                crate::host::nonce_attr(nonce),
            ));
        }
        let code = runtime.app.page_status();
        // The app's own title (`cx.set_title`, #154), else the top screen's.
        let shown = runtime
            .app
            .title()
            .map(str::to_string)
            .or_else(|| html.screen_title());
        let title = match shown {
            Some(screen) if screen != config.title => format!("{screen} · {}", config.title),
            _ => config.title.clone(),
        };
        let prefix = self.static_prefix();
        let html::PageHtml {
            root: body,
            layers,
            css: ssr_css,
        } = html.to_page();
        crate::scene_blobs::first_paint(sid, html.take_scenes());
        let head = &self.inner.head;
        let page = format!(
            r#"<!doctype html>
<html lang="{lang}" dir="{dir}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<link rel="stylesheet" href="{prefix}/{css}"{n}>
{app_head}{sheets}<style id="prism-page"{n}>:root{{color-scheme:{scheme}}}
{app_css}{font_css}</style>
<style id="prism-ssr"{n}>{ssr_css}</style>
</head>
<body>
<main id="prism-root">{body}</main>
<div id="prism-island" role="status" aria-live="polite"></div>
<div id="prism-toasts" role="status" aria-live="polite"></div>
<div id="prism-layers" popover="manual">{layers}</div>
<div id="prism-connection" role="status" aria-live="polite" hidden>{reconnecting}</div>
<script src="{prefix}/{js}" data-base="{base}"{page}{n}></script>
</body>
</html>"#,
            n = crate::host::nonce_attr(nonce),
            app_head = head.render(nonce, &|src: &str| self.picture_url(src)),
            app_css = crate::host::css_text(&head.css),
            font_css = crate::session::font_face_css(&fonts),
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
            page = page_id.map_or(String::new(), |id| format!(
                r#" data-page="{}""#,
                html::escape(id)
            )),
        );
        (page, code)
    }

    /// What a socket request tells about the browser: session cookie,
    /// language, time zone, window size, device, and the page's path.
    pub(crate) fn socket_visitor(
        &self,
        uri: &Uri,
        headers: &HeaderMap,
    ) -> (String, String, VisitorOwned) {
        let param = |k: &str| query_param(uri, k);
        let num = |k: &str| param(k).and_then(|v| v.parse::<f32>().ok());
        let cookies = cookie_header(headers);
        let header = |name: header::HeaderName| headers.get(name).and_then(|v| v.to_str().ok());
        let sid = self.browser(headers).unwrap_or_else(new_session_id);
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
    /// proxy, set [`public_origin`](Self::public_origin).
    pub fn allow_origins<I, S>(self, origins: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let origins: Vec<String> = origins.into_iter().map(Into::into).collect();
        self.configure(|i| i.allowed_origins.extend(origins))
    }

    /// Lets the server fetch pictures and videos with headers
    /// (`picture(url).header(…)`) from `origin` (`https://api.internal`,
    /// `http://10.0.0.5:8080`) although it is on a private, loopback or
    /// link-local address. Other origins must resolve to public addresses
    /// only; redirects to another origin are never followed.
    /// The origin browsers reach the app at (`https://app.example.com`),
    /// for a server behind a proxy (also `PRISM_PUBLIC_ORIGIN`). It is then
    /// the page's origin (`cx.request()`, sign-in callbacks, whether the
    /// browser's id cookie is `Secure`) and, with
    /// [`allow_origins`](Self::allow_origins), the only origin a live
    /// session accepts: `Host`, `X-Forwarded-Host`, `X-Forwarded-Proto` and
    /// `Forwarded` are ignored. Without it they are trusted, so a proxy in
    /// front must set them (and drop the browser's own).
    ///
    /// # Panics
    ///
    /// If `origin` is not `http(s)://host[:port]`.
    pub fn public_origin(self, origin: impl AsRef<str>) -> Self {
        let (scheme, host, port) = crate::host::origin_parts(origin.as_ref())
            .unwrap_or_else(|| panic!("public_origin: {:?} is not an origin", origin.as_ref()));
        let default = if scheme == "https" { 443 } else { 80 };
        let origin = if port == default {
            format!("{scheme}://{host}")
        } else {
            format!("{scheme}://{host}:{port}")
        };
        self.configure(|i| i.public_origin = Some(origin))
    }

    pub(crate) fn public(&self) -> Option<&str> {
        self.inner.public_origin.as_deref()
    }

    pub fn trust_media_origin(self, origin: impl AsRef<str>) -> Self {
        let origin = crate::pictures::origin(origin.as_ref());
        self.configure(|i| i.media_origins.extend(origin))
    }

    /// The largest picture with headers (`picture(url).header(..)`) the
    /// server fetches for a page, in bytes: a larger one is refused with
    /// `502` as soon as it passes the limit. 16 MiB by default
    /// ([`DEFAULT_MAX_PICTURE`]). Videos and audio with headers are
    /// fetched in ranges of 4 MiB and need no limit.
    pub fn max_picture_bytes(self, bytes: u64) -> Self {
        self.configure(|i| i.max_picture = bytes)
    }

    /// At most `n` live sessions at once (each is a thread with its own
    /// runtime): further handshakes are refused with `503`, and the pages
    /// retry with back-off. 1,024 by default ([`DEFAULT_MAX_SESSIONS`];
    /// `usize::MAX` for no limit). Each session takes its permit when the
    /// handshake is checked and frees it when it ends.
    pub fn max_sessions(self, n: usize) -> Self {
        self.configure(|i| i.max_sessions = Some(n))
    }

    /// The longest message a page may send, in bytes: a longer one ends
    /// its session (the Axum router also refuses longer WebSocket frames).
    /// 8 MiB by default ([`DEFAULT_MAX_MESSAGE`]); picked files go in
    /// chunks of 256 KiB, so only a huge paste comes near it.
    pub fn max_message_size(self, bytes: usize) -> Self {
        self.configure(|i| i.max_message = bytes)
    }

    /// The most bytes a session lets wait, in each direction: messages from
    /// a page faster than the app handles them, and (with the Axum router)
    /// patches for a page that stops reading. Past it the session ends and
    /// the page reconnects. 32 MiB by default ([`DEFAULT_MAX_QUEUED`]).
    pub fn max_queued_bytes(self, bytes: usize) -> Self {
        self.configure(|i| i.max_queued = bytes)
    }

    /// The most files one pick, drop or clipboard read may upload: more
    /// fail the call before anything is stored. 256 by default
    /// ([`DEFAULT_MAX_UPLOAD_FILES`]). A pick of a single file takes one,
    /// and `PickImages::limit` applies too.
    pub fn max_upload_files(self, n: usize) -> Self {
        self.configure(|i| i.upload_limits.files = n)
    }

    /// The most bytes of uploaded files one browser keeps on the server,
    /// those arriving included: an upload that would pass it fails before
    /// anything is stored. 1 GiB by default ([`DEFAULT_MAX_UPLOAD_BYTES`]).
    pub fn max_upload_bytes(self, bytes: u64) -> Self {
        self.configure(|i| i.upload_limits.bytes = bytes)
    }

    /// The most bytes all sessions upload at once: an upload that would
    /// pass it fails ("try again later") before anything is stored. 4 GiB
    /// by default ([`DEFAULT_MAX_UPLOADING_BYTES`]).
    pub fn max_uploading_bytes(self, bytes: u64) -> Self {
        self.configure(|i| i.upload_limits.uploading = bytes)
    }

    /// How long uploaded files stay on the server: older ones are deleted
    /// (and `PickedFile::read` no longer finds them), those of browsers
    /// that never come back included, after a restart too. 24 hours by
    /// default ([`DEFAULT_KEEP_UPLOADS`]). Copy the files the app keeps to
    /// its own storage.
    pub fn keep_uploads(self, keep: std::time::Duration) -> Self {
        self.configure(|i| i.upload_limits.keep = keep)
    }

    pub(crate) fn upload_limits(&self) -> UploadLimits {
        self.inner.upload_limits
    }

    pub(crate) fn uploading(&self) -> &Arc<std::sync::atomic::AtomicU64> {
        &self.inner.uploading
    }

    /// Deletes the expired uploads of every browser on a background thread,
    /// at most every [`keep_uploads`](Self::keep_uploads) (once a minute to
    /// once an hour).
    pub(crate) fn sweep_uploads(&self) {
        let every = self.inner.upload_limits.keep.clamp(
            std::time::Duration::from_secs(60),
            std::time::Duration::from_secs(3600),
        );
        let now = std::time::Instant::now();
        {
            let Ok(mut swept) = self.inner.swept.lock() else {
                return;
            };
            if swept.is_some_and(|at| now.duration_since(at) < every) {
                return;
            }
            *swept = Some(now);
        }
        let app = self.clone();
        let _ = std::thread::Builder::new()
            .name("prism-web uploads".into())
            .spawn(move || crate::uploads::sweep_all(&app, std::time::SystemTime::now()));
    }

    /// Ends a session that heard nothing from its page (no event, resize or
    /// visibility change) for `timeout`. The page keeps its content and
    /// reconnects (to a fresh session) at the next interaction.
    pub fn idle_timeout(self, timeout: std::time::Duration) -> Self {
        self.configure(|i| i.idle_timeout = Some(timeout))
    }

    /// Lets the first paint wait up to `timeout` for the data it loads.
    ///
    /// A screen that loads with `cx.spawn` + `cx.background` (HTTP, a
    /// database, files), from `init`, its constructor or `render`, otherwise
    /// ships its loading state in the server-rendered page and gets its data
    /// once the socket connects. With a wait, the page is rendered after
    /// that work ends (each step it starts is waited for too), or at
    /// `timeout` with what is still loading as today. The live session's
    /// first tree waits the same way, so the page does not go back to its
    /// loading state when the socket connects. Timers and channels are not
    /// waited for. Zero (the default) renders at once.
    pub fn first_paint_wait(self, timeout: std::time::Duration) -> Self {
        self.configure(|i| i.first_paint_wait = timeout)
    }

    /// Serves the blobs of 3D scenes (meshes, instance sets, clips, embedded
    /// images: `<base>/_prism/3d/<id>`) to anyone, and lets shared caches
    /// such as a CDN keep them. Only for an app whose scenes hold nothing
    /// private. By default (`false`) a blob goes only to the browsers it
    /// was sent to, with `Cache-Control: private`.
    pub fn public_scenes(self, public: bool) -> Self {
        self.configure(|i| i.public_scenes = public)
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
        if !crate::host::origin_allowed(headers, &self.inner.allowed_origins, self.public()) {
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

    /// [`accept_socket`](WebApp::accept_socket), taking the session's
    /// permit at once: concurrent handshakes never get more permits than
    /// [`max_sessions`](WebApp::max_sessions).
    pub(crate) fn reserve_session(
        &self,
        headers: &HeaderMap,
    ) -> Result<crate::session::SessionPermit, StatusCode> {
        if !crate::host::origin_allowed(headers, &self.inner.allowed_origins, self.public()) {
            return Err(StatusCode::FORBIDDEN);
        }
        crate::session::SessionPermit::take(&self.inner.sessions, self.inner.max_sessions)
            .ok_or(StatusCode::SERVICE_UNAVAILABLE)
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
/// Gzipped (once, then cached) when the browser accepts it and the type is
/// worth it; that form has its own ETag (`"<hash>-gzip"`).
fn file_response(f: StaticFile, headers: &HeaderMap) -> Response<Vec<u8>> {
    use crate::compress;
    let varies = compress::worth_it(f.content_type, f.body.len());
    let gzip = varies && compress::accepts_gzip(headers);
    let etag = if gzip {
        compress::gzip_etag(&f.etag)
    } else {
        f.etag.clone()
    };
    let mut r = if compress::etag_matches(headers, &f.etag) {
        status(StatusCode::NOT_MODIFIED)
    } else {
        let body = if gzip {
            compress::gzip_static(&f.etag, &f.body, f.cache_control == crate::IMMUTABLE)
        } else {
            f.body
        };
        let mut r = Response::new(body);
        set(&mut r, header::CONTENT_TYPE, f.content_type);
        if gzip {
            set(&mut r, header::CONTENT_ENCODING, "gzip");
        }
        r
    };
    set(&mut r, header::CACHE_CONTROL, f.cache_control);
    if varies {
        compress::vary(&mut r);
    }
    if let Ok(v) = HeaderValue::from_str(&etag) {
        r.headers_mut().insert(header::ETAG, v);
    }
    r
}

/// The policy of a file the app did not make (an upload): no scripts, no
/// subresources, an opaque origin, whatever the page's own policy is.
pub(crate) const SANDBOX: &str = "default-src 'none'; sandbox";

/// Whether a browser shows `content_type` without running anything: raster
/// pictures and videos (not SVG, HTML, PDF…).
fn inert(content_type: &str) -> bool {
    (content_type.starts_with("image/") && content_type != "image/svg+xml")
        || content_type.starts_with("video/")
}

fn cookie_header(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::COOKIE).and_then(|v| v.to_str().ok())
}

/// The browser's id cookie over HTTPS: host-only, `Secure`, `Path=/`.
/// Browsers refuse a `__Host-` cookie from a sibling domain or plain HTTP,
/// so no other site can choose the id (session fixation).
pub(crate) const SECURE_SID: &str = "__Host-prism_sid";
/// Over plain HTTP (local development): `Path` is the mount point.
pub(crate) const PLAIN_SID: &str = "prism_sid";

impl WebApp {
    /// Whether the browser reached the app over HTTPS: its public origin
    /// says so, else `X-Forwarded-Proto` or `Forwarded: proto=` from the
    /// proxy in front.
    fn https(&self, headers: &HeaderMap) -> bool {
        match self.public() {
            Some(origin) => origin.starts_with("https://"),
            None => crate::host::request_scheme(headers, None) == "https",
        }
    }

    /// The browser's id ([`browser_id`]).
    pub(crate) fn browser(&self, headers: &HeaderMap) -> Option<String> {
        browser_id(headers, self.https(headers))
    }
}

/// The browser's id: its `__Host-prism_sid` cookie, or over plain HTTP its
/// `prism_sid`. Over HTTPS a plain `prism_sid` is ignored: a sibling site
/// may have set it.
pub(crate) fn browser_id(headers: &HeaderMap, https: bool) -> Option<String> {
    let cookies = cookie_header(headers)?;
    cookie_id(cookies, SECURE_SID).or_else(|| {
        if https {
            None
        } else {
            cookie_id(cookies, PLAIN_SID)
        }
    })
}

/// The id in cookie `name` of a Cookie header, if well-formed.
fn cookie_id(cookie_header: &str, name: &str) -> Option<String> {
    cookie_header
        .split(';')
        .filter_map(|kv| kv.trim().strip_prefix(name)?.strip_prefix('='))
        .find(|v| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_string)
}

/// A random 128-bit id, from the operating system's CSPRNG.
pub(crate) fn new_session_id() -> String {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .expect("the system random number generator");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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

/// The value of `key` in `uri`'s query, still escaped (`path=%2F` → `%2F`).
pub(crate) fn query_param<'a>(uri: &'a Uri, key: &str) -> Option<&'a str> {
    uri.query()?
        .split('&')
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
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
    use crate::temp_dir::TempDir;
    use prism_core::prelude::*;

    struct Page;
    impl Render for Page {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(text("Hello"))
                .child(picture("logo.png").alt("Logo"))
        }
    }

    fn app(tag: &str) -> (WebApp, TempDir) {
        let dir = TempDir::new(&format!("webapp-{tag}"));
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
        let (app, _dir) = app("cache");
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
    }

    #[test]
    fn embedded_assets_need_no_directory() {
        // No assets directory at all: the pictures come from the binary.
        let dir = TempDir::new("webapp-embed");
        let config = AppConfig {
            title: "Test".into(),
            assets: Some(dir.join("missing").to_string_lossy().into()),
            embedded_assets: prism_core::EmbeddedAssets::new().file("logo.png", &b"old"[..]),
            ..Default::default()
        };
        let init: AppInit = Arc::new(|cx: &mut App| {
            let root = cx.new_entity(|_| Page);
            cx.set_root(root);
        });
        let app = WebApp::new(config, init)
            .data_dir(dir.join("data"))
            .embedded_assets(prism_core::EmbeddedAssets::new().file("logo.png", &b"png!"[..]));
        let html = String::from_utf8(get(&app, "/", &[]).body().clone()).unwrap();
        let logo = format!("/assets/logo.{}.png", crate::content_hash(b"png!"));
        assert!(html.contains(&logo), "{html}");
        let image = get(&app, &logo, &[]);
        assert_eq!(
            (image.body().as_slice(), header(&image, "cache-control")),
            (&b"png!"[..], crate::IMMUTABLE)
        );
        assert_eq!(header(&image, "content-type"), "image/png");
        let plain = get(&app, "/assets/logo.png", &[]);
        assert_eq!(
            (plain.body().as_slice(), header(&plain, "cache-control")),
            (&b"png!"[..], crate::REVALIDATE)
        );
        assert_eq!(app.config().embedded_assets.len(), 1);
        let out = dir.join("public");
        app.export_static(&out).unwrap();
        assert_eq!(std::fs::read(out.join(&logo[1..])).unwrap(), b"png!");
    }

    #[test]
    fn mounted_under_a_prefix_and_on_a_cdn() {
        let (app, _dir) = app("base");
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
    }

    #[test]
    fn pictures_with_headers_are_fetched_by_the_server_for_their_session() {
        use std::io::{Read, Write};
        static UPSTREAM: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let ok = String::from_utf8_lossy(&buf[..n]).contains("Bearer s3cret");
            let _ = s.write_all(if ok {
                b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 4\r\n\r\npng!"
            } else {
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"
            });
        });
        UPSTREAM.get_or_init(|| format!("http://{addr}/me.png"));
        struct Avatar;
        impl Render for Avatar {
            fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
                picture(UPSTREAM.get().unwrap().as_str()).header("Authorization", "Bearer s3cret")
            }
        }
        let config = AppConfig {
            title: "Test".into(),
            ..Default::default()
        };
        let init: AppInit = Arc::new(|cx: &mut App| {
            let root = cx.new_entity(|_| Avatar);
            cx.set_root(root);
        });
        let dir = TempDir::new("webapp-pic");
        // The upstream is on loopback: the app trusts it.
        let app = WebApp::new(config, init)
            .data_dir(dir.join("data"))
            .base_path("/app")
            .trust_media_origin(format!("http://{addr}"));
        let page = get(
            &app,
            "/app/",
            &[("cookie", "prism_sid=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")],
        );
        let html = String::from_utf8(page.body().clone()).unwrap();
        assert!(!html.contains("s3cret"), "{html}");
        let src = html
            .split("src=\"/app/_prism/picture/")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .map(|id| format!("/app/_prism/picture/{id}"))
            .expect("a proxied picture");
        // Another browser cannot load it, even with the URL.
        let other = get(
            &app,
            &src,
            &[("cookie", "prism_sid=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")],
        );
        assert_eq!(other.status(), StatusCode::NOT_FOUND);
        let image = get(
            &app,
            &src,
            &[("cookie", "prism_sid=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")],
        );
        assert_eq!(image.status(), StatusCode::OK);
        assert_eq!(image.body().as_slice(), b"png!");
        assert_eq!(header(&image, "content-type"), "image/png");
    }

    #[test]
    fn push_needs_a_vapid_key() {
        let (app, _dir) = app("push");
        assert_eq!(app.services().capabilities().push, None);
        let app = app.vapid_public_key("BKVqC4g3");
        assert_eq!(
            app.services().capabilities().push,
            Some(prism_core::PushService::WebPush)
        );
    }

    #[test]
    fn blobs_of_a_first_paint_stay_fetchable() {
        use prism_core::scene3d::{Instances, cube};
        struct Crowd;
        impl Render for Crowd {
            fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
                // Built during the render: nothing else holds this blob.
                let set = Instances::grid([3, 1, 3], [1., 1., 1.]);
                prism_core::scene3d()
                    .child(cube(0.5).name("crowd").instances(&set))
                    .h(px(100.))
            }
        }
        let init: AppInit = Arc::new(|cx: &mut App| {
            let root = cx.new_entity(|_| Crowd);
            cx.set_root(root);
        });
        let dir = TempDir::new("webapp-3d");
        let app = WebApp::new(AppConfig::default(), init.clone()).data_dir(dir.join("data"));
        let page = get(&app, "/", &[]);
        let cookie = header(&page, "set-cookie")
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let page = String::from_utf8(page.body().clone()).unwrap();
        let scene = page
            .split("data-scene=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .expect("a scene")
            .replace("&quot;", "\"");
        let id = scene
            .split("\"inst\":{\"id\":\"")
            .nth(1)
            .and_then(|s| s.get(..32))
            .unwrap_or_else(|| panic!("an instance blob in {scene}"));
        let path = format!("/_prism/3d/{id}");
        let blob = get(&app, &path, &[("cookie", &cookie)]);
        assert_eq!(blob.status(), StatusCode::OK);
        assert!(blob.body().starts_with(b"P3DI"));
        assert_eq!(
            header(&blob, "cache-control"),
            "private, max-age=31536000, immutable"
        );
        // S29: another browser, or none, gets nothing, even with the id.
        let other = format!("prism_sid={}", "ab".repeat(16));
        assert_eq!(
            get(&app, &path, &[("cookie", &other)]).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(get(&app, &path, &[]).status(), StatusCode::NOT_FOUND);
        // An app whose scenes are public serves them to anyone, and caches
        // may keep them.
        let public = WebApp::new(AppConfig::default(), init)
            .data_dir(dir.join("public"))
            .public_scenes(true);
        let _ = get(&public, "/", &[]);
        let blob = get(&public, &path, &[]);
        assert_eq!(blob.status(), StatusCode::OK);
        assert_eq!(
            header(&blob, "cache-control"),
            "public, max-age=31536000, immutable"
        );
    }
}
