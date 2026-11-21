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
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    let (mut parts, _) = req.into_parts();
    // `strict_csp`: a fresh nonce and the policy, unless the host's own
    // middleware already put a nonce on the request.
    let csp =
        (app.has_strict_csp() && parts.extensions.get::<crate::CspNonce>().is_none()).then(|| {
            let nonce = crate::CspNonce::random();
            let policy = app.content_security_policy(&nonce);
            parts.extensions.insert(nonce);
            policy
        });
    let req = http::Request::from_parts(parts, ());
    match tokio::task::spawn_blocking(move || app.handle(&req)).await {
        Ok(response) => {
            let mut response = response.map(Body::from);
            // Another policy, which browsers enforce with the answer's own:
            // an upload, a proxied picture or a download stays sandboxed, a
            // page keeps its framing policy.
            if let Some(policy) = csp.and_then(|p| http::HeaderValue::from_str(&p).ok()) {
                response
                    .headers_mut()
                    .append(http::header::CONTENT_SECURITY_POLICY, policy);
            }
            response
        }
        Err(_) => http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// The Axum router pings every live page this often; a page that sends
/// nothing back (not even the browser's automatic pong) for
/// [`SILENT_PEER`] is gone, and its session ends.
const PING_EVERY: std::time::Duration = std::time::Duration::from_secs(30);
const SILENT_PEER: std::time::Duration = std::time::Duration::from_secs(75);

async fn socket(State(app): State<WebApp>, parts: Parts, ws: WebSocketUpgrade) -> Response {
    // A foreign origin (403) or too many sessions (503): no upgrade. The
    // permit is taken now and moves into the session.
    let permit = match app.reserve_session(&parts.headers) {
        Ok(permit) => permit,
        Err(status) => return status.into_response(),
    };
    let max_message = app.inner.max_message;
    let max_queued = app.inner.max_queued;
    let req = http::Request::from_parts(parts, ());
    ws.max_message_size(max_message)
        .max_frame_size(max_message)
        .on_upgrade(move |socket| async move {
            let (mut sink, mut stream) = socket.split();
            let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<crate::Frame>();
            // Bytes sent to the writer and not written yet: a page that
            // stops reading ends its session past `max_queued_bytes`.
            let unsent = Arc::new(AtomicUsize::new(0));
            let written = unsent.clone();
            // Large messages deflated for pages that can inflate them, on the
            // session's thread (not the async runtime's).
            let compress = crate::Frame::negotiated(req.uri());
            let session = app.connect_reserved(&req, permit, move |text| {
                let frame = crate::Frame::encode(text, compress);
                let n = frame.len();
                if unsent.fetch_add(n, Ordering::SeqCst) + n > max_queued {
                    return false;
                }
                out_tx.send(frame).is_ok()
            });
            let mut writer = tokio::spawn(async move {
                let mut ping = tokio::time::interval(PING_EVERY);
                ping.tick().await;
                loop {
                    tokio::select! {
                        frame = out_rx.recv() => {
                            let Some(frame) = frame else { break };
                            let n = frame.len();
                            let msg = match frame {
                                crate::Frame::Text(text) => Message::Text(text.into()),
                                crate::Frame::Binary(bytes) => Message::Binary(bytes.into()),
                            };
                            let sent = sink.send(msg).await;
                            written.fetch_sub(n, Ordering::SeqCst);
                            if sent.is_err() {
                                break;
                            }
                        }
                        _ = ping.tick() => {
                            if sink.send(Message::Ping(Default::default())).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                let _ = sink.close().await;
            });
            loop {
                tokio::select! {
                    msg = tokio::time::timeout(SILENT_PEER, stream.next()) => match msg {
                        Ok(Some(Ok(Message::Text(text)))) => {
                            if !session.receive(text.as_str()) {
                                break;
                            }
                        }
                        // Pongs and pings say the page is there.
                        Ok(Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_)))) => {}
                        // Closed, broken, or silent for too long.
                        _ => break,
                    },
                    // The writer stopped (the page is gone, or stopped
                    // reading): stop reading too, so the session ends now.
                    _ = &mut writer => break,
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
        let mut router = app.router::<()>();
        // On a loopback address, only local names (or a proxy here): a
        // page whose name was pointed at 127.0.0.1 is refused.
        if listener.local_addr()?.ip().is_loopback() {
            let public = app.public().map(str::to_string);
            router = router.layer(axum::middleware::from_fn(
                move |req: Request, next: axum::middleware::Next| {
                    let allowed =
                        crate::host::loopback_host_allowed(req.headers(), public.as_deref());
                    async move {
                        if allowed {
                            next.run(req).await
                        } else {
                            (http::StatusCode::MISDIRECTED_REQUEST, "Unknown host").into_response()
                        }
                    }
                },
            ));
        }
        axum::serve(listener, router).await
    })
}
