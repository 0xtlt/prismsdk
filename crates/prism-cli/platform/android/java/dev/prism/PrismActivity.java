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
        android.content.res.Configuration conf = getResources().getConfiguration();
        PrismHost.watch = (conf.uiMode & android.content.res.Configuration.UI_MODE_TYPE_MASK)
                == android.content.res.Configuration.UI_MODE_TYPE_WATCH;
        PrismHost.round = conf.isScreenRound();
        PrismHost.tv = (conf.uiMode & android.content.res.Configuration.UI_MODE_TYPE_MASK)
                == android.content.res.Configuration.UI_MODE_TYPE_TELEVISION;
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
            // A round watch: the corners are outside the screen.
            if (PrismHost.tv) return insets; // The safe area above already covers it.
            int side = PrismHost.round ? Math.round(getResources().getDisplayMetrics().widthPixels * 0.09f) : 0;
            v.setPadding(bars.left + side, bars.top, bars.right + side, bars.bottom);
            // `cx.safe_area()` (#28): the bars and cutouts, not the keyboard.
            android.graphics.Insets sys = insets.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            PrismHost.safeInsets = new int[] {sys.top, sys.right + side, sys.bottom, sys.left + side};
            return insets;
        });
        if (PrismHost.round) {
            // Before any insets arrive (watches have no system bars to report).
            int side = Math.round(getResources().getDisplayMetrics().widthPixels * 0.09f);
            outer.setPadding(side, 0, side, 0);
            PrismHost.safeInsets = new int[] {0, side, 0, side};
        } else if (PrismHost.tv) {
            // Android TV's safe area: TVs may crop the picture's edges (overscan).
            float d = getResources().getDisplayMetrics().density;
            outer.setPadding(Math.round(48 * d), Math.round(27 * d), Math.round(48 * d), Math.round(27 * d));
            PrismHost.safeInsets = new int[] {Math.round(27 * d), Math.round(48 * d), Math.round(27 * d), Math.round(48 * d)};
        }
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
                    PrismHost.send(resized(w, h));
                    // Launched by a notification tap or action while the app was closed.
                    PrismDevice.notificationTapped(getIntent());
                });
            } else {
                JSONObject e = resized(w, h);
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

    /**
     * The root's new size, what surrounds it in the window (`pad`: bars, cutouts and the
     * keyboard) and what of that the system covers for good (`safe`: not the keyboard),
     * top, right, bottom, left, in pixels (#28).
     */
    JSONObject resized(float w, float h) {
        JSONObject e = PrismHost.event("resize", -1);
        try {
            e.put("w", w);
            e.put("h", h);
            android.view.View outer = (android.view.View) PrismHost.root.getParent();
            if (outer != null) {
                e.put("pad", new org.json.JSONArray(new int[] {
                        outer.getPaddingTop(), outer.getPaddingRight(), outer.getPaddingBottom(), outer.getPaddingLeft()}));
            }
            e.put("safe", new org.json.JSONArray(PrismHost.safeInsets));
        } catch (JSONException ignored) {}
        return e;
    }

    /** Keys for Prism (`on_key_down`, `on_global_key_down`): D-pad, keyboards, TV remotes. F5 and Ctrl+R refresh. */
    @Override
    public boolean dispatchKeyEvent(android.view.KeyEvent e) {
        if (ready && (PrismRefresh.key(e) || PrismInput.key(e, getCurrentFocus()))) return true;
        return super.dispatchKeyEvent(e);
    }

    /** Keys held when the window loses the focus get their release (#159). */
    @Override
    public void onWindowFocusChanged(boolean focused) {
        super.onWindowFocusChanged(focused);
        if (ready && !focused) PrismInput.releaseKeys();
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
        // A sign-in's redirect goes to the waiting call, not the router.
        if (PrismSecurity.redirected(intent)) return;
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
        // A sign-in redirect nobody waits for any more (the app was restarted).
        if (path.startsWith("/oauth?") || path.equals("/oauth")) return;
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

    @Override protected void onResume() { super.onResume(); lifecycle("active"); PrismSecurity.resumed(); }
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
            // Form factor: phone/tablet, watch (Wear OS), TV (Android TV), car (Automotive).
            int type = getResources().getConfiguration().uiMode & android.content.res.Configuration.UI_MODE_TYPE_MASK;
            o.put("uiType", type == android.content.res.Configuration.UI_MODE_TYPE_WATCH ? "watch"
                    : type == android.content.res.Configuration.UI_MODE_TYPE_TELEVISION ? "tv"
                    : type == android.content.res.Configuration.UI_MODE_TYPE_CAR ? "car" : "normal");
            o.put("round", getResources().getConfiguration().isScreenRound());
            o.put("touch", pm.hasSystemFeature(PackageManager.FEATURE_TOUCHSCREEN));
            o.put("car", hasClass("dev.prism.PrismCarService"));
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

    private static boolean hasClass(String name) {
        try {
            Class.forName(name);
            return true;
        } catch (ClassNotFoundException e) {
            return false;
        }
    }
}
