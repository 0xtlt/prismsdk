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
        JSONObject ev = PrismHost.event("drag", n.id);
        put(ev, "phase", phase);
        put(ev, "x", dp(n.down[0]));
        put(ev, "y", dp(n.down[1]));
        // Raw coordinates: the view may move while it is dragged (reordering).
        // The fingers' center: a second finger (a pinch) moves it too.
        float[] c = e == null ? null : center(e, -1);
        put(ev, "dx", c == null ? 0 : dp(c[0] - n.down[2]));
        put(ev, "dy", c == null ? 0 : dp(c[1] - n.down[3]));
        // The button and keys it started with (#136).
        put(ev, "button", n.dragButton);
        put(ev, "mods", n.dragMods);
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
            for (android.net.Uri u : uris) {
                JSONObject f = PrismDevice.copy(u, true);
                if (f != null && accepts(types, f.optString("name"), f.optString("mime"))) files.put(f);
            }
            if (grant != null) grant.release();
            if (files.length() == 0) return;
            PrismHost.main.post(() -> {
                JSONObject ev = PrismHost.event("drop", n.id);
                try { ev.put("files", files); } catch (JSONException ignored) {}
                PrismHost.send(ev);
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
                    if (!n.dragging && dx * dx + dy * dy > slop * slop) {
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
                    if (n.dragging) {
                        n.dragging = false;
                        drag(n, "end", e);
                        return true;
                    }
                    break;
                case MotionEvent.ACTION_CANCEL:
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

    /** `cx.scroll_by` / `cx.scroll_to` (#193): points to pixels; the view keeps it within its content. */
    static void scrollTo(PrismHost.Node n, float x, float y, boolean relative, boolean animated) {
        View v = n.view;
        float d = PrismHost.density;
        int px = (int) Math.max(-1e8f, Math.min(1e8f, x * d)), py = (int) Math.max(-1e8f, Math.min(1e8f, y * d));
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
                    ? View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS : View.IMPORTANT_FOR_ACCESSIBILITY_AUTO);
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
     *  `backdrop=…`, `bare` (the size is Rust's layout). */
    static void overlayConfig(PrismHost.Node n, String config) {
        n.overlay = "sheet";
        n.placed = false;
        n.backdrop = "";
        n.bareDialog = false;
        n.fixed = false;
        n.medium = false;
        n.manual = false;
        n.keepFocus = false;
        n.side = "bottom";
        n.align = "start";
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
        int[] loc = new int[2];
        anchor.getLocationInWindow(loc);
        int x0 = loc[0], y0 = loc[1], x1 = x0 + anchor.getWidth(), y1 = y0 + anchor.getHeight();
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
        return n.side.equals("bottom") && n.align.equals("start");
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
            p.setBackgroundDrawable(surface(new float[]{r, r, r, r, r, r, r, r}));
            p.setElevation(12 * d);
            // `keepfocus` too (#182): not focusable, so the touch outside
            // still reaches the anchor's field while the popup closes.
            p.setOutsideTouchable(!(n.fixed || n.manual));
            // A touch outside closes it: say so (#156); otherwise it is Back.
            p.setTouchInterceptor((v, e) -> {
                if (e.getAction() == MotionEvent.ACTION_OUTSIDE) n.dismissReason = "outside";
                return false;
            });
            p.setOnDismissListener(() -> {
                if (n.popup == p) n.popup = null;
                if (!n.closing) dismissed(n);
            });
            n.popup = p;
            n.closing = false;
            if (dropDown(n)) p.showAsDropDown(anchor, 0, Math.round(4 * d), Gravity.START);
            else {
                int[] at = popoverOrigin(n, anchor);
                p.showAtLocation(anchor, Gravity.LEFT | Gravity.TOP, at[0], at[1]);
            }
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
        dialog.setOnCancelListener(x -> dismissed(n));
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

    static void detach(View v) {
        if (v.getParent() instanceof ViewGroup) ((ViewGroup) v.getParent()).removeView(v);
    }
}
