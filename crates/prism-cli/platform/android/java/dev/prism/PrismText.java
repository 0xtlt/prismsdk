package dev.prism;

import android.content.res.AssetManager;
import android.graphics.Typeface;
import android.text.SpannableStringBuilder;
import android.text.Spanned;
import android.text.TextPaint;
import android.text.TextUtils;
import android.text.method.LinkMovementMethod;
import android.text.style.BackgroundColorSpan;
import android.text.style.ClickableSpan;
import android.text.style.ForegroundColorSpan;
import android.text.style.RelativeSizeSpan;
import android.text.style.StrikethroughSpan;
import android.text.style.TypefaceSpan;
import android.text.style.UnderlineSpan;
import android.util.TypedValue;
import android.view.MotionEvent;
import android.view.View;
import android.view.accessibility.AccessibilityNodeInfo;
import android.widget.TextView;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;

/**
 * Typography and rich text of text nodes (prism-core `spans`, the style op's
 * typography keys, see prism_native::remote): one function styles a TextView
 * from (text, style, runs), used both to draw a node and to measure it, so
 * Rust's layout gets exactly what is drawn.
 *
 * Fonts: `mono` and `serif` are the system's monospace and serif families;
 * `rounded` has no Android equivalent (the default font); a name is a font
 * bundled in `assets/fonts/` (family read from the file's `name` table, or
 * its file name), else an installed family of that name.
 */
final class PrismText {
    private PrismText() {}

    // --- Drawing ----------------------------------------------------------------------------

    /** Shows `n.text` (or `n.runs`) in `n.style`. */
    static void render(PrismHost.Node n) {
        if (!(n.view instanceof TextView)) return;
        TextView t = (TextView) n.view;
        JSONObject s = n.style;
        float size = (float) s.optDouble("font", 0);
        if (size <= 0) size = t.getTextSize();
        configure(t, s, size, s.optInt("weight", 400));
        CharSequence text = build(n.text, s, n.runs, size, n.id);
        t.setText(text);
        boolean links = hasLinks(n.runs);
        // Links: tappable, reachable with the keyboard / D-pad, listed by TalkBack.
        t.setMovementMethod(links ? LinkMovementMethod.getInstance() : null);
        if (!links && !n.clickable && n.menu == null) {
            t.setFocusable(false);
            t.setClickable(false);
        }
        t.setLinksClickable(links);
    }

    /**
     * What TalkBack calls a text or a container: the control its role names (a
     * drawn radio, checkbox or switch, #84, read with its state: "Monthly, radio
     * button, checked"), else a button when a text is clickable.
     */
    static void clickable(PrismHost.Node n) {
        if (!n.kind.equals("text") && !n.kind.equals("view")) return;
        String role = n.a11yRole == null ? "" : n.a11yRole;
        String cls;
        switch (role) {
            case "radio": cls = "android.widget.RadioButton"; break;
            case "radiogroup": cls = "android.widget.RadioGroup"; break;
            case "checkbox": cls = "android.widget.CheckBox"; break;
            case "switch": cls = "android.widget.Switch"; break;
            default: cls = n.clickable && n.kind.equals("text") ? "android.widget.Button" : null;
        }
        boolean checkable = role.equals("radio") || role.equals("checkbox") || role.equals("switch");
        n.view.setAccessibilityDelegate(cls == null ? null : new View.AccessibilityDelegate() {
            @SuppressWarnings("deprecation") // setChecked(int) is API 36
            @Override public void onInitializeAccessibilityNodeInfo(View host, AccessibilityNodeInfo info) {
                super.onInitializeAccessibilityNodeInfo(host, info);
                info.setClassName(cls);
                if (checkable) {
                    info.setCheckable(true);
                    info.setChecked(Boolean.TRUE.equals(n.a11yChecked));
                }
            }
        });
    }

    /** A tap on a link of a clickable text (whose touches a gesture detector takes). */
    static boolean tapLink(PrismHost.Node n, MotionEvent e) {
        if (!(n.view instanceof TextView) || !hasLinks(n.runs)) return false;
        TextView t = (TextView) n.view;
        android.text.Layout layout = t.getLayout();
        if (layout == null || !(t.getText() instanceof Spanned)) return false;
        int x = (int) e.getX() - t.getTotalPaddingLeft() + t.getScrollX();
        int y = (int) e.getY() - t.getTotalPaddingTop() + t.getScrollY();
        int line = layout.getLineForVertical(y);
        int offset = layout.getOffsetForHorizontal(line, x);
        ClickableSpan[] spans = ((Spanned) t.getText()).getSpans(offset, offset, ClickableSpan.class);
        if (spans.length == 0) return false;
        spans[0].onClick(t);
        return true;
    }

    static boolean hasLinks(JSONArray runs) {
        if (runs == null) return false;
        for (int i = 0; i < runs.length(); i++) {
            JSONObject r = runs.optJSONObject(i);
            if (r != null && r.has("link")) return true;
        }
        return false;
    }

    /** Font, spacing and line settings of a text view (everything reset when unset). */
    static void configure(TextView t, JSONObject s, float size, int weight) {
        t.setTextSize(TypedValue.COMPLEX_UNIT_PX, size);
        t.setTypeface(font(s.optString("ff", "system"), weight, s.optBoolean("it")));
        float ls = (float) s.optDouble("ls", 0);
        t.setLetterSpacing(size > 0 ? ls / size : 0);
        t.setFontFeatureSettings(s.optBoolean("tn") ? "tnum" : null);
        double lh = s.optDouble("lh", 0);
        if (lh > 0) t.setLineHeight(Math.max(1, Math.round((float) lh)));
        else t.setLineSpacing(0, 1);
        int maxLines = s.optInt("ml", 0);
        boolean noWrap = s.optBoolean("nw");
        t.setHorizontallyScrolling(noWrap);
        t.setMaxLines(maxLines > 0 ? maxLines : Integer.MAX_VALUE);
        t.setEllipsize(maxLines > 0 ? TextUtils.TruncateAt.END : null);
    }

    /** The text with its runs as spans (case transform applied, icons as their glyph). */
    static CharSequence build(String text, JSONObject s, JSONArray runs, float size, long node) {
        String tt = s.optString("tt", "");
        SpannableStringBuilder b = new SpannableStringBuilder();
        if (runs == null || runs.length() == 0) {
            b.append(transform(text == null ? "" : text, tt));
        } else {
            String family = s.optString("ff", "system");
            int weight = s.optInt("weight", 400);
            boolean italic = s.optBoolean("it");
            for (int i = 0; i < runs.length(); i++) {
                JSONObject r = runs.optJSONObject(i);
                if (r == null) continue;
                String t = r.optString("t", "");
                if (!r.has("icon")) t = transform(t, tt);
                int start = b.length();
                b.append(t);
                int end = b.length();
                if (start == end) continue;
                String f = r.optString("ff", family);
                int w = r.optInt("w", weight);
                boolean it = italic || r.optBoolean("i");
                if (!f.equals(family) || w != weight || it != italic) {
                    b.setSpan(new TypefaceSpan(font(f, w, it)), start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
                }
                double z = r.optDouble("z", 0);
                if (z > 0 && z != 1) b.setSpan(new RelativeSizeSpan((float) z), start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
                Integer c = PrismHost.color(r.optJSONArray("c"));
                if (c != null) b.setSpan(new ForegroundColorSpan(c), start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
                Integer bg = PrismHost.color(r.optJSONArray("bg"));
                if (bg != null) b.setSpan(new BackgroundColorSpan(bg), start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
                boolean u = r.optBoolean("u");
                if (r.has("link")) {
                    b.setSpan(new Link(r.optLong("link"), u), start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
                } else if (u) {
                    b.setSpan(new UnderlineSpan(), start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
                }
                if (r.optBoolean("s")) b.setSpan(new StrikethroughSpan(), start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
            }
        }
        int len = b.length();
        if (len > 0 && s.optBoolean("ul")) b.setSpan(new UnderlineSpan(), 0, len, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        if (len > 0 && s.optBoolean("st")) b.setSpan(new StrikethroughSpan(), 0, len, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        return b;
    }

    /** A clickable run: reported like a menu item; keeps the text's color. */
    static final class Link extends ClickableSpan {
        final long item;
        final boolean underline;

        Link(long item, boolean underline) {
            this.item = item;
            this.underline = underline;
        }

        @Override public void onClick(View widget) {
            PrismHost.send(PrismHost.event("menu", item));
        }

        @Override public void updateDrawState(TextPaint ds) {
            ds.setUnderlineText(underline);
        }
    }

    static Locale locale() {
        return PrismHost.lang.isEmpty() ? Locale.getDefault() : Locale.forLanguageTag(PrismHost.lang);
    }

    /** `upper`, `lower`, `capitalize` (each word's first letter), like CSS. */
    static String transform(String s, String tt) {
        switch (tt) {
            case "upper": return s.toUpperCase(locale());
            case "lower": return s.toLowerCase(locale());
            case "capitalize": {
                StringBuilder out = new StringBuilder(s.length());
                boolean start = true;
                for (int i = 0; i < s.length(); ) {
                    int c = s.codePointAt(i);
                    if (start && Character.isLetterOrDigit(c)) out.append(new String(Character.toChars(c)).toUpperCase(locale()));
                    else out.appendCodePoint(c);
                    start = !Character.isLetterOrDigit(c) && c != '\'' && c != '’';
                    i += Character.charCount(c);
                }
                return out.toString();
            }
            default: return s;
        }
    }

    // --- Measuring --------------------------------------------------------------------------

    private static String lastSpec;
    private static JSONObject lastParsed;

    /** A text's natural size in pixels, styled like `render` draws it. */
    static float[] measure(String text, float size, int weight, float maxWidth, String spec) {
        JSONObject s = new JSONObject();
        if (spec != null && !spec.isEmpty()) {
            if (!spec.equals(lastSpec)) {
                try {
                    lastParsed = new JSONObject(spec);
                    lastSpec = spec;
                } catch (JSONException e) {
                    lastParsed = new JSONObject();
                    lastSpec = spec;
                }
            }
            s = lastParsed;
        }
        TextView v = new TextView(PrismHost.activity);
        v.setPadding(0, 0, 0, 0);
        v.setIncludeFontPadding(true);
        configure(v, s, size, weight);
        v.setText(build(text, s, s.optJSONArray("runs"), size, -1));
        int widthSpec = maxWidth >= 0
                ? View.MeasureSpec.makeMeasureSpec((int) Math.ceil(maxWidth), View.MeasureSpec.AT_MOST)
                : View.MeasureSpec.makeMeasureSpec(0, View.MeasureSpec.UNSPECIFIED);
        v.measure(widthSpec, View.MeasureSpec.makeMeasureSpec(0, View.MeasureSpec.UNSPECIFIED));
        return new float[]{v.getMeasuredWidth(), v.getMeasuredHeight()};
    }

    // --- Fonts ------------------------------------------------------------------------------

    /** A bundled font file: its family, weight and style, from its tables. */
    static final class Face {
        String path;
        String family;
        int weight = 400;
        boolean italic;
        boolean variable;
        Typeface typeface;
    }

    private static Map<String, List<Face>> bundled;
    private static final Map<String, Typeface> cache = new HashMap<>();

    /** `family` (`system`, `mono`, `serif`, `rounded` or a name) at `weight` (100–900). */
    static Typeface font(String family, int weight, boolean italic) {
        weight = Math.max(1, Math.min(1000, weight));
        String key = family + "/" + weight + "/" + italic;
        Typeface cached = cache.get(key);
        if (cached != null) return cached;
        Typeface base;
        switch (family) {
            case "":
            case "system":
            case "rounded": base = Typeface.DEFAULT; break;
            case "mono": base = Typeface.MONOSPACE; break;
            case "serif": base = Typeface.SERIF; break;
            default: {
                Typeface named = bundledFont(family, weight, italic);
                if (named != null) {
                    cache.put(key, named);
                    return named;
                }
                base = Typeface.create(family, Typeface.NORMAL);
            }
        }
        Typeface tf = Typeface.create(base, weight, italic);
        cache.put(key, tf);
        return tf;
    }

    /** The closest face of a font bundled in `assets/fonts/`, at exactly `weight` when it is variable. */
    private static Typeface bundledFont(String family, int weight, boolean italic) {
        if (bundled == null) loadBundled();
        List<Face> faces = bundled.get(family.toLowerCase(Locale.ROOT));
        if (faces == null || faces.isEmpty()) return null;
        Face best = null;
        int bestScore = Integer.MAX_VALUE;
        for (Face f : faces) {
            int score = Math.abs(f.weight - weight) + (f.italic == italic ? 0 : 10000);
            if (score < bestScore) { best = f; bestScore = score; }
        }
        AssetManager assets = PrismHost.context.getAssets();
        if (best.variable) {
            Typeface v = new Typeface.Builder(assets, best.path)
                    .setFontVariationSettings("'wght' " + weight + (italic ? ", 'ital' 1" : ""))
                    .setWeight(weight).setItalic(italic).build();
            if (v != null) return v;
        }
        if (best.typeface == null) {
            try {
                best.typeface = Typeface.createFromAsset(assets, best.path);
            } catch (RuntimeException e) {
                return null;
            }
        }
        if (best.weight == weight && best.italic == italic) return best.typeface;
        // Synthesized (bolder or slanted) from the closest face.
        return Typeface.create(best.typeface, weight, italic);
    }

    /** Indexes `assets/fonts/*.{ttf,otf,ttc}` by family name and by file name. */
    private static synchronized void loadBundled() {
        if (bundled != null) return;
        Map<String, List<Face>> out = new HashMap<>();
        try {
            AssetManager assets = PrismHost.context.getAssets();
            String[] files = assets.list("fonts");
            if (files != null) {
                for (String file : files) {
                    String lower = file.toLowerCase(Locale.ROOT);
                    if (!lower.endsWith(".ttf") && !lower.endsWith(".otf") && !lower.endsWith(".ttc")) continue;
                    Face f = new Face();
                    f.path = "fonts/" + file;
                    try (InputStream in = assets.open(f.path)) {
                        parse(readAll(in), f);
                    } catch (Exception ignored) {
                    }
                    String stem = file.substring(0, file.lastIndexOf('.'));
                    List<String> keys = new ArrayList<>();
                    if (f.family != null) keys.add(f.family.toLowerCase(Locale.ROOT));
                    keys.add(stem.toLowerCase(Locale.ROOT));
                    int dash = stem.indexOf('-');
                    if (dash > 0) keys.add(stem.substring(0, dash).toLowerCase(Locale.ROOT));
                    for (String k : keys) {
                        List<Face> list = out.get(k);
                        if (list == null) out.put(k, list = new ArrayList<>());
                        if (!list.contains(f)) list.add(f);
                    }
                }
            }
        } catch (Exception e) {
            android.util.Log.w("prism", "fonts: " + e);
        }
        bundled = out;
    }

    private static byte[] readAll(InputStream in) throws java.io.IOException {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        byte[] buf = new byte[16384];
        for (int r; (r = in.read(buf)) > 0; ) bytes.write(buf, 0, r);
        return bytes.toByteArray();
    }

    private static int u16(byte[] b, int o) { return ((b[o] & 0xff) << 8) | (b[o + 1] & 0xff); }

    private static int u32(byte[] b, int o) { return (u16(b, o) << 16) | u16(b, o + 2); }

    /** Family (`name` IDs 16, then 1), weight and italic (`OS/2`), variable (`fvar`) of an sfnt font. */
    static void parse(byte[] b, Face f) {
        int base = 0;
        if (b.length >= 16 && b[0] == 't' && b[1] == 't' && b[2] == 'c' && b[3] == 'f') base = u32(b, 12);
        int tables = u16(b, base + 4);
        String typographic = null, family = null;
        for (int i = 0; i < tables; i++) {
            int rec = base + 12 + i * 16;
            if (rec + 16 > b.length) break;
            String tag = new String(b, rec, 4, java.nio.charset.StandardCharsets.US_ASCII);
            int off = u32(b, rec + 8);
            switch (tag) {
                case "OS/2":
                    if (off + 64 <= b.length) {
                        f.weight = u16(b, off + 4);
                        f.italic = (u16(b, off + 62) & 1) != 0;
                    }
                    break;
                case "fvar": f.variable = true; break;
                case "name": {
                    int count = u16(b, off + 2), strings = off + u16(b, off + 4);
                    for (int j = 0; j < count; j++) {
                        int r = off + 6 + j * 12;
                        if (r + 12 > b.length) break;
                        int platform = u16(b, r), nameId = u16(b, r + 6), len = u16(b, r + 8), at = strings + u16(b, r + 10);
                        if ((nameId != 1 && nameId != 16) || at + len > b.length) continue;
                        String v;
                        if (platform == 3 || platform == 0) v = new String(b, at, len, java.nio.charset.StandardCharsets.UTF_16BE);
                        else if (platform == 1) v = new String(b, at, len, java.nio.charset.StandardCharsets.ISO_8859_1);
                        else continue;
                        if (nameId == 16 && (typographic == null || platform == 3)) typographic = v;
                        if (nameId == 1 && (family == null || platform == 3)) family = v;
                    }
                    break;
                }
            }
        }
        f.family = typographic != null ? typographic : family;
    }
}
