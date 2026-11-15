package dev.prism;

import android.app.BroadcastOptions;
import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.os.Build;
import android.util.Base64;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.List;
import java.util.UUID;

/**
 * Remote push for the Rust app (`cx.register_for_push()`), without Gradle:
 *
 * - **FCM** when the app was built with `[push] google_services` (the
 *   Firebase SDK is then bundled: `PrismFcm`, in platform/android/fcm) and
 *   Google Play services is on the device;
 * - otherwise **UnifiedPush** (an open standard, spec AND_3): a distributor
 *   app (ntfy, NextPush, Sunup…) receives Web Push messages for us. This side
 *   is plain framework code: broadcasts in, broadcasts out.
 *
 * Events go to Rust as JSON through `PrismHost.nativePush`, with the app
 * open or not (the library is then loaded without the Activity).
 */
public final class PrismPush {
    static final String REGISTER = "org.unifiedpush.android.distributor.REGISTER";
    static final String UNREGISTER = "org.unifiedpush.android.distributor.UNREGISTER";
    static final String MESSAGE_ACK = "org.unifiedpush.android.distributor.MESSAGE_ACK";
    static final String PREFS = "prism_push";

    private PrismPush() {}

    static SharedPreferences prefs(Context c) {
        return c.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    /** The FCM bridge, when the build bundles Firebase. */
    static Class<?> fcm() {
        try {
            return Class.forName("dev.prism.PrismFcm");
        } catch (ClassNotFoundException e) {
            return null;
        }
    }

    static boolean fcmAvailable(Context c) {
        Class<?> f = fcm();
        if (f == null) return false;
        try {
            return (Boolean) f.getMethod("available", Context.class).invoke(null, c);
        } catch (ReflectiveOperationException e) {
            return false;
        }
    }

    /** UnifiedPush distributors installed on the device (not this app). */
    static List<ResolveInfo> distributors(Context c) {
        List<ResolveInfo> found = c.getPackageManager().queryBroadcastReceivers(
                new Intent(REGISTER), PackageManager.GET_RESOLVED_FILTER);
        found.removeIf(r -> r.activityInfo == null || c.getPackageName().equals(r.activityInfo.packageName));
        return found;
    }

    /** What `cx.capabilities().push` says: "fcm", "unifiedpush" or null. */
    static String service(Context c) {
        if (fcmAvailable(c)) return "fcm";
        if (!distributors(c).isEmpty()) return "unifiedpush";
        return null;
    }

    /** The distributor to use: the one chosen before if still installed, else the first. */
    static String distributor(Context c) {
        List<ResolveInfo> all = distributors(c);
        String saved = prefs(c).getString("distributor", null);
        for (ResolveInfo r : all) {
            if (r.activityInfo.packageName.equals(saved)) return saved;
        }
        return all.isEmpty() ? null : all.get(0).activityInfo.packageName;
    }

    /** Our registration token with the distributor (random, kept). */
    static String token(Context c) {
        SharedPreferences p = prefs(c);
        String t = p.getString("token", null);
        if (t == null) {
            t = UUID.randomUUID().toString();
            p.edit().putString("token", t).apply();
        }
        return t;
    }

    static void register(Context c) {
        if (fcmAvailable(c)) {
            try {
                fcm().getMethod("register", Context.class).invoke(null, c);
                return;
            } catch (ReflectiveOperationException e) {
                android.util.Log.w("prism", "FCM registration failed", e);
            }
        }
        String d = distributor(c);
        if (d == null) {
            error(c, "unavailable");
            return;
        }
        prefs(c).edit().putString("distributor", d).apply();
        Intent i = new Intent(REGISTER).setPackage(d)
                .putExtra("token", token(c))
                .putExtra("message", c.getApplicationInfo().loadLabel(c.getPackageManager()).toString());
        String vapid = meta(c, "prism.vapid");
        if (vapid != null) i.putExtra("vapid", vapid);
        send(c, i);
    }

    static void unregister(Context c) {
        if (fcmAvailable(c)) {
            try {
                fcm().getMethod("unregister", Context.class).invoke(null, c);
            } catch (ReflectiveOperationException ignored) {
            }
            return;
        }
        String d = prefs(c).getString("distributor", null);
        String t = prefs(c).getString("token", null);
        if (d != null && t != null) send(c, new Intent(UNREGISTER).setPackage(d).putExtra("token", t));
        // The token is dropped even without an answer (AND_3).
        prefs(c).edit().remove("token").remove("endpoint").apply();
    }

    /**
     * Sends to the distributor with our identity: Android 14+ shares it with
     * the broadcast; before, a PendingIntent tells our package (AND_3).
     */
    static void send(Context c, Intent i) {
        if (Build.VERSION.SDK_INT >= 34) {
            BroadcastOptions o = BroadcastOptions.makeBasic();
            o.setShareIdentityEnabled(true);
            c.sendBroadcast(i, null, o.toBundle());
        } else {
            Intent dummy = new Intent("org.unifiedpush.android.distributor.dummy").setPackage("org.unifiedpush.dummy_app");
            i.putExtra("pi", PendingIntent.getBroadcast(c, 0, dummy, PendingIntent.FLAG_IMMUTABLE));
            c.sendBroadcast(i);
        }
    }

    static String meta(Context c, String key) {
        try {
            android.os.Bundle m = c.getPackageManager()
                    .getApplicationInfo(c.getPackageName(), PackageManager.GET_META_DATA).metaData;
            return m == null ? null : m.getString(key);
        } catch (PackageManager.NameNotFoundException e) {
            return null;
        }
    }

    /** Hands an event to Rust on the main thread (loading the app if needed). */
    public static void deliver(Context c, JSONObject event) {
        Context app = c.getApplicationContext();
        PrismHost.main.post(() -> {
            PrismHost.load(app);
            PrismHost.nativePush(event.toString(), PrismHost.baseInfo(app).toString());
        });
    }

    static void error(Context c, String why) {
        JSONObject e = new JSONObject();
        try {
            e.put("t", "push_err").put("err", why);
        } catch (JSONException ignored) {
        }
        deliver(c, e);
    }

    /** UnifiedPush connector: the distributor's broadcasts. */
    public static final class Receiver extends BroadcastReceiver {
        @Override
        public void onReceive(Context c, Intent intent) {
            String action = intent.getAction();
            String token = intent.getStringExtra("token");
            // Requests for another token (an old registration) are ignored.
            if (action == null || token == null || !token.equals(prefs(c).getString("token", null))) return;
            JSONObject e = new JSONObject();
            try {
                switch (action) {
                    case "org.unifiedpush.android.connector.NEW_ENDPOINT": {
                        String endpoint = intent.getStringExtra("endpoint");
                        if (endpoint == null) return;
                        prefs(c).edit().putString("endpoint", endpoint).apply();
                        e.put("t", "push_endpoint").put("endpoint", endpoint);
                        break;
                    }
                    case "org.unifiedpush.android.connector.MESSAGE": {
                        byte[] body = intent.getByteArrayExtra("bytesMessage");
                        if (body == null) return;
                        e.put("t", "push_message").put("body", Base64.encodeToString(body, Base64.NO_WRAP));
                        break;
                    }
                    case "org.unifiedpush.android.connector.REGISTRATION_FAILED":
                        e.put("t", "push_err").put("err", "registration failed: " + intent.getStringExtra("reason"));
                        break;
                    case "org.unifiedpush.android.connector.UNREGISTERED":
                        prefs(c).edit().remove("endpoint").apply();
                        e.put("t", "push_err").put("err", "unregistered by the distributor");
                        break;
                    case "org.unifiedpush.android.connector.TEMP_UNAVAILABLE":
                        e.put("t", "push_err").put("err", "distributor temporarily unavailable");
                        break;
                    default:
                        return;
                }
            } catch (JSONException ex) {
                return;
            }
            // Acknowledge, so the distributor does not resend.
            String id = intent.getStringExtra("id");
            String d = prefs(c).getString("distributor", null);
            if (id != null && d != null) {
                send(c, new Intent(MESSAGE_ACK).setPackage(d).putExtra("token", token).putExtra("id", id));
            }
            // Keep the process alive while the app handles it.
            PendingResult pending = goAsync();
            deliver(c, e);
            PrismHost.main.postDelayed(pending::finish, 8000);
        }
    }
}
