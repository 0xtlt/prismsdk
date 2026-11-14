package dev.prism;

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
 * the app is closed.
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

    /** From the app: `[{kind, link, families: {systemSmall: node, …}}]`. */
    static void reload(Context c, JSONArray snapshots) {
        SharedPreferences.Editor prefs = c.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit();
        for (int i = 0; i < snapshots.length(); i++) {
            JSONObject snap = snapshots.optJSONObject(i);
            if (snap != null) prefs.putString(className(snap.optString("kind")), snap.toString());
        }
        prefs.apply();
        AppWidgetManager mgr = AppWidgetManager.getInstance(c);
        for (int i = 0; i < snapshots.length(); i++) {
            JSONObject snap = snapshots.optJSONObject(i);
            if (snap == null) continue;
            ComponentName provider = new ComponentName(c, className(snap.optString("kind")));
            for (int id : mgr.getAppWidgetIds(provider)) {
                mgr.updateAppWidget(id, views(c, snap, mgr.getAppWidgetOptions(id)));
            }
        }
    }

    @Override
    public void onUpdate(Context c, AppWidgetManager mgr, int[] ids) {
        for (int id : ids) mgr.updateAppWidget(id, views(c, saved(c), mgr.getAppWidgetOptions(id)));
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
        JSONObject families = snap.optJSONObject("families");
        if (families == null || families.length() == 0) return placeholder(c);
        String link = snap.optString("link", "/");
        PrismSurface surface = new PrismSurface(c);
        Map<SizeF, RemoteViews> bySize = new HashMap<>();
        String smallest = null;
        for (Iterator<String> it = families.keys(); it.hasNext(); ) {
            String family = it.next();
            SizeF size = FAMILIES.get(family);
            JSONObject node = families.optJSONObject(family);
            if (size == null || node == null) continue; // Lock-screen families: none on Android.
            bySize.put(size, root(c, surface, node, link));
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

    /** The widget's card: the app's background unless the snapshot has one; taps open `link`. */
    private static RemoteViews root(Context c, PrismSurface surface, JSONObject node, String link) {
        try {
            if (!node.has("bg")) node.put("bg", new JSONArray().put("#FFFFFFFF").put("#1C1C1EFF"));
            if (!node.has("pd")) node.put("pd", new JSONArray().put(12).put(12).put(12).put(12));
            if (!node.has("r")) node.put("r", 20);
        } catch (org.json.JSONException ignored) {
        }
        RemoteViews v = surface.render(node);
        v.setOnClickPendingIntent(surface.node, PrismSurface.open(c, link));
        return v;
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
        return root(c, surface, n, "/");
    }
}
