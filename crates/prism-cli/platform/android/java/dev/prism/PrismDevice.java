package dev.prism;

import java.util.List;

import java.util.ArrayList;

import android.os.SystemClock;

import android.Manifest;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.RemoteInput;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.ContentValues;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.location.Location;
import android.location.LocationListener;
import android.location.LocationManager;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.CancellationSignal;
import android.os.Looper;
import android.os.VibrationEffect;
import android.os.Vibrator;
import android.provider.MediaStore;
import android.provider.OpenableColumns;
import android.database.Cursor;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.HashMap;
import java.util.Map;
import java.util.UUID;

/**
 * Device APIs and notifications for the Rust app (the same JSON calls as
 * Prism's web client): permissions, location, motion, the photo picker and
 * camera, haptics, clipboard, sharing, links, notifications with actions.
 */
final class PrismDevice {
    private PrismDevice() {}

    static Context ctx() { return PrismHost.context; }

    // --- Answers ----------------------------------------------------------------------------

    static void reply(long id, String key, Object value) {
        JSONObject e = PrismHost.event("device_reply", id);
        try { if (key != null) e.put(key, value); } catch (JSONException ignored) {}
        PrismHost.main.post(() -> PrismHost.send(e));
    }

    static void fail(long id, String message, boolean stream) {
        JSONObject e = PrismHost.event(stream ? "device_event" : "device_reply", id);
        try { e.put("err", message); } catch (JSONException ignored) {}
        PrismHost.main.post(() -> PrismHost.send(e));
    }

    static void streamEvent(long id, String key, Object value) {
        JSONObject e = PrismHost.event("device_event", id);
        try { e.put(key, value); } catch (JSONException ignored) {}
        PrismHost.main.post(() -> PrismHost.send(e));
    }

    // --- Calls ------------------------------------------------------------------------------

    static void call(JSONObject c) {
        long id = c.optLong("id");
        switch (c.optString("call")) {
            case "permission_status": reply(id, "permission", status(c.optString("permission"))); break;
            case "request_permission": request(id, c.optString("permission")); break;
            case "location": location(id, c.optString("accuracy"), false); break;
            case "watch_location": location(id, c.optString("accuracy"), true); break;
            case "watch_motion": motion(id, c.optLong("interval", 100)); break;
            case "stop": stop(id); break;
            case "pick_images": pick(id, c.optInt("limit"), c.optBoolean("camera")); break;
            case "pick_files": pickFiles(id, c.optJSONArray("types"), c.optBoolean("multiple")); break;
            case "haptic": haptic(c.optString("kind")); reply(id, null, null); break;
            case "copy": {
                ClipboardManager cb = (ClipboardManager) ctx().getSystemService(Context.CLIPBOARD_SERVICE);
                cb.setPrimaryClip(ClipData.newPlainText("text", c.optString("text")));
                reply(id, null, null);
                break;
            }
            case "paste": {
                ClipboardManager cb = (ClipboardManager) ctx().getSystemService(Context.CLIPBOARD_SERVICE);
                ClipData clip = cb.getPrimaryClip();
                CharSequence text = clip != null && clip.getItemCount() > 0 ? clip.getItemAt(0).coerceToText(ctx()) : null;
                reply(id, "text", text == null ? JSONObject.NULL : text.toString());
                break;
            }
            case "share": share(c); reply(id, null, null); break;
            case "open_url": {
                try {
                    ctx().startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(c.optString("url"))));
                    reply(id, null, null);
                } catch (Exception e) {
                    fail(id, "unavailable", false);
                }
                break;
            }
            case "secure_get": case "secure_set": case "secure_delete": case "biometry":
            case "authenticate": case "auth_redirect": case "web_auth": case "apple_sign_in":
                PrismSecurity.call(id, c.optString("call"), c);
                break;
            default: fail(id, "unavailable", false);
        }
    }

    // --- Permissions --------------------------------------------------------------------------

    static String[] permissions(String name) {
        switch (name) {
            case "location": return new String[]{Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION};
            case "location_always": return new String[]{Manifest.permission.ACCESS_BACKGROUND_LOCATION};
            case "camera": return new String[]{Manifest.permission.CAMERA};
            case "microphone": return new String[]{Manifest.permission.RECORD_AUDIO};
            case "notifications":
                return Build.VERSION.SDK_INT >= 33 ? new String[]{Manifest.permission.POST_NOTIFICATIONS} : new String[0];
            // The photo picker and motion sensors need no permission.
            default: return new String[0];
        }
    }

    static String status(String name) {
        if (name.equals("motion")) {
            SensorManager s = (SensorManager) ctx().getSystemService(Context.SENSOR_SERVICE);
            return s.getDefaultSensor(Sensor.TYPE_ROTATION_VECTOR) != null ? "granted" : "unavailable";
        }
        String[] perms = permissions(name);
        if (perms.length == 0) return name.equals("photos") || name.equals("notifications") ? "granted" : "unavailable";
        boolean fine = false, any = false;
        for (String p : perms) {
            boolean ok = ctx().checkSelfPermission(p) == PackageManager.PERMISSION_GRANTED;
            any |= ok;
            if (p.equals(Manifest.permission.ACCESS_FINE_LOCATION)) fine = ok;
        }
        if (name.equals("location") && any && !fine) return "limited";
        if (any) return "granted";
        // Android does not tell "never asked" from "denied once": asking again is allowed.
        return "not_determined";
    }

    /** Pending permission requests: request code → (call id, name). */
    static final Map<Integer, Object[]> asking = new HashMap<>();
    static int nextCode = 1;

    static void request(long id, String name) {
        String[] perms = permissions(name);
        if (perms.length == 0 || status(name).equals("granted")) {
            reply(id, "permission", status(name));
            return;
        }
        int code = nextCode++;
        asking.put(code, new Object[]{id, name});
        PrismHost.activity.requestPermissions(perms, code);
    }

    static void permissionResult(int code, String[] permissions, int[] results) {
        if (code == NOTIFY_CODE) {
            boolean granted = status("notifications").equals("granted");
            for (Runnable r : awaitingPermission) if (granted) r.run();
            awaitingPermission.clear();
            return;
        }
        Object[] pending = asking.remove(code);
        if (pending == null) return;
        String state = status((String) pending[1]);
        reply((Long) pending[0], "permission", state.equals("not_determined") ? "denied" : state);
    }

    // --- Location -----------------------------------------------------------------------------

    static final Map<Long, Object> streams = new HashMap<>();

    static JSONObject json(Location l) {
        JSONObject o = new JSONObject();
        try {
            o.put("latitude", l.getLatitude());
            o.put("longitude", l.getLongitude());
            if (l.hasAltitude()) o.put("altitude", l.getAltitude());
            o.put("accuracy", l.hasAccuracy() ? l.getAccuracy() : 0);
            if (l.hasSpeed()) o.put("speed", l.getSpeed());
            if (l.hasBearing()) o.put("heading", l.getBearing());
            o.put("timestamp", l.getTime() / 1000.0);
        } catch (JSONException ignored) {
        }
        return o;
    }

    static String provider(LocationManager lm, String accuracy) {
        boolean gps = lm.isProviderEnabled(LocationManager.GPS_PROVIDER);
        boolean net = lm.isProviderEnabled(LocationManager.NETWORK_PROVIDER);
        if (accuracy.equals("best") && gps) return LocationManager.GPS_PROVIDER;
        if (net) return LocationManager.NETWORK_PROVIDER;
        if (gps) return LocationManager.GPS_PROVIDER;
        return lm.isProviderEnabled(LocationManager.FUSED_PROVIDER) ? LocationManager.FUSED_PROVIDER : null;
    }

    @SuppressWarnings("MissingPermission")
    static void location(long id, String accuracy, boolean stream) {
        if (!status("location").equals("granted") && !status("location").equals("limited")) {
            fail(id, "denied", stream);
            return;
        }
        LocationManager lm = (LocationManager) ctx().getSystemService(Context.LOCATION_SERVICE);
        String p = provider(lm, accuracy);
        if (p == null) {
            fail(id, "unavailable", stream);
            return;
        }
        if (stream) {
            LocationListener listener = l -> streamEvent(id, "location", json(l));
            streams.put(id, listener);
            lm.requestLocationUpdates(p, 1000, 0, listener, Looper.getMainLooper());
        } else {
            lm.getCurrentLocation(p, new CancellationSignal(), ctx().getMainExecutor(), l -> {
                if (l == null) fail(id, "location unknown", false);
                else reply(id, "location", json(l));
            });
        }
    }

    // --- Motion -------------------------------------------------------------------------------

    static void motion(long id, long intervalMs) {
        SensorManager sm = (SensorManager) ctx().getSystemService(Context.SENSOR_SERVICE);
        Sensor rotation = sm.getDefaultSensor(Sensor.TYPE_ROTATION_VECTOR);
        if (rotation == null) {
            fail(id, "unavailable", true);
            return;
        }
        final float[] accel = new float[3], gravity = new float[3], gyro = new float[3], attitude = new float[3];
        final long[] last = {0};
        SensorEventListener l = new SensorEventListener() {
            public void onSensorChanged(SensorEvent e) {
                switch (e.sensor.getType()) {
                    case Sensor.TYPE_LINEAR_ACCELERATION: System.arraycopy(e.values, 0, accel, 0, 3); break;
                    case Sensor.TYPE_GRAVITY: System.arraycopy(e.values, 0, gravity, 0, 3); break;
                    case Sensor.TYPE_GYROSCOPE: System.arraycopy(e.values, 0, gyro, 0, 3); break;
                    case Sensor.TYPE_ROTATION_VECTOR: {
                        float[] m = new float[9], o = new float[3];
                        SensorManager.getRotationMatrixFromVector(m, e.values);
                        SensorManager.getOrientation(m, o); // azimuth, pitch, roll
                        attitude[0] = o[2];
                        attitude[1] = o[1];
                        attitude[2] = o[0];
                        break;
                    }
                }
                long now = System.currentTimeMillis();
                if (now - last[0] < intervalMs) return;
                last[0] = now;
                JSONObject ev = PrismHost.event("device_event", id);
                try {
                    // Prism uses g for accelerations (Android: m/s²).
                    float g = SensorManager.GRAVITY_EARTH;
                    ev.put("acceleration", new JSONArray(new double[]{accel[0] / g, accel[1] / g, accel[2] / g}));
                    ev.put("gravity", new JSONArray(new double[]{gravity[0] / g, gravity[1] / g, gravity[2] / g}));
                    ev.put("rotation_rate", new JSONArray(new double[]{gyro[0], gyro[1], gyro[2]}));
                    ev.put("attitude", new JSONArray(new double[]{attitude[0], attitude[1], attitude[2]}));
                } catch (JSONException ignored) {
                }
                PrismHost.send(ev);
            }

            public void onAccuracyChanged(Sensor s, int a) {}
        };
        int delay = (int) Math.max(10_000, intervalMs * 1000);
        for (int type : new int[]{Sensor.TYPE_ROTATION_VECTOR, Sensor.TYPE_LINEAR_ACCELERATION, Sensor.TYPE_GRAVITY, Sensor.TYPE_GYROSCOPE}) {
            Sensor s = sm.getDefaultSensor(type);
            if (s != null) sm.registerListener(l, s, delay);
        }
        streams.put(id, l);
    }

    static void stop(long id) {
        Object s = streams.remove(id);
        if (s instanceof LocationListener) {
            ((LocationManager) ctx().getSystemService(Context.LOCATION_SERVICE)).removeUpdates((LocationListener) s);
        } else if (s instanceof SensorEventListener) {
            ((SensorManager) ctx().getSystemService(Context.SENSOR_SERVICE)).unregisterListener((SensorEventListener) s);
        }
    }

    // --- Photos and camera --------------------------------------------------------------------

    /** Pending pickers: request code → (call id, camera output uri). */
    static final Map<Integer, Object[]> picking = new HashMap<>();

    static void pick(long id, int limit, boolean camera) {
        int code = 10_000 + nextCode++;
        Intent intent;
        Uri output = null;
        if (camera) {
            if (!ctx().getPackageManager().hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY)) {
                fail(id, "unavailable", false);
                return;
            }
            // The camera app writes into a new MediaStore entry.
            ContentValues v = new ContentValues();
            v.put(MediaStore.Images.Media.DISPLAY_NAME, "Photo-" + System.currentTimeMillis() + ".jpg");
            v.put(MediaStore.Images.Media.MIME_TYPE, "image/jpeg");
            output = ctx().getContentResolver().insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, v);
            intent = new Intent(MediaStore.ACTION_IMAGE_CAPTURE).putExtra(MediaStore.EXTRA_OUTPUT, output);
        } else if (Build.VERSION.SDK_INT >= 33) {
            // The system photo picker: no permission needed.
            intent = new Intent(MediaStore.ACTION_PICK_IMAGES);
            if (limit != 1) intent.putExtra(MediaStore.EXTRA_PICK_IMAGES_MAX, limit > 0 ? Math.min(limit, MediaStore.getPickImagesMaxLimit()) : MediaStore.getPickImagesMaxLimit());
        } else {
            intent = new Intent(Intent.ACTION_GET_CONTENT).setType("image/*").putExtra(Intent.EXTRA_ALLOW_MULTIPLE, limit != 1);
        }
        picking.put(code, new Object[]{id, output});
        try {
            PrismHost.activity.startActivityForResult(intent, code);
        } catch (Exception e) {
            picking.remove(code);
            fail(id, "unavailable", false);
        }
    }

    /**
     * The system's document picker (ACTION_OPEN_DOCUMENT): MIME types, or
     * extensions mapped to theirs. Files are copied like photos.
     */
    static void pickFiles(long id, JSONArray types, boolean multiple) {
        int code = 10_000 + nextCode++;
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE);
        List<String> mimes = new ArrayList<>();
        for (int i = 0; types != null && i < types.length(); i++) {
            String t = types.optString(i).trim().toLowerCase(java.util.Locale.ROOT);
            if (t.startsWith(".")) t = android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(t.substring(1));
            if (t != null && !t.isEmpty()) mimes.add(t);
        }
        intent.setType(mimes.size() == 1 ? mimes.get(0) : "*/*");
        if (mimes.size() > 1) intent.putExtra(Intent.EXTRA_MIME_TYPES, mimes.toArray(new String[0]));
        intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, multiple);
        picking.put(code, new Object[]{id, null, Boolean.TRUE});
        try {
            PrismHost.activity.startActivityForResult(intent, code);
        } catch (Exception e) {
            picking.remove(code);
            fail(id, "unavailable", false);
        }
    }

    static void activityResult(int code, int result, Intent data) {
        Object[] pending = picking.remove(code);
        if (pending == null) return;
        long id = (Long) pending[0];
        if (result != android.app.Activity.RESULT_OK) {
            fail(id, "cancelled", false);
            return;
        }
        java.util.List<Uri> uris = new java.util.ArrayList<>();
        if (pending[1] != null) uris.add((Uri) pending[1]);
        else if (data != null && data.getClipData() != null) {
            for (int i = 0; i < data.getClipData().getItemCount(); i++) uris.add(data.getClipData().getItemAt(i).getUri());
        } else if (data != null && data.getData() != null) uris.add(data.getData());
        JSONArray files = new JSONArray();
        boolean documents = pending.length > 2;
        for (Uri u : uris) {
            JSONObject f = copy(u, documents);
            if (f != null) files.put(f);
        }
        if (files.length() == 0) fail(id, "cancelled", false);
        else reply(id, "files", files);
    }

    /** Copies a picked item into the app's cache: what the app can read. */
    static JSONObject copy(Uri uri) {
        return copy(uri, false);
    }

    static JSONObject copy(Uri uri, boolean document) {
        String name = document ? "file" : "photo.jpg";
        try (Cursor c = ctx().getContentResolver().query(uri, new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (c != null && c.moveToFirst()) name = c.getString(0);
        } catch (Exception ignored) {
        }
        String mime = ctx().getContentResolver().getType(uri);
        File dir = new File(ctx().getCacheDir(), "prism-picked");
        dir.mkdirs();
        String ext = name.contains(".") ? name.substring(name.lastIndexOf('.')) : document ? "" : ".jpg";
        File out = new File(dir, UUID.randomUUID() + ext);
        try (InputStream in = ctx().getContentResolver().openInputStream(uri); OutputStream os = new FileOutputStream(out)) {
            byte[] buf = new byte[64 * 1024];
            int n;
            while ((n = in.read(buf)) > 0) os.write(buf, 0, n);
            JSONObject f = new JSONObject();
            f.put("path", out.getAbsolutePath());
            f.put("name", name);
            f.put("mime", mime != null ? mime : document ? "application/octet-stream" : "image/jpeg");
            f.put("size", out.length());
            return f;
        } catch (Exception e) {
            return null;
        }
    }

    // --- Haptics, sharing ---------------------------------------------------------------------

    /** The device's vibrator (VibratorManager from Android 12). */
    @SuppressWarnings("deprecation")
    static Vibrator vibrator() {
        if (Build.VERSION.SDK_INT >= 31) {
            android.os.VibratorManager vm = (android.os.VibratorManager) ctx().getSystemService(Context.VIBRATOR_MANAGER_SERVICE);
            return vm == null ? null : vm.getDefaultVibrator();
        }
        return (Vibrator) ctx().getSystemService(Context.VIBRATOR_SERVICE);
    }

    static void haptic(String kind) {
        Vibrator v = PrismDevice.vibrator();
        if (v == null || !v.hasVibrator()) return;
        int effect;
        switch (kind) {
            case "selection": case "light": effect = VibrationEffect.EFFECT_TICK; break;
            case "heavy": case "warning": case "error": effect = VibrationEffect.EFFECT_HEAVY_CLICK; break;
            case "success": effect = VibrationEffect.EFFECT_DOUBLE_CLICK; break;
            default: effect = VibrationEffect.EFFECT_CLICK;
        }
        v.vibrate(VibrationEffect.createPredefined(effect));
    }

    static void share(JSONObject c) {
        StringBuilder text = new StringBuilder();
        if (!c.isNull("text")) text.append(c.optString("text"));
        if (!c.isNull("url")) {
            if (text.length() > 0) text.append('\n');
            text.append(c.optString("url"));
        }
        Intent send = new Intent(Intent.ACTION_SEND);
        JSONArray files = c.optJSONArray("files");
        if (files != null && files.length() > 0) {
            // Files go through Prism's own provider (no AndroidX needed).
            java.util.ArrayList<Uri> uris = new java.util.ArrayList<>();
            for (int i = 0; i < files.length(); i++) uris.add(PrismFiles.uriFor(ctx(), new File(files.optString(i))));
            send = new Intent(uris.size() == 1 ? Intent.ACTION_SEND : Intent.ACTION_SEND_MULTIPLE);
            if (uris.size() == 1) send.putExtra(Intent.EXTRA_STREAM, uris.get(0));
            else send.putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris);
            send.setType("*/*");
            send.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        } else {
            send.setType("text/plain");
        }
        if (text.length() > 0) send.putExtra(Intent.EXTRA_TEXT, text.toString());
        ctx().startActivity(Intent.createChooser(send, null));
    }

    // --- Notifications ------------------------------------------------------------------------

    static final String CHANNEL = "prism";
    static final Map<String, Runnable> delayed = new HashMap<>();

    static NotificationManager manager() {
        NotificationManager nm = (NotificationManager) ctx().getSystemService(Context.NOTIFICATION_SERVICE);
        if (nm.getNotificationChannel(CHANNEL) == null) {
            String name = ctx().getApplicationInfo().loadLabel(ctx().getPackageManager()).toString();
            nm.createNotificationChannel(new NotificationChannel(CHANNEL, name, NotificationManager.IMPORTANCE_DEFAULT));
        }
        return nm;
    }

    static void requestNotificationPermission() {
        if (Build.VERSION.SDK_INT >= 33 && !status("notifications").equals("granted")) {
            PrismHost.activity.requestPermissions(new String[]{Manifest.permission.POST_NOTIFICATIONS}, NOTIFY_CODE);
        }
    }

    /** A tap or an action: back to this Activity with the notification's details. */
    static PendingIntent intent(String nid, String action, String link, boolean reply) {
        Intent i = new Intent(ctx(), PrismActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP | Intent.FLAG_ACTIVITY_CLEAR_TOP)
                .putExtra("prism.nid", nid);
        if (action != null) i.putExtra("prism.action", action);
        if (link != null) i.putExtra("prism.link", link);
        // Distinct per action, so their extras do not merge.
        i.setData(Uri.parse("prism-notification://" + Uri.encode(nid) + "/" + Uri.encode(action == null ? "" : action)));
        int flags = PendingIntent.FLAG_UPDATE_CURRENT | (reply ? PendingIntent.FLAG_MUTABLE : PendingIntent.FLAG_IMMUTABLE);
        return PendingIntent.getActivity(ctx(), (nid + "/" + action).hashCode(), i, flags);
    }

    static void notify(JSONObject n) {
        String nid = n.optString("id");
        cancelDelayed(nid);
        Runnable show = () -> {
            Notification.Builder b = new Notification.Builder(ctx(), CHANNEL)
                    .setSmallIcon(android.R.drawable.ic_dialog_info)
                    .setContentTitle(n.optString("title"))
                    .setAutoCancel(true)
                    .setContentIntent(intent(nid, null, n.isNull("link") ? null : n.optString("link"), false));
            String body = n.optString("body");
            if (!n.isNull("subtitle")) b.setSubText(n.optString("subtitle"));
            if (!body.isEmpty()) b.setContentText(body).setStyle(new Notification.BigTextStyle().bigText(body));
            if (!n.isNull("thread")) b.setGroup(n.optString("thread"));
            JSONArray actions = n.optJSONArray("actions");
            for (int i = 0; actions != null && i < actions.length(); i++) {
                JSONObject a = actions.optJSONObject(i);
                String aid = a.optString("id");
                boolean reply = !a.isNull("reply");
                Notification.Action.Builder ab = new Notification.Action.Builder(
                        null, a.optString("title"),
                        intent(nid, aid, a.isNull("link") ? null : a.optString("link"), reply));
                if (reply) ab.addRemoteInput(new RemoteInput.Builder("prism.text").setLabel(a.optString("reply")).build());
                b.addAction(ab.build());
            }
            manager().notify(nid, 0, b.build());
        };
        // `delay` is in seconds (as on every backend).
        long due = SystemClock.uptimeMillis() + Math.round(n.optDouble("delay", 0) * 1000);
        Runnable schedule = () -> {
            delayed.put(nid, show);
            PrismHost.main.postAtTime(() -> {
                delayed.remove(nid);
                show.run();
            }, nid, due);
        };
        whenAllowed(schedule);
    }

    /** Like iOS: the first notification asks for permission, and is posted once granted. */
    static void whenAllowed(Runnable post) {
        if (Build.VERSION.SDK_INT >= 33 && !status("notifications").equals("granted")) {
            // In a background job there is no screen to ask on.
            if (PrismHost.activity == null) return;
            awaitingPermission.add(post);
            if (awaitingPermission.size() == 1) {
                PrismHost.activity.requestPermissions(new String[]{Manifest.permission.POST_NOTIFICATIONS}, NOTIFY_CODE);
            }
        } else {
            post.run();
        }
    }

    static final int NOTIFY_CODE = 0x7072;
    static final List<Runnable> awaitingPermission = new ArrayList<>();

    static void cancelDelayed(String nid) {
        if (delayed.remove(nid) != null) PrismHost.main.removeCallbacksAndMessages(nid);
    }

    static void cancelNotification(String nid) {
        cancelDelayed(nid);
        manager().cancel(nid, 0);
    }

    /** The app was opened from a notification or one of its actions. */
    static void notificationTapped(Intent i) {
        if (i == null || !i.hasExtra("prism.nid")) return;
        String nid = i.getStringExtra("prism.nid");
        manager().cancel(nid, 0);
        JSONObject e = PrismHost.event("notif", -1);
        try {
            e.put("nid", nid);
            if (i.hasExtra("prism.action")) e.put("action", i.getStringExtra("prism.action"));
            if (i.hasExtra("prism.link")) e.put("link", i.getStringExtra("prism.link"));
            Bundle input = RemoteInput.getResultsFromIntent(i);
            if (input != null && input.getCharSequence("prism.text") != null) {
                e.put("text", input.getCharSequence("prism.text").toString());
            }
        } catch (JSONException ignored) {
        }
        PrismHost.send(e);
    }
}
