package dev.prism;

import android.graphics.Bitmap;
import android.graphics.BlurMaskFilter;
import android.graphics.Canvas;
import android.graphics.ColorFilter;
import android.graphics.DashPathEffect;
import android.graphics.LinearGradient;
import android.graphics.Matrix;
import android.graphics.Outline;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.PixelFormat;
import android.graphics.RadialGradient;
import android.graphics.Rect;
import android.graphics.RectF;
import android.graphics.Shader;
import android.graphics.drawable.Drawable;
import android.os.Build;
import android.view.View;
import android.view.ViewOutlineProvider;
import android.view.ViewTreeObserver;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.Map;
import java.util.WeakHashMap;

/**
 * Box decorations of a Prism style (prism_core `decoration`): shadows (drop
 * and inset, several), per-side, dashed and dotted borders, gradients,
 * clipping to the rounded box, and backdrop blur. Styles arrive in pixels.
 */
final class PrismDecor {
    private PrismDecor() {}

    /** Whether the style needs more than a GradientDrawable. */
    static boolean needed(JSONObject s) {
        return s.has("shadows") || s.has("sides") || s.has("bstyle") || s.has("grad") || s.optDouble("blur", 0) > 0;
    }

    /** The rounded box `l, t, r, b`: `corners` bits 1 top left, 2 top right, 4 bottom right, 8 bottom left. */
    static Path box(float l, float t, float r, float b, float radius, int corners) {
        float rad = Math.max(0, Math.min(radius, Math.min(r - l, b - t) / 2));
        float tl = (corners & 1) != 0 ? rad : 0, tr = (corners & 2) != 0 ? rad : 0;
        float br = (corners & 4) != 0 ? rad : 0, bl = (corners & 8) != 0 ? rad : 0;
        Path p = new Path();
        p.addRoundRect(l, t, r, b, new float[]{tl, tl, tr, tr, br, br, bl, bl}, Path.Direction.CW);
        return p;
    }

    /** A CSS blur radius (a standard deviation) as Skia's blur radius. */
    static float skiaRadius(float sigma) {
        return Math.max(0.01f, (sigma - 0.5f) / 0.57735f);
    }

    /** The dash pattern of `bstyle` for a line `w` wide (prism_core BorderStyle::dash), or null. */
    static DashPathEffect dash(String style, float w) {
        w = Math.max(1, w);
        if ("dashed".equals(style)) return new DashPathEffect(new float[]{3 * w, 2 * w}, 0);
        if ("dotted".equals(style)) return new DashPathEffect(new float[]{0.01f, 2 * w}, 0);
        return null;
    }

    /** Background, borders, shadows, gradient and backdrop of one view. */
    static final class Box extends Drawable {
        final View view;
        final Integer bg;
        final float radius;
        final int corners;
        final JSONArray shadows;
        final float bw;
        final Integer bc;
        /** Per side: widths and colors (top, right, bottom, left), or null. */
        final float[] sideW;
        final Integer[] sideC;
        final String bstyle;
        final JSONObject grad;
        final float blur;
        final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        /** The backdrop (what is behind the view, downscaled), and its blurred node. */
        Bitmap backdrop;
        Object node;
        boolean recorded;

        Box(View view, JSONObject s) {
            this.view = view;
            bg = PrismHost.color(s.optJSONArray("bg"));
            radius = (float) s.optDouble("radius", 0);
            corners = s.optInt("corners", 15);
            shadows = s.optJSONArray("shadows");
            bw = (float) s.optDouble("bw", 0);
            bc = PrismHost.color(s.optJSONArray("bc"));
            JSONArray sides = s.optJSONArray("sides");
            if (sides != null && sides.length() == 4) {
                sideW = new float[4];
                sideC = new Integer[4];
                for (int i = 0; i < 4; i++) {
                    JSONArray side = sides.optJSONArray(i);
                    sideW[i] = side == null ? 0 : (float) side.optDouble(0, 0);
                    sideC[i] = side == null ? null : PrismHost.color(side.optJSONArray(1));
                }
            } else {
                sideW = null;
                sideC = null;
            }
            bstyle = s.optString("bstyle", "solid");
            grad = s.optJSONObject("grad");
            blur = (float) s.optDouble("blur", 0);
            if (blur > 0) Backdrops.add(view, this);
        }

        Path shape(float inset) {
            Rect b = getBounds();
            return box(b.left + inset, b.top + inset, b.right - inset, b.bottom - inset, radius - inset, corners);
        }

        @Override
        public void draw(Canvas c) {
            Rect b = getBounds();
            float w = b.width(), h = b.height();
            if (w <= 0 || h <= 0) return;
            Path shape = shape(0);
            // Drop shadows, the first on top: outside the box only (like CSS).
            if (shadows != null) {
                for (int i = shadows.length() - 1; i >= 0; i--) {
                    JSONObject sh = shadows.optJSONObject(i);
                    if (sh == null || sh.optBoolean("inset")) continue;
                    drawShadow(c, shape, sh, b);
                }
            }
            if (blur > 0 && Backdrops.capturing == null) drawBackdrop(c, shape, b);
            if (bg != null) {
                paint.reset();
                paint.setAntiAlias(true);
                paint.setColor(bg);
                c.drawPath(shape, paint);
            }
            if (grad != null) {
                paint.reset();
                paint.setAntiAlias(true);
                paint.setShader(gradient(grad, b));
                c.drawPath(shape, paint);
                paint.setShader(null);
            }
            if (shadows != null) {
                for (int i = shadows.length() - 1; i >= 0; i--) {
                    JSONObject sh = shadows.optJSONObject(i);
                    if (sh == null || !sh.optBoolean("inset")) continue;
                    drawInset(c, shape, sh, b);
                }
            }
            drawBorders(c, shape, b);
        }

        void drawShadow(Canvas c, Path shape, JSONObject sh, Rect b) {
            Integer color = PrismHost.color(sh.optJSONArray("color"));
            if (color == null) return;
            float x = (float) sh.optDouble("x"), y = (float) sh.optDouble("y");
            float blurR = (float) sh.optDouble("blur"), spread = (float) sh.optDouble("spread");
            paint.reset();
            paint.setAntiAlias(true);
            paint.setColor(color);
            if (blurR > 0) paint.setMaskFilter(new BlurMaskFilter(skiaRadius(blurR / 2), BlurMaskFilter.Blur.NORMAL));
            Path s = box(b.left - spread + x, b.top - spread + y, b.right + spread + x, b.bottom + spread + y,
                    radius > 0 ? radius + spread : 0, corners);
            c.save();
            c.clipOutPath(shape);
            c.drawPath(s, paint);
            c.restore();
            paint.setMaskFilter(null);
        }

        void drawInset(Canvas c, Path shape, JSONObject sh, Rect b) {
            Integer color = PrismHost.color(sh.optJSONArray("color"));
            if (color == null) return;
            float x = (float) sh.optDouble("x"), y = (float) sh.optDouble("y");
            float blurR = (float) sh.optDouble("blur"), spread = (float) sh.optDouble("spread");
            paint.reset();
            paint.setAntiAlias(true);
            paint.setColor(color);
            if (blurR > 0) paint.setMaskFilter(new BlurMaskFilter(skiaRadius(blurR / 2), BlurMaskFilter.Blur.NORMAL));
            // Everything around the box moved by the offset and shrunk by the
            // spread, seen through the box: the shadow falls inside.
            float far = Math.max(b.width(), b.height()) + blurR * 2 + Math.abs(x) + Math.abs(y);
            Path ring = new Path();
            ring.setFillType(Path.FillType.EVEN_ODD);
            ring.addRect(b.left - far, b.top - far, b.right + far, b.bottom + far, Path.Direction.CW);
            ring.addPath(box(b.left + spread + x, b.top + spread + y, b.right - spread + x, b.bottom - spread + y,
                    Math.max(0, radius - spread), corners));
            c.save();
            c.clipPath(shape);
            c.drawPath(ring, paint);
            c.restore();
            paint.setMaskFilter(null);
        }

        void drawBorders(Canvas c, Path shape, Rect b) {
            paint.reset();
            paint.setAntiAlias(true);
            paint.setStyle(Paint.Style.STROKE);
            if ("dotted".equals(bstyle)) paint.setStrokeCap(Paint.Cap.ROUND);
            if (sideW == null) {
                if (bc == null || bw <= 0) return;
                paint.setColor(bc);
                paint.setStrokeWidth(bw);
                paint.setPathEffect(dash(bstyle, bw));
                c.drawPath(shape(bw / 2), paint);
                return;
            }
            // Each side along its edge, clipped to the rounded box.
            c.save();
            c.clipPath(shape);
            float l = b.left, t = b.top, r = b.right, bo = b.bottom;
            float[][] lines = {
                    {l, t + sideW[0] / 2, r, t + sideW[0] / 2},
                    {r - sideW[1] / 2, t, r - sideW[1] / 2, bo},
                    {l, bo - sideW[2] / 2, r, bo - sideW[2] / 2},
                    {l + sideW[3] / 2, t, l + sideW[3] / 2, bo},
            };
            for (int i = 0; i < 4; i++) {
                if (sideW[i] <= 0 || sideC[i] == null) continue;
                paint.setColor(sideC[i]);
                paint.setStrokeWidth(sideW[i]);
                paint.setPathEffect(dash(bstyle, sideW[i]));
                float[] p = lines[i];
                c.drawLine(p[0], p[1], p[2], p[3], paint);
            }
            c.restore();
            paint.setPathEffect(null);
        }

        /** The backdrop captured before this frame, blurred, inside the box. */
        void drawBackdrop(Canvas c, Path shape, Rect b) {
            Bitmap bmp = backdrop;
            if (bmp == null) return;
            c.save();
            c.clipPath(shape);
            if (Build.VERSION.SDK_INT >= 31 && c.isHardwareAccelerated()) {
                android.graphics.RenderNode n = (android.graphics.RenderNode) node;
                if (n == null) {
                    n = new android.graphics.RenderNode("prism-backdrop");
                    node = n;
                    recorded = false;
                }
                if (!recorded || n.getWidth() != b.width() || n.getHeight() != b.height()) {
                    n.setPosition(0, 0, b.width(), b.height());
                    android.graphics.RecordingCanvas rc = n.beginRecording(b.width(), b.height());
                    Paint f = new Paint(Paint.FILTER_BITMAP_FLAG);
                    rc.drawBitmap(bmp, null, new RectF(0, 0, b.width(), b.height()), f);
                    n.endRecording();
                    float r = skiaRadius(blur);
                    n.setRenderEffect(android.graphics.RenderEffect.createBlurEffect(r, r, Shader.TileMode.CLAMP));
                    recorded = true;
                }
                c.translate(b.left, b.top);
                c.drawRenderNode(n);
            } else {
                // Before Android 12: a strongly downscaled copy, smoothed when
                // scaled back up (a soft box blur).
                Paint f = new Paint(Paint.FILTER_BITMAP_FLAG);
                c.drawBitmap(bmp, null, new RectF(b), f);
            }
            c.restore();
        }

        @Override
        public void getOutline(Outline o) {
            Rect b = getBounds();
            if (corners == 15 || radius <= 0) {
                o.setRoundRect(b, Math.max(0, Math.min(radius, Math.min(b.width(), b.height()) / 2f)));
            } else if (Build.VERSION.SDK_INT >= 30) {
                o.setPath(shape(0));
            } else {
                o.setConvexPath(shape(0));
            }
        }

        @Override public void setAlpha(int alpha) {}
        @Override public void setColorFilter(ColorFilter f) {}
        @Override public int getOpacity() { return PixelFormat.TRANSLUCENT; }
    }

    /** CSS gradient geometry (prism_core Gradient::geometry) as a shader in `b`. */
    static Shader gradient(JSONObject g, Rect b) {
        JSONArray stops = g.optJSONArray("stops");
        int count = stops == null ? 0 : stops.length();
        int[] colors = new int[Math.max(2, count)];
        float[] at = new float[colors.length];
        for (int i = 0; i < count; i++) {
            JSONArray st = stops.optJSONArray(i);
            Integer c = st == null ? null : PrismHost.color(st.optJSONArray(0));
            colors[i] = c == null ? 0 : c;
            at[i] = st == null ? 0 : (float) st.optDouble(1, 0);
        }
        if (count == 1) {
            colors[1] = colors[0];
            at[1] = 1;
        }
        float w = b.width(), h = b.height(), cx = b.left + w / 2, cy = b.top + h / 2;
        if ("radial".equals(g.optString("kind"))) {
            float k = (float) Math.sqrt(2) / 2, rx = Math.max(1, w * k), ry = Math.max(1, h * k);
            RadialGradient r = new RadialGradient(cx, cy, rx, colors, at, Shader.TileMode.CLAMP);
            Matrix m = new Matrix();
            m.setScale(1, ry / rx, cx, cy);
            r.setLocalMatrix(m);
            return r;
        }
        double a = Math.toRadians(g.optDouble("angle", 180));
        float dx = (float) Math.sin(a), dy = (float) -Math.cos(a);
        float half = (w * Math.abs(dx) + h * Math.abs(dy)) / 2;
        return new LinearGradient(cx - dx * half, cy - dy * half, cx + dx * half, cy + dy * half, colors, at, Shader.TileMode.CLAMP);
    }

    // --- Clipping -------------------------------------------------------------------------

    /** Clips the view's children to its rounded box (`clip`; pictures always). */
    static void clip(PrismHost.Node n) {
        JSONObject s = n.style;
        View v = n.view;
        boolean clip = s.optBoolean("clip") || n.kind.equals("picture");
        float radius = (float) s.optDouble("radius", 0);
        int corners = s.optInt("corners", 15);
        boolean perCorner = radius > 0 && corners != 15;
        // Outlines clip any shape from Android 13 (Outline.canClip); before,
        // only rectangles and round rectangles: per-corner boxes clip in draw.
        boolean outline = clip && (!perCorner || Build.VERSION.SDK_INT >= 33);
        if (outline) {
            v.setOutlineProvider(new ViewOutlineProvider() {
                @Override public void getOutline(View view, Outline o) {
                    int w = view.getWidth(), h = view.getHeight();
                    if (!perCorner) {
                        o.setRoundRect(0, 0, w, h, Math.max(0, Math.min(radius, Math.min(w, h) / 2f)));
                    } else if (Build.VERSION.SDK_INT >= 30) {
                        o.setPath(box(0, 0, w, h, radius, corners));
                    }
                }
            });
        } else {
            v.setOutlineProvider(ViewOutlineProvider.BACKGROUND);
        }
        v.setClipToOutline(outline);
        if (n.host != null && v == n.host) {
            n.host.clipRadius = clip && !outline ? radius : -1;
            n.host.clipCorners = corners;
            n.host.invalidate();
        }
    }

    // --- Backdrop blur --------------------------------------------------------------------

    /**
     * Views with a backdrop blur. Android has no backdrop filter for views
     * (RenderEffect blurs a view's own content, window blur is per window):
     * what is behind is drawn into a small bitmap before each frame where
     * something moved (scrolls, layouts, Prism updates), then blurred with
     * RenderEffect (Android 12+) or smoothed when scaled back up.
     */
    static final class Backdrops {
        static final Map<View, Box> boxes = new WeakHashMap<>();
        /** The view whose backdrop is being drawn (it and its children skip). */
        static View capturing;
        static boolean dirty = true, installed;

        static void add(View v, Box b) {
            boxes.put(v, b);
            dirty = true;
            if (installed) return;
            installed = true;
            ViewTreeObserver vto = PrismHost.activity.getWindow().getDecorView().getViewTreeObserver();
            vto.addOnPreDrawListener(() -> {
                if (dirty && !boxes.isEmpty()) {
                    dirty = false;
                    capture();
                }
                return true;
            });
            vto.addOnScrollChangedListener(Backdrops::changed);
            vto.addOnGlobalLayoutListener(Backdrops::changed);
        }

        /** Something behind may have moved: capture again before the next frame. */
        static void changed() {
            if (boxes.isEmpty()) return;
            dirty = true;
            for (View v : boxes.keySet()) v.invalidate();
        }

        static void capture() {
            View root = PrismHost.activity.getWindow().getDecorView();
            int[] r = new int[2], at = new int[2];
            root.getLocationInWindow(r);
            for (Map.Entry<View, Box> e : boxes.entrySet()) {
                View v = e.getKey();
                Box b = e.getValue();
                if (v.getBackground() != b || !v.isAttachedToWindow() || v.getWidth() <= 0 || v.getHeight() <= 0) continue;
                // Small: blurred anyway (an 8th before Android 12, where the
                // upscaling is the blur; a 4th with RenderEffect).
                float scale = Build.VERSION.SDK_INT >= 31 ? 0.25f : 0.125f;
                int w = Math.max(1, Math.round(v.getWidth() * scale)), h = Math.max(1, Math.round(v.getHeight() * scale));
                Bitmap bmp = b.backdrop;
                if (bmp == null || bmp.getWidth() != w || bmp.getHeight() != h) {
                    bmp = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888);
                }
                bmp.eraseColor(0);
                v.getLocationInWindow(at);
                Canvas c = new Canvas(bmp);
                c.scale(scale, scale);
                c.translate(r[0] - at[0], r[1] - at[1]);
                capturing = v;
                try {
                    root.draw(c);
                } catch (RuntimeException ignored) {
                    // A view that only draws with the GPU: no backdrop this frame.
                } finally {
                    capturing = null;
                }
                b.backdrop = bmp;
                b.recorded = false;
                v.invalidate();
            }
        }
    }
}
