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

    /** A chip run (its label, icon, colors and options, #329), an inline icon (its glyph), or null. */
    static Chip chip(JSONObject r, float density) {
        Integer color = PrismHost.color(r.optJSONArray("c"));
        Integer bg = PrismHost.color(r.optJSONArray("bg"));
        long link = r.has("link") ? r.optLong("link") : -1;
        if (r.has("chip")) {
            Chip c = new Chip(r.optString("cl", ""), r.optString("ci", ""), bg, color, link, true, density);
            c.runs = r.optJSONArray("nr");
            c.vector = r.optJSONArray("cv");
            c.ratio = (float) r.optDouble("cvr", 1);
            JSONArray ring = r.optJSONArray("ring");
            if (ring != null && ring.length() == 2) {
                c.ringWidth = (float) ring.optDouble(0) * density;
                c.ringColor = PrismHost.color(ring.optJSONArray(1));
            }
            c.radius = (float) r.optDouble("rad", 4) * density;
            c.height = (float) r.optDouble("ht", 0) * density;
            c.maxWidth = r.has("mw") ? (float) r.optDouble("mw") * density : -1;
            c.tooltip = r.optString("tt", null);
            c.state = r.optString("st", null);
            c.testId = r.optString("tid", null);
            return c;
        }
        if (r.has("icon")) {
            Chip c = new Chip(r.optString("glyph", ""), "", null, color, link, false, density);
            c.tooltip = r.optString("tt", null);
            return c;
        }
        return null;
    }

    /**
     * A chip: a rounded box with its icon (a glyph or a vector drawing) and label (styled runs,
     * cut with an ellipsis at its largest width), a ring, on the baseline like the text around it
     * and centered on it when taller (#115, #329); or an inline icon, its glyph alone. One
     * character the caret steps over. Its `state` and `testId` are there for tests (Espresso:
     * the field's `getText().getSpans(…, PrismRich.Chip.class)`).
     */
    static final class Chip extends ReplacementSpan {
        final String label, icon;
        final Integer bg, color;
        final long link;
        final boolean box;
        final float density;
        /** Label runs `[{t, w, i, c}]`, or null: `label` in the run's style. */
        JSONArray runs;
        /** A vector icon: paths in a box 100 units tall, `ratio` times as wide. */
        JSONArray vector;
        float ratio = 1;
        float ringWidth;
        Integer ringColor;
        float radius;
        /** Height and largest width, pixels (0 and -1: none). */
        float height;
        float maxWidth = -1;
        String tooltip, state, testId;
        private final RectF rect = new RectF();
        private final Paint runPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint vectorPaint = new Paint(Paint.ANTI_ALIAS_FLAG);

        Chip(String label, String icon, Integer bg, Integer color, long link, boolean box, float density) {
            this.label = label;
            this.icon = icon;
            this.bg = bg;
            this.color = color;
            this.link = link;
            this.box = box;
            this.density = density;
            this.radius = 4 * density;
        }

        float pad(Paint p) {
            return box ? Math.round(p.getTextSize() * 0.35f) : 0;
        }

        float iconWidth(Paint p) {
            if (vector != null) return p.getTextSize() * ratio;
            return icon.isEmpty() ? 0 : p.measureText(icon);
        }

        float gap(Paint p) {
            return iconWidth(p) == 0 || label.isEmpty() ? 0 : Math.round(p.getTextSize() * 0.25f);
        }

        /** `p` set up for label run `i` (its weight, italic), or the run's own paint. */
        private Paint runPaint(Paint p, int i) {
            if (runs == null) return p;
            JSONObject r = runs.optJSONObject(i);
            runPaint.set(p);
            if (r == null) return runPaint;
            int weight = r.optInt("w", p.getTypeface() != null ? p.getTypeface().getWeight() : 400);
            boolean italic = r.optBoolean("i") || (p.getTypeface() != null && p.getTypeface().isItalic());
            android.graphics.Typeface base = p.getTypeface() != null ? p.getTypeface() : android.graphics.Typeface.DEFAULT;
            runPaint.setTypeface(android.graphics.Typeface.create(base, weight, italic));
            return runPaint;
        }

        private int runCount() {
            return runs == null ? 1 : runs.length();
        }

        private String runText(int i) {
            if (runs == null) return label;
            JSONObject r = runs.optJSONObject(i);
            return r == null ? "" : r.optString("t", "");
        }

        float labelWidth(Paint p) {
            float w = 0;
            for (int i = 0; i < runCount(); i++) w += runPaint(p, i).measureText(runText(i));
            return w;
        }

        float width(Paint p) {
            float natural = pad(p) * 2 + iconWidth(p) + gap(p) + labelWidth(p);
            if (maxWidth < 0 || natural <= maxWidth) return natural;
            return Math.max(maxWidth, pad(p) * 2 + iconWidth(p) + gap(p));
        }

        /** How much taller than its text the box is. */
        float extra(Paint p) {
            Paint.FontMetrics m = p.getFontMetrics();
            return Math.max(0, height - (m.descent - m.ascent));
        }

        @Override
        public int getSize(Paint p, CharSequence text, int start, int end, Paint.FontMetricsInt fm) {
            if (fm != null) {
                Paint.FontMetricsInt m = p.getFontMetricsInt();
                int half = (int) Math.ceil(extra(p) / 2);
                fm.ascent = m.ascent - half;
                fm.descent = m.descent + half;
                fm.top = Math.min(m.top, fm.ascent);
                fm.bottom = Math.max(m.bottom, fm.descent);
                fm.leading = m.leading;
            }
            return (int) Math.ceil(width(p));
        }

        @Override
        public void draw(Canvas c, CharSequence text, int start, int end, float x, int top, int y, int bottom, Paint p) {
            int ink = p.getColor();
            Paint.Style style = p.getStyle();
            float strokeWidth = p.getStrokeWidth();
            int chipInk = color != null ? color : ink;
            Paint.FontMetrics m = p.getFontMetrics();
            float half = extra(p) / 2;
            float w = width(p);
            if (box) {
                rect.set(x, y + m.ascent - half, x + w, y + m.descent + half);
                p.setStyle(Paint.Style.FILL);
                // The system's gray at 18% by default, like the other platforms.
                p.setColor(bg != null ? bg : 0x2E8E8E93);
                c.drawRoundRect(rect, radius, radius, p);
                if (ringWidth > 0 && ringColor != null) {
                    p.setStyle(Paint.Style.STROKE);
                    p.setStrokeWidth(ringWidth);
                    p.setColor(ringColor);
                    float r = Math.max(0, radius - ringWidth / 2);
                    RectF inner = new RectF(rect.left + ringWidth / 2, rect.top + ringWidth / 2,
                            rect.right - ringWidth / 2, rect.bottom - ringWidth / 2);
                    c.drawRoundRect(inner, r, r, p);
                    p.setStrokeWidth(strokeWidth);
                }
            }
            p.setStyle(Paint.Style.FILL);
            p.setColor(chipInk);
            float at = x + pad(p);
            if (vector != null) {
                float em = p.getTextSize();
                float iconTop = y + (m.ascent + m.descent) / 2 - em / 2;
                drawVector(c, at, iconTop, em / 100f, chipInk);
                at += iconWidth(p) + gap(p);
            } else if (!icon.isEmpty()) {
                c.drawText(icon, at, y, p);
                at += p.measureText(icon) + gap(p);
            }
            // The label's runs, cut with an ellipsis where the box ends.
            float room = x + w - pad(p) - at;
            boolean cut = labelWidth(p) > room + 0.5f;
            float ellipsis = p.measureText("…");
            for (int i = 0; i < runCount(); i++) {
                Paint rp = runPaint(p, i);
                JSONObject r = runs == null ? null : runs.optJSONObject(i);
                Integer runColor = r == null ? null : PrismHost.color(r.optJSONArray("c"));
                rp.setColor(runColor != null ? runColor : chipInk);
                String t = runText(i);
                float tw = rp.measureText(t);
                float left = x + w - pad(p) - at;
                if (cut && tw > left - ellipsis) {
                    int n = rp.breakText(t, true, Math.max(0, left - ellipsis), null);
                    String shown = t.substring(0, n).replaceAll("\\s+$", "") + "…";
                    c.drawText(shown, at, y, rp);
                    break;
                }
                c.drawText(t, at, y, rp);
                at += tw;
            }
            p.setColor(ink);
            p.setStyle(style);
        }

        /** The vector icon at (`left`, `top`), `scale` pixels a unit; its text color is `ink`. */
        private void drawVector(Canvas c, float left, float top, float scale, int ink) {
            // Stroke widths are points.
            PrismShape.draw(c, vector, vectorPaint, scale, left, top, density, ink);
        }
    }

    // --- Tool tips (#329) --------------------------------------------------------------------

    /**
     * A mouse over a chip with a tool tip shows it; elsewhere, the field's own. Touch screens
     * have no hover: a long press selects text there, and no chip tool tip shows.
     */
    static void hover(EditText e, MotionEvent ev) {
        PrismHost.Node n = PrismInput.byView.get(e);
        if (n == null || !n.rich) return;
        int a = ev.getActionMasked();
        Chip chip = a == MotionEvent.ACTION_HOVER_EXIT ? null : chipAt(e, ev.getX(), ev.getY());
        CharSequence own = n.view == e ? n.ownTip : null;
        CharSequence tip = chip != null && chip.tooltip != null ? chip.tooltip : own;
        CharSequence now = e.getTooltipText();
        if (tip == null ? now != null : !tip.toString().equals(now == null ? null : now.toString())) {
            e.setTooltipText(tip);
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
