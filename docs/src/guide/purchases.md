# In-app purchases

One-time unlocks (non-consumables: "Pro", a theme pack) sold through the platform's store: the App Store on Apple platforms, Google Play on Android. On the web and on desktops without a store, the app plugs in its own [`Store`](#your-own-store-the-web-desktops), usually the host's payment provider. The same calls work everywhere:

```rust
cx.spawn(async move |this, cx| {
    // Names and prices for the buy button, localized, in the user's currency.
    let products = cx.store_products(&["pro"]).await?;    // Vec<Product>: id, name, description, price
    match cx.purchase("pro").await? {
        PurchaseOutcome::Purchased => {}                   // cx.owns("pro") is now true
        PurchaseOutcome::Pending => {}                     // Ask to Buy, a slow payment: it arrives later
        PurchaseOutcome::Cancelled => {}
    }
    let owned = cx.restore_purchases().await?;             // the Restore button
});

// In render, in a widget, anywhere: what the user owns.
if cx.owns("pro") { /* … */ }
cx.entitlements();                                         // every owned id
// A purchase on another device, a refund, an approved Ask to Buy.
cx.on_entitlements(|app| { /* … */ });
```

`cx.owns` answers at launch: Prism keeps the store's last answer between runs, so the first frame already shows the unlocked screens. It then asks the store, and keeps listening while the app runs. Every change re-renders the windows and reloads the widgets (they render in the app, so `cx.owns` works in them too). `cx.entitlements_confirmed()` says whether the store has answered since launch.

Errors are `DeviceError`s: `Unavailable` where there is no store (or the Play Store app is missing), `Denied` where purchases are turned off (Screen Time), and the store's own errors with their name (`DeviceError::Platform`, such as Play's `NETWORK_ERROR`).

`purchase_with(Purchase::new("pro").account_token(uuid))` ties the purchase to your user (a UUID). It becomes StoreKit's `appAccountToken` and Play's obfuscated account id, which your server finds in the transaction.

| | Store | Products, purchase | Owned | Changes |
|---|---|---|---|---|
| iOS, iPadOS, macOS, visionOS, tvOS, watchOS | StoreKit 2 | `Product.products`, `purchase(confirmIn:)` | `Transaction.currentEntitlements`, verified, not revoked | `Transaction.updates` |
| Android | Google Play Billing 9 (`[store] enabled`) | `queryProductDetailsAsync`, `launchBillingFlow` | `queryPurchasesAsync`, acknowledged by Prism | `onPurchasesUpdated`, each return to the app |
| Web | the app's `Store` (none: `Unavailable`) | a checkout page in a popup | `Store::entitlements`, for the session's request | `prism::web::entitlements_changed` (a webhook) |
| Linux, Windows | the app's `Store` (none: `Unavailable`) | the checkout page in the browser | `Store::entitlements` | `refresh_entitlements` |

Products are created in App Store Connect and in the Play Console (Monetize › Products › One-time products), with the same ids on both if the app wants one id per unlock.

## Configuration

Apple platforms need nothing: StoreKit is part of the system. Android builds bundle Google Play Billing when the app sells:

```toml
[store]
enabled = true   # Android: Google Play Billing, fetched from Google's Maven and bundled
```

`prism run|build android` then fetches the library (version 9.1.0) and its dependencies, checked against `android-deps.lock`, merges their manifests (the `BILLING` permission, Play's proxy activities) and compiles `PrismBilling.java`. Without it, `cx.capabilities().purchases` is `false` on Android and the calls answer `Unavailable`. Play Billing needs the Play Store app on the device: emulators with Google Play have it.

Prism acknowledges Play purchases, as Google requires within three days (it refunds those left unacknowledged), when they arrive and again whenever it reads what the user owns. On Apple platforms it finishes verified transactions.

## Testing without the stores

`TestStore` sells at once, on every platform, with no App Store Connect, Play Console or payment provider. Its products come from code or from a StoreKit configuration file (`.storekit`, the JSON Xcode edits: its non-consumables, named in the app's language when the file has it, priced in the storefront's currency):

```rust
let app = Application::new();
#[cfg(debug_assertions)]
let app = app.store(TestStore::storekit(include_str!("../Products.storekit")).unwrap());
// or TestStore::new().product(Product::new("pro", "Pro", "$2.99")).owned(["pro"]).answer(PurchaseOutcome::Pending)
app.run(|cx| cx.new_entity(Home::new));
```

Xcode applies a `.storekit` file only to apps it runs itself; `prism run` builds without Xcode, so `TestStore` reads the file instead. The counter example sells "Counter Pro" this way in debug builds (Settings: it unlocks two accent colors).

Against the real stores:

- **Apple**: create the product in App Store Connect, then buy with a Sandbox account (on the device, Settings › Developer › Sandbox Apple Account on iOS 18 and later). StoreKit's sandbox charges nothing.
- **Google Play**: upload a build to a testing track (`prism publish android`, track `internal`), add the testers as license testers (Play Console › Settings › License testing), and install from the Play Store. Test cards charge nothing.

## Your own store: the web, desktops

The web has no store: the app's server sells through its payment provider (Stripe, Paddle, Lemon Squeezy…). Desktops outside the Mac App Store and the Microsoft Store do the same. The app implements `Store` and sets it with `Application::store` (or `cx.set_store`). Prism calls it on the server, with the session's request, so it reads the signed-in user that the host's middleware attached (`prism::web::RequestContext`). `Buyer::locale` is the app's language, for products' names:

```rust
struct Shop { db: Db, stripe: Stripe }

impl Store for Shop {
    fn products(&self, _: &Buyer, ids: &[String]) -> Result<Vec<Product>, DeviceError> {
        Ok(ids.iter().filter(|id| *id == "pro").map(|_| Product::new("pro", "Pro", "9 €")).collect())
    }
    // Read from your database, not from the payment API: the first paint waits for it.
    fn entitlements(&self, buyer: &Buyer) -> Result<Vec<String>, DeviceError> {
        Ok(buyer.context::<User>().map(|u| self.db.products_of(&u.id)).unwrap_or_default())
    }
    fn purchase(&self, buyer: &Buyer, p: &Purchase, return_url: Option<&str>) -> Result<Checkout, DeviceError> {
        let user = buyer.context::<User>().ok_or(DeviceError::Denied)?;
        // Stripe Checkout: success_url is Prism's return URL.
        let back = return_url.ok_or(DeviceError::Unavailable)?;
        Ok(Checkout::Open(self.stripe.checkout_session(&user.id, &p.product, back)?))
    }
    fn buyer_id(&self, buyer: &Buyer) -> Option<String> {
        buyer.context::<User>().map(|u| u.id.clone())
    }
}

Application::new().store(Shop::new()).run(|cx| cx.new_entity(Home::new));
```

On the web, `Checkout::Open(url)` opens the provider's page in a popup. `return_url` is `<origin><base>/_prism/store/return?state=…`, a URL of the app's own server, valid once and for this session only: the provider sends the user back there (keep its query and add yours, such as Stripe's `{CHECKOUT_SESSION_ID}`). Prism then closes the popup and calls `Store::checkout_returned` with the URL. By default it answers `Purchased` if `entitlements` now lists the product, else `Pending`; a provider's cancel URL can be the same URL with a marker of yours, which your `checkout_returned` turns into `Cancelled`. Closing the popup answers `Cancelled`. Only the server completes a checkout: the page can cancel one, never confirm it. Checkout pages must be `https://` (or `http://localhost` while testing).

Payments are confirmed by the provider's webhook. Once your handler has recorded the purchase, tell the buyer's live sessions:

```rust
// Axum: Stripe's webhook, its signature checked.
async fn stripe_webhook(event: StripeEvent) -> StatusCode {
    db::grant(&event.user_id, &event.product).await;
    prism::web::entitlements_changed(&event.user_id);   // sessions whose buyer_id is this read entitlements again
    StatusCode::OK
}
```

It reaches the sessions of this server process; with several processes, run it in each (a message bus), or the others see the change when they reconnect.

Natively, `Checkout::Open(url)` opens the page in the browser and the purchase answers `Pending`; call `cx.refresh_entitlements()` when the user comes back (a deep link, the app becoming active). `Checkout::Done(outcome)` ends the purchase at once (a saved card was charged, a licence key accepted).

The App Store and Google Play require their own payment system for digital goods. On Apple platforms and Android, ship without your own `Store` (keep `TestStore` to debug builds) so purchases go through StoreKit and Play Billing.
