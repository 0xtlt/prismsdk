//! In-app purchases (#306): the platform's store through device calls, the
//! app's own store (`set_store`) on any platform, the web's checkout.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prism_core::prelude::*;
use prism_core::{
    AppInit, Buyer, Capabilities, Checkout, DeviceCall, DeviceError, DeviceEvent, DeviceValue,
    PlatformRequest, Purchase, Runtime, RuntimeOptions, Storage, Store, TestStore, WebRequest,
};

/// A storage two runs share (the previous run's answer).
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<HashMap<String, String>>>);

impl Storage for Shared {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&mut self, key: &str, value: &str) {
        self.0.lock().unwrap().insert(key.into(), value.into());
    }
    fn remove(&mut self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
    fn keys(&self) -> Vec<String> {
        self.0.lock().unwrap().keys().cloned().collect()
    }
}

/// Shows whether the user owns `pro`; keeps what it rendered last.
struct Shop {
    shown: Arc<Mutex<String>>,
}

impl Render for Shop {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = if cx.owns("pro") { "unlocked" } else { "locked" };
        *self.shown.lock().unwrap() = label.into();
        text(label)
    }
}

type Log = Arc<Mutex<Vec<String>>>;

fn runtime(
    storage: Shared,
    purchases: bool,
    setup: impl Fn(&mut App) + Send + Sync + 'static,
) -> (Runtime, Arc<Mutex<String>>) {
    let shown: Arc<Mutex<String>> = Default::default();
    let r = shown.clone();
    let init: AppInit = Arc::new(move |cx: &mut App| {
        setup(cx);
        let root = cx.new_entity(|_| Shop { shown: r.clone() });
        cx.set_root(root);
    });
    let rt = Runtime::with_options(
        &init,
        RuntimeOptions {
            path: "/".into(),
            capabilities: Capabilities {
                purchases,
                ..Default::default()
            },
            storage: Some(Box::new(storage)),
            ..Default::default()
        },
    );
    (rt, shown)
}

fn device_calls(rt: &mut Runtime) -> Vec<(u64, DeviceCall)> {
    rt.take_requests()
        .into_iter()
        .filter_map(|r| match r {
            PlatformRequest::Device { id, call } => Some((id, call)),
            _ => None,
        })
        .collect()
}

fn shows(shown: &Arc<Mutex<String>>, s: &str) -> bool {
    *shown.lock().unwrap() == s
}

/// Polls until the background work of the app's store is done.
fn settle(rt: &mut Runtime) {
    rt.render_settled(Duration::from_secs(5));
    rt.poll();
}

fn spawn_logged<F: std::future::Future<Output = String> + 'static>(
    rt: &mut Runtime,
    log: &Log,
    f: impl FnOnce(AsyncApp) -> F + 'static,
) {
    let log = log.clone();
    rt.app.spawn(async move |cx| {
        let line = f(cx).await;
        log.lock().unwrap().push(line);
    });
}

#[test]
fn the_platform_store_is_watched_once_the_app_asks() {
    let storage = Shared::default();
    let (mut rt, shown) = runtime(storage.clone(), true, |_| {});
    rt.render();
    let calls = device_calls(&mut rt);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].1, DeviceCall::WatchEntitlements);
    assert!(shows(&shown, "locked"));
    assert!(!rt.app.entitlements_confirmed());
    let watch = calls[0].0;

    // The store answers: the app re-renders and keeps it for the next run.
    let changes: Log = Default::default();
    let c = changes.clone();
    rt.app.on_entitlements(move |app| {
        c.lock().unwrap().push(app.entitlements().join(","));
    });
    rt.device_event(watch, DeviceEvent::Entitlements(vec!["pro".into()]));
    rt.poll();
    assert!(shows(&shown, "unlocked"));
    assert!(rt.app.entitlements_confirmed());
    assert_eq!(*changes.lock().unwrap(), ["pro"]);
    assert!(device_calls(&mut rt).is_empty(), "watched once");
    // The same answer again changes nothing.
    rt.device_event(watch, DeviceEvent::Entitlements(vec!["pro".into()]));
    assert_eq!(changes.lock().unwrap().len(), 1);

    // Next launch: owned before the store answers.
    let (mut next, next_shown) = runtime(storage.clone(), true, |_| {});
    next.render();
    assert!(shows(&next_shown, "unlocked"));
    assert!(!next.app.entitlements_confirmed());
    // A refund: gone.
    let watch = device_calls(&mut next)[0].0;
    next.device_event(watch, DeviceEvent::Entitlements(vec![]));
    next.poll();
    assert!(shows(&next_shown, "locked"));
}

#[test]
fn no_store_no_watch() {
    let (mut rt, _) = runtime(Shared::default(), false, |_| {});
    rt.render();
    assert!(device_calls(&mut rt).is_empty());
    assert!(!rt.app.capabilities().purchases);
}

#[test]
fn purchases_through_the_platform_store() {
    let (mut rt, shown) = runtime(Shared::default(), true, |_| {});
    rt.render();
    device_calls(&mut rt);
    let log: Log = Default::default();

    spawn_logged(&mut rt, &log, async move |cx| {
        format!("{:?}", cx.store_products(&["themes", "pro", "gone"]).await)
    });
    spawn_logged(&mut rt, &log, async move |cx| {
        format!("{:?}", cx.purchase("pro").await)
    });
    spawn_logged(&mut rt, &log, async move |cx| {
        format!("{:?}", cx.purchase("themes").await)
    });
    spawn_logged(&mut rt, &log, async move |cx| {
        let p = Purchase::new("pro").account_token("user-42");
        format!("{:?}", cx.purchase_with(p).await)
    });
    rt.poll();
    let calls = device_calls(&mut rt);
    assert_eq!(
        calls.len(),
        3,
        "the bad token never reaches the store: {calls:?}"
    );
    assert_eq!(
        calls[0].1,
        DeviceCall::StoreProducts(vec!["themes".into(), "pro".into(), "gone".into()])
    );
    assert_eq!(calls[1].1, DeviceCall::Purchase(Purchase::new("pro")));
    // In the store's order; the app gets them in its own.
    let products = Product::list_to_json(&[
        Product::new("pro", "Pro", "$4.99"),
        Product::new("themes", "Themes", "$0.99"),
    ]);
    rt.device_reply(calls[0].0, Ok(DeviceValue::Text(Some(products))));
    rt.device_reply(calls[1].0, Ok(DeviceValue::Text(Some("purchased".into()))));
    rt.device_reply(calls[2].0, Err(DeviceError::Cancelled));
    rt.poll();
    let log = log.lock().unwrap().clone();
    assert!(log[0].contains("not a UUID"), "{log:?}");
    assert!(log[1].contains(r#"id: "themes""#) && log[1].find("themes") < log[1].find(r#""pro""#));
    assert_eq!(log[2], "Ok(Purchased)");
    assert_eq!(log[3], "Ok(Cancelled)");
    // Verified by the store: owned at once.
    assert!(rt.app.owns("pro"));
    assert!(shows(&shown, "unlocked"));
}

#[test]
fn restoring_answers_what_is_owned() {
    let (mut rt, _) = runtime(Shared::default(), true, |_| {});
    rt.render();
    device_calls(&mut rt);
    let log: Log = Default::default();
    spawn_logged(&mut rt, &log, async move |cx| {
        format!("{:?}", cx.restore_purchases().await)
    });
    rt.poll();
    let calls = device_calls(&mut rt);
    assert_eq!(calls[0].1, DeviceCall::RestorePurchases);
    rt.device_reply(
        calls[0].0,
        Ok(DeviceValue::Text(Some(r#"["pro","themes"]"#.into()))),
    );
    rt.poll();
    assert_eq!(log.lock().unwrap()[0], r#"Ok(["pro", "themes"])"#);
    assert!(rt.app.owns("themes") && rt.app.entitlements_confirmed());
}

#[test]
fn the_apps_own_store_answers_before_the_first_render() {
    let store = Arc::new(
        TestStore::new()
            .product(Product::new("pro", "Pro", "$1"))
            .owned(["pro"]),
    );
    let s = store.clone();
    let (mut rt, shown) = runtime(Shared::default(), false, move |cx| cx.set_store(s.clone()));
    rt.render();
    assert!(shows(&shown, "unlocked"));
    assert!(rt.app.capabilities().purchases);
    assert!(rt.app.entitlements_confirmed());
    // Nothing for the backend: no watch, no device call.
    assert!(device_calls(&mut rt).is_empty());
}

#[test]
fn a_test_store_sells_at_once() {
    let store = Arc::new(
        TestStore::new()
            .product(Product::new("pro", "Pro", "$1"))
            .product(Product::new("themes", "Themes", "$2")),
    );
    let s = store.clone();
    let (mut rt, shown) = runtime(Shared::default(), true, move |cx| cx.set_store(s.clone()));
    rt.render();
    assert!(shows(&shown, "locked"));
    let log: Log = Default::default();
    spawn_logged(&mut rt, &log, async move |cx| {
        let products = cx.store_products(&["themes", "pro"]).await.unwrap();
        let ids: Vec<String> = products.into_iter().map(|p| p.id).collect();
        let bought = cx.purchase("pro").await;
        format!("{ids:?} {bought:?}")
    });
    settle(&mut rt);
    assert_eq!(log.lock().unwrap()[0], r#"["themes", "pro"] Ok(Purchased)"#);
    assert!(shows(&shown, "unlocked"));
    assert!(
        device_calls(&mut rt).is_empty(),
        "the platform's store is not used"
    );
}

/// A host's payment provider: a checkout page, then a webhook.
#[derive(Default)]
struct Provider {
    paid: Mutex<Vec<String>>,
    asked: Mutex<Vec<String>>,
}

struct User(String);

impl Store for Provider {
    fn products(&self, _: &Buyer, ids: &[String]) -> Result<Vec<Product>, DeviceError> {
        Ok(ids.iter().map(|id| Product::new(id, id, "9 €")).collect())
    }
    fn entitlements(&self, buyer: &Buyer) -> Result<Vec<String>, DeviceError> {
        match buyer.context::<User>() {
            Some(u) if u.0 == "ada" => Ok(self.paid.lock().unwrap().clone()),
            _ => Ok(Vec::new()),
        }
    }
    fn purchase(
        &self,
        buyer: &Buyer,
        p: &Purchase,
        return_url: Option<&str>,
    ) -> Result<Checkout, DeviceError> {
        let user = buyer.context::<User>().ok_or(DeviceError::Denied)?;
        self.asked
            .lock()
            .unwrap()
            .push(format!("{} {} {return_url:?}", user.0, p.product));
        Ok(Checkout::Open(format!(
            "https://pay.example/c/{}",
            p.product
        )))
    }
    fn buyer_id(&self, buyer: &Buyer) -> Option<String> {
        buyer.context::<User>().map(|u| u.0.clone())
    }
}

fn signed_in(user: &str) -> WebRequest {
    WebRequest {
        url: "https://shop.example/app".into(),
        origin: "https://shop.example".into(),
        context: Some(Arc::new(User(user.into()))),
        ..Default::default()
    }
}

#[test]
fn a_web_checkout_comes_back_then_the_webhook_confirms() {
    let provider = Arc::new(Provider::default());
    let p = provider.clone();
    let (mut rt, shown) = runtime(Shared::default(), false, move |cx| {
        cx.set_request(signed_in("ada"));
        cx.set_store(p.clone());
    });
    rt.render();
    assert!(shows(&shown, "locked"));
    let log: Log = Default::default();
    spawn_logged(&mut rt, &log, async move |cx| {
        format!("{:?}", cx.purchase("pro").await)
    });
    rt.poll();
    let calls = device_calls(&mut rt);
    assert_eq!(calls, [(calls[0].0, DeviceCall::StoreReturnUrl)]);
    let back = "https://shop.example/app/_prism/store/return?state=s1";
    rt.device_reply(calls[0].0, Ok(DeviceValue::Text(Some(back.into()))));
    settle(&mut rt);
    let calls = device_calls(&mut rt);
    assert_eq!(
        calls[0].1,
        DeviceCall::StoreCheckout {
            url: "https://pay.example/c/pro".into(),
            return_url: back.into(),
        }
    );
    assert_eq!(
        provider.asked.lock().unwrap()[0],
        format!("ada pro Some({back:?})")
    );
    // Back before the webhook: pending.
    rt.device_reply(
        calls[0].0,
        Ok(DeviceValue::Text(Some(format!("{back}&session_id=cs_1")))),
    );
    settle(&mut rt);
    assert_eq!(log.lock().unwrap()[0], "Ok(Pending)");
    assert!(shows(&shown, "locked"));

    // The webhook: another buyer's changes nothing, ada's unlocks.
    provider.paid.lock().unwrap().push("pro".into());
    rt.app.entitlements_changed(Some("grace"));
    settle(&mut rt);
    assert!(shows(&shown, "locked"));
    rt.app.entitlements_changed(Some("ada"));
    settle(&mut rt);
    assert!(shows(&shown, "unlocked"));
}

#[test]
fn a_closed_checkout_is_cancelled_and_natively_the_browser_takes_it() {
    let provider = Arc::new(Provider::default());
    let p = provider.clone();
    let (mut rt, _) = runtime(Shared::default(), false, move |cx| {
        cx.set_request(signed_in("ada"));
        cx.set_store(p.clone());
    });
    rt.render();
    let log: Log = Default::default();
    spawn_logged(&mut rt, &log, async move |cx| {
        format!("{:?}", cx.purchase("pro").await)
    });
    rt.poll();
    let id = device_calls(&mut rt)[0].0;
    rt.device_reply(
        id,
        Ok(DeviceValue::Text(Some(
            "https://shop.example/_prism/store/return?state=s".into(),
        ))),
    );
    settle(&mut rt);
    let id = device_calls(&mut rt)[0].0;
    rt.device_reply(id, Err(DeviceError::Cancelled));
    settle(&mut rt);
    assert_eq!(log.lock().unwrap()[0], "Ok(Cancelled)");

    // A native app with its own store: no return URL, the browser opens.
    let provider = Arc::new(Provider::default());
    let p = provider.clone();
    let (mut native, _) = runtime(Shared::default(), false, move |cx| cx.set_store(p.clone()));
    native.render();
    let log: Log = Default::default();
    spawn_logged(&mut native, &log, async move |cx| {
        format!("{:?}", cx.purchase("pro").await)
    });
    settle(&mut native);
    // No signed-in user natively: this provider refuses.
    assert_eq!(log.lock().unwrap()[0], "Err(Denied)");
}

/// A native store whose checkout is a web page.
struct Licenses;

impl Store for Licenses {
    fn products(&self, _: &Buyer, _: &[String]) -> Result<Vec<Product>, DeviceError> {
        Ok(Vec::new())
    }
    fn entitlements(&self, _: &Buyer) -> Result<Vec<String>, DeviceError> {
        Ok(Vec::new())
    }
    fn purchase(
        &self,
        _: &Buyer,
        _: &Purchase,
        url: Option<&str>,
    ) -> Result<Checkout, DeviceError> {
        assert_eq!(url, None, "no return URL natively");
        Ok(Checkout::Open("https://licenses.example/buy".into()))
    }
}

#[test]
fn natively_a_checkout_page_opens_in_the_browser() {
    let (mut rt, _) = runtime(Shared::default(), false, |cx| {
        cx.set_store(Arc::new(Licenses))
    });
    rt.render();
    let log: Log = Default::default();
    spawn_logged(&mut rt, &log, async move |cx| {
        format!("{:?}", cx.purchase("pro").await)
    });
    settle(&mut rt);
    assert_eq!(log.lock().unwrap()[0], "Ok(Pending)");
    let calls = device_calls(&mut rt);
    assert_eq!(
        calls.iter().map(|c| &c.1).collect::<Vec<_>>(),
        [&DeviceCall::OpenUrl("https://licenses.example/buy".into())]
    );
}
