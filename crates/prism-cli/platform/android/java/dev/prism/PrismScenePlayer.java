package dev.prism;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * What happens to a 3D scene on the device between two renders of the app: a
 * port of prism-core's `scene3d::player` (the retained node table, orbiting
 * camera, spins, transitions, animation clips, skins, culling, shadows, hit
 * testing), `scene3d::wire` (the update JSON), `scene3d::blobs` and
 * `scene3d::env` (the binary blobs), `scene3d::clip` (keyframe sampling),
 * `scene3d::geometry` (the primitives), `scene3d::math` and
 * `scene3d::shaders` (the GLSL, verbatim), so Android moves, animates, draws
 * and picks exactly like the other platforms.
 *
 * Plain Java, no Android classes (a desktop JVM runs its tests): it reads the
 * update JSON itself ({@link Json}) and gets blobs through {@link Blobs}. Not
 * thread-safe: PrismScene guards it with a lock. Times are seconds, sizes points.
 *
 * {@link #advance} and {@link #frame} run every displayed frame while the scene
 * moves: they allocate nothing (per-row matrices, pooled draws, scratch arrays),
 * so animation causes no garbage collections.
 */
final class PrismScenePlayer {
    /** Degrees the camera turns per point dragged. */
    static final float DEGREES_PER_POINT = 0.5f;
    /** The camera stays this many degrees away from straight up or down. */
    static final float MAX_PITCH = 89;
    /** A press that moves less than this (points) is a tap. */
    static final float TAP_SLOP = 6;
    /** Width and height of the shadow map, texels. */
    static final int SHADOW_MAP_SIZE = 2048;
    /** Most lights a draw uses (ambient lights are summed apart; shaders.rs MAX_LIGHTS). */
    static final int MAX_LIGHTS = 8;
    /** Segments around a sphere, cylinder, cone or torus. */
    static final int SEGMENTS = 48;
    /** Rings from pole to pole of a sphere, and around a torus' tube. */
    static final int RINGS = 24;

    /** Decoded blobs by id (see blobs.rs): a {@link MeshData}, {@link InstancesBlob}, {@link ClipData}, {@link SkinBlob} or {@link EnvMap}. */
    interface Blobs {
        /** The decoded blob, or null while it has not arrived (the host fetches it and calls {@link #blobsArrived}). */
        Object get(String id);
    }

    // --- JSON ---------------------------------------------------------------------------------

    /** A small JSON reader: objects are Maps, arrays Lists, numbers Doubles. */
    static final class Json {
        private final String s;
        private int i;

        private Json(String s) {
            this.s = s;
        }

        /** The value of `text`; IllegalArgumentException if it is not JSON. */
        static Object parse(String text) {
            Json j = new Json(text);
            j.ws();
            Object v = j.value();
            j.ws();
            if (j.i != text.length()) throw j.error();
            return v;
        }

        private IllegalArgumentException error() {
            return new IllegalArgumentException("bad JSON at " + i);
        }

        private void ws() {
            while (i < s.length() && s.charAt(i) <= ' ') i++;
        }

        private char peek() {
            if (i >= s.length()) throw error();
            return s.charAt(i);
        }

        private void word(String w) {
            if (!s.startsWith(w, i)) throw error();
            i += w.length();
        }

        private Object value() {
            char c = peek();
            switch (c) {
                case '{': {
                    i++;
                    Map<String, Object> m = new HashMap<>();
                    ws();
                    if (peek() == '}') {
                        i++;
                        return m;
                    }
                    while (true) {
                        ws();
                        if (peek() != '"') throw error();
                        String k = string();
                        ws();
                        if (peek() != ':') throw error();
                        i++;
                        ws();
                        m.put(k, value());
                        ws();
                        char d = peek();
                        i++;
                        if (d == '}') return m;
                        if (d != ',') throw error();
                    }
                }
                case '[': {
                    i++;
                    List<Object> a = new ArrayList<>();
                    ws();
                    if (peek() == ']') {
                        i++;
                        return a;
                    }
                    while (true) {
                        ws();
                        a.add(value());
                        ws();
                        char d = peek();
                        i++;
                        if (d == ']') return a;
                        if (d != ',') throw error();
                    }
                }
                case '"': return string();
                case 't': word("true"); return Boolean.TRUE;
                case 'f': word("false"); return Boolean.FALSE;
                case 'n': word("null"); return null;
                default: {
                    int start = i;
                    while (i < s.length() && "+-0123456789.eE".indexOf(s.charAt(i)) >= 0) i++;
                    if (start == i) throw error();
                    try {
                        return Double.parseDouble(s.substring(start, i));
                    } catch (NumberFormatException e) {
                        throw error();
                    }
                }
            }
        }

        private String string() {
            i++; // the quote
            StringBuilder b = null;
            int start = i;
            while (true) {
                char c = peek();
                if (c == '"') {
                    String out = b == null ? s.substring(start, i) : b.append(s, start, i).toString();
                    i++;
                    return out;
                }
                if (c != '\\') {
                    i++;
                    continue;
                }
                if (b == null) b = new StringBuilder();
                b.append(s, start, i);
                i++;
                char e = peek();
                i++;
                switch (e) {
                    case 'n': b.append('\n'); break;
                    case 't': b.append('\t'); break;
                    case 'r': b.append('\r'); break;
                    case 'b': b.append('\b'); break;
                    case 'f': b.append('\f'); break;
                    case 'u':
                        if (i + 4 > s.length()) throw error();
                        b.append((char) Integer.parseInt(s.substring(i, i + 4), 16));
                        i += 4;
                        break;
                    default: b.append(e);
                }
                start = i;
            }
        }
    }

    @SuppressWarnings("unchecked")
    private static Map<String, Object> obj(Object o) {
        return o instanceof Map ? (Map<String, Object>) o : null;
    }

    private static Map<String, Object> obj(Map<String, Object> o, String k) {
        return obj(o.get(k));
    }

    @SuppressWarnings("unchecked")
    private static List<Object> arr(Map<String, Object> o, String k) {
        Object v = o.get(k);
        return v instanceof List ? (List<Object>) v : null;
    }

    private static float num(Map<String, Object> o, String k, float fallback) {
        Object v = o.get(k);
        return v instanceof Number ? ((Number) v).floatValue() : fallback;
    }

    private static boolean bool(Map<String, Object> o, String k, boolean fallback) {
        Object v = o.get(k);
        return v instanceof Boolean ? (Boolean) v : fallback;
    }

    private static String str(Map<String, Object> o, String k) {
        Object v = o.get(k);
        return v instanceof String ? (String) v : null;
    }

    private static float at(List<Object> a, int i, float fallback) {
        return i < a.size() && a.get(i) instanceof Number ? ((Number) a.get(i)).floatValue() : fallback;
    }

    /** A fixed-size vector (the fallback's length; shorter arrays: the fallback). */
    private static float[] floats(Map<String, Object> o, String k, float[] fallback) {
        List<Object> a = arr(o, k);
        if (a == null || a.size() < fallback.length) return fallback;
        float[] v = new float[fallback.length];
        for (int i = 0; i < v.length; i++) v[i] = at(a, i, 0);
        return v;
    }

    private static List<String> strings(Map<String, Object> o, String k) {
        List<Object> a = arr(o, k);
        if (a == null) return null;
        List<String> out = new ArrayList<>(a.size());
        for (Object v : a) if (v instanceof String) out.add((String) v);
        return out;
    }

    // --- The wire format (wire.rs and mod.rs, their serde defaults) -------------------------

    /** sRGB color with alpha, and its dark variant (`[r,g,b,a]` or `[r,g,b,a, dr,dg,db,da]`). Immutable. */
    static final class Rgba {
        final float[] light;
        /** Null: the color does not adapt. */
        final float[] dark;

        Rgba(float[] light, float[] dark) {
            this.light = light;
            this.dark = dark;
        }

        float[] resolve(boolean isDark) {
            return isDark && dark != null ? dark : light;
        }

        static Rgba of(List<Object> a, Rgba fallback) {
            if (a == null) return fallback;
            return new Rgba(new float[]{at(a, 0, 0), at(a, 1, 0), at(a, 2, 0), at(a, 3, 1)},
                    a.size() >= 8 ? new float[]{at(a, 4, 0), at(a, 5, 0), at(a, 6, 0), at(a, 7, 1)} : null);
        }

        static boolean same(Rgba a, Rgba b) {
            if (a == b) return true;
            if (a == null || b == null) return false;
            return eq(a.light, b.light) && (a.dark == null ? b.dark == null : b.dark != null && eq(a.dark, b.dark));
        }
    }

    static final Rgba WHITE = new Rgba(new float[]{1, 1, 1, 1}, null);

    static final class Camera {
        float[] position = {0, 2, 6};
        float[] target = {0, 0, 0};
        float fov = 45, near = 0.1f, far = 1000;
        /** Orthographic: the visible height; NaN: perspective. */
        float ortho = Float.NaN;

        static Camera of(Map<String, Object> c) {
            Camera cam = new Camera();
            if (c == null) return cam;
            cam.position = floats(c, "pos", cam.position);
            cam.target = floats(c, "target", cam.target);
            cam.fov = num(c, "fov", 45);
            cam.near = num(c, "near", 0.1f);
            cam.far = num(c, "far", 1000);
            cam.ortho = num(c, "ortho", Float.NaN);
            return cam;
        }

        boolean same(Camera o) {
            return eq(position, o.position) && eq(target, o.target) && fov == o.fov && near == o.near
                    && far == o.far && sameOpt(ortho, o.ortho);
        }
    }

    /** What the user may do with the camera (`Orbit` in mod.rs). */
    static final class OrbitSpec {
        boolean rotate = true, zoom = true, pan;
        float auto, min, max;

        static boolean same(OrbitSpec a, OrbitSpec b) {
            if (a == null || b == null) return a == b;
            return a.rotate == b.rotate && a.zoom == b.zoom && a.pan == b.pan && a.auto == b.auto
                    && a.min == b.min && a.max == b.max;
        }
    }

    static final int AMBIENT = 0, DIRECTIONAL = 1, POINT = 2, SPOT = 3;

    static final class Light {
        int kind;
        Rgba color = WHITE;
        float intensity;
        float[] position = {0, 0, 0};
        float[] direction = {0, 0, 0};
        float angle, range;
        boolean shadows;

        Light(int kind, float intensity) {
            this.kind = kind;
            this.intensity = intensity;
        }
    }

    /** The lights of a scene without lights nor environment (`SceneSettings::effective_lights`). */
    private static final List<Light> DEFAULT_RIG;

    static {
        Light key = new Light(DIRECTIONAL, 0.9f);
        key.direction = new float[]{-0.5f, -1, -0.6f};
        DEFAULT_RIG = Collections.unmodifiableList(Arrays.asList(new Light(AMBIENT, 0.35f), key));
    }

    static final class Transition {
        float ms, delay;
        float[] bezier = {0, 0, 1, 1};
    }

    /** Everything about a scene but its nodes (`SceneSettings`). */
    static final class Settings {
        /** Null: transparent. */
        Rgba background;
        Camera camera = new Camera();
        /** Null: the camera stays where it is. */
        OrbitSpec orbit;
        List<Light> lights = new ArrayList<>();
        /** Null: changes apply at once. */
        Transition transition;
        boolean tap;
        /** The environment blob; null: none. */
        String envId;
        float envIntensity = 1;
        boolean envBackground;
        float exposure = 1;

        /** The app's lights, or the default rig when it gave none and has no environment. */
        List<Light> effectiveLights() {
            return lights.isEmpty() && envId == null ? DEFAULT_RIG : lights;
        }

        static Settings of(Map<String, Object> o) {
            Settings s = new Settings();
            if (o == null) return s;
            List<Object> bg = arr(o, "bg");
            s.background = Rgba.of(bg, null);
            s.camera = Camera.of(obj(o, "camera"));
            Map<String, Object> orbit = obj(o, "orbit");
            if (orbit != null) {
                OrbitSpec b = new OrbitSpec();
                b.rotate = bool(orbit, "rotate", true);
                b.zoom = bool(orbit, "zoom", true);
                b.pan = bool(orbit, "pan", false);
                b.auto = num(orbit, "auto", 0);
                b.min = num(orbit, "min", 0);
                b.max = num(orbit, "max", 0);
                s.orbit = b;
            }
            List<Object> lights = arr(o, "lights");
            for (int i = 0; lights != null && i < lights.size(); i++) {
                Map<String, Object> l = obj(lights.get(i));
                if (l == null) continue;
                String type = String.valueOf(l.get("type"));
                int kind = type.equals("ambient") ? AMBIENT : type.equals("directional") ? DIRECTIONAL
                        : type.equals("point") ? POINT : type.equals("spot") ? SPOT : -1;
                if (kind < 0) continue;
                Light light = new Light(kind, num(l, "intensity", 1));
                light.color = Rgba.of(arr(l, "color"), WHITE);
                light.position = floats(l, "pos", light.position);
                light.direction = floats(l, "dir", light.direction);
                light.angle = num(l, "angle", 0);
                light.range = num(l, "range", 0);
                light.shadows = bool(l, "shadows", false);
                s.lights.add(light);
            }
            Map<String, Object> t = obj(o, "t");
            if (t != null) {
                Transition tr = new Transition();
                tr.ms = num(t, "ms", 0);
                tr.delay = num(t, "delay", 0);
                tr.bezier = floats(t, "bezier", tr.bezier);
                s.transition = tr;
            }
            s.tap = bool(o, "tap", false);
            Map<String, Object> env = obj(o, "env");
            if (env != null && str(env, "id") != null) {
                s.envId = str(env, "id");
                s.envIntensity = num(env, "intensity", 1);
                s.envBackground = bool(env, "background", false);
            }
            s.exposure = num(o, "exposure", 1);
            return s;
        }
    }

    /** A node's shape; `type` is serde's tag (box, sphere, plane, cylinder, cone, torus, blob). Immutable. */
    static final class Mesh {
        String type;
        float[] size = {1, 1, 1};
        float radius, height, tube;
        /** Blob meshes: the `M` blob and its positions' box. */
        String id;
        float[] blobBounds;
        private String key;

        static Mesh of(Map<String, Object> o) {
            if (o == null) return null;
            Mesh m = new Mesh();
            m.type = String.valueOf(o.get("type"));
            switch (m.type) {
                case "box": m.size = floats(o, "size", new float[]{1, 1, 1}); break;
                case "plane": m.size = floats(o, "size", new float[]{1, 1}); break;
                case "sphere": m.radius = num(o, "radius", 0.5f); break;
                case "cylinder": case "cone":
                    m.radius = num(o, "radius", 0.5f);
                    m.height = num(o, "height", 1);
                    break;
                case "torus":
                    m.radius = num(o, "radius", 1);
                    m.tube = num(o, "tube", 0.25f);
                    break;
                case "blob":
                    m.id = str(o, "id");
                    if (m.id == null) return null;
                    m.blobBounds = floats(o, "bounds", new float[6]);
                    break;
                default: return null;
            }
            return m;
        }

        static Mesh box(float x, float y, float z) {
            Mesh m = new Mesh();
            m.type = "box";
            m.size = new float[]{x, y, z};
            return m;
        }

        /** Its box in its own space (`Mesh::bounds`). */
        float[] bounds(float[] out) {
            float x, y, z;
            switch (type) {
                case "box": x = size[0] / 2; y = size[1] / 2; z = size[2] / 2; break;
                case "sphere": x = y = z = radius; break;
                case "plane": x = size[0] / 2; y = 0; z = size[1] / 2; break;
                case "cylinder": case "cone": x = z = radius; y = height / 2; break;
                case "torus": x = z = radius + tube; y = tube; break;
                default:
                    System.arraycopy(blobBounds, 0, out, 0, 6);
                    return out;
            }
            out[0] = -x; out[1] = -y; out[2] = -z; out[3] = x; out[4] = y; out[5] = z;
            return out;
        }

        /** Identifies the triangles (`Mesh::key`): a blob's id, or the primitive and its sizes. */
        String key() {
            if (key == null) {
                switch (type) {
                    case "box": key = "box:" + f(size[0]) + "," + f(size[1]) + "," + f(size[2]); break;
                    case "plane": key = "plane:" + f(size[0]) + "," + f(size[1]); break;
                    case "sphere": key = "sphere:" + f(radius); break;
                    case "cylinder": case "cone": key = type + ":" + f(radius) + "," + f(height); break;
                    case "torus": key = "torus:" + f(radius) + "," + f(tube); break;
                    default: key = id;
                }
            }
            return key;
        }

        /** Rust's `{}` of an f32: shortest digits, never an exponent. */
        private static String f(float v) {
            if (v == Math.rint(v) && Math.abs(v) < 1e15f) return Long.toString((long) v);
            return new java.math.BigDecimal(Float.toString(v)).stripTrailingZeros().toPlainString();
        }

        static boolean same(Mesh a, Mesh b) {
            if (a == b) return true;
            if (a == null || b == null || !a.type.equals(b.type)) return false;
            return eq(a.size, b.size) && a.radius == b.radius && a.height == b.height && a.tube == b.tube
                    && (a.id == null ? b.id == null : a.id.equals(b.id))
                    && (a.blobBounds == null ? b.blobBounds == null : b.blobBounds != null && eq(a.blobBounds, b.blobBounds));
        }
    }

    /** How a material uses alpha (`u_alpha_mode`). */
    static final int BLEND = 0, MASK = 1, OPAQUE = 2;

    /** Immutable. */
    static final class Material {
        Rgba color = new Rgba(new float[]{0.8f, 0.8f, 0.8f, 1}, null);
        float metalness, roughness = 0.5f;
        /** Null: none (black). */
        Rgba emissive;
        boolean unlit, doubleSided;
        /** Image sources (picture sources or `p3d:<blob>`), or null. */
        String texture, mrMap, normalMap, emissiveMap, aoMap;
        float normalScale = 1;
        int alpha = BLEND;
        float cutoff;

        static Material of(Map<String, Object> m) {
            if (m == null) return DEFAULT_MATERIAL;
            Material mat = new Material();
            mat.color = Rgba.of(arr(m, "color"), mat.color);
            mat.metalness = num(m, "metal", 0);
            mat.roughness = num(m, "rough", 0.5f);
            mat.emissive = Rgba.of(arr(m, "emissive"), null);
            mat.unlit = bool(m, "unlit", false);
            mat.doubleSided = bool(m, "double", false);
            mat.texture = str(m, "map");
            mat.mrMap = str(m, "mr_map");
            mat.normalMap = str(m, "normal_map");
            mat.normalScale = num(m, "normal_scale", 1);
            mat.emissiveMap = str(m, "emissive_map");
            mat.aoMap = str(m, "ao_map");
            String alpha = str(m, "alpha");
            mat.alpha = "mask".equals(alpha) ? MASK : "opaque".equals(alpha) ? OPAQUE : BLEND;
            mat.cutoff = num(m, "cutoff", 0);
            return mat;
        }
    }

    static final Material DEFAULT_MATERIAL = new Material();

    /** One node without its children (`FlatNode`). Immutable once read. */
    static final class FlatNode {
        String key, parent, name, label;
        /** Null: a group. */
        Mesh mesh;
        Material material = DEFAULT_MATERIAL;
        float[] position = {0, 0, 0};
        float[] rotation = {0, 0, 0};
        /** Null: `rotation` is used. */
        float[] quat;
        float[] scale = {1, 1, 1};
        float[] spin = {0, 0, 0};
        boolean hidden;
        /** Instances: the `I` blob, how many, their box in the node's space; null: none. */
        String instId;
        int instCount;
        float[] instBounds;
        /** The clip it plays: the `A` blob (null: none), speed, loop, fade, run. */
        String playClip;
        float playSpeed = 1, playFade;
        boolean playLoop;
        long playRun;
        /** Its skin: the `K` blob (null: none) and the key of its root. */
        String skinId, skinRoot;
        boolean castShadows = true, receiveShadows = true;

        FlatNode(String key) {
            this.key = key;
        }

        static FlatNode of(Map<String, Object> o) {
            String key = str(o, "key");
            if (key == null) return null;
            FlatNode n = new FlatNode(key);
            n.parent = str(o, "parent");
            n.name = str(o, "name");
            n.label = str(o, "label");
            n.mesh = Mesh.of(obj(o, "mesh"));
            n.material = Material.of(obj(o, "mat"));
            n.position = floats(o, "pos", n.position);
            n.rotation = floats(o, "rot", n.rotation);
            List<Object> quat = arr(o, "quat");
            n.quat = quat != null && quat.size() >= 4 ? floats(o, "quat", new float[4]) : null;
            n.scale = floats(o, "scale", n.scale);
            n.spin = floats(o, "spin", n.spin);
            n.hidden = bool(o, "hidden", false);
            Map<String, Object> inst = obj(o, "inst");
            if (inst != null && str(inst, "id") != null) {
                n.instId = str(inst, "id");
                n.instCount = (int) num(inst, "count", 0);
                n.instBounds = floats(inst, "bounds", new float[6]);
            }
            Map<String, Object> play = obj(o, "play");
            if (play != null && str(play, "clip") != null) {
                n.playClip = str(play, "clip");
                n.playSpeed = num(play, "speed", 1);
                n.playLoop = bool(play, "loop", false);
                n.playFade = num(play, "fade", 0);
                Object run = play.get("run");
                n.playRun = run instanceof Number ? ((Number) run).longValue() : 0;
            }
            Map<String, Object> skin = obj(o, "skin");
            if (skin != null && str(skin, "id") != null && str(skin, "root") != null) {
                n.skinId = str(skin, "id");
                n.skinRoot = str(skin, "root");
            }
            n.castShadows = bool(o, "cast", true);
            n.receiveShadows = bool(o, "recv", true);
            return n;
        }

        /** A group's quick constructor (tests). */
        FlatNode mesh(Mesh m) { mesh = m; return this; }

        FlatNode at(float x, float y, float z) { position = new float[]{x, y, z}; return this; }
    }

    /** A change of scene (`SceneUpdate`): full (`nodes`) or a diff (`upsert`, `remove`, `order`). */
    static final class Update {
        boolean full;
        /** Null in a diff: unchanged. */
        Settings settings;
        /** Full: every node in pre-order; diff: those added or changed. */
        List<FlatNode> nodes = new ArrayList<>();
        List<String> remove = new ArrayList<>();
        /** Diff: every key in pre-order when nodes were added, removed or moved; else null. */
        List<String> order;

        /** Reads an update's JSON; null if it is not a version 2 update. */
        static Update parse(String json) {
            Map<String, Object> o = obj(Json.parse(json));
            if (o == null || num(o, "v", 0) != 2) return null;
            Update u = new Update();
            u.full = bool(o, "full", false);
            Map<String, Object> s = obj(o, "settings");
            u.settings = s != null || u.full ? Settings.of(s) : null;
            List<Object> nodes = arr(o, u.full ? "nodes" : "upsert");
            for (int i = 0; nodes != null && i < nodes.size(); i++) {
                Map<String, Object> n = obj(nodes.get(i));
                FlatNode f = n == null ? null : FlatNode.of(n);
                if (f != null) u.nodes.add(f);
            }
            if (!u.full) {
                List<String> remove = strings(o, "remove");
                if (remove != null) u.remove = remove;
                u.order = strings(o, "order");
            }
            return u;
        }
    }

    // --- Blobs (blobs.rs, env.rs) ---------------------------------------------------------

    /** An instance set (`I`): its matrices (`T · R(q) · S`, 16 per instance), colors (4 per instance, or null) and box. */
    static final class InstancesBlob {
        int count;
        float[] matrices, colors, bounds;
    }

    /** A skin (`K`): 16 floats per joint (inverse binds) and each joint's child-index path from the skin's root. */
    static final class SkinBlob {
        float[] inverseBind;
        String[] joints;
    }

    /** A prefiltered environment (`E`): level i is (width >> i) × (height >> i) RGBA half floats at `offsets[i]` in `bytes`. */
    static final class EnvMap {
        int width, height, levels;
        float[] sh = new float[27];
        byte[] bytes;
        int[] offsets;

        int levelWidth(int i) {
            return Math.max(1, width >> i);
        }

        int levelHeight(int i) {
            return Math.max(1, height >> i);
        }
    }

    /**
     * Decodes a blob's bytes by its header: a {@link MeshData}, {@link InstancesBlob},
     * {@link ClipData}, {@link SkinBlob} or {@link EnvMap}; null if malformed or an image.
     */
    static Object decodeBlob(byte[] bytes) {
        if (bytes == null || bytes.length < 8 || bytes[0] != 'P' || bytes[1] != '3' || bytes[2] != 'D') return null;
        ByteBuffer b = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN);
        b.position(4);
        try {
            if (b.getInt() != 1) return null;
            switch (bytes[3]) {
                case 'M': return decodeMesh(b);
                case 'I': return decodeInstances(b);
                case 'A': return decodeClip(b);
                case 'K': return decodeSkin(b);
                case 'E': return decodeEnv(b, bytes);
                default: return null;
            }
        } catch (RuntimeException e) {
            // Past the end, or sizes that make no sense.
            return null;
        }
    }

    private static float[] f32s(ByteBuffer b, long n) {
        if (n < 0 || n * 4 > b.remaining()) throw new IndexOutOfBoundsException();
        float[] v = new float[(int) n];
        b.asFloatBuffer().get(v);
        b.position(b.position() + v.length * 4);
        return v;
    }

    private static int count(ByteBuffer b) {
        int n = b.getInt();
        if (n < 0) throw new IndexOutOfBoundsException();
        return n;
    }

    private static void align(ByteBuffer b) {
        b.position(Math.min(b.limit(), (b.position() + 3) & ~3));
    }

    private static String utf8(ByteBuffer b) {
        int len = count(b);
        if (len > b.remaining()) throw new IndexOutOfBoundsException();
        String s = new String(b.array(), b.position(), len, StandardCharsets.UTF_8);
        b.position(b.position() + len);
        align(b);
        return s;
    }

    private static final int MESH_UV = 1, MESH_SKIN = 4, MESH_COLOR = 8;

    private static MeshData decodeMesh(ByteBuffer b) {
        int n = count(b), m = count(b), flags = b.getInt();
        MeshData d = new MeshData();
        d.bounds = f32s(b, 6);
        d.positions = f32s(b, 3L * n);
        d.normals = f32s(b, 3L * n);
        if ((flags & MESH_UV) != 0) d.uvs = f32s(b, 2L * n);
        if ((flags & MESH_SKIN) != 0) {
            if (8L * n > b.remaining()) throw new IndexOutOfBoundsException();
            d.joints = new short[4 * n];
            b.asShortBuffer().get(d.joints);
            b.position(b.position() + 8 * n);
            align(b);
            d.weights = f32s(b, 4L * n);
        }
        if ((flags & MESH_COLOR) != 0) d.colors = f32s(b, 4L * n);
        if (4L * m > b.remaining()) throw new IndexOutOfBoundsException();
        int[] idx = new int[m];
        b.asIntBuffer().get(idx);
        for (int i : idx) if (i < 0 || i >= n) return null;
        // Whole triangles only.
        d.indices = m % 3 == 0 ? idx : Arrays.copyOf(idx, m / 3 * 3);
        return d;
    }

    private static InstancesBlob decodeInstances(ByteBuffer b) {
        InstancesBlob s = new InstancesBlob();
        int count = count(b), flags = b.getInt();
        s.bounds = f32s(b, 6);
        float[] raw = f32s(b, 10L * count);
        s.count = count;
        s.matrices = new float[16 * count];
        float[] m = new float[16];
        for (int i = 0; i < count; i++) {
            int o = i * 10;
            composeQuat(raw[o], raw[o + 1], raw[o + 2], raw[o + 3], raw[o + 4], raw[o + 5], raw[o + 6],
                    raw[o + 7], raw[o + 8], raw[o + 9], m);
            System.arraycopy(m, 0, s.matrices, i * 16, 16);
        }
        if ((flags & 1) != 0) s.colors = f32s(b, 4L * count);
        return s;
    }

    private static ClipData decodeClip(ByteBuffer b) {
        int count = count(b);
        ClipData clip = new ClipData();
        clip.duration = b.getFloat();
        // Each channel takes at least 12 bytes: no huge allocation on bad input.
        if (count > b.remaining() / 12) throw new IndexOutOfBoundsException();
        clip.channels = new ClipChannel[count];
        for (int i = 0; i < count; i++) {
            ClipChannel c = new ClipChannel();
            c.target = utf8(b);
            c.property = b.get();
            c.interpolation = b.get();
            if (c.property < 0 || c.property > 2 || c.interpolation < 0 || c.interpolation > 2) return null;
            b.getShort();
            int k = count(b);
            c.times = f32s(b, k);
            c.values = f32s(b, (long) k * c.components() * (c.interpolation == CUBIC ? 3 : 1));
            clip.channels[i] = c;
        }
        return clip;
    }

    private static SkinBlob decodeSkin(ByteBuffer b) {
        int n = count(b);
        SkinBlob s = new SkinBlob();
        s.inverseBind = f32s(b, 16L * n);
        s.joints = new String[n];
        for (int i = 0; i < n; i++) s.joints[i] = utf8(b);
        return s;
    }

    private static EnvMap decodeEnv(ByteBuffer b, byte[] bytes) {
        EnvMap e = new EnvMap();
        e.width = b.getInt();
        e.height = b.getInt();
        e.levels = b.getInt();
        if (e.width <= 0 || e.height <= 0 || e.levels <= 0 || e.levels > 32) return null;
        e.sh = f32s(b, 27);
        e.offsets = new int[e.levels];
        long at = b.position();
        for (int i = 0; i < e.levels; i++) {
            e.offsets[i] = (int) at;
            at += 8L * e.levelWidth(i) * e.levelHeight(i);
            if (at > bytes.length) return null;
        }
        if (at != bytes.length) return null;
        e.bytes = bytes;
        return e;
    }

    // --- Clips (clip.rs) ------------------------------------------------------------------

    static final int TRANSLATION = 0, ROTATION = 1, SCALE = 2;
    static final int LINEAR = 0, STEP = 1, CUBIC = 2;

    /** One animated property of one node. */
    static final class ClipChannel {
        /** `""`, a child-index path (`"0/2"`) or `"#name"`. */
        String target;
        int property, interpolation;
        float[] times, values;

        int components() {
            return property == ROTATION ? 4 : 3;
        }

        /**
         * The value at `t` seconds into `out` (`[x, y, z, _]` for translations and
         * scales, a unit quaternion for rotations): the first key's value before
         * it, the last one's after it.
         */
        void sample(float t, float[] out) {
            int c = components();
            boolean cubic = interpolation == CUBIC;
            int stride = cubic ? 3 * c : c;
            int k = Math.min(times.length, values.length / stride);
            if (k == 0) {
                out[0] = out[1] = out[2] = property == SCALE ? 1 : 0;
                out[3] = property == ROTATION ? 1 : 0;
                return;
            }
            if (t <= times[0] || k == 1) {
                value(0, stride, c, cubic, out);
                return;
            }
            if (t >= times[k - 1]) {
                value(k - 1, stride, c, cubic, out);
                return;
            }
            // The key before t (times increase: binary search for the first key after t).
            int lo = 0, hi = k;
            while (lo < hi) {
                int mid = (lo + hi) >>> 1;
                if (times[mid] <= t) lo = mid + 1;
                else hi = mid;
            }
            int i = Math.max(lo - 1, 0);
            float t0 = times[i], t1 = times[i + 1];
            float dt = t1 - t0;
            float s = dt > 0 ? (t - t0) / dt : 0;
            if (interpolation == STEP) {
                value(i, stride, c, cubic, out);
            } else if (interpolation == LINEAR) {
                int a = i * stride, b = (i + 1) * stride;
                if (property == ROTATION) {
                    slerp(values[a], values[a + 1], values[a + 2], values[a + 3],
                            values[b], values[b + 1], values[b + 2], values[b + 3], s, out);
                } else {
                    for (int j = 0; j < 4; j++) out[j] = j < c ? lerp(values[a + j], values[b + j], s) : 0;
                }
            } else {
                // glTF: p(s) = (2s³ − 3s² + 1)·v0 + (s³ − 2s² + s)·dt·b0
                //            + (−2s³ + 3s²)·v1 + (s³ − s²)·dt·a1
                // (b0: key i's out-tangent, a1: key i+1's in-tangent).
                float s2 = s * s, s3 = s * s * s;
                int v0 = i * stride + c, b0 = i * stride + 2 * c, a1 = (i + 1) * stride, v1 = (i + 1) * stride + c;
                for (int j = 0; j < 4; j++) {
                    out[j] = j >= c ? 0 : (2 * s3 - 3 * s2 + 1) * values[v0 + j] + (s3 - 2 * s2 + s) * dt * values[b0 + j]
                            + (-2 * s3 + 3 * s2) * values[v1 + j] + (s3 - s2) * dt * values[a1 + j];
                }
                if (property == ROTATION) quatNormalize(out);
            }
        }

        private void value(int i, int stride, int c, boolean cubic, float[] out) {
            int at = i * stride + (cubic ? c : 0);
            for (int j = 0; j < 4; j++) out[j] = j < c ? values[at + j] : 0;
            if (property == ROTATION) quatNormalize(out);
        }
    }

    /** A clip's channels (the `A` blob). */
    static final class ClipData {
        float duration;
        ClipChannel[] channels;
    }

    // --- Poses (player.rs) ------------------------------------------------------------------

    /** The eased progress at `p` (0 to 1) of a CSS cubic Bézier `[x1, y1, x2, y2]` (x solved by bisection). */
    static float ease(float[] bezier, float p) {
        p = Math.max(0, Math.min(1, p));
        if (p == 0 || p == 1) return p;
        float x1 = bezier[0], y1 = bezier[1], x2 = bezier[2], y2 = bezier[3];
        float lo = 0, hi = 1;
        for (int i = 0; i < 24; i++) {
            float mid = (lo + hi) / 2;
            if (bez(x1, x2, mid) < p) lo = mid;
            else hi = mid;
        }
        return bez(y1, y2, (lo + hi) / 2);
    }

    private static float bez(float a, float b, float t) {
        float u = 1 - t;
        return 3 * u * u * t * a + 3 * u * t * t * b + t * t * t;
    }

    /** The animatable part of a node (colors as light and optional dark variants). */
    static final class Pose {
        final float[] position = new float[3], rotation = new float[3], scale = new float[3];
        final float[] quat = new float[4];
        boolean hasQuat;
        final float[] color = new float[4], colorDark = new float[4], emissive = new float[4], emissiveDark = new float[4];
        boolean hasColorDark, hasEmissiveDark;
        float metalness, roughness;

        static Pose of(FlatNode n) {
            Pose p = new Pose();
            System.arraycopy(n.position, 0, p.position, 0, 3);
            System.arraycopy(n.rotation, 0, p.rotation, 0, 3);
            System.arraycopy(n.scale, 0, p.scale, 0, 3);
            if (n.quat != null) {
                p.hasQuat = true;
                System.arraycopy(n.quat, 0, p.quat, 0, 4);
            }
            Material m = n.material;
            System.arraycopy(m.color.light, 0, p.color, 0, 4);
            if (m.color.dark != null) {
                p.hasColorDark = true;
                System.arraycopy(m.color.dark, 0, p.colorDark, 0, 4);
            }
            if (m.emissive != null) {
                System.arraycopy(m.emissive.light, 0, p.emissive, 0, 4);
                if (m.emissive.dark != null) {
                    p.hasEmissiveDark = true;
                    System.arraycopy(m.emissive.dark, 0, p.emissiveDark, 0, 4);
                }
            } else {
                p.emissive[3] = 1;
            }
            p.metalness = m.metalness;
            p.roughness = m.roughness;
            return p;
        }

        Pose copy() {
            Pose p = new Pose();
            p.set(this);
            return p;
        }

        void set(Pose o) {
            System.arraycopy(o.position, 0, position, 0, 3);
            System.arraycopy(o.rotation, 0, rotation, 0, 3);
            System.arraycopy(o.scale, 0, scale, 0, 3);
            System.arraycopy(o.quat, 0, quat, 0, 4);
            hasQuat = o.hasQuat;
            System.arraycopy(o.color, 0, color, 0, 4);
            System.arraycopy(o.colorDark, 0, colorDark, 0, 4);
            System.arraycopy(o.emissive, 0, emissive, 0, 4);
            System.arraycopy(o.emissiveDark, 0, emissiveDark, 0, 4);
            hasColorDark = o.hasColorDark;
            hasEmissiveDark = o.hasEmissiveDark;
            metalness = o.metalness;
            roughness = o.roughness;
        }

        float[] color(boolean dark) {
            return dark && hasColorDark ? colorDark : color;
        }

        float[] emissive(boolean dark) {
            return dark && hasEmissiveDark ? emissiveDark : emissive;
        }

        /** This pose between `a` and `b` at `t`; `q` is scratch (4 floats × 2). */
        void lerp(Pose a, Pose b, float t, float[] q) {
            if (!a.hasQuat && !b.hasQuat) {
                // Degrees, like v1 and 2D `rotate`: every degree in between.
                lerp3(a.rotation, b.rotation, t, rotation);
                hasQuat = false;
            } else {
                if (a.hasQuat) System.arraycopy(a.quat, 0, q, 0, 4);
                else quatFromEuler(a.rotation, q, 0);
                if (b.hasQuat) System.arraycopy(b.quat, 0, q, 4, 4);
                else quatFromEuler(b.rotation, q, 4);
                System.arraycopy(b.rotation, 0, rotation, 0, 3);
                slerp(q[0], q[1], q[2], q[3], q[4], q[5], q[6], q[7], t, quat);
                hasQuat = true;
            }
            lerp3(a.position, b.position, t, position);
            lerp3(a.scale, b.scale, t, scale);
            for (int i = 0; i < 4; i++) {
                color[i] = PrismScenePlayer.lerp(a.color[i], b.color[i], t);
                emissive[i] = PrismScenePlayer.lerp(a.emissive[i], b.emissive[i], t);
            }
            hasColorDark = a.hasColorDark || b.hasColorDark;
            if (hasColorDark) {
                float[] x = a.color(true), y = b.color(true);
                for (int i = 0; i < 4; i++) colorDark[i] = PrismScenePlayer.lerp(x[i], y[i], t);
            }
            hasEmissiveDark = a.hasEmissiveDark || b.hasEmissiveDark;
            if (hasEmissiveDark) {
                float[] x = a.emissive(true), y = b.emissive(true);
                for (int i = 0; i < 4; i++) emissiveDark[i] = PrismScenePlayer.lerp(x[i], y[i], t);
            }
            metalness = PrismScenePlayer.lerp(a.metalness, b.metalness, t);
            roughness = PrismScenePlayer.lerp(a.roughness, b.roughness, t);
        }

        boolean same(Pose o) {
            return eq(position, o.position) && eq(rotation, o.rotation) && eq(scale, o.scale)
                    && hasQuat == o.hasQuat && (!hasQuat || eq(quat, o.quat))
                    && eq(color, o.color) && hasColorDark == o.hasColorDark && (!hasColorDark || eq(colorDark, o.colorDark))
                    && eq(emissive, o.emissive) && hasEmissiveDark == o.hasEmissiveDark
                    && (!hasEmissiveDark || eq(emissiveDark, o.emissiveDark))
                    && metalness == o.metalness && roughness == o.roughness;
        }
    }

    /** What clips set on a node (each part replaces the node's own). */
    static final class ClipPose {
        boolean hasT, hasR, hasS;
        final float[] t = new float[3], r = new float[4], s = new float[3];

        void clear() {
            hasT = hasR = hasS = false;
        }

        void set(ClipPose o) {
            hasT = o.hasT;
            hasR = o.hasR;
            hasS = o.hasS;
            System.arraycopy(o.t, 0, t, 0, 3);
            System.arraycopy(o.r, 0, r, 0, 4);
            System.arraycopy(o.s, 0, s, 0, 3);
        }

        boolean same(ClipPose o) {
            return hasT == o.hasT && hasR == o.hasR && hasS == o.hasS && (!hasT || eq(t, o.t))
                    && (!hasR || eq(r, o.r)) && (!hasS || eq(s, o.s));
        }
    }

    private static final ClipPose NO_CLIP_POSE = new ClipPose();

    /** A clip playing on a node. */
    private static final class Track {
        final ClipData clip;
        /** Clip time at `anchor`. */
        float base;
        double anchor;
        float speed;
        boolean looping;
        /** The row each channel animates (-1: none), resolved after every update. */
        int[] targets = new int[0];

        Track(ClipData clip, double now, float speed, boolean looping) {
            this.clip = clip;
            this.base = speed < 0 ? clip.duration : 0;
            this.anchor = now;
            this.speed = speed;
            this.looping = looping;
        }

        /** Unbounded clip time at `now`. */
        float rawTime(double now) {
            return base + (float) (now - anchor) * speed;
        }

        /** The time to sample at `now`. */
        float time(double now) {
            float d = clip.duration, t = rawTime(now);
            if (d <= 0) return 0;
            if (looping) return remEuclid(t, d);
            return Math.max(0, Math.min(d, t));
        }

        /** Still moving at `now` (looping, or not past its end). */
        boolean running(double now) {
            float t = rawTime(now);
            return clip.duration > 0 && speed != 0
                    && (looping || speed > 0 && t < clip.duration || speed < 0 && t > 0);
        }

        /** Changes the rate without a jump in clip time. */
        void setSpeed(float speed, double now) {
            if (speed != this.speed) {
                base = rawTime(now);
                anchor = now;
                this.speed = speed;
            }
        }
    }

    /** The clips of a node with `play`. */
    private static final class Playing {
        /** The clip playing (null while its blob has not arrived). */
        Track current;
        /** The clip blending out (null: none), from `fadeStart` over `fade` seconds. */
        Track previous;
        double fadeStart;
        float fade;
        /** It moved at the last refresh: sample once more after it stops, so the last pose is exact. */
        boolean live;
    }

    /** The camera as an orbit around its target. */
    static final class Orbit {
        /** Degrees around Y; 0: on +Z of the target. */
        float yaw;
        /** Degrees above the horizon. */
        float pitch;
        float distance;
        final float[] target = new float[3];
        float fov;
        /** Visible height of an orthographic camera; NaN: perspective. */
        float ortho;

        static Orbit of(Camera c) {
            float[] o = sub(c.position, c.target);
            float d = Math.max(length(o), 1e-4f);
            Orbit r = new Orbit();
            r.yaw = (float) Math.toDegrees(Math.atan2(o[0], o[2]));
            r.pitch = (float) Math.toDegrees(Math.asin(Math.max(-1, Math.min(1, o[1] / d))));
            r.distance = d;
            System.arraycopy(c.target, 0, r.target, 0, 3);
            r.fov = c.fov;
            r.ortho = c.ortho;
            return r;
        }

        void set(Orbit o) {
            yaw = o.yaw;
            pitch = o.pitch;
            distance = o.distance;
            System.arraycopy(o.target, 0, target, 0, 3);
            fov = o.fov;
            ortho = o.ortho;
        }

        Orbit copy() {
            Orbit r = new Orbit();
            r.set(this);
            return r;
        }

        void eye(float[] out) {
            double y = Math.toRadians(yaw), p = Math.toRadians(pitch);
            out[0] = target[0] + (float) (distance * Math.cos(p) * Math.sin(y));
            out[1] = target[1] + (float) (distance * Math.sin(p));
            out[2] = target[2] + (float) (distance * Math.cos(p) * Math.cos(y));
        }

        /** This orbit between `a` and `b` at `t` (yaw the shorter way around). */
        void lerp(Orbit a, Orbit b, float t) {
            float dyaw = remEuclid(b.yaw - a.yaw + 540, 360) - 180;
            yaw = a.yaw + dyaw * t;
            pitch = PrismScenePlayer.lerp(a.pitch, b.pitch, t);
            distance = PrismScenePlayer.lerp(a.distance, b.distance, t);
            lerp3(a.target, b.target, t, target);
            fov = PrismScenePlayer.lerp(a.fov, b.fov, t);
            ortho = !Float.isNaN(a.ortho) && !Float.isNaN(b.ortho) ? PrismScenePlayer.lerp(a.ortho, b.ortho, t) : b.ortho;
        }
    }

    // --- Frames --------------------------------------------------------------------------------

    /** One light of a frame, ready for the shader's uniforms. */
    static final class FrameLight {
        /** 1 directional, 2 point, 3 spot (`u_light_kind`). */
        int kind;
        /** Linear color × intensity; world position; unit direction the light travels. */
        final float[] color = new float[3], position = new float[3], direction = new float[3];
        /** Range (0: none), cos of the outer half-angle, cos of the inner one, 0. */
        final float[] params = new float[4];
    }

    /** One mesh to draw; a frame's draws are its own copies (another thread may draw them). */
    static final class Draw {
        /** The node's row ({@link #node}). */
        int node;
        String key;
        /** The nearest named ancestor's name (itself included). */
        String name;
        Mesh mesh;
        /** `Mesh::key`: cache GPU buffers by it. */
        String meshKey;
        final float[] world = new float[16];
        /** Normal matrix, column-major 3×3. */
        final float[] normal = new float[9];
        // The material resolved for this frame.
        /** Linear rgb and alpha (1 when opaque). */
        final float[] color = new float[4];
        /** Linear. */
        final float[] emissive = new float[3];
        float metalness, roughness, normalScale, alphaCutoff;
        boolean unlit, doubleSided;
        String texture, mrMap, normalMap, emissiveMap, aoMap;
        /** {@link #BLEND}, {@link #MASK} or {@link #OPAQUE} (`u_alpha_mode`). */
        int alpha;
        /** Alpha below 1 (blend mode): drawn after the opaque meshes, blended, without writing depth. */
        boolean transparent;
        /** Instanced: the `I` blob (null: not instanced) and how many to draw. */
        String instances;
        int instanceCount;
        /** Skinned: `jointCount` joint matrices (16 floats each, `u_bones`); -1: not skinned. */
        float[] joints = new float[0];
        int jointCount = -1;
        boolean castShadows, receiveShadows;
        /** Its box in scene coordinates. */
        final float[] bounds = new float[6];
        /** Back-to-front sorting: distance from the camera to its box's center. */
        float depth;

        boolean instanced() {
            return instances != null;
        }

        boolean skinned() {
            return jointCount >= 0;
        }
    }

    /** Everything one frame draws (reused from frame to frame). */
    static final class Frame {
        float width, height;
        final float[] view = new float[16], projection = new float[16], viewProj = new float[16],
                inverseViewProj = new float[16], cameraPosition = new float[3];
        /** Premultiplied (`[0; 4]`: transparent). */
        final float[] clear = new float[4];
        /** The ambient lights, summed (linear × intensity). */
        final float[] ambient = new float[3];
        final FrameLight[] lights = new FrameLight[MAX_LIGHTS];
        int lightCount;
        /** Opaque meshes, then transparent ones back to front; culled ones left out. */
        Draw[] draws = new Draw[0];
        int drawCount;
        /** The shadow map to render first: when `hasShadow`. */
        boolean hasShadow;
        /** The light casting it (an index into lights). */
        int shadowLight;
        /** The light's `projection · view`: the depth pass's `u_view_proj` and the main pass's `u_shadow_view_proj`. */
        final float[] shadowViewProj = new float[16];
        int shadowMapSize;
        float shadowBias, shadowNormalBias;
        /** The draws to render into the map: indices into draws. */
        int[] casters = new int[0];
        int casterCount;
        /** Image-based lighting: the `E` blob (null: none). */
        String env;
        float envIntensity;
        boolean envBackground;
        /** Tone-mapping exposure. */
        float exposure;
        /** Something moves: draw another frame. */
        boolean animating;

        Frame() {
            for (int i = 0; i < lights.length; i++) lights[i] = new FrameLight();
        }

        /** Transparent draws being sorted, and spare draw objects. */
        private Draw[] transparent = new Draw[0];

        /** Pool slot `i` (allocated once). */
        Draw draw(int i) {
            if (i >= draws.length) draws = grow(draws, i);
            return draws[i];
        }

        /** The spare object at slot `i` of the transparent pool. */
        Draw spare(int i) {
            if (i >= transparent.length) transparent = grow(transparent, i);
            return transparent[i];
        }

        private static Draw[] grow(Draw[] pool, int i) {
            Draw[] more = Arrays.copyOf(pool, Math.max(8, Math.max(i + 1, pool.length * 2)));
            for (int k = pool.length; k < more.length; k++) more[k] = new Draw();
            return more;
        }
    }

    /** A tap's result (`SceneTapEvent`). */
    static final class Tap {
        /** The named node hit (null: nothing named), its labeled part, the instance (-1: none), the world point (null: nothing hit). */
        String node, part;
        int instance = -1;
        float[] point;
    }

    // --- The player -------------------------------------------------------------------------

    /** A row of the node table. */
    private static final class Row {
        FlatNode flat;
        int parent = -1;
        /** Its subtree is `index..end`. */
        int end;
        final IntList children = new IntList();
        /** The nearest named row, itself included (-1: none). */
        int named = -1;
        /** The nearest labeled row up to the named one (-1: none). */
        int part = -1;
        /** Neither it nor an ancestor is hidden. */
        boolean visible = true;
        String meshKey = "";
        /** When it appeared (its spin starts there). */
        double appeared;
        /** Its pose in the latest update. */
        Pose target;
        /** A running transition: from this pose (null: none), starting at `fromStart`. */
        Pose from;
        double fromStart;
        /** Scratch: its pose mid-transition. */
        final Pose current = new Pose();
        /** What clips set on it this frame (when `hasClipPose`). */
        final ClipPose clipPose = new ClipPose();
        boolean hasClipPose;
        Playing playing;
        /** The joint rows of its skin (-1: unresolved), resolved after every update. */
        int[] jointNodes = new int[0];
        /** Its joints' matrices (16 floats each) once computed (`jointCount`, -1: never). */
        float[] joints = new float[0];
        int jointCount = -1;
        /** Scratch: where each joint puts the mesh (`world · J_i`). */
        float[] placed = new float[0];
        final float[] local = IDENTITY.clone(), world = IDENTITY.clone(), normal = {1, 0, 0, 0, 1, 0, 0, 0, 1};
        /** Its draw's box in scene coordinates. */
        final float[] bounds = new float[6];
        /** The tick its world last changed. */
        long moved;
        /** Its local matrix needs recomputing. */
        boolean dirty = true;
        // Clip sampling scratch, valid when its stamp is the current pass's.
        final ClipPose sampleNew = new ClipPose(), sampleOld = new ClipPose(), posed = new ClipPose();
        int newStamp, oldStamp;
        long posedStamp;

        Row(FlatNode flat, double now) {
            this.flat = flat;
            meshKey = flat.mesh != null ? flat.mesh.key() : "";
            target = Pose.of(flat);
            appeared = now;
        }
    }

    /** A growable int list. */
    static final class IntList {
        int[] a = new int[8];
        int n;

        void add(int v) {
            if (n == a.length) a = Arrays.copyOf(a, n * 2);
            a[n++] = v;
        }

        int get(int i) {
            return a[i];
        }

        void clear() {
            n = 0;
        }

        boolean contains(int v) {
            for (int i = 0; i < n; i++) if (a[i] == v) return true;
            return false;
        }
    }

    private final Blobs blobs;
    private Settings settings = new Settings();
    /** No update applied yet: the first one does not animate. */
    private boolean fresh = true;
    private final ArrayList<Row> nodes = new ArrayList<>();
    private Map<String, Integer> index = new HashMap<>();
    /** Visible rows with a mesh, sorted by (shader variant, base texture, mesh key). */
    private int[] drawNodes = new int[0];
    private final IntList transitioning = new IntList();
    /** Visible rows that spin, rows playing clips, visible rows with a resolved skin. */
    private int[] spinning = new int[0], players = new int[0], skinned = new int[0];
    private Orbit orbit;
    /** A running camera transition: from this orbit (null: none), starting at `cameraFromStart`. */
    private Orbit cameraFrom;
    private double cameraFromStart;
    private float minDistance, maxDistance;
    private double now;
    private double last = Double.NaN;
    private boolean dragging;
    /** Something but the camera moves (spin, transition, clip). */
    private boolean moving;
    private long tick;
    private Map<String, ClipData> clips = new HashMap<>();
    private Map<String, SkinBlob> skins = new HashMap<>();
    private Map<String, InstancesBlob> instances = new HashMap<>();
    private final Map<String, MeshData> geometry = new HashMap<>();
    /** Blob ids, texture sources and mesh keys the scene uses (the host keeps their data and GPU objects). */
    private Set<String> used = new HashSet<>(), textures = new HashSet<>(), meshKeys = new HashSet<>();

    // Scratch for the per-frame work (no garbage while animating).
    private final IntList touched = new IntList(), posedRows = new IntList(), newTargets = new IntList(),
            oldTargets = new IntList(), done = new IntList();
    private int samplePass;
    private final float[] q8 = new float[8], v4 = new float[4], m16 = new float[16], m16b = new float[16],
            box6 = new float[6], planes = new float[24], up = {0, 1, 0};
    private final ClipPose blendScratch = new ClipPose();
    private final Orbit orbitScratch = new Orbit();
    private final Frame hitFrame = new Frame();

    /** A player showing nothing until its first {@link #apply}. */
    PrismScenePlayer(Blobs blobs, double now) {
        this.blobs = blobs;
        this.now = now;
        orbit = Orbit.of(settings.camera);
    }

    Settings settings() {
        return settings;
    }

    /** The nodes, in pre-order. */
    List<FlatNode> nodes() {
        List<FlatNode> out = new ArrayList<>(nodes.size());
        for (Row r : nodes) out.add(r.flat);
        return out;
    }

    /** The blob ids the scene refers to (meshes, instances, clips, skins, environment, `p3d:` images); replaced, never changed, by updates. */
    Set<String> usedBlobs() {
        return used;
    }

    /** The texture sources of its materials (replaced by updates). */
    Set<String> usedTextures() {
        return textures;
    }

    /** The mesh keys of its nodes, drawn or culled (replaced by updates). */
    Set<String> usedMeshKeys() {
        return meshKeys;
    }

    /**
     * Shows an update at `now`: changed nodes and camera animate from where they
     * are to their new values if the scene has a transition; a changed camera
     * replaces where the user moved it. Clips whose `play` changed start (or
     * crossfade) here.
     */
    void apply(Update update, double now) {
        this.now = now;
        if (update.settings != null) setSettings(update.settings, now);
        Transition t = fresh ? null : settings.transition;
        fresh = false;

        Map<String, Row> rows = new HashMap<>();
        List<String> order = new ArrayList<>();
        for (Row r : nodes) rows.put(r.flat.key, r);
        // Keys in their new order (removed ones simply find no row).
        if (update.full) {
            for (FlatNode n : update.nodes) order.add(n.key);
        } else {
            for (String k : update.remove) rows.remove(k);
            if (update.order != null) {
                order.addAll(update.order);
            } else {
                for (Row r : nodes) order.add(r.flat.key);
                for (FlatNode n : update.nodes) order.add(n.key);
            }
        }
        for (FlatNode flat : update.nodes) {
            Row row = rows.get(flat.key);
            if (row != null) {
                updateRow(row, flat, now, t);
            } else {
                row = new Row(flat, now);
                startPlay(row, null, now);
                rows.put(flat.key, row);
            }
        }
        Set<String> seen = new HashSet<>();
        nodes.clear();
        for (String k : order) {
            if (!seen.add(k)) continue;
            Row r = rows.remove(k);
            if (r != null) nodes.add(r);
        }
        relink();
        refresh(now, true);
    }

    /**
     * Blobs arrived: clips that waited for theirs start, skins, instances and
     * clip targets resolve again. The host then draws a frame.
     */
    void blobsArrived(double now) {
        for (Row row : nodes) {
            FlatNode f = row.flat;
            if (f.playClip == null || row.playing == null || row.playing.current != null) continue;
            ClipData clip = clip(f.playClip);
            if (clip != null) {
                row.playing.current = new Track(clip, now, f.playSpeed, f.playLoop);
                row.playing.live = true;
            }
        }
        relink();
        refresh(now, true);
    }

    private void setSettings(Settings s, double now) {
        if (fresh) {
            orbit = Orbit.of(s.camera);
            setLimits(s);
        } else if (!s.camera.same(settings.camera)) {
            Orbit to = Orbit.of(s.camera);
            Orbit from = currentOrbit(now).copy();
            orbit = to;
            cameraFrom = s.transition != null && s.transition.ms > 0 ? from : null;
            cameraFromStart = cameraFrom != null ? now + s.transition.delay / 1000.0 : 0;
            setLimits(s);
        } else if (!OrbitSpec.same(s.orbit, settings.orbit)) {
            setLimits(s);
        }
        settings = s;
    }

    private void setLimits(Settings s) {
        float d = Orbit.of(s.camera).distance;
        OrbitSpec o = s.orbit != null ? s.orbit : new OrbitSpec();
        minDistance = o.min > 0 ? o.min : d * 0.25f;
        maxDistance = o.max > 0 ? o.max : d * 4;
    }

    /** A node of the update replaces its row's props. */
    private void updateRow(Row row, FlatNode flat, double now, Transition t) {
        Pose pose = Pose.of(flat);
        if (!pose.same(row.target)) {
            Pose current = poseAt(row, now).copy();
            if (t != null && t.ms > 0 && !current.same(pose)) {
                row.from = current;
                row.fromStart = now + t.delay / 1000.0;
            } else {
                row.from = null;
            }
            row.target = pose;
        }
        if (!Mesh.same(flat.mesh, row.flat.mesh)) row.meshKey = flat.mesh != null ? flat.mesh.key() : "";
        FlatNode old = row.flat;
        row.flat = flat;
        startPlay(row, old, now);
    }

    /** Starts, restarts, crossfades, retimes or stops a row's clip after its `play` changed from `old`'s. */
    private void startPlay(Row row, FlatNode old, double now) {
        FlatNode p = row.flat;
        if (p.playClip == null) {
            row.playing = null;
            return;
        }
        ClipData clip = clip(p.playClip);
        boolean same = old != null && old.playClip != null && old.playClip.equals(p.playClip) && old.playRun == p.playRun;
        if (row.playing != null && same) {
            Track track = row.playing.current;
            if (track != null) {
                track.setSpeed(p.playSpeed, now);
                track.looping = p.playLoop;
            }
            return;
        }
        Playing previous = row.playing;
        row.playing = null;
        Playing playing = new Playing();
        if (previous != null && p.playFade > 0 && previous.current != null) {
            playing.previous = previous.current;
            playing.fadeStart = now;
            playing.fade = p.playFade;
        }
        playing.current = clip != null ? new Track(clip, now, p.playSpeed, p.playLoop) : null;
        playing.live = true;
        row.playing = playing;
    }

    private ClipData clip(String id) {
        ClipData c = clips.get(id);
        if (c != null) return c;
        Object b = blobs.get(id);
        return b instanceof ClipData ? (ClipData) b : null;
    }

    /**
     * Rebuilds the table's links after an update: parents, subtrees, named and
     * labeled ancestors, visibility, the lists of moving rows, clip targets and
     * skin joints. O(nodes).
     */
    private void relink() {
        int n = nodes.size();
        // Pre-order from the parent keys, whatever order the rows came in
        // (rows whose parent is missing go to the top level).
        Map<String, Integer> keys = new HashMap<>();
        for (int i = 0; i < n; i++) keys.put(nodes.get(i).flat.key, i);
        List<List<Integer>> kids = new ArrayList<>(n);
        for (int i = 0; i < n; i++) kids.add(new ArrayList<>());
        List<Integer> roots = new ArrayList<>();
        for (int i = 0; i < n; i++) {
            String p = nodes.get(i).flat.parent;
            Integer pi = p != null ? keys.get(p) : null;
            if (pi != null && pi != i) kids.get(pi).add(i);
            else roots.add(i);
        }
        int[] order = new int[n];
        int placedCount = 0;
        boolean[] placed = new boolean[n];
        ArrayList<Integer> stack = new ArrayList<>();
        for (int i = roots.size() - 1; i >= 0; i--) stack.add(roots.get(i));
        while (!stack.isEmpty()) {
            int i = stack.remove(stack.size() - 1);
            if (placed[i]) continue;
            placed[i] = true;
            order[placedCount++] = i;
            List<Integer> k = kids.get(i);
            for (int j = k.size() - 1; j >= 0; j--) stack.add(k.get(j));
        }
        boolean reorder = placedCount != n;
        for (int i = 0; i < placedCount && !reorder; i++) reorder = order[i] != i;
        if (reorder) {
            List<Row> rows = new ArrayList<>(nodes);
            nodes.clear();
            for (int i = 0; i < placedCount; i++) nodes.add(rows.get(order[i]));
            n = nodes.size();
        }

        index = new HashMap<>();
        for (int i = 0; i < n; i++) index.put(nodes.get(i).flat.key, i);
        for (int i = 0; i < n; i++) {
            Row row = nodes.get(i);
            Integer pi = row.flat.parent != null ? index.get(row.flat.parent) : null;
            int parent = pi != null && pi < i ? pi : -1;
            int named = -1, part = -1;
            boolean visible = true;
            if (parent >= 0) {
                Row pn = nodes.get(parent);
                named = pn.named;
                part = pn.part;
                visible = pn.visible;
            }
            row.parent = parent;
            row.end = i + 1;
            row.children.clear();
            row.named = row.flat.name != null ? i : named;
            row.part = row.flat.label != null ? i : row.flat.name != null ? -1 : part;
            row.visible = visible && !row.flat.hidden;
            row.hasClipPose = false;
            row.dirty = true;
        }
        for (int i = n - 1; i >= 0; i--) {
            Row row = nodes.get(i);
            if (row.parent >= 0) {
                Row p = nodes.get(row.parent);
                p.end = Math.max(p.end, row.end);
            }
        }
        for (int i = 0; i < n; i++) {
            Row row = nodes.get(i);
            if (row.parent >= 0) nodes.get(row.parent).children.add(i);
        }

        // Blobs in use, decoded once.
        Map<String, ClipData> clipsNow = new HashMap<>();
        Map<String, SkinBlob> skinsNow = new HashMap<>();
        Map<String, InstancesBlob> instancesNow = new HashMap<>();
        Set<String> used = new HashSet<>(), textures = new HashSet<>(), meshKeys = new HashSet<>();
        if (settings.envId != null) used.add(settings.envId);
        for (Row row : nodes) {
            FlatNode f = row.flat;
            Material m = f.material;
            if (f.mesh != null) meshKeys.add(row.meshKey);
            if (f.mesh != null && f.mesh.id != null) used.add(f.mesh.id);
            for (String src : new String[]{m.texture, m.mrMap, m.normalMap, m.emissiveMap, m.aoMap}) {
                if (src == null) continue;
                textures.add(src);
                if (src.startsWith("p3d:")) used.add(src.substring(4));
            }
            if (f.playClip != null) {
                used.add(f.playClip);
                ClipData c = clip(f.playClip);
                if (c != null) clipsNow.put(f.playClip, c);
            }
            if (f.skinId != null) {
                used.add(f.skinId);
                SkinBlob s = skins.get(f.skinId);
                if (s == null && blobs.get(f.skinId) instanceof SkinBlob) s = (SkinBlob) blobs.get(f.skinId);
                if (s != null) skinsNow.put(f.skinId, s);
            }
            if (f.instId != null) {
                used.add(f.instId);
                InstancesBlob s = instances.get(f.instId);
                if (s == null && blobs.get(f.instId) instanceof InstancesBlob) s = (InstancesBlob) blobs.get(f.instId);
                if (s != null) instancesNow.put(f.instId, s);
            }
        }
        clips = clipsNow;
        skins = skinsNow;
        instances = instancesNow;
        this.used = used;
        this.textures = textures;
        this.meshKeys = meshKeys;
        geometry.keySet().retainAll(meshKeys);

        // Clip targets and skin joints.
        for (int i = 0; i < n; i++) {
            Row row = nodes.get(i);
            if (row.playing != null) {
                Track[] tracks = {row.playing.current, row.playing.previous};
                for (Track track : tracks) {
                    if (track == null) continue;
                    ClipChannel[] channels = track.clip.channels;
                    track.targets = new int[channels.length];
                    for (int c = 0; c < channels.length; c++) track.targets[c] = resolve(i, channels[c].target);
                }
            }
            int[] joints = new int[0];
            if (row.flat.skinId != null) {
                SkinBlob skin = skins.get(row.flat.skinId);
                Integer root = index.get(row.flat.skinRoot);
                if (skin != null && root != null) {
                    joints = new int[skin.joints.length];
                    for (int j = 0; j < joints.length; j++) joints[j] = resolve(root, skin.joints[j]);
                }
            }
            row.jointNodes = joints;
        }

        IntList draw = new IntList(), spin = new IntList(), play = new IntList(), skin = new IntList();
        transitioning.clear();
        for (int i = 0; i < n; i++) {
            Row row = nodes.get(i);
            if (row.visible && row.flat.mesh != null) draw.add(i);
            if (row.visible && (row.flat.spin[0] != 0 || row.flat.spin[1] != 0 || row.flat.spin[2] != 0)) spin.add(i);
            if (row.from != null) transitioning.add(i);
            if (row.playing != null) play.add(i);
            if (row.visible && row.jointNodes.length > 0) skin.add(i);
        }
        // Fewer program, texture and buffer switches: the opaque draws of a
        // frame keep this order (stable, like the reference's sort).
        Integer[] sorted = new Integer[draw.n];
        for (int i = 0; i < draw.n; i++) sorted[i] = draw.a[i];
        Arrays.sort(sorted, (a, b) -> {
            Row x = nodes.get(a), y = nodes.get(b);
            int c = Integer.compare(variantOf(x), variantOf(y));
            if (c != 0) return c;
            c = compareOpt(x.flat.material.texture, y.flat.material.texture);
            return c != 0 ? c : x.meshKey.compareTo(y.meshKey);
        });
        drawNodes = new int[sorted.length];
        for (int i = 0; i < sorted.length; i++) drawNodes[i] = sorted[i];
        spinning = Arrays.copyOf(spin.a, spin.n);
        players = Arrays.copyOf(play.a, play.n);
        skinned = Arrays.copyOf(skin.a, skin.n);
    }

    /** The shader variant's order (`Variant`'s Ord: instanced, then skinned). */
    private static int variantOf(Row r) {
        return (r.flat.instId != null ? 2 : 0) + (r.jointNodes.length > 0 ? 1 : 0);
    }

    /** `Option<&str>`'s order: none first. */
    private static int compareOpt(String a, String b) {
        if (a == null) return b == null ? 0 : -1;
        return b == null ? 1 : a.compareTo(b);
    }

    /** The row a clip target or joint path names, from row `from`: `""` itself, `"0/2"` a child-index path, `"#name"` a named node; -1: none. */
    private int resolve(int from, String target) {
        if (target.startsWith("#")) {
            Integer i = index.get(target);
            return i != null ? i : -1;
        }
        int at = from;
        for (String step : target.split("/")) {
            if (step.isEmpty()) continue;
            int k;
            try {
                k = Integer.parseInt(step);
            } catch (NumberFormatException e) {
                return -1;
            }
            IntList kids = nodes.get(at).children;
            if (k < 0 || k >= kids.n) return -1;
            at = kids.a[k];
        }
        return at;
    }

    /** NaN when no transition runs at `now`. */
    private float transitionProgress(double start, double now) {
        Transition t = settings.transition;
        if (t == null || t.ms <= 0) return Float.NaN;
        float p = (float) ((now - start) * 1000.0 / t.ms);
        return p < 1 ? ease(t.bezier, Math.max(p, 0)) : Float.NaN;
    }

    /** A row's pose at `now` (its target, or its `current` scratch mid-transition). */
    private Pose poseAt(Row n, double now) {
        if (n.from == null) return n.target;
        float p = transitionProgress(n.fromStart, now);
        if (Float.isNaN(p)) return n.target;
        n.current.lerp(n.from, n.target, p, q8);
        return n.current;
    }

    /** The camera at `now` (`orbit`, or a scratch orbit mid-transition). */
    private Orbit currentOrbit(double now) {
        if (cameraFrom == null) return orbit;
        float p = transitionProgress(cameraFromStart, now);
        if (Float.isNaN(p)) return orbit;
        orbitScratch.lerp(cameraFrom, orbit, p);
        return orbitScratch;
    }

    /** The finger went down (auto-rotation pauses). */
    void pointerDown() {
        dragging = true;
    }

    /** The finger moved by `dx`, `dy` points while down: turns the camera if the orbit allows it. */
    void drag(float dx, float dy) {
        if (settings.orbit == null || !settings.orbit.rotate) return;
        settleCamera();
        orbit.yaw -= dx * DEGREES_PER_POINT;
        orbit.pitch = Math.max(-MAX_PITCH, Math.min(MAX_PITCH, orbit.pitch + dy * DEGREES_PER_POINT));
    }

    /** The finger went up. */
    void pointerUp() {
        dragging = false;
    }

    /** Pinch or scroll: `factor` above 1 moves closer. */
    void zoom(float factor) {
        if (settings.orbit == null || !settings.orbit.zoom || !(factor > 0)) return;
        settleCamera();
        if (!Float.isNaN(orbit.ortho)) orbit.ortho /= factor;
        else orbit.distance = Math.max(minDistance, Math.min(maxDistance, orbit.distance / factor));
    }

    /** Two-finger drag: moves the target with the content (`height`: the element's, points). */
    void pan(float dx, float dy, float height) {
        if (settings.orbit == null || !settings.orbit.pan || height <= 0) return;
        settleCamera();
        float[] eye = new float[3];
        orbit.eye(eye);
        float[] f = normalize(sub(orbit.target, eye));
        float[] right = normalize(cross(f, new float[]{0, 1, 0}));
        float[] up = cross(right, f);
        float visible = !Float.isNaN(orbit.ortho) ? orbit.ortho
                : (float) (2 * orbit.distance * Math.tan(Math.toRadians(orbit.fov) / 2));
        float k = visible / height;
        float[] moved = add(scale(right, -dx * k), scale(up, dy * k));
        for (int i = 0; i < 3; i++) orbit.target[i] += moved[i];
    }

    /** A gesture takes over from a running camera transition. */
    private void settleCamera() {
        if (cameraFrom != null) {
            orbit = currentOrbit(now).copy();
            cameraFrom = null;
        }
    }

    /** Moves time to `now`: auto-rotation turns by the time since the last frame; spins, transitions and clips move the nodes. */
    void advance(double now) {
        float dt = Double.isNaN(last) ? 0 : (float) Math.max(0, Math.min(0.25, now - last));
        last = now;
        this.now = now;
        OrbitSpec o = settings.orbit;
        if (o != null && o.auto != 0 && !dragging) {
            orbit.yaw += o.auto * dt;
            if (cameraFrom != null) cameraFrom.yaw += o.auto * dt;
        }
        if (cameraFrom != null && now >= cameraFromStart && Float.isNaN(transitionProgress(cameraFromStart, now))) {
            cameraFrom = null;
        }
        refresh(now, false);
    }

    /**
     * Recomputes what moved at `now`: clip poses, then the local matrices of
     * moving rows and the worlds of their subtrees, then skins. `all`: every
     * row (after an update).
     */
    private void refresh(double now, boolean all) {
        tick++;
        long tick = this.tick;
        boolean moving = false;
        touched.clear();

        // Fades that are over: the rows only the old clip moved go back to their own pose.
        for (int p : players) {
            Playing playing = nodes.get(p).playing;
            if (playing == null || playing.previous == null || !(now - playing.fadeStart >= playing.fade)) continue;
            Track track = playing.previous;
            playing.previous = null;
            playing.live = true;
            for (int t : track.targets) {
                if (t < 0) continue;
                nodes.get(t).hasClipPose = false;
                nodes.get(t).dirty = true;
                touched.add(t);
            }
        }
        // Clips: sample every running track (and, after an update, the
        // finished ones too: their last pose holds).
        posedRows.clear();
        for (int p : players) {
            Playing playing = nodes.get(p).playing;
            if (playing == null) continue;
            boolean running = playing.previous != null || playing.current != null && playing.current.running(now);
            boolean skip = !running && !playing.live && !all;
            playing.live = running;
            if (skip) continue;
            moving |= running;
            int pass = ++samplePass;
            newTargets.clear();
            oldTargets.clear();
            if (playing.current != null) sample(playing.current, now, pass, true, newTargets);
            if (playing.previous != null) {
                float w = Math.max(0, Math.min(1, (float) ((now - playing.fadeStart) / (double) playing.fade)));
                sample(playing.previous, now, pass, false, oldTargets);
                for (int k = 0; k < oldTargets.n; k++) {
                    Row row = nodes.get(oldTargets.a[k]);
                    ClipPose b = row.newStamp == pass ? row.sampleNew : NO_CLIP_POSE;
                    row.newStamp = 0; // taken
                    blend(oldTargets.a[k], row.sampleOld, b, w, now, posed(oldTargets.a[k], tick));
                }
                for (int k = 0; k < newTargets.n; k++) {
                    Row row = nodes.get(newTargets.a[k]);
                    if (row.newStamp == pass) blend(newTargets.a[k], NO_CLIP_POSE, row.sampleNew, w, now, posed(newTargets.a[k], tick));
                }
            } else {
                for (int k = 0; k < newTargets.n; k++) {
                    posed(newTargets.a[k], tick).set(nodes.get(newTargets.a[k]).sampleNew);
                }
            }
        }
        for (int k = 0; k < posedRows.n; k++) {
            int target = posedRows.a[k];
            Row row = nodes.get(target);
            if (!row.hasClipPose || !row.clipPose.same(row.posed)) {
                row.clipPose.set(row.posed);
                row.hasClipPose = true;
                row.dirty = true;
                touched.add(target);
            }
        }

        // Spins and transitions.
        for (int i : spinning) {
            nodes.get(i).dirty = true;
            touched.add(i);
            moving = true;
        }
        done.clear();
        for (int k = 0; k < transitioning.n; k++) {
            int i = transitioning.a[k];
            Row row = nodes.get(i);
            row.dirty = true;
            touched.add(i);
            if (row.from != null && (now < row.fromStart || !Float.isNaN(transitionProgress(row.fromStart, now)))) {
                moving = true;
            } else {
                done.add(i);
            }
        }
        if (done.n > 0) {
            for (int k = 0; k < done.n; k++) nodes.get(done.a[k]).from = null;
            int kept = 0;
            for (int k = 0; k < transitioning.n; k++) {
                if (!done.contains(transitioning.a[k])) transitioning.a[kept++] = transitioning.a[k];
            }
            transitioning.n = kept;
        }

        // Locals, then the worlds of the subtrees of the touched rows (in
        // pre-order, a subtree is a range: one pass covers nested ones).
        int n = nodes.size();
        if (all) {
            touched.clear();
            for (int i = 0; i < n; i++) touched.add(i);
        } else {
            Arrays.sort(touched.a, 0, touched.n);
        }
        int covered = 0;
        for (int k = 0; k < touched.n; k++) {
            int i = touched.a[k];
            if (i < covered) continue;
            covered = nodes.get(i).end;
            for (int j = i; j < covered; j++) {
                Row row = nodes.get(j);
                if (all || row.dirty) {
                    local(j, now, row.local);
                    row.dirty = false;
                }
                updateWorld(j, tick);
            }
        }

        // Skins whose joints or node moved.
        for (int s : skinned) {
            Row row = nodes.get(s);
            boolean moved = all || row.moved == tick;
            for (int k = 0; !moved && k < row.jointNodes.length; k++) {
                moved = row.jointNodes[k] >= 0 && nodes.get(row.jointNodes[k]).moved == tick;
            }
            if (!moved) continue;
            SkinBlob skin = row.flat.skinId != null ? skins.get(row.flat.skinId) : null;
            if (skin == null) continue;
            int count = row.jointNodes.length;
            if (row.placed.length != 16 * count) row.placed = new float[16 * count];
            if (row.joints.length != 16 * count) row.joints = new float[16 * count];
            // Where each joint puts the mesh: world · J_i = world(joint i) ·
            // inverse_bind[i] (the node's own transform places nothing).
            for (int k = 0; k < count; k++) {
                int j = row.jointNodes[k];
                if (j >= 0) {
                    if (16 * (k + 1) <= skin.inverseBind.length) {
                        System.arraycopy(skin.inverseBind, 16 * k, m16, 0, 16);
                    } else {
                        System.arraycopy(IDENTITY, 0, m16, 0, 16);
                    }
                    mul(nodes.get(j).world, 0, m16, 0, row.placed, 16 * k);
                } else {
                    System.arraycopy(row.world, 0, row.placed, 16 * k, 16);
                }
            }
            invert(row.world, m16b);
            for (int k = 0; k < count; k++) mul(m16b, 0, row.placed, 16 * k, row.joints, 16 * k);
            row.jointCount = count;
            // Every skinned vertex is a weighted mean of `world · J_i · v`, so
            // the union of the mesh box under each `world · J_i` holds them.
            if (row.flat.mesh != null && row.flat.instId == null && count > 0) {
                float[] b = row.flat.mesh.bounds(box6);
                for (int k = 0; k < count; k++) {
                    System.arraycopy(row.placed, 16 * k, m16, 0, 16);
                    if (k == 0) {
                        transformAabb(m16, b, row.bounds);
                    } else {
                        transformAabb(m16, b, v6);
                        aabbUnion(row.bounds, v6, row.bounds);
                    }
                }
            }
        }
        this.moving = moving;
    }

    private final float[] v6 = new float[6];

    /** Samples `track` at `now` into the rows' new or old scratch poses (stamped `pass`), listing the rows in `out`. */
    private void sample(Track track, double now, int pass, boolean isNew, IntList out) {
        float t = track.time(now);
        ClipChannel[] channels = track.clip.channels;
        for (int c = 0; c < channels.length && c < track.targets.length; c++) {
            int target = track.targets[c];
            if (target < 0) continue;
            ClipChannel ch = channels[c];
            ch.sample(t, v4);
            Row row = nodes.get(target);
            ClipPose pose;
            if (isNew) {
                if (row.newStamp != pass) {
                    row.newStamp = pass;
                    row.sampleNew.clear();
                    out.add(target);
                }
                pose = row.sampleNew;
            } else {
                if (row.oldStamp != pass) {
                    row.oldStamp = pass;
                    row.sampleOld.clear();
                    out.add(target);
                }
                pose = row.sampleOld;
            }
            switch (ch.property) {
                case TRANSLATION: pose.hasT = true; System.arraycopy(v4, 0, pose.t, 0, 3); break;
                case ROTATION: pose.hasR = true; System.arraycopy(v4, 0, pose.r, 0, 4); break;
                default: pose.hasS = true; System.arraycopy(v4, 0, pose.s, 0, 3);
            }
        }
    }

    /** Row `target`'s pose for this refresh (listed once in `posedRows`). */
    private ClipPose posed(int target, long tick) {
        Row row = nodes.get(target);
        if (row.posedStamp != tick) {
            row.posedStamp = tick;
            posedRows.add(target);
        }
        return row.posed;
    }

    /** Blends two clips' poses of row `target` into `out` (`w`: the new one's weight); a part only one clip sets blends with the row's own pose. */
    private void blend(int target, ClipPose a, ClipPose b, float w, double now, ClipPose out) {
        Pose own = poseAt(nodes.get(target), now);
        ClipPose r = blendScratch;
        r.clear();
        if (a.hasT || b.hasT) {
            r.hasT = true;
            lerp3(a.hasT ? a.t : own.position, b.hasT ? b.t : own.position, w, r.t);
        }
        if (a.hasR || b.hasR) {
            float[] q = q8;
            if (own.hasQuat) System.arraycopy(own.quat, 0, q, 0, 4);
            else quatFromEuler(own.rotation, q, 0);
            float[] x = a.hasR ? a.r : q, y = b.hasR ? b.r : q;
            r.hasR = true;
            slerp(x[0], x[1], x[2], x[3], y[0], y[1], y[2], y[3], w, r.r);
        }
        if (a.hasS || b.hasS) {
            r.hasS = true;
            lerp3(a.hasS ? a.s : own.scale, b.hasS ? b.s : own.scale, w, r.s);
        }
        out.set(r);
    }

    /** Row `i`'s local matrix at `now`, into `out`. */
    private void local(int i, double now, float[] out) {
        Row row = nodes.get(i);
        Pose pose = poseAt(row, now);
        ClipPose clip = row.hasClipPose ? row.clipPose : NO_CLIP_POSE;
        float[] position = clip.hasT ? clip.t : pose.position;
        float[] scale = clip.hasS ? clip.s : pose.scale;
        float[] spin = row.flat.spin;
        boolean turns = (spin[0] != 0 || spin[1] != 0 || spin[2] != 0) && row.visible;
        float elapsed = (float) (now - row.appeared);
        float tx = turns ? spin[0] * elapsed : 0, ty = turns ? spin[1] * elapsed : 0, tz = turns ? spin[2] * elapsed : 0;
        float[] q = clip.hasR ? clip.r : pose.hasQuat ? pose.quat : null;
        if (q != null) {
            float[] r = q8;
            System.arraycopy(q, 0, r, 0, 4);
            if (tx != 0 || ty != 0 || tz != 0) {
                quatFromEuler(tx, ty, tz, r, 4);
                quatMul(q[0], q[1], q[2], q[3], r[4], r[5], r[6], r[7], r);
            }
            composeQuat(position[0], position[1], position[2], r[0], r[1], r[2], r[3], scale[0], scale[1], scale[2], out);
        } else {
            compose(position, pose.rotation[0] + tx, pose.rotation[1] + ty, pose.rotation[2] + tz, scale, out);
        }
    }

    /** Row `i`'s world matrix from its parent's, and what follows from it. */
    private void updateWorld(int i, long tick) {
        Row row = nodes.get(i);
        float[] world = m16;
        if (row.parent >= 0) mul(nodes.get(row.parent).world, 0, row.local, 0, world, 0);
        else System.arraycopy(row.local, 0, world, 0, 16);
        if (!eq(row.world, world) || row.moved == 0) {
            System.arraycopy(world, 0, row.world, 0, 16);
            row.moved = tick;
            normalMatrix(world, row.normal, m16b);
        }
        // A skinned mesh's box follows its joints (set with them, after the
        // worlds); until they resolve, its node places it.
        boolean skinnedRow = row.jointNodes.length > 0 && row.flat.instId == null;
        if (row.flat.mesh != null && !skinnedRow) {
            float[] local = row.flat.instId != null ? row.flat.instBounds : row.flat.mesh.bounds(box6);
            transformAabb(row.world, local, row.bounds);
        }
    }

    /**
     * The frame at the time of the last {@link #advance}, for a viewport of
     * `width` × `height` points, into `f` (copies: the caller may draw it while
     * the player changes).
     */
    Frame frame(float width, float height, boolean dark, Frame f) {
        Orbit cam = currentOrbit(now);
        float[] eye = f.cameraPosition;
        cam.eye(eye);
        float aspect = height > 0 ? width / height : 1;
        Camera c = settings.camera;
        f.width = width;
        f.height = height;
        if (!Float.isNaN(cam.ortho)) orthographic(cam.ortho, aspect, c.near, c.far, f.projection);
        else perspective(cam.fov, aspect, c.near, c.far, f.projection);
        lookAt(eye, cam.target, up, f.view);
        mul(f.projection, 0, f.view, 0, f.viewProj, 0);
        invert(f.viewProj, f.inverseViewProj);

        if (settings.background != null) {
            float[] bg = settings.background.resolve(dark);
            f.clear[0] = bg[0] * bg[3];
            f.clear[1] = bg[1] * bg[3];
            f.clear[2] = bg[2] * bg[3];
            f.clear[3] = bg[3];
        } else {
            Arrays.fill(f.clear, 0);
        }
        Arrays.fill(f.ambient, 0);
        f.lightCount = 0;
        int shadowLight = -1;
        List<Light> lights = settings.effectiveLights();
        for (int k = 0; k < lights.size(); k++) {
            Light l = lights.get(k);
            float[] rgb = l.color.resolve(dark);
            float r = srgbToLinear(rgb[0]) * l.intensity, g = srgbToLinear(rgb[1]) * l.intensity,
                    b = srgbToLinear(rgb[2]) * l.intensity;
            if (l.kind == AMBIENT) {
                f.ambient[0] += r;
                f.ambient[1] += g;
                f.ambient[2] += b;
                continue;
            }
            if (f.lightCount >= MAX_LIGHTS) continue;
            FrameLight fl = f.lights[f.lightCount];
            if (l.kind == DIRECTIONAL && l.shadows && shadowLight < 0) shadowLight = f.lightCount;
            fl.kind = l.kind;
            fl.color[0] = r;
            fl.color[1] = g;
            fl.color[2] = b;
            System.arraycopy(l.position, 0, fl.position, 0, 3);
            float len = length(l.direction);
            if (len == 0) {
                fl.direction[0] = 0;
                fl.direction[1] = -1;
                fl.direction[2] = 0;
            } else {
                for (int i = 0; i < 3; i++) fl.direction[i] = l.direction[i] * (1 / len);
            }
            double half = Math.toRadians(Math.max(l.angle, 1) / 2);
            fl.params[0] = l.kind == DIRECTIONAL ? 0 : l.range;
            fl.params[1] = l.kind == POINT ? -1 : l.kind == SPOT ? (float) Math.cos(half) : 0;
            fl.params[2] = l.kind == POINT ? -1 : l.kind == SPOT ? (float) Math.cos(half * 0.75) : 0;
            fl.params[3] = 0;
            f.lightCount++;
        }

        frustumPlanes(f.viewProj, planes);
        int count = 0, transparentCount = 0;
        for (int i : drawNodes) {
            Draw d = f.draw(count);
            if (!draw(i, dark, d) || !aabbInFrustum(planes, d.bounds)) continue;
            if (!d.transparent) {
                count++;
                continue;
            }
            // Back to front: by the distance to its box's center. It moves to
            // the transparent pool, whose spare takes its slot.
            float[] b = d.bounds;
            float dx = (b[0] + b[3]) / 2 - eye[0], dy = (b[1] + b[4]) / 2 - eye[1], dz = (b[2] + b[5]) / 2 - eye[2];
            d.depth = (float) Math.sqrt(dx * dx + dy * dy + dz * dz);
            f.draws[count] = f.spare(transparentCount);
            // Insertion sort, farthest first (stable).
            int at = transparentCount++;
            while (at > 0 && f.transparent[at - 1].depth < d.depth) {
                f.transparent[at] = f.transparent[at - 1];
                at--;
            }
            f.transparent[at] = d;
        }
        for (int k = 0; k < transparentCount; k++) {
            f.draw(count);
            Draw spare = f.draws[count];
            f.draws[count++] = f.transparent[k];
            f.transparent[k] = spare;
        }
        f.drawCount = count;

        shadowPass(shadowLight, f);
        f.env = settings.envId;
        f.envIntensity = settings.envIntensity;
        f.envBackground = settings.envBackground;
        f.exposure = settings.exposure;
        f.animating = moving || cameraFrom != null || settings.orbit != null && settings.orbit.auto != 0 && !dragging;
        return f;
    }

    /** Row `i`'s draw into `d` (false: invisible, alpha 0). */
    private boolean draw(int i, boolean dark, Draw d) {
        Row row = nodes.get(i);
        Mesh mesh = row.flat.mesh;
        if (mesh == null) return false;
        Material m = row.flat.material;
        Pose pose = poseAt(row, now);
        float[] rgba = pose.color(dark), e = pose.emissive(dark);
        float a = m.alpha == OPAQUE ? 1 : rgba[3];
        // Invisible: neither drawn nor hit.
        if (a <= 0) return false;
        d.node = i;
        d.key = row.flat.key;
        d.name = row.named >= 0 ? nodes.get(row.named).flat.name : null;
        d.mesh = mesh;
        d.meshKey = row.meshKey;
        System.arraycopy(row.world, 0, d.world, 0, 16);
        System.arraycopy(row.normal, 0, d.normal, 0, 9);
        d.color[0] = srgbToLinear(rgba[0]);
        d.color[1] = srgbToLinear(rgba[1]);
        d.color[2] = srgbToLinear(rgba[2]);
        d.color[3] = a;
        d.emissive[0] = srgbToLinear(e[0]);
        d.emissive[1] = srgbToLinear(e[1]);
        d.emissive[2] = srgbToLinear(e[2]);
        d.metalness = pose.metalness;
        d.roughness = pose.roughness;
        d.unlit = m.unlit;
        d.doubleSided = m.doubleSided;
        d.texture = m.texture;
        d.mrMap = m.mrMap;
        d.normalMap = m.normalMap;
        d.normalScale = m.normalScale;
        d.emissiveMap = m.emissiveMap;
        d.aoMap = m.aoMap;
        d.alpha = m.alpha;
        d.alphaCutoff = m.cutoff;
        d.transparent = m.alpha == BLEND && a < 1;
        d.instances = row.flat.instId;
        d.instanceCount = row.flat.instCount;
        if (row.jointNodes.length > 0 && row.jointCount == row.jointNodes.length) {
            int n = 16 * row.jointCount;
            if (d.joints.length < n) d.joints = new float[n];
            System.arraycopy(row.joints, 0, d.joints, 0, n);
            d.jointCount = row.jointCount;
        } else {
            d.jointCount = -1;
        }
        d.castShadows = row.flat.castShadows;
        d.receiveShadows = row.flat.receiveShadows;
        System.arraycopy(row.bounds, 0, d.bounds, 0, 6);
        return true;
    }

    private final float[] shadowRotate = new float[16], shadowView = new float[16], shadowProjection = new float[16],
            shadowC = new float[3], zero3 = new float[3], up0 = {0, 1, 0}, up1 = {0, 0, 1};

    /** The shadow map fitted to the visible draws, seen along light `index` (none: -1). */
    private void shadowPass(int index, Frame f) {
        f.hasShadow = false;
        f.casterCount = 0;
        if (index < 0 || f.drawCount == 0) return;
        float[] union = box6;
        System.arraycopy(f.draws[0].bounds, 0, union, 0, 6);
        for (int i = 1; i < f.drawCount; i++) aabbUnion(union, f.draws[i].bounds, union);
        if (f.casters.length < f.drawCount) f.casters = new int[f.drawCount];
        for (int i = 0; i < f.drawCount; i++) if (f.draws[i].castShadows) f.casters[f.casterCount++] = i;
        if (f.casterCount == 0) return;
        float cx = (union[0] + union[3]) / 2, cy = (union[1] + union[4]) / 2, cz = (union[2] + union[5]) / 2;
        float ex = union[3] - union[0], ey = union[4] - union[1], ez = union[5] - union[2];
        float radius = (float) Math.sqrt(ex * ex + ey * ey + ez * ez) / 2;
        // The radius in steps of 1/16 of its power of two, and the center on
        // whole texels: the map does not shimmer while the scene stays put.
        float r = Math.max(radius, 1e-3f);
        float step = (float) Math.pow(2, Math.ceil(log2(r)) - 4);
        radius = (float) Math.ceil(r / step) * step;
        float size = SHADOW_MAP_SIZE;
        float texel = 2 * radius / size;
        float[] dir = f.lights[index].direction;
        lookAt(zero3, dir, Math.abs(dir[1]) > 0.99f ? up1 : up0, shadowRotate);
        float[] c = shadowC;
        c[0] = cx;
        c[1] = cy;
        c[2] = cz;
        transformPoint(shadowRotate, c, c);
        System.arraycopy(IDENTITY, 0, m16, 0, 16);
        m16[12] = -snap(c[0], texel);
        m16[13] = -snap(c[1], texel);
        mul(m16, 0, shadowRotate, 0, shadowView, 0);
        // One texel of margin for the snap; depth around the center (view −Z).
        float half = radius + texel;
        orthographic(2 * half, 1, -c[2] - radius, -c[2] + radius, shadowProjection);
        mul(shadowProjection, 0, shadowView, 0, f.shadowViewProj, 0);
        f.hasShadow = true;
        f.shadowLight = index;
        f.shadowMapSize = SHADOW_MAP_SIZE;
        f.shadowBias = 1.5f / size;
        f.shadowNormalBias = 1.5f * texel;
    }

    /** f32's `log2` (exact for powers of two). */
    private static float log2(float x) {
        int bits = Float.floatToIntBits(x);
        int exp = ((bits >> 23) & 0xff) - 127;
        if ((bits & 0x7fffff) == 0) return exp;
        return (float) (Math.log(x) / Math.log(2));
    }

    private static float snap(float v, float texel) {
        return (float) Math.floor(v / texel) * texel;
    }

    /** The triangles of `draw` as stored (unskinned; cached by mesh key; null while a blob mesh has not arrived). */
    MeshData geometry(Draw draw) {
        MeshData g = geometry.get(draw.meshKey);
        if (g != null) return g;
        g = generate(draw.mesh, blobs);
        // A blob that has not arrived is not cached: it is retried.
        if (g != null) geometry.put(draw.meshKey, g);
        return g;
    }

    /** What a tap at `(x, y)` (points from the element's top-left) hits in the current frame. */
    Tap hit(float x, float y, float width, float height, boolean dark) {
        Tap tap = new Tap();
        if (width <= 0 || height <= 0) return tap;
        Frame frame = frame(width, height, dark, hitFrame);
        float nx = 2 * x / width - 1, ny = 1 - 2 * y / height;
        float[] near = transformPoint(frame.inverseViewProj, new float[]{nx, ny, -1});
        float[] far = transformPoint(frame.inverseViewProj, new float[]{nx, ny, 1});
        float[] dir = sub(far, near);
        // Nearest so far: ray parameter, row, instance.
        float[] best = {Float.NaN};
        int[] bestRow = {-1}, bestInstance = {-1};
        for (int k = 0; k < frame.drawCount; k++) {
            Draw d = frame.draws[k];
            float t0 = rayAabb(near, dir, d.bounds);
            if (Float.isNaN(t0) || !closer(t0, best[0])) continue;
            MeshData g = geometry(d);
            if (g == null) continue;
            float[] localBox = d.mesh.bounds(new float[6]);
            boolean skin = d.skinned() && g.joints != null && g.joints.length == g.vertexCount() * 4;
            InstancesBlob set = d.instances != null ? instances.get(d.instances) : null;
            int n = set != null ? set.count : 1;
            for (int inst = 0; inst < n; inst++) {
                // Where the mesh is: its world, times an instance's matrix.
                float[] m = d.world;
                if (set != null) {
                    m = new float[16];
                    mul(d.world, 0, set.matrices, 16 * inst, m, 0);
                }
                int instance = set != null ? inst : -1;
                if (skin) {
                    // Skinned: its triangles in the current pose, in the world.
                    float[] placed = new float[16 * d.jointCount];
                    for (int j = 0; j < d.jointCount; j++) mul(m, 0, d.joints, 16 * j, placed, 16 * j);
                    float[] p = new float[g.vertexCount() * 3];
                    for (int v = 0; v < g.vertexCount(); v++) skinnedPosition(g, v, placed, d.jointCount, m, p);
                    for (int t = 0; t + 2 < g.indices.length; t += 3) {
                        float h = rayTriangle(near, dir, vec(p, g.indices[t]), vec(p, g.indices[t + 1]), vec(p, g.indices[t + 2]));
                        if (!Float.isNaN(h) && closer(h, best[0])) {
                            best[0] = h;
                            bestRow[0] = d.node;
                            bestInstance[0] = instance;
                        }
                    }
                    continue;
                }
                float[] inv = invert(m);
                float[] o = transformPoint(inv, near);
                float[] v = transformDir(inv, dir);
                float tb = rayAabb(o, v, localBox);
                if (Float.isNaN(tb) || !closer(tb, best[0])) continue;
                for (int t = 0; t + 2 < g.indices.length; t += 3) {
                    float h = rayTriangle(o, v, g.position(g.indices[t]), g.position(g.indices[t + 1]), g.position(g.indices[t + 2]));
                    if (!Float.isNaN(h) && closer(h, best[0])) {
                        best[0] = h;
                        bestRow[0] = d.node;
                        bestInstance[0] = instance;
                    }
                }
            }
        }
        if (bestRow[0] >= 0) {
            Row row = nodes.get(bestRow[0]);
            tap.node = row.named >= 0 ? nodes.get(row.named).flat.name : null;
            tap.part = row.part >= 0 ? nodes.get(row.part).flat.label : null;
            tap.instance = bestInstance[0];
            tap.point = add(near, scale(dir, best[0]));
        }
        return tap;
    }

    private static boolean closer(float t, float best) {
        return Float.isNaN(best) || t < best;
    }

    /**
     * Vertex `i` of a skinned mesh in the current pose into `out[3i..]`: `Σ w_k ·
     * placed[j_k] · v` (`placed[j] = world · J[j]`), like the `SKINNED` vertex
     * shader; by `world` alone if its weights are all zero.
     */
    private static void skinnedPosition(MeshData g, int i, float[] placed, int joints, float[] world, float[] out) {
        float px = g.positions[i * 3], py = g.positions[i * 3 + 1], pz = g.positions[i * 3 + 2];
        float x = 0, y = 0, z = 0, total = 0;
        for (int k = 0; k < 4; k++) {
            float w = g.weights != null && i * 4 + k < g.weights.length ? g.weights[i * 4 + k] : 0;
            int j = g.joints[i * 4 + k] & 0xffff;
            if (j >= joints || w == 0) continue;
            int o = 16 * j;
            float tx = placed[o] * px + placed[o + 4] * py + placed[o + 8] * pz + placed[o + 12];
            float ty = placed[o + 1] * px + placed[o + 5] * py + placed[o + 9] * pz + placed[o + 13];
            float tz = placed[o + 2] * px + placed[o + 6] * py + placed[o + 10] * pz + placed[o + 14];
            float tw = placed[o + 3] * px + placed[o + 7] * py + placed[o + 11] * pz + placed[o + 15];
            if (tw != 0 && tw != 1) {
                tx /= tw;
                ty /= tw;
                tz /= tw;
            }
            x += tx * w;
            y += ty * w;
            z += tz * w;
            total += w;
        }
        if (total == 0) {
            float[] p = transformPoint(world, new float[]{px, py, pz});
            x = p[0];
            y = p[1];
            z = p[2];
        }
        out[i * 3] = x;
        out[i * 3 + 1] = y;
        out[i * 3 + 2] = z;
    }

    // --- Geometry (geometry.rs) -----------------------------------------------------------

    /**
     * Vertex data ready for a GPU: flat x,y,z / u,v arrays and three indices per
     * triangle. Optional attributes (uv, colors, skin) are null when absent.
     */
    static final class MeshData {
        float[] positions = new float[0], normals = new float[0], uvs, colors, weights;
        /** Four unsigned 16-bit joint indices per vertex. */
        short[] joints;
        int[] indices = new int[0];
        /** Its positions' box (blob meshes). */
        float[] bounds;

        float[] position(int i) {
            return new float[]{positions[i * 3], positions[i * 3 + 1], positions[i * 3 + 2]};
        }

        int vertexCount() {
            return positions.length / 3;
        }
    }

    /** Grows a MeshData. */
    private static final class Builder {
        float[] p = new float[64], n = new float[64], uv = new float[64];
        int[] idx = new int[64];
        int vertices, count;

        int vertex(float[] pos, float[] normal, float u, float v) {
            if ((vertices + 1) * 3 > p.length) {
                p = Arrays.copyOf(p, p.length * 2);
                n = Arrays.copyOf(n, n.length * 2);
                uv = Arrays.copyOf(uv, uv.length * 2);
            }
            System.arraycopy(pos, 0, p, vertices * 3, 3);
            System.arraycopy(normal, 0, n, vertices * 3, 3);
            uv[vertices * 2] = u;
            uv[vertices * 2 + 1] = v;
            return vertices++;
        }

        void triangle(int a, int b, int c) {
            if (count + 3 > idx.length) idx = Arrays.copyOf(idx, idx.length * 2);
            idx[count++] = a;
            idx[count++] = b;
            idx[count++] = c;
        }

        MeshData build() {
            MeshData m = new MeshData();
            m.positions = Arrays.copyOf(p, vertices * 3);
            m.normals = Arrays.copyOf(n, vertices * 3);
            m.uvs = Arrays.copyOf(uv, vertices * 2);
            m.indices = Arrays.copyOf(idx, count);
            return m;
        }
    }

    /** The triangles of `mesh` (a blob mesh: its decoded blob, null until it has arrived). */
    static MeshData generate(Mesh mesh, Blobs blobs) {
        switch (mesh.type) {
            case "box": return cuboid(mesh.size);
            case "sphere": return sphere(mesh.radius);
            case "plane": return plane(mesh.size);
            case "cylinder": return cylinder(mesh.radius, mesh.radius, mesh.height);
            case "cone": return cylinder(mesh.radius, 0, mesh.height);
            case "torus": return torus(mesh.radius, mesh.tube);
            default: {
                Object b = blobs.get(mesh.id);
                return b instanceof MeshData ? (MeshData) b : null;
            }
        }
    }

    /** A box centered on the origin: faces +X, −X, +Y, −Y, +Z, −Z, four vertices each. */
    static MeshData cuboid(float[] size) {
        float[] half = {size[0] / 2, size[1] / 2, size[2] / 2};
        Builder m = new Builder();
        // (normal, right, up) per face, seen from outside: right × up = normal.
        float[][][] faces = {
                {{1, 0, 0}, {0, 0, -1}, {0, 1, 0}},
                {{-1, 0, 0}, {0, 0, 1}, {0, 1, 0}},
                {{0, 1, 0}, {1, 0, 0}, {0, 0, -1}},
                {{0, -1, 0}, {1, 0, 0}, {0, 0, 1}},
                {{0, 0, 1}, {1, 0, 0}, {0, 1, 0}},
                {{0, 0, -1}, {-1, 0, 0}, {0, 1, 0}},
        };
        for (float[][] face : faces) {
            float[] n = face[0], r = face[1], u = face[2];
            float dn = extent(n, half), dr = extent(r, half), du = extent(u, half);
            // Top-left, top-right, bottom-right, bottom-left.
            int a = m.vertex(faceAt(n, r, u, dn, dr, du, -1, 1), n, 0, 0);
            int b = m.vertex(faceAt(n, r, u, dn, dr, du, 1, 1), n, 1, 0);
            int c = m.vertex(faceAt(n, r, u, dn, dr, du, 1, -1), n, 1, 1);
            int d = m.vertex(faceAt(n, r, u, dn, dr, du, -1, -1), n, 0, 1);
            m.triangle(a, d, c);
            m.triangle(a, c, b);
        }
        return m.build();
    }

    private static float extent(float[] v, float[] half) {
        return Math.abs(v[0]) * half[0] + Math.abs(v[1]) * half[1] + Math.abs(v[2]) * half[2];
    }

    private static float[] faceAt(float[] n, float[] r, float[] u, float dn, float dr, float du, float sr, float su) {
        float[] p = new float[3];
        for (int i = 0; i < 3; i++) p[i] = n[i] * dn + r[i] * dr * sr + u[i] * du * su;
        return p;
    }

    /** A UV sphere: RINGS rings from the top pole (v = 0), SEGMENTS + 1 vertices around each. */
    static MeshData sphere(float radius) {
        Builder m = new Builder();
        for (int i = 0; i <= RINGS; i++) {
            float v = (float) i / RINGS;
            float theta = v * (float) Math.PI;
            for (int j = 0; j <= SEGMENTS; j++) {
                float u = (float) j / SEGMENTS;
                float phi = u * (float) (2 * Math.PI);
                float[] n = {(float) (-Math.sin(theta) * Math.sin(phi)), (float) Math.cos(theta),
                        (float) (Math.sin(theta) * Math.cos(phi))};
                m.vertex(scale(n, radius), n, u, v);
            }
        }
        int row = SEGMENTS + 1;
        for (int i = 0; i < RINGS; i++) {
            for (int j = 0; j < SEGMENTS; j++) {
                int a = i * row + j, b = i * row + j + 1;
                int c = (i + 1) * row + j, d = (i + 1) * row + j + 1;
                if (i != 0) m.triangle(a, b, c);
                if (i != RINGS - 1) m.triangle(b, d, c);
            }
        }
        return m.build();
    }

    /** A rectangle in the XZ plane facing +Y; u along +X, v along +Z. */
    static MeshData plane(float[] size) {
        float hx = size[0] / 2, hz = size[1] / 2;
        Builder m = new Builder();
        float[] n = {0, 1, 0};
        int a = m.vertex(new float[]{-hx, 0, -hz}, n, 0, 0);
        int b = m.vertex(new float[]{hx, 0, -hz}, n, 1, 0);
        int c = m.vertex(new float[]{hx, 0, hz}, n, 1, 1);
        int d = m.vertex(new float[]{-hx, 0, hz}, n, 0, 1);
        m.triangle(a, d, c);
        m.triangle(a, c, b);
        return m.build();
    }

    /** A capped cylinder along Y (a cone when `top` is 0), centered: side, top cap, bottom cap. */
    static MeshData cylinder(float bottom, float top, float height) {
        Builder m = new Builder();
        float h = height / 2;
        // The side's normal leans up by the slope (r_bottom - r_top) / height.
        float slope = height > 0 ? (bottom - top) / height : 0;
        for (int j = 0; j <= SEGMENTS; j++) {
            float u = (float) j / SEGMENTS;
            float phi = u * (float) (2 * Math.PI);
            float s = (float) Math.sin(phi), c = (float) Math.cos(phi);
            float[] n = normalize(new float[]{s, slope, c});
            m.vertex(new float[]{top * s, h, top * c}, n, u, 0);
            m.vertex(new float[]{bottom * s, -h, bottom * c}, n, u, 1);
        }
        for (int j = 0; j < SEGMENTS; j++) {
            int t0 = j * 2, b0 = j * 2 + 1, t1 = j * 2 + 2, b1 = j * 2 + 3;
            if (top > 0) m.triangle(t0, b0, t1);
            m.triangle(t1, b0, b1);
        }
        float[][] caps = {{top, h, 1}, {bottom, -h, -1}};
        for (float[] cap : caps) {
            float radius = cap[0], y = cap[1], ny = cap[2];
            if (radius <= 0) continue;
            int center = m.vertex(new float[]{0, y, 0}, new float[]{0, ny, 0}, 0.5f, 0.5f);
            int first = center + 1;
            for (int j = 0; j <= SEGMENTS; j++) {
                float phi = (float) j / SEGMENTS * (float) (2 * Math.PI);
                float s = (float) Math.sin(phi), c = (float) Math.cos(phi);
                m.vertex(new float[]{radius * s, y, radius * c}, new float[]{0, ny, 0}, 0.5f + 0.5f * s, 0.5f + 0.5f * c * -ny);
            }
            for (int j = 0; j < SEGMENTS; j++) {
                if (ny > 0) m.triangle(center, first + j, first + j + 1);
                else m.triangle(center, first + j + 1, first + j);
            }
        }
        return m.build();
    }

    /** A torus in the XZ plane: SEGMENTS around the ring, RINGS around the tube. */
    static MeshData torus(float radius, float tube) {
        Builder m = new Builder();
        for (int i = 0; i <= SEGMENTS; i++) {
            float u = (float) i / SEGMENTS;
            float phi = u * (float) (2 * Math.PI);
            float s = (float) Math.sin(phi), c = (float) Math.cos(phi);
            for (int j = 0; j <= RINGS; j++) {
                float v = (float) j / RINGS;
                float theta = v * (float) (2 * Math.PI);
                float ts = (float) Math.sin(theta), tc = (float) Math.cos(theta);
                // Outward from the tube's center circle.
                float[] n = {s * tc, ts, c * tc};
                float[] p = {s * (radius + tube * tc), tube * ts, c * (radius + tube * tc)};
                m.vertex(p, n, u, v);
            }
        }
        int row = RINGS + 1;
        for (int i = 0; i < SEGMENTS; i++) {
            for (int j = 0; j < RINGS; j++) {
                int a = i * row + j, b = i * row + j + 1;
                int c = (i + 1) * row + j, d = (i + 1) * row + j + 1;
                m.triangle(a, c, b);
                m.triangle(b, c, d);
            }
        }
        return m.build();
    }

    private static float[] vec(float[] a, int i) {
        return new float[]{a[i * 3], a[i * 3 + 1], a[i * 3 + 2]};
    }

    /** Where the ray `origin + t · dir` crosses triangle `a b c` (either face): t in front of the origin, else NaN. */
    static float rayTriangle(float[] origin, float[] dir, float[] a, float[] b, float[] c) {
        float[] e1 = sub(b, a), e2 = sub(c, a);
        float[] p = cross(dir, e2);
        float det = dot(e1, p);
        if (Math.abs(det) < 1e-9f) return Float.NaN;
        float inv = 1 / det;
        float[] s = sub(origin, a);
        float u = dot(s, p) * inv;
        if (!(u >= 0 && u <= 1)) return Float.NaN;
        float[] q = cross(s, e1);
        float v = dot(dir, q) * inv;
        if (v < 0 || u + v > 1) return Float.NaN;
        float t = dot(e2, q) * inv;
        return t > 1e-6f ? t : Float.NaN;
    }

    // --- Math (math.rs): column-major 4×4, element (row, col) at col * 4 + row ------------

    static final float[] IDENTITY = {1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1};

    /** `a · b` (`b` applies first) into `out` at the offsets; `out` must not overlap `a` or `b`. */
    static void mul(float[] a, int ao, float[] b, int bo, float[] out, int oo) {
        for (int c = 0; c < 4; c++) {
            for (int r = 0; r < 4; r++) {
                float sum = 0;
                for (int k = 0; k < 4; k++) sum += a[ao + k * 4 + r] * b[bo + c * 4 + k];
                out[oo + c * 4 + r] = sum;
            }
        }
    }

    static float[] mul(float[] a, float[] b) {
        float[] out = new float[16];
        mul(a, 0, b, 0, out, 0);
        return out;
    }

    /** A node's local matrix `T · R · S` with `R = Rx · Ry · Rz` (degrees; Z applies first), into `out`. */
    static void compose(float[] p, float rx, float ry, float rz, float[] s, float[] out) {
        double x = Math.toRadians(rx), y = Math.toRadians(ry), z = Math.toRadians(rz);
        float sx = (float) Math.sin(x), cx = (float) Math.cos(x), sy = (float) Math.sin(y), cy = (float) Math.cos(y),
                sz = (float) Math.sin(z), cz = (float) Math.cos(z);
        // Rows of Rx · Ry · Rz.
        float r00 = cy * cz, r01 = -cy * sz, r02 = sy;
        float r10 = cx * sz + sx * sy * cz, r11 = cx * cz - sx * sy * sz, r12 = -sx * cy;
        float r20 = sx * sz - cx * sy * cz, r21 = sx * cz + cx * sy * sz, r22 = cx * cy;
        out[0] = r00 * s[0]; out[1] = r10 * s[0]; out[2] = r20 * s[0]; out[3] = 0;
        out[4] = r01 * s[1]; out[5] = r11 * s[1]; out[6] = r21 * s[1]; out[7] = 0;
        out[8] = r02 * s[2]; out[9] = r12 * s[2]; out[10] = r22 * s[2]; out[11] = 0;
        out[12] = p[0]; out[13] = p[1]; out[14] = p[2]; out[15] = 1;
    }

    static float[] compose(float[] position, float[] rotationDeg, float[] scale) {
        float[] m = new float[16];
        compose(position, rotationDeg[0], rotationDeg[1], rotationDeg[2], scale, m);
        return m;
    }

    /** `T · R(q) · S` into `out`. */
    static void composeQuat(float px, float py, float pz, float x, float y, float z, float w,
                            float sx, float sy, float sz, float[] out) {
        float xx = x * x, yy = y * y, zz = z * z, xy = x * y, xz = x * z, yz = y * z, wx = w * x, wy = w * y, wz = w * z;
        out[0] = (1 - 2 * (yy + zz)) * sx; out[1] = 2 * (xy + wz) * sx; out[2] = 2 * (xz - wy) * sx; out[3] = 0;
        out[4] = 2 * (xy - wz) * sy; out[5] = (1 - 2 * (xx + zz)) * sy; out[6] = 2 * (yz + wx) * sy; out[7] = 0;
        out[8] = 2 * (xz + wy) * sz; out[9] = 2 * (yz - wx) * sz; out[10] = (1 - 2 * (xx + yy)) * sz; out[11] = 0;
        out[12] = px; out[13] = py; out[14] = pz; out[15] = 1;
    }

    /** `a · b` (`b` applies first) into `out[0..4]`. */
    static void quatMul(float ax, float ay, float az, float aw, float bx, float by, float bz, float bw, float[] out) {
        float x = aw * bx + ax * bw + ay * bz - az * by;
        float y = aw * by - ax * bz + ay * bw + az * bx;
        float z = aw * bz + ax * by - ay * bx + az * bw;
        float w = aw * bw - ax * bx - ay * by - az * bz;
        out[0] = x;
        out[1] = y;
        out[2] = z;
        out[3] = w;
    }

    /** The quaternion of Euler angles in degrees (`Rx · Ry · Rz`) into `out[o..o + 4]`. */
    static void quatFromEuler(float dx, float dy, float dz, float[] out, int o) {
        double hx = Math.toRadians(dx) / 2, hy = Math.toRadians(dy) / 2, hz = Math.toRadians(dz) / 2;
        float sx = (float) Math.sin(hx), cx = (float) Math.cos(hx), sy = (float) Math.sin(hy), cy = (float) Math.cos(hy),
                sz = (float) Math.sin(hz), cz = (float) Math.cos(hz);
        // [sx,0,0,cx] · ([0,sy,0,cy] · [0,0,sz,cz])
        float yzx = sy * sz, yzy = sy * cz, yzz = cy * sz, yzw = cy * cz;
        float x = cx * yzx + sx * yzw;
        float y = cx * yzy - sx * yzz;
        float z = cx * yzz + sx * yzy;
        float w = cx * yzw - sx * yzx;
        out[o] = x;
        out[o + 1] = y;
        out[o + 2] = z;
        out[o + 3] = w;
    }

    static void quatFromEuler(float[] deg, float[] out, int o) {
        quatFromEuler(deg[0], deg[1], deg[2], out, o);
    }

    /** Unit length in place (the identity if zero or not finite). */
    static void quatNormalize(float[] q) {
        float l = (float) Math.sqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]);
        if (l > 1e-12f && !Float.isInfinite(l) && !Float.isNaN(l)) {
            for (int i = 0; i < 4; i++) q[i] /= l;
        } else {
            q[0] = q[1] = q[2] = 0;
            q[3] = 1;
        }
    }

    /** Spherical interpolation from a to b the shorter way around (normalized lerp when nearly equal) into `out`. */
    static void slerp(float ax, float ay, float az, float aw, float bx, float by, float bz, float bw, float t, float[] out) {
        float d = ax * bx + ay * by + az * bz + aw * bw;
        if (d < 0) {
            d = -d;
            bx = -bx;
            by = -by;
            bz = -bz;
            bw = -bw;
        }
        if (d > 0.9995f) {
            out[0] = lerp(ax, bx, t);
            out[1] = lerp(ay, by, t);
            out[2] = lerp(az, bz, t);
            out[3] = lerp(aw, bw, t);
        } else {
            float theta = (float) Math.acos(Math.max(-1, Math.min(1, d)));
            float s = (float) Math.sin(theta);
            float wa = (float) Math.sin((1 - t) * theta) / s, wb = (float) Math.sin(t * theta) / s;
            out[0] = ax * wa + bx * wb;
            out[1] = ay * wa + by * wb;
            out[2] = az * wa + bz * wb;
            out[3] = aw * wa + bw * wb;
        }
        quatNormalize(out);
    }

    /** The box holding `aabb` once transformed by `m` (its 8 corners'), into `out` (may be `aabb`). */
    static void transformAabb(float[] m, float[] aabb, float[] out) {
        float cx = (aabb[0] + aabb[3]) / 2, cy = (aabb[1] + aabb[4]) / 2, cz = (aabb[2] + aabb[5]) / 2;
        float hx = (aabb[3] - aabb[0]) / 2, hy = (aabb[4] - aabb[1]) / 2, hz = (aabb[5] - aabb[2]) / 2;
        float x = m[0] * cx + m[4] * cy + m[8] * cz + m[12];
        float y = m[1] * cx + m[5] * cy + m[9] * cz + m[13];
        float z = m[2] * cx + m[6] * cy + m[10] * cz + m[14];
        float w = m[3] * cx + m[7] * cy + m[11] * cz + m[15];
        if (w != 0 && w != 1) {
            x /= w;
            y /= w;
            z /= w;
        }
        // Each axis' extent: |M| · half (the 3×3 part, absolute values).
        float ex = Math.abs(m[0]) * hx + Math.abs(m[4]) * hy + Math.abs(m[8]) * hz;
        float ey = Math.abs(m[1]) * hx + Math.abs(m[5]) * hy + Math.abs(m[9]) * hz;
        float ez = Math.abs(m[2]) * hx + Math.abs(m[6]) * hy + Math.abs(m[10]) * hz;
        out[0] = x - ex;
        out[1] = y - ey;
        out[2] = z - ez;
        out[3] = x + ex;
        out[4] = y + ey;
        out[5] = z + ez;
    }

    /** The box holding both boxes, into `out` (may be either). */
    static void aabbUnion(float[] a, float[] b, float[] out) {
        for (int i = 0; i < 3; i++) {
            out[i] = Math.min(a[i], b[i]);
            out[i + 3] = Math.max(a[i + 3], b[i + 3]);
        }
    }

    /** The six planes (a, b, c, d) of the clip volume of `vp` (left, right, bottom, top, near, far), 4 floats each, into `out`. */
    static void frustumPlanes(float[] vp, float[] out) {
        for (int p = 0; p < 6; p++) {
            int r = p / 2;
            float sign = p % 2 == 0 ? 1 : -1;
            for (int c = 0; c < 4; c++) out[p * 4 + c] = vp[c * 4 + 3] + sign * vp[c * 4 + r];
        }
    }

    /** Whether `aabb` is at least partly inside the frustum (conservative). */
    static boolean aabbInFrustum(float[] planes, float[] aabb) {
        for (int p = 0; p < 6; p++) {
            float a = planes[p * 4], b = planes[p * 4 + 1], c = planes[p * 4 + 2], d = planes[p * 4 + 3];
            // The corner farthest along the plane's normal.
            float x = a >= 0 ? aabb[3] : aabb[0], y = b >= 0 ? aabb[4] : aabb[1], z = c >= 0 ? aabb[5] : aabb[2];
            if (!(a * x + b * y + c * z + d >= 0)) return false;
        }
        return true;
    }

    /** Where the ray `origin + t · dir` enters `aabb` (the slab test): the smallest t ≥ 0, NaN if it misses. */
    static float rayAabb(float[] origin, float[] dir, float[] aabb) {
        float t0 = 0, t1 = Float.MAX_VALUE;
        for (int a = 0; a < 3; a++) {
            float lo = aabb[a], hi = aabb[a + 3];
            if (Math.abs(dir[a]) < 1e-12f) {
                if (origin[a] < lo || origin[a] > hi) return Float.NaN;
                continue;
            }
            float inv = 1 / dir[a];
            float n = (lo - origin[a]) * inv, f = (hi - origin[a]) * inv;
            if (n > f) {
                float s = n;
                n = f;
                f = s;
            }
            t0 = Math.max(t0, n);
            t1 = Math.min(t1, f);
            if (t0 > t1) return Float.NaN;
        }
        return t0;
    }

    /** A perspective projection (vertical field of view in degrees) into `m`. */
    static void perspective(float fovDeg, float aspect, float near, float far, float[] m) {
        float f = (float) (1 / Math.tan(Math.toRadians(fovDeg) / 2));
        float nf = 1 / (near - far);
        Arrays.fill(m, 0);
        m[0] = f / aspect;
        m[5] = f;
        m[10] = (far + near) * nf;
        m[11] = -1;
        m[14] = 2 * far * near * nf;
    }

    /** An orthographic projection showing `height` units vertically, into `m`. */
    static void orthographic(float height, float aspect, float near, float far, float[] m) {
        float t = height / 2, r = height / 2 * aspect;
        System.arraycopy(IDENTITY, 0, m, 0, 16);
        m[0] = 1 / r;
        m[5] = 1 / t;
        m[10] = -2 / (far - near);
        m[14] = -(far + near) / (far - near);
    }

    /** The view matrix of a camera at `eye` looking at `target`, `up` up, into `m`. */
    static void lookAt(float[] eye, float[] target, float[] up, float[] m) {
        float fx = target[0] - eye[0], fy = target[1] - eye[1], fz = target[2] - eye[2];
        float fl = (float) Math.sqrt(fx * fx + fy * fy + fz * fz);
        if (fl != 0) {
            fx /= fl;
            fy /= fl;
            fz /= fl;
        }
        if ((float) Math.sqrt(fx * fx + fy * fy + fz * fz) == 0) {
            fx = 0;
            fy = 0;
            fz = -1;
        }
        float sx = fy * up[2] - fz * up[1], sy = fz * up[0] - fx * up[2], sz = fx * up[1] - fy * up[0];
        if ((float) Math.sqrt(sx * sx + sy * sy + sz * sz) < 1e-6f) {
            // Looking straight up or down: any horizontal side vector (f × −Z).
            sx = -fy;
            sy = fx;
            sz = 0;
        }
        float sl = (float) Math.sqrt(sx * sx + sy * sy + sz * sz);
        if (sl != 0) {
            sx /= sl;
            sy /= sl;
            sz /= sl;
        }
        float ux = sy * fz - sz * fy, uy = sz * fx - sx * fz, uz = sx * fy - sy * fx;
        m[0] = sx; m[1] = ux; m[2] = -fx; m[3] = 0;
        m[4] = sy; m[5] = uy; m[6] = -fy; m[7] = 0;
        m[8] = sz; m[9] = uz; m[10] = -fz; m[11] = 0;
        m[12] = -(sx * eye[0] + sy * eye[1] + sz * eye[2]);
        m[13] = -(ux * eye[0] + uy * eye[1] + uz * eye[2]);
        m[14] = fx * eye[0] + fy * eye[1] + fz * eye[2];
        m[15] = 1;
    }

    /** The inverse into `inv` (the identity if `m` is singular); `inv` must not be `m`. */
    static void invert(float[] m, float[] inv) {
        inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
        inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
        inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
        inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
        inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
        inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
        inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
        inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
        inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
        inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
        inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
        inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
        inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
        inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
        inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
        inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];
        float det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
        // Only a truly singular matrix: a centimeter rig has a tiny determinant and a fine inverse.
        boolean ok = det != 0 && !Float.isInfinite(det) && !Float.isNaN(det);
        if (ok) {
            for (int i = 0; i < 16; i++) {
                inv[i] /= det;
                if (Float.isInfinite(inv[i]) || Float.isNaN(inv[i])) ok = false;
            }
        }
        if (!ok) System.arraycopy(IDENTITY, 0, inv, 0, 16);
    }

    static float[] invert(float[] m) {
        float[] inv = new float[16];
        invert(m, inv);
        return inv;
    }

    /** The normal matrix (inverse transpose of the upper 3×3, column-major 3×3) into `out`; `scratch`: 16 floats. */
    static void normalMatrix(float[] m, float[] out, float[] scratch) {
        invert(m, scratch);
        out[0] = scratch[0]; out[1] = scratch[4]; out[2] = scratch[8];
        out[3] = scratch[1]; out[4] = scratch[5]; out[5] = scratch[9];
        out[6] = scratch[2]; out[7] = scratch[6]; out[8] = scratch[10];
    }

    /** `m · (p, 1)`, divided by w, into `out` (may be `p`). */
    static float[] transformPoint(float[] m, float[] p, float[] out) {
        float x = m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12];
        float y = m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13];
        float z = m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14];
        float w = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15];
        if (w != 0 && w != 1) {
            x /= w;
            y /= w;
            z /= w;
        }
        out[0] = x;
        out[1] = y;
        out[2] = z;
        return out;
    }

    static float[] transformPoint(float[] m, float[] p) {
        return transformPoint(m, p, new float[3]);
    }

    /** `m · (v, 0)`. */
    static float[] transformDir(float[] m, float[] v) {
        return new float[]{
                m[0] * v[0] + m[4] * v[1] + m[8] * v[2],
                m[1] * v[0] + m[5] * v[1] + m[9] * v[2],
                m[2] * v[0] + m[6] * v[1] + m[10] * v[2]};
    }

    static float[] add(float[] a, float[] b) {
        return new float[]{a[0] + b[0], a[1] + b[1], a[2] + b[2]};
    }

    static float[] sub(float[] a, float[] b) {
        return new float[]{a[0] - b[0], a[1] - b[1], a[2] - b[2]};
    }

    static float[] scale(float[] v, float s) {
        return new float[]{v[0] * s, v[1] * s, v[2] * s};
    }

    static float dot(float[] a, float[] b) {
        return a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    }

    static float[] cross(float[] a, float[] b) {
        return new float[]{a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]};
    }

    static float length(float[] v) {
        return (float) Math.sqrt(dot(v, v));
    }

    /** Unit vector (zero stays zero). */
    static float[] normalize(float[] v) {
        float l = length(v);
        return l == 0 ? v : scale(v, 1 / l);
    }

    static float lerp(float a, float b, float t) {
        return a + (b - a) * t;
    }

    static void lerp3(float[] a, float[] b, float t, float[] out) {
        for (int i = 0; i < 3; i++) out[i] = lerp(a[i], b[i], t);
    }

    /** sRGB component to linear light. */
    static float srgbToLinear(float c) {
        return c <= 0.04045f ? c / 12.92f : (float) Math.pow((c + 0.055f) / 1.055f, 2.4);
    }

    /** Rust's `f32::rem_euclid`. */
    private static float remEuclid(float x, float m) {
        float r = x % m;
        return r < 0 ? r + Math.abs(m) : r;
    }

    /** Element-wise `==` (Rust's PartialEq: 0.0 equals -0.0). */
    private static boolean eq(float[] a, float[] b) {
        if (a == b) return true;
        if (a.length != b.length) return false;
        for (int i = 0; i < a.length; i++) if (a[i] != b[i]) return false;
        return true;
    }

    /** Two optional floats (NaN: none). */
    private static boolean sameOpt(float a, float b) {
        return Float.isNaN(a) ? Float.isNaN(b) : a == b;
    }

    // --- Shaders (shaders.rs) --------------------------------------------------------------

    /** The GLSL of prism-core `scene3d::shaders`, verbatim (the desktop test compares it with shaders.rs byte for byte). */
    static final class Shaders {
        /** First line for OpenGL ES 3. */
        static final String ES_HEADER = "#version 300 es\n";

        /**
         * The full source of one stage of a variant's program (`program_source`):
         * `header`, the variant's `#define`s in their canonical order, the body.
         */
        static String programSource(String header, boolean fragment, boolean instanced, boolean skinned, boolean depthOnly) {
            return header + (instanced ? "#define INSTANCED\n" : "") + (skinned ? "#define SKINNED\n" : "")
                    + (depthOnly ? "#define DEPTH_ONLY\n" : "") + (fragment ? FRAGMENT : VERTEX);
        }

        static final String VERTEX = """
                precision highp float;
                precision highp int;
                layout(location = 0) in vec3 a_position;
                layout(location = 1) in vec3 a_normal;
                layout(location = 2) in vec2 a_uv;
                layout(location = 3) in vec4 a_color;
                #ifdef SKINNED
                layout(location = 4) in uvec4 a_joints;
                layout(location = 5) in vec4 a_weights;
                uniform highp sampler2D u_bones;
                #endif
                #ifdef INSTANCED
                layout(location = 6) in mat4 a_instance;
                layout(location = 10) in vec4 a_instance_color;
                #endif
                uniform mat4 u_model;
                uniform mat3 u_normal_matrix;
                uniform mat4 u_view_proj;
                out vec3 v_world;
                out vec3 v_normal;
                out vec2 v_uv;
                out vec4 v_color;
                #ifdef SKINNED
                mat4 bone(uint j) {
                  int y = int(j);
                  return mat4(texelFetch(u_bones, ivec2(0, y), 0), texelFetch(u_bones, ivec2(1, y), 0),
                              texelFetch(u_bones, ivec2(2, y), 0), texelFetch(u_bones, ivec2(3, y), 0));
                }
                #endif
                void main() {
                  vec4 position = vec4(a_position, 1.0);
                  vec3 normal = a_normal;
                  v_color = a_color;
                #ifdef SKINNED
                  mat4 skin = a_weights.x * bone(a_joints.x) + a_weights.y * bone(a_joints.y)
                            + a_weights.z * bone(a_joints.z) + a_weights.w * bone(a_joints.w);
                  position = skin * position;
                  normal = mat3(skin) * normal;
                #endif
                #ifdef INSTANCED
                  position = a_instance * position;
                  normal = transpose(inverse(mat3(a_instance))) * normal;
                  v_color *= a_instance_color;
                #endif
                  vec4 world = u_model * position;
                  v_world = world.xyz;
                  v_normal = u_normal_matrix * normal;
                  v_uv = a_uv;
                  gl_Position = u_view_proj * world;
                }
                """;

        static final String FRAGMENT = """
                precision highp float;
                precision highp int;
                #ifdef DEPTH_ONLY
                void main() {}
                #else
                precision highp sampler2DShadow;
                #define MAX_LIGHTS 8
                in vec3 v_world;
                in vec3 v_normal;
                in vec2 v_uv;
                in vec4 v_color;
                uniform vec3 u_camera;
                uniform vec3 u_ambient;
                uniform int u_light_count;
                uniform int u_light_kind[MAX_LIGHTS];
                uniform vec3 u_light_color[MAX_LIGHTS];
                uniform vec3 u_light_position[MAX_LIGHTS];
                uniform vec3 u_light_direction[MAX_LIGHTS];
                uniform vec4 u_light_params[MAX_LIGHTS];
                uniform float u_exposure;
                uniform vec4 u_color;
                uniform vec3 u_emissive;
                uniform float u_metalness;
                uniform float u_roughness;
                uniform int u_unlit;
                uniform int u_alpha_mode;
                uniform float u_alpha_cutoff;
                uniform int u_has_texture;
                uniform sampler2D u_texture;
                uniform int u_has_mr_map;
                uniform sampler2D u_mr_map;
                uniform int u_has_normal_map;
                uniform sampler2D u_normal_map;
                uniform float u_normal_scale;
                uniform int u_has_emissive_map;
                uniform sampler2D u_emissive_map;
                uniform int u_has_ao_map;
                uniform sampler2D u_ao_map;
                uniform int u_has_env;
                uniform sampler2D u_env;
                uniform float u_env_levels;
                uniform vec3 u_env_sh[9];
                uniform float u_env_intensity;
                uniform int u_has_shadow;
                uniform int u_receive_shadows;
                uniform sampler2DShadow u_shadow_map;
                uniform mat4 u_shadow_view_proj;
                uniform int u_shadow_light;
                uniform float u_shadow_bias;
                uniform float u_shadow_normal_bias;
                uniform float u_shadow_texel;
                out vec4 o_color;
                const float PI = 3.14159265;

                // Equirectangular coordinates of a direction (v = 0: straight up).
                vec2 equirect(vec3 d) {
                  return vec2(0.5 + atan(d.x, -d.z) / (2.0 * PI), acos(clamp(d.y, -1.0, 1.0)) / PI);
                }

                // The environment's irradiance around n, from its spherical harmonics.
                vec3 sh_irradiance(vec3 n) {
                  return u_env_sh[0] * 0.282095
                    + u_env_sh[1] * 0.488603 * n.y
                    + u_env_sh[2] * 0.488603 * n.z
                    + u_env_sh[3] * 0.488603 * n.x
                    + u_env_sh[4] * 1.092548 * n.x * n.y
                    + u_env_sh[5] * 1.092548 * n.y * n.z
                    + u_env_sh[6] * 0.315392 * (3.0 * n.z * n.z - 1.0)
                    + u_env_sh[7] * 1.092548 * n.x * n.z
                    + u_env_sh[8] * 0.546274 * (n.x * n.x - n.y * n.y);
                }

                // Karis' analytic approximation of the split-sum environment BRDF.
                vec3 env_brdf(vec3 f0, float rough, float nv) {
                  const vec4 c0 = vec4(-1.0, -0.0275, -0.572, 0.022);
                  const vec4 c1 = vec4(1.0, 0.0425, 1.04, -0.04);
                  vec4 r = rough * c0 + c1;
                  float a004 = min(r.x * r.x, exp2(-9.28 * nv)) * r.x + r.y;
                  vec2 ab = vec2(-1.04, 1.04) * a004 + r.zw;
                  return f0 * ab.x + ab.y;
                }

                // The normal bent by the normal map, in a cotangent frame built from screen
                // derivatives (no tangents needed). Texture v grows downward while the map's
                // green points up the image: the bitangent is -dp/dv.
                vec3 perturb(vec3 n, vec3 p, vec2 uv) {
                  vec3 m = texture(u_normal_map, uv).xyz * 2.0 - 1.0;
                  m.xy *= u_normal_scale;
                  vec3 dp1 = dFdx(p);
                  vec3 dp2 = dFdy(p);
                  vec2 duv1 = dFdx(uv);
                  vec2 duv2 = dFdy(uv);
                  vec3 dp2perp = cross(dp2, n);
                  vec3 dp1perp = cross(n, dp1);
                  vec3 t = dp2perp * duv1.x + dp1perp * duv2.x;
                  vec3 b = -(dp2perp * duv1.y + dp1perp * duv2.y);
                  float scale = inversesqrt(max(max(dot(t, t), dot(b, b)), 1e-20));
                  return normalize(mat3(t * scale, b * scale, n) * m);
                }

                // 1: lit, 0: in the shadow light's shadow (3x3 PCF).
                float shadow(vec3 n) {
                  vec4 p = u_shadow_view_proj * vec4(v_world + n * u_shadow_normal_bias, 1.0);
                  vec3 c = p.xyz / p.w * 0.5 + 0.5;
                  if (c.x < 0.0 || c.x > 1.0 || c.y < 0.0 || c.y > 1.0 || c.z > 1.0) return 1.0;
                  float sum = 0.0;
                  for (int x = -1; x <= 1; x++) {
                    for (int y = -1; y <= 1; y++) {
                      vec2 o = vec2(float(x), float(y)) * u_shadow_texel;
                      sum += texture(u_shadow_map, vec3(c.xy + o, c.z - u_shadow_bias));
                    }
                  }
                  return sum / 9.0;
                }

                // ACES filmic curve (Narkowicz' fit).
                vec3 aces(vec3 x) {
                  return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
                }

                void main() {
                  vec4 base = u_color * v_color;
                  if (u_has_texture == 1) {
                    vec4 t = texture(u_texture, v_uv);
                    base *= vec4(pow(t.rgb, vec3(2.2)), t.a);
                  }
                  if (u_alpha_mode == 1) {
                    if (base.a < u_alpha_cutoff) discard;
                    base.a = 1.0;
                  } else if (u_alpha_mode == 2) {
                    base.a = 1.0;
                  }
                  vec3 emissive = u_emissive;
                  if (u_has_emissive_map == 1) {
                    emissive *= pow(texture(u_emissive_map, v_uv).rgb, vec3(2.2));
                  }
                  vec3 color;
                  if (u_unlit == 1) {
                    color = base.rgb;
                  } else {
                    vec3 n = normalize(v_normal);
                    if (!gl_FrontFacing) n = -n;
                    vec3 geometric = n;
                    if (u_has_normal_map == 1) n = perturb(n, v_world, v_uv);
                    float metal = u_metalness;
                    float rough = u_roughness;
                    if (u_has_mr_map == 1) {
                      vec4 mr = texture(u_mr_map, v_uv);
                      rough *= mr.g;
                      metal *= mr.b;
                    }
                    float ao = u_has_ao_map == 1 ? texture(u_ao_map, v_uv).r : 1.0;
                    vec3 v = normalize(u_camera - v_world);
                    float nv = max(dot(n, v), 1e-4);
                    rough = clamp(rough, 0.04, 1.0);
                    float a = rough * rough;
                    float a2 = a * a;
                    float k = (rough + 1.0) * (rough + 1.0) / 8.0;
                    vec3 f0 = mix(vec3(0.04), base.rgb, metal);
                    vec3 diffuse = base.rgb * (1.0 - metal);
                    // Ambient light: diffuse, plus a little specular so metals are not black.
                    color = u_ambient * (diffuse + f0 * 0.5) * ao;
                    if (u_has_env == 1) {
                      vec3 irradiance = max(sh_irradiance(n), vec3(0.0)) / PI;
                      vec3 r = reflect(-v, n);
                      vec3 prefiltered = textureLod(u_env, equirect(r), rough * (u_env_levels - 1.0)).rgb;
                      color += (irradiance * diffuse + prefiltered * env_brdf(f0, rough, nv)) * u_env_intensity * ao;
                    }
                    float lit = 1.0;
                    if (u_has_shadow == 1 && u_receive_shadows == 1) lit = shadow(geometric);
                    for (int i = 0; i < MAX_LIGHTS; i++) {
                      if (i >= u_light_count) break;
                      vec3 l;
                      float att = 1.0;
                      if (u_light_kind[i] == 1) {
                        l = -normalize(u_light_direction[i]);
                      } else {
                        vec3 d = u_light_position[i] - v_world;
                        float dist = length(d);
                        l = d / max(dist, 1e-4);
                        float range = u_light_params[i].x;
                        if (range > 0.0) {
                          float f = clamp(1.0 - dist / range, 0.0, 1.0);
                          att = f * f;
                        }
                        if (u_light_kind[i] == 3) {
                          float c = dot(-l, normalize(u_light_direction[i]));
                          att *= smoothstep(u_light_params[i].y, u_light_params[i].z, c);
                        }
                      }
                      if (i == u_shadow_light) att *= lit;
                      float nl = max(dot(n, l), 0.0);
                      if (nl <= 0.0 || att <= 0.0) continue;
                      vec3 h = normalize(v + l);
                      float nh = max(dot(n, h), 0.0);
                      float vh = max(dot(v, h), 0.0);
                      float dd = nh * nh * (a2 - 1.0) + 1.0;
                      float D = a2 / (PI * dd * dd);
                      float G = (nv / (nv * (1.0 - k) + k)) * (nl / (nl * (1.0 - k) + k));
                      vec3 F = f0 + (1.0 - f0) * pow(1.0 - vh, 5.0);
                      vec3 spec = D * G * F / (4.0 * nv * nl + 1e-4);
                      vec3 kd = (1.0 - F) * (1.0 - metal);
                      // Punctual lights in "intensity 1 = full white" units: × PI.
                      color += (kd * base.rgb / PI + spec) * u_light_color[i] * att * nl * PI;
                    }
                  }
                  color += emissive;
                  color = pow(aces(color * u_exposure), vec3(1.0 / 2.2));
                  // Premultiplied alpha (canvases, TextureViews and GL areas composite so).
                  o_color = vec4(color * base.a, base.a);
                }
                #endif
                """;

        static final String SKY_VERTEX = """
                precision highp float;
                precision highp int;
                out vec2 v_ndc;
                void main() {
                  vec2 p = vec2(float((gl_VertexID & 1) * 4 - 1), float((gl_VertexID & 2) * 2 - 1));
                  v_ndc = p;
                  gl_Position = vec4(p, 0.0, 1.0);
                }
                """;

        static final String SKY_FRAGMENT = """
                precision highp float;
                in vec2 v_ndc;
                uniform mat4 u_inverse_view_proj;
                uniform vec3 u_camera;
                uniform sampler2D u_env;
                uniform float u_env_intensity;
                uniform float u_exposure;
                out vec4 o_color;
                const float PI = 3.14159265;
                vec3 aces(vec3 x) {
                  return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
                }
                void main() {
                  vec4 far = u_inverse_view_proj * vec4(v_ndc, 1.0, 1.0);
                  vec3 d = normalize(far.xyz / far.w - u_camera);
                  vec2 uv = vec2(0.5 + atan(d.x, -d.z) / (2.0 * PI), acos(clamp(d.y, -1.0, 1.0)) / PI);
                  vec3 c = textureLod(u_env, uv, 0.0).rgb * u_env_intensity;
                  o_color = vec4(pow(aces(c * u_exposure), vec3(1.0 / 2.2)), 1.0);
                }
                """;
    }
}
