package dev.prism;

import android.app.Activity;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.hardware.Sensor;
import android.hardware.SensorManager;
import android.os.Build;
import android.os.Bundle;
import android.os.Vibrator;
import android.util.DisplayMetrics;
import android.view.WindowInsets;
import android.window.OnBackInvokedDispatcher;

import org.json.JSONException;
import org.json.JSONObject;


/** The app's only Activity: loads the Rust library and hosts its views. */
public class PrismActivity extends Activity {
    private boolean ready;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        PrismHost.load(this);
        // Prism draws its own navigation bars.
        if (getActionBar() != null) getActionBar().hide();
        PrismHost.activity = this;
        PrismHost.density = getResources().getDisplayMetrics().density;
        PrismHost.root = new PrismLayout(this);
        // Edge to edge is the default from Android 15: keep content inside
        // the system bars and above the keyboard.
        android.widget.FrameLayout outer = new android.widget.FrameLayout(this);
        outer.setOnApplyWindowInsetsListener((v, insets) -> {
            android.graphics.Insets bars = insets.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.ime() | WindowInsets.Type.displayCutout());
            v.setPadding(bars.left, bars.top, bars.right, bars.bottom);
            return insets;
        });
        outer.addView(PrismHost.root, new android.widget.FrameLayout.LayoutParams(-1, -1));
        setContentView(outer);
        PrismHost.root.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
            float ow = or - ol, oh = ob - ot;
            float w = r - l, h = b - t;
            if (w <= 0 || h <= 0 || (w == ow && h == oh && ready)) return;
            if (!ready) {
                ready = true;
                PrismHost.main.post(() -> {
                    PrismHost.nativeReady(w, h, PrismHost.density, deviceInfo());
                    // Launched by a notification tap or action while the app was closed.
                    PrismDevice.notificationTapped(getIntent());
                });
            } else {
                JSONObject e = PrismHost.event("resize", -1);
                try { e.put("w", w); e.put("h", h); } catch (JSONException ignored) {}
                PrismHost.main.post(() -> PrismHost.send(e));
            }
        });
        if (Build.VERSION.SDK_INT >= 33) {
            getOnBackInvokedDispatcher().registerOnBackInvokedCallback(
                    OnBackInvokedDispatcher.PRIORITY_DEFAULT, () -> {
                        if (!PrismHost.handleBack()) finish();
                    });
        }
        handleLink(getIntent());
    }

    @Override
    @SuppressWarnings("deprecation")
    public void onBackPressed() {
        if (!PrismHost.handleBack()) super.onBackPressed();
    }

    /** The system's dark mode changed (the Activity stays: `configChanges`). */
    @Override
    public void onConfigurationChanged(android.content.res.Configuration config) {
        super.onConfigurationChanged(config);
        if (ready) PrismHost.restyle();
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        handleLink(intent);
        PrismDevice.notificationTapped(intent);
    }

    /**
     * The path of a deep link (`<scheme>:/todos/1`), or null. A notification's intent
     * also carries a URI (to keep its actions apart): that one is not a link.
     */
    static String linkPath(Intent intent) {
        if (intent == null || intent.getData() == null || intent.hasExtra("prism.nid")) return null;
        String s = intent.getData().toString();
        int colon = s.indexOf(':');
        String path = colon >= 0 ? s.substring(colon + 1).replaceFirst("^//", "") : s;
        return path.startsWith("/") ? path : "/" + path;
    }

    private void handleLink(Intent intent) {
        String path = linkPath(intent);
        if (path == null || !ready) return;
        JSONObject e = PrismHost.event("link", -1);
        try { e.put("path", path); } catch (JSONException ignored) {}
        PrismHost.send(e);
    }

    private void lifecycle(String state) {
        if (!ready) return;
        JSONObject e = PrismHost.event("lifecycle", -1);
        try { e.put("state", state); } catch (JSONException ignored) {}
        PrismHost.send(e);
    }

    @Override protected void onResume() { super.onResume(); lifecycle("active"); }
    @Override protected void onPause() { super.onPause(); lifecycle("inactive"); }
    @Override protected void onStop() { super.onStop(); lifecycle("background"); }

    @Override
    public void onRequestPermissionsResult(int code, String[] permissions, int[] results) {
        PrismDevice.permissionResult(code, permissions, results);
    }

    @Override
    protected void onActivityResult(int code, int result, Intent data) {
        PrismDevice.activityResult(code, result, data);
    }

    /** What Rust needs about the device and the environment. */
    private String deviceInfo() {
        JSONObject o = PrismHost.baseInfo(this);
        DisplayMetrics m = getResources().getDisplayMetrics();
        PackageManager pm = getPackageManager();
        SensorManager sensors = (SensorManager) getSystemService(SENSOR_SERVICE);
        Vibrator vibrator = PrismDevice.vibrator();
        try {
            android.view.WindowMetrics wm = getWindowManager().getMaximumWindowMetrics();
            o.put("widthPx", wm.getBounds().width());
            o.put("heightPx", wm.getBounds().height());
            o.put("xdpi", m.xdpi);
            o.put("ydpi", m.ydpi);
            o.put("refresh", getDisplay() != null ? getDisplay().getRefreshRate() : 60);
            o.put("smallestWidthDp", getResources().getConfiguration().smallestScreenWidthDp);
            o.put("multiWindow", isInMultiWindowMode());
            o.put("cutout", getDisplay() != null && getDisplay().getCutout() != null);
            o.put("location", pm.hasSystemFeature(PackageManager.FEATURE_LOCATION));
            o.put("gyroscope", sensors != null && sensors.getDefaultSensor(Sensor.TYPE_GYROSCOPE) != null);
            o.put("camera", pm.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY));
            o.put("vibrator", vibrator != null && vibrator.hasVibrator());
            Intent intent = getIntent();
            String path = linkPath(intent);
            if (path != null) o.put("path", path);
        } catch (JSONException ignored) {
        }
        return o.toString();
    }
}
