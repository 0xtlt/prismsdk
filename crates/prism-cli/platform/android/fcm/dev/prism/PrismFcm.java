package dev.prism;

import android.content.Context;

import com.google.android.gms.common.ConnectionResult;
import com.google.android.gms.common.GoogleApiAvailability;
import com.google.firebase.messaging.FirebaseMessaging;
import com.google.firebase.messaging.FirebaseMessagingService;
import com.google.firebase.messaging.RemoteMessage;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.Map;

/**
 * Firebase Cloud Messaging, compiled only into apps built with
 * `[push] google_services` (the Firebase SDK is then fetched from Google's
 * Maven and bundled: scripts/android-run.sh). PrismPush finds it by name.
 *
 * Prism's server sends data messages: they arrive here in the foreground and
 * the background, and Rust shows visible ones (as UnifiedPush does).
 * Notification messages (Firebase console) are shown by the SDK itself while
 * the app is in the background, and reach the app when tapped.
 */
public final class PrismFcm extends FirebaseMessagingService {
    /** Firebase is configured (google-services values) and Play services present. */
    public static boolean available(Context c) {
        return GoogleApiAvailability.getInstance().isGooglePlayServicesAvailable(c) == ConnectionResult.SUCCESS
                && c.getResources().getIdentifier("google_app_id", "string", c.getPackageName()) != 0;
    }

    public static void register(Context c) {
        FirebaseMessaging.getInstance().getToken().addOnCompleteListener(task -> {
            JSONObject e = new JSONObject();
            try {
                if (task.isSuccessful() && task.getResult() != null) {
                    e.put("t", "push_token").put("token", task.getResult());
                } else {
                    Exception ex = task.getException();
                    e.put("t", "push_err").put("err", ex == null ? "FCM registration failed" : ex.getMessage());
                }
            } catch (JSONException ignored) {
            }
            PrismPush.deliver(c, e);
        });
    }

    public static void unregister(Context c) {
        FirebaseMessaging.getInstance().deleteToken();
    }

    @Override
    public void onNewToken(String token) {
        JSONObject e = new JSONObject();
        try {
            e.put("t", "push_token").put("token", token);
        } catch (JSONException ignored) {
        }
        PrismPush.deliver(this, e);
    }

    @Override
    public void onMessageReceived(RemoteMessage m) {
        // Data values are strings: Prism's payload keys plus custom data.
        JSONObject payload = new JSONObject();
        try {
            for (Map.Entry<String, String> kv : m.getData().entrySet()) payload.put(kv.getKey(), kv.getValue());
            RemoteMessage.Notification n = m.getNotification();
            if (n != null) {
                if (n.getTitle() != null) payload.put("title", n.getTitle());
                if (n.getBody() != null) payload.put("body", n.getBody());
            }
            if (m.getMessageId() != null && !payload.has("id")) payload.put("id", m.getMessageId());
            PrismPush.deliver(this, new JSONObject().put("t", "push_data").put("payload", payload.toString()));
        } catch (JSONException ignored) {
        }
    }
}
