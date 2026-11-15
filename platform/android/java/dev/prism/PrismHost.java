package dev.prism;

import android.app.Activity;
import android.app.AlertDialog;
import android.app.Dialog;
import android.content.res.Configuration;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.os.Handler;
import android.os.Looper;
import android.text.Editable;
import android.text.TextUtils;
import android.text.TextWatcher;
import android.text.style.RelativeSizeSpan;
import android.text.Spanned;
import android.text.SpannableString;
import android.util.TypedValue;
import android.view.GestureDetector;
import android.view.Gravity;
import android.view.Menu;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.PopupMenu;
import android.widget.ProgressBar;
import android.widget.ScrollView;
import android.widget.Switch;
import android.widget.TextView;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.InputStream;
import java.net.URL;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;

/**
 * Shows the app Prism runs in Rust with real Android views. Rust sends
 * JSON ops (create, style, text, frame…, see prism_native::remote); this
 * class applies them, measures text for Rust's layout, and sends user events
 * back as JSON.
 */
public final class PrismHost {
    // --- Rust ---------------------------------------------------------------------------
    static native void nativeMain();
    static native void nativeReady(float width, float height, float density, String info);
    static native void nativeEvent(String json);
    static native void nativeWake();
    static native void nativeBackground(String task, String info);
    static native void nativeBackgroundExpired(String task);
    /** The car (PrismCarService): `connected` (1/0), `select` (id), `back` (depth). */
    static native void nativeCarEvent(String kind, long value, String info);
    /** Remote push (PrismPush, PrismFcm): a token, an endpoint, a message, an error. */
    static native void nativePush(String json, String info);

    /** The application (also set when a background job starts without the Activity). */
    static android.content.Context context;
    static boolean loaded;

    /** Loads the app's library (named by the Activity's `prism.lib`) and runs its main. */
    static void load(android.content.Context c) {
        context = c.getApplicationContext();
        if (loaded) return;
        String lib = "app";
        try {
            android.content.pm.ActivityInfo info = c.getPackageManager().getActivityInfo(
                    new android.content.ComponentName(c, PrismActivity.class),
                    android.content.pm.PackageManager.GET_META_DATA);
            if (info.metaData != null) lib = info.metaData.getString("prism.lib", lib);
        } catch (android.content.pm.PackageManager.NameNotFoundException ignored) {
        }
        System.loadLibrary(lib);
        nativeMain();
        loaded = true;
    }

    /** What the app needs with or without a screen: language, storage, the device. */
    static JSONObject baseInfo(android.content.Context c) {
        JSONObject o = new JSONObject();
        Configuration conf = c.getResources().getConfiguration();
        try {
            o.put("locale", conf.getLocales().get(0).toLanguageTag());
            o.put("utcOffset", java.util.TimeZone.getDefault().getOffset(System.currentTimeMillis()) / 1000);
            o.put("fontScale", conf.fontScale);
            o.put("reduceMotion", android.provider.Settings.Global.getFloat(c.getContentResolver(),
                    android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f);
            o.put("dataDir", c.getFilesDir().getAbsolutePath());
            o.put("model", android.os.Build.MODEL);
            o.put("manufacturer", android.os.Build.MANUFACTURER);
            o.put("release", android.os.Build.VERSION.RELEASE);
            o.put("emulator", android.os.Build.FINGERPRINT.contains("generic") || android.os.Build.HARDWARE.contains("ranchu"));
            // Owner checks (PrismSecurity): the sensor, a lock screen, a browser for sign-in.
            o.put("biometry", PrismSecurity.kind());
            o.put("deviceSecure", PrismSecurity.deviceSecure());
            o.put("biometricReady", PrismSecurity.canAuthenticate() == android.hardware.biometrics.BiometricManager.BIOMETRIC_SUCCESS);
            o.put("browser", c.getPackageManager().resolveActivity(new android.content.Intent(android.content.Intent.ACTION_VIEW,
                    android.net.Uri.parse("https://example.com")), 0) != null);
            // Remote push: "fcm", "unifiedpush", or none (PrismPush).
            String push = PrismPush.service(c);
            if (push != null) o.put("push", push);
        } catch (JSONException ignored) {
        }
        return o;
    }

    /** The car's templates: PrismCarService exists only in apps with car screens. */
    static void car(String json) {
        try {
            Class.forName("dev.prism.PrismCarService").getDeclaredMethod("set", String.class).invoke(null, json);
        } catch (ReflectiveOperationException ignored) {
        }
    }

    /** From Rust: `cloud_storage` changed; the Backup Service backs it up later (PrismBackup). */
    public static void cloudChanged() {
        if (context != null) new android.app.backup.BackupManager(context).dataChanged();
    }

    /** From Rust: background task `task` is done. */
    public static void jobFinished(String task, boolean ok) {
        PrismJob.finished(task, ok);
    }

    static Activity activity;
    static PrismLayout root;
    static final Handler main = new Handler(Looper.getMainLooper());
    static float density = 1;
    /** Set while Prism changes widgets: their listeners stay quiet. */
    static boolean updating;

    static final Map<Long, Node> nodes = new HashMap<>();
    static long rootId = -1;

    static final float HEADER_DP = 56, TAB_BAR_DP = 64;
    /** Wear OS: compact chrome, icon-only tabs; round screens keep content inside the circle. */
    static boolean watch, round, tv;

    static float headerDp() { return watch ? 38 : HEADER_DP; }

    static float tabBarDp() { return watch ? 40 : TAB_BAR_DP; }

    /** One Prism node and its views. */
    static final class Node {
        long id;
        String kind;
        /** The view placed in the parent. */
        View view;
        /** Where children go (containers, scroll content, stack/tab hosts). */
        PrismLayout host;
        String text = "";
        String icon = "";
        String glyph = "";
        double value;
        boolean clickable;
        boolean swipeLeft, swipeRight, scrollEvents;
        JSONObject style = new JSONObject();
        JSONObject menu;
        List<Long> children = new ArrayList<>();
        // Stacks: the header. Tabs: the bar.
        TextView title;
        Button back;
        View header;
        LinearLayout bar;
        Dialog dialog;
        int[] viewport = {0, 0};
    }

    static void send(JSONObject event) {
        nativeEvent(event.toString());
    }

    static JSONObject event(String type, long id) {
        JSONObject e = new JSONObject();
        try {
            e.put("t", type);
            if (id >= 0) e.put("id", id);
        } catch (JSONException ignored) {
        }
        return e;
    }

    // --- Called from Rust (main thread) ---------------------------------------------------

    /** Wakes the Rust runtime after `delay` ms (any thread). */
    public static void post(long delay) {
        main.postDelayed(PrismHost::nativeWake, Math.max(0, delay));
    }

    public static void apply(String json) {
        updating = true;
        try {
            JSONArray ops = new JSONArray(json);
            for (int i = 0; i < ops.length(); i++) {
                op(ops.getJSONObject(i));
            }
        } catch (JSONException e) {
            android.util.Log.e("prism", "bad ops", e);
        } finally {
            updating = false;
        }
    }

    /** Natural size of a leaf, in pixels: {width, height}. */
    public static float[] measure(String kind, String text, float size, int weight, float maxWidth) {
        TextView v;
        switch (kind) {
            case "button": v = new Button(activity); ((Button) v).setAllCaps(false); break;
            case "text_input": v = new EditText(activity); break;
            case "toggle": {
                Switch s = new Switch(activity);
                s.measure(View.MeasureSpec.UNSPECIFIED, View.MeasureSpec.UNSPECIFIED);
                return new float[]{s.getMeasuredWidth(), s.getMeasuredHeight()};
            }
            case "progress": {
                ProgressBar p = new ProgressBar(activity, null, android.R.attr.progressBarStyleHorizontal);
                p.measure(View.MeasureSpec.UNSPECIFIED, View.MeasureSpec.UNSPECIFIED);
                return new float[]{maxWidth > 0 ? maxWidth : p.getMeasuredWidth(), p.getMeasuredHeight()};
            }
            default: v = new TextView(activity);
        }
        if (kind.equals("text_input")) {
            v.setSingleLine(true);
            v.setText("Ag");
        } else {
            v.setText(text);
        }
        v.setTextSize(TypedValue.COMPLEX_UNIT_PX, size > 0 ? size : 17 * density);
        v.setTypeface(Typeface.create(Typeface.DEFAULT, Math.max(100, Math.min(900, weight)), false));
        if (!kind.equals("button") && !kind.equals("text_input")) v.setPadding(0, 0, 0, 0);
        int widthSpec = maxWidth >= 0
                ? View.MeasureSpec.makeMeasureSpec((int) Math.ceil(maxWidth), View.MeasureSpec.AT_MOST)
                : View.MeasureSpec.makeMeasureSpec(0, View.MeasureSpec.UNSPECIFIED);
        v.measure(widthSpec, View.MeasureSpec.makeMeasureSpec(0, View.MeasureSpec.UNSPECIFIED));
        float w = kind.equals("text_input") && maxWidth >= 0 ? maxWidth : v.getMeasuredWidth();
        return new float[]{w, v.getMeasuredHeight()};
    }

    // --- Ops --------------------------------------------------------------------------------

    static void op(JSONObject p) throws JSONException {
        String op = p.getString("op");
        long id = p.optLong("id", -1);
        Node n = nodes.get(id);
        switch (op) {
            case "create": nodes.put(id, create(id, p.getString("kind"))); return;
            case "destroy": destroy(n); return;
            case "root": setRoot(id); return;
            case "device": PrismDevice.call(p); return;
            case "notify": PrismDevice.notify(p.getJSONObject("n")); return;
            case "notify_cancel": PrismDevice.cancelNotification(p.getString("nid")); return;
            case "notify_permission": PrismDevice.requestNotificationPermission(); return;
            case "alert": alert(p.getJSONObject("alert")); return;
            case "bg_schedule": PrismJob.schedule(context, p); return;
            case "push_register": PrismPush.register(context); return;
            case "push_unregister": PrismPush.unregister(context); return;
            case "activity": PrismSurface.activity(context, p, false); return;
            case "widgets": PrismWidget.reload(context, p.getJSONArray("snaps")); return;
            case "car": car(p.getString("json")); return;
            case "activity_end": PrismSurface.activity(context, p, true); return;
            case "env": setEnvironment(p); return;
            case "appearance": appearance = p.optString("mode", "system"); restyle(); return;
        }
        if (n == null) return;
        switch (op) {
            case "style": {
                n.style = p.getJSONObject("s");
                JSONObject anim = p.optJSONObject("anim");
                if (anim != null) PrismMotion.style(n.view, anim, () -> applyStyle(n));
                else applyStyle(n);
                break;
            }
            case "exit": PrismMotion.exit(n.view, p.getJSONObject("to"), p.getJSONObject("anim")); break;
            case "repeat": PrismMotion.repeat(id, n.view, p.optJSONObject("from"), p.optJSONObject("to"), p.optJSONObject("anim")); break;
            case "text": setText(n, p.optString("text")); break;
            case "draw": if (n.view instanceof PrismShape) ((PrismShape) n.view).setPaths(p.optJSONArray("paths")); break;
            case "placeholder": if (n.view instanceof EditText) ((EditText) n.view).setHint(p.optString("text")); break;
            case "icon":
                n.icon = p.optString("icon");
                n.glyph = p.optString("glyph");
                if (n.kind.equals("image")) ((TextView) n.view).setText(n.glyph);
                refreshTabs(parentOf(n));
                break;
            case "value": setValue(n, p.optDouble("value")); break;
            case "clickable": n.clickable = p.optBoolean("on"); setClickable(n); break;
            case "display": n.view.setVisibility(p.optBoolean("hidden") ? View.GONE : View.VISIBLE); break;
            case "a11y":
                String label = p.optString("label", null);
                if (label != null && !label.equals("null")) n.view.setContentDescription(label);
                n.view.setImportantForAccessibility(p.optBoolean("hidden")
                        ? View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS : View.IMPORTANT_FOR_ACCESSIBILITY_AUTO);
                break;
            case "children": setChildren(n, p.getJSONArray("children")); break;
            case "frame": {
                JSONArray f = p.getJSONArray("f");
                JSONObject anim = p.optJSONObject("anim");
                if (anim != null) {
                    PrismMotion.frame(id, n.view, new int[]{Math.round((float) f.getDouble(0)), Math.round((float) f.getDouble(1)),
                            Math.round((float) f.getDouble(2)), Math.round((float) f.getDouble(3))}, anim);
                    break;
                }
                android.animation.ValueAnimator moving = PrismMotion.frames.remove(id);
                if (moving != null) moving.cancel();
                PrismLayout.Frame frame = PrismLayout.frameOf(n.view);
                frame.x = Math.round((float) f.getDouble(0));
                frame.y = Math.round((float) f.getDouble(1));
                frame.width = Math.round((float) f.getDouble(2));
                frame.height = Math.round((float) f.getDouble(3));
                n.view.setLayoutParams(frame);
                break;
            }
            case "content":
                if (n.host != null) {
                    JSONArray c = p.getJSONArray("size");
                    n.host.fixedWidth = Math.round((float) c.getDouble(0));
                    n.host.fixedHeight = Math.round((float) c.getDouble(1));
                    n.host.requestLayout();
                }
                break;
            case "menu": n.menu = p.optJSONObject("menu"); setClickable(n); break;
            case "swipe": n.swipeLeft = p.optBoolean("left"); n.swipeRight = p.optBoolean("right"); setClickable(n); break;
            case "scrollev": n.scrollEvents = p.optBoolean("on"); break;
        }
    }

    static Node create(long id, String kind) {
        Node n = new Node();
        n.id = id;
        n.kind = kind;
        switch (kind) {
            case "text": {
                TextView t = new TextView(activity);
                t.setPadding(0, 0, 0, 0);
                t.setIncludeFontPadding(true);
                n.view = t;
                break;
            }
            case "button": {
                Button b = new Button(activity);
                b.setAllCaps(false);
                b.setOnClickListener(v -> send(event("click", id)));
                n.view = b;
                break;
            }
            case "text_input": {
                EditText e = new EditText(activity);
                e.setSingleLine(true);
                e.addTextChangedListener(new TextWatcher() {
                    public void beforeTextChanged(CharSequence s, int a, int b, int c) {}
                    public void onTextChanged(CharSequence s, int a, int b, int c) {}
                    public void afterTextChanged(Editable s) {
                        if (updating) return;
                        JSONObject ev = event("input", id);
                        try { ev.put("v", s.toString()); } catch (JSONException ignored) {}
                        send(ev);
                    }
                });
                n.view = e;
                break;
            }
            case "toggle": {
                Switch s = new Switch(activity);
                s.setOnCheckedChangeListener((b, on) -> {
                    if (updating) return;
                    JSONObject ev = event("change", id);
                    try { ev.put("v", on ? 1 : 0); } catch (JSONException ignored) {}
                    send(ev);
                });
                n.view = s;
                break;
            }
            case "progress": {
                ProgressBar p = new ProgressBar(activity, null, android.R.attr.progressBarStyleHorizontal);
                p.setMax(1000);
                n.view = p;
                break;
            }
            case "image": case "timer": {
                TextView t = new TextView(activity);
                t.setPadding(0, 0, 0, 0);
                t.setGravity(Gravity.CENTER);
                n.view = t;
                break;
            }
            case "picture": {
                ImageView i = new ImageView(activity);
                i.setScaleType(ImageView.ScaleType.CENTER_CROP);
                i.setClipToOutline(true);
                n.view = i;
                break;
            }
            case "scroll": {
                ScrollView s = new ScrollView(activity);
                s.setFillViewport(false);
                n.host = new PrismLayout(activity);
                s.addView(n.host, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
                s.setOnScrollChangeListener((v, x, y, ox, oy) -> {
                    if (!n.scrollEvents) return;
                    JSONObject ev = event("scroll", id);
                    try { ev.put("o", y); ev.put("v", v.getHeight()); } catch (JSONException ignored) {}
                    send(ev);
                });
                n.view = s;
                break;
            }
            case "stack": {
                LinearLayout col = new LinearLayout(activity);
                col.setOrientation(LinearLayout.VERTICAL);
                FrameLayout header = new FrameLayout(activity);
                n.back = new Button(activity, null, android.R.attr.borderlessButtonStyle);
                n.back.setAllCaps(false);
                n.back.setTextSize(TypedValue.COMPLEX_UNIT_SP, 24);
                n.header = header;
                n.back.setOnClickListener(v -> back(n));
                n.title = new TextView(activity);
                n.title.setGravity(Gravity.CENTER);
                n.title.setTypeface(Typeface.DEFAULT_BOLD);
                n.title.setTextSize(TypedValue.COMPLEX_UNIT_SP, watch ? 14 : 18);
                if (watch) {
                    // The top of a watch is narrow: one line, inside the circle.
                    // Back is the swipe to the right (the system's back gesture).
                    int inset = Math.round(activity.getResources().getDisplayMetrics().widthPixels * 0.16f);
                    n.title.setPadding(inset, Math.round(6 * density), inset, 0);
                    n.title.setSingleLine(true);
                    n.title.setEllipsize(TextUtils.TruncateAt.END);
                }
                header.addView(n.title, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
                header.addView(n.back, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.MATCH_PARENT, Gravity.START));
                col.addView(header, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, Math.round(headerDp() * density)));
                n.host = new PrismLayout(activity);
                col.addView(n.host, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1));
                reportViewport(n, n.host);
                n.view = col;
                styleChrome(n);
                break;
            }
            case "tabs": {
                LinearLayout col = new LinearLayout(activity);
                col.setOrientation(LinearLayout.VERTICAL);
                n.host = new PrismLayout(activity);
                col.addView(n.host, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1));
                n.bar = new LinearLayout(activity);
                n.bar.setOrientation(LinearLayout.HORIZONTAL);
                col.addView(n.bar, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, Math.round(tabBarDp() * density)));
                // On a round watch the bottom of the circle is narrow: keep the icons inside.
                if (round) {
                    // (The screen's sides are already 9% in.)
                    int inset = Math.round(activity.getResources().getDisplayMetrics().widthPixels * 0.11f);
                    n.bar.setPadding(inset, 0, inset, 0);
                }
                reportViewport(n, n.host);
                n.view = col;
                styleChrome(n);
                break;
            }
            case "modal": {
                n.host = new PrismLayout(activity);
                // Full screen, following the system's light or dark mode.
                Dialog d = new Dialog(activity, dark() ? android.R.style.Theme_DeviceDefault_NoActionBar : android.R.style.Theme_DeviceDefault_Light_NoActionBar);
                d.setContentView(n.host);
                d.setOnCancelListener(x -> {
                    // Dismissed: the modal's stack goes back to nothing.
                    if (!n.children.isEmpty()) {
                        JSONObject ev = event("back", n.children.get(0));
                        try { ev.put("depth", 0); } catch (JSONException ignored) {}
                        send(ev);
                    }
                });
                n.dialog = d;
                reportViewport(n, n.host);
                n.view = n.host;
                break;
            }
            case "shape": n.view = new PrismShape(activity); break;
            default: { // view, screen, tab
                n.host = new PrismLayout(activity);
                n.view = n.host;
            }
        }
        return n;
    }

    /** Tells Rust the area a stack, tab view or modal gives its screens. */
    static void reportViewport(Node n, View area) {
        area.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
            int w = r - l, h = b - t;
            if (w == n.viewport[0] && h == n.viewport[1]) return;
            n.viewport = new int[]{w, h};
            JSONObject ev = event("viewport", n.id);
            try { ev.put("w", w); ev.put("h", h); } catch (JSONException ignored) {}
            main.post(() -> send(ev));
        });
    }

    static void destroy(Node n) {
        if (n == null) return;
        nodes.remove(n.id);
        PrismMotion.forget(n.id);
        if (n.dialog != null) n.dialog.dismiss();
        ViewGroup parent = (ViewGroup) n.view.getParent();
        if (parent != null) parent.removeView(n.view);
        timers.remove(n.id);
    }

    static void setRoot(long id) {
        rootId = id;
        Node n = nodes.get(id);
        if (n == null) return;
        root.removeAllViews();
        PrismLayout.Frame f = PrismLayout.frameOf(n.view);
        root.addView(n.view, f);
    }

    static Node parentOf(Node child) {
        for (Node n : nodes.values()) if (n.children.contains(child.id)) return n;
        return null;
    }

    static void setChildren(Node n, JSONArray ids) throws JSONException {
        n.children.clear();
        List<Node> kids = new ArrayList<>();
        for (int i = 0; i < ids.length(); i++) {
            long cid = ids.getLong(i);
            n.children.add(cid);
            Node c = nodes.get(cid);
            if (c != null) kids.add(c);
        }
        if (n.host == null) return;
        if (n.kind.equals("modal")) {
            n.host.removeAllViews();
            for (Node c : kids) attach(n.host, c);
            n.dialog.show();
            return;
        }
        n.host.removeAllViews();
        for (Node c : kids) {
            if (c.kind.equals("modal")) {
                if (c.dialog != null && !c.dialog.isShowing()) c.dialog.show();
                continue;
            }
            attach(n.host, c);
        }
        if (n.kind.equals("stack")) refreshStack(n);
        if (n.kind.equals("tabs")) refreshTabs(n);
    }

    static void attach(PrismLayout host, Node c) {
        ViewGroup old = (ViewGroup) c.view.getParent();
        if (old != null) old.removeView(c.view);
        host.addView(c.view, PrismLayout.frameOf(c.view));
    }

    /** Only the top screen shows; the header shows its title and Back. */
    static void refreshStack(Node n) {
        List<Node> screens = new ArrayList<>();
        for (long c : n.children) {
            Node s = nodes.get(c);
            if (s != null && !s.kind.equals("modal")) screens.add(s);
        }
        for (int i = 0; i < screens.size(); i++) {
            screens.get(i).view.setVisibility(i == screens.size() - 1 ? View.VISIBLE : View.GONE);
        }
        n.back.setVisibility(screens.size() > 1 && !watch ? View.VISIBLE : View.GONE);
        n.title.setText(screens.isEmpty() ? "" : screens.get(screens.size() - 1).text);
    }

    static void back(Node stack) {
        int depth = 0;
        for (long c : stack.children) {
            Node s = nodes.get(c);
            if (s != null && !s.kind.equals("modal")) depth++;
        }
        JSONObject ev = event("back", stack.id);
        try { ev.put("depth", Math.max(0, depth - 1)); } catch (JSONException ignored) {}
        send(ev);
    }

    /** System Back: the deepest visible stack with something to pop. */
    static boolean handleBack() {
        Node best = null;
        for (Node n : nodes.values()) {
            if (!n.kind.equals("stack") || !n.view.isShown()) continue;
            int screens = 0;
            for (long c : n.children) {
                Node s = nodes.get(c);
                if (s != null && !s.kind.equals("modal")) screens++;
            }
            if (screens > 1) best = n;
        }
        if (best == null) return false;
        back(best);
        return true;
    }

    static int themeColor(int attr) {
        TypedValue v = new TypedValue();
        activity.getTheme().resolveAttribute(attr, v, true);
        return v.resourceId != 0 ? activity.getColor(v.resourceId) : v.data;
    }

    static int accent() { return themeColor(android.R.attr.colorAccent); }

    static void refreshTabs(Node n) {
        if (n == null || !n.kind.equals("tabs")) return;
        n.bar.removeAllViews();
        int selected = (int) n.value;
        for (int i = 0; i < n.children.size(); i++) {
            Node tab = nodes.get(n.children.get(i));
            if (tab == null) continue;
            tab.view.setVisibility(i == selected ? View.VISIBLE : View.GONE);
            Button b = new Button(activity, null, android.R.attr.borderlessButtonStyle);
            b.setAllCaps(false);
            if (TextUtils.isEmpty(tab.glyph)) {
                b.setText(tab.text);
            } else if (watch) {
                // Too narrow for labels: the icon, read aloud with its name.
                b.setText(tab.glyph);
                b.setContentDescription(tab.text);
                b.setPadding(0, 0, 0, 0);
                b.setMinWidth(0);
                b.setMinimumWidth(0);
            } else {
                SpannableString label = new SpannableString(tab.glyph + "\n" + tab.text);
                label.setSpan(new RelativeSizeSpan(1.8f), 0, tab.glyph.length(), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
                b.setText(label);
                b.setContentDescription(tab.text);
            }
            b.setTextSize(TypedValue.COMPLEX_UNIT_SP, watch && !TextUtils.isEmpty(tab.glyph) ? 18 : 12);
            b.setTextColor(i == selected ? accent() : (dark() ? 0xFF98989F : 0xFF6C6C72));
            b.setLineSpacing(0, 0.9f);
            b.setTypeface(i == selected ? Typeface.DEFAULT_BOLD : Typeface.DEFAULT);
            final int index = i;
            b.setOnClickListener(v -> {
                JSONObject ev = event("change", n.id);
                try { ev.put("v", index); } catch (JSONException ignored) {}
                send(ev);
            });
            n.bar.addView(b, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.MATCH_PARENT, 1));
        }
    }

    static void setText(Node n, String text) {
        n.text = text;
        switch (n.kind) {
            case "text": case "button": ((TextView) n.view).setText(text); break;
            case "text_input": {
                EditText e = (EditText) n.view;
                // Typing echoes back the same text; only a change from the app (clearing
                // after "Add") differs. Runs inside apply(), so the watcher stays quiet.
                if (!e.getText().toString().equals(text)) {
                    e.setText(text);
                    e.setSelection(text.length());
                }
                break;
            }
            case "picture": loadPicture((ImageView) n.view, text); break;
            case "screen": {
                Node parent = parentOf(n);
                if (parent != null && parent.kind.equals("stack")) refreshStack(parent);
                break;
            }
            case "tab": refreshTabs(parentOf(n)); break;
        }
    }

    // --- Timers ---------------------------------------------------------------------------

    static final Map<Long, Node> timers = new HashMap<>();
    static boolean ticking;

    static void tick() {
        long now = System.currentTimeMillis();
        for (Node t : timers.values()) {
            long left = Math.max(0, Math.round(t.value - now / 1000.0));
            String s = left >= 3600
                    ? String.format(Locale.ROOT, "%d:%02d:%02d", left / 3600, left / 60 % 60, left % 60)
                    : String.format(Locale.ROOT, "%d:%02d", left / 60, left % 60);
            ((TextView) t.view).setText(s);
        }
        if (!timers.isEmpty()) main.postDelayed(PrismHost::tick, 1000);
        else ticking = false;
    }

    static void setValue(Node n, double value) {
        n.value = value;
        switch (n.kind) {
            case "toggle": ((Switch) n.view).setChecked(value != 0); break;
            case "progress": ((ProgressBar) n.view).setProgress((int) Math.round(value * 1000)); break;
            case "picture":
                ((ImageView) n.view).setScaleType(value == 1 ? ImageView.ScaleType.FIT_CENTER : ImageView.ScaleType.CENTER_CROP);
                break;
            case "timer":
                timers.put(n.id, n);
                if (!ticking) { ticking = true; tick(); }
                break;
            case "tabs": refreshTabs(n); break;
        }
    }

    // --- Styles ---------------------------------------------------------------------------

    /** `system`, `light` or `dark`: the app's choice (`cx.set_appearance`). */
    static String appearance = "system";
    static boolean rtl;
    static String backLabel = "Back";

    static boolean dark() {
        if (appearance.equals("dark")) return true;
        if (appearance.equals("light")) return false;
        // Watches are dark (Wear OS and watchOS alike).
        if (watch) return true;
        return (activity.getResources().getConfiguration().uiMode & Configuration.UI_MODE_NIGHT_MASK)
                == Configuration.UI_MODE_NIGHT_YES;
    }

    /** A context for menus and dialogs in the app's light or dark mode. */
    static android.content.Context themed() {
        return new android.view.ContextThemeWrapper(activity, dark()
                ? android.R.style.Theme_DeviceDefault : android.R.style.Theme_DeviceDefault_Light);
    }

    /** The language's direction: headers, tab bars and Back mirror; frames come mirrored. */
    static void setEnvironment(JSONObject p) {
        rtl = p.optBoolean("rtl");
        backLabel = p.optString("back", "Back");
        View decor = activity.getWindow().getDecorView();
        decor.setLayoutDirection(rtl ? View.LAYOUT_DIRECTION_RTL : View.LAYOUT_DIRECTION_LTR);
        // Each text by its first letter; no letter ("+1", "12:30") reads left to right.
        decor.setTextDirection(View.TEXT_DIRECTION_FIRST_STRONG_LTR);
        for (Node n : nodes.values()) if (n.kind.equals("stack")) styleChrome(n);
    }

    /** Light/dark changed (the app's choice or the system's): every color again. */
    static void restyle() {
        for (Node n : nodes.values()) {
            if (n.style.length() > 0) applyStyle(n);
            if (n.kind.equals("stack") || n.kind.equals("tabs")) styleChrome(n);
            if (n.kind.equals("tabs")) refreshTabs(n);
            if (n.view instanceof PrismShape) n.view.invalidate();
        }
    }

    /** Header and tab bar colors, the status and navigation bars behind them. */
    static void styleChrome(Node n) {
        boolean d = dark();
        // Watches: black, so the screen's edge disappears (Wear OS's convention).
        int bg = watch ? 0xFF000000 : d ? 0xFF1C1C1E : 0xFFF2F2F7, fg = d ? 0xFFF2F2F7 : 0xFF1C1C1E;
        if (n.header != null) n.header.setBackgroundColor(bg);
        if (n.bar != null) n.bar.setBackgroundColor(bg);
        if (n.title != null) n.title.setTextColor(fg);
        if (n.back != null) {
            n.back.setTextColor(fg);
            n.back.setText(rtl ? "→" : "←");
            n.back.setContentDescription(backLabel);
        }
        View outer = (View) root.getParent();
        if (outer != null) outer.setBackgroundColor(bg);
        activity.getWindow().getDecorView().setBackgroundColor(bg);
        int light = android.view.WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS
                | android.view.WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS;
        android.view.WindowInsetsController c = activity.getWindow().getInsetsController();
        if (c != null) c.setSystemBarsAppearance(d ? 0 : light, light);
    }

    /** `[r,g,b,a, dark r,g,b,a]` (0…1) → an Android color. */
    static Integer color(JSONArray c) {
        if (c == null || c.length() < 4) return null;
        int o = dark() && c.length() >= 8 ? 4 : 0;
        return Color.argb((float) c.optDouble(o + 3), (float) c.optDouble(o), (float) c.optDouble(o + 1), (float) c.optDouble(o + 2));
    }

    static void applyStyle(Node n) {
        JSONObject s = n.style;
        Integer bg = color(s.optJSONArray("bg"));
        Integer border = color(s.optJSONArray("bc"));
        float radius = (float) s.optDouble("radius", 0);
        if (bg != null || border != null || radius > 0) {
            GradientDrawable d = new GradientDrawable();
            d.setColor(bg != null ? bg : Color.TRANSPARENT);
            d.setCornerRadius(radius);
            if (border != null) d.setStroke(Math.max(1, Math.round((float) s.optDouble("bw", 1))), border);
            if (!(n.view instanceof Button) || bg != null) n.view.setBackground(d);
            n.view.setClipToOutline(radius > 0);
        } else if (n.host != null && !(n.view instanceof ScrollView)) {
            n.view.setBackground(null);
        }
        n.view.setAlpha((float) s.optDouble("opacity", 1));
        PrismMotion.transform(n.view, s.optJSONArray("tf"));
        if (n.view instanceof TextView) {
            TextView t = (TextView) n.view;
            Integer fg = color(s.optJSONArray("color"));
            if (fg != null) t.setTextColor(fg);
            float size = (float) s.optDouble("font", 0);
            if (size > 0) t.setTextSize(TypedValue.COMPLEX_UNIT_PX, size);
            t.setTypeface(Typeface.create(Typeface.DEFAULT, s.optInt("weight", 400), false));
            if (!n.kind.equals("image") && !n.kind.equals("timer") && !(t instanceof Button)) {
                String a = s.optString("talign", "left");
                // Absolute: Prism already mirrors alignment for right-to-left languages
                // (START/END would follow each text's own direction instead).
                t.setGravity(a.equals("center") ? Gravity.CENTER_HORIZONTAL : a.equals("right") ? Gravity.RIGHT : Gravity.LEFT);
            }
        }
    }

    // --- Interaction ------------------------------------------------------------------------

    static void setClickable(Node n) {
        View v = n.view;
        boolean gestures = n.clickable || n.menu != null || n.swipeLeft || n.swipeRight;
        if (!gestures || n.kind.equals("button") || n.kind.equals("toggle") || n.kind.equals("text_input")) return;
        GestureDetector g = new GestureDetector(activity, new GestureDetector.SimpleOnGestureListener() {
            @Override public boolean onDown(MotionEvent e) { return true; }
            @Override public boolean onSingleTapUp(MotionEvent e) {
                if (n.clickable) { send(event("click", n.id)); return true; }
                return false;
            }
            @Override public void onLongPress(MotionEvent e) { if (n.menu != null) showMenu(n); }
            @Override public boolean onFling(MotionEvent a, MotionEvent b, float vx, float vy) {
                if (Math.abs(vx) < Math.abs(vy) * 2 || Math.abs(vx) < 800) return false;
                String dir = vx < 0 ? "left" : "right";
                if ((vx < 0 && n.swipeLeft) || (vx > 0 && n.swipeRight)) {
                    JSONObject ev = event("swipe", n.id);
                    try { ev.put("dir", dir); } catch (JSONException ignored) {}
                    send(ev);
                    return true;
                }
                return false;
            }
        });
        v.setOnTouchListener((view, e) -> g.onTouchEvent(e));
        v.setClickable(true);
        // Remotes and keyboards (Android TV, D-pad): select clicks, holding it
        // opens the menu. Touches never get here (the gesture detector takes them).
        v.setOnClickListener(n.clickable ? x -> send(event("click", n.id)) : null);
        v.setOnLongClickListener(n.menu != null ? x -> { showMenu(n); return true; } : null);
        v.setFocusable(n.clickable || n.menu != null);
        v.setOnFocusChangeListener((view, focused) -> focusEffect(n, focused));
    }

    /** The focused view rises, like tvOS: visible from the sofa. */
    static void focusEffect(Node n, boolean focused) {
        View v = n.view;
        if (v.isInTouchMode()) return;
        // At most 8%, and no more than 24dp wider (full-width rows stay on screen).
        float grow = v.getWidth() > 0 ? Math.min(1.08f, 1 + 24 * density / v.getWidth()) : 1.08f;
        v.animate().scaleX(focused ? grow : 1f).scaleY(focused ? grow : 1f)
                .translationZ(focused ? 12 * density : 0).setDuration(150).start();
        // A ring in the view's own shape, light on dark and dark on light.
        android.graphics.drawable.GradientDrawable ring = null;
        if (focused) {
            ring = new android.graphics.drawable.GradientDrawable();
            // Styles arrive in pixels (zoomed by Rust).
            ring.setCornerRadius((float) n.style.optDouble("radius", 8 * density));
            ring.setStroke(Math.round(3 * density), dark() ? 0xEEFFFFFF : 0xEE1C1C1E);
        }
        v.setForeground(ring);
    }

    /** The context menu (press and hold), submenus flattened. */
    static void showMenu(Node n) {
        PopupMenu popup = new PopupMenu(themed(), n.view);
        List<Long> ids = new ArrayList<>();
        addItems(popup.getMenu(), n.menu.optJSONArray("items"), ids);
        popup.setOnMenuItemClickListener(item -> {
            send(event("menu", ids.get(item.getItemId())));
            return true;
        });
        popup.show();
    }

    /** Android item ids are ints: they index `ids`, Prism's (u64) item ids. */
    static void addItems(Menu menu, JSONArray items, List<Long> ids) {
        if (items == null) return;
        for (int i = 0; i < items.length(); i++) {
            JSONObject it = items.optJSONObject(i);
            if (it == null) continue;
            switch (it.optString("t")) {
                case "act": {
                    String title = it.optString("title");
                    if (it.optBoolean("checked")) title = "✓ " + title;
                    ids.add(it.optLong("id"));
                    android.view.MenuItem m = menu.add(Menu.NONE, ids.size() - 1, Menu.NONE, title);
                    m.setEnabled(it.optBoolean("enabled", true));
                    break;
                }
                case "sub": {
                    JSONObject sub = it.optJSONObject("menu");
                    addItems(menu.addSubMenu(it.optString("title")), sub == null ? null : sub.optJSONArray("items"), ids);
                    break;
                }
            }
        }
    }

    static void alert(JSONObject a) {
        long id = a.optLong("id");
        JSONArray actions = a.optJSONArray("actions");
        AlertDialog.Builder b = new AlertDialog.Builder(activity, dark()
                ? android.R.style.Theme_DeviceDefault_Dialog_Alert
                : android.R.style.Theme_DeviceDefault_Light_Dialog_Alert).setTitle(a.optString("title"));
        String message = a.optString("message", null);
        int count = actions == null ? 0 : actions.length();
        if (count <= 3) {
            if (message != null && !message.equals("null")) b.setMessage(message);
            for (int i = 0; i < count; i++) {
                JSONObject act = actions.optJSONObject(i);
                final int index = i;
                android.content.DialogInterface.OnClickListener l = (d, w) -> {
                    JSONObject ev = event("alert", id);
                    try { ev.put("index", index); } catch (JSONException ignored) {}
                    send(ev);
                };
                String role = act.optString("role");
                if (role.equals("cancel")) b.setNegativeButton(act.optString("title"), l);
                else if (i == 0 || role.equals("destructive")) b.setPositiveButton(act.optString("title"), l);
                else b.setNeutralButton(act.optString("title"), l);
            }
        } else {
            String[] titles = new String[count];
            for (int i = 0; i < count; i++) titles[i] = actions.optJSONObject(i).optString("title");
            b.setItems(titles, (d, i) -> {
                JSONObject ev = event("alert", id);
                try { ev.put("index", i); } catch (JSONException ignored) {}
                send(ev);
            });
        }
        b.show();
    }

    // --- Pictures ---------------------------------------------------------------------------

    static void loadPicture(ImageView view, String src) {
        if (src.startsWith("http://") || src.startsWith("https://")) {
            new Thread(() -> {
                try (InputStream in = new URL(src).openStream()) {
                    Bitmap b = BitmapFactory.decodeStream(in);
                    main.post(() -> view.setImageBitmap(b));
                } catch (Exception e) {
                    android.util.Log.w("prism", "picture " + src, e);
                }
            }).start();
            return;
        }
        try {
            Bitmap b;
            if (src.startsWith("/")) {
                b = BitmapFactory.decodeFile(src);
            } else {
                try (InputStream in = activity.getAssets().open(src)) {
                    b = BitmapFactory.decodeStream(in);
                }
            }
            view.setImageBitmap(b);
        } catch (Exception e) {
            android.util.Log.w("prism", "picture " + src, e);
        }
    }
}
