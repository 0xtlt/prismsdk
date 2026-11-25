package dev.prism;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.os.Looper;

import com.android.billingclient.api.AcknowledgePurchaseParams;
import com.android.billingclient.api.BillingClient;
import com.android.billingclient.api.BillingClientStateListener;
import com.android.billingclient.api.BillingFlowParams;
import com.android.billingclient.api.BillingResult;
import com.android.billingclient.api.PendingPurchasesParams;
import com.android.billingclient.api.ProductDetails;
import com.android.billingclient.api.Purchase;
import com.android.billingclient.api.QueryProductDetailsParams;
import com.android.billingclient.api.QueryPurchasesParams;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeSet;
import java.util.function.Consumer;

/**
 * In-app purchases (#306): Google Play Billing for one-time products. Bundled
 * when the app sells through the platform's store (Prism.toml `[store]`, the
 * library fetched from Google's Maven); PrismDevice reaches it by reflection,
 * as PrismPush reaches PrismFcm. Answers the JSON calls `store_products`,
 * `purchase`, `store_entitlements`, `restore_purchases` and
 * `watch_entitlements` (events `entitlements`: the ids the user owns).
 *
 * Everything runs on the main thread. Purchases are acknowledged (Play refunds
 * those left unacknowledged for three days), here and again at each query.
 */
public final class PrismBilling {
    private PrismBilling() {}

    /** Play's billing service, which the Play Store app provides. */
    static final String SERVICE = "com.android.vending.billing.InAppBillingService.BIND";

    private static BillingClient client;
    private static boolean connecting;
    /** Calls waiting for the connection: run once it is set up, else failed. */
    private static final List<Runnable> ready = new ArrayList<>();
    private static final List<Consumer<String>> failed = new ArrayList<>();
    /** Products the app asked for, for their purchase flow. */
    private static final Map<String, ProductDetails> details = new HashMap<>();
    /** The purchase flow on screen (one at a time): its call id and product. */
    private static long buying = -1;
    private static String buyingProduct;
    /** Entitlement streams of the app. */
    private static final Set<Long> watchers = new LinkedHashSet<>();

    /** Whether Play's billing service is on this device (the Play Store app). */
    public static boolean available(Context c) {
        Intent i = new Intent(SERVICE).setPackage("com.android.vending");
        return !c.getPackageManager().queryIntentServices(i, 0).isEmpty();
    }

    public static void call(long id, String name, JSONObject c) {
        main(() -> {
            // No Play Store (an emulator without Google Play, a Huawei
            // phone): BillingClient would only retry and fail.
            if (!available(PrismDevice.ctx())) {
                PrismDevice.fail(id, "unavailable", name.startsWith("watch_"));
                return;
            }
            switch (name) {
                case "store_products": products(id, c.optJSONArray("ids")); break;
                case "purchase": purchase(id, c.optString("product"), c.isNull("account") ? null : c.optString("account")); break;
                case "store_entitlements": case "restore_purchases": entitlements(id); break;
                case "watch_entitlements": watchers.add(id); refresh(); break;
                default: PrismDevice.fail(id, "unavailable", false);
            }
        });
    }

    public static void stop(long id) {
        main(() -> watchers.remove(id));
    }

    /**
     * The app came back: purchases may have changed outside it (a pending one
     * paid, a promo code redeemed in the Play Store).
     */
    public static void resumed() {
        main(() -> { if (!watchers.isEmpty()) refresh(); });
    }

    private static void main(Runnable r) {
        if (Looper.myLooper() == Looper.getMainLooper()) r.run();
        else PrismHost.main.post(r);
    }

    // --- Connection -------------------------------------------------------------------------

    private static void connected(Runnable then, Consumer<String> otherwise) {
        if (client == null) {
            client = BillingClient.newBuilder(PrismDevice.ctx())
                    .setListener((result, purchases) -> main(() -> updated(result, purchases)))
                    .enablePendingPurchases(PendingPurchasesParams.newBuilder().enableOneTimeProducts().build())
                    .enableAutoServiceReconnection()
                    .build();
        }
        if (client.isReady()) {
            then.run();
            return;
        }
        ready.add(then);
        failed.add(otherwise);
        if (connecting) return;
        connecting = true;
        client.startConnection(new BillingClientStateListener() {
            @Override public void onBillingSetupFinished(BillingResult result) {
                main(() -> {
                    connecting = false;
                    List<Runnable> run = new ArrayList<>(ready);
                    List<Consumer<String>> fail = new ArrayList<>(failed);
                    ready.clear();
                    failed.clear();
                    if (result.getResponseCode() == BillingClient.BillingResponseCode.OK) {
                        for (Runnable r : run) r.run();
                    } else {
                        String m = message(result);
                        for (Consumer<String> f : fail) f.accept(m);
                    }
                });
            }

            // Calls made later reconnect (enableAutoServiceReconnection).
            @Override public void onBillingServiceDisconnected() {
                main(() -> connecting = false);
            }
        });
    }

    /** `cancelled` and `unavailable` as Rust reads them, else Play's code and message. */
    static String message(BillingResult r) {
        switch (r.getResponseCode()) {
            case BillingClient.BillingResponseCode.USER_CANCELED: return "cancelled";
            case BillingClient.BillingResponseCode.BILLING_UNAVAILABLE:
            case BillingClient.BillingResponseCode.FEATURE_NOT_SUPPORTED: return "unavailable";
            default: {
                String name = codeName(r.getResponseCode());
                String text = r.getDebugMessage() == null || r.getDebugMessage().isEmpty() ? name : r.getDebugMessage();
                return name + "\u001f" + text;
            }
        }
    }

    private static String codeName(int code) {
        switch (code) {
            case BillingClient.BillingResponseCode.SERVICE_DISCONNECTED: return "SERVICE_DISCONNECTED";
            case BillingClient.BillingResponseCode.SERVICE_UNAVAILABLE: return "SERVICE_UNAVAILABLE";
            case BillingClient.BillingResponseCode.ITEM_UNAVAILABLE: return "ITEM_UNAVAILABLE";
            case BillingClient.BillingResponseCode.DEVELOPER_ERROR: return "DEVELOPER_ERROR";
            case BillingClient.BillingResponseCode.ITEM_ALREADY_OWNED: return "ITEM_ALREADY_OWNED";
            case BillingClient.BillingResponseCode.ITEM_NOT_OWNED: return "ITEM_NOT_OWNED";
            case BillingClient.BillingResponseCode.NETWORK_ERROR: return "NETWORK_ERROR";
            default: return "ERROR";
        }
    }

    // --- Products ---------------------------------------------------------------------------

    private static void products(long id, JSONArray ids) {
        List<String> wanted = new ArrayList<>();
        for (int i = 0; ids != null && i < ids.length(); i++) {
            String p = ids.optString(i);
            if (!p.isEmpty() && !wanted.contains(p)) wanted.add(p);
        }
        query(wanted, found -> {
            JSONArray out = new JSONArray();
            for (String p : wanted) {
                ProductDetails d = found.get(p);
                if (d != null) out.put(json(d));
            }
            PrismDevice.reply(id, "text", out.toString());
        }, m -> PrismDevice.fail(id, m, false));
    }

    /** The details of `ids` (one-time products), kept for their purchase. */
    private static void query(List<String> ids, Consumer<Map<String, ProductDetails>> then, Consumer<String> otherwise) {
        if (ids.isEmpty()) {
            then.accept(Collections.emptyMap());
            return;
        }
        connected(() -> {
            List<QueryProductDetailsParams.Product> list = new ArrayList<>();
            for (String p : ids) {
                list.add(QueryProductDetailsParams.Product.newBuilder()
                        .setProductId(p).setProductType(BillingClient.ProductType.INAPP).build());
            }
            client.queryProductDetailsAsync(QueryProductDetailsParams.newBuilder().setProductList(list).build(),
                    (result, found) -> main(() -> {
                        if (result.getResponseCode() != BillingClient.BillingResponseCode.OK) {
                            otherwise.accept(message(result));
                            return;
                        }
                        Map<String, ProductDetails> byId = new HashMap<>();
                        for (ProductDetails d : found.getProductDetailsList()) {
                            if (offer(d) == null) continue;
                            byId.put(d.getProductId(), d);
                            details.put(d.getProductId(), d);
                        }
                        then.accept(byId);
                    }));
        }, otherwise);
    }

    /** A one-time product's offer: the first Play lists. */
    private static ProductDetails.OneTimePurchaseOfferDetails offer(ProductDetails d) {
        List<ProductDetails.OneTimePurchaseOfferDetails> all = d.getOneTimePurchaseOfferDetailsList();
        if (all != null && !all.isEmpty()) return all.get(0);
        return d.getOneTimePurchaseOfferDetails();
    }

    private static JSONObject json(ProductDetails d) {
        JSONObject o = new JSONObject();
        ProductDetails.OneTimePurchaseOfferDetails offer = offer(d);
        try {
            o.put("id", d.getProductId());
            o.put("name", d.getName());
            o.put("description", d.getDescription());
            o.put("price", offer.getFormattedPrice());
            o.put("micros", offer.getPriceAmountMicros());
            o.put("currency", offer.getPriceCurrencyCode());
        } catch (JSONException ignored) {
        }
        return o;
    }

    // --- Buying -----------------------------------------------------------------------------

    private static void purchase(long id, String product, String account) {
        if (buying != -1) {
            PrismDevice.fail(id, "a purchase is already on screen", false);
            return;
        }
        Consumer<String> otherwise = m -> PrismDevice.fail(id, m, false);
        ProductDetails known = details.get(product);
        if (known != null) {
            connected(() -> launch(id, known, account), otherwise);
            return;
        }
        List<String> one = new ArrayList<>();
        one.add(product);
        query(one, found -> {
            ProductDetails d = found.get(product);
            if (d == null) PrismDevice.fail(id, "ITEM_UNAVAILABLE\u001fno product " + product + " in Google Play", false);
            else launch(id, d, account);
        }, otherwise);
    }

    private static void launch(long id, ProductDetails d, String account) {
        Activity activity = PrismHost.activity;
        if (activity == null) {
            PrismDevice.fail(id, "unavailable", false);
            return;
        }
        if (buying != -1) {
            PrismDevice.fail(id, "a purchase is already on screen", false);
            return;
        }
        List<BillingFlowParams.ProductDetailsParams> items = new ArrayList<>();
        items.add(BillingFlowParams.ProductDetailsParams.newBuilder()
                .setProductDetails(d).setOfferToken(offer(d).getOfferToken()).build());
        BillingFlowParams.Builder params = BillingFlowParams.newBuilder().setProductDetailsParamsList(items);
        // The app's account, on the purchase (Purchase.getAccountIdentifiers).
        if (account != null) params.setObfuscatedAccountId(account);
        BillingResult r = client.launchBillingFlow(activity, params.build());
        switch (r.getResponseCode()) {
            case BillingClient.BillingResponseCode.OK:
                buying = id;
                buyingProduct = d.getProductId();
                break;
            case BillingClient.BillingResponseCode.ITEM_ALREADY_OWNED:
                PrismDevice.reply(id, "text", "purchased");
                refresh();
                break;
            default: PrismDevice.fail(id, message(r), false);
        }
    }

    /** Play's answer to a purchase flow, or a purchase made elsewhere. */
    private static void updated(BillingResult result, List<Purchase> purchases) {
        boolean changed = false;
        String answer = null;
        for (Purchase p : purchases == null ? Collections.<Purchase>emptyList() : purchases) {
            int state = p.getPurchaseState();
            if (state == Purchase.PurchaseState.PURCHASED) {
                acknowledge(p);
                changed = true;
            }
            if (buyingProduct != null && p.getProducts().contains(buyingProduct)) {
                if (state == Purchase.PurchaseState.PURCHASED) answer = "purchased";
                else if (state == Purchase.PurchaseState.PENDING) answer = "pending";
            }
        }
        if (buying != -1) {
            long id = buying;
            int code = result.getResponseCode();
            if (code == BillingClient.BillingResponseCode.OK && answer != null) {
                PrismDevice.reply(id, "text", answer);
            } else if (code == BillingClient.BillingResponseCode.ITEM_ALREADY_OWNED) {
                PrismDevice.reply(id, "text", "purchased");
                changed = true;
            } else if (code != BillingClient.BillingResponseCode.OK) {
                PrismDevice.fail(id, message(result), false);
            } else {
                id = -1; // another purchase's update: the flow goes on
            }
            if (id != -1) {
                buying = -1;
                buyingProduct = null;
            }
        }
        if (changed) refresh();
    }

    private static void acknowledge(Purchase p) {
        if (p.isAcknowledged()) return;
        client.acknowledgePurchase(
                AcknowledgePurchaseParams.newBuilder().setPurchaseToken(p.getPurchaseToken()).build(),
                r -> {}); // retried at the next query if it failed
    }

    // --- What the user owns -----------------------------------------------------------------

    private static void owned(Consumer<Set<String>> then, Consumer<String> otherwise) {
        connected(() -> client.queryPurchasesAsync(
                QueryPurchasesParams.newBuilder().setProductType(BillingClient.ProductType.INAPP).build(),
                (result, purchases) -> main(() -> {
                    if (result.getResponseCode() != BillingClient.BillingResponseCode.OK) {
                        otherwise.accept(message(result));
                        return;
                    }
                    Set<String> ids = new TreeSet<>();
                    for (Purchase p : purchases) {
                        if (p.getPurchaseState() != Purchase.PurchaseState.PURCHASED) continue;
                        acknowledge(p);
                        ids.addAll(p.getProducts());
                    }
                    then.accept(ids);
                })), otherwise);
    }

    private static void entitlements(long id) {
        owned(ids -> PrismDevice.reply(id, "text", new JSONArray(ids).toString()),
                m -> PrismDevice.fail(id, m, false));
    }

    /** Tells the app's streams what the user owns now. */
    private static void refresh() {
        if (watchers.isEmpty()) return;
        owned(ids -> {
            for (long w : watchers) PrismDevice.streamEvent(w, "entitlements", new JSONArray(ids));
        }, m -> {
            for (long w : watchers) PrismDevice.fail(w, m, true);
        });
    }
}
