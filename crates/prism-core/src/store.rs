//! In-app purchases: one-time unlocks (non-consumables) sold through the
//! platform's store, or through a [`Store`] the app plugs in (#306).
//!
//! ```ignore
//! // Names and prices for the buy button, in the user's currency.
//! cx.spawn(async move |this, cx| {
//!     let products = cx.store_products(&["pro"]).await?;   // Vec<Product>
//!     match cx.purchase("pro").await? {
//!         PurchaseOutcome::Purchased => {}                  // cx.owns("pro") is now true
//!         PurchaseOutcome::Pending => {}                    // Ask to Buy, a slow payment
//!         PurchaseOutcome::Cancelled => {}
//!     }
//!     cx.restore_purchases().await?;                        // the Restore button
//! });
//! // In render, in a widget, anywhere: what the user owns.
//! if cx.owns("pro") { /* … */ }
//! // A purchase on another device, a refund, an approved Ask to Buy.
//! cx.on_entitlements(|app| { /* … */ });
//! ```
//!
//! | | Store | Products, purchase | Owned | Changes |
//! |---|---|---|---|---|
//! | iOS, iPadOS, macOS, visionOS, tvOS, watchOS | StoreKit 2 | `Product.products`, `purchase` | `Transaction.currentEntitlements` (verified) | `Transaction.updates` |
//! | Android | Google Play Billing (`[store] enabled`) | `queryProductDetailsAsync`, `launchBillingFlow` | `queryPurchasesAsync` (acknowledged) | `onPurchasesUpdated`, each return to the app |
//! | Web | the app's [`Store`] (none: `Unavailable`) | a checkout page in a popup | [`Store::entitlements`], per request | `prism_web::entitlements_changed` |
//! | Linux, Windows | the app's [`Store`] (none: `Unavailable`) | the checkout page in the browser | [`Store::entitlements`] | [`refresh_entitlements`](AsyncApp::refresh_entitlements) |
//!
//! [`App::set_store`] replaces the platform's store with the app's own
//! (a [`TestStore`] in debug builds, the host's payment provider on the
//! web). What the user owns is kept between runs, so [`App::owns`] answers
//! at launch, before the store does.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use crate::app::App;
use crate::device::{DeviceCall, DeviceError, DeviceValue};
use crate::i18n::Locale;
use crate::request::WebRequest;
use crate::task::AsyncApp;

/// Where the last answer of the platform's store is kept between runs.
const CACHE_KEY: &str = "prism.store.entitlements";

/// Something the user can buy, as the store describes it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Product {
    /// Its id in the store (App Store Connect, the Play Console).
    pub id: String,
    /// Its name, in the user's language.
    pub name: String,
    /// Its description, in the user's language.
    pub description: String,
    /// Its price as the store shows it, in the user's currency (`4,99 €`).
    pub price: String,
    /// The price in millionths of the currency's unit (`4_990_000` for
    /// 4.99), when the store gives it.
    pub price_micros: Option<i64>,
    /// The currency's ISO 4217 code (`EUR`), when the store gives it.
    pub currency: Option<String>,
}

impl Product {
    /// A product with its name and price as shown (`"$4.99"`).
    pub fn new(id: impl Into<String>, name: impl Into<String>, price: impl Into<String>) -> Self {
        Product {
            id: id.into(),
            name: name.into(),
            price: price.into(),
            ..Default::default()
        }
    }

    /// With a description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// With the price as a number: millionths of the unit of `currency`
    /// (ISO 4217).
    pub fn amount(mut self, micros: i64, currency: impl Into<String>) -> Self {
        self.price_micros = Some(micros);
        self.currency = Some(currency.into());
        self
    }

    /// The products of a backend's answer: a JSON array of
    /// `{"id","name","description","price","micros","currency"}`.
    pub fn list_from_json(json: &str) -> Result<Vec<Product>, DeviceError> {
        let v: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| DeviceError::Failed(format!("the store's products: {e}")))?;
        let items = v
            .as_array()
            .ok_or_else(|| DeviceError::Failed("the store's products are not a list".into()))?;
        Ok(items
            .iter()
            .filter_map(|p| {
                let s = |k: &str| p.get(k).and_then(|v| v.as_str()).map(str::to_string);
                Some(Product {
                    id: s("id")?,
                    name: s("name").unwrap_or_default(),
                    description: s("description").unwrap_or_default(),
                    price: s("price").unwrap_or_default(),
                    price_micros: p.get("micros").and_then(|v| v.as_i64()),
                    currency: s("currency").filter(|c| !c.is_empty()),
                })
            })
            .collect())
    }

    /// The JSON form of [`list_from_json`](Self::list_from_json).
    pub fn list_to_json(products: &[Product]) -> String {
        serde_json::Value::Array(
            products
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "id": p.id, "name": p.name, "description": p.description,
                        "price": p.price, "micros": p.price_micros, "currency": p.currency,
                    })
                })
                .collect(),
        )
        .to_string()
    }
}

/// How a purchase ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PurchaseOutcome {
    /// Paid and verified: [`App::owns`] says so already.
    Purchased,
    /// Waiting for someone or something: a parent's approval (Ask to Buy),
    /// a slow payment method, a checkout whose payment is not confirmed
    /// yet. [`App::on_entitlements`] runs when it completes.
    Pending,
    /// The user cancelled.
    Cancelled,
}

impl PurchaseOutcome {
    /// `purchased`, `pending` or `cancelled` (backends' answers).
    pub fn name(self) -> &'static str {
        match self {
            PurchaseOutcome::Purchased => "purchased",
            PurchaseOutcome::Pending => "pending",
            PurchaseOutcome::Cancelled => "cancelled",
        }
    }

    /// The outcome named `name` (see [`name`](Self::name)).
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "purchased" => Some(PurchaseOutcome::Purchased),
            "pending" => Some(PurchaseOutcome::Pending),
            "cancelled" => Some(PurchaseOutcome::Cancelled),
            _ => None,
        }
    }
}

/// A purchase to make ([`AsyncApp::purchase_with`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Purchase {
    /// The product's id.
    pub product: String,
    /// Your user's id for this purchase, a UUID: StoreKit's
    /// `appAccountToken`, Play's obfuscated account id. Your server finds
    /// it in the transaction, to tie the purchase to the account.
    pub account_token: Option<String>,
}

impl Purchase {
    /// Buys `product`.
    pub fn new(product: impl Into<String>) -> Self {
        Purchase {
            product: product.into(),
            account_token: None,
        }
    }

    /// Ties the purchase to your user, a UUID
    /// (`"8b2f…-…"`, see [`account_token`](Self::account_token)).
    pub fn account_token(mut self, uuid: impl Into<String>) -> Self {
        self.account_token = Some(uuid.into());
        self
    }
}

/// Whether `s` is a UUID (`8-4-4-4-12` hex digits), as StoreKit wants.
fn is_uuid(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// What [`Store::purchase`] does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Checkout {
    /// The purchase is over (a saved payment method was charged, a test
    /// store, the user already owns it).
    Done(PurchaseOutcome),
    /// The user pays on this page (Stripe Checkout, Paddle, Lemon
    /// Squeezy…). On the web it opens in a popup and comes back to the
    /// `return_url` given to [`Store::purchase`]; natively it opens in the
    /// browser and the purchase answers `Pending`.
    Open(String),
}

/// Who buys: what a [`Store`] knows of the user.
#[derive(Clone, Debug, Default)]
pub struct Buyer {
    /// The web request the session runs for: its cookies, headers and the
    /// host's context (the signed-in user). `None` natively.
    pub request: Option<WebRequest>,
    /// The app's language ([`App::locale`]), for products' names and
    /// descriptions.
    pub locale: Locale,
}

impl Buyer {
    /// What the host attached to the request, if it is a `T`: the
    /// signed-in user (see `prism_web::RequestContext`). `None` natively.
    pub fn context<T: std::any::Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.request.as_ref()?.context::<T>()
    }
}

/// A store the app plugs in with [`App::set_store`] (or
/// `Application::store`): the host's payment provider on the web, a
/// [`TestStore`], a desktop app's own licensing.
///
/// Prism calls it on a background thread, except
/// [`entitlements`](Self::entitlements) when the store is set (the first
/// paint must know what the user owns) and [`buyer_id`](Self::buyer_id):
/// read what the user owns from your database or from the request's
/// context, not from a payment API.
///
/// ```ignore
/// struct Shop;
///
/// impl Store for Shop {
///     fn products(&self, _: &Buyer, ids: &[String]) -> Result<Vec<Product>, DeviceError> {
///         Ok(ids.iter().filter(|id| *id == "pro").map(|_| Product::new("pro", "Pro", "9 €")).collect())
///     }
///     fn entitlements(&self, buyer: &Buyer) -> Result<Vec<String>, DeviceError> {
///         // The middleware put the signed-in user in the request.
///         Ok(buyer.context::<User>().map(|u| u.products.clone()).unwrap_or_default())
///     }
///     fn purchase(&self, buyer: &Buyer, p: &Purchase, return_url: Option<&str>) -> Result<Checkout, DeviceError> {
///         let user = buyer.context::<User>().ok_or(DeviceError::Denied)?;
///         let url = stripe::checkout_session(&user, &p.product, return_url.unwrap_or("https://example.com/thanks"))?;
///         Ok(Checkout::Open(url))
///     }
///     fn buyer_id(&self, buyer: &Buyer) -> Option<String> {
///         buyer.context::<User>().map(|u| u.id.clone())
///     }
/// }
/// ```
pub trait Store: Send + Sync + 'static {
    /// The products among `ids` that exist, with their price for this
    /// buyer. Unknown ids are left out.
    fn products(&self, buyer: &Buyer, ids: &[String]) -> Result<Vec<Product>, DeviceError>;

    /// The ids of the products this buyer owns.
    fn entitlements(&self, buyer: &Buyer) -> Result<Vec<String>, DeviceError>;

    /// Starts buying `purchase.product`. `return_url` is where a checkout
    /// page sends the user back on the web (Prism's
    /// `<origin><base>/_prism/store/return?state=…`; keep its query and
    /// add yours); `None` natively.
    fn purchase(
        &self,
        buyer: &Buyer,
        purchase: &Purchase,
        return_url: Option<&str>,
    ) -> Result<Checkout, DeviceError>;

    /// The checkout page of `product` came back to `url` (the return URL,
    /// with what the payment provider added to it). The default looks at
    /// what the buyer owns now: `Purchased` if it has the product, else
    /// `Pending` (the payment's webhook has not arrived yet).
    fn checkout_returned(
        &self,
        buyer: &Buyer,
        product: &str,
        url: &str,
    ) -> Result<PurchaseOutcome, DeviceError> {
        let _ = url;
        Ok(if self.entitlements(buyer)?.iter().any(|p| p == product) {
            PurchaseOutcome::Purchased
        } else {
            PurchaseOutcome::Pending
        })
    }

    /// Restores purchases made elsewhere; [`entitlements`](Self::entitlements)
    /// is read again after it. Nothing by default.
    fn restore(&self, buyer: &Buyer) -> Result<(), DeviceError> {
        let _ = buyer;
        Ok(())
    }

    /// Who the buyer is, for `prism_web::entitlements_changed(id)`: the
    /// sessions whose buyer has this id read their entitlements again.
    /// `None` (the default): only [`refresh_entitlements`](AsyncApp::refresh_entitlements)
    /// and the app's own purchases update them.
    fn buyer_id(&self, buyer: &Buyer) -> Option<String> {
        let _ = buyer;
        None
    }
}

/// A store for tests and debug builds: it sells its products at once,
/// without App Store Connect, the Play Console or a payment provider, on
/// every platform. Its products come from code or from a StoreKit
/// configuration file (`.storekit`, Xcode's format: its non-consumables).
/// What was bought is kept while the process runs, for every buyer.
///
/// ```ignore
/// let app = Application::new();
/// #[cfg(debug_assertions)]
/// let app = app.store(TestStore::storekit(include_str!("../Products.storekit")).unwrap());
/// app.run(|cx| cx.new_entity(Home::new));
/// ```
pub struct TestStore {
    products: Vec<Product>,
    /// A StoreKit file's localizations: product id → (locale, name, description).
    localizations: Vec<(String, Locale, String, String)>,
    owned: Mutex<BTreeSet<String>>,
    answer: PurchaseOutcome,
}

impl Default for TestStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for TestStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestStore")
            .field("products", &self.products)
            .field("owned", &self.owned_now())
            .field("answer", &self.answer)
            .finish()
    }
}

impl TestStore {
    /// A store selling nothing yet.
    pub fn new() -> Self {
        TestStore {
            products: Vec::new(),
            localizations: Vec::new(),
            owned: Mutex::new(BTreeSet::new()),
            answer: PurchaseOutcome::Purchased,
        }
    }

    /// Sells `product` too.
    pub fn product(mut self, product: Product) -> Self {
        self.products.retain(|p| p.id != product.id);
        self.products.push(product);
        self
    }

    /// The non-consumables of a StoreKit configuration file (the JSON
    /// Xcode writes): each one's id, name and description, in the app's
    /// language when the file has it (else the file's locale, else its
    /// first localization), and its price in the storefront's currency.
    pub fn storekit(json: &str) -> Result<Self, String> {
        let v: serde_json::Value =
            serde_json::from_str(json).map_err(|e| format!("not a StoreKit configuration: {e}"))?;
        let settings = v.get("settings");
        let setting = |k: &str| settings.and_then(|s| s.get(k)).and_then(|v| v.as_str());
        let locale = setting("_locale").unwrap_or("en_US");
        let currency = storefront_currency(setting("_storefront").unwrap_or("USA"));
        let mut store = TestStore::new();
        for p in v
            .get("products")
            .and_then(|p| p.as_array())
            .into_iter()
            .flatten()
        {
            if p.get("type").and_then(|t| t.as_str()) != Some("NonConsumable") {
                continue;
            }
            let Some(id) = p.get("productID").and_then(|v| v.as_str()) else {
                continue;
            };
            let localizations = p.get("localizations").and_then(|l| l.as_array());
            let localization = localizations.and_then(|l| {
                l.iter()
                    .find(|x| x.get("locale").and_then(|v| v.as_str()) == Some(locale))
                    .or_else(|| l.first())
            });
            let text = |k: &str| {
                localization
                    .and_then(|l| l.get(k))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string()
            };
            let reference = p.get("referenceName").and_then(|v| v.as_str());
            let mut name = text("displayName");
            if name.is_empty() {
                name = reference.unwrap_or(id).to_string();
            }
            let amount = p
                .get("displayPrice")
                .and_then(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .or_else(|| Some(v.to_string()))
                })
                .unwrap_or_else(|| "0".into());
            let mut product = Product::new(id, name, format_price(&amount, currency))
                .description(text("description"));
            if let Some(micros) = micros(&amount) {
                product = product.amount(micros, currency);
            }
            for l in localizations.into_iter().flatten() {
                let get = |k: &str| l.get(k).and_then(|v| v.as_str()).unwrap_or_default();
                if get("locale").is_empty() || get("displayName").is_empty() {
                    continue;
                }
                store.localizations.push((
                    id.to_string(),
                    Locale::new(get("locale")),
                    get("displayName").to_string(),
                    get("description").to_string(),
                ));
            }
            store = store.product(product);
        }
        Ok(store)
    }

    /// Starts with the user owning `ids` (a restored purchase, a test of
    /// the unlocked screens).
    pub fn owned<S: Into<String>>(self, ids: impl IntoIterator<Item = S>) -> Self {
        self.owned_set().extend(ids.into_iter().map(Into::into));
        self
    }

    /// What every purchase answers: `Purchased` (the default; the product
    /// is then owned), `Pending` (Ask to Buy) or `Cancelled`.
    pub fn answer(mut self, outcome: PurchaseOutcome) -> Self {
        self.answer = outcome;
        self
    }

    /// `product` in `locale`'s language when the StoreKit file has it: the
    /// same tag, else the same language.
    fn localized(&self, product: &Product, locale: &Locale) -> Product {
        let own = || self.localizations.iter().filter(|l| l.0 == product.id);
        let found = own()
            .find(|l| l.1.tag().eq_ignore_ascii_case(locale.tag()))
            .or_else(|| own().find(|l| l.1.language().eq_ignore_ascii_case(locale.language())));
        match found {
            Some((_, _, name, description)) => Product {
                name: name.clone(),
                description: description.clone(),
                ..product.clone()
            },
            None => product.clone(),
        }
    }

    /// The products it sells.
    pub fn products_sold(&self) -> &[Product] {
        &self.products
    }

    /// What the user owns now.
    pub fn owned_now(&self) -> Vec<String> {
        self.owned_set().iter().cloned().collect()
    }

    fn owned_set(&self) -> std::sync::MutexGuard<'_, BTreeSet<String>> {
        self.owned.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Store for TestStore {
    fn products(&self, buyer: &Buyer, ids: &[String]) -> Result<Vec<Product>, DeviceError> {
        Ok(self
            .products
            .iter()
            .filter(|p| ids.contains(&p.id))
            .map(|p| self.localized(p, &buyer.locale))
            .collect())
    }

    fn entitlements(&self, _: &Buyer) -> Result<Vec<String>, DeviceError> {
        Ok(self.owned_now())
    }

    fn purchase(
        &self,
        _: &Buyer,
        purchase: &Purchase,
        _: Option<&str>,
    ) -> Result<Checkout, DeviceError> {
        if !self.products.iter().any(|p| p.id == purchase.product) {
            return Err(DeviceError::Failed(format!(
                "the test store does not sell {:?}",
                purchase.product
            )));
        }
        if self.answer == PurchaseOutcome::Purchased {
            self.owned_set().insert(purchase.product.clone());
        }
        Ok(Checkout::Done(self.answer))
    }
}

/// The currency of an App Store storefront (ISO 3166 alpha-3), for a
/// StoreKit configuration file.
fn storefront_currency(storefront: &str) -> &'static str {
    match storefront {
        "FRA" | "DEU" | "ESP" | "ITA" | "NLD" | "BEL" | "AUT" | "IRL" | "PRT" | "FIN" | "GRC"
        | "LUX" | "SVK" | "SVN" | "EST" | "LVA" | "LTU" | "HRV" | "CYP" | "MLT" => "EUR",
        "GBR" => "GBP",
        "JPN" => "JPY",
        "CAN" => "CAD",
        "AUS" => "AUD",
        "CHE" => "CHF",
        "CHN" => "CNY",
        "IND" => "INR",
        "BRA" => "BRL",
        "MEX" => "MXN",
        "KOR" => "KRW",
        "SWE" => "SEK",
        "NOR" => "NOK",
        "DNK" => "DKK",
        "POL" => "PLN",
        _ => "USD",
    }
}

/// `4.99` in millionths (`4_990_000`), without rounding through a float.
fn micros(amount: &str) -> Option<i64> {
    let (whole, fraction) = amount.trim().split_once('.').unwrap_or((amount.trim(), ""));
    if whole.is_empty() && fraction.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 6
    {
        return None;
    }
    let whole: i64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let fraction: i64 = format!("{fraction:0<6}").parse().ok()?;
    whole.checked_mul(1_000_000)?.checked_add(fraction)
}

/// A test price as a store would show it: `$4.99`, `4,99 €`, `£4.99`.
fn format_price(amount: &str, currency: &str) -> String {
    let amount = amount.trim();
    match currency {
        "USD" => format!("${amount}"),
        "GBP" => format!("£{amount}"),
        "JPY" => format!("¥{amount}"),
        "CNY" => format!("CN¥{amount}"),
        "INR" => format!("₹{amount}"),
        "EUR" => format!("{} €", amount.replace('.', ",")),
        "CHF" => format!("CHF {amount}"),
        "SEK" | "NOK" | "DKK" | "PLN" => format!("{} {currency}", amount.replace('.', ",")),
        _ => format!("{amount} {currency}"),
    }
}

type Handler = Rc<dyn Fn(&mut App)>;

/// In-app purchase state kept by the [`App`].
#[derive(Default)]
pub(crate) struct StoreState {
    /// The app's own store ([`App::set_store`]), instead of the platform's.
    provider: Option<Arc<dyn Store>>,
    /// What the user owns: the store's last answer, or (before it answers)
    /// what it said in the previous run. `None` until read.
    owned: RefCell<Option<BTreeSet<String>>>,
    /// The store answered in this run.
    confirmed: bool,
    /// The app asked what the user owns: the platform's store is watched.
    wanted: Cell<bool>,
    /// The watch was sent.
    watching: bool,
    handlers: Vec<Handler>,
}

impl App {
    /// Sells through `store` instead of the platform's store: the host's
    /// payment provider on the web (where there is no store otherwise), a
    /// [`TestStore`] in debug builds, a desktop app's licensing. Its
    /// [`entitlements`](Store::entitlements) are read now, so the first
    /// render knows what the user owns. `Application::store` calls it for
    /// every window and web session.
    ///
    /// The App Store and Google Play require their own payment system for
    /// digital goods: on Apple platforms and Android, use it for tests.
    pub fn set_store(&mut self, store: Arc<dyn Store>) {
        let buyer = self.store_buyer();
        let owned = match store.entitlements(&buyer) {
            Ok(ids) => ids.into_iter().collect(),
            Err(e) => {
                eprintln!("prism: the app's store could not say what the user owns: {e}");
                BTreeSet::new()
            }
        };
        self.store.provider = Some(store);
        self.set_entitlements(owned, true);
    }

    /// The app's own store, if [`set_store`](App::set_store) set one.
    pub fn store(&self) -> Option<Arc<dyn Store>> {
        self.store.provider.clone()
    }

    /// Whether the user owns `product`: bought on any of their devices
    /// (restored by the store), not refunded. Answers at once, also at
    /// launch (the store's answer from the previous run), and the app
    /// re-renders when it changes. Starts watching the platform's store.
    pub fn owns(&self, product: &str) -> bool {
        self.store.wanted.set(true);
        self.with_owned(|owned| owned.contains(product))
    }

    /// The ids of every product the user owns, in order.
    pub fn entitlements(&self) -> Vec<String> {
        self.store.wanted.set(true);
        self.with_owned(|owned| owned.iter().cloned().collect())
    }

    /// Whether the store answered in this run: until then,
    /// [`owns`](App::owns) gives what it said the last time.
    pub fn entitlements_confirmed(&self) -> bool {
        self.store.confirmed
    }

    /// Runs `f` whenever what the user owns changes: a purchase (also on
    /// another device), a refund, an approved Ask to Buy, a restore. The
    /// app re-renders (and widgets redraw) by themselves; this is for the
    /// rest (a thank-you toast, a server call). Starts watching the
    /// platform's store.
    pub fn on_entitlements(&mut self, f: impl Fn(&mut App) + 'static) {
        self.store.wanted.set(true);
        self.store.handlers.push(Rc::new(f));
    }

    /// For backends: the host says a buyer's entitlements changed (a
    /// payment's webhook). The session reads its store's entitlements
    /// again when its buyer has this [`buyer_id`](Store::buyer_id) (`None`:
    /// whoever it is).
    pub fn entitlements_changed(&mut self, buyer_id: Option<&str>) {
        let Some(store) = self.store.provider.clone() else {
            return;
        };
        if let Some(id) = buyer_id
            && store.buyer_id(&self.store_buyer()).as_deref() != Some(id)
        {
            return;
        }
        self.spawn(async move |cx| {
            if let Err(e) = cx.refresh_entitlements().await {
                eprintln!("prism: entitlements not read again: {e}");
            }
        });
    }

    /// The buyer of this session, for the app's store.
    fn store_buyer(&self) -> Buyer {
        Buyer {
            request: self.request().cloned(),
            locale: self.locale().clone(),
        }
    }

    /// `f` with what the user owns, reading the previous run's answer the
    /// first time.
    fn with_owned<R>(&self, f: impl FnOnce(&BTreeSet<String>) -> R) -> R {
        let mut owned = self.store.owned.borrow_mut();
        let owned = owned.get_or_insert_with(|| {
            if self.store.provider.is_some() {
                return BTreeSet::new();
            }
            self.storage
                .get(CACHE_KEY)
                .and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok())
                .unwrap_or_default()
                .into_iter()
                .collect()
        });
        f(owned)
    }

    /// What the user owns is now `owned` (`confirmed`: the store said so in
    /// this run). Re-renders, redraws widgets and runs the handlers if it
    /// changed; keeps the platform store's answer for the next launch.
    pub(crate) fn set_entitlements(&mut self, owned: BTreeSet<String>, confirmed: bool) {
        let before = self.with_owned(|o| o.clone());
        let was_confirmed = std::mem::replace(&mut self.store.confirmed, confirmed);
        *self.store.owned.borrow_mut() = Some(owned.clone());
        if self.store.provider.is_none() {
            let json = serde_json::to_string(&owned.iter().collect::<Vec<_>>())
                .unwrap_or_else(|_| "[]".into());
            if self.storage.get(CACHE_KEY).as_deref() != Some(json.as_str()) {
                self.storage.set(CACHE_KEY, &json);
            }
        }
        if before == owned {
            if was_confirmed != confirmed {
                self.refresh();
            }
            return;
        }
        self.refresh();
        self.reload_widgets();
        for h in self.store.handlers.clone() {
            h(self);
        }
    }

    /// For [`take_requests`](App::take_requests): watches the platform's
    /// store once the app asked what the user owns.
    pub(crate) fn store_watch_request(&mut self) {
        if self.store.wanted.get()
            && !self.store.watching
            && self.store.provider.is_none()
            && self.surfaces.capabilities.purchases
        {
            self.store.watching = true;
            let id = self.device_call(DeviceCall::WatchEntitlements);
            self.device_stream(
                id,
                Rc::new(|e, app| match e {
                    crate::device::DeviceEvent::Entitlements(ids) => {
                        app.set_entitlements(ids.iter().cloned().collect(), true)
                    }
                    crate::device::DeviceEvent::Error(err) => {
                        eprintln!("prism: the store's entitlements: {err}")
                    }
                    _ => {}
                }),
            );
        }
    }
}

/// The product ids of a backend's answer (a JSON array of strings).
fn ids_from(value: DeviceValue) -> Result<BTreeSet<String>, DeviceError> {
    match value {
        DeviceValue::Text(Some(json)) => serde_json::from_str::<Vec<String>>(&json)
            .map(|v| v.into_iter().collect())
            .map_err(|e| DeviceError::Failed(format!("the store's entitlements: {e}"))),
        DeviceValue::Text(None) | DeviceValue::Done => Ok(BTreeSet::new()),
        other => Err(DeviceError::Failed(format!("unexpected answer {other:?}"))),
    }
}

impl AsyncApp {
    /// The products named `ids`, with their localized name and price, in
    /// the order asked; unknown ids are left out. `Err(Unavailable)` where
    /// there is no store (`capabilities().purchases`).
    pub async fn store_products<S: AsRef<str>>(
        &self,
        ids: impl IntoIterator<Item = S>,
    ) -> Result<Vec<Product>, DeviceError> {
        let ids: Vec<String> = ids.into_iter().map(|s| s.as_ref().to_string()).collect();
        let products = match self.update(|app| (app.store(), app.store_buyer())) {
            (Some(store), buyer) => {
                let asked = ids.clone();
                self.background(move || store.products(&buyer, &asked))
                    .await?
            }
            (None, _) => match self.device(DeviceCall::StoreProducts(ids.clone())).await? {
                DeviceValue::Text(Some(json)) => Product::list_from_json(&json)?,
                other => return Err(DeviceError::Failed(format!("unexpected answer {other:?}"))),
            },
        };
        Ok(ids
            .iter()
            .filter_map(|id| products.iter().find(|p| &p.id == id).cloned())
            .collect())
    }

    /// Buys `product` with the store's own sheet (or the app's store's
    /// checkout). Once `Purchased`, [`App::owns`] says so; a purchase
    /// already made answers `Purchased` too. `Err(Unavailable)` where there
    /// is no store, `Err(Denied)` where purchases are not allowed (parental
    /// controls).
    pub async fn purchase(&self, product: &str) -> Result<PurchaseOutcome, DeviceError> {
        self.purchase_with(Purchase::new(product)).await
    }

    /// Like [`purchase`](Self::purchase), with a full [`Purchase`] (your
    /// user's account token).
    pub async fn purchase_with(&self, purchase: Purchase) -> Result<PurchaseOutcome, DeviceError> {
        if let Some(token) = &purchase.account_token
            && !is_uuid(token)
        {
            return Err(DeviceError::Failed(format!(
                "the account token {token:?} is not a UUID"
            )));
        }
        let product = purchase.product.clone();
        let outcome = match self.update(|app| (app.store(), app.store_buyer())) {
            (Some(store), buyer) => self.purchase_from(store, buyer, purchase).await?,
            (None, _) => match self.device(DeviceCall::Purchase(purchase)).await {
                Ok(DeviceValue::Text(Some(name))) => PurchaseOutcome::from_name(&name)
                    .ok_or_else(|| DeviceError::Failed(format!("unexpected answer {name:?}")))?,
                Err(DeviceError::Cancelled) => PurchaseOutcome::Cancelled,
                Err(e) => return Err(e),
                Ok(other) => {
                    return Err(DeviceError::Failed(format!("unexpected answer {other:?}")));
                }
            },
        };
        if outcome == PurchaseOutcome::Purchased {
            // Verified by the store: owned now, before its next update.
            self.update(|app| {
                let mut owned = app.with_owned(|o| o.clone());
                if owned.insert(product) {
                    let confirmed = app.store.confirmed;
                    app.set_entitlements(owned, confirmed);
                }
            });
        }
        Ok(outcome)
    }

    /// A purchase through the app's own store.
    async fn purchase_from(
        &self,
        store: Arc<dyn Store>,
        buyer: Buyer,
        purchase: Purchase,
    ) -> Result<PurchaseOutcome, DeviceError> {
        // The web gives a checkout page a URL of the server to come back to.
        let return_url = if buyer.request.is_some() {
            match self.device(DeviceCall::StoreReturnUrl).await? {
                DeviceValue::Text(url) => url,
                _ => None,
            }
        } else {
            None
        };
        let product = purchase.product.clone();
        let (s, b, url) = (store.clone(), buyer.clone(), return_url.clone());
        let checkout = self
            .background(move || s.purchase(&b, &purchase, url.as_deref()))
            .await?;
        let outcome = match checkout {
            Checkout::Done(outcome) => outcome,
            Checkout::Open(url) => match return_url {
                Some(return_url) => {
                    match self
                        .device(DeviceCall::StoreCheckout { url, return_url })
                        .await
                    {
                        Ok(DeviceValue::Text(Some(came_back))) => {
                            let (s, b, p) = (store.clone(), buyer.clone(), product.clone());
                            self.background(move || s.checkout_returned(&b, &p, &came_back))
                                .await?
                        }
                        Err(DeviceError::Cancelled) => PurchaseOutcome::Cancelled,
                        Err(e) => return Err(e),
                        Ok(other) => {
                            return Err(DeviceError::Failed(format!(
                                "unexpected answer {other:?}"
                            )));
                        }
                    }
                }
                // Natively the browser takes it from here.
                None => {
                    self.update(|app| app.open_url(url));
                    PurchaseOutcome::Pending
                }
            },
        };
        if outcome != PurchaseOutcome::Cancelled {
            self.refresh_entitlements().await?;
        }
        Ok(outcome)
    }

    /// Restores purchases made on the user's other devices or before a
    /// reinstall (the Restore button App Review asks for), and answers
    /// what the user owns afterwards. On Apple platforms the user may be
    /// asked to sign in to the App Store.
    pub async fn restore_purchases(&self) -> Result<Vec<String>, DeviceError> {
        let owned = match self.update(|app| (app.store(), app.store_buyer())) {
            (Some(store), buyer) => {
                self.background(move || -> Result<BTreeSet<String>, DeviceError> {
                    store.restore(&buyer)?;
                    Ok(store.entitlements(&buyer)?.into_iter().collect())
                })
                .await?
            }
            (None, _) => ids_from(self.device(DeviceCall::RestorePurchases).await?)?,
        };
        Ok(self.update(|app| {
            app.set_entitlements(owned.clone(), true);
            owned.into_iter().collect()
        }))
    }

    /// Reads what the user owns from the store again and answers it: after
    /// your server learned of a purchase, when a native checkout came back
    /// (a deep link, the app back in the foreground).
    pub async fn refresh_entitlements(&self) -> Result<Vec<String>, DeviceError> {
        let owned = match self.update(|app| (app.store(), app.store_buyer())) {
            (Some(store), buyer) => self
                .background(move || store.entitlements(&buyer))
                .await?
                .into_iter()
                .collect(),
            (None, _) => ids_from(self.device(DeviceCall::StoreEntitlements).await?)?,
        };
        Ok(self.update(|app| {
            app.set_entitlements(owned.clone(), true);
            owned.into_iter().collect()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOREKIT: &str = r#"{
      "identifier" : "4E6A",
      "nonRenewingSubscriptions" : [],
      "products" : [
        {
          "displayPrice" : "4.99",
          "familyShareable" : false,
          "internalID" : "1",
          "localizations" : [
            { "description" : "Everything, forever", "displayName" : "Pro", "locale" : "en_US" },
            { "description" : "Tout, pour toujours", "displayName" : "Pro", "locale" : "fr_FR" }
          ],
          "productID" : "pro",
          "referenceName" : "Pro unlock",
          "type" : "NonConsumable"
        },
        {
          "displayPrice" : "0.99",
          "familyShareable" : false,
          "internalID" : "2",
          "localizations" : [],
          "productID" : "coins",
          "referenceName" : "Coins",
          "type" : "Consumable"
        },
        {
          "displayPrice" : "12",
          "internalID" : "3",
          "localizations" : [],
          "productID" : "themes",
          "referenceName" : "Themes",
          "type" : "NonConsumable"
        }
      ],
      "settings" : { "_locale" : "fr_FR", "_storefront" : "FRA" },
      "subscriptionGroups" : [],
      "version" : { "major" : 4, "minor" : 0 }
    }"#;

    #[test]
    fn reads_the_non_consumables_of_a_storekit_file() {
        let store = TestStore::storekit(STOREKIT).unwrap();
        let products = store.products_sold();
        assert_eq!(products.len(), 2, "{products:?}");
        let pro = &products[0];
        assert_eq!(pro.id, "pro");
        assert_eq!(pro.name, "Pro");
        assert_eq!(pro.description, "Tout, pour toujours");
        assert_eq!(pro.price, "4,99 €");
        assert_eq!(pro.price_micros, Some(4_990_000));
        assert_eq!(pro.currency.as_deref(), Some("EUR"));
        // No localization: the reference name.
        assert_eq!(products[1].name, "Themes");
        assert_eq!(products[1].price_micros, Some(12_000_000));
        assert!(TestStore::storekit("{").is_err());
    }

    #[test]
    fn a_storekit_file_speaks_the_buyers_language() {
        let store = TestStore::storekit(STOREKIT).unwrap();
        let ids = ["pro".to_string(), "themes".to_string()];
        let description = |tag: &str| {
            let buyer = Buyer {
                locale: Locale::new(tag),
                ..Buyer::default()
            };
            let products = store.products(&buyer, &ids).unwrap();
            // The file's own name when it has no localization.
            assert_eq!(products[1].name, "Themes");
            products[0].description.clone()
        };
        assert_eq!(description("en-US"), "Everything, forever");
        assert_eq!(description("en-GB"), "Everything, forever");
        assert_eq!(description("fr-CA"), "Tout, pour toujours");
        // A language the file lacks: its own locale.
        assert_eq!(description("de-DE"), "Tout, pour toujours");
    }

    #[test]
    fn prices_and_ids() {
        assert_eq!(micros("4.99"), Some(4_990_000));
        assert_eq!(micros("0.5"), Some(500_000));
        assert_eq!(micros("12"), Some(12_000_000));
        assert_eq!(micros("1.2345678"), None);
        assert_eq!(micros("abc"), None);
        assert_eq!(micros(""), None);
        assert_eq!(format_price("4.99", "USD"), "$4.99");
        assert_eq!(format_price("4.99", "SEK"), "4,99 SEK");
        assert!(is_uuid("8b2fa7c4-1d3e-4f5a-9b6c-7d8e9f0a1b2c"));
        assert!(!is_uuid("user-42"));
        assert!(!is_uuid("8b2fa7c4-1d3e-4f5a-9b6c-7d8e9f0a1b2"));
    }

    #[test]
    fn products_round_trip_as_json() {
        let products = vec![
            Product::new("pro", "Pro", "$4.99")
                .description("All of it")
                .amount(4_990_000, "USD"),
            Product::new("themes", "Themes", "1 €"),
        ];
        let json = Product::list_to_json(&products);
        assert_eq!(Product::list_from_json(&json).unwrap(), products);
        assert!(Product::list_from_json("{}").is_err());
    }

    #[test]
    fn the_test_store_sells_and_answers() {
        let store = TestStore::new()
            .product(Product::new("pro", "Pro", "$1"))
            .owned(["old"]);
        let buyer = Buyer::default();
        assert_eq!(
            store.purchase(&buyer, &Purchase::new("pro"), None),
            Ok(Checkout::Done(PurchaseOutcome::Purchased))
        );
        assert_eq!(store.entitlements(&buyer).unwrap(), ["old", "pro"]);
        assert!(
            store
                .purchase(&buyer, &Purchase::new("nope"), None)
                .is_err()
        );
        let pending = TestStore::new()
            .product(Product::new("pro", "Pro", "$1"))
            .answer(PurchaseOutcome::Pending);
        assert_eq!(
            pending.purchase(&buyer, &Purchase::new("pro"), None),
            Ok(Checkout::Done(PurchaseOutcome::Pending))
        );
        assert!(pending.owned_now().is_empty());
        // The default return check: owned or not yet.
        assert_eq!(
            store.checkout_returned(&buyer, "pro", "https://x/_prism/store/return?state=s"),
            Ok(PurchaseOutcome::Purchased)
        );
        assert_eq!(
            pending.checkout_returned(&buyer, "pro", ""),
            Ok(PurchaseOutcome::Pending)
        );
    }
}
