//! In-app purchases on the web (#306), through a live page: the app's
//! store opens a checkout page, which comes back to the server; a webhook
//! updates the buyer's other sessions.

use std::sync::{Arc, Mutex};

use prism_core::prelude::*;
use prism_core::{Buyer, Checkout, DeviceError, Purchase, Store};
use serde_json::{Value, json};

use crate::tests::{Page, op_with_text, web_app};

/// A payment provider whose checkout page carries the return URL in its
/// query, as Stripe's `success_url` would.
#[derive(Default)]
struct Pay {
    paid: Mutex<Vec<String>>,
}

impl Store for Pay {
    fn products(&self, _: &Buyer, ids: &[String]) -> Result<Vec<Product>, DeviceError> {
        Ok(ids.iter().map(|id| Product::new(id, id, "9 €")).collect())
    }
    fn entitlements(&self, _: &Buyer) -> Result<Vec<String>, DeviceError> {
        Ok(self.paid.lock().unwrap().clone())
    }
    fn purchase(
        &self,
        _: &Buyer,
        p: &Purchase,
        return_url: Option<&str>,
    ) -> Result<Checkout, DeviceError> {
        let back = return_url.ok_or(DeviceError::Unavailable)?;
        Ok(Checkout::Open(format!(
            "https://pay.example/{}?back={}",
            p.product,
            prism_core::encode_query([("x", back)]).trim_start_matches("x=")
        )))
    }
    fn checkout_returned(
        &self,
        _: &Buyer,
        product: &str,
        url: &str,
    ) -> Result<prism_core::PurchaseOutcome, DeviceError> {
        // The provider's own proof that it was paid.
        if url.contains("paid=1") {
            self.paid.lock().unwrap().push(product.into());
            Ok(prism_core::PurchaseOutcome::Purchased)
        } else {
            Ok(prism_core::PurchaseOutcome::Pending)
        }
    }
    fn buyer_id(&self, _: &Buyer) -> Option<String> {
        Some("ada".into())
    }
}

#[derive(Default)]
struct Shop {
    started: bool,
    outcome: String,
}

impl Render for Shop {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if !std::mem::replace(&mut self.started, true) {
            cx.spawn(async move |this, cx| {
                let r = cx.purchase("pro").await;
                cx.update_entity(&this, |s, _| s.outcome = format!("{r:?}"));
            });
        }
        div()
            .child(text(format!("<{}>", self.outcome)))
            .child(text(if cx.owns("pro") { "unlocked" } else { "locked" }))
    }
}

fn shop(pay: Arc<Pay>, tag: &str) -> (crate::WebApp, crate::temp_dir::TempDir) {
    let init: prism_core::AppInit = Arc::new(move |cx: &mut App| {
        cx.set_store(pay.clone());
        let root = cx.new_entity(|_| Shop::default());
        cx.set_root(root);
    });
    web_app(init, tag)
}

/// The next `store_checkout` device call of the page.
fn checkout(page: &Page) -> Value {
    loop {
        let m = page.next();
        if m["t"] == "device" {
            assert_eq!(m["call"], "store_checkout", "{m}");
            return m;
        }
    }
}

/// The path and query of the return URL a checkout page carries.
fn return_path(call: &Value) -> String {
    let url = call["url"].as_str().unwrap();
    let back = url.split_once("back=").unwrap().1;
    let back = crate::app::percent_decode(back);
    let at = back.find("/_prism/").unwrap();
    back[at..].to_string()
}

fn get(app: &crate::WebApp, path: &str) -> String {
    let req = http::Request::get(path).body(()).unwrap();
    String::from_utf8(app.handle(&req).into_body()).unwrap()
}

#[test]
fn a_checkout_comes_back_to_its_session() {
    let pay = Arc::new(Pay::default());
    let (app, _dir) = shop(pay.clone(), "store-return");
    let page = Page::open(&app, "/ws?path=%2F&w=1280&h=800");
    let reset = page.next();
    let outcome = op_with_text(&reset["p"], "<>");
    let locked = op_with_text(&reset["p"], "locked");
    let call = checkout(&page);
    assert_eq!(call["provider"], "pay.example");
    let back = return_path(&call);
    assert!(back.starts_with("/_prism/store/return?state="), "{back}");

    // A sign-in callback with the checkout's state completes nothing.
    let state = back.split_once("state=").unwrap().1;
    let page_html = get(&app, &format!("/_prism/auth/callback?state={state}"));
    assert!(page_html.contains("expired"), "{page_html}");
    // The page claiming the checkout came back is ignored too.
    page.send(
        json!({ "t": "device_reply", "id": call["id"], "text": "https://evil.example/?paid=1" }),
    );

    // The provider sends the user back, paid.
    let html = get(&app, &format!("{back}&paid=1"));
    assert!(html.contains("Back to the app"), "{html}");
    let mut texts = std::collections::HashMap::new();
    while texts.len() < 2 {
        let m = page.next();
        for p in m["p"].as_array().into_iter().flatten() {
            if p["op"] == "text" {
                texts.insert(p["id"].as_u64().unwrap(), p["text"].clone());
            }
        }
    }
    assert_eq!(texts[&outcome], "<Ok(Purchased)>");
    assert_eq!(texts[&locked], "unlocked");
    // Once only.
    assert!(get(&app, &format!("{back}&paid=1")).contains("expired"));
}

#[test]
fn a_closed_checkout_is_cancelled_and_a_webhook_unlocks() {
    let pay = Arc::new(Pay::default());
    let (app, _dir) = shop(pay.clone(), "store-webhook");
    let page = Page::open(&app, "/ws?path=%2F&w=1280&h=800");
    let reset = page.next();
    let outcome = op_with_text(&reset["p"], "<>");
    let locked = op_with_text(&reset["p"], "locked");
    let call = checkout(&page);
    // The user closed the popup.
    page.send(json!({ "t": "device_reply", "id": call["id"], "err": "cancelled" }));
    assert_eq!(
        page.next(),
        json!({ "t": "patch", "p": [{ "op": "text", "id": outcome, "text": "<Ok(Cancelled)>" }] })
    );
    let back = return_path(&call);
    assert!(
        get(&app, &back).contains("expired"),
        "a cancelled checkout is forgotten"
    );

    // They paid in another tab; the provider's webhook tells the app.
    pay.paid.lock().unwrap().push("pro".into());
    assert!(crate::entitlements_changed("grace") >= 1);
    assert!(crate::entitlements_changed("ada") >= 1);
    assert_eq!(
        page.next(),
        json!({ "t": "patch", "p": [{ "op": "text", "id": locked, "text": "unlocked" }] })
    );
}

#[test]
fn the_first_paint_knows_what_the_user_owns() {
    let pay = Arc::new(Pay::default());
    pay.paid.lock().unwrap().push("pro".into());
    let (app, _dir) = shop(pay, "store-paint");
    let html = get(&app, "/");
    assert!(html.contains("unlocked"), "{html}");
}
