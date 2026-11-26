package dev.prism;

import android.app.AlarmManager;
import android.app.PendingIntent;
import android.appwidget.AppWidgetManager;
import android.appwidget.AppWidgetProvider;
import android.content.ComponentName;
import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;
import android.os.Bundle;
import android.util.SizeF;
import android.widget.RemoteViews;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.HashMap;
import java.util.Iterator;
import java.util.Map;

/**
 * A home-screen widget (`cx.register_widget`). The build script generates one
 * subclass per widget kind (`dev.prism.widgets.Widget_<kind>`), declared in the
 * manifest with its size. The app sends snapshots whenever its widgets change
 * (`ReloadWidgets`); they are kept here, so the launcher can redraw them while
 * the app is closed. A snapshot drawn ahead (`Widget::ahead`, #281) shows the
 * latest drawing whose instant has come, and an alarm draws the widget again
 * at the next one, or when a timer with an `after` text ends. A widget with a
 * refresh hook (`Widget::refresh`, #331) has a second alarm, at its reload
 * policy's time (`"reload"`): it runs the hook ({@link Refresh}), in the app's
 * process, without opening the app.
 */
public class PrismWidget extends AppWidgetProvider {
    static final String PREFS = "prism.widgets";

    /** Android's widget sizes for Prism's families (dp, like iOS points). */
    static final Map<String, SizeF> FAMILIES = new HashMap<>();
    static {
        FAMILIES.put("systemSmall", new SizeF(110, 110));
        FAMILIES.put("systemMedium", new SizeF(250, 110));
        FAMILIES.put("systemLarge", new SizeF(250, 250));
        FAMILIES.put("systemExtraLarge", new SizeF(500, 250));
    }

    /** `my-widget` → `my_widget`, the subclass's suffix. */
    static String className(String kind) {
        return "dev.prism.widgets.Widget_" + kind.replaceAll("[^A-Za-z0-9_]", "_");
    }

    /** From the app: `[{kind, link, families: {systemSmall: node, …}, ahead: [{at, families}], reload, refresh}]`. */
    static void reload(Context c, JSONArray snapshots) {
        SharedPreferences.Editor prefs = c.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit();
        long now = System.currentTimeMillis();
        for (int i = 0; i < snapshots.length(); i++) {
            JSONObject snap = snapshots.optJSONObject(i);
            if (snap == null) continue;
            String name = className(snap.optString("kind"));
            prefs.putString(name, snap.toString());
            // A fresh drawing: the next refresh counts from now.
            prefs.putLong(name + REFRESH_AT, refreshAt(snap, now));
        }
        prefs.apply();
        AppWidgetManager mgr = AppWidgetManager.getInstance(c);
        for (int i = 0; i < snapshots.length(); i++) {
            JSONObject snap = snapshots.optJSONObject(i);
            if (snap == null) continue;
            ComponentName provider = new ComponentName(c, className(snap.optString("kind")));
            int[] ids = mgr.getAppWidgetIds(provider);
            for (int id : ids) {
                mgr.updateAppWidget(id, views(c, snap, mgr.getAppWidgetOptions(id)));
            }
            schedule(c, provider, snap, ids);
            scheduleRefresh(c, snap.optString("kind"), ids.length > 0 ? refreshAt(snap, now) : 0);
        }
    }

    @Override
    public void onUpdate(Context c, AppWidgetManager mgr, int[] ids) {
        JSONObject snap = saved(c);
        for (int id : ids) mgr.updateAppWidget(id, views(c, snap, mgr.getAppWidgetOptions(id)));
        ComponentName provider = new ComponentName(c, getClass());
        int[] all = mgr.getAppWidgetIds(provider);
        schedule(c, provider, snap, all);
        if (snap == null || !snap.optBoolean("refresh") || all.length == 0) return;
        // A widget just placed, the device restarted (alarms are lost), or a
        // refresh overdue: refresh now; otherwise keep the refresh alarm.
        String kind = snap.optString("kind");
        long at = c.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getLong(getClass().getName() + REFRESH_AT, 0);
        boolean placed = ids.length < all.length || !Refresh.armed(c, kind);
        if (at <= System.currentTimeMillis() || placed) {
            c.sendBroadcast(new android.content.Intent(c, Refresh.class).putExtra("kind", kind));
        } else {
            scheduleRefresh(c, kind, at);
        }
    }

    /** The preferences key, after the class name, of when the next refresh is due (ms). */
    static final String REFRESH_AT = ".refreshAt";
    /**
     * When the refresh hook of `snap`'s widget runs next, from `now` (ms); 0
     * for never ({@link PrismRefreshPolicy}).
     */
    static long refreshAt(JSONObject snap, long now) {
        if (snap == null) return 0;
        JSONArray ahead = snap.optJSONArray("ahead");
        JSONObject last = ahead == null || ahead.length() == 0 ? null : ahead.optJSONObject(ahead.length() - 1);
        double lastAhead = last == null ? Double.NaN : last.optDouble("at", Double.NaN);
        return PrismRefreshPolicy.refreshAt(snap.optBoolean("refresh"), snap.opt("reload"), lastAhead, now);
    }

    /** Runs widget `kind`'s refresh hook at `at` (ms; inexact, like the drawing alarm), or never (0). */
    static void scheduleRefresh(Context c, String kind, long at) {
        AlarmManager alarms = c.getSystemService(AlarmManager.class);
        if (alarms == null) return;
        PendingIntent pending = Refresh.intent(c, kind, PendingIntent.FLAG_UPDATE_CURRENT);
        if (at == 0) alarms.cancel(pending);
        else alarms.set(AlarmManager.RTC, at, pending);
    }

    /** The families shown at `now` (ms): the latest drawing ahead whose instant has come, else the ones written. */
    static JSONObject current(JSONObject snap, long now) {
        JSONObject families = snap.optJSONObject("families");
        JSONArray ahead = snap.optJSONArray("ahead");
        for (int i = 0; ahead != null && i < ahead.length(); i++) {
            JSONObject e = ahead.optJSONObject(i);
            if (e == null) continue;
            if (Math.round(e.optDouble("at", 0) * 1000) > now) break;
            JSONObject f = e.optJSONObject("families");
            if (f != null) families = f;
        }
        return families;
    }

    /** When the drawing changes after `now` (ms): the next drawing ahead, or the end of a timer with an `after` text; 0 for never. */
    static long nextChange(JSONObject snap, long now) {
        long next = Long.MAX_VALUE;
        JSONArray ahead = snap.optJSONArray("ahead");
        for (int i = 0; ahead != null && i < ahead.length(); i++) {
            JSONObject e = ahead.optJSONObject(i);
            long at = e == null ? 0 : Math.round(e.optDouble("at", 0) * 1000);
            if (at > now) { next = at; break; }
        }
        JSONObject families = current(snap, now);
        for (Iterator<String> it = families == null ? null : families.keys(); it != null && it.hasNext(); ) {
            next = Math.min(next, timerEnd(families.optJSONObject(it.next()), now));
        }
        return next == Long.MAX_VALUE ? 0 : next;
    }

    /** The first end after `now` (ms) of a timer with an `after` text in `n`, or Long.MAX_VALUE. */
    private static long timerEnd(JSONObject n, long now) {
        if (n == null) return Long.MAX_VALUE;
        long next = Long.MAX_VALUE;
        if ("m".equals(n.optString("k")) && !n.optString("ph").isEmpty()) {
            long end = Math.round(n.optDouble("v", 0) * 1000);
            if (end > now) next = end;
        }
        JSONArray kids = n.optJSONArray("c");
        for (int i = 0; kids != null && i < kids.length(); i++) next = Math.min(next, timerEnd(kids.optJSONObject(i), now));
        return next;
    }

    /**
     * Draws the widgets of `provider` again when their drawing changes, with an
     * inexact alarm (no permission needed; Doze may delay it a little): an
     * update broadcast to the provider, which picks the drawing of that time.
     */
    static void schedule(Context c, ComponentName provider, JSONObject snap, int[] ids) {
        AlarmManager alarms = c.getSystemService(AlarmManager.class);
        if (alarms == null) return;
        android.content.Intent i = new android.content.Intent(AppWidgetManager.ACTION_APPWIDGET_UPDATE)
                .setComponent(provider)
                .putExtra(AppWidgetManager.EXTRA_APPWIDGET_IDS, ids);
        PendingIntent pending = PendingIntent.getBroadcast(c, provider.getClassName().hashCode(), i,
                PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        long next = snap == null || ids.length == 0 ? 0 : nextChange(snap, System.currentTimeMillis());
        if (next == 0) alarms.cancel(pending);
        else alarms.set(AlarmManager.RTC, next, pending);
    }

    @Override
    public void onAppWidgetOptionsChanged(Context c, AppWidgetManager mgr, int id, Bundle options) {
        mgr.updateAppWidget(id, views(c, saved(c), options));
    }

    private JSONObject saved(Context c) {
        String json = c.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(getClass().getName(), null);
        try {
            return json == null ? null : new JSONObject(json);
        } catch (org.json.JSONException e) {
            return null;
        }
    }

    /** One layout per family: Android 12+ picks by size; before, by the widget's width. */
    static RemoteViews views(Context c, JSONObject snap, Bundle options) {
        if (snap == null) return placeholder(c);
        JSONObject families = current(snap, System.currentTimeMillis());
        if (families == null || families.length() == 0) return placeholder(c);
        String link = snap.optString("link", "/");
        // One bitmap budget for every size: the RemoteViews carries them all.
        PrismSurface surface = new PrismSurface(c);
        Map<SizeF, RemoteViews> bySize = new HashMap<>();
        java.util.List<SizeF> sizes = new java.util.ArrayList<>();
        for (Iterator<String> it = families.keys(); it.hasNext(); ) {
            SizeF size = FAMILIES.get(it.next());
            if (size != null) sizes.add(size);
        }
        java.util.List<SizeF> actual = actualSizes(options);
        String smallest = null;
        for (Iterator<String> it = families.keys(); it.hasNext(); ) {
            String family = it.next();
            SizeF size = FAMILIES.get(family);
            JSONObject node = families.optJSONObject(family);
            if (size == null || node == null) continue; // Lock-screen families: none on Android.
            SizeF bounds = bounds(size, sizes, actual);
            bySize.put(size, root(c, surface, node, link, bounds));
            if (smallest == null || size.getWidth() < FAMILIES.get(smallest).getWidth()) smallest = family;
        }
        if (bySize.isEmpty()) return placeholder(c);
        if (Build.VERSION.SDK_INT >= 31) return new RemoteViews(bySize);
        float width = options == null ? 0 : options.getInt(AppWidgetManager.OPTION_APPWIDGET_MIN_WIDTH, 0);
        SizeF best = FAMILIES.get(smallest);
        for (SizeF s : bySize.keySet()) {
            if (s.getWidth() <= width && s.getWidth() > best.getWidth()) best = s;
        }
        return bySize.get(best);
    }

    /** The sizes the launcher gives this widget (dp): portrait and landscape cells. */
    private static java.util.List<SizeF> actualSizes(Bundle options) {
        java.util.List<SizeF> out = new java.util.ArrayList<>();
        if (options == null) return out;
        if (Build.VERSION.SDK_INT >= 33) {
            java.util.ArrayList<SizeF> sizes =
                    options.getParcelableArrayList(AppWidgetManager.OPTION_APPWIDGET_SIZES, SizeF.class);
            if (sizes != null) out.addAll(sizes);
        } else if (Build.VERSION.SDK_INT >= 31) {
            @SuppressWarnings("deprecation")
            java.util.ArrayList<SizeF> sizes = options.getParcelableArrayList(AppWidgetManager.OPTION_APPWIDGET_SIZES);
            if (sizes != null) out.addAll(sizes);
        }
        if (out.isEmpty()) {
            int w = options.getInt(AppWidgetManager.OPTION_APPWIDGET_MIN_WIDTH, 0);
            int h = options.getInt(AppWidgetManager.OPTION_APPWIDGET_MAX_HEIGHT, 0);
            if (w > 0 && h > 0) out.add(new SizeF(w, h));
        }
        return out;
    }

    /**
     * What a family's layout will be drawn in: the largest real size that
     * picks it (the system takes the largest layout that fits), else its
     * nominal size. Drawings (charts) are rendered for it.
     */
    private static SizeF bounds(SizeF family, java.util.List<SizeF> families, java.util.List<SizeF> actual) {
        SizeF best = null;
        for (SizeF a : actual) {
            SizeF pick = null;
            for (SizeF f : families) {
                if (f.getWidth() <= a.getWidth() && f.getHeight() <= a.getHeight()
                        && (pick == null || f.getWidth() * f.getHeight() > pick.getWidth() * pick.getHeight())) {
                    pick = f;
                }
            }
            if (family.equals(pick) && (best == null || a.getWidth() > best.getWidth())) best = a;
        }
        return best != null ? best : family;
    }

    /** The widget's card: the app's background unless the snapshot has one; taps open `link`. */
    private static RemoteViews root(Context c, PrismSurface surface, JSONObject node, String link, SizeF bounds) {
        try {
            if (!node.has("bg")) node.put("bg", new JSONArray().put("#FFFFFFFF").put("#1C1C1EFF"));
            if (!node.has("pd")) node.put("pd", new JSONArray().put(12).put(12).put(12).put(12));
            if (!node.has("r")) node.put("r", 20);
        } catch (org.json.JSONException ignored) {
        }
        RemoteViews v = surface.render(node, bounds.getWidth(), bounds.getHeight());
        v.setOnClickPendingIntent(surface.node, PrismSurface.open(c, link));
        return v;
    }

    /**
     * Widget buttons (`widget_action`, #47): a tap broadcasts here, in the
     * app's process, without opening the app. The app's `on_widget_action`
     * job runs (without a screen when the app is closed) and the broadcast
     * stays alive (goAsync) until it is done; the job's reload redraws the
     * widgets. Declared in the manifest when the app has widgets.
     */
    public static final class Action extends android.content.BroadcastReceiver {
        static final String PREFIX = "prism.widget-action:";
        static final Map<String, PendingResult> running = new HashMap<>();
        static long next;

        /** The broadcast a button sends: `{k: kind, a: action, p: payload}`. */
        static android.app.PendingIntent intent(Context c, JSONObject action) {
            String kind = action.optString("k"), name = action.optString("a"), payload = action.optString("p");
            android.content.Intent i = new android.content.Intent(c, Action.class)
                    .putExtra("kind", kind).putExtra("action", name).putExtra("payload", payload);
            // One pending intent per action and payload (extras do not tell them apart).
            i.setData(android.net.Uri.parse("prism-action:" + android.net.Uri.encode(kind) + "/"
                    + android.net.Uri.encode(name) + "/" + android.net.Uri.encode(payload)));
            return android.app.PendingIntent.getBroadcast(c, (kind + "\n" + name + "\n" + payload).hashCode(), i,
                    android.app.PendingIntent.FLAG_UPDATE_CURRENT | android.app.PendingIntent.FLAG_IMMUTABLE);
        }

        @Override
        public void onReceive(Context c, android.content.Intent i) {
            String task = PREFIX + (++next);
            running.put(task, goAsync());
            PrismHost.load(c);
            PrismHost.nativeWidgetAction(task, i.getStringExtra("kind"), i.getStringExtra("action"),
                    i.getStringExtra("payload"), PrismHost.baseInfo(c).toString());
        }

        /** From Rust (through PrismHost.jobFinished): the action's job is done. */
        static boolean finished(String task) {
            PendingResult r = running.remove(task);
            if (r == null) return false;
            r.finish();
            return true;
        }
    }

    /**
     * A widget's refresh (`Widget::refresh`, #331): its alarm broadcasts here,
     * in the app's process, without opening the app. The hook runs (without a
     * screen when the app is closed) and the broadcast stays alive (goAsync)
     * until it ends; one that succeeds redraws the widget ({@link #reload}, which
     * sets the next refresh), one that fails or times out leaves its drawing.
     * Declared in the manifest when the app has widgets.
     */
    public static final class Refresh extends android.content.BroadcastReceiver {
        static final String PREFIX = "prism.widget-refresh:";
        static final Map<String, PendingResult> running = new HashMap<>();
        static long next;

        /** The broadcast refreshing widget `kind` (one per kind). */
        static PendingIntent intent(Context c, String kind, int flags) {
            android.content.Intent i = new android.content.Intent(c, Refresh.class)
                    .putExtra("kind", kind)
                    .setData(android.net.Uri.parse("prism-refresh:" + android.net.Uri.encode(kind)));
            return PendingIntent.getBroadcast(c, ("refresh\n" + kind).hashCode(), i, flags | PendingIntent.FLAG_IMMUTABLE);
        }

        /** Whether widget `kind`'s refresh alarm is set. */
        static boolean armed(Context c, String kind) {
            return intent(c, kind, PendingIntent.FLAG_NO_CREATE) != null;
        }

        @Override
        public void onReceive(Context c, android.content.Intent i) {
            String kind = i.getStringExtra("kind");
            if (kind == null) return;
            // The next try, should this one fail (a success draws the widget
            // again, which sets it anew).
            String name = className(kind);
            SharedPreferences prefs = c.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
            JSONObject snap = null;
            try {
                String json = prefs.getString(name, null);
                snap = json == null ? null : new JSONObject(json);
            } catch (org.json.JSONException ignored) {
            }
            long at = refreshAt(snap, System.currentTimeMillis());
            prefs.edit().putLong(name + REFRESH_AT, at).apply();
            scheduleRefresh(c, kind, at);
            String task = PREFIX + (++next);
            running.put(task, goAsync());
            PrismHost.load(c);
            PrismHost.nativeWidgetRefresh(task, kind, PrismHost.baseInfo(c).toString());
        }

        /** From Rust (through PrismHost.jobFinished): the refresh ended. */
        static boolean finished(String task) {
            PendingResult r = running.remove(task);
            if (r == null) return false;
            r.finish();
            return true;
        }
    }

    /** Before the app first ran: its name; a tap opens it. */
    private static RemoteViews placeholder(Context c) {
        JSONObject n = new JSONObject();
        try {
            n.put("k", "t");
            n.put("s", c.getApplicationInfo().loadLabel(c.getPackageManager()).toString());
            n.put("ta", "c");
        } catch (org.json.JSONException ignored) {
        }
        PrismSurface surface = new PrismSurface(c);
        return root(c, surface, n, "/", FAMILIES.get("systemSmall"));
    }
}
