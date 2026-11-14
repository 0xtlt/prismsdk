package dev.prism;

import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;

import androidx.annotation.NonNull;
import androidx.car.app.CarAppService;
import androidx.car.app.CarContext;
import androidx.car.app.Screen;
import androidx.car.app.ScreenManager;
import androidx.car.app.Session;
import androidx.car.app.model.Action;
import androidx.car.app.model.CarIcon;
import androidx.car.app.model.GridItem;
import androidx.car.app.model.GridTemplate;
import androidx.car.app.model.ItemList;
import androidx.car.app.model.ListTemplate;
import androidx.car.app.model.Pane;
import androidx.car.app.model.PaneTemplate;
import androidx.car.app.model.Row;
import androidx.car.app.model.SectionedItemList;
import androidx.car.app.model.Template;
import androidx.car.app.validation.HostValidator;
import androidx.core.graphics.drawable.IconCompat;
import androidx.lifecycle.DefaultLifecycleObserver;
import androidx.lifecycle.LifecycleOwner;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;

/**
 * Android Auto and Android Automotive (Car App Library): the app's car
 * templates (prism-core `car`, JSON) as ListTemplate, GridTemplate and
 * PaneTemplate. Rust sends the whole stack; screens are pushed, popped and
 * refreshed to match. Selections and Back go back to Rust.
 */
public final class PrismCarService extends CarAppService {
    @NonNull
    @Override
    public HostValidator createHostValidator() {
        // Debug builds accept any host (the emulator's, the Desktop Head Unit);
        // release builds only the system's template renderers.
        boolean debug = (getApplicationInfo().flags & ApplicationInfo.FLAG_DEBUGGABLE) != 0;
        return debug ? HostValidator.ALLOW_ALL_HOSTS_VALIDATOR
                : new HostValidator.Builder(this).addAllowedHosts(androidx.car.app.R.array.hosts_allowlist_sample).build();
    }

    @NonNull
    @Override
    public Session onCreateSession() {
        return new CarSession();
    }

    /** The stack Rust last sent, and the session showing it. */
    static JSONArray stack = new JSONArray();
    static CarSession session;

    /** From Rust (`PrismHost.apply`, op `car`). */
    static void set(String json) {
        try {
            stack = new JSONObject(json).getJSONArray("stack");
        } catch (Exception e) {
            return;
        }
        if (session != null) session.sync();
    }

    static final class CarSession extends Session {
        final List<CarScreen> screens = new ArrayList<>();
        boolean syncing;

        @NonNull
        @Override
        public Screen onCreateScreen(@NonNull Intent intent) {
            session = this;
            PrismHost.load(getCarContext());
            CarScreen root = new CarScreen(getCarContext(), this, 0);
            screens.add(root);
            getLifecycle().addObserver(new DefaultLifecycleObserver() {
                @Override
                public void onDestroy(@NonNull LifecycleOwner owner) {
                    if (session == CarSession.this) session = null;
                    PrismHost.nativeCarEvent("connected", 0, PrismHost.baseInfo(getCarContext()).toString());
                }
            });
            PrismHost.nativeCarEvent("connected", 1, PrismHost.baseInfo(getCarContext()).toString());
            return root;
        }

        /** Matches the screens to the stack: push, pop, refresh. */
        void sync() {
            ScreenManager manager = getCarContext().getCarService(ScreenManager.class);
            syncing = true;
            try {
                int want = Math.max(1, stack.length());
                while (screens.size() > want) {
                    screens.remove(screens.size() - 1);
                    manager.pop();
                }
                for (CarScreen s : screens) s.invalidate();
                while (screens.size() < want) {
                    CarScreen s = new CarScreen(getCarContext(), this, screens.size());
                    screens.add(s);
                    manager.push(s);
                }
            } finally {
                syncing = false;
            }
        }

        /** The driver went back (the screen was popped by the host). */
        void popped(CarScreen screen) {
            if (syncing || !screens.contains(screen)) return;
            screens.subList(screen.depth, screens.size()).clear();
            PrismHost.nativeCarEvent("back", Math.max(0, screen.depth - 1), "");
        }
    }

    static final class CarScreen extends Screen {
        final CarSession session;
        final int depth;

        CarScreen(CarContext context, CarSession session, int depth) {
            super(context);
            this.session = session;
            this.depth = depth;
            getLifecycle().addObserver(new DefaultLifecycleObserver() {
                @Override
                public void onDestroy(@NonNull LifecycleOwner owner) {
                    session.popped(CarScreen.this);
                }
            });
        }

        @NonNull
        @Override
        public Template onGetTemplate() {
            JSONObject node = stack.optJSONObject(depth);
            Action header = depth == 0 ? Action.APP_ICON : Action.BACK;
            if (node == null) return new ListTemplate.Builder().setLoading(true).setHeaderAction(header).build();
            // Tabs: the first tab (TabTemplate needs Car API level 6).
            if ("tabs".equals(node.optString("t"))) {
                JSONArray tabs = node.optJSONArray("tabs");
                JSONObject first = tabs != null ? tabs.optJSONObject(0) : null;
                if (first != null && first.optJSONObject("template") != null) node = first.optJSONObject("template");
            }
            String title = node.optString("title");
            switch (node.optString("t")) {
                case "grid": {
                    ItemList.Builder items = new ItemList.Builder();
                    JSONArray buttons = node.optJSONArray("buttons");
                    for (int i = 0; buttons != null && i < buttons.length(); i++) {
                        JSONObject b = buttons.optJSONObject(i);
                        GridItem.Builder g = new GridItem.Builder().setTitle(b.optString("title"))
                                .setImage(icon(b.optString("glyph", "•")));
                        long id = b.optLong("id", -1);
                        if (!b.isNull("id")) g.setOnClickListener(() -> select(id));
                        items.addItem(g.build());
                    }
                    return new GridTemplate.Builder().setTitle(title).setHeaderAction(header).setSingleList(items.build()).build();
                }
                case "pane": {
                    Pane.Builder pane = new Pane.Builder();
                    JSONArray rows = node.optJSONArray("rows");
                    for (int i = 0; rows != null && i < rows.length(); i++) {
                        JSONArray r = rows.optJSONArray(i);
                        Row.Builder row = new Row.Builder().setTitle(r.optString(0));
                        if (r.length() > 1 && !r.optString(1).isEmpty()) row.addText(r.optString(1));
                        pane.addRow(row.build());
                    }
                    if (rows == null || rows.length() == 0) pane.addRow(new Row.Builder().setTitle(title).build());
                    JSONArray actions = node.optJSONArray("actions");
                    for (int i = 0; actions != null && i < actions.length() && i < 2; i++) {
                        JSONObject a = actions.optJSONObject(i);
                        Action.Builder action = new Action.Builder().setTitle(a.optString("title"));
                        if ("primary".equals(a.optString("style"))) action.setFlags(Action.FLAG_PRIMARY);
                        long id = a.optLong("id", -1);
                        if (!a.isNull("id")) action.setOnClickListener(() -> select(id));
                        pane.addAction(action.build());
                    }
                    return new PaneTemplate.Builder(pane.build()).setTitle(title).setHeaderAction(header).build();
                }
                default: {
                    ListTemplate.Builder list = new ListTemplate.Builder().setTitle(title).setHeaderAction(header);
                    JSONArray sections = node.optJSONArray("sections");
                    List<SectionedItemList> lists = new ArrayList<>();
                    for (int i = 0; sections != null && i < sections.length(); i++) {
                        JSONObject s = sections.optJSONObject(i);
                        JSONArray items = s.optJSONArray("items");
                        if (items == null || items.length() == 0) continue; // The host refuses empty sections.
                        ItemList.Builder rows = new ItemList.Builder();
                        for (int j = 0; j < items.length(); j++) rows.addItem(row(items.optJSONObject(j)));
                        lists.add(SectionedItemList.create(rows.build(), s.isNull("header") ? "" : s.optString("header")));
                    }
                    if (lists.size() == 1 && sections.optJSONObject(0) != null && sections.optJSONObject(0).isNull("header")) {
                        list.setSingleList(lists.get(0).getItemList());
                    } else if (lists.isEmpty()) {
                        list.setSingleList(new ItemList.Builder().setNoItemsMessage("—").build());
                    } else {
                        for (SectionedItemList l : lists) list.addSectionedList(l);
                    }
                    return list.build();
                }
            }
        }

        private Row row(JSONObject n) {
            Row.Builder row = new Row.Builder().setTitle(n.optString("title"));
            if (!n.isNull("detail")) row.addText(n.optString("detail"));
            if (!n.isNull("glyph")) row.setImage(icon(n.optString("glyph")));
            long id = n.optLong("id", -1);
            if (!n.isNull("id")) {
                row.setOnClickListener(() -> select(id));
                if (n.optBoolean("browsable")) row.setBrowsable(true);
            }
            return row.build();
        }

        private void select(long id) {
            PrismHost.nativeCarEvent("select", id, "");
        }

        /** A glyph drawn as an icon (cars show images, not text, in that slot). */
        private CarIcon icon(String glyph) {
            int size = 96;
            Bitmap bitmap = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888);
            Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
            paint.setColor(Color.WHITE);
            paint.setTextSize(size * 0.75f);
            paint.setTextAlign(Paint.Align.CENTER);
            Paint.FontMetrics m = paint.getFontMetrics();
            new Canvas(bitmap).drawText(glyph, size / 2f, size / 2f - (m.ascent + m.descent) / 2, paint);
            return new CarIcon.Builder(IconCompat.createWithBitmap(bitmap)).build();
        }
    }
}
