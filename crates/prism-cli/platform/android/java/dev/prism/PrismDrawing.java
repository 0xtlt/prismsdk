package dev.prism;

import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.BlurMaskFilter;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.ColorFilter;
import android.graphics.ColorMatrixColorFilter;
import android.graphics.DashPathEffect;
import android.graphics.LinearGradient;
import android.graphics.Matrix;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.PorterDuff;
import android.graphics.PorterDuffColorFilter;
import android.graphics.PorterDuffXfermode;
import android.graphics.RadialGradient;
import android.graphics.RectF;
import android.graphics.Shader;
import android.os.Build;
import android.util.Base64;
import android.util.LruCache;

import org.json.JSONArray;
import org.json.JSONObject;

/**
 * What a shape draws (prism-core `shape::Item`), read once from a `draw` op into paints, paths
 * and bitmaps: paths in a flat color or a gradient, lines of text, images, and groups shown as
 * one picture (blurred, over shadows, through a clip or a mask, at an opacity), as an SVG
 * document has them (#342). Everything is in the view's pixels; colors follow light/dark mode
 * when drawn.
 */
final class PrismDrawing {
    static final PrismDrawing EMPTY = new PrismDrawing(new Item[0]);

    private final Item[] items;
    private final RectF frame = new RectF();

    private PrismDrawing(Item[] items) {
        this.items = items;
    }

    /** `paths` of a `draw` op: `[{d, fill, stroke, w, round, dash, eo, fg, sg}]`. */
    static PrismDrawing ofPaths(JSONArray paths) {
        return paths == null ? EMPTY : new PrismDrawing(items(paths));
    }

    /** `items` of a `draw` op: `[{t: "p" path, "x" text, "i" image, "g" group, …}]`. */
    static PrismDrawing ofItems(JSONArray items) {
        return items == null ? EMPTY : new PrismDrawing(items(items));
    }

    /** Draws it in a view `width` × `height` pixels. */
    void draw(Canvas c, float width, float height) {
        frame.set(0, 0, width, height);
        boolean dark = PrismHost.dark();
        for (Item item : items) item.draw(c, dark, frame);
    }

    // --- Reading ---------------------------------------------------------------------------

    private static Item[] items(JSONArray all) {
        java.util.ArrayList<Item> items = new java.util.ArrayList<>();
        for (int i = 0; i < all.length(); i++) {
            JSONObject o = all.optJSONObject(i);
            if (o == null) continue;
            switch (o.optString("t", "p")) {
                case "p": items.add(new PathItem(o)); break;
                case "x": items.add(new TextItem(o)); break;
                case "i": items.add(new ImageItem(o)); break;
                case "g": items.add(new Group(o)); break;
                default: break;
            }
        }
        return items.toArray(new Item[0]);
    }

    /** `[r,g,b,a, dark r,g,b,a]` (0…1) → the light and the dark color, or null. */
    private static int[] colors(JSONArray c) {
        if (c == null || c.length() < 4) return null;
        int d = c.length() >= 8 ? 4 : 0;
        return new int[]{
                Color.argb((float) c.optDouble(3), (float) c.optDouble(0), (float) c.optDouble(1), (float) c.optDouble(2)),
                Color.argb((float) c.optDouble(d + 3), (float) c.optDouble(d), (float) c.optDouble(d + 1), (float) c.optDouble(d + 2))};
    }

    /** `[a, b, c, d, e, f]` (SVG `matrix()` order: x' = a·x + c·y + e, y' = b·x + d·y + f). */
    static Matrix matrix(JSONArray m) {
        Matrix matrix = new Matrix();
        if (m != null && m.length() >= 6) {
            matrix.setValues(new float[]{
                    (float) m.optDouble(0), (float) m.optDouble(2), (float) m.optDouble(4),
                    (float) m.optDouble(1), (float) m.optDouble(3), (float) m.optDouble(5),
                    0, 0, 1});
        }
        return matrix;
    }

    /** A blur's standard deviation as the radius Android's blurs take. */
    static float blurRadius(float sigma) {
        return (sigma - 0.5f) / 0.57735f;
    }

    /** How far a blur of standard deviation `sigma` reaches, in whole pixels. */
    private static float reach(float sigma) {
        return (float) Math.ceil(3 * sigma);
    }

    private static void roundOut(RectF r) {
        r.set((float) Math.floor(r.left), (float) Math.floor(r.top), (float) Math.ceil(r.right), (float) Math.ceil(r.bottom));
    }

    private static RectF union(Item[] items) {
        RectF all = new RectF();
        for (Item item : items) all.union(item.bounds);
        return all;
    }

    // --- Items -----------------------------------------------------------------------------

    /** One thing drawn. */
    private abstract static class Item {
        /** Where it draws at most (what a layer has to hold); empty when it draws nothing. */
        final RectF bounds = new RectF();

        /** Draws it in a view that covers `frame`. */
        abstract void draw(Canvas c, boolean dark, RectF frame);
    }

    /**
     * A linear or radial gradient (`{k, c, m, sp, o, cl}`): `c` in its own coordinates, `m`
     * from them to pixels (as the shader's matrix, so a skewed gradient stays skewed).
     */
    private static final class Gradient {
        final boolean radial;
        final float[] c;
        final Matrix m;
        final Shader.TileMode tile;
        final float[] offsets;
        /** The stops' colors, light then dark. */
        final int[][] stops = new int[2][];
        private final Shader[] shaders = new Shader[2];

        /** The gradient of `g`, or null when it has no stop (the flat color is drawn). */
        static Gradient of(JSONObject g) {
            if (g == null) return null;
            JSONArray c = g.optJSONArray("c"), o = g.optJSONArray("o"), cl = g.optJSONArray("cl");
            boolean radial = g.optString("k").equals("r");
            if (c == null || o == null || cl == null || cl.length() == 0 || c.length() < (radial ? 5 : 4)) return null;
            return new Gradient(g, radial, c, o, cl);
        }

        private Gradient(JSONObject g, boolean radial, JSONArray c, JSONArray o, JSONArray cl) {
            this.radial = radial;
            this.c = new float[c.length()];
            for (int i = 0; i < c.length(); i++) this.c[i] = (float) c.optDouble(i);
            m = matrix(g.optJSONArray("m"));
            int spread = g.optInt("sp");
            tile = spread == 1 ? Shader.TileMode.MIRROR : spread == 2 ? Shader.TileMode.REPEAT : Shader.TileMode.CLAMP;
            // A shader takes two colors at least: a single stop is both ends.
            int n = Math.max(2, cl.length());
            offsets = new float[n];
            stops[0] = new int[n];
            stops[1] = new int[n];
            for (int i = 0; i < n; i++) {
                int k = Math.min(i, cl.length() - 1);
                int[] color = colors(cl.optJSONArray(k));
                offsets[i] = Math.max(0, Math.min(1, (float) o.optDouble(k, 0)));
                if (i > 0) offsets[i] = Math.max(offsets[i], offsets[i - 1]);
                stops[0][i] = color == null ? 0 : color[0];
                stops[1][i] = color == null ? 0 : color[1];
            }
        }

        /** A line without length, a circle without radius: the last color everywhere (as in SVG). */
        boolean flat() {
            return radial ? !(c[2] > 0) : c[0] == c[2] && c[1] == c[3];
        }

        int last(boolean dark) {
            int[] colors = stops[dark ? 1 : 0];
            return colors[colors.length - 1];
        }

        Shader shader(boolean dark) {
            int mode = dark ? 1 : 0;
            if (shaders[mode] == null) {
                shaders[mode] = radial ? radial(stops[mode]) : new LinearGradient(c[0], c[1], c[2], c[3], stops[mode], offsets, tile);
                shaders[mode].setLocalMatrix(m);
            }
            return shaders[mode];
        }

        /** Offset 0 at the focal point (`c[3]`, `c[4]`), 1 on the circle `c[2]` around (`c[0]`, `c[1]`). */
        private Shader radial(int[] colors) {
            float fx = c[3] - c[0], fy = c[4] - c[1];
            float off = (float) Math.hypot(fx, fy);
            // Android 12+ has the gradient between two circles; before, the focal point is
            // the center.
            if (off < 1e-4f * c[2] || Build.VERSION.SDK_INT < 31) return new RadialGradient(c[0], c[1], c[2], colors, offsets, tile);
            // A focal point outside the circle is brought back inside it (as in SVG).
            float k = Math.min(1, 0.99f * c[2] / off);
            long[] packed = new long[colors.length];
            for (int i = 0; i < colors.length; i++) packed[i] = Color.pack(colors[i]);
            return new RadialGradient(c[0] + fx * k, c[1] + fy * k, 0, c[0], c[1], c[2], packed, offsets, tile);
        }
    }

    /** A path: filled, then stroked, each in a flat color or a gradient. */
    private static final class PathItem extends Item {
        final Path path;
        final int[] fill, stroke;
        final Gradient fillGradient, strokeGradient;
        final Paint fillPaint = new Paint(Paint.ANTI_ALIAS_FLAG), strokePaint = new Paint(Paint.ANTI_ALIAS_FLAG);

        PathItem(JSONObject p) {
            path = PrismShape.path(p.optJSONArray("d"), 1, 0, 0);
            if (p.optBoolean("eo")) path.setFillType(Path.FillType.EVEN_ODD);
            fill = colors(p.optJSONArray("fill"));
            stroke = colors(p.optJSONArray("stroke"));
            fillGradient = Gradient.of(p.optJSONObject("fg"));
            strokeGradient = Gradient.of(p.optJSONObject("sg"));
            fillPaint.setStyle(Paint.Style.FILL);
            float width = (float) p.optDouble("w", 1);
            strokePaint.setStyle(Paint.Style.STROKE);
            strokePaint.setStrokeWidth(width);
            if (p.optBoolean("round")) {
                strokePaint.setStrokeCap(Paint.Cap.ROUND);
                strokePaint.setStrokeJoin(Paint.Join.ROUND);
            }
            JSONArray dash = p.optJSONArray("dash");
            if (dash != null && dash.length() == 2) {
                strokePaint.setPathEffect(new DashPathEffect(new float[]{(float) dash.optDouble(0), (float) dash.optDouble(1)}, 0));
            }
            if (!path.isEmpty() && (fill != null || fillGradient != null || stroke != null || strokeGradient != null)) {
                path.computeBounds(bounds, true);
                // A stroke's corners reach up to twice its width from the outline (its miter limit).
                float out = stroke != null || strokeGradient != null ? Math.max(1, width * 2) : 1;
                bounds.inset(-out, -out);
                roundOut(bounds);
            }
        }

        @Override
        void draw(Canvas c, boolean dark, RectF frame) {
            if (ink(fillPaint, fill, fillGradient, dark)) c.drawPath(path, fillPaint);
            if (ink(strokePaint, stroke, strokeGradient, dark)) c.drawPath(path, strokePaint);
        }

        /** Gives `paint` the gradient, or the flat color without one. False: nothing to draw. */
        private static boolean ink(Paint paint, int[] flat, Gradient gradient, boolean dark) {
            if (gradient != null && !gradient.flat()) {
                paint.setColor(0xFF000000);
                paint.setShader(gradient.shader(dark));
                return true;
            }
            paint.setShader(null);
            if (gradient == null && flat == null) return false;
            paint.setColor(gradient != null ? gradient.last(dark) : flat[dark ? 1 : 0]);
            return true;
        }
    }

    /**
     * One line of text: runs laid out left to right from its anchor, in the coordinates `m`
     * maps to pixels (the text's rotation and scale).
     */
    private static final class TextItem extends Item {
        final Matrix m;
        final Run[] runs;

        /** A stretch in one font and color, its pen position worked out once. */
        static final class Run {
            String text;
            Paint paint;
            int[] color;
            float x, y;
        }

        TextItem(JSONObject t) {
            m = matrix(t.optJSONArray("m"));
            JSONArray all = t.optJSONArray("r");
            runs = new Run[all == null ? 0 : all.length()];
            float pen = 0, reach = 0, top = 0, bottom = 0;
            for (int i = 0; i < runs.length; i++) {
                JSONObject r = all.optJSONObject(i);
                if (r == null) r = new JSONObject();
                Run run = runs[i] = new Run();
                run.text = r.optString("s");
                run.color = colors(r.optJSONArray("c"));
                float size = (float) r.optDouble("z", 16), spacing = (float) r.optDouble("k", 0);
                // Laid out at any scale: no hinting, no advances rounded to whole pixels.
                Paint p = run.paint = new Paint(Paint.ANTI_ALIAS_FLAG | Paint.SUBPIXEL_TEXT_FLAG | Paint.LINEAR_TEXT_FLAG);
                p.setHinting(Paint.HINTING_OFF);
                p.setTypeface(PrismText.font(r.optString("ff", "system"), r.optInt("w", 400), r.optBoolean("it")));
                p.setTextSize(size);
                // Android puts half of the spacing on each side of a character: moved back
                // by that half, the spacing comes after each one.
                if (spacing != 0 && size > 0) p.setLetterSpacing(spacing / size);
                pen += (float) r.optDouble("dx", 0);
                run.x = pen - spacing / 2;
                run.y = (float) r.optDouble("dy", 0);
                pen += p.measureText(run.text);
                Paint.FontMetrics fm = p.getFontMetrics();
                // Room for the glyphs of fallback fonts and for slanted ones.
                reach = Math.max(reach, size);
                top = Math.min(top, run.y + fm.top);
                bottom = Math.max(bottom, run.y + fm.bottom);
            }
            float x = (float) t.optDouble("x", 0) - start(t.optInt("a"), pen);
            float y = (float) t.optDouble("y", 0) + (runs.length == 0 ? 0 : baseline(t.optInt("b"), runs[0].paint));
            for (Run run : runs) {
                run.x += x;
                run.y += y;
            }
            if (runs.length > 0) {
                bounds.set(x - reach, y + top - reach, x + pen + reach, y + bottom + reach);
                m.mapRect(bounds);
                roundOut(bounds);
            }
        }

        /** How far left of its anchor a line `width` wide starts: 0 start, 1 middle, 2 end. */
        static float start(int anchor, float width) {
            return anchor == 1 ? width / 2 : anchor == 2 ? width : 0;
        }

        /**
         * How far below the text's `y` the baseline is, in the font of `p`: 0 alphabetic,
         * 1 middle (half the x-height), 2 central (halfway between ascent and descent),
         * 3 hanging (0.8 × the ascent).
         */
        static float baseline(int kind, Paint p) {
            Paint.FontMetrics fm = p.getFontMetrics();
            switch (kind) {
                case 1: {
                    android.graphics.Rect x = new android.graphics.Rect();
                    p.getTextBounds("x", 0, 1, x);
                    return -x.top / 2f;
                }
                case 2: return (-fm.ascent - fm.descent) / 2;
                case 3: return 0.8f * -fm.ascent;
                default: return 0;
            }
        }

        @Override
        void draw(Canvas c, boolean dark, RectF frame) {
            c.save();
            c.concat(m);
            for (Run run : runs) {
                int color = run.color == null ? 0 : run.color[dark ? 1 : 0];
                // A transparent run takes its room and draws nothing.
                if (Color.alpha(color) == 0) continue;
                run.paint.setColor(color);
                c.drawText(run.text, run.x, run.y, run.paint);
            }
            c.restore();
        }
    }

    /** A bitmap in its rectangle (`r`, in the coordinates `m` maps to pixels). */
    private static final class ImageItem extends Item {
        /** Decoded `data:` URLs by `src`: a document is read again each time its view resizes. */
        private static final LruCache<String, Bitmap> decoded = new LruCache<String, Bitmap>(16 << 20) {
            @Override
            protected int sizeOf(String src, Bitmap b) {
                return b.getByteCount();
            }
        };

        final Matrix m;
        final RectF rect = new RectF();
        final RectF placed;
        final boolean slice;
        final Bitmap bitmap;
        final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG | Paint.FILTER_BITMAP_FLAG);

        ImageItem(JSONObject i) {
            m = matrix(i.optJSONArray("m"));
            JSONArray r = i.optJSONArray("r"), a = i.optJSONArray("a");
            if (r != null && r.length() >= 4) {
                rect.set((float) r.optDouble(0), (float) r.optDouble(1),
                        (float) (r.optDouble(0) + r.optDouble(2)), (float) (r.optDouble(1) + r.optDouble(3)));
            }
            int fit = i.optInt("f");
            slice = fit == 1;
            bitmap = rect.isEmpty() ? null : decode(i.optString("src"));
            placed = bitmap == null ? rect : place(fit, a == null ? 0.5f : (float) a.optDouble(0, 0.5), a == null ? 0.5f : (float) a.optDouble(1, 0.5),
                    rect, bitmap.getWidth(), bitmap.getHeight());
            paint.setAlpha(Math.round(255 * Math.max(0, Math.min(1, (float) i.optDouble("o", 1)))));
            if (bitmap != null && paint.getAlpha() > 0) {
                m.mapRect(bounds, rect);
                bounds.inset(-1, -1);
                roundOut(bounds);
            }
        }

        /**
         * Where a bitmap `w` × `h` goes in `rect`: 2 stretched over it; 0 (meet) as large as
         * fits inside, 1 (slice) as small as covers it, both keeping its proportions, the room
         * left (or the overflow) split by (`ax`, `ay`): 0 the left / top, 1 the right / bottom.
         */
        static RectF place(int fit, float ax, float ay, RectF rect, float w, float h) {
            if (fit == 2 || w <= 0 || h <= 0) return rect;
            float sx = rect.width() / w, sy = rect.height() / h;
            float scale = fit == 1 ? Math.max(sx, sy) : Math.min(sx, sy);
            float left = rect.left + (rect.width() - w * scale) * ax, top = rect.top + (rect.height() - h * scale) * ay;
            return new RectF(left, top, left + w * scale, top + h * scale);
        }

        /** The bitmap of a base64 `data:` URL, or null when it is not one Android decodes. */
        private static Bitmap decode(String src) {
            Bitmap known = decoded.get(src);
            if (known != null) return known;
            int comma = src.indexOf(',');
            if (!src.startsWith("data:") || comma < 0 || !src.substring(0, comma).endsWith(";base64")) return null;
            try {
                byte[] bytes = Base64.decode(src.substring(comma + 1), Base64.DEFAULT);
                Bitmap b = BitmapFactory.decodeByteArray(bytes, 0, bytes.length);
                if (b != null) decoded.put(src, b);
                return b;
            } catch (IllegalArgumentException | OutOfMemoryError e) {
                return null;
            }
        }

        @Override
        void draw(Canvas c, boolean dark, RectF frame) {
            if (bitmap == null || paint.getAlpha() == 0) return;
            c.save();
            c.concat(m);
            if (slice) c.clipRect(rect);
            c.drawBitmap(bitmap, null, placed, paint);
            c.restore();
        }
    }

    /**
     * Items drawn together, then shown as one picture: blurred, over its shadows, cut to its
     * clip, seen through its mask, at its opacity (in that order, SVG's).
     */
    private static final class Group extends Item {
        /** A mask's layer onto the group's: the group stays where the mask is opaque. */
        private static final Paint ALPHA_MASK = new Paint();
        /** The same, for a mask drawn over black: where it is bright (its luminance as the opacity). */
        private static final Paint LUMINANCE_MASK = new Paint();

        static {
            ALPHA_MASK.setXfermode(new PorterDuffXfermode(PorterDuff.Mode.DST_IN));
            LUMINANCE_MASK.setXfermode(new PorterDuffXfermode(PorterDuff.Mode.DST_IN));
            LUMINANCE_MASK.setColorFilter(new ColorMatrixColorFilter(new float[]{
                    0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0,
                    0.2126f, 0.7152f, 0.0722f, 0, 0}));
        }

        final int alpha;
        final Path clip;
        final Item[] mask;
        final boolean luminance;
        /** Each shadow: how far right and down, its blur's standard deviation. */
        final float[][] shadows;
        final int[][] shadowColors;
        final float blur;
        final Item[] items;
        /** Where the items draw, before the blur and the shadows. */
        final RectF content;

        /** The layers of the blur and the shadows (`RenderNode`s, Android 12+). */
        private Object[] nodes;
        /** The blur and the shadows as one bitmap, where there are no such layers. */
        private Bitmap baked;
        private boolean bakedDark;
        private final RectF bakedFrame = new RectF();
        private float bakedLeft, bakedTop;

        Group(JSONObject g) {
            alpha = Math.round(255 * Math.max(0, Math.min(1, (float) g.optDouble("o", 1))));
            JSONObject clipped = g.optJSONObject("clip"), masked = g.optJSONObject("mask");
            clip = clipped == null ? null : PrismShape.path(clipped.optJSONArray("d"), 1, 0, 0);
            if (clip != null && clipped.optBoolean("eo")) clip.setFillType(Path.FillType.EVEN_ODD);
            JSONArray maskItems = masked == null ? null : masked.optJSONArray("i");
            mask = masked == null ? null : maskItems == null ? new Item[0] : PrismDrawing.items(maskItems);
            luminance = masked != null && masked.optBoolean("l");
            JSONArray sh = g.optJSONArray("sh");
            int count = sh == null ? 0 : sh.length();
            shadows = new float[count][];
            shadowColors = new int[count][];
            for (int i = 0; i < count; i++) {
                JSONArray s = sh.optJSONArray(i);
                if (s == null) s = new JSONArray();
                shadows[i] = new float[]{(float) s.optDouble(0, 0), (float) s.optDouble(1, 0), Math.max(0, (float) s.optDouble(2, 0))};
                int[] color = colors(s.optJSONArray(3));
                shadowColors[i] = color == null ? new int[]{0, 0} : color;
            }
            blur = Math.max(0, (float) g.optDouble("b", 0));
            JSONArray inner = g.optJSONArray("i");
            items = inner == null ? new Item[0] : PrismDrawing.items(inner);
            content = union(items);

            if (!content.isEmpty()) {
                bounds.set(content);
                bounds.inset(-reach(blur), -reach(blur));
                for (float[] s : shadows) {
                    RectF shadow = new RectF(content);
                    float out = reach(shadowBlur(s));
                    shadow.inset(-out, -out);
                    shadow.offset(s[0], s[1]);
                    bounds.union(shadow);
                }
                if (clip != null) {
                    RectF inside = new RectF();
                    clip.computeBounds(inside, true);
                    inside.inset(-1, -1);
                    if (!bounds.intersect(inside)) bounds.setEmpty();
                }
                if (mask != null && !bounds.intersect(union(mask))) bounds.setEmpty();
                roundOut(bounds);
            }
        }

        /** A shadow is cast by the blurred picture: one blur after the other. */
        private float shadowBlur(float[] shadow) {
            return (float) Math.hypot(shadow[2], blur);
        }

        @Override
        void draw(Canvas c, boolean dark, RectF frame) {
            if (alpha == 0 || bounds.isEmpty()) return;
            // The mask and the opacity apply to the whole picture: a layer of its own.
            int saved = mask != null || alpha < 255 ? c.saveLayerAlpha(bounds, alpha) : c.save();
            if (mask != null) c.save();
            if (clip != null) c.clipPath(clip);
            if (blur > 0 || shadows.length > 0) effects(c, dark, frame);
            else for (Item item : items) item.draw(c, dark, frame);
            if (mask != null) {
                c.restore();
                c.saveLayer(bounds, luminance ? LUMINANCE_MASK : ALPHA_MASK);
                // Over black, a color's brightness times its opacity is the brightness left.
                if (luminance) c.drawColor(0xFF000000);
                for (Item item : mask) item.draw(c, dark, frame);
            }
            c.restoreToCount(saved);
        }

        /** The shadows (the first at the bottom), then the items, blurred if the group is. */
        private void effects(Canvas c, boolean dark, RectF frame) {
            if (Build.VERSION.SDK_INT >= 31 && c.isHardwareAccelerated()) {
                if (nodes == null) nodes = new Object[shadows.length + 1];
                for (int i = 0; i < shadows.length; i++) {
                    float[] s = shadows[i];
                    layer(c, i, shadowBlur(s), new PorterDuffColorFilter(shadowColors[i][dark ? 1 : 0], PorterDuff.Mode.SRC_IN), s[0], s[1], dark, frame);
                }
                if (blur > 0) layer(c, shadows.length, blur, null, 0, 0, dark, frame);
                else for (Item item : items) item.draw(c, dark, frame);
                return;
            }
            // Before Android 12, or drawn into a bitmap (a transition's snapshot).
            if (baked == null || bakedDark != dark || !bakedFrame.equals(frame)) bake(dark, frame);
            if (baked != null) c.drawBitmap(baked, bakedLeft, bakedTop, null);
            else for (Item item : items) item.draw(c, dark, frame);
        }

        /**
         * The items in a layer of their own, blurred (`sigma`), filled with `tint`'s color (a
         * shadow) and moved by (`dx`, `dy`). The layer holds what the blur brings into `frame`.
         */
        private void layer(Canvas c, int index, float sigma, ColorFilter tint, float dx, float dy, boolean dark, RectF frame) {
            float out = reach(sigma);
            RectF area = new RectF(content);
            area.inset(-out, -out);
            if (!area.intersect(frame.left - dx - out, frame.top - dy - out, frame.right - dx + out, frame.bottom - dy + out)) return;
            roundOut(area);
            android.graphics.RenderNode node = (android.graphics.RenderNode) nodes[index];
            if (node == null) nodes[index] = node = new android.graphics.RenderNode("prism-drawing");
            node.setPosition((int) area.left, (int) area.top, (int) area.right, (int) area.bottom);
            Canvas rc = node.beginRecording();
            rc.translate(-area.left, -area.top);
            for (Item item : items) item.draw(rc, dark, frame);
            node.endRecording();
            float radius = blurRadius(sigma);
            android.graphics.RenderEffect effect = radius > 0
                    ? android.graphics.RenderEffect.createBlurEffect(radius, radius, Shader.TileMode.DECAL) : null;
            if (tint != null) {
                effect = effect == null ? android.graphics.RenderEffect.createColorFilterEffect(tint)
                        : android.graphics.RenderEffect.createColorFilterEffect(tint, effect);
            }
            node.setRenderEffect(effect);
            c.save();
            c.translate(dx, dy);
            c.drawRenderNode(node);
            c.restore();
        }

        /** Draws the shadows and the (blurred) items into `baked`, once for `frame` and the mode. */
        private void bake(boolean dark, RectF frame) {
            baked = null;
            bakedDark = dark;
            bakedFrame.set(frame);
            float out = reach(blur);
            for (float[] s : shadows) out = Math.max(out, reach(shadowBlur(s)) + (float) Math.ceil(Math.max(Math.abs(s[0]), Math.abs(s[1]))));
            // What is drawn of the items: what the blur and the shadows bring into the frame.
            RectF from = new RectF(frame);
            from.inset(-out, -out);
            if (!from.intersect(content)) return;
            roundOut(from);
            RectF to = new RectF(from);
            to.inset(-out, -out);
            int width = (int) to.width(), height = (int) to.height();
            // Too large for a bitmap: the items without their effects.
            if (width <= 0 || height <= 0 || (long) width * height > 8L << 20) return;
            try {
                Bitmap source = Bitmap.createBitmap((int) from.width(), (int) from.height(), Bitmap.Config.ARGB_8888);
                Canvas sc = new Canvas(source);
                sc.translate(-from.left, -from.top);
                for (Item item : items) item.draw(sc, dark, frame);
                Bitmap all = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
                Canvas ac = new Canvas(all);
                ac.translate(-to.left, -to.top);
                Paint paint = new Paint();
                int[] at = new int[2];
                for (int i = 0; i < shadows.length; i++) {
                    // The picture's opacity, blurred, in the shadow's color.
                    float[] s = shadows[i];
                    Paint soft = new Paint();
                    float radius = blurRadius(shadowBlur(s));
                    if (radius > 0) soft.setMaskFilter(new BlurMaskFilter(radius, BlurMaskFilter.Blur.NORMAL));
                    Bitmap shadow = source.extractAlpha(soft, at);
                    paint.setColor(shadowColors[i][dark ? 1 : 0]);
                    ac.drawBitmap(shadow, from.left + at[0] + s[0], from.top + at[1] + s[1], paint);
                    shadow.recycle();
                }
                if (blur > 0) {
                    int pad = (int) reach(blur);
                    Bitmap soft = blurred(source, blur, pad);
                    ac.drawBitmap(soft, from.left - pad, from.top - pad, null);
                    soft.recycle();
                } else {
                    ac.drawBitmap(source, from.left, from.top, null);
                }
                source.recycle();
                baked = all;
                bakedLeft = to.left;
                bakedTop = to.top;
            } catch (OutOfMemoryError e) {
                baked = null;
            }
        }
    }

    // --- Blur without a render effect ------------------------------------------------------

    /**
     * `src` blurred (standard deviation `sigma`), `pad` pixels larger on each side: three box
     * blurs each way, which is a Gaussian blur to the eye.
     */
    static Bitmap blurred(Bitmap src, float sigma, int pad) {
        int w = src.getWidth() + 2 * pad, h = src.getHeight() + 2 * pad;
        int[] pixels = new int[w * h];
        src.getPixels(pixels, pad * w + pad, w, 0, 0, src.getWidth(), src.getHeight());
        int[] boxes = boxes(sigma);
        int[] a = new int[w * h], b = new int[w * h], alpha = null, result = new int[w * h];
        // Opacity first, then each color times its opacity (a transparent pixel has no color
        // to spread), all out of 255 × 255.
        for (int channel = 0; channel < 4; channel++) {
            int shift = 24 - 8 * channel;
            for (int i = 0; i < pixels.length; i++) {
                a[i] = ((pixels[i] >>> shift) & 0xff) * (channel == 0 ? 255 : pixels[i] >>> 24);
            }
            for (int radius : boxes) {
                for (int y = 0; y < h; y++) box(a, b, y * w, w, 1, radius);
                for (int x = 0; x < w; x++) box(b, a, x, h, w, radius);
            }
            if (channel == 0) {
                alpha = a.clone();
                for (int i = 0; i < result.length; i++) result[i] = Math.round(alpha[i] / 255f) << 24;
            } else {
                for (int i = 0; i < result.length; i++) {
                    if (alpha[i] > 0) result[i] |= Math.min(255, Math.round(a[i] * 255f / alpha[i])) << shift;
                }
            }
        }
        Bitmap out = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888);
        out.setPixels(result, 0, w, 0, 0, w, h);
        return out;
    }

    /** The radii of three box blurs that add up to a Gaussian of standard deviation `sigma`. */
    static int[] boxes(float sigma) {
        double ideal = Math.sqrt(12.0 * sigma * sigma / 3 + 1);
        int low = (int) Math.floor(ideal);
        if (low % 2 == 0) low--;
        low = Math.max(1, low);
        long narrow = Math.round((12.0 * sigma * sigma - 3.0 * low * low - 12.0 * low - 9) / (-4.0 * low - 4));
        int[] radii = new int[3];
        for (int i = 0; i < 3; i++) radii[i] = ((i < narrow ? low : low + 2) - 1) / 2;
        return radii;
    }

    /** One line of a box blur: each of `count` values (`step` apart from `start`) becomes the mean of those within `radius`. */
    private static void box(int[] from, int[] to, int start, int count, int step, int radius) {
        int size = 2 * radius + 1;
        long sum = 0;
        for (int i = 0; i <= radius && i < count; i++) sum += from[start + i * step];
        for (int i = 0; i < count; i++) {
            to[start + i * step] = (int) (sum / size);
            if (i + radius + 1 < count) sum += from[start + (i + radius + 1) * step];
            if (i - radius >= 0) sum -= from[start + (i - radius) * step];
        }
    }
}
