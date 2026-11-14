//! Prism as the rendering layer of an existing Axum server: the API stays in
//! Axum, the UI is a Prism app mounted at `/app`.
//!
//! ```text
//! cargo run -p axum-server        # http://127.0.0.1:3100/app, /api/orders
//! ```

use axum::Json;
use axum::response::Redirect;
use axum::routing::get;
use prism::prelude::*;

/// Shared with the API: in a real server, a database.
const ORDERS: &[(u32, &str)] = &[(41, "Keyboard"), (42, "Monitor"), (43, "Desk lamp")];

struct Orders;

impl Render for Orders {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen("Orders").child(
            div()
                .p(20.)
                .gap(12.)
                .child(text("Rendered by Prism inside an Axum server."))
                .children(ORDERS.iter().map(|(id, name)| {
                    button(format!("#{id} · {name}")).on_click(navigate_to(format!("/orders/{id}")))
                })),
        )
    }
}

struct Order {
    id: u32,
}

impl Render for Order {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        let name = ORDERS
            .iter()
            .find(|(id, _)| *id == self.id)
            .map_or("Unknown", |(_, n)| n);
        screen(format!("Order #{}", self.id)).child(
            div()
                .p(20.)
                .child(text(format!("{name}, shipped tomorrow."))),
        )
    }
}

#[tokio::main]
async fn main() {
    let prism = Application::new()
        .title("Shop")
        .web_app(|cx| {
            Router::new()
                .route("/", |cx, _| cx.new_entity(|_| Orders))
                .route("/orders/:id", |cx, p| {
                    let id = p.parse("id").unwrap_or(0);
                    cx.new_entity(move |_| Order { id })
                })
                .build(cx)
        })
        .base_path("/app");

    let app = axum::Router::new()
        .route("/", get(|| async { Redirect::to("/app") }))
        .route(
            "/api/orders",
            get(|| async {
                Json(serde_json::json!(
                    ORDERS
                        .iter()
                        .map(|(id, name)| serde_json::json!({ "id": id, "name": name }))
                        .collect::<Vec<_>>()
                ))
            }),
        )
        .nest("/app", prism.router());

    let port = std::env::var("PORT").unwrap_or_else(|_| "3100".into());
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .expect("bind");
    println!("http://127.0.0.1:{port}/app  ·  http://127.0.0.1:{port}/api/orders");
    axum::serve(listener, app).await.expect("serve");
}
