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

    static final int HOVER = 1, MOVE = 2, DRAG = 4;

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
        put(ev, "dx", e == null ? 0 : dp(e.getRawX() - n.down[2]));
        put(ev, "dy", e == null ? 0 : dp(e.getRawY() - n.down[3]));
        PrismHost.send(ev);
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
    }

    /**
     * Touches of a view with gestures (`g`: taps, long presses, flings): moves
     * while touching, and drags past the touch slop. A drag cancels the tap.
     */
    static boolean touch(PrismHost.Node n, android.view.GestureDetector g, MotionEvent e) {
        int a = e.getActionMasked();
        if ((n.pointerMask & MOVE) != 0 && (a == MotionEvent.ACTION_DOWN || a == MotionEvent.ACTION_MOVE)) {
            pointer(n, "move", e.getX(), e.getY());
        }
        if ((n.pointerMask & DRAG) != 0) {
            switch (a) {
                case MotionEvent.ACTION_DOWN:
                    n.down = new float[]{e.getX(), e.getY(), e.getRawX(), e.getRawY()};
                    n.dragging = false;
                    break;
                case MotionEvent.ACTION_MOVE: {
                    float dx = e.getRawX() - n.down[2], dy = e.getRawY() - n.down[3];
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
        return g.onTouchEvent(e) || n.pointerMask != 0;
    }

    /** A tap's position goes with its click. */
    static void click(PrismHost.Node n, MotionEvent e) {
        JSONObject ev = PrismHost.event("click", n.id);
        put(ev, "x", dp(e.getX()));
        put(ev, "y", dp(e.getY()));
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
        return (e.isShiftPressed() ? 1 : 0) | (e.isCtrlPressed() ? 2 : 0) | (e.isAltPressed() ? 4 : 0) | (e.isMetaPressed() ? 8 : 0);
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
        }
        PrismHost.send(ev);
        boolean arrow = name.startsWith("Arrow");
        return local && arrow && !(focused instanceof EditText);
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
                case "popover": case "dialog": n.overlay = t; break;
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
            p.setOutsideTouchable(!(n.fixed || n.manual || n.keepFocus));
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
        n.pendingOpen = false;
        n.closing = true;
        if (n.popup != null) { n.popup.dismiss(); n.popup = null; }
        if (n.dialog != null) { n.dialog.dismiss(); n.dialog = null; }
        n.closing = false;
    }

    static void detach(View v) {
        if (v.getParent() instanceof ViewGroup) ((ViewGroup) v.getParent()).removeView(v);
    }
}
