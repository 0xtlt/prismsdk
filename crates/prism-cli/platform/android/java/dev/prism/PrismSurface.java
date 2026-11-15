package dev.prism;

import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.res.ColorStateList;
import android.content.res.Configuration;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.DashPathEffect;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RectF;
import android.graphics.Typeface;
import android.graphics.drawable.Icon;
import android.net.Uri;
import android.os.Build;
import android.os.SystemClock;
import android.text.SpannableString;
import android.text.Spanned;
import android.text.TextPaint;
import android.text.style.StyleSpan;
import android.text.style.TypefaceSpan;
import android.util.DisplayMetrics;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.widget.RemoteViews;

import org.json.JSONArray;
import org.json.JSONObject;

/**
 * Renders Prism static snapshots (compact JSON from prism-core `SNode`) as
 * RemoteViews, for home-screen widgets and Live Activities (ongoing
 * notifications). Mirrors PrismRenderer.swift (iOS) and `snode()` (web).
 *
 * RemoteViews only inflate layout resources: the build script generates
 * `prism_<kind>_<size>.xml` for each kind (col, row, text, chrono, progress,
 * image, space) and size (wrap, fill, growh, growv, both), each rooted at
 * `@id/prism_node`.
 *
 * RemoteViews cannot host custom views, so shapes (charts) are drawn into
 * bitmaps here, at the size the node will most likely get: RemoteViews do
 * not tell their size, so it is estimated from the host's bounds, paddings,
 * fixed sizes and measured text. The image view stretches the bitmap to the
 * real frame (`fitXY`), so an estimate that is a little off only scales it.
 * Bitmaps count against a memory budget (see {@link #widgetBudget} and
 * {@link #NOTIFICATION_BUDGET}); past it they are drawn at a lower resolution.
 */
final class PrismSurface {
    private final Context c;
    private final String pkg;
    private final float density;
    private final boolean dark;
    /** Android 12+: colors resolve against the host's night mode (day/night pairs). */
    private final boolean dayNight = Build.VERSION.SDK_INT >= 31;
    private final boolean rtl;
    /** Bytes of bitmaps this surface may still create. */
    private long budget;
    /** `@id/prism_node`: the root of every generated layout. */
    final int node;

    /**
     * Ongoing notifications: NotificationManagerService warns about custom
     * views over 2 000 000 bytes and strips them over 5 000 000
     * (`config_notificationWarnRemoteViewSizeBytes`,
     * `config_notificationStripRemoteViewSizeBytes`); stay under the warning,
     * for the collapsed and expanded views together.
     */
    static final long NOTIFICATION_BUDGET = 2_000_000L;

    PrismSurface(Context c) {
        this.c = c;
        this.pkg = c.getPackageName();
        this.density = c.getResources().getDisplayMetrics().density;
        this.dark = PrismHost.activity != null ? PrismHost.dark()
                : (c.getResources().getConfiguration().uiMode & Configuration.UI_MODE_NIGHT_MASK)
                        == Configuration.UI_MODE_NIGHT_YES;
        this.rtl = c.getResources().getConfiguration().getLayoutDirection() == View.LAYOUT_DIRECTION_RTL;
        this.node = c.getResources().getIdentifier("prism_node", "id", pkg);
        this.budget = widgetBudget(c);
    }

    /**
     * AppWidgetManager.updateAppWidget: "The combined memory used by the
     * Bitmaps and Icons in the RemoteViews object must not exceed memory that
     * is required to fill the screen 1.5 times, ie. (screen width x screen
     * height x 4 x 1.5) bytes" (an error from Android 17).
     */
    static long widgetBudget(Context c) {
        DisplayMetrics m = c.getResources().getDisplayMetrics();
        return (long) m.widthPixels * m.heightPixels * 4 * 3 / 2;
    }

    PrismSurface budget(long bytes) {
        budget = bytes;
        return this;
    }

    /** The root fills what the host gives it (a widget cell, a notification), `w` × `h` dp. */
    RemoteViews render(JSONObject n, float w, float h) {
        return build(n, "both", w, h);
    }

    private RemoteViews inflate(String kind, String size) {
        int id = c.getResources().getIdentifier("prism_" + kind + "_" + size, "layout", pkg);
        return new RemoteViews(pkg, id);
    }

    private int px(double dp) {
        return Math.round((float) dp * density);
    }

    /** `"#RRGGBB[AA]"`. */
    private static Integer hex(String s) {
        s = s.replace("#", "");
        if (s.length() == 6) s += "FF";
        if (s.length() != 8) return null;
        try {
            long rgba = Long.parseLong(s, 16);
            return Color.argb((int) (rgba & 0xFF), (int) (rgba >> 24 & 0xFF), (int) (rgba >> 16 & 0xFF), (int) (rgba >> 8 & 0xFF));
        } catch (NumberFormatException e) {
            return null;
        }
    }

    /** `"#RRGGBBAA"` or `["#light", "#dark"]` as {light, dark}. */
    private static int[] colors(Object v) {
        if (v instanceof JSONArray) {
            JSONArray pair = (JSONArray) v;
            if (pair.length() < 2) return null;
            Integer l = hex(pair.optString(0)), d = hex(pair.optString(1));
            return l == null || d == null ? null : new int[]{l, d};
        }
        if (!(v instanceof String)) return null;
        Integer c = hex((String) v);
        return c == null ? null : new int[]{c, c};
    }

    /** The color for the current mode (before Android 12, or where a single color is needed). */
    private Integer color(Object v) {
        int[] c = colors(v);
        return c == null ? null : c[dark ? 1 : 0];
    }

    /** Sets an int color with `method`: day/night on Android 12+, else the current mode. */
    private void setColor(RemoteViews v, String method, int[] c) {
        if (dayNight) v.setColorInt(node, method, c[0], c[1]);
        else v.setInt(node, method, c[dark ? 1 : 0]);
    }

    private RemoteViews build(JSONObject n, String size, float w, float h) {
        String k = n.optString("k", "v");
        RemoteViews v;
        switch (k) {
            case "t":
            case "i":
                v = inflate("text", size);
                text(v, n, k.equals("i") ? n.optString("gl", "•") : n.optString("s"));
                break;
            case "m": {
                v = inflate("chrono", size);
                long remaining = Math.max(0, Math.round(n.optDouble("v", 0) * 1000) - System.currentTimeMillis());
                v.setChronometer(node, SystemClock.elapsedRealtime() + remaining, null, remaining > 0);
                v.setChronometerCountDown(node, true);
                text(v, n, null);
                break;
            }
            case "p": {
                v = inflate("progress", size);
                double value = Math.min(1, Math.max(0, n.optDouble("v", 0)));
                v.setProgressBar(node, 1000, (int) Math.round(value * 1000), false);
                int[] fg = colors(n.opt("fg"));
                if (fg != null && dayNight) {
                    v.setColorStateList(node, "setProgressTintList",
                            ColorStateList.valueOf(fg[0]), ColorStateList.valueOf(fg[1]));
                }
                break;
            }
            case "sh":
                v = inflate("image", size);
                shape(v, n, w, h);
                break;
            default:
                v = container(n, size, w, h);
        }
        decorate(v, n);
        return v;
    }

    private void text(RemoteViews v, JSONObject n, String s) {
        if (s != null) {
            SpannableString span = new SpannableString(s);
            int fw = n.optInt("fw", 0);
            if (fw >= 2) span.setSpan(new StyleSpan(Typeface.BOLD), 0, s.length(), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
            else if (fw == 1) span.setSpan(new TypefaceSpan("sans-serif-medium"), 0, s.length(), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
            v.setTextViewText(node, span);
        }
        v.setTextViewTextSize(node, TypedValue.COMPLEX_UNIT_DIP, (float) n.optDouble("fs", 16));
        int[] fg = colors(n.opt("fg"));
        setColor(v, "setTextColor", fg != null ? fg : new int[]{0xFF1C1C1E, 0xFFF2F2F7});
        String ta = n.optString("ta");
        v.setInt(node, "setGravity", ta.equals("c") ? Gravity.CENTER_HORIZONTAL : ta.equals("r") ? Gravity.END : Gravity.START);
    }

    /** A flexbox container as a LinearLayout; gaps and `justify-content` as spacers. */
    private RemoteViews container(JSONObject n, String size, float w, float h) {
        boolean row = n.optInt("d", 0) == 1;
        RemoteViews v = inflate(row ? "row" : "col", size);
        // Hosts reapply updates onto the views they have: start empty.
        v.removeAllViews(node);
        String ai = n.optString("ai"), jc = n.optString("jc");
        int gravity = row
                ? (ai.equals("s") ? Gravity.TOP : ai.equals("e") ? Gravity.BOTTOM : Gravity.CENTER_VERTICAL)
                : (ai.equals("c") ? Gravity.CENTER_HORIZONTAL : ai.equals("e") ? Gravity.END : Gravity.START);
        v.setInt(node, "setGravity", gravity);
        JSONArray kids = n.optJSONArray("c");
        int count = kids == null ? 0 : kids.length();
        double gap = n.optDouble("g", 0);
        String grow = row ? "growh" : "growv";
        // The space inside: what children are laid out in (dp).
        JSONArray pd = n.optJSONArray("pd");
        float innerW = fixed(n, "w", w) - (pd == null ? 0 : (float) (pd.optDouble(1) + pd.optDouble(3)));
        float innerH = fixed(n, "h", h) - (pd == null ? 0 : (float) (pd.optDouble(0) + pd.optDouble(2)));
        // Rows: flexible children share what fixed ones and gaps leave.
        float rest = innerW - (float) gap * Math.max(0, count - 1), weights = 0;
        if (row) {
            for (int i = 0; i < count; i++) {
                JSONObject child = kids.optJSONObject(i);
                if (child == null) continue;
                float weight = flex(child);
                if (weight > 0) weights += weight;
                else rest -= measure(child);
            }
        }
        boolean around = jc.equals("a") || jc.equals("v");
        if (jc.equals("e") || jc.equals("c") || around) v.addView(node, inflate("space", grow));
        for (int i = 0; i < count; i++) {
            JSONObject child = kids.optJSONObject(i);
            if (child == null) continue;
            if (i > 0) {
                if (jc.equals("b") || around) v.addView(node, inflate("space", grow));
                if (gap > 0) v.addView(node, spacer(row, gap));
            }
            String childSize;
            if (child.optDouble("f", 0) > 0) childSize = grow;
            else if (!row && ai.isEmpty()) childSize = "fill"; // align-items: stretch
            else if ("f".equals(child.optString("w"))) childSize = row ? "growh" : "fill";
            else childSize = "wrap";
            float childW = row
                    ? (flex(child) > 0 ? Math.max(0, rest) * flex(child) / weights : measure(child))
                    : innerW - marginX(child);
            v.addView(node, build(child, childSize, childW, innerH));
        }
        if (jc.equals("c") || around) v.addView(node, inflate("space", grow));
        return v;
    }

    /** A fixed size (`w`/`h` in dp), else what the parent offers. */
    private static float fixed(JSONObject n, String key, float offered) {
        Object v = n.opt(key);
        return v instanceof Number ? ((Number) v).floatValue() : offered;
    }

    /** Flex weight in a row: `f`, or 1 for a fill width (laid out as `growh`). */
    private static float flex(JSONObject n) {
        double f = n.optDouble("f", 0);
        if (f > 0) return (float) f;
        return "f".equals(n.optString("w")) ? 1 : 0;
    }

    private static float marginX(JSONObject n) {
        JSONArray m = n.optJSONArray("m");
        return m == null ? 0 : (float) (m.optDouble(1) + m.optDouble(3));
    }

    /** The natural width of a node, in dp (an estimate: what wrap_content would give). */
    private float measure(JSONObject n) {
        Object w = n.opt("w");
        if (w instanceof Number) return ((Number) w).floatValue() + marginX(n);
        JSONArray pd = n.optJSONArray("pd");
        float pad = (pd == null ? 0 : (float) (pd.optDouble(1) + pd.optDouble(3))) + marginX(n);
        String k = n.optString("k", "v");
        float fs = (float) n.optDouble("fs", 16);
        switch (k) {
            case "t":
            case "i":
            case "m": {
                TextPaint p = new TextPaint(Paint.ANTI_ALIAS_FLAG);
                p.setTextSize(fs * density);
                if (n.optInt("fw", 0) >= 2) p.setTypeface(Typeface.DEFAULT_BOLD);
                String s = k.equals("t") ? n.optString("s") : k.equals("i") ? n.optString("gl", "•") : "00:00";
                return p.measureText(s) / density + pad;
            }
            case "p":
            case "sh":
                return pad;
            default: {
                JSONArray kids = n.optJSONArray("c");
                boolean row = n.optInt("d", 0) == 1;
                float total = 0;
                int count = kids == null ? 0 : kids.length();
                for (int i = 0; i < count; i++) {
                    JSONObject child = kids.optJSONObject(i);
                    if (child == null) continue;
                    float cw = measure(child);
                    total = row ? total + cw : Math.max(total, cw);
                }
                if (row && count > 1) total += (float) n.optDouble("g", 0) * (count - 1);
                return total + pad;
            }
        }
    }

    // --- Shapes (charts) as bitmaps ---------------------------------------------------------

    /**
     * Draws the node's drawing at its estimated frame (`w`/`h` when fixed,
     * else what the layout offers), at the screen's density. Android 12+:
     * with light/dark colors, one bitmap per mode and the host picks
     * (`setIcon` with a night variant); before, the current mode.
     */
    private void shape(RemoteViews v, JSONObject n, float w, float h) {
        String drawing = n.optString("s");
        float wd = fixed(n, "w", w), hd = fixed(n, "h", h > 0 ? Math.min(h, 120) : 40);
        int wp = px(wd), hp = px(hd);
        if (wp <= 0 || hp <= 0 || drawing.isEmpty()) return;
        boolean pair = dayNight && drawing.contains("/#");
        long need = (long) wp * hp * 4 * (pair ? 2 : 1);
        float scale = 1;
        if (need > budget) {
            scale = (float) Math.sqrt((double) budget / need);
            if (scale < 0.25f) {
                android.util.Log.w("prism", "surface: no memory left for a " + wp + "x" + hp + " drawing");
                return;
            }
        }
        int bw = Math.max(1, Math.round(wp * scale)), bh = Math.max(1, Math.round(hp * scale));
        budget -= (long) bw * bh * 4 * (pair ? 2 : 1);
        float zoom = density * scale;
        if (pair) {
            v.setIcon(node, "setImageIcon",
                    Icon.createWithBitmap(draw(drawing, bw, bh, zoom, false, rtl)),
                    Icon.createWithBitmap(draw(drawing, bw, bh, zoom, true, rtl)));
        } else {
            v.setImageViewBitmap(node, draw(drawing, bw, bh, zoom, dark, rtl));
        }
    }

    /**
     * Draws a prism-core `Drawing` (text form: `i<x>,<y>` inset, then per
     * path paint tokens `f`/`s` color, `w` width, `r` round, `d` dash, `|`,
     * segments in fractions of the frame: `M x y`, `L x y`, `C …6`,
     * `A cx cy r a0 a1` in degrees clockwise from 12 o'clock, `Z`) into a
     * `w` × `h` px bitmap; widths, dashes and insets are dp × `zoom`.
     */
    static Bitmap draw(String drawing, int w, int h, float zoom, boolean dark, boolean mirror) {
        Bitmap bmp = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888);
        Canvas canvas = new Canvas(bmp);
        if (mirror) canvas.scale(-1, 1, w / 2f, 0);
        float ix = 0, iy = 0;
        Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        for (String line : drawing.split("\n")) {
            if (line.startsWith("i")) {
                String[] v = line.substring(1).split(",");
                ix = parse(v, 0) * zoom;
                iy = parse(v, 1) * zoom;
                continue;
            }
            int bar = line.indexOf('|');
            if (bar < 0) continue;
            Integer fill = null, stroke = null;
            float width = 1;
            boolean round = false;
            float[] dash = null;
            for (String t : line.substring(0, bar).trim().split("\\s+")) {
                if (t.isEmpty()) continue;
                String v = t.substring(1);
                switch (t.charAt(0)) {
                    case 'f': fill = paintColor(v, dark); break;
                    case 's': stroke = paintColor(v, dark); break;
                    case 'w': width = parse(new String[]{v}, 0); break;
                    case 'r': round = true; break;
                    case 'd': {
                        String[] d = v.split(",");
                        if (d.length == 2) dash = new float[]{parse(d, 0) * zoom, parse(d, 1) * zoom};
                        break;
                    }
                    default: break;
                }
            }
            Path path = path(line.substring(bar + 1).trim().split("\\s+"), ix, iy,
                    Math.max(0, w - 2 * ix), Math.max(0, h - 2 * iy));
            if (fill != null) {
                paint.reset();
                paint.setAntiAlias(true);
                paint.setStyle(Paint.Style.FILL);
                paint.setColor(fill);
                canvas.drawPath(path, paint);
            }
            if (stroke != null) {
                paint.reset();
                paint.setAntiAlias(true);
                paint.setStyle(Paint.Style.STROKE);
                paint.setColor(stroke);
                paint.setStrokeWidth(width * zoom);
                if (round) {
                    paint.setStrokeCap(Paint.Cap.ROUND);
                    paint.setStrokeJoin(Paint.Join.ROUND);
                }
                if (dash != null) paint.setPathEffect(new DashPathEffect(dash, 0));
                canvas.drawPath(path, paint);
            }
        }
        return bmp;
    }

    private static float parse(String[] v, int i) {
        try {
            return i < v.length ? Float.parseFloat(v[i]) : 0;
        } catch (NumberFormatException e) {
            return 0;
        }
    }

    /** `#RRGGBB[AA]` or `#light/#dark`. */
    private static Integer paintColor(String spec, boolean dark) {
        String[] parts = spec.split("/");
        return hex(parts[dark && parts.length == 2 ? 1 : 0]);
    }

    /** Segments in fractions of the `iw` × `ih` box at (`ix`, `iy`); radii of its smaller side. */
    private static Path path(String[] t, float ix, float iy, float iw, float ih) {
        Path p = new Path();
        float side = Math.min(iw, ih);
        boolean open = false;
        int i = 0;
        while (i < t.length) {
            String cmd = t[i++];
            int count = cmd.equals("M") || cmd.equals("L") ? 2 : cmd.equals("C") ? 6 : cmd.equals("A") ? 5 : 0;
            if (!cmd.equals("Z") && count == 0) break;
            if (i + count > t.length) break;
            float[] v = new float[count];
            for (int k = 0; k < count; k++) v[k] = parse(t, i + k);
            i += count;
            switch (cmd) {
                case "M":
                    p.moveTo(ix + v[0] * iw, iy + v[1] * ih);
                    open = true;
                    break;
                case "L":
                    if (open) p.lineTo(ix + v[0] * iw, iy + v[1] * ih);
                    else p.moveTo(ix + v[0] * iw, iy + v[1] * ih);
                    open = true;
                    break;
                case "C":
                    if (!open) p.moveTo(ix + v[0] * iw, iy + v[1] * ih);
                    p.cubicTo(ix + v[0] * iw, iy + v[1] * ih, ix + v[2] * iw, iy + v[3] * ih, ix + v[4] * iw, iy + v[5] * ih);
                    open = true;
                    break;
                case "A": {
                    float cx = ix + v[0] * iw, cy = iy + v[1] * ih, r = v[2] * side;
                    double a0 = Math.toRadians(v[3]);
                    float sx = cx + r * (float) Math.sin(a0), sy = cy - r * (float) Math.cos(a0);
                    if (open) p.lineTo(sx, sy);
                    else p.moveTo(sx, sy);
                    open = true;
                    // Android's angles start at 3 o'clock; arcTo treats sweeps mod 360, so
                    // full circles go in halves.
                    RectF oval = new RectF(cx - r, cy - r, cx + r, cy + r);
                    float sweep = Math.max(-360, Math.min(360, v[4] - v[3]));
                    int parts = Math.abs(sweep) > 180 ? 2 : 1;
                    for (int k = 0; k < parts; k++) {
                        p.arcTo(oval, v[3] - 90 + sweep / parts * k, sweep / parts, false);
                    }
                    break;
                }
                default:
                    p.close();
                    open = false;
            }
        }
        return p;
    }

    // --- Layout parameters ------------------------------------------------------------------

    private RemoteViews spacer(boolean row, double gap) {
        RemoteViews s = inflate("space", "wrap");
        int p = px(gap);
        s.setViewPadding(node, row ? p : 0, row ? 0 : p, 0, 0);
        return s;
    }

    private void decorate(RemoteViews v, JSONObject n) {
        JSONArray pd = n.optJSONArray("pd");
        if (pd != null && pd.length() == 4) {
            // [top, right, bottom, left]
            v.setViewPadding(node, px(pd.optDouble(3)), px(pd.optDouble(0)), px(pd.optDouble(1)), px(pd.optDouble(2)));
        }
        if (Build.VERSION.SDK_INT >= 31) {
            if (n.has("w") && !"f".equals(n.optString("w"))) v.setViewLayoutWidth(node, (float) n.optDouble("w"), TypedValue.COMPLEX_UNIT_DIP);
            if (n.has("h") && !"f".equals(n.optString("h"))) v.setViewLayoutHeight(node, (float) n.optDouble("h"), TypedValue.COMPLEX_UNIT_DIP);
            // Margins; a negative one pulls the node over its previous sibling
            // (a chart's label over its ring).
            JSONArray m = n.optJSONArray("m");
            if (m != null && m.length() == 4) {
                int[] sides = {RemoteViews.MARGIN_TOP, RemoteViews.MARGIN_RIGHT, RemoteViews.MARGIN_BOTTOM, RemoteViews.MARGIN_LEFT};
                for (int i = 0; i < 4; i++) {
                    if (m.optDouble(i) != 0) v.setViewLayoutMargin(node, sides[i], (float) m.optDouble(i), TypedValue.COMPLEX_UNIT_DIP);
                }
            }
        }
        int[] bg = colors(n.opt("bg"));
        if (bg != null) {
            setColor(v, "setBackgroundColor", bg);
            double r = n.optDouble("r", 0);
            // RemoteViews outlines round every corner alike: some corners
            // only ("rc") stay square here.
            if (r > 0 && n.optInt("rc", 15) == 15 && Build.VERSION.SDK_INT >= 31) {
                v.setViewOutlinePreferredRadius(node, (float) r, TypedValue.COMPLEX_UNIT_DIP);
                v.setBoolean(node, "setClipToOutline", true);
            }
        }
        if (n.has("o")) v.setFloat(node, "setAlpha", (float) n.optDouble("o", 1));
        if (n.has("al")) v.setContentDescription(node, n.optString("al"));
        String link = n.optString("l", "");
        if (!link.isEmpty()) v.setOnClickPendingIntent(node, open(c, link));
    }

    // --- Live Activities: ongoing notifications ------------------------------------------

    static final String LIVE_CHANNEL = "prism.live";
    static final String LIVE_TAG = "prism.activity";
    /** How long an ended activity stays (iOS keeps them up to four hours). */
    static final long ENDED_FOR_MS = 4 * 3600 * 1000L;

    /** Starts or updates Live Activity `aid` from `{ls, l, tn, sm}`. */
    static void activity(Context c, JSONObject p, boolean ended) {
        String aid = p.optString("aid");
        android.app.NotificationManager nm = c.getSystemService(android.app.NotificationManager.class);
        JSONObject content = p.optJSONObject("content");
        if (content == null || (ended && p.optBoolean("immediately"))) {
            nm.cancel(LIVE_TAG, aid.hashCode());
            return;
        }
        if (nm.getNotificationChannel(LIVE_CHANNEL) == null) {
            // Quiet: it changes often and must not ring.
            android.app.NotificationChannel ch = new android.app.NotificationChannel(
                    LIVE_CHANNEL, "Live", android.app.NotificationManager.IMPORTANCE_LOW);
            nm.createNotificationChannel(ch);
        }
        JSONObject ls = content.optJSONObject("ls");
        // One budget for the collapsed and expanded views.
        PrismSurface s = new PrismSurface(c).budget(NOTIFICATION_BUDGET);
        String summary = content.optString("sm");
        android.app.Notification.Builder b = new android.app.Notification.Builder(c, LIVE_CHANNEL)
                .setSmallIcon(android.R.drawable.ic_popup_reminder)
                .setContentTitle(summary)
                .setOnlyAlertOnce(true)
                .setOngoing(!ended)
                .setShowWhen(false)
                .setCategory(android.app.Notification.CATEGORY_PROGRESS)
                .setContentIntent(open(c, content.optString("l", "/")));
        // The tint colors the notification's icon and app name.
        Integer tint = s.color(content.opt("tn"));
        if (tint != null) b.setColor(tint);
        if (ls != null) {
            // The decorated style's content area: the shade's width less its
            // margins (16 dp each side for the card, 16 dp for the content).
            DisplayMetrics m = c.getResources().getDisplayMetrics();
            float w = Math.min(m.widthPixels, m.heightPixels) / m.density - 64;
            b.setCustomContentView(s.render(ls, w, 64))
                    .setCustomBigContentView(s.render(ls, w, 256))
                    .setStyle(new android.app.Notification.DecoratedCustomViewStyle());
        }
        if (ended) b.setTimeoutAfter(ENDED_FOR_MS);
        android.app.Notification n = b.build();
        PrismDevice.whenAllowed(() -> nm.notify(LIVE_TAG, aid.hashCode(), n));
    }

    /** Opens the app on `path` (a deep link). */
    static PendingIntent open(Context c, String path) {
        Intent i = new Intent(Intent.ACTION_VIEW, Uri.parse("prism:" + path), c, PrismActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_SINGLE_TOP);
        return PendingIntent.getActivity(c, path.hashCode(), i, PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
    }
}
