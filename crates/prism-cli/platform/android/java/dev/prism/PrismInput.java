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
        PrismHost.send(ev);
        boolean arrow = name.startsWith("Arrow");
        return local && arrow && !(focused instanceof EditText);
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

    /** `sheet`/`popover`/`dialog`, `fixed`, `medium`. */
    static void overlayConfig(PrismHost.Node n, String config) {
        n.overlay = "sheet";
        n.fixed = false;
        n.medium = false;
        for (String t : config.trim().split("\\s+")) {
            switch (t) {
                case "popover": case "dialog": n.overlay = t; break;
                case "fixed": n.fixed = true; break;
                case "medium": n.medium = true; break;
            }
        }
        reportArea(n);
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
        if (n.popup != null && n.popup.isShowing()) n.popup.update(w, h);
        if (n.dialog != null && n.dialog.getWindow() != null) size(n, n.dialog.getWindow());
    }

    static void size(PrismHost.Node n, Window w) {
        int cw = Math.max(1, n.contentSize[0]), ch = Math.max(1, n.contentSize[1]);
        w.setLayout(n.overlay.equals("sheet") && PrismHost.root.getWidth() <= cw ? ViewGroup.LayoutParams.MATCH_PARENT : cw, ch);
    }

    static void open(PrismHost.Node n, boolean open) {
        if (!open) {
            close(n);
            return;
        }
        if ((n.dialog != null && n.dialog.isShowing()) || (n.popup != null && n.popup.isShowing())) return;
        reportArea(n);
        float d = PrismHost.density;
        if (n.overlay.equals("popover")) {
            View anchor = n.view.getParent() instanceof View ? (View) n.view.getParent() : PrismHost.root;
            detach(n.host);
            PopupWindow p = new PopupWindow(n.host, Math.max(1, n.contentSize[0]), Math.max(1, n.contentSize[1]), true);
            float r = 14 * d;
            p.setBackgroundDrawable(surface(new float[]{r, r, r, r, r, r, r, r}));
            p.setElevation(12 * d);
            p.setOutsideTouchable(!n.fixed);
            p.setOnDismissListener(() -> {
                if (n.popup == p) n.popup = null;
                if (!n.closing) dismissed(n);
            });
            n.popup = p;
            n.closing = false;
            p.showAsDropDown(anchor, 0, Math.round(4 * d), Gravity.START);
            return;
        }
        boolean sheet = n.overlay.equals("sheet");
        Dialog dialog = new Dialog(PrismHost.themed(), PrismHost.dark()
                ? android.R.style.Theme_DeviceDefault_Dialog_NoActionBar
                : android.R.style.Theme_DeviceDefault_Light_Dialog_NoActionBar);
        detach(n.host);
        dialog.setContentView(n.host);
        dialog.setCancelable(!n.fixed);
        dialog.setCanceledOnTouchOutside(!n.fixed);
        dialog.setOnCancelListener(x -> dismissed(n));
        dialog.setOnKeyListener((x, code, e) -> key(e, dialog.getCurrentFocus()));
        Window w = dialog.getWindow();
        if (w != null) {
            float r = (sheet ? 16 : 28) * d;
            w.setBackgroundDrawable(surface(sheet ? new float[]{r, r, r, r, 0, 0, 0, 0} : new float[]{r, r, r, r, r, r, r, r}));
            w.addFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND);
            w.setDimAmount(0.4f);
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

    /** The user closed it (Back, Escape, a tap outside): Prism's on_dismiss. */
    static void dismissed(PrismHost.Node n) {
        JSONObject ev = PrismHost.event("back", n.id);
        put(ev, "depth", 0);
        PrismHost.send(ev);
    }

    static void close(PrismHost.Node n) {
        n.closing = true;
        if (n.popup != null) { n.popup.dismiss(); n.popup = null; }
        if (n.dialog != null) { n.dialog.dismiss(); n.dialog = null; }
        n.closing = false;
    }

    static void detach(View v) {
        if (v.getParent() instanceof ViewGroup) ((ViewGroup) v.getParent()).removeView(v);
    }
}
