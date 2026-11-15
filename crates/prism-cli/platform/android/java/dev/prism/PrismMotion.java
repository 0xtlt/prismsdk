package dev.prism;

import android.animation.ValueAnimator;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.GradientDrawable;
import android.view.View;
import android.view.ViewGroup;
import android.view.animation.PathInterpolator;
import android.widget.ImageView;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.HashMap;
import java.util.Map;

/**
 * Prism's motion on Android (prism-core `animation`): transforms, animated
 * style and frame changes, exit pictures and loops, with property animators.
 * Values come in pixels and seconds; curves as cubic Béziers.
 */
final class PrismMotion {
    private PrismMotion() {}

    /** Running loops, by node. */
    static final Map<Long, ValueAnimator> loops = new HashMap<>();
    /** Running frame animations, by node. */
    static final Map<Long, ValueAnimator> frames = new HashMap<>();

    /** `tf`: [x, y, scale, degrees], or null for none (around the center). */
    static void transform(View v, JSONArray tf) {
        v.setTranslationX(tf == null ? 0 : (float) tf.optDouble(0));
        v.setTranslationY(tf == null ? 0 : (float) tf.optDouble(1));
        float s = tf == null ? 1 : (float) tf.optDouble(2, 1);
        v.setScaleX(s);
        v.setScaleY(s);
        v.setRotation(tf == null ? 0 : (float) tf.optDouble(3));
    }

    static PathInterpolator curve(JSONObject anim) {
        JSONArray e = anim.optJSONArray("ease");
        if (e == null || e.length() < 4) return new PathInterpolator(0.42f, 0f, 0.58f, 1f);
        return new PathInterpolator((float) e.optDouble(0), (float) e.optDouble(1), (float) e.optDouble(2), (float) e.optDouble(3));
    }

    static long ms(JSONObject anim, String key) {
        return Math.round(anim.optDouble(key, 0) * 1000);
    }

    /** What animates of a view: alpha, translation, scale, rotation. */
    static float[] pose(View v) {
        return new float[]{v.getAlpha(), v.getTranslationX(), v.getTranslationY(), v.getScaleX(), v.getRotation()};
    }

    static void set(View v, float[] p) {
        v.setAlpha(p[0]);
        v.setTranslationX(p[1]);
        v.setTranslationY(p[2]);
        v.setScaleX(p[3]);
        v.setScaleY(p[3]);
        v.setRotation(p[4]);
    }

    /** A pose sent by Rust: `{o, tf}`. */
    static float[] pose(JSONObject p) {
        JSONArray tf = p.optJSONArray("tf");
        return new float[]{(float) p.optDouble("o", 1),
                tf == null ? 0 : (float) tf.optDouble(0), tf == null ? 0 : (float) tf.optDouble(1),
                tf == null ? 1 : (float) tf.optDouble(2, 1), tf == null ? 0 : (float) tf.optDouble(3)};
    }

    static Integer color(View v) {
        Drawable d = v.getBackground();
        if (d instanceof GradientDrawable && ((GradientDrawable) d).getColor() != null) {
            return ((GradientDrawable) d).getColor().getDefaultColor();
        }
        return null;
    }

    /** `apply` sets the new style at once; this animates there from before. */
    static void style(View v, JSONObject anim, Runnable apply) {
        float[] from = pose(v);
        Integer fromBg = color(v);
        apply.run();
        float[] to = pose(v);
        Integer toBg = color(v);
        set(v, from);
        v.animate().cancel();
        v.animate().alpha(to[0]).translationX(to[1]).translationY(to[2]).scaleX(to[3]).scaleY(to[3]).rotation(to[4])
                .setDuration(ms(anim, "d")).setStartDelay(ms(anim, "delay")).setInterpolator(curve(anim)).start();
        if (fromBg != null && toBg != null && !fromBg.equals(toBg)) {
            GradientDrawable d = (GradientDrawable) v.getBackground();
            d.setColor(fromBg);
            ValueAnimator a = ValueAnimator.ofArgb(fromBg, toBg);
            a.setDuration(ms(anim, "d"));
            a.setStartDelay(ms(anim, "delay"));
            a.setInterpolator(curve(anim));
            a.addUpdateListener(x -> d.setColor((Integer) x.getAnimatedValue()));
            a.start();
        }
    }

    /** Moves and resizes a child of a PrismLayout from its current frame. */
    static void frame(long id, View v, int[] to, JSONObject anim) {
        PrismLayout.Frame f = PrismLayout.frameOf(v);
        int[] from = {f.x, f.y, f.width, f.height};
        ValueAnimator old = frames.remove(id);
        if (old != null) old.cancel();
        ValueAnimator a = ValueAnimator.ofFloat(0, 1);
        a.setDuration(ms(anim, "d"));
        a.setStartDelay(ms(anim, "delay"));
        a.setInterpolator(curve(anim));
        a.addUpdateListener(x -> {
            float t = (Float) x.getAnimatedValue();
            PrismLayout.Frame g = PrismLayout.frameOf(v);
            g.x = Math.round(from[0] + (to[0] - from[0]) * t);
            g.y = Math.round(from[1] + (to[1] - from[1]) * t);
            g.width = Math.round(from[2] + (to[2] - from[2]) * t);
            g.height = Math.round(from[3] + (to[3] - from[3]) * t);
            v.setLayoutParams(g);
        });
        frames.put(id, a);
        a.start();
    }

    /** A picture of the view stays in its parent's overlay and animates out. */
    static void exit(View v, JSONObject to, JSONObject anim) {
        if (!(v.getParent() instanceof ViewGroup) || v.getWidth() <= 0 || v.getHeight() <= 0) return;
        ViewGroup parent = (ViewGroup) v.getParent();
        Bitmap b = Bitmap.createBitmap(v.getWidth(), v.getHeight(), Bitmap.Config.ARGB_8888);
        v.draw(new Canvas(b));
        ImageView picture = new ImageView(v.getContext());
        picture.setImageBitmap(b);
        picture.measure(View.MeasureSpec.makeMeasureSpec(v.getWidth(), View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(v.getHeight(), View.MeasureSpec.EXACTLY));
        picture.layout(v.getLeft(), v.getTop(), v.getRight(), v.getBottom());
        set(picture, pose(v));
        parent.getOverlay().add(picture);
        float[] p = pose(to);
        picture.animate().alpha(p[0]).translationX(p[1]).translationY(p[2]).scaleX(p[3]).scaleY(p[3]).rotation(p[4])
                .setDuration(ms(anim, "d")).setStartDelay(ms(anim, "delay")).setInterpolator(curve(anim))
                .withEndAction(() -> parent.getOverlay().remove(picture)).start();
    }

    /** Back and forth between two poses, forever (`to` null stops). */
    static void repeat(long id, View v, JSONObject from, JSONObject to, JSONObject anim) {
        ValueAnimator old = loops.remove(id);
        if (old != null) old.cancel();
        if (to == null || from == null || anim == null) {
            if (from != null) set(v, pose(from));
            return;
        }
        float[] a = pose(from), b = pose(to);
        ValueAnimator loop = ValueAnimator.ofFloat(0, 1);
        loop.setDuration(Math.max(10, ms(anim, "d")));
        loop.setStartDelay(ms(anim, "delay"));
        // Springs would overshoot every swing: loops ease in and out.
        loop.setInterpolator(new PathInterpolator(0.42f, 0f, 0.58f, 1f));
        loop.setRepeatMode(ValueAnimator.REVERSE);
        loop.setRepeatCount(ValueAnimator.INFINITE);
        loop.addUpdateListener(x -> {
            float t = (Float) x.getAnimatedValue();
            float[] p = new float[5];
            for (int i = 0; i < 5; i++) p[i] = a[i] + (b[i] - a[i]) * t;
            set(v, p);
        });
        loops.put(id, loop);
        loop.start();
    }

    /** A view is gone: its animations stop. */
    static void forget(long id) {
        ValueAnimator a = loops.remove(id);
        if (a != null) a.cancel();
        a = frames.remove(id);
        if (a != null) a.cancel();
    }
}
