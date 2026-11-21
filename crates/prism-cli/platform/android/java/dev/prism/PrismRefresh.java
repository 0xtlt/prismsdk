package dev.prism;

import android.animation.ValueAnimator;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.os.SystemClock;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.ViewConfiguration;
import android.widget.ScrollView;

/**
 * Pull to refresh (`Scroll::on_refresh`) with the framework alone, like
 * androidx's SwipeRefreshLayout: pulled down at its top, a refreshable scroll
 * view shows a disc that follows the finger; released past the threshold, it
 * refreshes (a click on the scroll view) and the disc spins until the app
 * stops refreshing (`Scroll::refreshing`, the `value` op). F5 or Ctrl+R
 * refresh the shown ones from a keyboard.
 */
final class PrismRefresh {
    private PrismRefresh() {}

    /** The disc's size, how far it rests while refreshing, and how much of the finger's travel it follows. */
    static final float DISC_DP = 40, REST_DP = 64, DRAG_RATE = 0.5f;

    /** A vertical scroll view that refreshes when pulled (`clickable` and vertical). */
    static final class Scroll extends ScrollView {
        final PrismHost.Node node;
        final float density, disc, rest;
        final int slop;
        final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG), arc = new Paint(Paint.ANTI_ALIAS_FLAG);
        final RectF oval = new RectF();
        /** The touch: where it went down, its pointer, and where the pull started (NaN: not pulling). */
        float downY, lastY, pullFrom = Float.NaN;
        int pointer = -1;
        /** How far the disc is down (px), its scale as it goes, and whether it spins. */
        float offset, scale = 1, pulled;
        boolean refreshing;
        long spinFrom;
        ValueAnimator settle;

        Scroll(Context c, PrismHost.Node node) {
            super(c);
            this.node = node;
            density = c.getResources().getDisplayMetrics().density;
            disc = DISC_DP * density;
            rest = REST_DP * density;
            slop = ViewConfiguration.get(c).getScaledTouchSlop();
            arc.setStyle(Paint.Style.STROKE);
            arc.setStrokeCap(Paint.Cap.ROUND);
            arc.setStrokeWidth(2.5f * density);
            setWillNotDraw(false);
        }

        boolean pullable() {
            return node.clickable && !refreshing && isEnabled();
        }

        @Override
        public boolean onInterceptTouchEvent(MotionEvent e) {
            if (!pullable()) return super.onInterceptTouchEvent(e);
            if (track(e)) return true;
            return super.onInterceptTouchEvent(e);
        }

        @Override
        public boolean onTouchEvent(MotionEvent e) {
            if (!pullable() && Float.isNaN(pullFrom)) return super.onTouchEvent(e);
            boolean wasPulling = !Float.isNaN(pullFrom);
            if (track(e)) {
                // The scroll view lets go of the touch it may have started.
                if (!wasPulling) {
                    MotionEvent cancel = MotionEvent.obtain(e);
                    cancel.setAction(MotionEvent.ACTION_CANCEL);
                    super.onTouchEvent(cancel);
                    cancel.recycle();
                }
                return true;
            }
            if (wasPulling) {
                // Pushed back above where it started: the content scrolls again.
                MotionEvent down = MotionEvent.obtain(e);
                down.setAction(MotionEvent.ACTION_DOWN);
                super.onTouchEvent(down);
                down.recycle();
                return true;
            }
            return super.onTouchEvent(e);
        }

        /** Follows the touch; true while it pulls the disc down. */
        boolean track(MotionEvent e) {
            switch (e.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                    pointer = e.getPointerId(0);
                    downY = lastY = e.getY();
                    pullFrom = Float.NaN;
                    return false;
                case MotionEvent.ACTION_POINTER_DOWN: {
                    // The newest finger leads, the disc stays where it is.
                    int i = e.getActionIndex();
                    pointer = e.getPointerId(i);
                    float y = e.getY(i);
                    if (!Float.isNaN(pullFrom)) pullFrom = y - pulled / DRAG_RATE;
                    downY = lastY = y;
                    return !Float.isNaN(pullFrom);
                }
                case MotionEvent.ACTION_POINTER_UP: {
                    int i = e.getActionIndex();
                    if (e.getPointerId(i) == pointer) {
                        int other = i == 0 ? 1 : 0;
                        pointer = e.getPointerId(other);
                        float y = e.getY(other);
                        if (!Float.isNaN(pullFrom)) pullFrom = y - pulled / DRAG_RATE;
                        downY = lastY = y;
                    }
                    return !Float.isNaN(pullFrom);
                }
                case MotionEvent.ACTION_MOVE: {
                    int i = e.findPointerIndex(pointer);
                    if (i < 0) return !Float.isNaN(pullFrom);
                    float y = e.getY(i);
                    boolean down = y >= lastY;
                    lastY = y;
                    if (Float.isNaN(pullFrom)) {
                        // Down from the top: past the slop from where it went
                        // down, or as soon as the content reaches its top.
                        if (!down || y - downY <= slop || canScrollVertically(-1)) return false;
                        pullFrom = y;
                        stopSettling();
                        scale = 1;
                    }
                    pulled = (y - pullFrom) * DRAG_RATE;
                    if (pulled < 0) {
                        pulled = 0;
                        pullFrom = Float.NaN;
                        follow(0);
                        return false;
                    }
                    follow(pulled);
                    return true;
                }
                case MotionEvent.ACTION_UP:
                case MotionEvent.ACTION_CANCEL: {
                    boolean pulling = !Float.isNaN(pullFrom);
                    pullFrom = Float.NaN;
                    pointer = -1;
                    if (!pulling) return false;
                    if (e.getActionMasked() == MotionEvent.ACTION_UP && pulled >= rest) refresh(this);
                    else settle(0, false);
                    pulled = 0;
                    return true;
                }
            }
            return !Float.isNaN(pullFrom);
        }

        /** The disc under the finger: one to one up to where it rests, then slower. */
        void follow(float pull) {
            offset = pull <= rest ? pull : rest + rest * (1 - 1 / (1 + (pull - rest) / rest));
            invalidate();
        }

        void stopSettling() {
            if (settle != null) settle.cancel();
            settle = null;
        }

        /** Moves the disc to `to` (spinning there), or shrinks it away. */
        void settle(float to, boolean shrink) {
            stopSettling();
            ValueAnimator a = shrink ? ValueAnimator.ofFloat(scale, 0) : ValueAnimator.ofFloat(offset, to);
            a.setDuration(shrink ? 150 : 200);
            a.addUpdateListener(v -> {
                if (shrink) scale = (float) v.getAnimatedValue();
                else offset = (float) v.getAnimatedValue();
                invalidate();
            });
            a.addListener(new android.animation.AnimatorListenerAdapter() {
                boolean cancelled;

                @Override
                public void onAnimationCancel(android.animation.Animator x) {
                    cancelled = true;
                }

                @Override
                public void onAnimationEnd(android.animation.Animator x) {
                    if (cancelled || !shrink) return;
                    offset = 0;
                    scale = 1;
                    invalidate();
                }
            });
            settle = a;
            a.start();
        }

        void setRefreshing(boolean on) {
            if (on == refreshing) return;
            refreshing = on;
            if (on) {
                spinFrom = SystemClock.uptimeMillis();
                scale = 1;
                settle(rest, false);
            } else {
                settle(0, offset > 0);
            }
            invalidate();
        }

        @Override
        public void draw(Canvas canvas) {
            super.draw(canvas);
            if (offset <= 0 || scale <= 0) return;
            float r = disc / 2;
            float cx = getScrollX() + getWidth() / 2f;
            float cy = getScrollY() + offset - r;
            canvas.save();
            canvas.scale(scale, scale, cx, cy);
            boolean dark = PrismHost.dark();
            fill.setColor(dark ? 0xFF2C2C2E : 0xFFFAFAFA);
            fill.setShadowLayer(3 * density, 0, 1.5f * density, 0x40000000);
            canvas.drawCircle(cx, cy, r, fill);
            float ring = 7.5f * density;
            oval.set(cx - ring, cy - ring, cx + ring, cy + ring);
            int accent = PrismHost.accent();
            float start, sweep;
            if (refreshing) {
                // Turns and breathes, like the platform's indeterminate spinner.
                long t = SystemClock.uptimeMillis() - spinFrom;
                start = (t * 0.3f) % 360;
                sweep = 20 + 250 * (0.5f - 0.5f * (float) Math.cos(2 * Math.PI * t / 1332));
                arc.setColor(accent);
                postInvalidateOnAnimation();
            } else {
                // Fills as it is pulled, faint until a release would refresh.
                float progress = Math.min(1, offset / rest);
                start = -90 + progress * 180;
                sweep = 290 * progress;
                arc.setColor(offset >= rest ? accent : (accent & 0x00FFFFFF) | 0x4C000000);
            }
            canvas.drawArc(oval, start, sweep, false, arc);
            canvas.restore();
        }
    }

    /** Refreshes `s` as the app asked: the disc spins and the app hears a click. */
    static void refresh(Scroll s) {
        s.setRefreshing(true);
        PrismHost.send(PrismHost.event("click", s.node.id));
    }

    /** `Scroll::refreshing`: the disc spins until it is off. */
    static void setRefreshing(PrismHost.Node n, boolean on) {
        if (n.view instanceof Scroll) ((Scroll) n.view).setRefreshing(on);
    }

    /** F5 or Ctrl+R: refreshes the refreshable scroll views on screen. */
    static boolean key(KeyEvent e) {
        if (e.getAction() != KeyEvent.ACTION_DOWN || e.getRepeatCount() > 0) return false;
        if (!isReload(e.getKeyCode(), e.isCtrlPressed(), e.isShiftPressed() || e.isAltPressed() || e.isMetaPressed())) {
            return false;
        }
        boolean any = false;
        for (PrismHost.Node n : PrismHost.nodes.values()) {
            if (!n.clickable || !(n.view instanceof Scroll) || !n.view.isShown()) continue;
            Scroll s = (Scroll) n.view;
            if (s.refreshing) continue;
            refresh(s);
            any = true;
        }
        return any;
    }

    static boolean isReload(int key, boolean ctrl, boolean others) {
        if (others) return false;
        return key == KeyEvent.KEYCODE_F5 ? !ctrl : key == KeyEvent.KEYCODE_R && ctrl;
    }
}
