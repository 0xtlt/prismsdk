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
