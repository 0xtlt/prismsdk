package dev.prism;

import android.app.Dialog;
import android.content.Context;
import android.graphics.Rect;
import android.graphics.drawable.GradientDrawable;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewConfiguration;
import android.view.ViewGroup;
import android.view.Window;
import android.view.WindowManager;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.PopupWindow;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.Map;
import java.util.WeakHashMap;

/**
 * Pointer, drag and key events, focus, scrolling into view, hit testing,
 * the runtime's layers and overlays (sheets, popovers, dialogs) a screen
 * opens without a route. Positions are sent in Prism points (pixels / density).
 */
final class PrismInput {
    private PrismInput() {}

    /** Every Prism view, to find the node that has the focus. */
    static final Map<View, PrismHost.Node> byView = new WeakHashMap<>();

    static final int HOVER = 1, MOVE = 2, DRAG = 4, WHEEL = 8, MAGNIFY = 16, DRAG_MIDDLE = 32, DRAG_SECONDARY = 64;

    static float dp(float px) { return px / PrismHost.density; }

    static void put(JSONObject o, String k, Object v) {
        try { o.put(k, v); } catch (JSONException ignored) {}
    }

    // --- Pointer --------------------------------------------------------------------------

    static void pointer(PrismHost.Node n, String phase, float x, float y) {
        JSONObject ev = PrismHost.event("pointer", n.id);
        put(ev, "phase", phase);
        put(ev, "x", dp(x));
        put(ev, "y", dp(y));
        PrismHost.send(ev);
    }

    static void drag(PrismHost.Node n, String phase, MotionEvent e) {
        float[] c = e == null ? null : center(e, -1);
        JSONObject ev = dragEvent(n, phase, c);
        // A draggable's drag (#251): with the drop target under the finger.
        if (n.dragKind != null) payload(n, phase, c, ev);
        PrismHost.send(ev);
    }

    /** A drag report of `n`, the pointers' raw center at `c` (null: none). */
    static JSONObject dragEvent(PrismHost.Node n, String phase, float[] c) {
        JSONObject ev = PrismHost.event("drag", n.id);
        put(ev, "phase", phase);
        put(ev, "x", dp(n.down[0]));
        put(ev, "y", dp(n.down[1]));
        // Raw coordinates: the view may move while it is dragged (reordering).
        // The fingers' center: a second finger (a pinch) moves it too.
        put(ev, "dx", c == null ? 0 : dp(c[0] - n.down[2]));
        put(ev, "dy", c == null ? 0 : dp(c[1] - n.down[3]));
        // The button and keys it started with (#136).
        put(ev, "button", n.dragButton);
        put(ev, "mods", n.dragMods);
        return ev;
    }

    // --- Drag and drop between elements (#251) ------------------------------------------

    /** The payload being dragged: its node, preview, target, scroll view to scroll. */
    private static PrismHost.Node payloadNode, payloadOver;
    private static android.widget.ImageView payloadPreview;
    private static float[] payloadAt, payloadGrab;
    private static View payloadScroller;
    private static int payloadStepX, payloadStepY;
    private static boolean payloadTicking;
    static final float PAYLOAD_EDGE = 44, PAYLOAD_STEP = 14;

    /** Locates the drop target under `c` (raw pixels), moves the preview, adds `over` to `ev`. */
    static void payload(PrismHost.Node n, String phase, float[] c, JSONObject ev) {
        boolean ending = phase.equals("end") || phase.equals("cancel");
        if (phase.equals("start") || payloadNode != n) payloadStart(n);
        if (c == null) c = payloadAt;
        if (c == null) { if (ending) payloadEnd(); return; }
        payloadAt = c;
        if (payloadPreview != null) {
            int[] decor = new int[2];
            ((View) payloadPreview.getParent()).getLocationOnScreen(decor);
            payloadPreview.setTranslationX(c[0] - payloadGrab[0] - decor[0]);
            payloadPreview.setTranslationY(c[1] - payloadGrab[1] - decor[1]);
        }
        View hit = PrismHost.root == null ? null : viewAt(PrismHost.root, c[0], c[1]);
        PrismHost.Node target = phase.equals("cancel") ? null : targetAt(n, hit);
        if (target != payloadOver && !ending) {
            if (payloadOver != null) PrismHost.state(payloadOver, "drop_hover", false);
            if (target != null) PrismHost.state(target, "drop_hover", true);
            payloadOver = target;
        }
        if (target != null) {
            int[] at = new int[2];
            target.view.getLocationOnScreen(at);
            put(ev, "over", target.id);
            put(ev, "ox", dp(c[0] - at[0] + target.view.getScrollX()));
            put(ev, "oy", dp(c[1] - at[1] + target.view.getScrollY()));
        }
        if (ending) { payloadEnd(); return; }
        autoscroll(n, hit, c);
    }

    private static void payloadStart(PrismHost.Node n) {
        payloadEnd();
        payloadNode = n;
        int[] at = new int[2];
        n.view.getLocationOnScreen(at);
        payloadGrab = new float[]{n.down[2] - at[0], n.down[3] - at[1]};
        int w = n.view.getWidth(), h = n.view.getHeight();
        if (!n.dragPreview || w <= 0 || h <= 0 || PrismHost.activity == null) return;
        // A see-through picture of the element, above everything, not touchable.
        android.graphics.Bitmap bitmap = android.graphics.Bitmap.createBitmap(w, h, android.graphics.Bitmap.Config.ARGB_8888);
        n.view.draw(new android.graphics.Canvas(bitmap));
        android.widget.ImageView preview = new android.widget.ImageView(n.view.getContext());
        preview.setImageBitmap(bitmap);
        preview.setAlpha(0.85f);
        preview.setElevation(12 * PrismHost.density);
        preview.setClickable(false);
        preview.setFocusable(false);
        preview.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS);
        ViewGroup decor = (ViewGroup) PrismHost.activity.getWindow().getDecorView();
        decor.addView(preview, new FrameLayout.LayoutParams(w, h));
        payloadPreview = preview;
    }

    private static void payloadEnd() {
        if (payloadOver != null) PrismHost.state(payloadOver, "drop_hover", false);
        if (payloadPreview != null && payloadPreview.getParent() != null) {
            ((ViewGroup) payloadPreview.getParent()).removeView(payloadPreview);
        }
        payloadNode = null;
        payloadOver = null;
        payloadPreview = null;
        payloadAt = null;
        payloadScroller = null;
    }

    /** The innermost visible Prism view under raw (`x`, `y`), children drawn last first. */
    private static View viewAt(View v, float x, float y) {
        if (v.getVisibility() != View.VISIBLE || v == payloadPreview) return null;
        android.graphics.Rect r = new android.graphics.Rect();
        if (!v.getGlobalVisibleRect(r) || !r.contains((int) x, (int) y)) return null;
        if (v instanceof ViewGroup) {
            ViewGroup g = (ViewGroup) v;
            for (int i = g.getChildCount() - 1; i >= 0; i--) {
                View hit = viewAt(g.getChildAt(i), x, y);
                if (hit != null) return hit;
            }
        }
        return v;
    }

    /** The innermost node around `hit` taking `n`'s kind, leaving out `n` and what is in it. */
    private static PrismHost.Node targetAt(PrismHost.Node n, View hit) {
        java.util.List<PrismHost.Node> chain = new java.util.ArrayList<>();
        for (View v = hit; v != null; v = v.getParent() instanceof View ? (View) v.getParent() : null) {
            PrismHost.Node node = byView.get(v);
            if (node != null && (chain.isEmpty() || chain.get(chain.size() - 1) != node)) chain.add(node);
        }
        int from = chain.indexOf(n) + 1;
        for (int i = from; i < chain.size(); i++) {
            PrismHost.Node t = chain.get(i);
            if (t.accepts != null && t.accepts.contains(n.dragKind)) return t;
        }
        return null;
    }

    /** Scrolls the scroll view whose edge the finger stays near, every frame. */
    private static void autoscroll(PrismHost.Node n, View hit, float[] c) {
        payloadScroller = null;
        float edge = PAYLOAD_EDGE * PrismHost.density, step = PAYLOAD_STEP * PrismHost.density;
        for (View v = hit; v != null; v = v.getParent() instanceof View ? (View) v.getParent() : null) {
            boolean vertical = v instanceof android.widget.ScrollView, horizontal = v instanceof android.widget.HorizontalScrollView;
            if (!vertical && !horizontal) continue;
            int[] at = new int[2];
            v.getLocationOnScreen(at);
            float from = vertical ? c[1] - at[1] : c[0] - at[0];
            float length = vertical ? v.getHeight() : v.getWidth();
            float e = Math.min(edge, length / 3);
            float speed = from < e ? -step * (1 - Math.max(0, from) / e)
                    : from > length - e ? step * (1 - Math.max(0, length - from) / e) : 0;
            if (speed == 0) continue;
            payloadScroller = v;
            payloadStepX = vertical ? 0 : Math.round(speed);
            payloadStepY = vertical ? Math.round(speed) : 0;
            break;
        }
        if (payloadScroller != null && !payloadTicking) {
            payloadTicking = true;
            PrismHost.main.postDelayed(() -> tick(n), 16);
        }
    }

    private static void tick(PrismHost.Node n) {
        payloadTicking = false;
        View s = payloadScroller;
        if (payloadNode != n || s == null || payloadAt == null) return;
        int x = s.getScrollX(), y = s.getScrollY();
        s.scrollBy(payloadStepX, payloadStepY);
        if (s.getScrollX() == x && s.getScrollY() == y) { payloadScroller = null; return; }
        // The content moved under the finger: where it is now.
        JSONObject ev = dragEvent(n, "move", payloadAt);
        payload(n, "move", payloadAt, ev);
        PrismHost.send(ev);
    }

    /** The mouse button a press is made with (fingers and pens: primary). */
    static String button(MotionEvent e) {
        if (!e.isFromSource(android.view.InputDevice.SOURCE_MOUSE)) return "primary";
        int b = e.getButtonState();
        if ((b & MotionEvent.BUTTON_TERTIARY) != 0) return "middle";
        if ((b & MotionEvent.BUTTON_SECONDARY) != 0) return "secondary";
        return "primary";
    }

    /** The pointer-mask bit that lets `button` start a drag (`drag_buttons`, #136). */
    static int dragBit(String button) {
        switch (button) {
            case "middle": return DRAG_MIDDLE;
            case "secondary": return DRAG_SECONDARY;
            default: return DRAG;
        }
    }

    /** The raw center of the touching pointers, leaving out pointer `skip` (-1: none). */
    static float[] center(MotionEvent e, int skip) {
        float x = 0, y = 0;
        int count = 0;
        for (int i = 0; i < e.getPointerCount(); i++) {
            if (i == skip) continue;
            x += e.getRawX(i);
            y += e.getRawY(i);
            count++;
        }
        return count == 0 ? new float[]{e.getRawX(), e.getRawY()} : new float[]{x / count, y / count};
    }

    /** A finger joined or left: the drag goes on from the new center without a jump. */
    static void recenter(PrismHost.Node n, float[] before, float[] after) {
        n.down[2] += after[0] - before[0];
        n.down[3] += after[1] - before[1];
    }

    /** A pinch report (#133): `{"t":"magnify","id","scale","x","y","phase"}`. */
    static void magnify(PrismHost.Node n, String phase, float scale, float x, float y) {
        JSONObject ev = PrismHost.event("magnify", n.id);
        put(ev, "phase", phase);
        put(ev, "scale", scale);
        put(ev, "x", dp(x));
        put(ev, "y", dp(y));
        PrismHost.send(ev);
    }

    /** How far the pointers are from their center, doubled (two fingers: their distance), leaving out `skip`. */
    static float span(MotionEvent e, int skip) {
        float[] c = focus(e, skip);
        float sum = 0;
        int count = 0;
        for (int i = 0; i < e.getPointerCount(); i++) {
            if (i == skip) continue;
            sum += (float) Math.hypot(e.getX(i) - c[0], e.getY(i) - c[1]);
            count++;
        }
        return count < 2 ? 0 : 2 * sum / count;
    }

    /** The pointers' center in the view, leaving out `skip`. */
    static float[] focus(MotionEvent e, int skip) {
        float x = 0, y = 0;
        int count = 0;
        for (int i = 0; i < e.getPointerCount(); i++) {
            if (i == skip) continue;
            x += e.getX(i);
            y += e.getY(i);
            count++;
        }
        return count == 0 ? new float[]{e.getX(), e.getY()} : new float[]{x / count, y / count};
    }

    /**
     * Two fingers pinch (a touchscreen, a touchpad's pinch) over a view with
     * `on_magnify` (#133): once they moved apart or together past the touch
     * slop (no minimum distance between them, unlike `ScaleGestureDetector`),
     * the scale since the previous report, around their center. The view's
     * taps stop when the pinch starts.
     */
    static void pinch(PrismHost.Node n, MotionEvent e, android.view.GestureDetector g) {
        int a = e.getActionMasked();
        int count = e.getPointerCount();
        if (a == MotionEvent.ACTION_UP || a == MotionEvent.ACTION_CANCEL
                || (a == MotionEvent.ACTION_POINTER_UP && count == 2)) {
            if (n.scaling) {
                n.scaling = false;
                float[] f = focus(e, -1);
                magnify(n, "end", 1f, f[0], f[1]);
            }
            n.pinchSpan = 0;
            return;
        }
        if (a == MotionEvent.ACTION_POINTER_DOWN || a == MotionEvent.ACTION_POINTER_UP) {
            // The fingers changed: measure from the ones that stay.
            n.pinchSpan = span(e, a == MotionEvent.ACTION_POINTER_UP ? e.getActionIndex() : -1);
            if (!n.scaling) n.pinchFrom = n.pinchSpan;
            return;
        }
        if (a != MotionEvent.ACTION_MOVE || count < 2 || n.pinchSpan <= 0) return;
        float span = span(e, -1);
        float[] f = focus(e, -1);
        if (!n.scaling) {
            int slop = ViewConfiguration.get(n.view.getContext()).getScaledTouchSlop();
            if (Math.abs(span - n.pinchFrom) <= slop) return;
            n.scaling = true;
            if (n.view.getParent() != null) n.view.getParent().requestDisallowInterceptTouchEvent(true);
            // A pinch, not a tap or a long press.
            MotionEvent cancel = MotionEvent.obtain(e);
            cancel.setAction(MotionEvent.ACTION_CANCEL);
            g.onTouchEvent(cancel);
            cancel.recycle();
            magnify(n, "start", 1f, f[0], f[1]);
            n.pinchSpan = n.pinchFrom;
        }
        if (span <= 0) return;
        magnify(n, "move", Math.max(0.01f, span / n.pinchSpan), f[0], f[1]);
        n.pinchSpan = span;
    }

    /**
     * Mouse wheels and touchpad scrolling over a view with `on_wheel` (#133),
     * in points like the web's deltas: positive y scrolls toward the end.
     */
    static void wheel(PrismHost.Node n) {
        View v = n.view;
        if ((n.pointerMask & WHEEL) == 0) {
            v.setOnGenericMotionListener(null);
            return;
        }
        v.setOnGenericMotionListener((view, e) -> {
            if (e.getActionMasked() != MotionEvent.ACTION_SCROLL) return false;
            ViewConfiguration c = ViewConfiguration.get(view.getContext());
            float dx = e.getAxisValue(MotionEvent.AXIS_HSCROLL) * c.getScaledHorizontalScrollFactor();
            float dy = -e.getAxisValue(MotionEvent.AXIS_VSCROLL) * c.getScaledVerticalScrollFactor();
            if (dx == 0 && dy == 0) return false;
            JSONObject ev = PrismHost.event("wheel", n.id);
            put(ev, "dx", dp(dx));
            put(ev, "dy", dp(dy));
            put(ev, "x", dp(e.getX()));
            put(ev, "y", dp(e.getY()));
            put(ev, "mods", mods(e.getMetaState()));
            // A touchpad's fingers scroll by points, a mouse wheel by notches.
            put(ev, "precise", e.getToolType(0) == MotionEvent.TOOL_TYPE_FINGER);
            PrismHost.send(ev);
            return true;
        });
    }

    /** Hover (mouse, stylus): enter, move, leave. */
    // --- Drop targets (#111) --------------------------------------------------------------

    /**
     * Files dragged from another app (a split screen, a desktop window) onto
     * the node: `drop_hover` while accepted ones are over it, then their
     * copies in the cache (like picked files) as a `drop` event. `types`:
     * MIME types or extensions (empty: any file), null: not a drop target.
     */
    static void setDropTarget(PrismHost.Node n, JSONArray types) {
        if (types == null) {
            n.view.setOnDragListener(null);
            return;
        }
        n.view.setOnDragListener((v, e) -> {
            switch (e.getAction()) {
                case android.view.DragEvent.ACTION_DRAG_STARTED:
                    return e.getClipDescription() != null && offers(e.getClipDescription(), types);
                case android.view.DragEvent.ACTION_DRAG_ENTERED:
                    PrismHost.state(n, "drop_hover", true);
                    return true;
                case android.view.DragEvent.ACTION_DRAG_EXITED:
                case android.view.DragEvent.ACTION_DRAG_ENDED:
                    PrismHost.state(n, "drop_hover", false);
                    return true;
                case android.view.DragEvent.ACTION_DROP:
                    PrismHost.state(n, "drop_hover", false);
                    drop(n, e, types);
                    return true;
                default:
                    return true;
            }
        });
    }

    /** Whether the drag may hold files `types` accept (extensions are checked on drop). */
    private static boolean offers(android.content.ClipDescription d, JSONArray types) {
        if (types.length() == 0) return d.getMimeTypeCount() > 0;
        for (int i = 0; i < types.length(); i++) {
            String t = types.optString(i).trim().toLowerCase();
            if (t.startsWith(".") || d.hasMimeType(t)) return true;
        }
        return false;
    }

    /** `PickFiles::accepts`: MIME types (`image/*`) or extensions (`.pdf`). */
    static boolean accepts(JSONArray types, String name, String mime) {
        if (types.length() == 0) return true;
        String m = mime == null ? "" : mime.toLowerCase();
        for (int i = 0; i < types.length(); i++) {
            String t = types.optString(i).trim().toLowerCase();
            if (t.startsWith(".") ? name.toLowerCase().endsWith(t)
                    : t.endsWith("/*") ? m.startsWith(t.substring(0, t.length() - 1)) : m.equals(t)) return true;
        }
        return false;
    }

    /** Copies the dropped files (off the main thread), then reports them. */
    private static void drop(PrismHost.Node n, android.view.DragEvent e, JSONArray types) {
        android.content.ClipData clip = e.getClipData();
        if (clip == null) return;
        // Reading another app's content needs the drop's permission.
        android.view.DragAndDropPermissions grant = PrismHost.activity == null ? null
                : PrismHost.activity.requestDragAndDropPermissions(e);
        java.util.List<android.net.Uri> uris = new java.util.ArrayList<>();
        for (int i = 0; i < clip.getItemCount(); i++) {
            android.net.Uri u = clip.getItemAt(i).getUri();
            if (u != null) uris.add(u);
        }
        new Thread(() -> {
            JSONArray files = new JSONArray();
            int unread = 0, refused = 0;
            for (android.net.Uri u : uris) {
                JSONObject f = PrismDevice.copy(u, true);
                if (f == null) unread++;
                else if (accepts(types, f.optString("name"), f.optString("mime"))) files.put(f);
                else refused++;
            }
            if (grant != null) grant.release();
            // What did not reach the app, and why (#290).
            String error = null, reason = null;
            if (unread > 0) {
                error = "failed";
                reason = unread + " of " + uris.size() + " dropped files could not be read";
            } else if (files.length() == 0 && refused > 0) {
                error = "not_accepted";
                reason = "no dropped file is of a type the target takes";
            }
            final String err = error, why = reason;
            if (files.length() == 0 && err == null) return;
            PrismHost.main.post(() -> {
                if (files.length() > 0) {
                    JSONObject ev = PrismHost.event("drop", n.id);
                    try { ev.put("files", files); } catch (JSONException ignored) {}
                    PrismHost.send(ev);
                }
                if (err != null) {
                    JSONObject ev = PrismHost.event("drop_error", n.id);
                    try { ev.put("error", err); ev.put("reason", why); } catch (JSONException ignored) {}
                    PrismHost.send(ev);
                }
            });
        }, "prism-drop").start();
    }

    static void hover(PrismHost.Node n) {
        // Shared with the hover state (`.hover(|s| …)`): one listener.
        PrismHost.installHover(n);
        wheel(n);
    }

    /**
     * Touches of a view with gestures (`g`: taps, long presses, flings): moves
     * while touching, and drags past the touch slop. A drag cancels the tap.
     */
    static boolean touch(PrismHost.Node n, android.view.GestureDetector g, MotionEvent e) {
        int a = e.getActionMasked();
        if ((n.pointerMask & MAGNIFY) != 0) pinch(n, e, g);
        if ((n.pointerMask & MOVE) != 0 && (a == MotionEvent.ACTION_DOWN || a == MotionEvent.ACTION_MOVE)) {
            pointer(n, "move", e.getX(), e.getY());
        }
        if (a == MotionEvent.ACTION_DOWN) {
            n.dragButton = button(e);
            n.dragMods = mods(e.getMetaState());
        }
        if ((n.pointerMask & dragBit(n.dragButton)) != 0) {
            switch (a) {
                case MotionEvent.ACTION_DOWN:
                    n.down = new float[]{e.getX(), e.getY(), e.getRawX(), e.getRawY()};
                    n.dragging = false;
                    // A finger or a pen on a node that drags after a long
                    // press (#243): held still, it lifts; a swipe before
                    // is the scroll view's. A mouse drags on movement.
                    unhold(n);
                    n.holdPress = n.dragDelay > 0 && e.getToolType(0) != MotionEvent.TOOL_TYPE_MOUSE;
                    if (n.holdPress) {
                        long downTime = e.getDownTime();
                        n.lift = () -> {
                            n.lift = null;
                            n.dragging = true;
                            if (n.view.getParent() != null) n.view.getParent().requestDisallowInterceptTouchEvent(true);
                            n.view.performHapticFeedback(android.view.HapticFeedbackConstants.LONG_PRESS);
                            // No tap or long press after this.
                            MotionEvent cancel = MotionEvent.obtain(downTime, android.os.SystemClock.uptimeMillis(),
                                MotionEvent.ACTION_CANCEL, n.down[0], n.down[1], 0);
                            g.onTouchEvent(cancel);
                            cancel.recycle();
                            drag(n, "start", null);
                        };
                        n.view.postDelayed(n.lift, n.dragDelay);
                    }
                    break;
                case MotionEvent.ACTION_POINTER_DOWN:
                    recenter(n, center(e, e.getActionIndex()), center(e, -1));
                    break;
                case MotionEvent.ACTION_POINTER_UP:
                    recenter(n, center(e, -1), center(e, e.getActionIndex()));
                    break;
                case MotionEvent.ACTION_MOVE: {
                    float[] c = center(e, -1);
                    float dx = c[0] - n.down[2], dy = c[1] - n.down[3];
                    int slop = ViewConfiguration.get(n.view.getContext()).getScaledTouchSlop();
                    if (n.holdPress && !n.dragging) {
                        // Moved before the hold: a scroll, not a drag.
                        if (dx * dx + dy * dy > slop * slop) unhold(n);
                    } else if (!n.dragging && dx * dx + dy * dy > slop * slop) {
                        n.dragging = true;
                        if (n.view.getParent() != null) n.view.getParent().requestDisallowInterceptTouchEvent(true);
                        // No tap or long press after this.
                        MotionEvent cancel = MotionEvent.obtain(e);
                        cancel.setAction(MotionEvent.ACTION_CANCEL);
                        g.onTouchEvent(cancel);
                        cancel.recycle();
                        drag(n, "start", e);
                    } else if (n.dragging) {
                        drag(n, "move", e);
                    }
                    break;
                }
                case MotionEvent.ACTION_UP:
                    unhold(n);
                    if (n.dragging) {
                        n.dragging = false;
                        drag(n, "end", e);
                        return true;
                    }
                    break;
                case MotionEvent.ACTION_CANCEL:
                    unhold(n);
                    if (n.dragging) {
                        n.dragging = false;
                        drag(n, "cancel", e);
                    }
                    break;
            }
            if (n.dragging) return true;
        }
        if (n.scaling) return true;
        return g.onTouchEvent(e) || n.pointerMask != 0;
    }

    /** The pending lift of a long-press drag (#243), if any, will not happen. */
    static void unhold(PrismHost.Node n) {
        if (n.lift != null) n.view.removeCallbacks(n.lift);
        n.lift = null;
    }

    /** A tap's position goes with its click. */
    static void click(PrismHost.Node n, MotionEvent e) {
        JSONObject ev = PrismHost.event("click", n.id);
        put(ev, "x", dp(e.getX()));
        put(ev, "y", dp(e.getY()));
        // The keys held (a keyboard, a mouse) and the tap's number in a quick
        // series, with the system's double-tap timeout and slop (#159).
        put(ev, "mods", mods(e.getMetaState()));
        int slop = ViewConfiguration.get(n.view.getContext()).getScaledDoubleTapSlop();
        boolean series = n.tapCount > 0
            && e.getEventTime() - n.tapTime <= ViewConfiguration.getDoubleTapTimeout()
            && Math.hypot(e.getRawX() - n.tapX, e.getRawY() - n.tapY) <= slop;
        n.tapCount = series ? n.tapCount + 1 : 1;
        n.tapTime = e.getEventTime();
        n.tapX = e.getRawX();
        n.tapY = e.getRawY();
        put(ev, "n", n.tapCount);
        PrismHost.send(ev);
    }

    // --- Keys -----------------------------------------------------------------------------

    static void keys(PrismHost.Node n, boolean local, boolean global) {
        n.keysLocal = local;
        n.keysGlobal = global;
        if (local) {
            // Takes the focus (D-pad, keyboard, cx.focus) without a first tap
            // only focusing it: touches go through Prism's gesture listener.
            n.view.setFocusable(true);
            n.view.setFocusableInTouchMode(true);
            // The focus ring, and the focus state if the app styles it.
            n.focusRing = true;
            PrismHost.installFocus(n);
        }
    }

    /** The web's `KeyboardEvent.key` name, or null for a lone modifier. */
    static String keyName(KeyEvent e) {
        switch (e.getKeyCode()) {
            case KeyEvent.KEYCODE_DPAD_UP: return "ArrowUp";
            case KeyEvent.KEYCODE_DPAD_DOWN: return "ArrowDown";
            case KeyEvent.KEYCODE_DPAD_LEFT: return "ArrowLeft";
            case KeyEvent.KEYCODE_DPAD_RIGHT: return "ArrowRight";
            case KeyEvent.KEYCODE_DPAD_CENTER: case KeyEvent.KEYCODE_ENTER: case KeyEvent.KEYCODE_NUMPAD_ENTER: return "Enter";
            case KeyEvent.KEYCODE_ESCAPE: return "Escape";
            case KeyEvent.KEYCODE_TAB: return "Tab";
            case KeyEvent.KEYCODE_DEL: return "Backspace";
            case KeyEvent.KEYCODE_FORWARD_DEL: return "Delete";
            case KeyEvent.KEYCODE_MOVE_HOME: return "Home";
            case KeyEvent.KEYCODE_MOVE_END: return "End";
            case KeyEvent.KEYCODE_PAGE_UP: return "PageUp";
            case KeyEvent.KEYCODE_PAGE_DOWN: return "PageDown";
            case KeyEvent.KEYCODE_SPACE: return " ";
            case KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE: return "MediaPlayPause";
            case KeyEvent.KEYCODE_MENU: return "ContextMenu";
        }
        int code = e.getKeyCode();
        if (code >= KeyEvent.KEYCODE_F1 && code <= KeyEvent.KEYCODE_F12) return "F" + (code - KeyEvent.KEYCODE_F1 + 1);
        int c = e.getUnicodeChar(e.getMetaState() & ~(KeyEvent.META_CTRL_MASK | KeyEvent.META_META_MASK));
        if (c <= 0 || Character.isISOControl(c)) return null;
        return new String(Character.toChars(c));
    }

    static int mods(KeyEvent e) {
        return mods(e.getMetaState());
    }

    /** Prism's modifier bits (shift 1, control 2, alt 4, meta 8). */
    static int mods(int meta) {
        return ((meta & KeyEvent.META_SHIFT_ON) != 0 ? 1 : 0) | ((meta & KeyEvent.META_CTRL_ON) != 0 ? 2 : 0)
            | ((meta & KeyEvent.META_ALT_ON) != 0 ? 4 : 0) | ((meta & KeyEvent.META_META_ON) != 0 ? 8 : 0);
    }

    /** The node of `v` or of its nearest Prism ancestor. */
    static PrismHost.Node nodeOf(View v) {
        while (v != null) {
            PrismHost.Node n = byView.get(v);
            if (n != null) return n;
            v = v.getParent() instanceof View ? (View) v.getParent() : null;
        }
        return null;
    }

    /**
     * A key press anywhere (the Activity, a dialog, a popover): reported when
     * the focus is in a node listening to keys, or some node listens to every
     * key. Returns whether it was consumed (arrows a focused pad handles do
     * not also move the focus).
     */
    static boolean key(KeyEvent e, View focused) {
        if (e.getAction() == KeyEvent.ACTION_UP) {
            keyUp(e);
            // A consumed key's release stays the app's too (a text field
            // acts on Enter's release).
            String up = keyName(e);
            return up != null && CONSUMED_DOWN.remove(up);
        }
        if (e.getAction() != KeyEvent.ACTION_DOWN) return false;
        String name = keyName(e);
        if (name == null) return false;
        PrismHost.Node target = nodeOf(focused);
        boolean local = false;
        for (PrismHost.Node a = target; a != null; a = PrismHost.parentOf(a)) {
            if (a.keysLocal) { local = true; break; }
        }
        boolean global = false;
        for (PrismHost.Node n : PrismHost.nodes.values()) {
            if (n.keysGlobal) { global = true; break; }
        }
        // The app's alone (`consume_keys`, #181): decided as pressed, before
        // its handler may close the menu that declared it.
        // Where a field's caret is (#211), for `consume_key_at`.
        String edges = focused instanceof EditText ? edges((EditText) focused) : null;
        boolean consumed = consumes(target, name, edges);
        boolean typing = focused instanceof EditText && name.length() <= 2 && (mods(e) & (2 | 8)) == 0;
        // Typing in a field reaches only that field's own handlers.
        if (typing && !local) return false;
        if (!local && !global) return false;
        JSONObject ev = PrismHost.event("key", target != null ? target.id : 0);
        put(ev, "key", name);
        put(ev, "mods", mods(e));
        put(ev, "repeat", e.getRepeatCount() > 0);
        // In a text field: its selection before the key (#86).
        if (focused instanceof EditText) {
            int[] sel = selection((EditText) focused);
            if (sel != null) put(ev, "sel", new JSONArray().put(sel[0]).put(sel[1]));
            if (edges != null) put(ev, "edges", edges);
            // A selection group's keys (#231): ⇧+arrows at the field's
            // edge, and arrows, edits and copies of a selection across its
            // fields. Asked with the key as it is sent, before it is.
            if (target != null && PrismHost.nativeConsumesKey(ev.toString())) consumed = true;
        }
        PrismHost.send(ev);
        HELD.put(name, target != null ? target.id : 0);
        if (consumed) {
            CONSUMED_DOWN.add(name);
            return true;
        }
        boolean arrow = name.startsWith("Arrow");
        return local && arrow && !(focused instanceof EditText);
    }

    /**
     * Backspace from a soft keyboard in field `e` with the caret at its start
     * (#294): it deletes nothing, so it is reported like a hardware press.
     */
    static void backspaceAtStart(EditText e) {
        if (e.getSelectionStart() != 0 || e.getSelectionEnd() != 0 || HELD.containsKey("Backspace")) return;
        long now = android.os.SystemClock.uptimeMillis();
        key(new KeyEvent(now, now, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DEL, 0), e);
        key(new KeyEvent(now, now, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DEL, 0), e);
    }

    /** Keys whose press was consumed, so their release is too (#181). */
    static final java.util.Set<String> CONSUMED_DOWN = new java.util.HashSet<>();

    /** `consume_keys` of a node (#181): null or empty, none. */
    static void consume(PrismHost.Node n, JSONArray keys) {
        if (keys == null || keys.length() == 0) {
            n.consumeKeys = null;
            return;
        }
        n.consumeKeys = new java.util.HashSet<>();
        for (int i = 0; i < keys.length(); i++) n.consumeKeys.add(keys.optString(i));
    }

    /**
     * Whether `key` pressed in `n` is taken by it or an ancestor (#181).
     * `ArrowUp@first-line` (`consume_key_at`, #211) only while the focused
     * field's caret is there (`edges`; null: no field, taken).
     */
    static boolean consumes(PrismHost.Node n, String key, String edges) {
        for (PrismHost.Node a = n; a != null; a = PrismHost.parentOf(a)) {
            if (a.consumeKeys == null) continue;
            if (a.consumeKeys.contains(key)) return true;
            for (String k : a.consumeKeys) {
                int at = k.indexOf('@');
                if (at <= 0 || !k.substring(0, at).equals(key)) continue;
                String edge = k.substring(at + 1);
                String letter = edge.equals("start") ? "s" : edge.equals("end") ? "e"
                    : edge.equals("first-line") ? "f" : edge.equals("last-line") ? "l" : null;
                if (letter != null && (edges == null || edges.contains(letter))) return true;
            }
        }
        return false;
    }

    /**
     * Where a field's caret is (#211): "s" start, "e" end, "f" first visual
     * line, "l" last (the layout's lines, wrapping included); "" while text
     * is selected.
     */
    static String edges(EditText f) {
        int a = f.getSelectionStart(), b = f.getSelectionEnd();
        if (a < 0 || b < 0) return null;
        if (a != b) return "";
        int len = f.length();
        StringBuilder out = new StringBuilder();
        if (a == 0) out.append('s');
        if (a >= len) out.append('e');
        android.text.Layout layout = f.getLayout();
        int line = layout != null ? layout.getLineForOffset(a) : 0;
        int lines = layout != null ? layout.getLineCount() : 1;
        if (line == 0) out.append('f');
        if (line >= lines - 1) out.append('l');
        return out.toString();
    }

    /** Keys whose press was reported, and the node it went to (0: none), for their release (#159). */
    static final Map<String, Long> HELD = new java.util.LinkedHashMap<>();

    /** A key released: reported when its press was, to the same node (#159). */
    static void keyUp(KeyEvent e) {
        String name = keyName(e);
        if (name == null) return;
        Long id = HELD.remove(name);
        if (id == null) return;
        JSONObject ev = PrismHost.event("keyup", id);
        put(ev, "key", name);
        put(ev, "mods", mods(e));
        PrismHost.send(ev);
    }

    /** The window lost the focus: the keys still held get no release, so they are released now (#159). */
    static void releaseKeys() {
        for (Map.Entry<String, Long> k : HELD.entrySet()) {
            JSONObject ev = PrismHost.event("keyup", k.getValue());
            put(ev, "key", k.getKey());
            put(ev, "mods", 0);
            PrismHost.send(ev);
        }
        HELD.clear();
    }

    // --- Text field selections (#86): Prism counts characters (code points) -------------

    /** Characters of `t` before UTF-16 offset `units`. */
    static int chars(CharSequence t, int units) {
        return Character.codePointCount(t, 0, Math.max(0, Math.min(units, t.length())));
    }

    /** The UTF-16 offset of character `chars` in `t` (clamped to its end). */
    static int units(CharSequence t, int chars) {
        int n = Character.codePointCount(t, 0, t.length());
        return Character.offsetByCodePoints(t, 0, Math.max(0, Math.min(chars, n)));
    }

    /** A field's selection in characters, ordered; null without one. */
    static int[] selection(EditText e) {
        int a = e.getSelectionStart(), b = e.getSelectionEnd();
        if (a < 0 || b < 0) return null;
        CharSequence t = e.getText();
        return new int[]{chars(t, Math.min(a, b)), chars(t, Math.max(a, b))};
    }

    /**
     * A field's selection moved (`onSelectionChanged`): reported once the
     * edit is done (posted, so after its `input`), once per turn, and only
     * when it moved.
     */
    static void selectionChanged(EditText e) {
        PrismHost.Node n = byView == null ? null : byView.get(e);
        if (n == null || !n.selectEvents || n.selPosted) return;
        n.selPosted = true;
        e.post(() -> {
            n.selPosted = false;
            if (!n.selectEvents) return;
            int[] s = selection(e);
            if (s == null || java.util.Arrays.equals(s, n.lastSel)) return;
            n.lastSel = s;
            JSONObject ev = PrismHost.event("select", n.id);
            put(ev, "start", s[0]);
            put(ev, "end", s[1]);
            PrismHost.send(ev);
        });
    }

    // --- A rich input's editing hooks (#217) -----------------------------------------------

    /** Op `editev`: the editing events the field reports (prism_core::edit_events bits). */
    static void editEvents(PrismHost.Node n, int mask) {
        n.editMask = mask;
        n.lastRect = null;
        if ((mask & 1) != 0 && n.view instanceof EditText) rectChanged((EditText) n.view);
    }

    /** Copy, cut or paste from the field's menu: the app's when it has the hook. */
    static boolean menuItem(EditText e, int item) {
        PrismHost.Node n = byView.get(e);
        if (n == null) return false;
        boolean cut = item == android.R.id.cut;
        if (item == android.R.id.copy || cut) {
            if ((n.editMask & (cut ? 4 : 2)) == 0) return false;
            int a = Math.min(e.getSelectionStart(), e.getSelectionEnd());
            int b = Math.max(e.getSelectionStart(), e.getSelectionEnd());
            if (a < 0 || a == b) return false;
            CharSequence t = e.getText();
            JSONObject ev = PrismHost.event("copy", n.id);
            put(ev, "cut", cut);
            put(ev, "start", chars(t, a));
            put(ev, "end", chars(t, b));
            put(ev, "text", t.subSequence(a, b).toString());
            PrismHost.send(ev);
            if (cut) e.getText().delete(a, b);
            return true;
        }
        if (item == android.R.id.paste || item == android.R.id.pasteAsPlainText) {
            if ((n.editMask & 8) == 0) return false;
            int[] s = selection(e);
            JSONObject ev = PrismHost.event("paste", n.id);
            put(ev, "start", s == null ? 0 : s[0]);
            put(ev, "end", s == null ? 0 : s[1]);
            android.content.ClipboardManager cb = (android.content.ClipboardManager) e.getContext().getSystemService(Context.CLIPBOARD_SERVICE);
            android.content.ClipData clip = cb == null ? null : cb.getPrimaryClip();
            JSONArray types = new JSONArray();
            JSONArray data = new JSONArray();
            String text = "", html = null;
            if (clip != null) {
                for (int i = 0; i < clip.getDescription().getMimeTypeCount(); i++) types.put(clip.getDescription().getMimeType(i));
                if (clip.getItemCount() > 0) {
                    android.content.ClipData.Item it = clip.getItemAt(0);
                    CharSequence c = it.coerceToText(e.getContext());
                    text = c == null ? "" : c.toString();
                    html = it.getHtmlText();
                }
                data = clipData(clip);
            }
            put(ev, "text", text);
            if (html != null) put(ev, "html", html);
            put(ev, "types", types);
            put(ev, "data", data);
            PrismHost.send(ev);
            return true;
        }
        return false;
    }

    /**
     * Where a clip keeps the text of a type other than plain text and HTML
     * (Markdown, the app's own `ClipboardContent::custom` types): its
     * description lists the type, its extras hold the text under this
     * prefix and the type. The clipboard service keeps both.
     */
    static final String CLIP_TYPE = "dev.prism.clip:";

    /** Op `clip`: what a copy writes: plain text, HTML, and every other type as text. */
    static void writeClipboard(JSONArray items) {
        if (items == null) return;
        String text = "", html = null;
        java.util.List<String> others = new java.util.ArrayList<>();
        android.os.PersistableBundle extras = new android.os.PersistableBundle();
        for (int i = 0; i < items.length(); i++) {
            JSONArray pair = items.optJSONArray(i);
            if (pair == null) continue;
            String type = pair.optString(0);
            if ("text/plain".equals(type)) text = pair.optString(1);
            else if ("text/html".equals(type)) html = pair.optString(1);
            else if (!type.isEmpty() && !others.contains(type)) {
                others.add(type);
                extras.putString(CLIP_TYPE + type, pair.optString(1));
            }
        }
        android.content.ClipboardManager cb = (android.content.ClipboardManager) PrismHost.themed().getSystemService(Context.CLIPBOARD_SERVICE);
        if (cb == null) return;
        android.content.ClipData clip = html != null
                ? android.content.ClipData.newHtmlText("text", text, html)
                : android.content.ClipData.newPlainText("text", text);
        if (!others.isEmpty()) {
            android.content.ClipDescription d = clip.getDescription();
            String[] all = new String[d.getMimeTypeCount() + others.size()];
            for (int i = 0; i < d.getMimeTypeCount(); i++) all[i] = d.getMimeType(i);
            for (int i = 0; i < others.size(); i++) all[d.getMimeTypeCount() + i] = others.get(i);
            android.content.ClipDescription withTypes = new android.content.ClipDescription("text", all);
            withTypes.setExtras(extras);
            clip = new android.content.ClipData(withTypes, clip.getItemAt(0));
        }
        cb.setPrimaryClip(clip);
    }

    /**
     * The text of a clip's types other than plain text and HTML, as
     * `[[type, text]…]`: what a Prism copy put in its extras, else for a
     * `text/…` type (Markdown from another app), the item's text.
     */
    static JSONArray clipData(android.content.ClipData clip) {
        JSONArray out = new JSONArray();
        android.content.ClipDescription d = clip.getDescription();
        android.os.PersistableBundle extras = d.getExtras();
        CharSequence itemText = clip.getItemCount() > 0 ? clip.getItemAt(0).getText() : null;
        java.util.Set<String> seen = new java.util.HashSet<>();
        for (int i = 0; i < d.getMimeTypeCount(); i++) {
            String type = d.getMimeType(i);
            if (type == null || !seen.add(type) || type.startsWith("text/plain") || type.equals("text/html")
                    || type.equals("text/uri-list") || type.equals(android.content.ClipDescription.MIMETYPE_TEXT_INTENT)) continue;
            String value = extras == null ? null : extras.getString(CLIP_TYPE + type);
            if (value == null && type.startsWith("text/") && itemText != null) value = itemText.toString();
            if (value == null) continue;
            JSONArray pair = new JSONArray();
            pair.put(type);
            pair.put(value);
            out.put(pair);
        }
        return out;
    }

    /** Op `instext`: a paste the app left to the field, over its selection. */
    static void insertText(PrismHost.Node n, String text) {
        if (!(n.view instanceof EditText)) return;
        EditText e = (EditText) n.view;
        // After the ops: the field then reports the edit.
        e.post(() -> {
            int a = Math.max(0, Math.min(e.getSelectionStart(), e.getSelectionEnd()));
            int b = Math.max(0, Math.max(e.getSelectionStart(), e.getSelectionEnd()));
            e.getText().replace(a, b, text);
        });
    }

    /** The field's caret or selection moved: its rectangle (points, in the field), once per turn. */
    static void rectChanged(EditText e) {
        PrismHost.Node n = byView == null ? null : byView.get(e);
        if (n == null || (n.editMask & 1) == 0 || n.rectPosted) return;
        n.rectPosted = true;
        e.post(() -> {
            n.rectPosted = false;
            android.text.Layout l = e.getLayout();
            int[] s = selection(e);
            if ((n.editMask & 1) == 0 || l == null || s == null) return;
            int a = Math.max(0, Math.min(e.getSelectionStart(), e.getSelectionEnd()));
            int b = Math.max(0, Math.max(e.getSelectionStart(), e.getSelectionEnd()));
            int la = l.getLineForOffset(a), lb = l.getLineForOffset(b);
            float dx = e.getTotalPaddingLeft() - e.getScrollX(), dy = e.getTotalPaddingTop() - e.getScrollY();
            float x0, x1;
            if (la == lb) {
                x0 = l.getPrimaryHorizontal(a);
                x1 = a == b ? x0 + PrismHost.density : l.getPrimaryHorizontal(b);
            } else {
                x0 = 0;
                x1 = l.getWidth();
            }
            float d = PrismHost.density;
            float x = (Math.min(x0, x1) + dx) / d, y = (l.getLineTop(la) + dy) / d;
            float w = Math.max(1, Math.abs(x1 - x0) / d), h = (l.getLineBottom(lb) - l.getLineTop(la)) / d;
            String key = s[0] + "," + s[1] + "," + Math.round(x * 2) + "," + Math.round(y * 2) + "," + Math.round(w * 2) + "," + Math.round(h * 2);
            if (key.equals(n.lastRect)) return;
            n.lastRect = key;
            JSONObject ev = PrismHost.event("selrect", n.id);
            put(ev, "start", s[0]);
            put(ev, "end", s[1]);
            put(ev, "x", x);
            put(ev, "y", y);
            put(ev, "w", w);
            put(ev, "h", h);
            PrismHost.send(ev);
        });
    }

    /** Op `selev`: whether the field reports its selection. */
    static void selectEvents(PrismHost.Node n, boolean on) {
        n.selectEvents = on;
        if (!on) n.lastSel = null;
    }

    /** Op `sel`: the app's selection, in characters (clamped). */
    static void select(PrismHost.Node n, int start, int end) {
        if (!(n.view instanceof EditText)) return;
        EditText e = (EditText) n.view;
        CharSequence t = e.getText();
        e.setSelection(units(t, start), units(t, Math.max(start, end)));
    }

    // --- Focus, scrolling ------------------------------------------------------------------

    static void focus(PrismHost.Node n) {
        View v = n.view;
        if (v instanceof EditText || n.keysLocal) v.setFocusableInTouchMode(true);
        v.requestFocus();
        if (v instanceof EditText) {
            v.post(() -> {
                InputMethodManager imm = (InputMethodManager) v.getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
                if (imm != null) imm.showSoftInput(v, InputMethodManager.SHOW_IMPLICIT);
            });
        }
    }

    /** `cx.dismiss_keyboard()` (#29): the focused view lets go and the IME hides. */
    static void dismissKeyboard() {
        android.app.Activity a = PrismHost.activity;
        View focused = a != null ? a.getCurrentFocus() : null;
        View token = focused != null ? focused : PrismHost.root;
        if (token == null) return;
        InputMethodManager imm = (InputMethodManager) token.getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
        if (imm != null) imm.hideSoftInputFromWindow(token.getWindowToken(), 0);
        if (focused != null) focused.clearFocus();
    }

    /** Scroll views around the view bring it on screen (smoothly). */
    static void reveal(PrismHost.Node n) {
        View v = n.view;
        v.post(() -> v.requestRectangleOnScreen(new Rect(0, 0, v.getWidth(), v.getHeight()), false));
    }

    /** `cx.scroll_by` / `cx.scroll_to` (#193): `x`, `y` already in pixels (the renderer zooms
     *  them by the density, #308); the view keeps it within its content. */
    static void scrollTo(PrismHost.Node n, float x, float y, boolean relative, boolean animated) {
        View v = n.view;
        int px = (int) Math.max(-1e8f, Math.min(1e8f, x)), py = (int) Math.max(-1e8f, Math.min(1e8f, y));
        v.post(() -> {
            if (v instanceof android.widget.ScrollView) {
                android.widget.ScrollView s = (android.widget.ScrollView) v;
                if (relative) { if (animated) s.smoothScrollBy(0, py); else s.scrollBy(0, py); }
                else if (animated) s.smoothScrollTo(0, py); else s.scrollTo(0, py);
            } else if (v instanceof android.widget.HorizontalScrollView) {
                android.widget.HorizontalScrollView s = (android.widget.HorizontalScrollView) v;
                if (relative) { if (animated) s.smoothScrollBy(px, 0); else s.scrollBy(px, 0); }
                else if (animated) s.smoothScrollTo(px, 0); else s.scrollTo(px, 0);
            }
        });
    }

    // --- Hit testing -----------------------------------------------------------------------

    /** `hit: false` and `inert: true` from the style op. */
    static void hitTest(PrismHost.Node n) {
        boolean noHit = n.style.has("hit") && !n.style.optBoolean("hit", true);
        boolean inert = n.style.optBoolean("inert", false);
        boolean was = n.noHit || n.inert;
        n.noHit = noHit;
        n.inert = inert;
        View v = n.view;
        if (v instanceof PrismLayout) {
            ((PrismLayout) v).passThrough = noHit || inert;
            ((ViewGroup) v).setDescendantFocusability(inert ? ViewGroup.FOCUS_BLOCK_DESCENDANTS : ViewGroup.FOCUS_BEFORE_DESCENDANTS);
        } else if (noHit || inert) {
            // A leaf lets touches through when it does not take them.
            v.setClickable(false);
            v.setLongClickable(false);
            v.setOnTouchListener(null);
            n.touchInstalled = false;
        } else if (was) {
            switch (n.kind) {
                case "button": case "toggle": case "checkbox": case "text_input": case "text_area":
                case "date_picker": case "slider": case "picker": v.setClickable(true); break;
                default: PrismHost.setClickable(n);
            }
        }
        if (inert) {
            v.setFocusable(false);
            v.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS);
        } else if (was) {
            v.setImportantForAccessibility(n.a11yHidden
                    ? View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS : PrismHost.shownImportance(n));
            if (n.keysLocal || n.clickable) v.setFocusable(true);
        }
    }

    // --- Layers ----------------------------------------------------------------------------

    static FrameLayout layers;

    /**
     * Runtime layers (the toaster) above the root, in the same area. They
     * are not clickable where they have no child: touches reach the app.
     * (Dialogs are other windows: layers stay under an open overlay.)
     */
    static void setLayers(JSONArray ids) {
        ViewGroup outer = PrismHost.root == null ? null : (ViewGroup) PrismHost.root.getParent();
        if (outer == null) return;
        if (layers == null) {
            layers = new FrameLayout(PrismHost.activity);
            layers.setClipChildren(false);
        }
        if (layers.getParent() != outer) {
            if (layers.getParent() != null) ((ViewGroup) layers.getParent()).removeView(layers);
            outer.addView(layers, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        }
        layers.removeAllViews();
        for (int i = 0; ids != null && i < ids.length(); i++) {
            PrismHost.Node n = PrismHost.nodes.get(ids.optLong(i));
            if (n == null) continue;
            ViewGroup old = (ViewGroup) n.view.getParent();
            if (old != null) old.removeView(n.view);
            PrismLayout host = new PrismLayout(PrismHost.activity);
            host.addView(n.view, PrismLayout.frameOf(n.view));
            layers.addView(host, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        }
    }

    // --- Overlays --------------------------------------------------------------------------

    /** The content's container: keys typed inside a popover reach Prism too. */
    static PrismLayout overlayHost(PrismHost.Node n) {
        PrismLayout host = new PrismLayout(PrismHost.activity) {
            @Override public boolean dispatchKeyEvent(KeyEvent e) {
                if (n.popup != null && key(e, findFocus())) return true;
                return super.dispatchKeyEvent(e);
            }
        };
        return host;
    }

    /** `sheet`/`popover`/`dialog`, `fixed`, `medium`, `manual`, `keepfocus`,
     *  `side=top`/`trailing`/`leading`, `align=center`/`end`, `placed`,
     *  `backdrop=…`, `bare` (the size is Rust's layout), `anchor=caret`,
     *  `anchor=text:…`/`field:…` (#327). */
    static void overlayConfig(PrismHost.Node n, String config) {
        n.overlay = "sheet";
        n.placed = false;
        n.anchor = "";
        n.backdrop = "";
        n.bareDialog = false;
        n.fixed = false;
        n.medium = false;
        n.manual = false;
        n.keepFocus = false;
        n.side = "bottom";
        n.align = "start";
        n.caret = false;
        for (String t : config.trim().split("\\s+")) {
            switch (t) {
                case "popover": case "dialog": case "keyboard": n.overlay = t; break;
                case "fixed": n.fixed = true; break;
                case "medium": n.medium = true; break;
                case "manual": n.manual = true; break;
                case "keepfocus": n.keepFocus = true; break;
                case "placed": n.placed = true; break;
                case "bare": n.bareDialog = true; break;
                default:
                    if (t.startsWith("side=")) n.side = t.substring(5);
                    else if (t.startsWith("align=")) n.align = t.substring(6);
                    else if (t.startsWith("backdrop=")) n.backdrop = t.substring(9);
                    else if (t.startsWith("anchor=")) {
                        String a = t.substring(7);
                        n.caret = a.equals("caret");
                        n.anchor = n.caret ? "" : a;
                    }
            }
        }
        reportArea(n);
        // Moved while shown.
        if (n.popup != null && n.popup.isShowing() && !dropDown(n) && n.view.getParent() instanceof View) {
            int[] at = popoverOrigin(n, (View) n.view.getParent());
            n.popup.update(at[0], at[1], -1, -1);
        }
    }

    /** Where a popover goes in its anchor's window (pixels, from the top
     *  left): on its side of the anchor (trailing is the right in
     *  left-to-right layouts), on the opposite side without room there,
     *  lined up with the anchor's start, center or end, kept inside the
     *  window (#113). */
    static int[] popoverOrigin(PrismHost.Node n, View anchor) {
        return placeOrigin(n, anchor, Math.round(4 * PrismHost.density));
    }

    /** As `place_beside` in prism-core: on the node's side of `anchor`,
     *  `gap` away, else the opposite side, lined up as it says, kept in the
     *  window: {x, y, 1 when that side has room, else 0} (#113, #79). */
    static int[] placeOrigin(PrismHost.Node n, View anchor, int gap) {
        float d = PrismHost.density;
        int w = Math.max(1, n.contentSize[0]), h = Math.max(1, n.contentSize[1]);
        View win = anchor.getRootView();
        int vw = win.getWidth(), vh = win.getHeight();
        int[] box = anchorBox(n, anchor);
        int x0 = box[0], y0 = box[1], x1 = box[2], y1 = box[3];
        int m = Math.round(8 * d);
        boolean rtl = anchor.getLayoutDirection() == View.LAYOUT_DIRECTION_RTL;
        String side;
        switch (n.side) {
            case "top": side = "top"; break;
            case "trailing": side = rtl ? "left" : "right"; break;
            case "leading": side = rtl ? "right" : "left"; break;
            default: side = "bottom";
        }
        if (!fits(side, x0, y0, x1, y1, w, h, vw, vh, m, gap)) {
            String other = side.equals("bottom") ? "top" : side.equals("top") ? "bottom" : side.equals("left") ? "right" : "left";
            if (fits(other, x0, y0, x1, y1, w, h, vw, vh, m, gap)) side = other;
        }
        int x, y;
        if (side.equals("bottom") || side.equals("top")) {
            y = side.equals("bottom") ? y1 + gap : y0 - gap - h;
            int start = rtl ? x1 - w : x0, end = rtl ? x0 : x1 - w;
            x = n.align.equals("center") ? x0 + (x1 - x0 - w) / 2 : n.align.equals("end") ? end : start;
        } else {
            x = side.equals("right") ? x1 + gap : x0 - gap - w;
            y = n.align.equals("center") ? y0 + (y1 - y0 - h) / 2 : n.align.equals("end") ? y1 - h : y0;
        }
        boolean room = fits(side, x0, y0, x1, y1, w, h, vw, vh, m, gap);
        x = Math.max(m, Math.min(x, vw - w - m));
        y = Math.max(m, Math.min(y, vh - h - m));
        return new int[]{x, y, room ? 1 : 0};
    }

    /** What an overlay goes against, in window pixels {x0, y0, x1, y1}: its
     *  anchor, or with `anchor=caret` the caret of the anchor's focused
     *  field (else its first one), kept inside the field; the whole field
     *  while it has no focus, the anchor without a field (#328); with
     *  another `anchor=` the field it names, see {@link #fieldBox}. */
    static int[] anchorBox(PrismHost.Node n, View anchor) {
        int[] loc = new int[2];
        anchor.getLocationInWindow(loc);
        int[] whole = {loc[0], loc[1], loc[0] + anchor.getWidth(), loc[1] + anchor.getHeight()};
        if (!n.anchor.isEmpty()) {
            int[] f = fieldBox(n);
            return f != null ? f : whole;
        }
        if (!n.caret) return whole;
        EditText e = caretField(n, anchor);
        if (e == null) return whole;
        e.getLocationInWindow(loc);
        int fx = loc[0], fy = loc[1], fw = e.getWidth(), fh = e.getHeight();
        android.text.Layout l = e.getLayout();
        int at = e.getSelectionEnd();
        if (!e.isFocused() || l == null || at < 0) return new int[]{fx, fy, fx + fw, fy + fh};
        int line = l.getLineForOffset(at);
        float cx = fx + e.getTotalPaddingLeft() - e.getScrollX() + l.getPrimaryHorizontal(at);
        float cy = fy + e.getTotalPaddingTop() - e.getScrollY() + l.getLineTop(line);
        // As `caret_in_field` in prism-core.
        int w = 1;
        int h = Math.round(Math.min(Math.max(0, l.getLineBottom(line) - l.getLineTop(line)), Math.max(0, fh)));
        int x = Math.round(Math.max(fx, Math.min(cx, fx + fw - w)));
        int y = Math.round(Math.max(fy, Math.min(cy, fy + fh - h)));
        return new int[]{x, y, x + w, y + h};
    }

    /** The box of a field anchor (#327), in window pixels {x0, y0, x1, y1}:
     *  the characters of `anchor=text:field:start:end` (a chip's, its line
     *  box kept inside the field's visible area), the field when they are
     *  gone or not laid out or with `anchor=field:field`; null without the
     *  field. */
    static int[] fieldBox(PrismHost.Node n) {
        String[] a = n.anchor.split(":");
        PrismHost.Node f = null;
        if (a.length >= 2) {
            try {
                f = PrismHost.nodes.get(Long.parseLong(a[1]));
            } catch (NumberFormatException ignored) { }
        }
        if (f == null || f.view == null || f.view.getWindowToken() == null) return null;
        View v = f.view;
        int[] loc = new int[2];
        v.getLocationInWindow(loc);
        if (a[0].equals("text") && a.length == 4 && v instanceof EditText) {
            float[] b = null;
            try {
                b = PrismRich.rangeBox((EditText) v, Integer.parseInt(a[2]), Integer.parseInt(a[3]));
            } catch (NumberFormatException ignored) { }
            if (b != null) {
                return new int[]{loc[0] + Math.round(b[0]), loc[1] + Math.round(b[1]),
                        loc[0] + Math.round(b[2]), loc[1] + Math.round(b[3])};
            }
        }
        return new int[]{loc[0], loc[1], loc[0] + v.getWidth(), loc[1] + v.getHeight()};
    }

    /** The field a caret anchor follows: the focused one inside `anchor`,
     *  else the first one there, never inside the overlay itself (#328). */
    static EditText caretField(PrismHost.Node n, View anchor) {
        View f = anchor.findFocus();
        if (f instanceof EditText && f != n.view && !inside(f, n.view)) return (EditText) f;
        return firstField(anchor, n.view);
    }

    static EditText firstField(View v, View skip) {
        if (v == skip || v.getVisibility() != View.VISIBLE) return null;
        if (v instanceof EditText && v.isEnabled()) return (EditText) v;
        if (v instanceof ViewGroup) {
            ViewGroup g = (ViewGroup) v;
            for (int i = 0; i < g.getChildCount(); i++) {
                EditText e = firstField(g.getChildAt(i), skip);
                if (e != null) return e;
            }
        }
        return null;
    }

    /** Follows the caret (#328) or the field anchor (#327) while shown:
     *  placed again before every frame of the app's window, so typing, a
     *  selection, the text reflowing or a scroll moves it in the frame
     *  that moves what it goes against. */
    static void followCaret(PrismHost.Node n) {
        if ((!n.caret && n.anchor.isEmpty()) || n.caretFollow != null || PrismHost.root == null) return;
        android.view.ViewTreeObserver.OnPreDrawListener l = () -> {
            placeAtCaret(n);
            return true;
        };
        n.caretFollow = l;
        n.caretTree = PrismHost.root;
        PrismHost.root.getViewTreeObserver().addOnPreDrawListener(l);
    }

    static void unfollowCaret(PrismHost.Node n) {
        if (n.caretFollow == null) return;
        android.view.ViewTreeObserver o = n.caretTree.getViewTreeObserver();
        if (o.isAlive()) o.removeOnPreDrawListener(n.caretFollow);
        n.caretFollow = null;
        n.caretTree = null;
    }

    static void placeAtCaret(PrismHost.Node n) {
        View anchor = n.view.getParent() instanceof View ? (View) n.view.getParent() : null;
        if (anchor == null || anchor.getWindowToken() == null) return;
        if (n.popup != null && n.popup.isShowing()) {
            int[] at = popoverOrigin(n, anchor);
            if (n.caretAt == null || n.caretAt[0] != at[0] || n.caretAt[1] != at[1]) {
                n.caretAt = at;
                n.popup.update(at[0], at[1], -1, -1);
            }
        } else if (n.dialog != null && n.dialog.isShowing() && n.dialog.getWindow() != null) {
            int[] at = placeOrigin(n, anchor, Math.round(8 * PrismHost.density));
            if (n.caretAt == null || n.caretAt[0] != at[0] || n.caretAt[1] != at[1] || n.caretAt[2] != at[2]) {
                n.caretAt = at;
                placeDialog(n, n.dialog.getWindow());
            }
        }
    }

    static boolean fits(String side, int x0, int y0, int x1, int y1, int w, int h, int vw, int vh, int m, int gap) {
        switch (side) {
            case "top": return y0 - gap - h >= m;
            case "left": return x0 - gap - w >= m;
            case "right": return x1 + gap + w <= vw - m;
            default: return y1 + gap + h <= vh - m;
        }
    }

    /** The default placement: under the anchor, leading-aligned (above it
     *  without room below), as `showAsDropDown` places it. */
    static boolean dropDown(PrismHost.Node n) {
        return n.side.equals("bottom") && n.align.equals("start") && !n.caret && n.anchor.isEmpty();
    }

    /** The area the content may take (Rust lays it out in it), in pixels. */
    static void reportArea(PrismHost.Node n) {
        if (PrismHost.root == null) return;
        int w = PrismHost.root.getWidth(), h = PrismHost.root.getHeight();
        if (w <= 0 || h <= 0) return;
        float d = PrismHost.density;
        int aw, ah;
        switch (n.overlay) {
            case "popover": aw = Math.round(Math.min(w - 16 * d, 320 * d)); ah = Math.round(h * 0.6f); break;
            case "dialog": aw = Math.round(Math.min(w - 48 * d, 560 * d)); ah = Math.round(h - 96 * d); break;
            // Above the IME (#242): the window's width, at most half its height.
            case "keyboard": aw = w; ah = Math.round(h * 0.5f); break;
            default: aw = Math.round(Math.min(w, 640 * d)); ah = Math.round(h * (n.medium ? 0.5f : 0.92f));
        }
        if (aw == n.viewport[0] && ah == n.viewport[1]) return;
        n.viewport = new int[]{aw, ah};
        JSONObject ev = PrismHost.event("viewport", n.id);
        put(ev, "w", aw);
        put(ev, "h", ah);
        PrismHost.main.post(() -> PrismHost.send(ev));
    }

    static GradientDrawable surface(float[] radii) {
        GradientDrawable g = new GradientDrawable();
        g.setColor(PrismHost.dark() ? 0xFF2C2C2E : 0xFFFFFFFF);
        g.setCornerRadii(radii);
        return g;
    }

    /** The content's size (the `content` op): the window follows it. */
    static void contentSize(PrismHost.Node n, int w, int h) {
        n.contentSize = new int[]{w, h};
        n.host.fixedWidth = w;
        n.host.fixedHeight = h;
        n.host.requestLayout();
        if (n.popup != null && n.popup.isShowing()) {
            if (!dropDown(n) && n.view.getParent() instanceof View) {
                int[] at = popoverOrigin(n, (View) n.view.getParent());
                n.popup.update(at[0], at[1], w, h);
            } else n.popup.update(w, h);
        }
        if (n.dialog != null && n.dialog.getWindow() != null) size(n, n.dialog.getWindow());
        if (n == shownBar) placeKeyboardBar(n);
        // Opened in the update that inserted it: its anchor is placed now.
        if (n.pendingOpen) open(n, true);
    }

    static void size(PrismHost.Node n, Window w) {
        int cw = Math.max(1, n.contentSize[0]), ch = Math.max(1, n.contentSize[1]);
        w.setLayout(n.overlay.equals("sheet") && PrismHost.root.getWidth() <= cw ? ViewGroup.LayoutParams.MATCH_PARENT : cw, ch);
        placeDialog(n, w);
    }

    /** A dialog with a side: its window beside the anchor (still a modal
     *  dialog), centered when neither side has room (#79). */
    static void placeDialog(PrismHost.Node n, Window w) {
        if (!n.overlay.equals("dialog") || !n.placed) return;
        WindowManager.LayoutParams lp = w.getAttributes();
        View anchor = n.view.getParent() instanceof View ? (View) n.view.getParent() : null;
        int[] at = anchor == null || anchor.getWindowToken() == null ? null
                : placeOrigin(n, anchor, Math.round(8 * PrismHost.density));
        if (at == null || at[2] == 0) {
            lp.gravity = Gravity.CENTER;
            lp.x = 0;
            lp.y = 0;
        } else {
            // Window coordinates of the app's full-screen window.
            lp.gravity = Gravity.TOP | Gravity.LEFT;
            lp.x = at[0];
            lp.y = at[1];
            w.addFlags(WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS | WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN);
        }
        w.setAttributes(lp);
    }

    /** What shows behind a sheet or dialog: nothing, a dim, or a dim with
     *  the window blurred (Android 12 and later; a dim before). */
    static void backdrop(PrismHost.Node n, Window w) {
        String b = n.backdrop.isEmpty() ? "dim" : n.backdrop;
        if (b.equals("clear")) {
            w.clearFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND);
            return;
        }
        w.addFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND);
        w.setDimAmount(b.equals("blur") ? 0.25f : 0.4f);
        if (b.equals("blur") && android.os.Build.VERSION.SDK_INT >= 31) {
            w.addFlags(WindowManager.LayoutParams.FLAG_BLUR_BEHIND);
            WindowManager.LayoutParams lp = w.getAttributes();
            lp.setBlurBehindRadius(Math.round(16 * PrismHost.density));
            w.setAttributes(lp);
        }
    }

    static void open(PrismHost.Node n, boolean open) {
        if (n.overlay.equals("keyboard")) {
            // Shown while a field it serves has the focus (#242).
            n.pendingOpen = false;
            reportArea(n);
            syncKeyboardBar();
            return;
        }
        if (!open) {
            close(n);
            return;
        }
        if ((n.dialog != null && n.dialog.isShowing()) || (n.popup != null && n.popup.isShowing())) return;
        reportArea(n);
        float d = PrismHost.density;
        n.pendingOpen = false;
        if (n.overlay.equals("popover")) {
            // Inserted open with its parent (#70): shown once laid out.
            if (!(n.view.getParent() instanceof View) || n.view.getWindowToken() == null) {
                n.pendingOpen = true;
                return;
            }
            View anchor = (View) n.view.getParent();
            detach(n.host);
            // `keepfocus`: not focusable, so the anchor's field keeps the
            // keys and the keyboard; the popup sits above the keyboard.
            PopupWindow p = new PopupWindow(n.host, Math.max(1, n.contentSize[0]), Math.max(1, n.contentSize[1]), !n.keepFocus);
            if (n.keepFocus) p.setInputMethodMode(PopupWindow.INPUT_METHOD_NEEDED);
            float r = 14 * d;
            if (n.bareDialog) {
                // No box of its own (#287): a transparent popup (a
                // background still, so a touch outside reaches it), no
                // elevation, no animation.
                p.setBackgroundDrawable(new android.graphics.drawable.ColorDrawable(android.graphics.Color.TRANSPARENT));
                p.setElevation(0);
                p.setAnimationStyle(0);
            } else {
                p.setBackgroundDrawable(surface(new float[]{r, r, r, r, r, r, r, r}));
                p.setElevation(12 * d);
            }
            // `keepfocus` too (#182): not focusable, so the touch outside
            // still reaches the anchor's field while the popup closes.
            p.setOutsideTouchable(!(n.fixed || n.manual));
            // A touch outside closes it: say so (#156); otherwise it is Back.
            p.setTouchInterceptor((v, e) -> {
                if (e.getAction() == MotionEvent.ACTION_OUTSIDE) n.dismissReason = "outside";
                return false;
            });
            p.setOnDismissListener(() -> {
                if (n.popup == p) {
                    n.popup = null;
                    unfollowCaret(n);
                }
                if (!n.closing) dismissed(n);
            });
            n.popup = p;
            n.closing = false;
            if (dropDown(n)) p.showAsDropDown(anchor, 0, Math.round(4 * d), Gravity.START);
            else {
                int[] at = popoverOrigin(n, anchor);
                n.caretAt = at;
                p.showAtLocation(anchor, Gravity.LEFT | Gravity.TOP, at[0], at[1]);
            }
            followCaret(n);
            return;
        }
        boolean sheet = n.overlay.equals("sheet");
        Dialog dialog = new Dialog(PrismHost.themed(), PrismHost.dark()
                ? android.R.style.Theme_DeviceDefault_Dialog_NoActionBar
                : android.R.style.Theme_DeviceDefault_Light_Dialog_NoActionBar) {
            /** A touch outside cancels it from here (#156): the reason, while it does. */
            @Override public boolean onTouchEvent(MotionEvent e) {
                n.dismissReason = "outside";
                boolean handled = super.onTouchEvent(e);
                if (isShowing()) n.dismissReason = null;
                return handled;
            }
        };
        detach(n.host);
        dialog.setContentView(n.host);
        dialog.setCancelable(!n.fixed);
        dialog.setCanceledOnTouchOutside(!n.fixed);
        dialog.setOnCancelListener(x -> {
            unfollowCaret(n);
            dismissed(n);
        });
        dialog.setOnKeyListener((x, code, e) -> {
            // Escape on a hardware keyboard closes it, as Back does (#156).
            if (code == KeyEvent.KEYCODE_ESCAPE && e.getAction() == KeyEvent.ACTION_UP && !n.fixed) {
                n.dismissReason = "escape";
                dialog.cancel();
                return true;
            }
            return key(e, dialog.getCurrentFocus());
        });
        Window w = dialog.getWindow();
        if (w != null) {
            float r = (sheet ? 16 : 28) * d;
            if (n.bareDialog && !sheet) {
                // No box of its own (#166): a transparent window (no
                // surface, so no shadow either), shown without animation.
                w.setBackgroundDrawable(new android.graphics.drawable.ColorDrawable(android.graphics.Color.TRANSPARENT));
                w.setWindowAnimations(0);
            } else w.setBackgroundDrawable(surface(sheet ? new float[]{r, r, r, r, 0, 0, 0, 0} : new float[]{r, r, r, r, r, r, r, r}));
            backdrop(n, w);
            if (sheet) {
                w.setGravity(Gravity.BOTTOM);
                w.setWindowAnimations(android.R.style.Animation_InputMethod);
            }
            size(n, w);
        }
        n.dialog = dialog;
        n.closing = false;
        PrismForms.show(dialog);
        if (w != null) size(n, w);
        n.caretAt = null;
        followCaret(n);
    }

    /** The user closed it (Back, Escape, a tap outside): Prism's on_dismiss, with how (#156). */
    static void dismissed(PrismHost.Node n) {
        JSONObject ev = PrismHost.event("back", n.id);
        put(ev, "depth", 0);
        put(ev, "reason", n.dismissReason != null ? n.dismissReason : "back");
        n.dismissReason = null;
        PrismHost.send(ev);
    }

    static void close(PrismHost.Node n) {
        if (n == shownBar) showKeyboardBar(null);
        n.pendingOpen = false;
        n.closing = true;
        unfollowCaret(n);
        if (n.popup != null) { n.popup.dismiss(); n.popup = null; }
        if (n.dialog != null) { n.dialog.dismiss(); n.dialog = null; }
        n.closing = false;
    }

    // --- Keyboard bars (#242) ------------------------------------------------------------

    /** The keyboard bar on screen: docked to the bottom of the content area,
     *  which the window insets keep above the IME. */
    static PrismHost.Node shownBar;

    /** Shows the bar of the focused field, once the focus has settled. */
    static void syncKeyboardBar() {
        PrismHost.main.post(PrismInput::applyKeyboardBar);
    }

    static void applyKeyboardBar() {
        View f = PrismHost.activity == null ? null : PrismHost.activity.getCurrentFocus();
        PrismHost.Node want = null;
        if (f != null) {
            for (PrismHost.Node n : PrismHost.nodes.values()) {
                if (n.keyboardBar >= 0 && (n.view == f || inside(f, n.view))) {
                    want = PrismHost.nodes.get(n.keyboardBar);
                    break;
                }
            }
            // A field of the bar itself keeps it.
            if (want == null && shownBar != null && inside(f, shownBar.host)) want = shownBar;
        }
        if (want != null && (!want.overlay.equals("keyboard") || want.host == null)) want = null;
        showKeyboardBar(want);
    }

    static boolean inside(View v, View ancestor) {
        for (android.view.ViewParent p = v.getParent(); p != null; p = p.getParent()) if (p == ancestor) return true;
        return false;
    }

    static void showKeyboardBar(PrismHost.Node bar) {
        if (bar == shownBar) return;
        if (shownBar != null) {
            detach(shownBar.host);
            setRootBottom(0);
        }
        shownBar = bar;
        if (bar == null || PrismHost.root == null || !(PrismHost.root.getParent() instanceof ViewGroup)) return;
        detach(bar.host);
        ((ViewGroup) PrismHost.root.getParent()).addView(bar.host,
                new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, Math.max(1, bar.contentSize[1]), Gravity.BOTTOM));
        placeKeyboardBar(bar);
    }

    /** The bar's height, and the app's content above it. */
    static void placeKeyboardBar(PrismHost.Node bar) {
        ViewGroup.LayoutParams lp = bar.host.getLayoutParams();
        int h = Math.max(1, bar.contentSize[1]);
        if (lp != null && lp.height != h) {
            lp.height = h;
            bar.host.setLayoutParams(lp);
        }
        setRootBottom(h);
    }

    static void setRootBottom(int px) {
        if (PrismHost.root == null || !(PrismHost.root.getLayoutParams() instanceof ViewGroup.MarginLayoutParams)) return;
        ViewGroup.MarginLayoutParams lp = (ViewGroup.MarginLayoutParams) PrismHost.root.getLayoutParams();
        if (lp.bottomMargin == px) return;
        lp.bottomMargin = px;
        PrismHost.root.setLayoutParams(lp);
    }

    // --- Other people's carets (#240) ---------------------------------------------------

    /**
     * Other people's carets and selections in a field: a drawable in its
     * overlay, drawn after its text with its own layout (wrapping and
     * scrolling included), never hit. Its text, selection and composition
     * are left alone. An idle caret's name fades out.
     */
    static void carets(PrismHost.Node n, JSONArray list) {
        if (!(n.view instanceof EditText)) return;
        EditText e = (EditText) n.view;
        RemoteCarets d = CARETS.get(e);
        if (list == null || list.length() == 0) {
            if (d != null) { e.getOverlay().remove(d); CARETS.remove(e); }
            return;
        }
        if (d == null) {
            d = new RemoteCarets(e);
            CARETS.put(e, d);
            e.getOverlay().add(d);
        }
        d.set(list);
        e.invalidate();
    }

    private static final Map<EditText, RemoteCarets> CARETS = new WeakHashMap<>();

    static final class RemoteCarets extends android.graphics.drawable.Drawable {
        private final java.lang.ref.WeakReference<EditText> field;
        private final java.util.ArrayList<JSONObject> list = new java.util.ArrayList<>();
        /** Each caret's tag opacity (0…1). */
        private final java.util.ArrayList<float[]> tags = new java.util.ArrayList<>();
        private final android.graphics.Paint paint = new android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG);
        private final android.graphics.Paint text = new android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG);

        RemoteCarets(EditText e) {
            field = new java.lang.ref.WeakReference<>(e);
            text.setColor(android.graphics.Color.WHITE);
            text.setTextSize(11 * PrismHost.density);
            text.setTypeface(android.graphics.Typeface.DEFAULT_BOLD);
            setBounds(0, 0, Integer.MAX_VALUE / 2, Integer.MAX_VALUE / 2);
        }

        void set(JSONArray a) {
            boolean[] wasIdle = new boolean[list.size()];
            for (int i = 0; i < list.size(); i++) wasIdle[i] = list.get(i).optBoolean("idle");
            list.clear();
            for (int i = 0; i < a.length(); i++) list.add(a.optJSONObject(i));
            while (tags.size() > list.size()) tags.remove(tags.size() - 1);
            while (tags.size() < list.size()) tags.add(new float[]{1});
            for (int i = 0; i < list.size(); i++) {
                boolean idle = list.get(i).optBoolean("idle");
                float[] tag = tags.get(i);
                if (!idle) tag[0] = 1;
                else if (i >= wasIdle.length || !wasIdle[i]) fade(tag);
            }
        }

        private void fade(float[] tag) {
            android.animation.ValueAnimator anim = android.animation.ValueAnimator.ofFloat(tag[0], 0);
            anim.setDuration(300);
            anim.addUpdateListener(v -> {
                tag[0] = (float) v.getAnimatedValue();
                EditText e = field.get();
                if (e != null) e.invalidate();
            });
            anim.start();
        }

        @Override public void draw(android.graphics.Canvas c) {
            EditText e = field.get();
            android.text.Layout layout = e != null ? e.getLayout() : null;
            if (layout == null) return;
            CharSequence t = e.getText();
            // The overlay draws in the field's scrolled coordinates.
            float dx = e.getCompoundPaddingLeft(), dy = e.getExtendedPaddingTop();
            float sx = e.getScrollX(), sy = e.getScrollY();
            float left = sx + e.getCompoundPaddingLeft(), right = sx + e.getWidth() - e.getCompoundPaddingRight();
            float top = sy + e.getExtendedPaddingTop(), bottom = sy + e.getHeight() - e.getExtendedPaddingBottom();
            float d = PrismHost.density;
            for (int i = 0; i < list.size(); i++) {
                JSONObject o = list.get(i);
                Integer color = PrismHost.color(o.optJSONArray("color"));
                if (color == null) continue;
                int a = units(t, o.optInt("anchor")), h = units(t, o.optInt("head"));
                int s = Math.min(a, h), en = Math.max(a, h);
                // The selection: one box per line.
                paint.setColor(color);
                paint.setAlpha(Math.round(android.graphics.Color.alpha(color) * 0.25f));
                for (int line = layout.getLineForOffset(s); s < en && line <= layout.getLineForOffset(en); line++) {
                    int ls = Math.max(s, layout.getLineStart(line)), le = Math.min(en, layout.getLineEnd(line));
                    float x1 = layout.getPrimaryHorizontal(ls) + dx;
                    float x2 = (le == layout.getLineEnd(line) && le < en ? layout.getLineRight(line) : layout.getPrimaryHorizontal(le)) + dx;
                    float y1 = layout.getLineTop(line) + dy, y2 = layout.getLineBottom(line) + dy;
                    float l = Math.max(Math.min(x1, x2), left), r = Math.min(Math.max(x1, x2), right);
                    float tp = Math.max(y1, top), bt = Math.min(y2, bottom);
                    if (r > l && bt > tp) c.drawRect(l, tp, r, bt, paint);
                }
                // The caret.
                int line = layout.getLineForOffset(h);
                float x = layout.getPrimaryHorizontal(h) + dx;
                float y1 = layout.getLineTop(line) + dy, y2 = layout.getLineBottom(line) + dy;
                if (x < left - d || x > right + d || y2 <= top || y1 >= bottom) continue;
                paint.setColor(color);
                c.drawRect(x - d, Math.max(y1, top), x + d, Math.min(y2, bottom), paint);
                // Its name: above the line, or under it at the top.
                String label = o.isNull("label") ? null : o.optString("label", null);
                float alpha = tags.get(i)[0];
                if (label == null || alpha <= 0) continue;
                float w = text.measureText(label) + 8 * d, th = 15 * d;
                float ty = y1 - th >= sy ? y1 - th : y2;
                float tx = Math.max(sx, Math.min(x - d, sx + e.getWidth() - w));
                paint.setAlpha(Math.round(android.graphics.Color.alpha(color) * alpha));
                c.drawRoundRect(tx, ty, tx + w, ty + th, 3 * d, 3 * d, paint);
                // White, or the app's (#267).
                Integer ink = PrismHost.color(o.optJSONArray("text"));
                text.setColor(ink != null ? ink : android.graphics.Color.WHITE);
                text.setAlpha(Math.round(android.graphics.Color.alpha(text.getColor()) * alpha));
                c.drawText(label, tx + 4 * d, ty + th - 4 * d, text);
            }
        }

        @Override public void setAlpha(int alpha) {}
        @Override public void setColorFilter(android.graphics.ColorFilter filter) {}
        @Override public int getOpacity() { return android.graphics.PixelFormat.TRANSLUCENT; }
    }

    // --- One selection across several fields (#231) ------------------------------------

    /**
     * Op `selhl`: the field's part of a selection across several fields,
     * drawn like a selection (the field's highlight colour) in its overlay
     * with its own layout, one-line inputs included; its text and its own
     * selection are left alone. `start == end` removes it.
     */
    static void highlight(PrismHost.Node n, int start, int end) {
        if (!(n.view instanceof EditText)) return;
        EditText e = (EditText) n.view;
        Highlight d = HIGHLIGHTS.get(e);
        if (start >= end) {
            if (d != null) { e.getOverlay().remove(d); HIGHLIGHTS.remove(e); e.invalidate(); }
            return;
        }
        if (d == null) {
            d = new Highlight(e);
            HIGHLIGHTS.put(e, d);
            e.getOverlay().add(d);
        }
        d.start = start;
        d.end = end;
        e.invalidate();
    }

    private static final Map<EditText, Highlight> HIGHLIGHTS = new WeakHashMap<>();

    static final class Highlight extends android.graphics.drawable.Drawable {
        private final java.lang.ref.WeakReference<EditText> field;
        private final android.graphics.Paint paint = new android.graphics.Paint();
        int start, end;

        Highlight(EditText e) {
            field = new java.lang.ref.WeakReference<>(e);
            setBounds(0, 0, Integer.MAX_VALUE / 2, Integer.MAX_VALUE / 2);
        }

        @Override public void draw(android.graphics.Canvas c) {
            EditText e = field.get();
            android.text.Layout layout = e != null ? e.getLayout() : null;
            if (layout == null) return;
            CharSequence t = e.getText();
            int s = units(t, start), en = units(t, end);
            if (s >= en) return;
            // The overlay draws in the field's scrolled coordinates.
            float dx = e.getCompoundPaddingLeft(), dy = e.getExtendedPaddingTop();
            float sx = e.getScrollX(), sy = e.getScrollY();
            float left = sx + e.getCompoundPaddingLeft(), right = sx + e.getWidth() - e.getCompoundPaddingRight();
            float top = sy + e.getExtendedPaddingTop(), bottom = sy + e.getHeight() - e.getExtendedPaddingBottom();
            paint.setColor(e.getHighlightColor());
            for (int line = layout.getLineForOffset(s); line <= layout.getLineForOffset(en); line++) {
                int ls = Math.max(s, layout.getLineStart(line)), le = Math.min(en, layout.getLineEnd(line));
                if (ls > le) continue;
                float x1 = layout.getPrimaryHorizontal(ls) + dx;
                // A line selected to its end reaches the line's edge.
                float x2 = (le == layout.getLineEnd(line) && le < en ? layout.getLineRight(line) : layout.getPrimaryHorizontal(le)) + dx;
                float y1 = layout.getLineTop(line) + dy, y2 = layout.getLineBottom(line) + dy;
                float l = Math.max(Math.min(x1, x2), left), r = Math.min(Math.max(x1, x2), right);
                float tp = Math.max(y1, top), bt = Math.min(y2, bottom);
                if (r > l && bt > tp) c.drawRect(l, tp, r, bt, paint);
            }
        }

        @Override public void setAlpha(int alpha) {}
        @Override public void setColorFilter(android.graphics.ColorFilter filter) {}
        @Override public int getOpacity() { return android.graphics.PixelFormat.TRANSLUCENT; }
    }

    /**
     * A field's input filters: text an input method types over a selection
     * across fields goes to the group (#231), then `max_length` (-1: none).
     */
    static void filters(EditText e, PrismHost.Node n, int max) {
        android.text.InputFilter across = new AcrossFilter(n);
        e.setFilters(max >= 0 ? new android.text.InputFilter[]{across, new android.text.InputFilter.LengthFilter(max)}
                : new android.text.InputFilter[]{across});
    }

    /**
     * Keeps what a soft keyboard or an input method types out of a field
     * while its group's selection spans several fields and the app takes
     * those edits (`on_edit_across`): sent as `typeacross` once the field's
     * callback returned, then the input method starts over (its
     * composition was never applied). Hardware keys go through `key`.
     */
    static final class AcrossFilter implements android.text.InputFilter {
        private final PrismHost.Node n;

        AcrossFilter(PrismHost.Node n) { this.n = n; }

        @Override public CharSequence filter(CharSequence source, int start, int end, android.text.Spanned dest, int dstart, int dend) {
            if (PrismHost.updating || !PrismHost.nativeTakesText(n.id)) return null;
            String text = source.subSequence(start, end).toString();
            View v = n.view;
            v.post(() -> {
                JSONObject ev = PrismHost.event("typeacross", n.id);
                put(ev, "text", text);
                PrismHost.send(ev);
                InputMethodManager imm = (InputMethodManager) v.getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
                if (imm != null) imm.restartInput(v);
            });
            // The field keeps its text.
            return dest.subSequence(dstart, dend);
        }
    }

    /** A mouse drag from a field of a selection group (#231). */
    private static final class AcrossDrag {
        PrismHost.Node anchor, group;
        /** Where it started, in characters. */
        int at;
        /** The group's text fields. */
        java.util.List<PrismHost.Node> fields;
        /** It left the field for another of the group: the field's own drag was cancelled. */
        boolean across;
    }

    private static AcrossDrag drag;

    /** The selection group `n` is in, or null. */
    static PrismHost.Node groupOf(PrismHost.Node n) {
        for (PrismHost.Node a = PrismHost.parentOf(n); a != null; a = PrismHost.parentOf(a)) {
            if (a.selGroup) return a;
        }
        return null;
    }

    /**
     * A mouse (or stylus) drag in a text field of a selection group (#231):
     * the field's own while it stays there; once it is over another field
     * of the group, the field's drag is cancelled and every move is sent
     * as `across` (back over the first field too), the runtime selecting
     * and highlighting the range. Finger drags scroll and use the field's
     * own selection handles. Returns whether the event was taken.
     */
    static boolean acrossTouch(EditText e, MotionEvent ev) {
        boolean pointer = ev.isFromSource(android.view.InputDevice.SOURCE_MOUSE)
            || ev.getToolType(0) == MotionEvent.TOOL_TYPE_STYLUS;
        switch (ev.getActionMasked()) {
            case MotionEvent.ACTION_DOWN: {
                drag = null;
                PrismHost.Node n = byView.get(e);
                PrismHost.Node group = n == null || !pointer ? null : groupOf(n);
                if (group == null) return false;
                AcrossDrag d = new AcrossDrag();
                d.anchor = n;
                d.group = group;
                d.at = chars(e.getText(), e.getOffsetForPosition(ev.getX(), ev.getY()));
                d.fields = new java.util.ArrayList<>();
                for (PrismHost.Node f : PrismHost.nodes.values()) {
                    if (f.view instanceof EditText && groupOf(f) == group) d.fields.add(f);
                }
                drag = d;
                // A pointer drag selects: the scroll views around keep out.
                if (e.getParent() != null) e.getParent().requestDisallowInterceptTouchEvent(true);
                return false;
            }
            case MotionEvent.ACTION_MOVE: {
                AcrossDrag d = drag;
                if (d == null || d.anchor.view != e) return false;
                PrismHost.Node over = null;
                int[] at = new int[2];
                for (PrismHost.Node f : d.fields) {
                    View v = f.view;
                    if (!v.isShown()) continue;
                    v.getLocationOnScreen(at);
                    if (ev.getRawX() >= at[0] && ev.getRawX() < at[0] + v.getWidth()
                            && ev.getRawY() >= at[1] && ev.getRawY() < at[1] + v.getHeight()) {
                        over = f;
                        break;
                    }
                }
                if (!d.across) {
                    if (over == null || over == d.anchor) return false;
                    d.across = true;
                    // The field where it started has the keys (⌃C, typing),
                    // which it would only take on the release it won't see.
                    if (!e.isFocused()) e.requestFocus();
                    // The field's own drag selection ends here.
                    MotionEvent cancel = MotionEvent.obtain(ev);
                    cancel.setAction(MotionEvent.ACTION_CANCEL);
                    e.onTouchEvent(cancel);
                    cancel.recycle();
                }
                // Between two fields: the range stays as it was.
                if (over == null) return true;
                EditText o = (EditText) over.view;
                o.getLocationOnScreen(at);
                int offset = o.getOffsetForPosition(ev.getRawX() - at[0], ev.getRawY() - at[1]);
                JSONObject msg = PrismHost.event("across", d.group.id);
                put(msg, "a", new JSONArray().put(d.anchor.id).put(d.at));
                put(msg, "h", new JSONArray().put(over.id).put(chars(o.getText(), Math.max(0, offset))));
                PrismHost.send(msg);
                return true;
            }
            case MotionEvent.ACTION_UP: case MotionEvent.ACTION_CANCEL: {
                AcrossDrag d = drag;
                drag = null;
                return d != null && d.across && d.anchor.view == e;
            }
        }
        return false;
    }

    static void detach(View v) {
        if (v.getParent() instanceof ViewGroup) ((ViewGroup) v.getParent()).removeView(v);
    }
}
