package dev.prism;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.DashPathEffect;
import android.graphics.Paint;
import android.graphics.Path;
import android.view.View;

import org.json.JSONArray;
import org.json.JSONObject;

/**
 * A Prism shape (prism-core `shape`): paths in pixels, resolved by Rust for
 * the view's current size. `d` is flat: `0 x y` move, `1 x y` line,
 * `2 x1 y1 x2 y2 x y` curve, `3` close. An SVG document also has text,
 * images and groups (`items`, #342). Colors follow light/dark mode when
 * drawn.
 */
final class PrismShape extends View {
    private PrismDrawing drawing = PrismDrawing.EMPTY;

    PrismShape(Context context) {
        super(context);
    }

    /** A `draw` op's `paths`. */
    void setPaths(JSONArray paths) {
        drawing = PrismDrawing.ofPaths(paths);
        invalidate();
    }

    /** A `draw` op's `items`: what a drawing with more than paths draws. */
    void setItems(JSONArray items) {
        drawing = PrismDrawing.ofItems(items);
        invalidate();
    }

    @Override
    protected void onDraw(Canvas canvas) {
        drawing.draw(canvas, getWidth(), getHeight());
    }

    /**
     * Draws `paths` scaled by `scale` and moved by (`dx`, `dy`), stroke widths times
     * `strokeScale`. A path with `fc` / `sc` (a chip's vector icon, #329) fills / strokes in
     * `ink` at that opacity: the text color it sits in.
     */
    static void draw(Canvas canvas, JSONArray paths, Paint paint, float scale, float dx, float dy,
            float strokeScale, int ink) {
        for (int i = 0; i < paths.length(); i++) {
            JSONObject p = paths.optJSONObject(i);
            if (p == null) continue;
            Path path = path(p.optJSONArray("d"), scale, dx, dy);
            if (p.optBoolean("eo")) path.setFillType(Path.FillType.EVEN_ODD);
            Integer fill = p.has("fc") ? Integer.valueOf(withAlpha(ink, p.optDouble("fc"))) : PrismHost.color(p.optJSONArray("fill"));
            if (fill != null) {
                paint.reset();
                paint.setAntiAlias(true);
                paint.setStyle(Paint.Style.FILL);
                paint.setColor(fill);
                canvas.drawPath(path, paint);
            }
            Integer stroke = p.has("sc") ? Integer.valueOf(withAlpha(ink, p.optDouble("sc"))) : PrismHost.color(p.optJSONArray("stroke"));
            if (stroke != null) {
                paint.reset();
                paint.setAntiAlias(true);
                paint.setStyle(Paint.Style.STROKE);
                paint.setColor(stroke);
                paint.setStrokeWidth((float) p.optDouble("w", 1) * strokeScale);
                if (p.optBoolean("round")) {
                    paint.setStrokeCap(Paint.Cap.ROUND);
                    paint.setStrokeJoin(Paint.Join.ROUND);
                }
                JSONArray dash = p.optJSONArray("dash");
                if (dash != null && dash.length() == 2) {
                    paint.setPathEffect(new DashPathEffect(new float[]{
                            (float) dash.optDouble(0) * strokeScale, (float) dash.optDouble(1) * strokeScale}, 0));
                }
                canvas.drawPath(path, paint);
            }
        }
    }

    /** `color` with its opacity times `alpha`. */
    private static int withAlpha(int color, double alpha) {
        int a = (int) Math.round(((color >>> 24) & 0xff) * Math.max(0, Math.min(1, alpha)));
        return (color & 0x00ffffff) | (a << 24);
    }

    /** The outline `d`, scaled by `scale` and moved by (`dx`, `dy`). */
    static Path path(JSONArray d, float scale, float dx, float dy) {
        Path p = new Path();
        if (d == null) return p;
        int i = 0;
        while (i < d.length()) {
            int cmd = d.optInt(i++);
            float[] v = new float[6];
            int count = cmd == 2 ? 6 : cmd == 3 ? 0 : 2;
            if (i + count > d.length()) break;
            for (int k = 0; k < count; k++) v[k] = (float) d.optDouble(i++) * scale + (k % 2 == 0 ? dx : dy);
            switch (cmd) {
                case 0: p.moveTo(v[0], v[1]); break;
                case 1: p.lineTo(v[0], v[1]); break;
                case 2: p.cubicTo(v[0], v[1], v[2], v[3], v[4], v[5]); break;
                case 3: p.close(); break;
                default: return p;
            }
        }
        return p;
    }
}
