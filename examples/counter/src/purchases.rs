//! In-app purchases (#306): "Counter Pro", a one-time unlock of two more
//! accent colors (Settings).
//!
//! Debug builds sell through a [`TestStore`] read from `Products.storekit`
//! (the StoreKit configuration Xcode edits): purchases succeed at once, on
//! every platform, with no App Store Connect or Play Console setup. Release
//! builds sell through the platform's store: the App Store, Google Play
//! (Prism.toml `[store] enabled`); elsewhere there is none.

use prism::DeviceError;
#[cfg(debug_assertions)]
use prism::TestStore;
use prism::prelude::*;
use prism::ui::{self, Theme};

use crate::strings;

/// The product's id in App Store Connect and the Play Console.
pub const PRO: &str = "dev.prism.counter.pro";

/// The store of debug builds.
#[cfg(debug_assertions)]
pub fn test_store() -> TestStore {
    TestStore::storekit(include_str!("../Products.storekit")).expect("Products.storekit")
}

/// The Pro card of Settings: price, Buy, Restore.
#[derive(Default)]
pub struct ProCard {
    product: Option<Product>,
    busy: bool,
    status: Option<String>,
}

impl ProCard {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            let r = cx.store_products(&[PRO]).await;
            cx.update_entity(&this, |card, cx| match r {
                Ok(products) => {
                    card.product = products.into_iter().next();
                    if card.product.is_none() {
                        card.status = Some(strings::pro_missing(cx));
                    }
                }
                Err(e) => card.status = Some(error(cx, &e)),
            });
        });
        ProCard::default()
    }

    fn buy(&mut self, cx: &mut Context<Self>) {
        self.busy = true;
        self.status = None;
        cx.spawn(async move |this, cx| {
            let r = cx.purchase(PRO).await;
            cx.update_entity(&this, |card, cx| {
                card.busy = false;
                card.status = match r {
                    Ok(PurchaseOutcome::Purchased) => None,
                    Ok(PurchaseOutcome::Pending) => Some(strings::pro_pending(cx)),
                    Ok(PurchaseOutcome::Cancelled) => None,
                    Err(e) => Some(error(cx, &e)),
                };
            });
        });
    }

    fn restore(&mut self, cx: &mut Context<Self>) {
        self.busy = true;
        self.status = None;
        cx.spawn(async move |this, cx| {
            let r = cx.restore_purchases().await;
            cx.update_entity(&this, |card, cx| {
                card.busy = false;
                card.status = Some(match r {
                    Ok(owned) if owned.iter().any(|id| id == PRO) => strings::pro_restored(cx),
                    Ok(_) => strings::pro_nothing(cx),
                    Err(e) => error(cx, &e),
                });
            });
        });
    }
}

fn error(cx: &App, e: &DeviceError) -> String {
    match e {
        DeviceError::Unavailable => strings::pro_unavailable(cx),
        e => strings::pro_error(cx, e.to_string()),
    }
}

impl Render for ProCard {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let muted = |s: String| text(s).text_size(13.).text_color(t.muted);
        let owned = cx.owns(PRO);
        let mut card = ui::card().child(
            div()
                .flex_row()
                .gap(8.)
                .items_center()
                .child(ui::heading(strings::pro_title(cx)))
                .when(owned, |d| d.child(ui::badge(strings::pro_owned(cx)))),
        );
        if let Some(p) = &self.product {
            card = card.child(muted(p.description.clone()));
        }
        if owned {
            card = card.child(muted(strings::pro_thanks(cx)));
        } else if let Some(p) = &self.product {
            let label = if self.busy {
                strings::pro_buying(cx)
            } else {
                strings::pro_buy(cx, &p.price)
            };
            card = card.child(
                div()
                    .flex_row()
                    .flex_wrap()
                    .gap(8.)
                    .child(
                        ui::button(label)
                            .disabled(self.busy)
                            .on_click(cx.listener(|card, _, cx| card.buy(cx))),
                    )
                    .child(
                        ui::button(strings::pro_restore(cx))
                            .variant(ui::Variant::Secondary)
                            .disabled(self.busy)
                            .on_click(cx.listener(|card, _, cx| card.restore(cx))),
                    ),
            );
        }
        if let Some(s) = &self.status {
            card = card.child(muted(s.clone()));
        }
        card
    }
}
