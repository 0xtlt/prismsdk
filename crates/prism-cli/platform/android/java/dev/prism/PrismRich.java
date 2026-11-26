package dev.prism;

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.text.Editable;
import android.text.Layout;
import android.text.Spanned;
import android.text.style.ReplacementSpan;
import android.view.MotionEvent;
import android.view.ViewConfiguration;
import android.widget.EditText;

import org.json.JSONArray;
import org.json.JSONObject;

/**
 * Rich inputs (#115): a text area's EditText showing the runs of a
 * `rich_input` as spans (PrismText's, like a text's runs), each chip and
 * inline icon a span drawn in place of its one character (U+FFFC), that the
 * caret, the selection and Backspace treat as one.
 *
 * <p>Edits reach the app as the plain text (`input`, chips included): prism-core gives the typed
 * text the style of the run around it and keeps the other runs. The app's runs come back
 * synchronously, inside the edit: when they hold the text the field shows (the echo of the typing,
 * or the app restyling it), their spans replace the old ones in place, caret and IME composition
 * untouched; other runs replace the text, the caret keeping its distance from the end (#73). What
 * is typed takes the field's own font and color until then. Pastes are plain text without chips;
 * a copy is plain text with each chip's label.
 */
final class PrismRich {
    private PrismRich() {}

    /** The character a chip or icon stands on. */
    static final char OBJECT = '￼';

    /** Op `rich`: the runs to show. */
    static void setRuns(PrismHost.Node n, JSONArray runs) {
        n.richRuns = runs;
        show(n);
    }

    /** Shows `n`'s runs: in place when the field holds their text already. */
    static void show(PrismHost.Node n) {
        if (!n.rich || n.richRuns == null || !(n.view instanceof EditText)) return;
        EditText e = (EditText) n.view;
        JSONArray runs = n.richRuns;
        StringBuilder all = new StringBuilder();
        for (int i = 0; i < runs.length(); i++) {
            JSONObject r = runs.optJSONObject(i);
            if (r != null) all.append(r.optString("t", ""));
        }
        String text = all.toString();
        boolean was = PrismHost.updating;
        PrismHost.updating = true;
        try {
            Editable ed = e.getText();
            if (!ed.toString().equals(text)) {
                int fromEnd = Math.max(0, ed.length() - Math.max(0, e.getSelectionEnd()));
                e.setText(text);
                ed = e.getText();
                e.setSelection(Math.max(0, text.length() - fromEnd));
            }
            for (Object s : n.richSpans) ed.removeSpan(s);
            n.richSpans.clear();
            String family = n.style.optString("ff", "system");
            int weight = n.style.optInt("weight", 400);
            boolean italic = n.style.optBoolean("it");
            int at = 0;
            for (int i = 0; i < runs.length(); i++) {
                JSONObject r = runs.optJSONObject(i);
                if (r == null) continue;
                int start = at, end = at + r.optString("t", "").length();
                at = end;
                if (start == end || end > ed.length()) continue;
                Chip chip = chip(r, e.getResources().getDisplayMetrics().density);
                if (chip != null && end == start + 1 && ed.charAt(start) == OBJECT) {
                    ed.setSpan(chip, start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
                    n.richSpans.add(chip);
                    continue;
                }
                PrismText.runSpans(ed, start, end, r, family, weight, italic, n.richSpans);
            }
            for (Object s : n.richSpans) {
                if (s instanceof PrismText.StyledUnderline) ((PrismText.StyledUnderline) s).view = e;
            }
        } finally {
            PrismHost.updating = was;
        }
    }

    /** A chip run (its label, icon and colors), an inline icon (its glyph), or null. */
    static Chip chip(JSONObject r, float density) {
        Integer color = PrismHost.color(r.optJSONArray("c"));
        Integer bg = PrismHost.color(r.optJSONArray("bg"));
        long link = r.has("link") ? r.optLong("link") : -1;
        if (r.has("chip")) {
            return new Chip(r.optString("cl", ""), r.optString("ci", ""), bg, color, link, true, density);
        }
        if (r.has("icon")) return new Chip(r.optString("glyph", ""), "", null, color, link, false, density);
        return null;
    }

    /**
     * A chip: a rounded box with its icon and label, on the baseline like the text around it; or
     * an inline icon, its glyph alone. One character the caret steps over.
     */
    static final class Chip extends ReplacementSpan {
        final String label, icon;
        final Integer bg, color;
        final long link;
        final boolean box;
        final float density;
        private final RectF rect = new RectF();

        Chip(String label, String icon, Integer bg, Integer color, long link, boolean box, float density) {
            this.label = label;
            this.icon = icon;
            this.bg = bg;
            this.color = color;
            this.link = link;
            this.box = box;
            this.density = density;
        }

        float pad(Paint p) {
            return box ? Math.round(p.getTextSize() * 0.35f) : 0;
        }

        float gap(Paint p) {
            return icon.isEmpty() || label.isEmpty() ? 0 : Math.round(p.getTextSize() * 0.25f);
        }

        float width(Paint p) {
            float iconWidth = icon.isEmpty() ? 0 : p.measureText(icon);
            return pad(p) * 2 + iconWidth + gap(p) + p.measureText(label);
        }

        @Override
        public int getSize(Paint p, CharSequence text, int start, int end, Paint.FontMetricsInt fm) {
            if (fm != null) {
                Paint.FontMetricsInt m = p.getFontMetricsInt();
                fm.ascent = m.ascent;
                fm.descent = m.descent;
                fm.top = m.top;
                fm.bottom = m.bottom;
                fm.leading = m.leading;
            }
            return (int) Math.ceil(width(p));
        }

        @Override
        public void draw(Canvas c, CharSequence text, int start, int end, float x, int top, int y, int bottom, Paint p) {
            int ink = p.getColor();
            Paint.Style style = p.getStyle();
            if (box) {
                Paint.FontMetrics m = p.getFontMetrics();
                rect.set(x, y + m.ascent, x + width(p), y + m.descent);
                p.setStyle(Paint.Style.FILL);
                // The system's gray at 18% by default, like the other platforms.
                p.setColor(bg != null ? bg : 0x2E8E8E93);
                c.drawRoundRect(rect, 4 * density, 4 * density, p);
            }
            p.setColor(color != null ? color : ink);
            float at = x + pad(p);
            if (!icon.isEmpty()) {
                c.drawText(icon, at, y, p);
                at += p.measureText(icon) + gap(p);
            }
            c.drawText(label, at, y, p);
            p.setColor(ink);
            p.setStyle(style);
        }
    }

    // --- Taps, copy and paste ---------------------------------------------------------------

    private static float downX, downY;

    /** A tap on a chip (or icon) with `on_click` reports it; the caret still moves. */
    static void touch(EditText e, MotionEvent ev) {
        PrismHost.Node n = PrismInput.byView.get(e);
        if (n == null || !n.rich) return;
        switch (ev.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                downX = ev.getX();
                downY = ev.getY();
                break;
            case MotionEvent.ACTION_UP: {
                float slop = ViewConfiguration.get(e.getContext()).getScaledTouchSlop();
                if (Math.abs(ev.getX() - downX) > slop || Math.abs(ev.getY() - downY) > slop) break;
                Chip chip = chipAt(e, ev.getX(), ev.getY());
                if (chip != null && chip.link >= 0) {
                    JSONObject m = PrismHost.event("menu", chip.link);
                    // Its box in the field, in points (#327).
                    int s = e.getText().getSpanStart(chip);
                    float[] b = s < 0 ? null : boxOf(e, s, s + 1);
                    if (b != null) {
                        float d = PrismHost.density;
                        try {
                            m.put("x", b[0] / d).put("y", b[1] / d).put("w", (b[2] - b[0]) / d).put("h", (b[3] - b[1]) / d);
                        } catch (org.json.JSONException ignored) { }
                    }
                    PrismHost.send(m);
                }
                break;
            }
        }
    }

    /** The box of characters `start..end` (in characters, as Rust counts
     *  them) in the field's view, in pixels {left, top, right, bottom}: a
     *  chip's own box, else the run on the first line it covers; null
     *  before the field is laid out (#327). */
    static float[] rangeBox(EditText e, int start, int end) {
        Editable ed = e.getText();
        int s = PrismInput.units(ed, start);
        return boxOf(e, s, Math.max(s, PrismInput.units(ed, Math.max(start, end))));
    }

    /** {@link #rangeBox} in UTF-16 units, kept inside the field's visible area. */
    static float[] boxOf(EditText e, int s, int en) {
        Layout layout = e.getLayout();
        if (layout == null) return null;
        Editable ed = e.getText();
        s = Math.max(0, Math.min(s, ed.length()));
        en = Math.max(s, Math.min(en, ed.length()));
        int line = layout.getLineForOffset(s);
        float x0 = layout.getPrimaryHorizontal(s), x1;
        Chip[] chips = en == s + 1 ? ed.getSpans(s, en, Chip.class) : new Chip[0];
        if (chips.length > 0) x1 = x0 + chips[0].width(e.getPaint());
        else if (en > s && layout.getLineForOffset(en) == line) x1 = layout.getPrimaryHorizontal(en);
        else if (en > s) x1 = layout.getLineRight(line);
        else x1 = x0;
        float dx = e.getTotalPaddingLeft() - e.getScrollX(), dy = e.getTotalPaddingTop() - e.getScrollY();
        float l = Math.min(x0, x1) + dx, r = Math.max(x0, x1) + dx;
        float t = layout.getLineTop(line) + dy, b = layout.getLineBottom(line) + dy;
        float w = e.getWidth(), h = e.getHeight();
        l = Math.max(0, Math.min(l, w));
        r = Math.max(l, Math.min(r, w));
        t = Math.max(0, Math.min(t, h));
        b = Math.max(t, Math.min(b, h));
        return new float[]{l, t, r, b};
    }

    /** The chip drawn at view point (x, y), if any. */
    static Chip chipAt(EditText e, float x, float y) {
        Layout layout = e.getLayout();
        if (layout == null) return null;
        float lx = x - e.getTotalPaddingLeft() + e.getScrollX();
        float ly = y - e.getTotalPaddingTop() + e.getScrollY();
        if (ly < 0 || ly > layout.getHeight()) return null;
        int line = layout.getLineForVertical((int) ly);
        int offset = layout.getOffsetForHorizontal(line, lx);
        Editable ed = e.getText();
        for (Chip c : ed.getSpans(Math.max(0, offset - 1), Math.min(ed.length(), offset + 1), Chip.class)) {
            int s = ed.getSpanStart(c);
            if (layout.getLineForOffset(s) != line) continue;
            float left = layout.getPrimaryHorizontal(s);
            float w = c.width(e.getPaint());
            if (lx >= Math.min(left, left + w) && lx <= Math.max(left, left + w)) return c;
        }
        return null;
    }

    /** The text of `from..to` with each chip as its label (icons and other objects dropped). */
    static String labelled(Spanned t, int from, int to) {
        StringBuilder out = new StringBuilder();
        for (int i = from; i < to; i++) {
            char ch = t.charAt(i);
            if (ch != OBJECT) {
                out.append(ch);
                continue;
            }
            Chip[] chips = t.getSpans(i, i + 1, Chip.class);
            if (chips.length > 0 && chips[0].box) out.append(chips[0].label);
        }
        return out.toString();
    }

    /** Copy, cut and paste in a rich input the app does not take (#217): plain text. */
    static boolean menuItem(EditText e, int item) {
        PrismHost.Node n = PrismInput.byView.get(e);
        if (n == null || !n.rich) return false;
        ClipboardManager cb = (ClipboardManager) e.getContext().getSystemService(Context.CLIPBOARD_SERVICE);
        if (cb == null) return false;
        Editable ed = e.getText();
        int a = Math.max(0, Math.min(e.getSelectionStart(), e.getSelectionEnd()));
        int b = Math.max(0, Math.max(e.getSelectionStart(), e.getSelectionEnd()));
        boolean cut = item == android.R.id.cut;
        if (item == android.R.id.copy || cut) {
            if (a == b) return false;
            cb.setPrimaryClip(ClipData.newPlainText(null, labelled(ed, a, b)));
            if (cut) ed.delete(a, b);
            return true;
        }
        if (item == android.R.id.paste || item == android.R.id.pasteAsPlainText) {
            ClipData clip = cb.getPrimaryClip();
            if (clip == null || clip.getItemCount() == 0) return true;
            CharSequence c = clip.getItemAt(0).coerceToText(e.getContext());
            String text = c == null ? "" : c.toString().replace(String.valueOf(OBJECT), "");
            ed.replace(a, b, text);
            e.setSelection(Math.min(ed.length(), a + text.length()));
            return true;
        }
        return false;
    }
}
