//! Axum integration (`server` feature, on by default): mount the app in your
//! own Axum server, or let [`run`](crate::run) serve it alone.
//!
//! ```ignore
//! let prism = prism_web::WebApp::new(config, init);
//! let app = axum::Router::new()
//!     .route("/api/health", get(|| async { "ok" }))
//!     .merge(prism.router());               // pages, /ws, /_prism, /assets, /files
//! // or under a prefix:
//! //  .nest("/app", prism.base_path("/app").router())
//! axum::serve(listener, app).await?;
//! ```
//!
//! Your middleware runs before Prism's handlers: it can refuse a request,
//! attach a [`RequestContext`](crate::RequestContext) (the signed-in user)
//! and a [`CspNonce`](crate::CspNonce) as request extensions. Both are read
//! from the page request and from every socket handshake.

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State, WebSocketUpgrade, ws::Message};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};

use crate::WebApp;

impl WebApp {
    /// An Axum router serving the app: `/ws` (live sessions) plus a fallback
    /// for pages and static files, so your own routes take precedence.
    /// `merge` it at the root, or `nest` it with [`base_path`](WebApp::base_path)
    /// set to the same prefix.
    pub fn router<S: Clone + Send + Sync + 'static>(&self) -> Router<S> {
        Router::new()
            .route("/ws", get(socket))
            .fallback(page)
            .with_state(self.clone())
    }
}

/// Pages render the app synchronously (and may touch the database), so
/// they run on the blocking pool.
async fn page(State(app): State<WebApp>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let req = http::Request::from_parts(parts, ());
    match tokio::task::spawn_blocking(move || app.handle(&req)).await {
        Ok(response) => response.map(Body::from),
        Err(_) => http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn socket(State(app): State<WebApp>, parts: Parts, ws: WebSocketUpgrade) -> Response {
    // A foreign origin (403) or too many sessions (503): no upgrade.
    if let Err(status) = app.accept_socket(&parts.headers) {
        return status.into_response();
    }
    let req = http::Request::from_parts(parts, ());
    ws.on_upgrade(move |socket| async move {
        let (mut sink, mut stream) = socket.split();
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let session = app.connect_request(&req, move |text| out_tx.send(text).is_ok());
        let writer = tokio::spawn(async move {
            while let Some(text) = out_rx.recv().await {
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        while let Some(Ok(msg)) = stream.next().await {
            match msg {
                Message::Text(text) => {
                    if !session.receive(text.as_str()) {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        drop(session);
        writer.abort();
    })
}

/// Serves `app` alone on `addr` until the process stops.
pub fn serve(app: WebApp, addr: &str) -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("prism-web")
        .build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app.router::<()>()).await
    })
}
