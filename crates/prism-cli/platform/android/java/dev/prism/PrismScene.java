package dev.prism;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Rect;
import android.graphics.SurfaceTexture;
import android.opengl.EGL14;
import android.opengl.EGLConfig;
import android.opengl.EGLContext;
import android.opengl.EGLDisplay;
import android.opengl.EGLExt;
import android.opengl.EGLSurface;
import android.opengl.GLES30;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Process;
import android.view.Choreographer;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.ScaleGestureDetector;
import android.view.TextureView;
import android.view.ViewConfiguration;
import android.view.ViewTreeObserver;
import android.widget.FrameLayout;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.FloatBuffer;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Iterator;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * A Prism 3D scene (prism-core `scene3d`), drawn with OpenGL ES 3 in a
 * transparent TextureView (so it composes inside scroll views, clips and
 * fades like any view) on its own GL thread.
 *
 * Threads: updates and gestures arrive on the UI thread, frames are drawn on
 * the GL thread; both touch the {@link PrismScenePlayer} only while holding
 * {@link #lock}. Each frame is copied out of the player under the lock, then
 * drawn without it. Blobs (meshes, instances, clips, skins, environments,
 * embedded images) are fetched once per process by id on a loader thread
 * ({@link Store}, JNI `PrismHost.sceneBlob`) and decoded there; each GL
 * context uploads them once (cached by mesh key, blob id, texture source).
 *
 * Frames are drawn on demand (an update, a gesture, a size, appearance,
 * blob or texture change) and follow the display (Choreographer) only while
 * the scene animates, and not while the view is hidden or scrolled away. The
 * GL thread and its EGL context live while the TextureView has a surface
 * (attached to a window); the player outlives them.
 *
 * The container (a FrameLayout around the TextureView) takes the app's
 * background, border and corners: TextureViews have no background.
 */
final class PrismScene extends FrameLayout implements TextureView.SurfaceTextureListener {
    private final long id;
    private final float density;
    private final TextureView texture;
    private final ScaleGestureDetector scale;

    /** Guards {@link #player}. */
    private final Object lock = new Object();
    /** Null until the first update. */
    private PrismScenePlayer player;
    /** The scene's settings (UI thread: what gestures may do; replaced by updates, never changed). */
    private PrismScenePlayer.Settings settings;
    /** Bumped by each update: the GL thread then drops GPU objects the scene no longer uses. */
    private volatile int version;
    private volatile boolean dark;
    /** Not hidden (the view and its ancestors visible). */
    private volatile boolean shown = true;
    /** Some of it is on screen (not scrolled away). */
    private volatile boolean onscreen = true;
    private Renderer renderer;
    private final AtomicBoolean arrivalPosted = new AtomicBoolean();

    // Touches (UI thread), pixels.
    private float downX, downY, lastX, lastY, lastCx, lastCy;
    private boolean tapping, resetCentroid, resetSingle;
    private final Rect visible = new Rect();

    PrismScene(Context context, long id) {
        super(context);
        this.id = id;
        density = context.getResources().getDisplayMetrics().density;
        dark = PrismHost.dark();
        texture = new TextureView(context);
        texture.setOpaque(false);
        texture.setSurfaceTextureListener(this);
        texture.setImportantForAccessibility(IMPORTANT_FOR_ACCESSIBILITY_NO);
        addView(texture, new LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT));
        scale = new ScaleGestureDetector(context, new ScaleGestureDetector.SimpleOnScaleGestureListener() {
            @Override
            public boolean onScale(ScaleGestureDetector d) {
                synchronized (lock) {
                    if (player != null) player.zoom(d.getScaleFactor());
                }
                requestRender();
                return true;
            }
        });
        // A double tap then drag would zoom too: taps are the app's.
        scale.setQuickScaleEnabled(false);
    }

    /** Op `scene`: an update of the scene (`SceneUpdate`'s JSON, the first one full). */
    void setScene(String json) {
        PrismScenePlayer.Update update;
        try {
            update = PrismScenePlayer.Update.parse(json);
        } catch (RuntimeException e) {
            android.util.Log.w("prism", "bad scene update", e);
            return;
        }
        if (update == null) return;
        // Blobs that were missing may exist now.
        Store.shared.retry();
        double now = now();
        Set<String> used;
        synchronized (lock) {
            if (player == null) player = new PrismScenePlayer(Store.shared, now);
            player.apply(update, now);
            settings = player.settings();
            used = player.usedBlobs();
        }
        if (isAttachedToWindow()) Store.shared.use(this, used);
        version++;
        dark = PrismHost.dark();
        PrismScenePlayer.OrbitSpec o = settings.orbit;
        // Remotes turn the scene (D-pad left and right), watch crowns zoom it:
        // both reach the focused view.
        boolean keys = o != null && (PrismHost.tv && o.rotate || PrismHost.watch && o.zoom);
        setFocusable(keys);
        if (PrismHost.watch) {
            setFocusableInTouchMode(keys);
            if (keys && isAttachedToWindow() && getRootView().findFocus() == null) requestFocus();
        }
        requestRender();
    }

    /** Light/dark changed: adaptive colors again. */
    void restyle() {
        dark = PrismHost.dark();
        requestRender();
    }

    /** Blobs arrived (main thread): clips start, skins resolve, meshes draw. */
    private void blobsArrived() {
        arrivalPosted.set(false);
        synchronized (lock) {
            if (player == null) return;
            player.blobsArrived(now());
        }
        requestRender();
    }

    private static double now() {
        return System.nanoTime() / 1e9;
    }

    private void requestRender() {
        Renderer r = renderer;
        if (r != null) r.request();
    }

    // --- Visibility -----------------------------------------------------------------------

    @Override
    public void onVisibilityAggregated(boolean isVisible) {
        super.onVisibilityAggregated(isVisible);
        shown = isVisible;
        if (isVisible) requestRender();
    }

    private final ViewTreeObserver.OnScrollChangedListener scrolled = this::checkOnscreen;
    private final ViewTreeObserver.OnGlobalLayoutListener laidOut = this::checkOnscreen;

    private void checkOnscreen() {
        boolean on = getGlobalVisibleRect(visible);
        if (on == onscreen) return;
        onscreen = on;
        if (on) requestRender();
    }

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        getViewTreeObserver().addOnScrollChangedListener(scrolled);
        getViewTreeObserver().addOnGlobalLayoutListener(laidOut);
        Set<String> used = null;
        synchronized (lock) {
            if (player != null) used = player.usedBlobs();
        }
        if (used != null) {
            Store.shared.use(this, used);
            // Blobs that arrived while it was not attached (the first update comes before).
            if (arrivalPosted.compareAndSet(false, true)) PrismHost.main.post(this::blobsArrived);
        }
    }

    @Override
    protected void onDetachedFromWindow() {
        getViewTreeObserver().removeOnScrollChangedListener(scrolled);
        getViewTreeObserver().removeOnGlobalLayoutListener(laidOut);
        Store.shared.drop(this);
        super.onDetachedFromWindow();
    }

    // --- The TextureView's surface ------------------------------------------------------

    @Override
    public void onSurfaceTextureAvailable(SurfaceTexture surface, int width, int height) {
        renderer = new Renderer(surface);
        renderer.request();
    }

    @Override
    public void onSurfaceTextureSizeChanged(SurfaceTexture surface, int width, int height) {
        requestRender();
    }

    @Override
    public boolean onSurfaceTextureDestroyed(SurfaceTexture surface) {
        // The GL thread releases the surface once EGL no longer draws into it.
        if (renderer != null) renderer.stop();
        renderer = null;
        return false;
    }

    @Override
    public void onSurfaceTextureUpdated(SurfaceTexture surface) {
    }

    // --- Input ----------------------------------------------------------------------------

    @Override
    public boolean dispatchTouchEvent(MotionEvent e) {
        boolean mine = touch(e);
        // The app's own listeners (pointer events, states) still see the touch.
        return super.dispatchTouchEvent(e) | mine;
    }

    /**
     * One finger turns, two pinch (zoom) and move (pan), a press that moved
     * less than TAP_SLOP is a tap. Without an orbit only taps are taken
     * (when the app listens), so scroll views still scroll.
     */
    private boolean touch(MotionEvent e) {
        PrismScenePlayer.Settings s = settings;
        if (s == null || s.orbit == null && !s.tap) return false;
        PrismScenePlayer.OrbitSpec o = s.orbit;
        if (o != null && o.zoom) scale.onTouchEvent(e);
        switch (e.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                downX = lastX = e.getX();
                downY = lastY = e.getY();
                tapping = true;
                resetSingle = false;
                synchronized (lock) {
                    if (player != null) player.pointerDown();
                }
                // Scroll views must not take the drag away from the orbit.
                if (o != null && o.rotate) disallowIntercept();
                requestRender();
                return true;
            case MotionEvent.ACTION_POINTER_DOWN:
                tapping = false;
                resetCentroid = true;
                if (o != null && (o.zoom || o.pan)) disallowIntercept();
                return true;
            case MotionEvent.ACTION_POINTER_UP:
                resetCentroid = true;
                resetSingle = true;
                return true;
            case MotionEvent.ACTION_MOVE: {
                if (e.getPointerCount() >= 2) {
                    float cx = 0, cy = 0;
                    int count = e.getPointerCount();
                    for (int i = 0; i < count; i++) {
                        cx += e.getX(i) / count;
                        cy += e.getY(i) / count;
                    }
                    if (!resetCentroid && o != null && o.pan) {
                        synchronized (lock) {
                            if (player != null) player.pan((cx - lastCx) / density, (cy - lastCy) / density, texture.getHeight() / density);
                        }
                        requestRender();
                    }
                    resetCentroid = false;
                    lastCx = cx;
                    lastCy = cy;
                    return true;
                }
                float x = e.getX(), y = e.getY();
                if (resetSingle) {
                    // Back to one finger after a pinch: no jump.
                    resetSingle = false;
                } else if (o != null && o.rotate) {
                    synchronized (lock) {
                        if (player != null) player.drag((x - lastX) / density, (y - lastY) / density);
                    }
                    requestRender();
                }
                lastX = x;
                lastY = y;
                float dx = x - downX, dy = y - downY, slop = PrismScenePlayer.TAP_SLOP * density;
                if (dx * dx + dy * dy >= slop * slop) tapping = false;
                return true;
            }
            case MotionEvent.ACTION_UP: {
                PrismScenePlayer.Tap hit = null;
                synchronized (lock) {
                    if (player != null) {
                        player.pointerUp();
                        if (tapping && s.tap) {
                            hit = player.hit((e.getX() - texture.getLeft()) / density, (e.getY() - texture.getTop()) / density,
                                    texture.getWidth() / density, texture.getHeight() / density, dark);
                        }
                    }
                }
                if (hit != null) sendTap(hit);
                tapping = false;
                requestRender();
                return true;
            }
            case MotionEvent.ACTION_CANCEL:
                synchronized (lock) {
                    if (player != null) player.pointerUp();
                }
                tapping = false;
                requestRender();
                return true;
        }
        return true;
    }

    private void disallowIntercept() {
        if (getParent() != null) getParent().requestDisallowInterceptTouchEvent(true);
    }

    /** `{"t":"scene_tap","id","node","part","instance","point"}`. */
    private void sendTap(PrismScenePlayer.Tap hit) {
        JSONObject ev = PrismHost.event("scene_tap", id);
        try {
            ev.put("node", hit.node != null ? hit.node : JSONObject.NULL);
            if (hit.part != null) ev.put("part", hit.part);
            if (hit.instance >= 0) ev.put("instance", hit.instance);
            if (hit.point != null) {
                JSONArray p = new JSONArray();
                for (float v : hit.point) p.put((double) v);
                ev.put("point", p);
            } else {
                ev.put("point", JSONObject.NULL);
            }
        } catch (JSONException ignored) {
        }
        PrismHost.send(ev);
    }

    /** Mouse wheels and watch crowns zoom (like the web's `exp(-deltaY * 0.002)`, deltaY in points). */
    @Override
    public boolean onGenericMotionEvent(MotionEvent e) {
        PrismScenePlayer.Settings s = settings;
        if (e.getActionMasked() != MotionEvent.ACTION_SCROLL || s == null || s.orbit == null || !s.orbit.zoom) {
            return super.onGenericMotionEvent(e);
        }
        float factor = ViewConfiguration.get(getContext()).getScaledVerticalScrollFactor();
        float axis = e.isFromSource(InputDevice.SOURCE_ROTARY_ENCODER)
                ? e.getAxisValue(MotionEvent.AXIS_SCROLL) : e.getAxisValue(MotionEvent.AXIS_VSCROLL);
        float deltaY = -axis * factor / density;
        if (deltaY == 0) return super.onGenericMotionEvent(e);
        synchronized (lock) {
            if (player != null) player.zoom((float) Math.exp(-deltaY * 0.002));
        }
        requestRender();
        return true;
    }

    /** Android TV: D-pad left and right turn the scene (up and down still move the focus). */
    @Override
    public boolean onKeyDown(int keyCode, KeyEvent e) {
        PrismScenePlayer.Settings s = settings;
        boolean turn = keyCode == KeyEvent.KEYCODE_DPAD_LEFT || keyCode == KeyEvent.KEYCODE_DPAD_RIGHT;
        if (!turn || s == null || s.orbit == null || !s.orbit.rotate) return super.onKeyDown(keyCode, e);
        synchronized (lock) {
            if (player != null) player.drag(keyCode == KeyEvent.KEYCODE_DPAD_RIGHT ? 30 : -30, 0);
        }
        requestRender();
        return true;
    }

    // --- Blobs (process-wide) -------------------------------------------------------------

    /** Straight (not premultiplied) RGBA8 pixels, row 0 the image's top. */
    private static final class Image {
        final int width, height;
        final ByteBuffer rgba;

        Image(int width, int height, ByteBuffer rgba) {
            this.width = width;
            this.height = height;
            this.rgba = rgba;
        }
    }

    /**
     * Blobs by id, fetched once per process on a loader thread and decoded
     * there, shared by every scene. A blob the app no longer holds (JNI null) is
     * asked for again after the next update. Decoded blobs that no attached scene
     * uses are dropped. Embedded images are decoded per request (only the GPU
     * keeps their pixels).
     */
    private static final class Store implements PrismScenePlayer.Blobs {
        static final Store shared = new Store();
        /** Largest texture side (the GL contexts' limit, at most 4096). */
        static volatile int maxTexture = 4096;

        private final ConcurrentHashMap<String, Object> decoded = new ConcurrentHashMap<>();
        private final Set<String> asked = ConcurrentHashMap.newKeySet(), missing = ConcurrentHashMap.newKeySet();
        private final Handler loader;
        /** Attached scenes and the blobs each uses (main thread). */
        private final Map<PrismScene, Set<String>> scenes = new HashMap<>();

        private Store() {
            HandlerThread t = new HandlerThread("PrismSceneLoader", Process.THREAD_PRIORITY_BACKGROUND);
            t.start();
            loader = new Handler(t.getLooper());
        }

        /** The decoded blob, or null while it loads (any thread). */
        @Override
        public Object get(String id) {
            Object v = decoded.get(id);
            if (v == null && asked.add(id)) loader.post(() -> load(id));
            return v;
        }

        private void load(String id) {
            Object v = PrismScenePlayer.decodeBlob(fetch(id));
            if (v == null) {
                missing.add(id);
                return;
            }
            decoded.put(id, v);
            PrismHost.main.post(this::arrived);
        }

        private static byte[] fetch(String id) {
            try {
                return PrismHost.sceneBlob(id);
            } catch (Throwable e) {
                // OutOfMemoryError for a huge blob: drawn without it.
                android.util.Log.w("prism", "scene3d blob " + id, e);
                return null;
            }
        }

        /** Tells the scenes (main thread), once per batch each. */
        private void arrived() {
            for (PrismScene s : scenes.keySet()) {
                if (s.arrivalPosted.compareAndSet(false, true)) PrismHost.main.post(s::blobsArrived);
            }
        }

        /** Missing blobs are asked for again (an update may have brought them back). */
        void retry() {
            if (missing.isEmpty()) return;
            asked.removeAll(missing);
            missing.clear();
        }

        /** `scene` uses `ids` now (main thread). */
        void use(PrismScene scene, Set<String> ids) {
            Set<String> before = scenes.put(scene, ids);
            if (before != null && !ids.containsAll(before)) prune();
        }

        /** `scene` left the window (main thread). */
        void drop(PrismScene scene) {
            if (scenes.remove(scene) != null) prune();
        }

        private void prune() {
            Set<String> all = new HashSet<>();
            for (Set<String> s : scenes.values()) all.addAll(s);
            decoded.keySet().retainAll(all);
            asked.retainAll(all);
        }

        /** Loads texture source `src` for `r` (a `p3d:` image blob, else a picture source); `r.arrived` gets it. */
        void image(String src, Renderer r) {
            if (src.startsWith("p3d:")) {
                loader.post(() -> {
                    byte[] bytes = fetch(src.substring(4));
                    if (bytes == null) return;
                    BitmapFactory.Options o = new BitmapFactory.Options();
                    // Straight alpha: the shader multiplies the color by it.
                    o.inPremultiplied = false;
                    o.inPreferredConfig = Bitmap.Config.ARGB_8888;
                    Bitmap b = BitmapFactory.decodeByteArray(bytes, 0, bytes.length, o);
                    if (b != null) r.arrived(src, pixels(b));
                });
            } else {
                // Loaded like `picture()` sources (app assets, files, http(s), data: URLs), on the UI thread.
                PrismHost.main.post(() -> PrismHost.bitmap(src, b -> {
                    if (b != null) loader.post(() -> r.arrived(src, pixels(b)));
                }));
            }
        }

        /** A bitmap's straight RGBA (scaled down past maxTexture). */
        private static Image pixels(Bitmap b) {
            int max = maxTexture;
            if (b.getWidth() > max || b.getHeight() > max) {
                float k = (float) max / Math.max(b.getWidth(), b.getHeight());
                b = Bitmap.createScaledBitmap(b, Math.max(1, Math.round(b.getWidth() * k)), Math.max(1, Math.round(b.getHeight() * k)), true);
            }
            if (b.getConfig() != Bitmap.Config.ARGB_8888) b = b.copy(Bitmap.Config.ARGB_8888, false);
            int w = b.getWidth(), h = b.getHeight();
            ByteBuffer rgba = ByteBuffer.allocateDirect(w * h * 4).order(ByteOrder.nativeOrder());
            if (!b.isPremultiplied()) {
                // ARGB_8888 is stored R, G, B, A.
                b.copyPixelsToBuffer(rgba);
            } else {
                int[] argb = new int[w * h];
                // getPixels un-premultiplies.
                b.getPixels(argb, 0, w, 0, 0, w, h);
                for (int c : argb) rgba.put((byte) (c >> 16)).put((byte) (c >> 8)).put((byte) c).put((byte) (c >>> 24));
            }
            rgba.position(0);
            return new Image(w, h, rgba);
        }
    }

    // --- Drawing (GL thread) ------------------------------------------------------------

    /** A linked program and its uniforms (`shaders.rs`' table). */
    private static final class Program {
        final int id;
        final int viewProj, camera, ambient, lightCount, lightKind, lightColor, lightPosition, lightDirection,
                lightParams, exposure, hasEnv, envLevels, envSh, envIntensity, hasShadow, shadowViewProj,
                shadowLight, shadowBias, shadowNormalBias, shadowTexel, model, normal, color, emissive,
                metalness, roughness, unlit, alphaMode, alphaCutoff, receiveShadows, hasTexture, hasMrMap,
                hasNormalMap, normalScale, hasEmissiveMap, hasAoMap, inverseViewProj;
        /** The frame whose per-frame uniforms it holds. */
        int frame = -1;

        Program(String vertex, String fragment) {
            id = link(vertex, fragment);
            viewProj = at("u_view_proj");
            camera = at("u_camera");
            ambient = at("u_ambient");
            lightCount = at("u_light_count");
            lightKind = at("u_light_kind");
            lightColor = at("u_light_color");
            lightPosition = at("u_light_position");
            lightDirection = at("u_light_direction");
            lightParams = at("u_light_params");
            exposure = at("u_exposure");
            hasEnv = at("u_has_env");
            envLevels = at("u_env_levels");
            envSh = at("u_env_sh");
            envIntensity = at("u_env_intensity");
            hasShadow = at("u_has_shadow");
            shadowViewProj = at("u_shadow_view_proj");
            shadowLight = at("u_shadow_light");
            shadowBias = at("u_shadow_bias");
            shadowNormalBias = at("u_shadow_normal_bias");
            shadowTexel = at("u_shadow_texel");
            model = at("u_model");
            normal = at("u_normal_matrix");
            color = at("u_color");
            emissive = at("u_emissive");
            metalness = at("u_metalness");
            roughness = at("u_roughness");
            unlit = at("u_unlit");
            alphaMode = at("u_alpha_mode");
            alphaCutoff = at("u_alpha_cutoff");
            receiveShadows = at("u_receive_shadows");
            hasTexture = at("u_has_texture");
            hasMrMap = at("u_has_mr_map");
            hasNormalMap = at("u_has_normal_map");
            normalScale = at("u_normal_scale");
            hasEmissiveMap = at("u_has_emissive_map");
            hasAoMap = at("u_has_ao_map");
            inverseViewProj = at("u_inverse_view_proj");
            // Every sampler its own unit, once (samplers of different types may not share one).
            GLES30.glUseProgram(id);
            String[] samplers = {"u_texture", "u_mr_map", "u_normal_map", "u_emissive_map", "u_ao_map", "u_env", "u_shadow_map", "u_bones"};
            for (int unit = 0; unit < samplers.length; unit++) {
                int loc = at(samplers[unit]);
                if (loc >= 0) GLES30.glUniform1i(loc, unit);
            }
        }

        private int at(String name) {
            return GLES30.glGetUniformLocation(id, name);
        }

        private static int link(String vertex, String fragment) {
            int vs = shader(GLES30.GL_VERTEX_SHADER, vertex), fs = shader(GLES30.GL_FRAGMENT_SHADER, fragment);
            int p = GLES30.glCreateProgram();
            GLES30.glAttachShader(p, vs);
            GLES30.glAttachShader(p, fs);
            GLES30.glLinkProgram(p);
            int[] ok = new int[1];
            GLES30.glGetProgramiv(p, GLES30.GL_LINK_STATUS, ok, 0);
            if (ok[0] == 0) throw new RuntimeException("scene3d program: " + GLES30.glGetProgramInfoLog(p));
            GLES30.glDeleteShader(vs);
            GLES30.glDeleteShader(fs);
            return p;
        }

        private static int shader(int type, String source) {
            int s = GLES30.glCreateShader(type);
            GLES30.glShaderSource(s, source);
            GLES30.glCompileShader(s);
            int[] ok = new int[1];
            GLES30.glGetShaderiv(s, GLES30.GL_COMPILE_STATUS, ok, 0);
            if (ok[0] == 0) throw new RuntimeException("scene3d shader: " + GLES30.glGetShaderInfoLog(s));
            return s;
        }
    }

    /** A mesh's buffers on the GPU: positions, normals, uvs, colors, joints, weights (0: absent), indices. */
    private static final class GpuMesh {
        int vao, count;
        final int[] buffers = new int[7];
        boolean skin;
        /** Its vertex arrays with an instance set's attributes, by `I` blob. */
        final Map<String, Integer> instanced = new HashMap<>();
    }

    /** An instance set's per-instance buffers: matrices, colors (0: none). */
    private static final class GpuInstances {
        final int[] buffers = new int[2];
    }

    /** An environment: its mipmapped RGBA16F texture, level count and SH. */
    private static final class GpuEnv {
        int texture;
        float levels;
        float[] sh;
    }

    private static final int UNIT_ENV = 5, UNIT_SHADOW = 6, UNIT_BONES = 7;
    /** EXT_texture_filter_anisotropic. */
    private static final int TEXTURE_MAX_ANISOTROPY = 0x84FE, MAX_TEXTURE_MAX_ANISOTROPY = 0x84FF;

    /** The GL thread: an EGL context drawing into the TextureView's surface. */
    private final class Renderer implements Choreographer.FrameCallback {
        private final SurfaceTexture surface;
        private final HandlerThread thread = new HandlerThread("PrismScene");
        private final Handler handler;
        private final AtomicBoolean requested = new AtomicBoolean();
        // GL thread only.
        private Choreographer choreographer;
        private EGLDisplay display;
        private EGLContext context;
        private EGLSurface eglSurface;
        private boolean pending, broken;
        private int drawnVersion = -1, frameNumber;
        private float anisotropy;
        /** By variant: instanced × 4 + skinned × 2 + depth only. */
        private final Program[] programs = new Program[8];
        private Program sky;
        private int skyVao, shadowFbo, shadowMap, noShadow;
        private final PrismScenePlayer.Frame frame = new PrismScenePlayer.Frame();
        private final Map<String, GpuMesh> meshes = new HashMap<>();
        private final Map<String, GpuInstances> instanceSets = new HashMap<>();
        private final Map<String, Integer> textures = new HashMap<>();
        private final Map<String, GpuEnv> envs = new HashMap<>();
        /** Texture sources asked for (loading, loaded or failed): each is asked for once. */
        private final Set<String> asked = new HashSet<>();
        /** Pictures decoded on the loader thread, waiting for an upload: {source, Image}. */
        private final ConcurrentLinkedQueue<Object[]> arrived = new ConcurrentLinkedQueue<>();
        /** Joint matrix textures (one per skinned draw of a frame) and their rows. */
        private int[] bones = new int[0], bonesRows = new int[0];
        private FloatBuffer bonesData = FloatBuffer.allocate(0);
        // Per frame, by draw index: its mesh and its joints' texture (0: none).
        private GpuMesh[] drawMesh = new GpuMesh[0];
        private int[] drawBones = new int[0];
        // Per-frame uniforms, packed once.
        private final int[] lightKinds = new int[PrismScenePlayer.MAX_LIGHTS];
        private final float[] lightColors = new float[PrismScenePlayer.MAX_LIGHTS * 3],
                lightPositions = new float[PrismScenePlayer.MAX_LIGHTS * 3],
                lightDirections = new float[PrismScenePlayer.MAX_LIGHTS * 3],
                lightParams = new float[PrismScenePlayer.MAX_LIGHTS * 4];
        /** The texture bound to each unit 0–4 (skips rebinding). */
        private final int[] bound = new int[5];

        Renderer(SurfaceTexture surface) {
            this.surface = surface;
            thread.start();
            handler = new Handler(thread.getLooper());
            handler.post(this::start);
        }

        /** Draws a frame at the next vsync (any thread). */
        void request() {
            if (requested.compareAndSet(false, true)) {
                handler.post(() -> {
                    requested.set(false);
                    schedule();
                });
            }
        }

        void stop() {
            handler.post(() -> {
                release();
                surface.release();
                thread.quitSafely();
            });
        }

        /** A texture source's pixels arrived (loader thread). */
        void arrived(String src, Image image) {
            arrived.add(new Object[]{src, image});
            request();
        }

        private void schedule() {
            if (pending || broken || context == null) return;
            pending = true;
            choreographer.postFrameCallback(this);
        }

        @Override
        public void doFrame(long frameTimeNanos) {
            pending = false;
            // Hidden or scrolled away: no frames until it shows again.
            if (!shown || !onscreen) return;
            try {
                if (draw()) schedule();
            } catch (RuntimeException e) {
                android.util.Log.e("prism", "scene3d frame", e);
                broken = true;
            }
        }

        private void start() {
            choreographer = Choreographer.getInstance();
            try {
                display = EGL14.eglGetDisplay(EGL14.EGL_DEFAULT_DISPLAY);
                int[] v = new int[2];
                if (!EGL14.eglInitialize(display, v, 0, v, 1)) throw new RuntimeException("eglInitialize");
                EGLConfig config = config();
                if (config == null) throw new RuntimeException("no RGBA8888 OpenGL ES 3 config");
                context = EGL14.eglCreateContext(display, config, EGL14.EGL_NO_CONTEXT,
                        new int[]{EGL14.EGL_CONTEXT_CLIENT_VERSION, 3, EGL14.EGL_NONE}, 0);
                if (context == null || context == EGL14.EGL_NO_CONTEXT) throw new RuntimeException("eglCreateContext");
                eglSurface = EGL14.eglCreateWindowSurface(display, config, surface, new int[]{EGL14.EGL_NONE}, 0);
                if (eglSurface == null || eglSurface == EGL14.EGL_NO_SURFACE) throw new RuntimeException("eglCreateWindowSurface");
                if (!EGL14.eglMakeCurrent(display, eglSurface, eglSurface, context)) throw new RuntimeException("eglMakeCurrent");
                setup();
            } catch (RuntimeException e) {
                android.util.Log.e("prism", "scene3d: OpenGL ES 3 unavailable (EGL error 0x"
                        + Integer.toHexString(EGL14.eglGetError()) + ")", e);
                broken = true;
            }
        }

        /** RGBA8888 with depth, 4× MSAA when the GPU has it. */
        private EGLConfig config() {
            int[][] wanted = {{4, 24}, {4, 16}, {0, 24}, {0, 16}};
            for (int[] w : wanted) {
                int[] attrs = {
                        EGL14.EGL_RED_SIZE, 8, EGL14.EGL_GREEN_SIZE, 8, EGL14.EGL_BLUE_SIZE, 8, EGL14.EGL_ALPHA_SIZE, 8,
                        EGL14.EGL_DEPTH_SIZE, w[1],
                        EGL14.EGL_RENDERABLE_TYPE, EGLExt.EGL_OPENGL_ES3_BIT_KHR,
                        EGL14.EGL_SURFACE_TYPE, EGL14.EGL_WINDOW_BIT,
                        EGL14.EGL_SAMPLE_BUFFERS, w[0] > 0 ? 1 : 0, EGL14.EGL_SAMPLES, w[0],
                        EGL14.EGL_NONE};
                EGLConfig[] configs = new EGLConfig[16];
                int[] count = new int[1];
                if (!EGL14.eglChooseConfig(display, attrs, 0, configs, 0, configs.length, count, 0)) continue;
                // At least 8 bits each matches 10-bit configs too: take an exact RGBA8888.
                for (int i = 0; i < count[0]; i++) {
                    if (attr(configs[i], EGL14.EGL_RED_SIZE) == 8 && attr(configs[i], EGL14.EGL_GREEN_SIZE) == 8
                            && attr(configs[i], EGL14.EGL_BLUE_SIZE) == 8 && attr(configs[i], EGL14.EGL_ALPHA_SIZE) == 8) {
                        return configs[i];
                    }
                }
            }
            return null;
        }

        private int attr(EGLConfig c, int name) {
            int[] v = new int[1];
            EGL14.eglGetConfigAttrib(display, c, name, v, 0);
            return v[0];
        }

        /** Context-wide state: limits, constant attributes, the empty shadow map. */
        private void setup() {
            int[] v = new int[1];
            GLES30.glGetIntegerv(GLES30.GL_MAX_TEXTURE_SIZE, v, 0);
            if (v[0] > 0) Store.maxTexture = Math.min(Store.maxTexture, v[0]);
            String extensions = GLES30.glGetString(GLES30.GL_EXTENSIONS);
            if (extensions != null && extensions.contains("GL_EXT_texture_filter_anisotropic")) {
                float[] max = new float[1];
                GLES30.glGetFloatv(MAX_TEXTURE_MAX_ANISOTROPY, max, 0);
                anisotropy = Math.min(4, max[0]);
            }
            // Absent attributes: uv (0, 0), white vertex and instance colors
            // (current values are context state, not vertex-array state).
            GLES30.glVertexAttrib4f(2, 0, 0, 0, 1);
            GLES30.glVertexAttrib4f(3, 1, 1, 1, 1);
            GLES30.glVertexAttrib4f(10, 1, 1, 1, 1);
            // Unit 6 always holds a depth texture with compare mode (the sampler2DShadow's).
            noShadow = depthTexture(1);
            GLES30.glFrontFace(GLES30.GL_CCW);
            GLES30.glCullFace(GLES30.GL_BACK);
            GLES30.glDepthFunc(GLES30.GL_LEQUAL);
        }

        private int depthTexture(int size) {
            int[] t = new int[1];
            GLES30.glGenTextures(1, t, 0);
            GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, t[0]);
            GLES30.glTexImage2D(GLES30.GL_TEXTURE_2D, 0, GLES30.GL_DEPTH_COMPONENT24, size, size, 0,
                    GLES30.GL_DEPTH_COMPONENT, GLES30.GL_UNSIGNED_INT, null);
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_COMPARE_MODE, GLES30.GL_COMPARE_REF_TO_TEXTURE);
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_COMPARE_FUNC, GLES30.GL_LEQUAL);
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_MIN_FILTER, GLES30.GL_LINEAR);
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_MAG_FILTER, GLES30.GL_LINEAR);
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_WRAP_S, GLES30.GL_CLAMP_TO_EDGE);
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_WRAP_T, GLES30.GL_CLAMP_TO_EDGE);
            return t[0];
        }

        private Program program(boolean instanced, boolean skinned, boolean depthOnly) {
            int k = (instanced ? 4 : 0) + (skinned ? 2 : 0) + (depthOnly ? 1 : 0);
            Program p = programs[k];
            if (p == null) {
                String h = PrismScenePlayer.Shaders.ES_HEADER;
                p = programs[k] = new Program(
                        PrismScenePlayer.Shaders.programSource(h, false, instanced, skinned, depthOnly),
                        PrismScenePlayer.Shaders.programSource(h, true, instanced, skinned, depthOnly));
            }
            return p;
        }

        /** One frame; true while the scene animates. */
        private boolean draw() {
            int[] size = sizeScratch;
            EGL14.eglQuerySurface(display, eglSurface, EGL14.EGL_WIDTH, size, 0);
            EGL14.eglQuerySurface(display, eglSurface, EGL14.EGL_HEIGHT, size, 1);
            int pw = size[0], ph = size[1];
            if (pw <= 0 || ph <= 0) return false;
            for (Object[] a; (a = arrived.poll()) != null; ) upload((String) a[0], (Image) a[1]);
            boolean have = false;
            int sceneVersion = version;
            Set<String> blobs = null, sources = null, meshKeys = null;
            synchronized (lock) {
                if (player != null) {
                    player.advance(now());
                    player.frame(pw / density, ph / density, dark, frame);
                    have = true;
                    if (sceneVersion != drawnVersion) {
                        blobs = player.usedBlobs();
                        sources = player.usedTextures();
                        meshKeys = player.usedMeshKeys();
                    }
                }
            }
            if (blobs != null) forget(blobs, sources, meshKeys);
            drawnVersion = sceneVersion;
            frameNumber++;
            if (have) {
                render(pw, ph);
            } else {
                GLES30.glViewport(0, 0, pw, ph);
                GLES30.glClearColor(0, 0, 0, 0);
                GLES30.glClear(GLES30.GL_COLOR_BUFFER_BIT | GLES30.GL_DEPTH_BUFFER_BIT);
            }
            if (!EGL14.eglSwapBuffers(display, eglSurface)) {
                android.util.Log.w("prism", "scene3d: eglSwapBuffers 0x" + Integer.toHexString(EGL14.eglGetError()));
            }
            return have && frame.animating;
        }

        private final int[] sizeScratch = new int[2];

        private void render(int pw, int ph) {
            PrismScenePlayer.Frame f = frame;
            int n = f.drawCount;
            if (drawMesh.length < n) {
                drawMesh = new GpuMesh[Math.max(n, drawMesh.length * 2)];
                drawBones = new int[drawMesh.length];
            }
            // What each draw needs on the GPU (blobs still loading: not drawn yet).
            int skinnedDraws = 0;
            for (int i = 0; i < n; i++) {
                PrismScenePlayer.Draw d = f.draws[i];
                GpuMesh g = mesh(d);
                if (g != null && d.instanced() && instances(d.instances) == null) g = null;
                drawMesh[i] = g;
                drawBones[i] = g != null && g.skin && d.skinned() ? bones(skinnedDraws++, d) : 0;
            }
            GpuEnv env = f.env != null ? env(f.env) : null;
            for (int i = 0; i < f.lightCount; i++) {
                PrismScenePlayer.FrameLight l = f.lights[i];
                lightKinds[i] = l.kind;
                System.arraycopy(l.color, 0, lightColors, i * 3, 3);
                System.arraycopy(l.position, 0, lightPositions, i * 3, 3);
                System.arraycopy(l.direction, 0, lightDirections, i * 3, 3);
                System.arraycopy(l.params, 0, lightParams, i * 4, 4);
            }
            boolean shadow = f.hasShadow && shadowPass(f);

            GLES30.glBindFramebuffer(GLES30.GL_FRAMEBUFFER, 0);
            GLES30.glViewport(0, 0, pw, ph);
            GLES30.glDepthMask(true);
            GLES30.glClearColor(f.clear[0], f.clear[1], f.clear[2], f.clear[3]);
            GLES30.glClear(GLES30.GL_COLOR_BUFFER_BIT | GLES30.GL_DEPTH_BUFFER_BIT);
            GLES30.glActiveTexture(GLES30.GL_TEXTURE0 + UNIT_ENV);
            GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, env != null ? env.texture : 0);
            GLES30.glActiveTexture(GLES30.GL_TEXTURE0 + UNIT_SHADOW);
            GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, shadow ? shadowMap : noShadow);
            if (env != null && f.envBackground) sky(f, env);

            GLES30.glEnable(GLES30.GL_DEPTH_TEST);
            java.util.Arrays.fill(bound, -1);
            Program current = null;
            boolean blending = false, culling = false;
            GLES30.glDisable(GLES30.GL_BLEND);
            GLES30.glDisable(GLES30.GL_CULL_FACE);
            GLES30.glBlendFunc(GLES30.GL_ONE, GLES30.GL_ONE_MINUS_SRC_ALPHA);
            for (int i = 0; i < n; i++) {
                PrismScenePlayer.Draw d = f.draws[i];
                GpuMesh g = drawMesh[i];
                if (g == null || g.count == 0) continue;
                Program p = program(d.instanced(), drawBones[i] != 0, false);
                if (p != current) {
                    GLES30.glUseProgram(p.id);
                    current = p;
                    if (p.frame != frameNumber) frameUniforms(p, f, env, shadow);
                }
                if (d.transparent != blending) {
                    // Premultiplied output (the shader's), over what is behind, without hiding it.
                    blending = d.transparent;
                    if (blending) GLES30.glEnable(GLES30.GL_BLEND);
                    else GLES30.glDisable(GLES30.GL_BLEND);
                    GLES30.glDepthMask(!blending);
                }
                if (d.doubleSided == culling) {
                    culling = !d.doubleSided;
                    if (culling) GLES30.glEnable(GLES30.GL_CULL_FACE);
                    else GLES30.glDisable(GLES30.GL_CULL_FACE);
                }
                GLES30.glUniformMatrix4fv(p.model, 1, false, d.world, 0);
                GLES30.glUniformMatrix3fv(p.normal, 1, false, d.normal, 0);
                GLES30.glUniform4fv(p.color, 1, d.color, 0);
                GLES30.glUniform3fv(p.emissive, 1, d.emissive, 0);
                GLES30.glUniform1f(p.metalness, d.metalness);
                GLES30.glUniform1f(p.roughness, d.roughness);
                GLES30.glUniform1i(p.unlit, d.unlit ? 1 : 0);
                GLES30.glUniform1i(p.alphaMode, d.alpha);
                GLES30.glUniform1f(p.alphaCutoff, d.alphaCutoff);
                GLES30.glUniform1i(p.receiveShadows, d.receiveShadows ? 1 : 0);
                GLES30.glUniform1i(p.hasTexture, bindMap(0, d.texture));
                GLES30.glUniform1i(p.hasMrMap, bindMap(1, d.mrMap));
                GLES30.glUniform1i(p.hasNormalMap, bindMap(2, d.normalMap));
                GLES30.glUniform1f(p.normalScale, d.normalScale);
                GLES30.glUniform1i(p.hasEmissiveMap, bindMap(3, d.emissiveMap));
                GLES30.glUniform1i(p.hasAoMap, bindMap(4, d.aoMap));
                submit(d, g, drawBones[i]);
            }
            GLES30.glBindVertexArray(0);
            GLES30.glDisable(GLES30.GL_BLEND);
            GLES30.glDepthMask(true);
        }

        /** Binds the draw's mesh (with its instances) and joints, and draws it. */
        private void submit(PrismScenePlayer.Draw d, GpuMesh g, int bonesTexture) {
            if (bonesTexture != 0) {
                GLES30.glActiveTexture(GLES30.GL_TEXTURE0 + UNIT_BONES);
                GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, bonesTexture);
            }
            if (d.instanced()) {
                GLES30.glBindVertexArray(instancedVao(g, d.instances));
                GLES30.glDrawElementsInstanced(GLES30.GL_TRIANGLES, g.count, GLES30.GL_UNSIGNED_INT, 0, d.instanceCount);
            } else {
                GLES30.glBindVertexArray(g.vao);
                GLES30.glDrawElements(GLES30.GL_TRIANGLES, g.count, GLES30.GL_UNSIGNED_INT, 0);
            }
        }

        /** Binds texture source `src` to `unit` if loaded: 1, else 0 (asked for once). */
        private int bindMap(int unit, String src) {
            int t = 0;
            if (src != null) {
                Integer tex = textures.get(src);
                if (tex != null) t = tex;
                else if (asked.add(src)) Store.shared.image(src, this);
            }
            if (t != 0 && bound[unit] != t) {
                GLES30.glActiveTexture(GLES30.GL_TEXTURE0 + unit);
                GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, t);
                bound[unit] = t;
            }
            return t != 0 ? 1 : 0;
        }

        /** Camera, lights, environment and shadow: once per program per frame. */
        private void frameUniforms(Program p, PrismScenePlayer.Frame f, GpuEnv env, boolean shadow) {
            p.frame = frameNumber;
            GLES30.glUniformMatrix4fv(p.viewProj, 1, false, f.viewProj, 0);
            GLES30.glUniform3fv(p.camera, 1, f.cameraPosition, 0);
            GLES30.glUniform3fv(p.ambient, 1, f.ambient, 0);
            int n = f.lightCount;
            GLES30.glUniform1i(p.lightCount, n);
            if (n > 0) {
                GLES30.glUniform1iv(p.lightKind, n, lightKinds, 0);
                GLES30.glUniform3fv(p.lightColor, n, lightColors, 0);
                GLES30.glUniform3fv(p.lightPosition, n, lightPositions, 0);
                GLES30.glUniform3fv(p.lightDirection, n, lightDirections, 0);
                GLES30.glUniform4fv(p.lightParams, n, lightParams, 0);
            }
            GLES30.glUniform1f(p.exposure, f.exposure);
            GLES30.glUniform1i(p.hasEnv, env != null ? 1 : 0);
            if (env != null) {
                GLES30.glUniform1f(p.envLevels, env.levels);
                GLES30.glUniform3fv(p.envSh, 9, env.sh, 0);
                GLES30.glUniform1f(p.envIntensity, f.envIntensity);
            }
            GLES30.glUniform1i(p.hasShadow, shadow ? 1 : 0);
            if (shadow) {
                GLES30.glUniformMatrix4fv(p.shadowViewProj, 1, false, f.shadowViewProj, 0);
                GLES30.glUniform1i(p.shadowLight, f.shadowLight);
                GLES30.glUniform1f(p.shadowBias, f.shadowBias);
                GLES30.glUniform1f(p.shadowNormalBias, f.shadowNormalBias);
                GLES30.glUniform1f(p.shadowTexel, 1f / f.shadowMapSize);
            } else {
                GLES30.glUniform1i(p.shadowLight, -1);
            }
        }

        /** The casters' depth from the light into the shadow map; false if it could not be made. */
        private boolean shadowPass(PrismScenePlayer.Frame f) {
            int size = f.shadowMapSize;
            if (shadowFbo == 0) {
                shadowMap = depthTexture(size);
                int[] fbo = new int[1];
                GLES30.glGenFramebuffers(1, fbo, 0);
                shadowFbo = fbo[0];
                GLES30.glBindFramebuffer(GLES30.GL_FRAMEBUFFER, shadowFbo);
                GLES30.glFramebufferTexture2D(GLES30.GL_FRAMEBUFFER, GLES30.GL_DEPTH_ATTACHMENT, GLES30.GL_TEXTURE_2D, shadowMap, 0);
                GLES30.glDrawBuffers(1, new int[]{GLES30.GL_NONE}, 0);
                GLES30.glReadBuffer(GLES30.GL_NONE);
                if (GLES30.glCheckFramebufferStatus(GLES30.GL_FRAMEBUFFER) != GLES30.GL_FRAMEBUFFER_COMPLETE) {
                    android.util.Log.w("prism", "scene3d: no shadow map framebuffer");
                }
            }
            GLES30.glBindFramebuffer(GLES30.GL_FRAMEBUFFER, shadowFbo);
            if (GLES30.glCheckFramebufferStatus(GLES30.GL_FRAMEBUFFER) != GLES30.GL_FRAMEBUFFER_COMPLETE) return false;
            GLES30.glViewport(0, 0, size, size);
            GLES30.glDepthMask(true);
            GLES30.glClear(GLES30.GL_DEPTH_BUFFER_BIT);
            GLES30.glEnable(GLES30.GL_DEPTH_TEST);
            GLES30.glDisable(GLES30.GL_BLEND);
            Program current = null;
            for (int k = 0; k < f.casterCount; k++) {
                int i = f.casters[k];
                PrismScenePlayer.Draw d = f.draws[i];
                GpuMesh g = drawMesh[i];
                if (g == null || g.count == 0) continue;
                Program p = program(d.instanced(), drawBones[i] != 0, true);
                if (p != current) {
                    GLES30.glUseProgram(p.id);
                    current = p;
                    if (p.frame != frameNumber) {
                        p.frame = frameNumber;
                        GLES30.glUniformMatrix4fv(p.viewProj, 1, false, f.shadowViewProj, 0);
                    }
                }
                if (d.doubleSided) GLES30.glDisable(GLES30.GL_CULL_FACE);
                else GLES30.glEnable(GLES30.GL_CULL_FACE);
                GLES30.glUniformMatrix4fv(p.model, 1, false, d.world, 0);
                GLES30.glUniformMatrix3fv(p.normal, 1, false, d.normal, 0);
                submit(d, g, drawBones[i]);
            }
            GLES30.glBindVertexArray(0);
            return true;
        }

        /** The environment behind everything, after the clear. */
        private void sky(PrismScenePlayer.Frame f, GpuEnv env) {
            if (sky == null) {
                String h = PrismScenePlayer.Shaders.ES_HEADER;
                sky = new Program(h + PrismScenePlayer.Shaders.SKY_VERTEX, h + PrismScenePlayer.Shaders.SKY_FRAGMENT);
                int[] vao = new int[1];
                GLES30.glGenVertexArrays(1, vao, 0);
                skyVao = vao[0];
            }
            GLES30.glUseProgram(sky.id);
            GLES30.glUniformMatrix4fv(sky.inverseViewProj, 1, false, f.inverseViewProj, 0);
            GLES30.glUniform3fv(sky.camera, 1, f.cameraPosition, 0);
            GLES30.glUniform1f(sky.envIntensity, f.envIntensity);
            GLES30.glUniform1f(sky.exposure, f.exposure);
            GLES30.glDisable(GLES30.GL_DEPTH_TEST);
            GLES30.glDepthMask(false);
            GLES30.glDisable(GLES30.GL_CULL_FACE);
            GLES30.glDisable(GLES30.GL_BLEND);
            GLES30.glBindVertexArray(skyVao);
            GLES30.glDrawArrays(GLES30.GL_TRIANGLES, 0, 3);
            GLES30.glDepthMask(true);
        }

        /** The draw's buffers (uploaded once per mesh key); null while its blob loads. */
        private GpuMesh mesh(PrismScenePlayer.Draw d) {
            GpuMesh g = meshes.get(d.meshKey);
            if (g != null) return g;
            PrismScenePlayer.MeshData data = PrismScenePlayer.generate(d.mesh, Store.shared);
            if (data == null) return null;
            g = new GpuMesh();
            g.count = data.indices.length;
            g.skin = data.joints != null && data.weights != null;
            int[] b = g.buffers;
            b[0] = buffer(GLES30.GL_ARRAY_BUFFER, floats(data.positions), data.positions.length * 4);
            b[1] = buffer(GLES30.GL_ARRAY_BUFFER, floats(data.normals), data.normals.length * 4);
            if (data.uvs != null) b[2] = buffer(GLES30.GL_ARRAY_BUFFER, floats(data.uvs), data.uvs.length * 4);
            if (data.colors != null) b[3] = buffer(GLES30.GL_ARRAY_BUFFER, floats(data.colors), data.colors.length * 4);
            if (g.skin) {
                ByteBuffer j = ByteBuffer.allocateDirect(data.joints.length * 2).order(ByteOrder.nativeOrder());
                j.asShortBuffer().put(data.joints);
                b[4] = buffer(GLES30.GL_ARRAY_BUFFER, j, data.joints.length * 2);
                b[5] = buffer(GLES30.GL_ARRAY_BUFFER, floats(data.weights), data.weights.length * 4);
            }
            ByteBuffer idx = ByteBuffer.allocateDirect(Math.max(4, data.indices.length * 4)).order(ByteOrder.nativeOrder());
            idx.asIntBuffer().put(data.indices);
            b[6] = buffer(GLES30.GL_ELEMENT_ARRAY_BUFFER, idx, data.indices.length * 4);
            g.vao = vertexArray(g, null);
            meshes.put(d.meshKey, g);
            return g;
        }

        /** A vertex array of the mesh's attributes, and an instance set's (divisor 1) if given. */
        private int vertexArray(GpuMesh g, GpuInstances inst) {
            int[] vao = new int[1];
            GLES30.glGenVertexArrays(1, vao, 0);
            GLES30.glBindVertexArray(vao[0]);
            int[] b = g.buffers;
            attribute(0, b[0], 3);
            attribute(1, b[1], 3);
            if (b[2] != 0) attribute(2, b[2], 2);
            if (b[3] != 0) attribute(3, b[3], 4);
            if (b[4] != 0) {
                GLES30.glBindBuffer(GLES30.GL_ARRAY_BUFFER, b[4]);
                GLES30.glEnableVertexAttribArray(4);
                GLES30.glVertexAttribIPointer(4, 4, GLES30.GL_UNSIGNED_SHORT, 0, 0);
                attribute(5, b[5], 4);
            }
            if (inst != null) {
                GLES30.glBindBuffer(GLES30.GL_ARRAY_BUFFER, inst.buffers[0]);
                for (int c = 0; c < 4; c++) {
                    GLES30.glEnableVertexAttribArray(6 + c);
                    GLES30.glVertexAttribPointer(6 + c, 4, GLES30.GL_FLOAT, false, 64, c * 16);
                    GLES30.glVertexAttribDivisor(6 + c, 1);
                }
                if (inst.buffers[1] != 0) {
                    attribute(10, inst.buffers[1], 4);
                    GLES30.glVertexAttribDivisor(10, 1);
                }
            }
            GLES30.glBindBuffer(GLES30.GL_ELEMENT_ARRAY_BUFFER, b[6]);
            GLES30.glBindVertexArray(0);
            return vao[0];
        }

        private void attribute(int location, int buffer, int size) {
            GLES30.glBindBuffer(GLES30.GL_ARRAY_BUFFER, buffer);
            GLES30.glEnableVertexAttribArray(location);
            GLES30.glVertexAttribPointer(location, size, GLES30.GL_FLOAT, false, 0, 0);
        }

        private int buffer(int target, java.nio.Buffer data, int bytes) {
            int[] b = new int[1];
            GLES30.glGenBuffers(1, b, 0);
            GLES30.glBindBuffer(target, b[0]);
            GLES30.glBufferData(target, bytes, data, GLES30.GL_STATIC_DRAW);
            return b[0];
        }

        private FloatBuffer floats(float[] values) {
            FloatBuffer f = ByteBuffer.allocateDirect(Math.max(4, values.length * 4)).order(ByteOrder.nativeOrder()).asFloatBuffer();
            f.put(values).position(0);
            return f;
        }

        /** An instance set's buffers (uploaded once per `I` blob); null while it loads. */
        private GpuInstances instances(String id) {
            GpuInstances g = instanceSets.get(id);
            if (g != null) return g;
            Object blob = Store.shared.get(id);
            if (!(blob instanceof PrismScenePlayer.InstancesBlob)) return null;
            PrismScenePlayer.InstancesBlob set = (PrismScenePlayer.InstancesBlob) blob;
            g = new GpuInstances();
            g.buffers[0] = buffer(GLES30.GL_ARRAY_BUFFER, floats(set.matrices), set.matrices.length * 4);
            if (set.colors != null) g.buffers[1] = buffer(GLES30.GL_ARRAY_BUFFER, floats(set.colors), set.colors.length * 4);
            instanceSets.put(id, g);
            return g;
        }

        /** The mesh's vertex array with instance set `id` (made once). */
        private int instancedVao(GpuMesh g, String id) {
            Integer vao = g.instanced.get(id);
            if (vao == null) {
                vao = vertexArray(g, instanceSets.get(id));
                g.instanced.put(id, vao);
            }
            return vao;
        }

        /** Uploads a skinned draw's joint matrices to bones texture `slot` (RGBA32F, 4 texels per joint). */
        private int bones(int slot, PrismScenePlayer.Draw d) {
            if (slot >= bones.length) {
                int[] more = new int[Math.max(4, slot * 2)];
                GLES30.glGenTextures(more.length - bones.length, more, bones.length);
                System.arraycopy(bones, 0, more, 0, bones.length);
                bones = more;
                bonesRows = java.util.Arrays.copyOf(bonesRows, more.length);
            }
            int rows = Math.max(1, d.jointCount), floats = rows * 16;
            if (bonesData.capacity() < floats) {
                bonesData = ByteBuffer.allocateDirect(floats * 4).order(ByteOrder.nativeOrder()).asFloatBuffer();
            }
            bonesData.clear();
            bonesData.put(d.joints, 0, d.jointCount * 16);
            while (bonesData.position() < floats) bonesData.put(0);
            bonesData.position(0);
            GLES30.glActiveTexture(GLES30.GL_TEXTURE0 + UNIT_BONES);
            GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, bones[slot]);
            if (bonesRows[slot] < rows) {
                GLES30.glTexImage2D(GLES30.GL_TEXTURE_2D, 0, GLES30.GL_RGBA32F, 4, rows, 0, GLES30.GL_RGBA, GLES30.GL_FLOAT, bonesData);
                GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_MIN_FILTER, GLES30.GL_NEAREST);
                GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_MAG_FILTER, GLES30.GL_NEAREST);
                bonesRows[slot] = rows;
            } else {
                GLES30.glTexSubImage2D(GLES30.GL_TEXTURE_2D, 0, 0, 0, 4, rows, GLES30.GL_RGBA, GLES30.GL_FLOAT, bonesData);
            }
            return bones[slot];
        }

        /** Environment `id` as one mipmapped RGBA16F texture (level i = mip i); null while it loads. */
        private GpuEnv env(String id) {
            GpuEnv e = envs.get(id);
            if (e != null) return e;
            Object blob = Store.shared.get(id);
            if (!(blob instanceof PrismScenePlayer.EnvMap)) return null;
            PrismScenePlayer.EnvMap map = (PrismScenePlayer.EnvMap) blob;
            int[] t = new int[1];
            GLES30.glGenTextures(1, t, 0);
            GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, t[0]);
            GLES30.glPixelStorei(GLES30.GL_UNPACK_ALIGNMENT, 4);
            for (int i = 0; i < map.levels; i++) {
                int w = map.levelWidth(i), h = map.levelHeight(i);
                // Little-endian half floats, row 0 = +Y, as the blob stores them.
                ByteBuffer texels = ByteBuffer.allocateDirect(w * h * 8).order(ByteOrder.nativeOrder());
                texels.put(map.bytes, map.offsets[i], w * h * 8).position(0);
                GLES30.glTexImage2D(GLES30.GL_TEXTURE_2D, i, GLES30.GL_RGBA16F, w, h, 0, GLES30.GL_RGBA, GLES30.GL_HALF_FLOAT, texels);
            }
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_BASE_LEVEL, 0);
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_MAX_LEVEL, map.levels - 1);
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_MIN_FILTER, GLES30.GL_LINEAR_MIPMAP_LINEAR);
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_MAG_FILTER, GLES30.GL_LINEAR);
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_WRAP_S, GLES30.GL_REPEAT);
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_WRAP_T, GLES30.GL_CLAMP_TO_EDGE);
            e = new GpuEnv();
            e.texture = t[0];
            e.levels = map.levels;
            e.sh = map.sh;
            envs.put(id, e);
            return e;
        }

        /** Uploads a picture: straight RGBA as stored (the shader linearizes colors), mipmapped, repeating. */
        private void upload(String src, Image image) {
            if (!asked.contains(src) || textures.containsKey(src)) return;
            int[] id = new int[1];
            GLES30.glGenTextures(1, id, 0);
            GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, id[0]);
            GLES30.glPixelStorei(GLES30.GL_UNPACK_ALIGNMENT, 1);
            // Row 0 is the image's top: uv (0, 0) is its top-left, as in geometry.rs.
            GLES30.glTexImage2D(GLES30.GL_TEXTURE_2D, 0, GLES30.GL_RGBA, image.width, image.height, 0,
                    GLES30.GL_RGBA, GLES30.GL_UNSIGNED_BYTE, image.rgba);
            GLES30.glGenerateMipmap(GLES30.GL_TEXTURE_2D);
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_MIN_FILTER, GLES30.GL_LINEAR_MIPMAP_LINEAR);
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_MAG_FILTER, GLES30.GL_LINEAR);
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_WRAP_S, GLES30.GL_REPEAT);
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, GLES30.GL_TEXTURE_WRAP_T, GLES30.GL_REPEAT);
            if (anisotropy > 1) GLES30.glTexParameterf(GLES30.GL_TEXTURE_2D, TEXTURE_MAX_ANISOTROPY, anisotropy);
            textures.put(src, id[0]);
        }

        /** After an update: frees what the scene no longer uses (drawn or culled). */
        private void forget(Set<String> blobs, Set<String> sources, Set<String> meshKeys) {
            for (Iterator<Map.Entry<String, GpuMesh>> it = meshes.entrySet().iterator(); it.hasNext(); ) {
                Map.Entry<String, GpuMesh> e = it.next();
                GpuMesh g = e.getValue();
                if (meshKeys.contains(e.getKey())) {
                    // Its vertex arrays with instance sets gone.
                    for (Iterator<Map.Entry<String, Integer>> v = g.instanced.entrySet().iterator(); v.hasNext(); ) {
                        Map.Entry<String, Integer> i = v.next();
                        if (!blobs.contains(i.getKey())) {
                            GLES30.glDeleteVertexArrays(1, new int[]{i.getValue()}, 0);
                            v.remove();
                        }
                    }
                    continue;
                }
                GLES30.glDeleteVertexArrays(1, new int[]{g.vao}, 0);
                for (int vao : g.instanced.values()) GLES30.glDeleteVertexArrays(1, new int[]{vao}, 0);
                for (int b : g.buffers) if (b != 0) GLES30.glDeleteBuffers(1, new int[]{b}, 0);
                it.remove();
            }
            for (Iterator<Map.Entry<String, GpuInstances>> it = instanceSets.entrySet().iterator(); it.hasNext(); ) {
                Map.Entry<String, GpuInstances> e = it.next();
                if (blobs.contains(e.getKey())) continue;
                for (int b : e.getValue().buffers) if (b != 0) GLES30.glDeleteBuffers(1, new int[]{b}, 0);
                it.remove();
            }
            for (Iterator<Map.Entry<String, Integer>> it = textures.entrySet().iterator(); it.hasNext(); ) {
                Map.Entry<String, Integer> e = it.next();
                if (sources.contains(e.getKey())) continue;
                GLES30.glDeleteTextures(1, new int[]{e.getValue()}, 0);
                it.remove();
            }
            asked.retainAll(sources);
            for (Iterator<Map.Entry<String, GpuEnv>> it = envs.entrySet().iterator(); it.hasNext(); ) {
                Map.Entry<String, GpuEnv> e = it.next();
                if (blobs.contains(e.getKey())) continue;
                GLES30.glDeleteTextures(1, new int[]{e.getValue().texture}, 0);
                it.remove();
            }
        }

        /** Destroying the context frees its buffers, textures and programs. */
        private void release() {
            if (choreographer != null) choreographer.removeFrameCallback(this);
            pending = false;
            broken = true;
            if (display == null) return;
            EGL14.eglMakeCurrent(display, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_CONTEXT);
            if (eglSurface != null) EGL14.eglDestroySurface(display, eglSurface);
            if (context != null) EGL14.eglDestroyContext(display, context);
            EGL14.eglReleaseThread();
            display = null;
            context = null;
            eglSurface = null;
        }
    }
}
