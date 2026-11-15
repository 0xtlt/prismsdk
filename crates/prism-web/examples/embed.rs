//! Prism inside an existing Axum server that signs people in itself.
//!
//! ```sh
//! cargo run -p prism-web --example embed     # then open http://127.0.0.1:4000
//! ```
//!
//! The host owns `/`, `/signin`, `/signout` and its middleware: it refuses
//! the signed-out (a redirect for pages, `401` for the socket), attaches the
//! user and a CSP nonce to each request, and sends a strict
//! `Content-Security-Policy`. Prism is nested at `/app`.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use prism_core::prelude::*;
use prism_core::{AppConfig, AppInit};
use prism_web::{CspNonce, RequestContext, WebApp};

/// Who the host signed in.
struct User {
    name: String,
}

struct Dashboard {
    clicks: u32,
}

impl Render for Dashboard {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let user = cx
            .request_context::<User>()
            .map_or("nobody".to_string(), |u| u.name.clone());
        let origin = cx.page_origin().unwrap_or("-").to_string();
        let card = || {
            div()
                .flex_col()
                .gap(10.)
                .p(16.)
                .rounded(14.)
                .bg(rgb(0x1c2230))
                .border(1.)
                .border_color(rgb(0x2c3446))
        };
        screen("Dashboard").child(
            div()
                .flex_col()
                .gap(16.)
                .p(24.)
                .child(
                    text(format!("Signed in as {user}"))
                        .text_size(22.)
                        .semibold()
                        .text_color(rgb(0xf2f4f8)),
                )
                .child(
                    text(format!("Served at {origin}"))
                        .text_size(14.)
                        .text_color(rgb(0x9aa4b8)),
                )
                .child(
                    card()
                        .child(
                            text(format!("Clicks: {}", self.clicks))
                                .text_size(18.)
                                .text_color(rgb(0xf2f4f8)),
                        )
                        .child(
                            div()
                                .flex_row()
                                .gap(8.)
                                .child(
                                    button("+1")
                                        .px(16.)
                                        .py(8.)
                                        .rounded(8.)
                                        .bg(rgb(0x5856d6))
                                        .text_color(rgb(0xffffff))
                                        .on_click(cx.listener(|this, _, cx| {
                                            this.clicks += 1;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    button("Sign out")
                                        .px(16.)
                                        .py(8.)
                                        .rounded(8.)
                                        .bg(rgb(0x2c3446))
                                        .text_color(rgb(0xf2f4f8))
                                        // Same origin: the host's page, in this tab.
                                        .on_click(cx.listener(|_, _, cx| cx.open_url("/signout"))),
                                ),
                        ),
                ),
        )
    }
}

fn prism() -> WebApp {
    let init: AppInit = Arc::new(|cx: &mut App| {
        let router = Router::new()
            .route("/", |cx, _| cx.new_entity(|_| Dashboard { clicks: 0 }))
            .build(cx);
        cx.set_root(router);
    });
    let config = AppConfig {
        title: "Dashboard".into(),
        ..Default::default()
    };
    WebApp::new(config, init)
        .base_path("/app")
        .theme_color("#101418")
        .meta("apple-mobile-web-app-title", "Dashboard")
        .body_background("#101418")
        .font_family("ui-rounded, system-ui, sans-serif")
        .style(".prism-header { background: #151a24; color: #f2f4f8; }")
        .max_sessions(1000)
        .idle_timeout(std::time::Duration::from_secs(30 * 60))
}

/// `host_session=<name>` from the cookies.
fn signed_in(req: &Request) -> Option<String> {
    let cookies = req.headers().get(header::COOKIE)?.to_str().ok()?;
    cookies
        .split(';')
        .find_map(|kv| kv.trim().strip_prefix("host_session="))
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// The host's sign-in check and CSP, around Prism.
async fn host_layer(mut req: Request, next: Next) -> Response {
    let Some(name) = signed_in(&req) else {
        // The socket gets a status; pages go to the sign-in page.
        if req.headers().contains_key(header::UPGRADE) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        return Redirect::to("/signin").into_response();
    };
    let nonce = CspNonce::random();
    req.extensions_mut()
        .insert(RequestContext::new(User { name }));
    req.extensions_mut().insert(nonce.clone());
    let mut res = next.run(req).await;
    let csp = format!(
        "default-src 'self'; style-src 'self' 'nonce-{0}'; script-src 'self' 'nonce-{0}'; \
         img-src 'self' data:; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'",
        nonce.0
    );
    if let Ok(v) = csp.parse() {
        res.headers_mut().insert("content-security-policy", v);
    }
    res
}

const PAGE: &str = "<!doctype html><meta charset=utf-8><title>Host</title>";

fn main() {
    let runtime = tokio::runtime::Runtime::new().expect("a tokio runtime");
    runtime.block_on(serve());
}

async fn serve() {
    let app = axum::Router::new()
        .route(
            "/",
            get(|| async {
                Html(format!(
                    "{PAGE}<p>The host's home. <a href=/app>Dashboard</a>"
                ))
            }),
        )
        .route(
            "/signin",
            get(|| async {
                Html(format!(
                    "{PAGE}<h1>Sign in</h1><a href=/signin/ada>Sign in as Ada</a>"
                ))
            }),
        )
        .route(
            "/signin/ada",
            get(|| async {
                (
                    [(
                        header::SET_COOKIE,
                        "host_session=Ada; Path=/; HttpOnly; SameSite=Lax",
                    )],
                    Redirect::to("/app"),
                )
            }),
        )
        .route(
            "/signout",
            get(|| async {
                (
                    [(header::SET_COOKIE, "host_session=; Path=/; Max-Age=0")],
                    Html(format!(
                        "{PAGE}<h1>Signed out</h1><a href=/signin>Sign in again</a>"
                    )),
                )
            }),
        )
        .nest(
            "/app",
            prism().router().layer(middleware::from_fn(host_layer)),
        );
    let addr = std::env::var("ADDR").unwrap_or_else(|_| "127.0.0.1:4000".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    println!("host on http://{addr}");
    axum::serve(listener, app).await.unwrap();
}
