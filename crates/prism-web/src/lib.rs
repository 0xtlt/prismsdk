//! # prism-web
//!
//! Runs a Prism app on the web, LiveView-style:
//!
//! 1. A page request renders the app on the server and returns plain HTML
//!    (usable before any JS runs), linking the hashed client script and CSS.
//! 2. The client opens `/ws`; the server starts a session (its own
//!    [`Runtime`](prism_core::Runtime)) and streams patches as JSON; the
//!    client applies them to the DOM. Large messages are deflated for
//!    browsers that can inflate them ([`Frame`]); pages and static files
//!    are gzipped when `Accept-Encoding` allows it.
//! 3. DOM events go back over the socket and are dispatched to Rust handlers.
//!
//! No WASM: the app logic stays in native Rust on the server.
//!
//! ## Modular
//!
//! - [`WebApp`] is the app as a rendering service: [`WebApp::handle`]
//!   (pages, static files) and [`WebApp::connect`] (live sessions) work with
//!   any HTTP server through the `http` crate's types.
//! - `server` feature (default): [`WebApp::router`] mounts it in Axum next to
//!   your API (`merge`, or `nest` with [`WebApp::base_path`]); [`run`] serves
//!   it alone.
//!
//! ## CDN
//!
//! `/_prism/*` and `/assets/*` have content hashes in their names and are
//! served `immutable`; pages are `private, no-store`.
//! [`WebApp::export_static`] writes the static files for a CDN and
//! [`WebApp::asset_url`] (or `PRISM_ASSET_URL`) points pages at it.

#![cfg_attr(docsrs, feature(doc_cfg))]

mod app;
mod assets;
mod compress;
mod cookies;
mod css;
mod device_info;
mod downloads;
mod fonts;
mod handoff;
mod host;
#[cfg(test)]
mod host_tests;
use prism_core::glyphs;
mod html;
mod islands;
mod navbar;
mod pictures;
mod security;
#[cfg(test)]
mod security_tests;
#[cfg(feature = "server")]
mod server;
mod session;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod typography_tests;
mod uploads;

pub use app::WebApp;
pub use assets::{IMMUTABLE, REVALIDATE, StaticFile, content_hash};
pub use compress::Frame;
pub use css::css;
pub use host::{CspNonce, RequestContext};
pub use html::{AssetUrls, HtmlBackend, PageHtml};
pub use islands::ModuleUrls;
#[cfg(feature = "server")]
pub use server::serve;
pub use session::{Encoder, Session};

use prism_core::{AppConfig, AppInit};

pub(crate) const CLIENT_JS: &str = include_str!("client.js");
pub(crate) const PRISM_CSS: &str = include_str!("prism.css");
/// The service worker for Web Push (`<base>/prism-sw.js`).
pub(crate) const SW_JS: &str = include_str!("sw.js");

/// Serves the app alone on `config.port` (`PRISM_PORT`, `PRISM_HOST` override
/// it; the host defaults to 127.0.0.1). `PRISM_EXPORT_STATIC=<dir>` writes
/// the hashed static files for a CDN instead, then returns.
pub fn run(config: AppConfig, init: AppInit) {
    run_app(WebApp::new(config, init))
}

/// [`run`] for an app already configured ([`WebApp::favicon`],
/// [`WebApp::max_sessions`]…).
pub fn run_app(app: WebApp) {
    if let Ok(dir) = std::env::var("PRISM_EXPORT_STATIC") {
        match app.export_static(std::path::Path::new(&dir)) {
            Ok(n) => println!("prism-web: wrote {n} static files to {dir}"),
            Err(e) => eprintln!("prism-web: cannot export to {dir}: {e}"),
        }
        return;
    }
    let port = std::env::var("PRISM_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(app.config().port);
    let host = std::env::var("PRISM_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let addr = format!("{host}:{port}");
    println!(
        "prism-web: serving \"{}\" on http://{addr}{}",
        app.config().title,
        app.base()
    );
    #[cfg(feature = "server")]
    if let Err(e) = serve(app, &addr) {
        panic!("prism-web: cannot serve on {addr}: {e}");
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (app, addr);
        eprintln!("prism-web: built without the `server` feature: mount WebApp in your server");
    }
}
