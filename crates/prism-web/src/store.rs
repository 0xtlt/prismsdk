//! In-app purchases on the web (#306): there is no store in a browser, so
//! the app sells through its own [`Store`](prism_core::Store) (the host's
//! payment provider: Stripe, Paddle, Lemon Squeezy…), which runs on the
//! server like the rest of the app. Prism adds what a checkout page needs:
//!
//! - a **return URL** of the server, `<origin><base>/_prism/store/return?state=…`,
//!   given to [`Store::purchase`](prism_core::Store::purchase): the checkout
//!   page opens in a popup and sends the user back there, which hands the
//!   URL (with what the provider added) to the session that opened it, by
//!   its `state`, once, and closes the popup;
//! - [`entitlements_changed`]: the host's webhook says a buyer's purchases
//!   changed, and the sessions of that buyer read them again.

use std::time::Instant;

use crate::security::Redirect;

/// The return URL of a new checkout for the app at `origin` + `base`, with
/// a state of its own.
pub(crate) fn return_url(origin: &str, base: &str) -> String {
    let state = crate::security::random_id();
    format!("{origin}{base}/_prism/store/return?state={state}")
}

/// The state of `url` if it is a return URL of this app (see
/// [`return_url`]): a checkout may only come back to the session's own.
pub(crate) fn return_state(url: &str, origin: &str, base: &str) -> Option<String> {
    let query = url.strip_prefix(&format!("{origin}{base}/_prism/store/return?"))?;
    crate::security::query_map(query.split('#').next()?)
        .remove("state")
        .filter(|s| !s.is_empty())
}

/// Whether a checkout page's URL may open: `https://` (or `http://` for a
/// test provider on this machine).
pub(crate) fn checkout_url_allowed(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("https://")
        || ["http://localhost", "http://127.0.0.1", "http://[::1]"]
            .iter()
            .any(|p| {
                lower
                    .strip_prefix(p)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/', '?', '#']))
            })
}

/// `GET <base>/_prism/store/return?…`: hands the URL to its session.
pub(crate) fn return_page(query: &str, nonce: Option<&crate::CspNonce>) -> String {
    let state = crate::security::query_map(query)
        .remove("state")
        .unwrap_or_default();
    let found = crate::security::take(Redirect::Checkout, &state, Instant::now());
    let delivered = found.is_some_and(|(tx, id)| {
        tx.send(crate::session::Inbound::StoreReturn(
            id,
            format!("/_prism/store/return?{query}"),
        ))
        .is_ok()
    });
    let message = if delivered {
        "Back to the app. You can close this window."
    } else {
        "This checkout has expired. Close this window and try again from the app."
    };
    // No `style` attribute, and the host's nonce: a strict CSP allows it.
    let n = crate::host::nonce_attr(nonce);
    format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>Checkout</title>\
<style{n}>p{{font:16px system-ui;margin:40px;text-align:center}}</style>\
<p>{message}</p>\
<script{n}>if (window.opener) window.close();</script>"
    )
}

/// Tells this process's live sessions that the purchases of the buyer
/// `buyer_id` changed (a payment's webhook, a refund): those whose store
/// says their buyer has this id ([`Store::buyer_id`](prism_core::Store::buyer_id))
/// read their entitlements again, and their pages re-render. Returns how
/// many sessions were told (each checks its own buyer).
///
/// ```ignore
/// // Axum: Stripe's webhook, once its signature is checked.
/// async fn stripe_webhook(event: StripeEvent) -> StatusCode {
///     db::grant(&event.user_id, &event.product).await;
///     prism::web::entitlements_changed(&event.user_id);
///     StatusCode::OK
/// }
/// ```
///
/// Sessions in other server processes are not told: run it in each (a
/// message bus), or they see the change when they reconnect.
pub fn entitlements_changed(buyer_id: &str) -> usize {
    crate::session::tell_sessions(|| crate::session::Inbound::EntitlementsChanged(buyer_id.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_urls_belong_to_the_app() {
        let url = return_url("https://shop.example", "/app");
        let state = return_state(&url, "https://shop.example", "/app").unwrap();
        assert_eq!(
            url,
            format!("https://shop.example/app/_prism/store/return?state={state}")
        );
        // What a provider adds stays out of the state.
        let back = format!("{url}&session_id=cs_test_1#x");
        assert_eq!(
            return_state(&back, "https://shop.example", "/app"),
            Some(state)
        );
        assert_eq!(return_state(&url, "https://evil.example", "/app"), None);
        assert_eq!(return_state(&url, "https://shop.example", ""), None);
        assert_eq!(
            return_state(
                "https://shop.example/app/_prism/store/return?state=",
                "https://shop.example",
                "/app"
            ),
            None
        );
        assert_ne!(return_url("https://a", ""), return_url("https://a", ""));
    }

    #[test]
    fn only_https_checkouts_open() {
        assert!(checkout_url_allowed(
            "https://checkout.stripe.com/c/pay/cs_test"
        ));
        assert!(checkout_url_allowed("http://localhost:4242/pay"));
        assert!(checkout_url_allowed("http://127.0.0.1/pay"));
        assert!(!checkout_url_allowed("http://localhost.evil.example/pay"));
        assert!(!checkout_url_allowed("http://shop.example/pay"));
        assert!(!checkout_url_allowed("javascript:alert(1)"));
        assert!(!checkout_url_allowed("/relative"));
    }
}
