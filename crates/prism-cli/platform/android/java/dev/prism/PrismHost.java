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
import android.widget.HorizontalScrollView;
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
    /** A widget button (PrismWidget.Action); `task` is reported back through jobFinished. */
    static native void nativeWidgetAction(String task, String kind, String action, String payload, String info);

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
    /** What the system bars and cutouts cover, top, right, bottom, left (px; #28). */
    static int[] safeInsets = new int[4];
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
        // Form controls (PrismForms): the config string, accessibility.
        String config = "";
        /** A rich text's runs (PrismText), null when plain. */
        JSONArray runs;
        String a11yLabel, a11yValue, announced;
        /** The ARIA role name the app gave (`radio`, `checkbox`…), and its checked state. */
        String a11yRole;
        Boolean a11yChecked;
        // `.sticky()`: the scroll view it follows (-1: none) and its range.
        long stickyScroll = -1;
        float stickyTop, stickyInset, stickyMax;
        // Interaction states the app styles (op `states`), and the
        // listeners they share with taps, menus and swipes.
        java.util.Set<String> states = new java.util.HashSet<>();
        GestureDetector gesture;
        boolean focusRing, touchInstalled;
        // Pointer, keys, hit testing, overlays (PrismInput).
        int pointerMask;
        boolean keysLocal, keysGlobal, noHit, inert, a11yHidden, dragging;
        /** Where the pointer went down: view x, y, then raw x, y (pixels). */
        float[] down = {0, 0, 0, 0};
        android.widget.PopupWindow popup;
        String overlay = "sheet";
        boolean fixed, medium, closing, manual, keepFocus, pendingOpen;
        // Padding (#14): the view's own padding while the app's replaces it,
        // and its own background while `.border(0.)` removes it.
        int[] basePad;
        boolean bare;
        android.graphics.drawable.Drawable baseBackground;
        int[] contentSize = {0, 0};
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
            // What blurred backdrops show may have changed.
            PrismDecor.Backdrops.changed();
        }
    }

    /** Natural size of a leaf, in pixels: {width, height}. */
    public static float[] measure(String kind, String text, float size, int weight, float maxWidth, String spec) {
        if (kind.equals("baseline")) return baseline(text, size, weight, maxWidth);
        float[] form = PrismForms.measure(kind, text, size, weight, maxWidth);
        if (form != null) return form;
        // Texts: styled exactly like PrismText draws them (typography, runs).
        if (kind.equals("text")) return PrismText.measure(text, size > 0 ? size : 17 * density, weight, maxWidth, spec);
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

    /**
     * `items_baseline()`: {0, distance from the top of a `kind` leaf drawn
     * `height` px tall to its first text baseline}, {0, 0} if unknown.
     */
    static float[] baseline(String kind, float size, int weight, float height) {
        TextView v;
        switch (kind) {
            case "text": v = new TextView(activity); v.setPadding(0, 0, 0, 0); break;
            case "timer": v = new TextView(activity); v.setPadding(0, 0, 0, 0); v.setGravity(Gravity.CENTER); break;
            case "button": v = new Button(activity); ((Button) v).setAllCaps(false); break;
            case "text_input": v = new EditText(activity); v.setSingleLine(true); break;
            case "date_picker": {
                View d = PrismForms.dateField(new Node());
                if (!(d instanceof TextView)) return new float[]{0, 0};
                v = (TextView) d;
                break;
            }
            default: return new float[]{0, 0};
        }
        v.setText("Ag");
        v.setTextSize(TypedValue.COMPLEX_UNIT_PX, size > 0 ? size : 17 * density);
        v.setTypeface(Typeface.create(Typeface.DEFAULT, Math.max(100, Math.min(900, weight)), false));
        int unspecified = View.MeasureSpec.makeMeasureSpec(0, View.MeasureSpec.UNSPECIFIED);
        int h = Math.round(height);
        v.measure(unspecified, h > 0 ? View.MeasureSpec.makeMeasureSpec(h, View.MeasureSpec.EXACTLY) : unspecified);
        v.layout(0, 0, v.getMeasuredWidth(), v.getMeasuredHeight());
        int b = v.getBaseline();
        return new float[]{0, Math.max(0, b)};
    }

    // --- Ops --------------------------------------------------------------------------------

    static void op(JSONObject p) throws JSONException {
        String op = p.getString("op");
        long id = p.optLong("id", -1);
        Node n = nodes.get(id);
        switch (op) {
            case "create": {
                Node created = create(id, p.getString("kind"));
                nodes.put(id, created);
                // Text fields always report focus (for focus-within).
                installFocus(created);
                return;
            }
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
            case "layers": PrismInput.setLayers(p.optJSONArray("ids")); return;
            case "blur": PrismInput.dismissKeyboard(); return;
        }
        if (n == null) return;
        switch (op) {
            case "style": {
                n.style = p.getJSONObject("s");
                if (n.kind.equals("scroll")) setScrollAxis(n, n.style.optBoolean("hscroll"));
                JSONObject anim = p.optJSONObject("anim");
                if (anim != null) PrismMotion.style(n.view, anim, () -> { applyStyle(n); applySticky(n); });
                else applyStyle(n);
                applySticky(n);
                // Bar tint (#30).
                if (n.kind.equals("tabs")) refreshTabs(n);
                if (n.kind.equals("stack")) { styleChrome(n); PrismNavbar.refresh(n); }
                break;
            }
            case "sticky": {
                long was = n.stickyScroll;
                n.stickyScroll = p.isNull("scroll") ? -1 : p.optLong("scroll", -1);
                n.stickyTop = (float) p.optDouble("top", 0);
                n.stickyInset = (float) p.optDouble("inset", 0);
                n.stickyMax = (float) p.optDouble("max", 0);
                PrismLayout.setRaised(n.view, n.stickyScroll >= 0);
                if (n.stickyScroll < 0 && was >= 0) PrismMotion.transform(n.view, n.style.optJSONArray("tf"));
                applySticky(n);
                break;
            }
            case "exit": PrismMotion.exit(n.view, p.getJSONObject("to"), p.getJSONObject("anim")); break;
            case "repeat": PrismMotion.repeat(id, n.view, p.optJSONObject("from"), p.optJSONObject("to"), p.optJSONObject("anim"), p.optBoolean("restart")); break;
            case "text": setText(n, p.optString("text")); break;
            case "draw": if (n.view instanceof PrismShape) ((PrismShape) n.view).setPaths(p.optJSONArray("paths")); break;
            case "placeholder":
                if (!PrismForms.placeholder(n, p.optString("text")) && n.view instanceof EditText) ((EditText) n.view).setHint(p.optString("text"));
                break;
            case "config":
                // A screen's bar buttons (#30).
                if (n.kind.equals("screen")) PrismNavbar.config(n, p.optString("config"), p.optJSONArray("glyphs"));
                else PrismForms.config(n, p.optString("config"));
                break;
            case "runs": {
                JSONArray runs = p.optJSONArray("runs");
                n.runs = runs != null && runs.length() > 0 ? runs : null;
                PrismText.render(n);
                break;
            }
            case "icon":
                n.icon = p.optString("icon");
                n.glyph = p.optString("glyph");
                if (n.kind.equals("image")) ((TextView) n.view).setText(n.glyph);
                if (n.kind.equals("text_input")) PrismForms.leadingIcon(n);
                refreshTabs(parentOf(n));
                break;
            case "value": setValue(n, p.optDouble("value")); break;
            case "clickable": n.clickable = p.optBoolean("on"); setClickable(n); applyCursor(n); break;
            case "display": n.view.setVisibility(p.optBoolean("hidden") ? View.GONE : View.VISIBLE); break;
            case "a11y":
                String label = p.optString("label", null);
                if (label != null && !label.equals("null")) {
                    n.view.setContentDescription(label);
                    n.a11yLabel = label;
                }
                n.a11yHidden = p.optBoolean("hidden");
                n.view.setImportantForAccessibility(n.a11yHidden || n.inert
                        ? View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS : View.IMPORTANT_FOR_ACCESSIBILITY_AUTO);
                n.a11yRole = p.isNull("role") ? null : p.optString("role", null);
                n.a11yChecked = p.isNull("checked") ? null : p.optBoolean("checked");
                PrismText.clickable(n);
                PrismForms.a11y(n, n.a11yRole, p.isNull("value") ? null : p.optString("value", null));
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
                if (n.kind.equals("overlay")) {
                    JSONArray c = p.getJSONArray("size");
                    PrismInput.contentSize(n, Math.round((float) c.getDouble(0)), Math.round((float) c.getDouble(1)));
                } else if (n.host != null) {
                    JSONArray c = p.getJSONArray("size");
                    n.host.fixedWidth = Math.round((float) c.getDouble(0));
                    n.host.fixedHeight = Math.round((float) c.getDouble(1));
                    n.host.requestLayout();
                }
                break;
            case "menu": n.menu = p.optJSONObject("menu"); setClickable(n); break;
            case "swipe": n.swipeLeft = p.optBoolean("left"); n.swipeRight = p.optBoolean("right"); setClickable(n); break;
            case "scrollev": n.scrollEvents = p.optBoolean("on"); break;
            case "states": {
                n.states.clear();
                JSONArray on = p.optJSONArray("on");
                for (int i = 0; on != null && i < on.length(); i++) n.states.add(on.optString(i));
                installInteractions(n);
                break;
            }
            case "pointer":
                n.pointerMask = p.optInt("mask");
                setClickable(n);
                PrismInput.hover(n);
                break;
            case "keys": PrismInput.keys(n, p.optBoolean("local"), p.optBoolean("global")); break;
            case "focus": PrismInput.focus(n); break;
            case "reveal": PrismInput.reveal(n); break;
        }
    }

    static Node create(long id, String kind) {
        Node n = createView(id, kind);
        PrismInput.byView.put(n.view, n);
        return n;
    }

    static Node createView(long id, String kind) {
        Node n = new Node();
        n.id = id;
        n.kind = kind;
        n.view = PrismForms.create(n);
        if (n.view != null) return n;
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
                // Completes from the config's suggestions (an EditText without them).
                android.widget.AutoCompleteTextView e = new android.widget.AutoCompleteTextView(activity);
                e.setThreshold(1);
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
                n.host = new PrismLayout(activity);
                n.view = newScroll(n, false);
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
            case "overlay": {
                // Takes no room; its parent is the popover's anchor. The
                // content is shown in a dialog or popup (PrismInput.open).
                n.host = PrismInput.overlayHost(n);
                n.view = new View(activity);
                break;
            }
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
        if (n.dialog != null || n.popup != null) PrismInput.close(n);
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
        // No header bar: for the whole stack (value flag 2, Router::header(false))
        // or while a bare screen (value 1) is on top. The screens' area grows,
        // and the new size is reported to Rust (reportViewport).
        boolean bare = !screens.isEmpty() && screens.get(screens.size() - 1).value == 1;
        boolean hidden = (((int) n.value) & 2) != 0 || bare;
        n.header.setVisibility(hidden ? View.GONE : View.VISIBLE);
        // The top screen's bar buttons (#30).
        PrismNavbar.refresh(n);
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
            b.setTextColor(i == selected ? PrismNavbar.tint(n, accent()) : (dark() ? 0xFF98989F : 0xFF6C6C72));
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
            case "text": PrismText.render(n); break;
            case "button": case "checkbox": ((TextView) n.view).setText(text); break;
            case "text_input": case "text_area": {
                EditText e = (EditText) n.view;
                // Typing echoes back the same text (events reach the app synchronously);
                // only a value from the app (clearing after "Add", a Markdown shortcut)
                // differs, written even while focused, the caret keeping its distance
                // from the end (#73). Runs inside apply(), so the watcher stays quiet.
                if (!e.getText().toString().equals(text)) {
                    int fromEnd = Math.max(0, e.length() - Math.max(0, e.getSelectionEnd()));
                    e.setText(text);
                    e.setSelection(Math.max(0, text.length() - fromEnd));
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
        if (PrismForms.value(n, value)) return;
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
            case "stack": refreshStack(n); break;
            case "screen": {
                Node parent = parentOf(n);
                if (parent != null && parent.kind.equals("stack")) refreshStack(parent);
                break;
            }
            case "overlay": PrismInput.open(n, value != 0); break;
        }
    }

    // --- Styles ---------------------------------------------------------------------------

    /** `system`, `light` or `dark`: the app's choice (`cx.set_appearance`). */
    static String appearance = "system";
    static boolean rtl;
    static String backLabel = "Back";
    /** The app's language (BCP 47), for formatted dates. */
    static String lang = "";

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
        lang = p.isNull("lang") ? "" : p.optString("lang", "");
        View decor = activity.getWindow().getDecorView();
        decor.setLayoutDirection(rtl ? View.LAYOUT_DIRECTION_RTL : View.LAYOUT_DIRECTION_LTR);
        // Each text by its first letter; no letter ("+1", "12:30") reads left to right.
        decor.setTextDirection(View.TEXT_DIRECTION_FIRST_STRONG_LTR);
        for (Node n : nodes.values()) {
            if (n.kind.equals("stack")) styleChrome(n);
            // Dates in the new language.
            if (n.kind.equals("date_picker")) PrismForms.showDate(n);
        }
    }

    /** Light/dark changed (the app's choice or the system's): every color again. */
    static void restyle() {
        for (Node n : nodes.values()) {
            if (n.style.length() > 0) applyStyle(n);
            else PrismForms.style(n);
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
        if (PrismDecor.needed(s)) {
            // Shadows, per-side or dashed borders, gradients, backdrop blur.
            if (!(n.view instanceof Button) || bg != null || s.has("grad")) n.view.setBackground(new PrismDecor.Box(n.view, s));
        } else if (bg != null || border != null || radius > 0) {
            GradientDrawable d = new GradientDrawable();
            d.setColor(bg != null ? bg : Color.TRANSPARENT);
            int corners = s.optInt("corners", 15);
            if (corners == 15) {
                d.setCornerRadius(radius);
            } else {
                // Bits: 1 top left, 2 top right, 4 bottom right, 8 bottom left.
                float tl = (corners & 1) != 0 ? radius : 0, tr = (corners & 2) != 0 ? radius : 0;
                float br = (corners & 4) != 0 ? radius : 0, bl = (corners & 8) != 0 ? radius : 0;
                d.setCornerRadii(new float[] {tl, tl, tr, tr, br, br, bl, bl});
            }
            if (border != null) d.setStroke(Math.max(1, Math.round((float) s.optDouble("bw", 1))), border);
            if (!(n.view instanceof Button) || bg != null) n.view.setBackground(d);
        } else if (n.host != null && !(n.view instanceof ScrollView) && !(n.view instanceof HorizontalScrollView)) {
            n.view.setBackground(null);
        }
        PrismDecor.clip(n);
        applyCursor(n);
        n.view.setAlpha((float) s.optDouble("opacity", 1));
        PrismMotion.transform(n.view, s.optJSONArray("tf"));
        if (n.view instanceof TextView) {
            TextView t = (TextView) n.view;
            Integer fg = color(s.optJSONArray("color"));
            if (fg != null) t.setTextColor(fg);
            float size = (float) s.optDouble("font", 0);
            if (size > 0) t.setTextSize(TypedValue.COMPLEX_UNIT_PX, size);
            t.setTypeface(PrismText.font(s.optString("ff", "system"), s.optInt("weight", 400), s.optBoolean("it")));
            if (!n.kind.equals("image") && !n.kind.equals("timer") && !(t instanceof Button)) {
                String a = s.optString("talign", "left");
                // Absolute: Prism already mirrors alignment for right-to-left languages
                // (START/END would follow each text's own direction instead).
                t.setGravity(a.equals("center") ? Gravity.CENTER_HORIZONTAL : a.equals("right") ? Gravity.RIGHT : Gravity.LEFT);
            }
        }
        // Texts: typography and runs (their colors follow light/dark too).
        if (n.kind.equals("text")) PrismText.render(n);
        PrismForms.style(n);
        applyPadding(n, bg != null || border != null || radius > 0 || PrismDecor.needed(s));
        PrismInput.hitTest(n);
    }

    /**
     * Padding (#14): `pad` [top, right, bottom, left] in place of the view's
     * own (a text's, button's or field's text inset; left and right already
     * mirrored right to left). `bnone` (`.border(0.)`): a field without its
     * own frame (the underline of an input, a date picker's pill).
     */
    static void applyPadding(Node n, boolean painted) {
        View v = n.view;
        boolean field = v instanceof EditText || n.kind.equals("date_picker");
        boolean bare = field && n.style.optBoolean("bnone") && !painted;
        if (bare && !n.bare) {
            n.bare = true;
            n.baseBackground = v.getBackground();
            v.setBackground(null);
        } else if (!bare && n.bare) {
            n.bare = false;
            if (!painted) v.setBackground(n.baseBackground);
            n.baseBackground = null;
        }
        JSONArray pad = n.style.optJSONArray("pad");
        if (pad != null) {
            if (n.basePad == null) {
                n.basePad = new int[]{v.getPaddingLeft(), v.getPaddingTop(), v.getPaddingRight(), v.getPaddingBottom()};
            }
            v.setPadding(Math.round((float) pad.optDouble(3)), Math.round((float) pad.optDouble(0)),
                    Math.round((float) pad.optDouble(1)), Math.round((float) pad.optDouble(2)));
        } else if (n.basePad != null) {
            v.setPadding(n.basePad[0], n.basePad[1], n.basePad[2], n.basePad[3]);
            n.basePad = null;
        }
    }

    // --- Scroll views: axis and sticky children ----------------------------------------------

    /** A vertical or horizontal scroll view around `n.host`, reporting scrolls. */
    static FrameLayout newScroll(Node n, boolean horizontal) {
        FrameLayout s;
        if (horizontal) {
            HorizontalScrollView h = new HorizontalScrollView(activity);
            h.setFillViewport(false);
            s = h;
        } else {
            ScrollView v = new ScrollView(activity);
            v.setFillViewport(false);
            s = v;
        }
        s.addView(n.host, horizontal
                ? new FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.MATCH_PARENT)
                : new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        long id = n.id;
        s.setOnScrollChangeListener((v, x, y, ox, oy) -> {
            if (horizontal) return;
            for (Node c : nodes.values()) if (c.stickyScroll == id) applySticky(c);
            if (!n.scrollEvents) return;
            JSONObject ev = event("scroll", id);
            try { ev.put("o", y); ev.put("v", v.getHeight()); } catch (JSONException ignored) {}
            send(ev);
        });
        return s;
    }

    /** `scroll_x()`: swaps the scroll view for one of the other axis, in place. */
    static void setScrollAxis(Node n, boolean horizontal) {
        if (n.host == null || horizontal == (n.view instanceof HorizontalScrollView)) return;
        View old = n.view;
        ((ViewGroup) old).removeView(n.host);
        FrameLayout s = newScroll(n, horizontal);
        ViewGroup.LayoutParams lp = old.getLayoutParams();
        ViewGroup parent = (ViewGroup) old.getParent();
        if (parent != null) {
            int i = parent.indexOfChild(old);
            parent.removeViewAt(i);
            parent.addView(s, i, lp);
        } else if (lp != null) {
            s.setLayoutParams(lp);
        }
        s.setContentDescription(old.getContentDescription());
        s.setImportantForAccessibility(old.getImportantForAccessibility());
        s.setVisibility(old.getVisibility());
        n.view = s;
        setClickable(n);
    }

    /** Moves a sticky view down as its scroll view scrolls (its frame stays). */
    static void applySticky(Node n) {
        if (n.stickyScroll < 0) return;
        Node scroll = nodes.get(n.stickyScroll);
        if (scroll == null) return;
        float y = scroll.view.getScrollY();
        float offset = Math.max(0, Math.min(Math.max(0, n.stickyMax), y + n.stickyInset - n.stickyTop));
        JSONArray tf = n.style.optJSONArray("tf");
        n.view.setTranslationY((tf == null ? 0 : (float) tf.optDouble(1)) + offset);
    }

    // --- Interaction ------------------------------------------------------------------------

    static void setClickable(Node n) {
        View v = n.view;
        PrismText.clickable(n);
        boolean gestures = n.clickable || n.menu != null || n.swipeLeft || n.swipeRight || n.pointerMask != 0;
        if (!gestures || n.noHit || n.inert) return;
        switch (n.kind) {
            // Controls handle their own touches.
            case "button": case "toggle": case "text_input": case "text_area":
            case "slider": case "picker": case "date_picker": case "checkbox": case "radio_group": return;
        }
        GestureDetector g = new GestureDetector(activity, new GestureDetector.SimpleOnGestureListener() {
            @Override public boolean onDown(MotionEvent e) { return true; }
            @Override public boolean onSingleTapUp(MotionEvent e) {
                // A link inside a clickable text wins over the text's own click.
                if (PrismText.tapLink(n, e)) return true;
                // With its position (ClickEvent::position).
                if (n.clickable) { PrismInput.click(n, e); return true; }
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
        n.gesture = g;
        installTouch(n);
        // Pointer events only: no click, focus or menu.
        if (!n.clickable && n.menu == null) { applyCursor(n); return; }
        v.setClickable(true);
        // Remotes and keyboards (Android TV, D-pad): select clicks, holding it
        // opens the menu. Touches never get here (the gesture detector takes them).
        v.setOnClickListener(n.clickable ? x -> send(event("click", n.id)) : null);
        v.setOnLongClickListener(n.menu != null ? x -> { showMenu(n); return true; } : null);
        v.setFocusable(n.clickable || n.menu != null || n.keysLocal);
        n.focusRing = true;
        installFocus(n);
        applyCursor(n);
    }

    // --- Interaction states (prism_core `interaction`) ---------------------------------------

    /** Reports that `n` entered or left a state (`hover`, `active`, `focus`). */
    static void state(Node n, String s, boolean on) {
        JSONObject ev = event("state", n.id);
        try { ev.put("s", s); ev.put("on", on); } catch (JSONException ignored) {}
        // Not while Prism changes views (focus moves when views go away).
        if (updating) main.post(() -> send(ev));
        else send(ev);
    }

    /** Listens for the states the app styles: hover (mouse, stylus), press, focus. */
    static void installInteractions(Node n) {
        View v = n.view;
        boolean hover = n.states.contains("hover");
        // Containers: over any child too (a child's own hover would end the parent's).
        if (n.host != null && v == n.host) {
            n.host.onHover = hover ? in -> state(n, "hover", in) : null;
            installTouch(n);
            installFocus(n);
            return;
        }
        installHover(n);
        installTouch(n);
        installFocus(n);
    }

    /**
     * One hover listener: the hover state the app styles (leaves; containers
     * use `PrismLayout.onHover`) and pointer events (`on_hover`,
     * `on_pointer_move`, PrismInput).
     */
    static void installHover(Node n) {
        View v = n.view;
        boolean container = n.host != null && v == n.host;
        boolean styled = !container && n.states.contains("hover");
        boolean pointer = (n.pointerMask & (PrismInput.HOVER | PrismInput.MOVE)) != 0;
        if (!styled && !pointer) {
            v.setOnHoverListener(null);
            return;
        }
        v.setOnHoverListener((x, e) -> {
            int a = e.getActionMasked();
            if (styled) {
                if (a == MotionEvent.ACTION_HOVER_ENTER) state(n, "hover", true);
                else if (a == MotionEvent.ACTION_HOVER_EXIT) state(n, "hover", false);
            }
            if (!pointer) return false;
            switch (a) {
                case MotionEvent.ACTION_HOVER_ENTER: PrismInput.pointer(n, "enter", e.getX(), e.getY()); return true;
                case MotionEvent.ACTION_HOVER_MOVE:
                    if ((n.pointerMask & PrismInput.MOVE) != 0) PrismInput.pointer(n, "move", e.getX(), e.getY());
                    return true;
                case MotionEvent.ACTION_HOVER_EXIT: PrismInput.pointer(n, "leave", e.getX(), e.getY()); return true;
            }
            return false;
        });
    }

    /** One touch listener: taps, menus and swipes (the gesture detector) and the pressed state. */
    static void installTouch(Node n) {
        View v = n.view;
        // A text area scrolls its own text with its own listener.
        if (n.kind.equals("text_area")) return;
        boolean active = n.states.contains("active");
        if (n.gesture == null && !active) {
            if (n.touchInstalled) v.setOnTouchListener(null);
            n.touchInstalled = false;
            return;
        }
        n.touchInstalled = true;
        // Containers take the touch to see it end; controls handle their own.
        boolean container = n.host != null && v == n.host;
        v.setOnTouchListener((view, e) -> {
            if (n.states.contains("active")) {
                switch (e.getActionMasked()) {
                    case MotionEvent.ACTION_DOWN: state(n, "active", true); break;
                    case MotionEvent.ACTION_UP: case MotionEvent.ACTION_CANCEL: state(n, "active", false); break;
                }
            }
            // Taps, menus, swipes, and pointer moves and drags (PrismInput).
            if (n.gesture != null) return PrismInput.touch(n, n.gesture, e);
            return container && e.getActionMasked() == MotionEvent.ACTION_DOWN;
        });
    }

    /** One focus listener: the TV focus ring, and the focus state (text fields always). */
    static void installFocus(Node n) {
        boolean field = n.kind.equals("text_input") || n.kind.equals("text_area");
        boolean report = field || n.states.contains("focus");
        if (!report && !n.focusRing) return;
        n.view.setOnFocusChangeListener((view, focused) -> {
            if (n.focusRing) focusEffect(n, focused);
            if (field || n.states.contains("focus")) state(n, "focus", focused);
        });
    }

    /** The style's cursor; a pointing hand over clickable views (mouse, trackpad). */
    static void applyCursor(Node n) {
        String name = n.style.isNull("cursor") ? null : n.style.optString("cursor", null);
        int type;
        if (name == null) {
            type = n.clickable || n.kind.equals("button") ? android.view.PointerIcon.TYPE_HAND : -1;
        } else {
            switch (name) {
                case "pointer": type = android.view.PointerIcon.TYPE_HAND; break;
                case "text": type = android.view.PointerIcon.TYPE_TEXT; break;
                case "crosshair": type = android.view.PointerIcon.TYPE_CROSSHAIR; break;
                case "move": type = android.view.PointerIcon.TYPE_ALL_SCROLL; break;
                case "grab": type = android.view.PointerIcon.TYPE_GRAB; break;
                case "grabbing": type = android.view.PointerIcon.TYPE_GRABBING; break;
                case "not-allowed": type = android.view.PointerIcon.TYPE_NO_DROP; break;
                case "ew-resize": type = android.view.PointerIcon.TYPE_HORIZONTAL_DOUBLE_ARROW; break;
                case "ns-resize": type = android.view.PointerIcon.TYPE_VERTICAL_DOUBLE_ARROW; break;
                case "help": type = android.view.PointerIcon.TYPE_HELP; break;
                case "progress": type = android.view.PointerIcon.TYPE_WAIT; break;
                default: type = android.view.PointerIcon.TYPE_ARROW;
            }
        }
        n.view.setPointerIcon(type < 0 ? null : android.view.PointerIcon.getSystemIcon(activity, type));
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
        view.setTag(src); // the source shown now (a late download checks it)
        if (src.startsWith("data:")) {
            // Bytes in memory (`picture_data`): base64 or percent-encoded.
            int comma = src.indexOf(',');
            try {
                String meta = src.substring(5, Math.max(5, comma));
                String payload = src.substring(comma + 1);
                byte[] bytes = meta.endsWith(";base64")
                        ? android.util.Base64.decode(payload, android.util.Base64.DEFAULT)
                        : java.net.URLDecoder.decode(payload, "UTF-8").getBytes("ISO-8859-1");
                view.setImageBitmap(BitmapFactory.decodeByteArray(bytes, 0, bytes.length));
            } catch (Exception e) {
                android.util.Log.w("prism", "picture data: URL", e);
            }
            return;
        }
        if (src.startsWith("http://") || src.startsWith("https://")) {
            // The URL, then one `name: value` line per header (`Picture::header`).
            String[] lines = src.split("\n");
            String url = lines[0];
            new Thread(() -> {
                try {
                    java.net.HttpURLConnection c = (java.net.HttpURLConnection) new URL(url).openConnection();
                    for (int i = 1; i < lines.length; i++) {
                        int colon = lines[i].indexOf(':');
                        if (colon > 0) {
                            c.setRequestProperty(lines[i].substring(0, colon).trim(), lines[i].substring(colon + 1).trim());
                        }
                    }
                    if (lines.length > 1) c.setUseCaches(false); // private: not in the HTTP cache
                    try (InputStream in = c.getInputStream()) {
                        Bitmap b = BitmapFactory.decodeStream(in);
                        // Only if the view still shows this source.
                        main.post(() -> { if (src.equals(view.getTag())) view.setImageBitmap(b); });
                    } finally {
                        c.disconnect();
                    }
                } catch (Exception e) {
                    // Never log the headers: they hold tokens.
                    android.util.Log.w("prism", "picture " + url, e);
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
