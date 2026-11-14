package dev.prism;

import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.res.ColorStateList;
import android.content.res.Configuration;
import android.graphics.Color;
import android.graphics.Typeface;
import android.net.Uri;
import android.os.Build;
import android.os.SystemClock;
import android.text.SpannableString;
import android.text.Spanned;
import android.text.style.StyleSpan;
import android.text.style.TypefaceSpan;
import android.util.TypedValue;
import android.view.Gravity;
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
 * space) and size (wrap, fill, growh, growv, both), each rooted at `@id/prism_node`.
 */
final class PrismSurface {
    private final Context c;
    private final String pkg;
    private final float density;
    private final boolean dark;
    /** `@id/prism_node`: the root of every generated layout. */
    final int node;

    PrismSurface(Context c) {
        this.c = c;
        this.pkg = c.getPackageName();
        this.density = c.getResources().getDisplayMetrics().density;
        this.dark = PrismHost.activity != null ? PrismHost.dark()
                : (c.getResources().getConfiguration().uiMode & Configuration.UI_MODE_NIGHT_MASK)
                        == Configuration.UI_MODE_NIGHT_YES;
        this.node = c.getResources().getIdentifier("prism_node", "id", pkg);
    }

    /** The root fills what the host gives it (a widget cell, a notification). */
    RemoteViews render(JSONObject n) {
        return build(n, "both");
    }

    private RemoteViews inflate(String kind, String size) {
        int id = c.getResources().getIdentifier("prism_" + kind + "_" + size, "layout", pkg);
        return new RemoteViews(pkg, id);
    }

    private int px(double dp) {
        return Math.round((float) dp * density);
    }

    /** `"#RRGGBBAA"` or `["#light", "#dark"]`. */
    private Integer color(Object v) {
        if (v instanceof JSONArray) {
            JSONArray pair = (JSONArray) v;
            if (pair.length() < 2) return null;
            v = pair.optString(dark ? 1 : 0);
        }
        if (!(v instanceof String)) return null;
        String s = ((String) v).replace("#", "");
        if (s.length() == 6) s += "FF";
        if (s.length() != 8) return null;
        long rgba = Long.parseLong(s, 16);
        return Color.argb((int) (rgba & 0xFF), (int) (rgba >> 24 & 0xFF), (int) (rgba >> 16 & 0xFF), (int) (rgba >> 8 & 0xFF));
    }

    private RemoteViews build(JSONObject n, String size) {
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
                Integer fg = color(n.opt("fg"));
                if (fg != null && Build.VERSION.SDK_INT >= 31) {
                    v.setColorStateList(node, "setProgressTintList", ColorStateList.valueOf(fg));
                }
                break;
            }
            default:
                v = container(n, size);
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
        Integer fg = color(n.opt("fg"));
        v.setTextColor(node, fg != null ? fg : (dark ? 0xFFF2F2F7 : 0xFF1C1C1E));
        String ta = n.optString("ta");
        v.setInt(node, "setGravity", ta.equals("c") ? Gravity.CENTER_HORIZONTAL : ta.equals("r") ? Gravity.END : Gravity.START);
    }

    /** A flexbox container as a LinearLayout; gaps and `justify-content` as spacers. */
    private RemoteViews container(JSONObject n, String size) {
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
            v.addView(node, build(child, childSize));
        }
        if (jc.equals("c") || around) v.addView(node, inflate("space", grow));
        return v;
    }

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
        }
        Integer bg = color(n.opt("bg"));
        if (bg != null) {
            v.setInt(node, "setBackgroundColor", bg);
            double r = n.optDouble("r", 0);
            if (r > 0 && Build.VERSION.SDK_INT >= 31) {
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
        PrismSurface s = new PrismSurface(c);
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
            b.setCustomContentView(s.render(ls))
                    .setCustomBigContentView(s.render(ls))
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
