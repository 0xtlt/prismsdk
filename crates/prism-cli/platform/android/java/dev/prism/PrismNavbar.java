package dev.prism;

import android.text.TextUtils;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.LinearLayout;

import org.json.JSONException;
import org.json.JSONObject;


/**
 * A screen's bar buttons in its stack's header (#30): borderless buttons
 * after Back (leading) and at the trailing edge, showing the icon's glyph
 * or the title. A tap reports {@code change} on the screen with the
 * button's index. Android has one title size: {@code large} / {@code inline}
 * change nothing. The tint (style {@code tint}) colors Back, the bar
 * buttons and the selected tab.
 */
final class PrismNavbar {
    private PrismNavbar() {}

    /** Each screen's buttons' glyphs (sent with the config; "" when the icon has none). */
    private static final java.util.Map<Long, org.json.JSONArray> GLYPHS = new java.util.HashMap<>();

    /** A screen's config: {@code large|inline}, then {@code L|T \t icon \t title \t label} per button. */
    static void config(PrismHost.Node screen, String config, org.json.JSONArray glyphs) {
        screen.config = config;
        if (glyphs != null) GLYPHS.put(screen.id, glyphs); else GLYPHS.remove(screen.id);
        for (PrismHost.Node n : PrismHost.nodes.values()) {
            if (n.kind.equals("stack") && n.children.contains(screen.id)) refresh(n);
        }
    }

    /** A screen is gone. */
    static void forget(long screen) {
        GLYPHS.remove(screen);
    }

    /** The tint of a stack or tab bar, else {@code fallback}. */
    static int tint(PrismHost.Node n, int fallback) {
        Integer t = PrismHost.color(n.style.optJSONArray("tint"));
        return t != null ? t : fallback;
    }

    /** The header's leading and trailing rows (Back moves into the leading one). */
    private static LinearLayout[] rows(PrismHost.Node stack) {
        FrameLayout header = (FrameLayout) stack.header;
        Object tag = header.getTag();
        if (tag instanceof LinearLayout[]) return (LinearLayout[]) tag;
        LinearLayout lead = new LinearLayout(header.getContext());
        LinearLayout trail = new LinearLayout(header.getContext());
        lead.setGravity(Gravity.CENTER_VERTICAL);
        trail.setGravity(Gravity.CENTER_VERTICAL);
        header.removeView(stack.back);
        lead.addView(stack.back, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.MATCH_PARENT));
        header.addView(lead, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.MATCH_PARENT, Gravity.START));
        header.addView(trail, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.MATCH_PARENT, Gravity.END));
        LinearLayout[] rows = {lead, trail};
        header.setTag(rows);
        return rows;
    }

    /** Puts the top screen's buttons in the header (replacing the previous ones). */
    static void refresh(PrismHost.Node stack) {
        if (stack.header == null) return;
        PrismHost.Node top = null;
        for (long c : stack.children) {
            PrismHost.Node s = PrismHost.nodes.get(c);
            if (s != null && s.kind.equals("screen")) top = s;
        }
        String config = top == null ? "" : top.config;
        if (TextUtils.isEmpty(config) && !(stack.header.getTag() instanceof LinearLayout[])) return;
        LinearLayout[] rows = rows(stack);
        // Back stays first in the leading row.
        while (rows[0].getChildCount() > 1) rows[0].removeViewAt(1);
        rows[1].removeAllViews();
        if (top == null) return;
        String[] lines = config.split("\n", -1);
        int fg = PrismHost.dark() ? 0xFFF2F2F7 : 0xFF1C1C1E;
        int color = tint(stack, fg);
        final long screen = top.id;
        org.json.JSONArray glyphs = GLYPHS.get(screen);
        for (int i = 1; i < lines.length; i++) {
            String[] parts = lines[i].split("\t", -1);
            boolean leading = parts[0].equals("L");
            String icon = parts.length > 1 ? parts[1] : "";
            String title = parts.length > 2 ? parts[2] : "";
            String label = parts.length > 3 ? parts[3] : "";
            String glyph = glyphs == null ? "" : glyphs.optString(i - 1, "");
            if (glyph.isEmpty()) glyph = null;
            Button b = new Button(stack.header.getContext(), null, android.R.attr.borderlessButtonStyle);
            b.setAllCaps(false);
            b.setText(glyph != null ? glyph : !title.isEmpty() ? title : "•");
            b.setTextSize(TypedValue.COMPLEX_UNIT_SP, glyph != null ? 24 : 16);
            b.setTextColor(color);
            b.setMinWidth(0);
            b.setMinimumWidth(0);
            b.setContentDescription(!label.isEmpty() ? label : !title.isEmpty() ? title : icon);
            final int index = i - 1;
            b.setOnClickListener(v -> {
                JSONObject ev = PrismHost.event("change", screen);
                try { ev.put("v", index); } catch (JSONException ignored) {}
                PrismHost.send(ev);
            });
            (leading ? rows[0] : rows[1]).addView(b, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.MATCH_PARENT));
        }
    }
}
