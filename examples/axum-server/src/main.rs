//! Prism as the rendering layer of an existing Axum server: the API stays in
//! Axum, the UI is a Prism app mounted at `/app`.
//!
//! The app draws its own chrome (`Router::header(false)`): a title bar that
//! stays put while the list scrolls (`scroll().flex_1()` in a window-sized
//! frame), and a filter kept in the URL (`/app/?status=shipped`), so it can
//! be bookmarked and Back undoes it.
//!
//! ```text
//! cargo run -p axum-server        # http://127.0.0.1:3100/app, /api/orders
//! ```

use axum::Json;
use axum::response::Redirect;
use axum::routing::get;
use prism::Div;
use prism::prelude::*;

/// Shared with the API: in a real server, a database.
const ORDERS: &[(u32, &str)] = &[
    (41, "Keyboard"),
    (42, "Monitor"),
    (43, "Desk lamp"),
    (44, "Chair"),
    (45, "Headphones"),
    (46, "Webcam"),
    (47, "Microphone"),
    (48, "Dock"),
    (49, "Cable set"),
    (50, "Mouse"),
    (51, "Notebook"),
    (52, "Standing mat"),
    (53, "Speakers"),
    (54, "Tablet stand"),
    (55, "Lamp bulb"),
];

/// Every other order has shipped.
fn shipped(id: u32) -> bool {
    id.is_multiple_of(2)
}

const FILTERS: [(&str, &str); 3] = [
    ("all", "All"),
    ("shipped", "Shipped"),
    ("pending", "Pending"),
];

/// The app's own title bar (the stack's header is turned off).
fn title_bar(title: &str, back: bool) -> Div {
    div()
        .flex_row()
        .items_center()
        .gap(12.)
        .px(20.)
        .py(14.)
        .bg(adaptive(rgb(0xF4F4F8), rgb(0x1C1C22)))
        .when(back, |d| d.child(button("‹ Orders").on_click(go_back())))
        .child(text(title).text_size(20.).bold())
}

struct Orders {
    status: String,
}

impl Render for Orders {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = ORDERS.iter().filter(|(id, _)| match self.status.as_str() {
            "shipped" => shipped(*id),
            "pending" => !shipped(*id),
            _ => true,
        });
        screen("Orders")
            .child(
                title_bar("Orders", false).child(div().flex_row().gap(6.).children(
                    FILTERS.iter().map(|(key, label)| {
                        let selected = self.status == *key;
                        button(if selected {
                            format!("• {label}")
                        } else {
                            label.to_string()
                        })
                        .on_click(cx.listener(move |this, _, cx| {
                            this.status = key.to_string();
                            // The URL follows the filter, without a new screen.
                            cx.replace_query([("status", *key)]);
                        }))
                    }),
                )),
            )
            .child(
                // Only this part scrolls; the title bar stays put.
                scroll()
                    .flex_1()
                    .p(20.)
                    .gap(10.)
                    .child(text("Rendered by Prism inside an Axum server."))
                    .children(rows.map(|(id, name)| {
                        let state = if shipped(*id) { "shipped" } else { "pending" };
                        button(format!("#{id} · {name} · {state}"))
                            .on_click(navigate_to(format!("/orders/{id}")))
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
        let when = if shipped(self.id) {
            "shipped"
        } else {
            "ships tomorrow"
        };
        screen(format!("Order #{}", self.id))
            .child(title_bar(&format!("Order #{}", self.id), true))
            .child(div().p(20.).child(text(format!("{name}, {when}."))))
    }
}

#[tokio::main]
async fn main() {
    let prism = Application::new()
        .title("Shop")
        .web_app(|cx| {
            Router::new()
                .header(false)
                .route("/", |cx, p| {
                    let status = p.query("status").unwrap_or("all").to_string();
                    cx.new_entity(|_| Orders { status })
                })
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
