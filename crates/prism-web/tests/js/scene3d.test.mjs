// 3D scenes in client.js: the port of prism-core's scene3d (geometry, blobs,
// wire, clips, ScenePlayer) against the Rust test vectors (crates/prism-core/
// src/scene3d/tests.rs), the GLSL taken from shaders.rs, then the WebGL 2
// element against a recording context: first paint, updates, gestures,
// taps, blobs, textures, instancing, shadows and teardown.
//
// Run: node --test crates/prism-web/tests/js/
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { boot, Element, shaderSource, withShaders } from "./dom.mjs";

// --- The port, run on its own ------------------------------------------------------

const raw = fs.readFileSync(new URL("../../src/client.js", import.meta.url), "utf8");
const source = withShaders(raw);
const core = source.slice(source.indexOf("  function scene3dCore()"), source.indexOf("  // </scene3d-core>"));
const S = new Function(`${core}\nreturn scene3dCore();`)();
const { cuboid, sphere, plane, cylinder, torus } = S.geometry;
const M = S.math;

const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const close = (a, b, eps, what) => assert.ok(Math.abs(a - b) < eps, `${what ?? ""} ${a} ≉ ${b}`);
const near = (a, b, what) => close(a, b, 1e-4, what);
const nearMat = (a, b, what) => Array.from(a).forEach((v, i) => close(v, b[i], 1e-4, `${what ?? ""}[${i}]`));

function facesPointOutward(m, center) {
  assert.equal(m.indices.length % 3, 0);
  for (let k = 0; k < m.indices.length; k += 3) {
    const t = m.indices.slice(k, k + 3);
    const [a, b, c] = t.map((i) => m.position(i));
    const n = cross(sub(b, a), sub(c, a));
    if (M.length(n) < 1e-7) continue; // degenerate (pole) triangles
    const mid = [0, 1, 2].map((i) => (a[i] + b[i] + c[i]) / 3);
    assert.ok(dot(n, sub(mid, center)) > 0, `triangle ${t} faces inward`);
    const vn = m.normals.slice(t[0] * 3, t[0] * 3 + 3);
    assert.ok(dot(n, vn) > 0, `normal of ${t} disagrees with its winding`);
  }
  assert.equal(m.normals.length, m.positions.length);
  assert.equal(m.uvs.length / 2, m.positions.length / 3);
}

// --- Blob encoders (blobs.rs' layouts, written the Rust way) ------------------------

class Writer {
  constructor(kind) { this.bytes = []; this.raw([80, 51, 68, kind.charCodeAt(0)]); this.u32(1); }
  raw(b) { this.bytes.push(...b); }
  u32(v) { const d = new DataView(new ArrayBuffer(4)); d.setUint32(0, v, true); this.raw(new Uint8Array(d.buffer)); }
  f32(v) { const d = new DataView(new ArrayBuffer(4)); d.setFloat32(0, v, true); this.raw(new Uint8Array(d.buffer)); }
  f32s(v) { for (const x of v) this.f32(x); }
  u16s(v) { for (const x of v) this.raw([x & 255, x >> 8]); this.pad(); }
  str(s) { const b = new TextEncoder().encode(s); this.u32(b.length); this.raw(b); this.pad(); }
  pad() { while (this.bytes.length % 4) this.bytes.push(0); }
  buffer() { return new Uint8Array(this.bytes).buffer; }
}
const aabbOf = (p) => {
  if (!p.length) return [0, 0, 0, 0, 0, 0];
  const b = [Infinity, Infinity, Infinity, -Infinity, -Infinity, -Infinity];
  for (let i = 0; i < p.length; i += 3) for (let k = 0; k < 3; k++) { b[k] = Math.min(b[k], p[i + k]); b[k + 3] = Math.max(b[k + 3], p[i + k]); }
  return b;
};
/** `encode_mesh` (flat arrays; optional ones empty or absent). */
function encodeMesh({ positions, normals, uvs = [], joints = [], weights = [], colors = [], indices }) {
  const n = positions.length / 3, w = new Writer("M");
  const uv = uvs.length === n * 2 && n > 0, skin = joints.length === n * 4 && weights.length === n * 4 && n > 0, color = colors.length === n * 4 && n > 0;
  w.u32(n); w.u32(indices.length); w.u32((uv ? 1 : 0) | (skin ? 4 : 0) | (color ? 8 : 0));
  w.f32s(aabbOf(positions)); w.f32s(positions); w.f32s(normals);
  if (uv) w.f32s(uvs);
  if (skin) { w.u16s(joints); w.f32s(weights); }
  if (color) w.f32s(colors);
  indices.forEach((i) => w.u32(i));
  return w.buffer();
}
function encodeInstances(transforms, colors, bounds) {
  const w = new Writer("I");
  w.u32(transforms.length); w.u32(colors ? 1 : 0); w.f32s(bounds);
  for (const t of transforms) w.f32s([...t.position, ...t.rotation, ...t.scale]);
  if (colors) w.f32s(colors.flat());
  return w.buffer();
}
/** `encode_clip`: channels `{ target, property: 0 T | 1 R | 2 S, interpolation: 0 linear | 1 step | 2 cubic, times, values }`. */
function encodeClip(channels) {
  const w = new Writer("A");
  w.u32(channels.length);
  w.f32(Math.max(0, ...channels.map((c) => c.times.at(-1) ?? 0)));
  for (const c of channels) { w.str(c.target); w.raw([c.property, c.interpolation, 0, 0]); w.u32(c.times.length); w.f32s(c.times); w.f32s(c.values); }
  return w.buffer();
}
function encodeSkin(inverseBind, joints) {
  const w = new Writer("K");
  w.u32(joints.length);
  inverseBind.forEach((m) => w.f32s(m));
  joints.forEach((j) => w.str(j));
  return w.buffer();
}
function encodeEnv(width, height, sh, levels) {
  const w = new Writer("E");
  w.u32(width); w.u32(height); w.u32(levels.length); w.f32s(sh);
  for (const l of levels) for (const v of l) w.raw([v & 255, v >> 8]);
  return w.buffer();
}
/** A blob store: ids → bytes; `resolve(id, kind)` decodes like the page's store (once per kind). */
function store() {
  const bytes = new Map(), decoded = new Map();
  let n = 0;
  return {
    bytes,
    put(buf) { const id = (++n).toString(16).padStart(32, "0"); bytes.set(id, buf); return id; },
    resolve: (id, kind) => {
      if (!bytes.has(id)) return null;
      const k = `${id}:${kind}`;
      if (!decoded.has(k)) decoded.set(k, S.decode[kind](bytes.get(id)));
      return decoded.get(k);
    },
  };
}
const TRS = (t = [0, 0, 0], r = [0, 0, 0, 1], s = [1, 1, 1]) => ({ position: t, rotation: r, scale: s });

// --- Scenes in the wire format (FlatNodes; serde's defaults left out, like the server sends) ---

const camera = (pos, target = [0, 0, 0]) => ({ pos, target, fov: 45, near: 0.1, far: 1000 });
const BOX = { type: "box", size: [1, 1, 1] };
const cube = (key, extra = {}) => ({ key, ...(key.startsWith("#") && !key.includes("/") ? { name: key.slice(1) } : {}), mesh: BOX, ...extra });
const full = (settings, nodes) => ({ v: 2, full: true, settings, nodes });
function player(update, now = 0, blobs) {
  const p = new S.ScenePlayer(now, blobs && blobs.resolve);
  p.apply(S.decodeUpdate(update), now);
  return p;
}
const show = (p, update, now) => p.apply(S.decodeUpdate(update), now);
const names = (p) => p.frame(1, 1, false).draws.map((d) => d.name ?? "-");
const FAR = camera([0, 0, 40]);
const linear = { ms: 1000, bezier: [0, 0, 1, 1] };

test("3D: primitives wind counter-clockwise from outside", () => {
  facesPointOutward(cuboid([1, 2, 3]), [0, 0, 0]);
  facesPointOutward(sphere(1), [0, 0, 0]);
  facesPointOutward(cylinder(1, 1, 2), [0, 0, 0]);
  facesPointOutward(cylinder(1, 0, 2), [0, 0, 0]);
  const p = plane([2, 2]);
  for (let k = 0; k < p.indices.length; k += 3) {
    const [a, b, c] = p.indices.slice(k, k + 3).map((i) => p.position(i));
    assert.ok(cross(sub(b, a), sub(c, a))[1] > 0, "the plane faces up");
  }
  const m = torus(2, 0.5);
  for (let k = 0; k < m.indices.length; k += 3) {
    const [a, b, c] = m.indices.slice(k, k + 3).map((i) => m.position(i));
    const n = cross(sub(b, a), sub(c, a));
    if (M.length(n) < 1e-7) continue;
    const mid = [0, 1, 2].map((i) => (a[i] + b[i] + c[i]) / 3);
    const r = Math.hypot(mid[0], mid[2]);
    assert.ok(dot(n, sub(mid, [(mid[0] / r) * 2, 0, (mid[2] / r) * 2])) > 0, "the torus faces away from its tube");
  }
  const box = cuboid([1, 2, 3]);
  const max = (axis) => Math.max(...box.positions.filter((_, i) => i % 3 === axis));
  assert.deepEqual([max(0), max(1), max(2)], [0.5, 1, 1.5]);
  assert.equal(box.vertexCount(), 24);
  assert.equal(box.indices.length, 36);
});

test("3D: matrices, quaternions, boxes and rays", () => {
  const m = M.compose([1, 2, 3], [30, 45, 60], [2, 2, 2]);
  M.mul(M.invert(m), m).forEach((v, i) => close(v, i % 5 === 0 ? 1 : 0, 1e-4, `[${i}]`));
  // Rz applies first: X turns to Y, then around Y by 90°: Y stays.
  close(M.transformPoint(M.rotation([0, 90, 90]), [1, 0, 0])[1], 1, 1e-5);
  close(M.transformPoint(M.rotationY(90), [0, 0, 1])[0], 1, 1e-5);
  const view = M.lookAt([0, 0, 5], [0, 0, 0], [0, 1, 0]);
  close(M.transformPoint(view, [0, 0, 0])[2], -5, 1e-5, "the target is 5 units in front");
  const c = M.transformPoint(M.mul(M.perspective(45, 1, 0.1, 100), view), [0, 0, 0]);
  assert.ok(Math.abs(c[0]) < 1e-5 && Math.abs(c[1]) < 1e-5 && c[2] > -1 && c[2] < 1);
  // A centimeter rig (1/100 twice): a determinant of 1e-12 still inverts.
  const tiny = M.compose([0, 0, 0], [0, 0, 0], [1e-4, 1e-4, 1e-4]);
  close(M.invert(tiny)[0], 1e4, 1e-2);
  for (const e of [[30, 45, 60], [0, 90, 0], [-120, 10, 200]]) {
    const q = M.quatFromEuler(e);
    nearMat(M.quatToMat4(q), M.rotation(e), JSON.stringify(e));
    nearMat(M.composeQuat([1, 2, 3], q, [2, 3, 4]), M.compose([1, 2, 3], e, [2, 3, 4]));
  }
  // Slerp halfway between identity and a half turn around Y: a quarter turn.
  nearMat(M.quatToMat4(M.slerp([0, 0, 0, 1], [0, 1, 0, 0], 0.5)), M.rotationY(90));
  const b = M.transformAabb(M.compose([10, 0, 0], [0, 0, 45], [1, 1, 1]), [-1, -1, -1, 1, 1, 1]);
  near(b[0], 10 - Math.SQRT2); near(b[3], 10 + Math.SQRT2); near(b[2], -1);
  assert.equal(M.rayAabb([0, 0, 5], [0, 0, -1], [-1, -1, -1, 1, 1, 1]), 4);
  assert.equal(M.rayAabb([3, 0, 5], [0, 0, -1], [-1, -1, -1, 1, 1, 1]), null);
});

test("3D: blobs decode every layout, and nothing malformed", () => {
  const mesh = {
    positions: [0, 0, 0, 1, 0, 0, 0, 2, 0], normals: [0, 0, 1, 0, 0, 1, 0, 0, 1], uvs: [0, 0, 1, 0, 0, 1],
    joints: [0, 1, 2, 3, 1, 0, 0, 0, 65535, 0, 0, 0], weights: [0.25, 0.25, 0.25, 0.25, 1, 0, 0, 0, 0.5, 0.5, 0, 0],
    colors: [1, 0, 0, 1, 0, 1, 0, 1, 0, 0, 1, 0.5], indices: [0, 1, 2],
  };
  const bytes = encodeMesh(mesh);
  assert.equal(bytes.byteLength % 4, 0);
  const back = S.decode.M(bytes);
  assert.deepEqual(back.bounds, [0, 0, 0, 1, 2, 0]);
  for (const k of ["positions", "normals", "uvs", "joints", "weights", "colors", "indices"]) assert.deepEqual(Array.from(back[k]), mesh[k], k);
  const plain = S.decode.M(encodeMesh({ ...mesh, uvs: [], joints: [], weights: [], colors: [] }));
  assert.ok(plain.uvs === null && plain.joints === null && plain.colors === null);
  assert.deepEqual(Array.from(plain.positions), mesh.positions);
  // A blob mesh's triangles: uvs (0, 0) when absent.
  const blobs = store(), id = blobs.put(encodeMesh({ ...mesh, uvs: [] }));
  const g = S.generate({ type: "blob", id }, blobs.resolve);
  assert.equal(g.vertexCount(), 3);
  assert.deepEqual(Array.from(g.uvs), [0, 0, 0, 0, 0, 0]);
  assert.equal(S.generate({ type: "blob", id: "f".repeat(32) }, blobs.resolve).vertexCount(), 0, "not arrived: empty");

  const set = S.decode.I(encodeInstances([TRS([1, 2, 3], [0, 0, 0, 1], [1, 2, 1]), TRS([1, 2, 3], [0, 0, 0, 1], [1, 2, 1])], [[1, 0.5, 0.25, 1], [1, 0.5, 0.25, 1]], [-1, -2, -3, 1, 2, 3]));
  assert.equal(set.count, 2);
  assert.deepEqual(set.bounds, [-1, -2, -3, 1, 2, 3]);
  assert.deepEqual(Array.from(set.colors), [1, 0.5, 0.25, 1, 1, 0.5, 0.25, 1]);
  assert.deepEqual(Array.from(set.matrix(1)), [1, 0, 0, 0, 0, 2, 0, 0, 0, 0, 1, 0, 1, 2, 3, 1]);

  const clip = S.decode.A(encodeClip([
    { target: "0/2", property: 0, interpolation: 0, times: [0, 2], values: [0, 0, 0, 1, 2, 3] },
    { target: "#héllo", property: 1, interpolation: 2, times: [0], values: [0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0] },
  ]));
  assert.equal(clip.duration, 2);
  assert.deepEqual(clip.channels.map((c) => [c.target, c.property, c.interpolation, c.values.length]), [["0/2", 0, 0, 6], ["#héllo", 1, 2, 12]]);

  const skin = S.decode.K(encodeSkin([M.translation([0, -2, 0]), M.IDENTITY], ["0", "0/1/12"]));
  assert.deepEqual(skin.joints, ["0", "0/1/12"]);
  assert.deepEqual(skin.inverseBind[0], M.translation([0, -2, 0]));

  const levels = [new Array(4 * 2 * 4).fill(0x3c00), new Array(2 * 1 * 4).fill(0x3800)];
  const envBytes = encodeEnv(4, 2, Array.from({ length: 27 }, (_, i) => i), levels);
  const env = S.decode.E(envBytes);
  assert.deepEqual(env.levels.map((l) => [l.w, l.h, l.data.length, l.data[0]]), [[4, 2, 32, 0x3c00], [2, 1, 8, 0x3800]]);
  assert.equal(env.sh[26], 26);
  const longer = new Uint8Array(envBytes.byteLength + 4);
  longer.set(new Uint8Array(envBytes));
  assert.equal(S.decode.E(longer.buffer), null, "trailing bytes");

  // Every truncation, and garbage: null, never a throw.
  const small = new Uint8Array(encodeMesh({ positions: [0, 0, 0, 0, 0, 0, 0, 0, 0], normals: [0, 1, 0, 0, 1, 0, 0, 1, 0], indices: [0, 1, 2] }));
  for (let n = 0; n < small.length; n++) assert.equal(S.decode.M(small.slice(0, n).buffer), null, `mesh[..${n}]`);
  const clipBytes = new Uint8Array(encodeClip([{ target: "", property: 0, interpolation: 0, times: [0, 1], values: [0, 0, 0, 1, 1, 1] }]));
  for (let n = 0; n < clipBytes.length; n++) assert.equal(S.decode.A(clipBytes.slice(0, n).buffer), null, `clip[..${n}]`);
  assert.equal(S.decode.K(new Uint8Array([80, 51, 68, 75, 1, 0, 0, 0, 255, 255, 255, 255]).buffer), null);
  assert.equal(S.decode.I(new Uint8Array([80, 51, 68, 73, 1, 0, 0, 0, 255, 255, 255, 127]).buffer), null);
  assert.equal(S.decode.M(clipBytes.buffer), null, "the wrong kind");
  const bad = small.slice();
  new DataView(bad.buffer).setUint32(bad.length - 4, 7, true);
  assert.equal(S.decode.M(bad.buffer), null, "an index out of range");
});

test("3D: the update format's defaults; unreadable updates are dropped", () => {
  const u = S.decodeUpdate(JSON.stringify(full({ bg: [1, 0, 0, 1, 0, 0, 1, 0.5] }, [{ key: "#a", name: "a", mesh: BOX }, { key: "#a/0", parent: "#a" }])));
  assert.equal(u.full, true);
  assert.deepEqual(u.settings.camera, { pos: [0, 2, 6], target: [0, 0, 0], fov: 45, near: 0.1, far: 1000, ortho: null });
  assert.deepEqual([u.settings.orbit, u.settings.tap, u.settings.env, u.settings.exposure], [null, false, null, 1]);
  assert.deepEqual(u.settings.bg, { light: [1, 0, 0, 1], dark: [0, 0, 1, 0.5] });
  const n = u.nodes[0];
  assert.deepEqual([n.pos, n.rot, n.quat, n.scale, n.spin, n.hidden, n.cast, n.recv], [[0, 0, 0], [0, 0, 0], null, [1, 1, 1], [0, 0, 0], false, true, true]);
  assert.deepEqual([n.mat.metal, n.mat.rough, n.mat.emissive, n.mat.alpha, n.mat.normal_scale, n.mat.map], [0, 0.5, null, "blend", 1, null]);
  assert.equal(n.mesh.key, "box:1,1,1");
  assert.deepEqual(n.mesh.bounds, [-0.5, -0.5, -0.5, 0.5, 0.5, 0.5]);
  assert.equal(u.nodes[1].mesh, null, "a group");
  assert.equal(S.decodeUpdate("not json"), null);
  assert.equal(S.decodeUpdate({ v: 1 }), null);
  const blob = S.decodeUpdate({ v: 2, full: true, nodes: [{ key: "0", mesh: { type: "blob", id: "zz", bounds: [0, 0, 0, 0, 0, 0] } }] });
  assert.equal(blob.nodes[0].mesh, null, "a bad blob id: no mesh");
  const diff = S.decodeUpdate({ v: 2 });
  assert.deepEqual(diff, { full: false, settings: null, nodes: [], remove: [], order: null });
});

/** wire.rs' keys: `#name`, else `<parent key>/<index>` (`<index>` at the top level); `~n` for a key taken twice. */
function flatten(nodes) {
  const out = [], used = new Set();
  const walk = (list, parent) => list.forEach((n, i) => {
    let key = n.name ? `#${n.name}` : parent === null ? String(i) : `${parent}/${i}`;
    if (used.has(key)) { let k = 2; while (used.has(`${key}~${k}`)) k++; key = `${key}~${k}`; }
    used.add(key);
    const { children, ...props } = n;
    out.push({ key, ...(parent !== null ? { parent } : {}), ...props });
    walk(children || [], key);
  });
  walk(nodes, null);
  return out;
}
/** `FlatScene::diff` (on the wire form). */
function diff(a, b) {
  const old = new Map(a.nodes.map((n) => [n.key, JSON.stringify(n)]));
  const kept = new Set(b.nodes.map((n) => n.key));
  const upsert = b.nodes.filter((n) => old.get(n.key) !== JSON.stringify(n));
  const remove = a.nodes.filter((n) => !kept.has(n.key)).map((n) => n.key);
  const sameOrder = a.nodes.length === b.nodes.length && a.nodes.every((n, i) => n.key === b.nodes[i].key);
  const u = { v: 2 };
  if (JSON.stringify(a.settings) !== JSON.stringify(b.settings)) u.settings = b.settings;
  if (upsert.length) u.upsert = upsert;
  if (remove.length) u.remove = remove;
  if (!sameOrder) u.order = b.nodes.map((n) => n.key);
  return Object.keys(u).length > 1 ? u : null;
}

test("3D: diffs apply back to the scene (the Rust vector, then random edits)", () => {
  // tests.rs `diffs_carry_only_what_changed`: the JSON it asserts, applied here.
  const scene = (x) => flatten([{ name: "a", mesh: BOX, pos: [x, 0, 0] }, { name: "b", mesh: { type: "sphere", radius: 1 }, children: [{ mesh: BOX }] }]);
  const rust = '{"v":2,"upsert":[{"key":"#a","name":"a","mesh":{"type":"box","size":[1.0,1.0,1.0]},"pos":[1.0,0.0,0.0]}]}';
  const a = S.decodeUpdate(full({}, scene(0))), b = S.decodeUpdate(full({}, scene(1)));
  const applied = S.applyFlat(S.applyFlat(null, a), S.decodeUpdate(rust));
  assert.deepEqual(applied.nodes, b.nodes);
  assert.deepEqual(scene(0).map((n) => n.key), ["#a", "#b", "#b/0"]);

  // tests.rs `random_edits_diff_and_apply_back`, on this side: the diff applies back, and a
  // player fed it draws what one fed the whole scene draws.
  let seed = 0x9e3779b9;
  const rnd = () => { seed ^= seed << 13; seed ^= seed >>> 17; seed ^= seed << 5; return (seed >>> 0) / 4294967296; };
  const below = (n) => Math.floor(rnd() * n);
  const node = (depth) => ({
    ...(rnd() < 0.4 ? { name: `n${below(6)}` } : {}),
    ...(rnd() < 0.7 ? { mesh: rnd() < 0.5 ? BOX : { type: "sphere", radius: 0.5 } } : {}),
    pos: [below(9) - 4, below(9) - 4, below(5) - 2],
    ...(depth < 2 && rnd() < 0.4 ? { children: Array.from({ length: below(3) }, () => node(depth + 1)) } : {}),
  });
  const edit = (list) => {
    const i = below(list.length + 1);
    switch (below(6)) {
      case 0: list.splice(i, 0, node(1)); break;
      case 1: if (list.length) list.splice(below(list.length), 1); break;
      case 2: if (list[i]) list[i].pos = [below(9) - 4, 0, 0]; break;
      case 3: if (list[i]) list[i].hidden = !list[i].hidden; break;
      case 4: if (list[i]) list[i].quat = [0, Math.SQRT1_2, 0, Math.SQRT1_2]; break;
      default: if (list[i]) edit(list[i].children ||= []); else list.push(node(1));
    }
  };
  for (let round = 0; round < 200; round++) {
    const old = Array.from({ length: below(5) }, () => node(0));
    const fresh = JSON.parse(JSON.stringify(old));
    for (let k = 0; k <= below(6); k++) edit(fresh);
    const cam = rnd() < 0.2 ? { camera: camera([rnd(), 2, 6]) } : {};
    const fa = { settings: { camera: FAR }, nodes: flatten(old) }, fb = { settings: cam.camera ? cam : { camera: FAR }, nodes: flatten(fresh) };
    const d = diff(fa, fb);
    const A = S.decodeUpdate(full(fa.settings, fa.nodes)), B = S.decodeUpdate(full(fb.settings, fb.nodes));
    const back = d ? S.applyFlat(S.applyFlat(null, A), S.decodeUpdate(JSON.stringify(d))) : S.applyFlat(null, A);
    assert.equal(JSON.stringify(back), JSON.stringify(S.applyFlat(null, B)), `round ${round}`);
    const p = player(full(fa.settings, fa.nodes));
    if (d) show(p, d, 0);
    const q = player(full(fb.settings, fb.nodes));
    const keys = (f) => f.draws.map((x) => [x.key, x.world.map((v) => Math.round(v * 1e5) / 1e5)]);
    assert.deepEqual(keys(p.frame(100, 100, false)), keys(q.frame(100, 100, false)), `round ${round}`);
  }
});

/** `named_scene`: a cube "a" at x, a linear 1 s transition. */
const named = (x) => full({ camera: FAR, t: linear }, [cube("#a", { pos: [x, 0, 0] })]);

test("3D: changes animate with the scene's transition", () => {
  const p = player(named(0));
  show(p, named(10), 1);
  p.advance(1.5);
  let f = p.frame(100, 100, false);
  close(f.draws[0].world[12], 5, 1e-3, "halfway");
  assert.ok(f.animating);
  p.advance(2.5);
  f = p.frame(100, 100, false);
  assert.equal(f.draws[0].world[12], 10);
  assert.ok(!f.animating);
  // A change mid-transition starts from where the node is.
  const q = player(named(0));
  show(q, { v: 2, upsert: [cube("#a", { pos: [10, 0, 0] })] }, 0);
  q.advance(0.5);
  show(q, { v: 2, upsert: [cube("#a")] }, 0.5);
  q.advance(0.5);
  close(q.frame(1, 1, false).draws[0].world[12], 5, 1e-3);
});

test("3D: rotations transition by degrees, or by the shorter arc with a quaternion", () => {
  const scene = (extra) => full({ t: linear }, [cube("#a", extra)]);
  const p = player(scene({}));
  show(p, scene({ rot: [0, 360, 0] }), 0);
  p.advance(0.5);
  near(M.transformDir(p.frame(1, 1, false).draws[0].world, [0, 0, 1])[2], -1, "a full turn is a half turn halfway");
  const q = player(scene({}));
  show(q, scene({ quat: [0, Math.SQRT1_2, 0, Math.SQRT1_2] }), 0);
  q.advance(0.5);
  const z = M.transformDir(q.frame(1, 1, false).draws[0].world, [0, 0, 1]);
  near(z[0], Math.sin(Math.PI / 4), "45° halfway");
  near(z[2], Math.cos(Math.PI / 4));
});

test("3D: spin turns from when the node appeared, across updates", () => {
  const s = full({}, [cube("#a", { spin: [0, 90, 0] })]);
  const p = player(s, 10);
  p.advance(11);
  let f = p.frame(1, 1, false);
  assert.ok(f.animating);
  // A quarter turn around Y: +Z went to +X.
  close(M.transformDir(f.draws[0].world, [0, 0, 1])[0], 1, 1e-4);
  show(p, s, 11);
  f = p.frame(1, 1, false);
  close(M.transformDir(f.draws[0].world, [0, 0, 1])[0], 1, 1e-4, "the phase is kept");
});

test("3D: dragging orbits the camera, zoom is clamped, no orbit no gestures", () => {
  const p = player(full({ camera: camera([0, 0, 4]), orbit: { rotate: true, zoom: true } }, []));
  p.advance(0);
  close(p.frame(1, 1, false).cameraPosition[2], 4, 1e-4);
  // Dragging right by 180 points: the camera goes 90° to the left (−X).
  p.drag(180, 0);
  close(p.frame(1, 1, false).cameraPosition[0], -4, 1e-3);
  p.drag(0, 10000);
  const y = p.frame(1, 1, false).cameraPosition[1];
  assert.ok(y < 4 && y > 3.99, `pitch stops short of straight up: ${y}`);
  p.zoom(100);
  close(M.length(p.frame(1, 1, false).cameraPosition), 1, 1e-3, "a quarter of 4");

  const still = player(full({}, []));
  const before = still.frame(1, 1, false).cameraPosition;
  still.drag(100, 100);
  still.zoom(2);
  assert.deepEqual(still.frame(1, 1, false).cameraPosition, before);
});

test("3D: auto-rotation pauses while dragging", () => {
  const p = player(full({ orbit: { rotate: false, zoom: false, auto: 90 } }, []));
  p.advance(0);
  p.advance(0.2);
  const a = p.frame(1, 1, false).cameraPosition;
  p.pointerDown();
  p.advance(0.4);
  assert.deepEqual(p.frame(1, 1, false).cameraPosition, a);
  p.pointerUp();
  p.advance(0.6);
  assert.notDeepEqual(p.frame(1, 1, false).cameraPosition, a);
});

test("3D: taps hit the nearest named node and its part", () => {
  const p = player(full({ camera: camera([0, 0, 5]) }, flatten([
    { name: "robot", label: "body", children: [
      { label: "arm", children: [{ mesh: BOX, pos: [0, 0, 1] }] }, // in front
      { name: "back", mesh: BOX, pos: [0, 0, -2] },
    ] },
    { mesh: { type: "sphere", radius: 0.5 }, pos: [1.5, 0, 0] },
  ])));
  const hit = p.hit(50, 50, 100, 100, false);
  assert.deepEqual([hit.node, hit.part, hit.instance], ["robot", "arm", null], "the nearest label below the named node");
  close(hit.point[2], 1.5, 1e-3, "the front face of the front cube");
  // The unnamed sphere: hit, but nothing named.
  const x = 50 + (50 * 1.5) / (5 * Math.tan((22.5 * Math.PI) / 180));
  const sphereHit = p.hit(x, 50, 100, 100, false);
  assert.ok(sphereHit.point && sphereHit.node === null && sphereHit.part === null, JSON.stringify(sphereHit));
  assert.deepEqual(p.hit(1, 1, 100, 100, false), { node: null, part: null, instance: null, point: null });
});

test("3D: draw order: opaque by program, texture and mesh; masked opaque; transparent last, back to front", () => {
  const half = { color: [1, 1, 1, 0.5], rough: 0.5 };
  assert.deepEqual(names(player(full({}, [
    cube("#near", { mat: half, pos: [0, 0, 2] }),
    cube("#far", { mat: half, pos: [0, 0, -2] }),
    cube("#solid"),
    cube("#masked", { mat: { ...half, alpha: "mask", cutoff: 0.3 } }),
  ]))), ["solid", "masked", "far", "near"]);
  const blobs = store(), inst = blobs.put(encodeInstances([TRS()], null, [-0.5, -0.5, -0.5, 0.5, 0.5, 0.5]));
  const ball = { type: "sphere", radius: 1 };
  assert.deepEqual(names(player(full({}, [
    { key: "#s1", name: "s1", mesh: ball },
    cube("#c1", { mat: { color: [0.8, 0.8, 0.8, 1], rough: 0.5, map: "b.png" } }),
    cube("#c2"),
    { key: "#s2", name: "s2", mesh: ball, mat: { color: [0.8, 0.8, 0.8, 1], rough: 0.5, map: "a.png" } },
    cube("#i", { inst: { id: inst, count: 1, bounds: [-0.5, -0.5, -0.5, 0.5, 0.5, 0.5] } }),
    { key: "#s3", name: "s3", mesh: ball },
  ]), 0, blobs)), ["c2", "s1", "s3", "s2", "c1", "i"]);
});

test("3D: default lights, adaptive background, environment", () => {
  const p = player(full({ bg: [1, 1, 1, 1, 0, 0, 0, 1] }, []));
  const light = p.frame(1, 1, false);
  assert.deepEqual(light.clear, [1, 1, 1, 1]);
  assert.deepEqual(p.frame(1, 1, true).clear, [0, 0, 0, 1]);
  assert.equal(light.lights.length, 1);
  assert.ok(light.ambient[0] > 0);
  assert.equal(light.exposure, 1);
  assert.ok(light.env === null && light.shadow === null);
  // An environment lights the scene instead of the default rig.
  const id = "e".repeat(32);
  const lit = player(full({ env: { id, intensity: 0.5, background: true }, exposure: 1.5 }, [])).frame(1, 1, false);
  assert.equal(lit.lights.length, 0);
  assert.deepEqual(lit.env, { id, intensity: 0.5, background: true });
  assert.equal(lit.exposure, 1.5);
});

test("3D: nodes outside the view are culled", () => {
  const blobs = store(), set = blobs.put(encodeInstances([TRS([-50, 1, 0])], null, [-50.5, 0.5, -0.5, -49.5, 1.5, 0.5]));
  const nodes = [
    cube("#seen"),
    cube("#behind", { pos: [0, 0, 20] }),
    cube("#aside", { pos: [50, 0, 0] }),
    // Its instances reach into the view.
    cube("#crowd", { pos: [50, 0, 0], inst: { id: set, count: 1, bounds: [-50.5, 0.5, -0.5, -49.5, 1.5, 0.5] } }),
  ];
  const cam = { camera: camera([0, 0, 5]) };
  assert.deepEqual(names(player(full(cam, nodes), 0, blobs)), ["seen", "crowd"]);
  // Hidden subtrees draw nothing.
  assert.deepEqual(names(player(full(cam, [{ ...nodes[0], hidden: true }, ...nodes.slice(1)]), 0, blobs)), ["crowd"]);
});

test("3D: instances are hit one by one", () => {
  const blobs = store();
  const q45 = M.quatFromEuler([0, 45, 0]);
  const set = blobs.put(encodeInstances([TRS([-3, 0, 0]), TRS([0, 0, 0], q45), TRS([3, 0, 0], [0, 0, 0, 1], [2, 2, 2])], [[1, 1, 1, 1], [1, 0, 0, 1], [1, 1, 1, 1]], [-3.5, -1, -1, 4, 1, 1]));
  const p = player(full({ camera: camera([0, 0, 10]), tap: true }, [
    { key: "#crowd", name: "crowd" },
    { key: "#crowd/0", parent: "#crowd", label: "box", mesh: BOX, inst: { id: set, count: 3, bounds: [-3.5, -1, -1, 4, 1, 1] } },
  ]), 0, blobs);
  const f = p.frame(100, 100, false);
  assert.equal(f.draws.length, 1);
  assert.deepEqual(f.draws[0].instances, { id: set, count: 3 });
  near(f.draws[0].bounds[0], -3.5);
  near(f.draws[0].bounds[3], 4);
  const hit = p.hit(50, 50, 100, 100, false);
  assert.deepEqual([hit.node, hit.part, hit.instance], ["crowd", "box", 1]);
  // The big one on the right.
  const x = 50 + (50 * 3) / (9 * Math.tan((22.5 * Math.PI) / 180));
  assert.equal(p.hit(x, 50, 100, 100, false).instance, 2);
  assert.equal(p.hit(50, 5, 100, 100, false).instance, null);
});

test("3D: shadows fit the visible draws", () => {
  const s = {
    camera: camera([0, 3, 8]), orbit: { rotate: true, zoom: true },
    lights: [
      { type: "ambient", color: [1, 1, 1, 1], intensity: 0.2 },
      { type: "point", color: [1, 1, 1, 1], intensity: 1 },
      { type: "directional", color: [1, 1, 1, 1], intensity: 1, dir: [-1, -2, -1], shadows: true },
    ],
  };
  const nodes = [
    { key: "#floor", name: "floor", mesh: { type: "plane", size: [10, 10] }, cast: false },
    cube("#box", { pos: [1, 0.5, 0] }),
    { key: "#ball", name: "ball", mesh: { type: "sphere", radius: 0.5 }, pos: [-2, 1, 1] },
  ];
  const p = player(full(s, nodes));
  const f = p.frame(200, 100, false), sh = f.shadow;
  assert.ok(sh, "a shadow pass");
  assert.equal(sh.light, 1, "the directional light, after the point light");
  assert.equal(sh.mapSize, S.SHADOW_MAP_SIZE);
  const casters = sh.casters.map((i) => f.draws[i].name);
  assert.equal(casters.length, 2);
  assert.ok(!casters.includes("floor"));
  // Every visible draw is inside the light's clip volume.
  for (const d of f.draws) {
    const b = d.bounds;
    for (let c = 0; c < 8; c++) {
      const corner = [b[c & 1 ? 3 : 0], b[c & 2 ? 4 : 1], b[c & 4 ? 5 : 2]];
      const q = M.transformPoint(sh.viewProj, corner);
      assert.ok(q.every((v) => Math.abs(v) <= 1.0001), `${corner} → ${q}`);
    }
  }
  assert.ok(sh.bias > 0 && sh.normalBias > 0);
  // Turning the camera (everything still in view): the same map.
  p.drag(10, 0);
  const g = p.frame(200, 100, false);
  if (g.draws.length === f.draws.length) assert.deepEqual(g.shadow.viewProj, sh.viewProj);
  // No light with shadows: no pass.
  assert.equal(player(full({ ...s, lights: s.lights.slice(0, 2) }, nodes)).frame(1, 1, false).shadow, null);
});

test("3D: clips sample linear, step and cubic keys", () => {
  const lin = { target: "", property: 0, interpolation: 0, times: [0, 1, 3], values: [0, 0, 0, 10, 0, 0, 10, 20, 0] };
  assert.equal(S.sampleChannel(lin, -1)[0], 0);
  assert.equal(S.sampleChannel(lin, 0.5)[0], 5);
  assert.equal(S.sampleChannel(lin, 2)[1], 10);
  assert.equal(S.sampleChannel(lin, 9)[1], 20);
  const step = { ...lin, interpolation: 1 };
  assert.equal(S.sampleChannel(step, 0.99)[0], 0);
  assert.equal(S.sampleChannel(step, 1)[0], 10);
  // glTF cubic spline: keys (in, value, out), at s = 0.25 (dt = 2).
  const cubic = { target: "", property: 2, interpolation: 2, times: [1, 3], values: [0, 0, 0, 0, 0, 0, 2, 0, 0, -4, 0, 0, 10, 0, 0, 0, 0, 0] };
  const s = 0.25;
  const expected = (s ** 3 - 2 * s * s + s) * 2 * 2 + (-2 * s ** 3 + 3 * s * s) * 10 + (s ** 3 - s * s) * 2 * -4;
  near(S.sampleChannel(cubic, 1.5)[0], expected);
  assert.equal(S.sampleChannel(cubic, 3)[0], 10);
  // Rotations slerp and stay unit length.
  const rot = { target: "", property: 1, interpolation: 0, times: [0, 1], values: [0, 0, 0, 1, 0, 1, 0, 0] };
  nearMat(M.quatToMat4(S.sampleChannel(rot, 0.5)), M.rotationY(90));
  // Flat tangents (the builder's cubic keys) ease in and out.
  const eased = { target: "", property: 0, interpolation: 2, times: [0, 1], values: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0] };
  near(S.sampleChannel(eased, 0.25)[0], 0.15625);
  near(S.sampleChannel(eased, 0.5)[0], 0.5);
  assert.deepEqual(S.sampleChannel({ ...lin, times: [], values: [] }, 1), [0, 0, 0, 0]);
});

/** `clip_scene`: group "rig" ⊃ cube "box" ⊃ sphere, the rig playing `play`. */
const clipScene = (play) => full({ camera: FAR }, [
  { key: "#rig", name: "rig", ...(play ? { play } : {}) },
  { key: "#box", parent: "#rig", name: "box", mesh: BOX },
  { key: "#box/0", parent: "#box", mesh: { type: "sphere", radius: 0.2 } },
]);
const xOf = (p, key) => p.frame(1, 1, false).draws.find((d) => d.key === key).world[12];

test("3D: clips play on their targets and hold or loop", () => {
  const blobs = store();
  const slide = blobs.put(encodeClip([
    { target: "0", property: 0, interpolation: 0, times: [0, 2], values: [0, 0, 0, 4, 0, 0] },
    { target: "#box/0", property: 0, interpolation: 0, times: [0, 2], values: [0, 0, 0, 0, 0, 0] },
  ]));
  const p = player(clipScene({ clip: slide }), 10, blobs);
  p.advance(11);
  near(xOf(p, "#box"), 2);
  assert.ok(p.frame(1, 1, false).animating);
  p.advance(13);
  near(xOf(p, "#box"), 4, "holds the last pose");
  assert.ok(!p.frame(1, 1, false).animating);
  // An update that changes nothing about the clip keeps it where it is.
  show(p, { v: 2, settings: { camera: camera([1, 0, 40]) } }, 13.5);
  near(xOf(p, "#box"), 4);
  // Looping at double speed: 2.5 s in is 1 s into the second loop.
  const looping = clipScene({ clip: slide, loop: true, speed: 2 });
  const q = player(looping, 0, blobs);
  q.advance(2.5);
  near(xOf(q, "#box"), 2);
  assert.ok(q.frame(1, 1, false).animating);
  // Stopping: back to the node's own pose.
  show(q, clipScene(null), 2.6);
  near(xOf(q, "#box"), 0);
  // Restarting with `run`.
  show(q, looping, 3);
  show(q, clipScene({ clip: slide, loop: true, speed: 2, run: 1 }), 3.5);
  q.advance(4);
  near(xOf(q, "#box"), 2, "restarted at 3.5");
});

test("3D: clips crossfade", () => {
  const blobs = store();
  const at = (x) => blobs.put(encodeClip([{ target: "0", property: 0, interpolation: 0, times: [0], values: [x, 0, 0] }]));
  const [a, b, c] = [at(0), at(10), at(3)];
  const p = player(clipScene({ clip: a, loop: true }), 0, blobs);
  show(p, clipScene({ clip: b, loop: true, fade: 1 }), 1);
  p.advance(1.5);
  near(xOf(p, "#box"), 5);
  assert.ok(p.frame(1, 1, false).animating, "fading");
  p.advance(2.5);
  near(xOf(p, "#box"), 10);
  assert.ok(!p.frame(1, 1, false).animating);
  // Without a fade: at once.
  show(p, clipScene({ clip: c, loop: true }), 3);
  near(xOf(p, "#box"), 3);
});

/**
 * tests.rs `skinned()` imported and flattened: the model's node "robot" (at x = 5) ⊃ body
 * (x = 1) ⊃ [bone (y = 2), the skinned triangle]; lamp. The skin's root is the model's node.
 */
function skinnedModel(blobs, extra = {}) {
  const mesh = blobs.put(encodeMesh({
    positions: [0, 0, 0, 1, 0, 0, 0, 1, 0], normals: [0, 0, 1, 0, 0, 1, 0, 0, 1], uvs: [0, 0, 0, 0, 0, 0],
    joints: new Array(12).fill(0), weights: [1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0], indices: [0, 1, 2],
  }));
  const skin = blobs.put(encodeSkin([M.translation([0, -2, 0])], ["0/0"]));
  const lift = blobs.put(encodeClip([{ target: "0/0", property: 0, interpolation: 0, times: [0, 1], values: [0, 2, 0, 0, 3, 0] }]));
  return {
    lift,
    nodes: (play) => [
      { key: "#robot", name: "robot", ...extra, ...(play ? { play } : {}) },
      { key: "#robot/0", parent: "#robot", label: "body", pos: [1, 0, 0] },
      { key: "#robot/0/0", parent: "#robot/0", label: "bone", pos: [0, 2, 0] },
      { key: "#robot/0/1", parent: "#robot/0", mesh: { type: "blob", id: mesh, bounds: [0, 0, 0, 1, 1, 0] }, skin: { id: skin, root: "#robot" },
        mat: { color: [0.5, 1, 0, 0.5], rough: 0.75, alpha: "mask", cutoff: 0.4, double: true } },
      { key: "#robot/1", parent: "#robot", label: "lamp", pos: [0, 0, -1] },
    ],
  };
}

test("3D: skins compute joint matrices, and their boxes follow them", () => {
  const blobs = store(), model = skinnedModel(blobs, { pos: [5, 0, 0] });
  const p = player(full({ camera: FAR, tap: true }, model.nodes(null)), 0, blobs);
  const draw = p.frame(1, 1, false).draws.find((d) => d.joints);
  assert.ok(draw, "a skinned draw");
  // At rest: inverse(mesh world) · bone world · inverse bind = identity.
  nearMat(draw.joints[0], M.IDENTITY);
  // The clip lifts the bone by 1: the joint moves the skin up by 1.
  show(p, full({ camera: FAR, tap: true }, model.nodes({ clip: model.lift })), 0);
  p.advance(1);
  const lifted = p.frame(1, 1, false).draws.find((d) => d.joints);
  nearMat(lifted.joints[0], M.translation([0, 1, 0]));
  const b = lifted.bounds;
  assert.ok([b[0] - 6, b[3] - 7, b[1] - 1, b[4] - 2].every((v) => Math.abs(v) < 1e-4), `${b}`);
});

test("3D: model parts are reported by taps", () => {
  const blobs = store(), model = skinnedModel(blobs);
  const hit = player(full({ camera: camera([1.25, 0.25, 4], [1.25, 0.25, 0]) }, model.nodes(null)), 0, blobs).hit(50, 50, 100, 100, false);
  assert.deepEqual([hit.node, hit.part], ["robot", "body"]);
});

test("3D: skinned meshes are placed by their joints (a centimeter rig)", () => {
  // tests.rs `centimeter_rig()` flattened: Armature (1/100) ⊃ [body (1/100) ⊃ quad, Hips (5 m aside)].
  const blobs = store();
  const quad = blobs.put(encodeMesh({
    positions: [-20, 0, 0, 20, 0, 0, 20, 188, 0, -20, 188, 0], normals: [0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1],
    joints: new Array(16).fill(0), weights: [1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0], indices: [0, 1, 2, 0, 2, 3],
  }));
  const skin = blobs.put(encodeSkin([M.IDENTITY], ["0/1"]));
  const rise = blobs.put(encodeClip([{ target: "0/1", property: 0, interpolation: 0, times: [0, 1], values: [500, 0, 0, 500, 100, 0] }]));
  const cm = [0.01, 0.01, 0.01];
  const nodes = (play) => [
    { key: "#soldier", name: "soldier", ...(play ? { play } : {}) },
    { key: "#soldier/0", parent: "#soldier", label: "Armature", scale: cm },
    { key: "#soldier/0/0", parent: "#soldier/0", label: "body", scale: cm },
    { key: "#soldier/0/0/0", parent: "#soldier/0/0", mesh: { type: "blob", id: quad, bounds: [-20, 0, 0, 20, 188, 0] }, skin: { id: skin, root: "#soldier" } },
    { key: "#soldier/0/1", parent: "#soldier/0", label: "Hips", pos: [500, 0, 0] },
  ];
  const look = (y) => camera([5, y, 4], [5, y, 0]);
  const p = player(full({ camera: look(0.94), tap: true }, nodes(null)), 0, blobs);
  // Not culled (its node, at the origin and 1.9 cm tall, is out of view).
  const draw = p.frame(100, 100, false).draws.find((d) => d.joints);
  assert.ok(draw, "drawn");
  [4.8, 0, 0, 5.2, 1.88, 0].forEach((v, i) => near(draw.bounds[i], v, `bounds[${i}]`));
  const hit = p.hit(50, 50, 100, 100, false);
  assert.deepEqual([hit.node, hit.part], ["soldier", "body"]);
  near(hit.point[0], 5); near(hit.point[1], 0.94); near(hit.point[2], 0);
  // Playing: the joint rises 1 m; the box, culling and hits follow it.
  show(p, full({ camera: look(1.94), tap: true }, nodes({ clip: rise, loop: true })), 0);
  p.advance(0.999);
  const b = p.frame(100, 100, false).draws[0].bounds;
  assert.ok(Math.abs(b[1] - 1) < 0.01 && Math.abs(b[4] - 2.88) < 0.01, `${b}`);
  const moved = p.hit(50, 50, 100, 100, false);
  assert.equal(moved.node, "soldier");
  close(moved.point[1], 1.94, 0.01);
});

test("3D: updates whose blobs have not arrived draw what they can, then the rest", () => {
  const id = "0123456789abcdef0123456789abcdef";
  const u = { v: 2, full: true, settings: { env: { id } }, nodes: [{
    key: "#a", name: "a", mesh: { type: "blob", id, bounds: [-1, -1, -1, 1, 1, 1] },
    inst: { id, count: 3, bounds: [-1, -1, -1, 1, 1, 1] }, play: { clip: id, loop: true }, skin: { id, root: "#a" },
  }] };
  const blobs = store();
  const p = player(u, 0, blobs);
  p.advance(1);
  const f = p.frame(100, 100, false);
  assert.equal(f.draws.length, 1);
  assert.equal(f.draws[0].joints, null, "no skin until it arrives");
  assert.equal(p.geometry(f.draws[0]).vertexCount(), 0);
  assert.deepEqual(p.hit(50, 50, 100, 100, false), { node: null, part: null, instance: null, point: null });
  // A clip that arrives later starts then.
  const q = player(clipScene({ clip: "c".repeat(32), loop: true }), 0, blobs);
  q.advance(5);
  near(xOf(q, "#box"), 0);
  blobs.bytes.set("c".repeat(32), encodeClip([{ target: "0", property: 0, interpolation: 0, times: [0, 2], values: [0, 0, 0, 4, 0, 0] }]));
  q.blobsArrived(5);
  q.advance(6);
  near(xOf(q, "#box"), 2, "1 s after it arrived");
});

test("3D: easing follows CSS cubic-bezier curves", () => {
  // A reference solved by Newton's method on x(t) = p.
  const reference = ([x1, y1, x2, y2], p) => {
    const at = (a, b, t) => 3 * (1 - t) ** 2 * t * a + 3 * (1 - t) * t * t * b + t ** 3;
    const slope = (a, b, t) => 3 * (1 - t) ** 2 * a + 6 * (1 - t) * t * (b - a) + 3 * t * t * (1 - b);
    let t = p;
    for (let i = 0; i < 50; i++) t -= (at(x1, x2, t) - p) / slope(x1, x2, t);
    return at(y1, y2, t);
  };
  for (const curve of [[0.25, 0.1, 0.25, 1], [0.42, 0, 0.58, 1], [0, 0, 1, 1], [0, 0, 0.58, 1]]) {
    for (const p of [0.1, 0.5, 0.9]) close(S.ease(curve, p), reference(curve, p), 1e-5, JSON.stringify(curve));
  }
  assert.equal(S.ease([0.25, 0.1, 0.25, 1], -1), 0);
  assert.equal(S.ease([0.25, 0.1, 0.25, 1], 2), 1);
});

test("3D: the GLSL is shaders.rs', put in like the server does, assembled like program_source", () => {
  const rs = fs.readFileSync(new URL("../../../prism-core/src/scene3d/shaders.rs", import.meta.url), "utf8");
  // client.js keeps no copy of the GLSL to drift: the server puts shaders.rs' in.
  assert.ok(!raw.includes("precision highp"), "a copy of the GLSL in client.js");
  for (const name of ["VERTEX", "FRAGMENT", "SKY_VERTEX", "SKY_FRAGMENT"]) {
    assert.ok(raw.includes(`const ${name} = __SCENE3D_${name}__;`), `${name}'s placeholder`);
    assert.equal(S[name], shaderSource(name), name);
  }
  assert.equal(S.ES_HEADER, /pub const ES_HEADER: &str = "(.*)";/.exec(rs)[1].replace("\\n", "\n"));
  assert.equal(S.MAX_LIGHTS, Number(/pub const MAX_LIGHTS: usize = (\d+);/.exec(rs)[1]));
  const src = S.programSource(S.ES_HEADER, "vertex", { instanced: true, skinned: true, depthOnly: true });
  assert.ok(src.startsWith("#version 300 es\n#define INSTANCED\n#define SKINNED\n#define DEPTH_ONLY\nprecision highp float;"));
  assert.equal(S.programSource("#version 330 core\n", "fragment", {}), `#version 330 core\n${S.FRAGMENT}`);
});

// --- The element, with a recording WebGL 2 context -------------------------------------

/**
 * A WebGL 2 context that records every call (constants are their names; uniform locations
 * `{ uniform }`), and keeps the one piece of state that bites: the element buffer belongs to the
 * bound vertex array (`errors`: draws whose vertex array has none).
 */
function fakeGl() {
  const calls = [], errors = [], elements = new Map();
  let lost = false, made = 0, vao = null;
  const track = (k, args) => {
    if (k === "bindVertexArray") vao = args[0];
    if (k === "bindBuffer" && args[0] === "ELEMENT_ARRAY_BUFFER") elements.set(vao, args[1]);
    if ((k === "drawElements" || k === "drawElementsInstanced") && !elements.get(vao)) errors.push([k, ...args]);
  };
  return new Proxy({}, {
    get(_, k) {
      if (k === "calls") return calls;
      if (k === "errors") return errors;
      if (k === "isContextLost") return () => lost;
      if (k === "getShaderParameter" || k === "getProgramParameter") return () => true;
      if (k === "getParameter") return () => 0;
      if (k === "getUniformLocation") return (prog, name) => ({ uniform: name });
      if (k === "getExtension") return (name) => (name === "WEBGL_lose_context" ? { loseContext: () => { lost = true; calls.push(["loseContext"]); } } : null);
      if (typeof k === "string" && /^[A-Z][A-Z0-9_]*$/.test(k)) return k;
      return (...args) => { calls.push([k, ...args]); track(k, args); return { of: k, n: ++made }; };
    },
  });
}
let webgl2 = true;
Element.prototype.getContext = function (type) {
  if (type !== "webgl2" || !webgl2) return null;
  return (this._gl ||= fakeGl());
};

const CUBE = full({ camera: camera([0, 2, 6]), orbit: { rotate: true, zoom: true }, tap: true }, [cube("#cube")]);
const create = (id, kind) => ({ op: "create", id, kind });
const sceneOp = (id, u) => ({ op: "scene", id, u });

/** Lays the canvas out at `w`×`h` points (the ResizeObserver's report). */
function size(page, canvas, w, h) {
  for (const o of page.window.resizeObservers) {
    if (o.targets.includes(canvas)) o.cb([{ target: canvas, contentRect: { width: w, height: h } }]);
  }
}
/** Runs the pending animation frames. */
const frames = (page) => page.frames.splice(0).forEach((f) => f(0));
const named_ = (gl, name) => gl.calls.filter((c) => c[0] === name);
const draws = (gl) => named_(gl, "drawElements").length + named_(gl, "drawElementsInstanced").length;
const uniform = (gl, fn, name) => gl.calls.filter((c) => c[0] === fn && c[1] && c[1].uniform === name).map((c) => c.at(-1));
const pointer = (canvas, type, x, y, extra = {}) =>
  canvas.dispatchEvent({ type, pointerId: 1, pointerType: "mouse", button: 0, clientX: x, clientY: y, ...extra });
const settle = () => new Promise((r) => setImmediate(r));
/** Serves blobs to the page: `fetch(<base>/_prism/3d/<id>)` → the bytes, once `release()` (or at once). */
function serve(page, blobs, { held = false } = {}) {
  const asked = [], waiting = [];
  page.window.fetch = (url) => {
    asked.push(url);
    const id = url.split("/").pop();
    return new Promise((ok) => {
      const answer = () => ok(blobs.bytes.has(id)
        ? { ok: true, status: 200, headers: { get: () => "application/octet-stream" }, arrayBuffer: () => Promise.resolve(blobs.bytes.get(id)) }
        : { ok: false, status: 404, headers: { get: () => "" } });
      if (held) waiting.push(answer); else answer();
    });
  };
  return { asked, release: () => waiting.splice(0).forEach((f) => f()) };
}

test("3D: the first paint's scene starts before the socket, the live tree takes it over", () => {
  const html = (s) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;");
  const page = boot({ ssr: `<div data-pid="4" data-kind="scene3d" class="prism-scene3d" data-scene="${html(JSON.stringify(CUBE))}"><canvas aria-hidden="true"></canvas></div>` });
  const canvas = page.mount.querySelector("canvas");
  const gl = canvas._gl;
  assert.ok(gl, "a WebGL 2 context");
  size(page, canvas, 200, 100);
  frames(page);
  assert.ok(gl.calls.some((c) => c[0] === "shaderSource" && c[2].startsWith("#version 300 es\nprecision highp float;")), "the plain program");
  assert.equal(canvas.width, 400, "the backing store is the size × devicePixelRatio");
  assert.equal(canvas.height, 200);
  assert.equal(draws(gl), 1);
  assert.ok(gl.calls.some((c) => c[0] === "clearColor" && c.slice(1).join() === "0,0,0,0"), "transparent without a background");
  assert.equal(page.frames.length, 0, "a still scene draws once, then idles");
  assert.equal(canvas.style.touchAction, "none", "orbiting owns the touches");

  page.ws.open();
  page.ws.deliver({ t: "reset", p: [create(4, "scene3d"), sceneOp(4, CUBE), { op: "root", id: 4 }] });
  const el = page.node(4);
  assert.equal(el.className, "prism-scene3d");
  assert.equal(el.querySelector("canvas"), canvas, "the same canvas, context and camera");
  assert.ok(!gl.calls.some((c) => c[0] === "loseContext"));
  assert.equal(page.frames.length, 0, "the same scene: nothing to draw again");
});

test("3D: diffs, gestures, taps and teardown", () => {
  const page = boot();
  page.ws.open();
  page.ws.deliver({ t: "reset", p: [create(1, "view"), create(2, "scene3d"), sceneOp(2, CUBE), { op: "children", id: 1, children: [2] }, { op: "root", id: 1 }] });
  const parentClicks = [];
  page.node(1).addEventListener("click", () => parentClicks.push(1));
  const canvas = page.node(2).querySelector("canvas");
  const gl = canvas._gl, st = canvas._scene3d;
  size(page, canvas, 200, 100);
  frames(page);
  assert.equal(draws(gl), 1);

  // A tap (moved less than 6 points) reports the named node and the point.
  pointer(canvas, "pointerdown", 100, 50);
  pointer(canvas, "pointermove", 103, 50);
  pointer(canvas, "pointerup", 103, 50);
  canvas.dispatchEvent({ type: "click" });
  const tap = page.ws.sent.filter((m) => m.t === "scene_tap");
  assert.equal(tap.length, 1);
  assert.deepEqual([tap[0].id, tap[0].node, tap[0].part, tap[0].instance], [2, "cube", null, null]);
  close(tap[0].point[2], 0.5, 1e-3, "on the cube's front face");
  assert.deepEqual(parentClicks, [], "the scene's tap is not the parent's click");

  // A drag turns the camera (and is not a tap).
  const yaw = st.player.orbit.yaw;
  pointer(canvas, "pointerdown", 100, 50);
  pointer(canvas, "pointermove", 140, 50);
  pointer(canvas, "pointerup", 140, 50);
  close(st.player.orbit.yaw, yaw - 20, 1e-9);
  assert.equal(page.ws.sent.filter((m) => m.t === "scene_tap").length, 1);
  frames(page);
  assert.equal(draws(gl), 2, "drawn again after the gesture, then idle");
  assert.equal(page.frames.length, 0);

  // The wheel zooms (and keeps the page from scrolling).
  const distance = st.player.orbit.distance;
  const wheel = { type: "wheel", deltaY: -100, deltaMode: 0 };
  canvas.dispatchEvent(wheel);
  assert.ok(wheel.defaultPrevented);
  close(st.player.orbit.distance, distance / Math.exp(0.2), 1e-9);

  // A diff keeps the player (and the user's camera); a still scene lets the page scroll.
  const settings = { ...CUBE.settings, orbit: undefined };
  page.ws.deliver({ t: "patch", p: [sceneOp(2, { v: 2, settings, upsert: [cube("#cube", { pos: [1, 0, 0] })] })] });
  assert.equal(canvas._scene3d, st);
  assert.equal(canvas.style.touchAction, "");
  const still = { type: "wheel", deltaY: -100, deltaMode: 0 };
  canvas.dispatchEvent(still);
  assert.ok(!still.defaultPrevented);
  frames(page);
  assert.equal(uniform(gl, "uniformMatrix4fv", "u_model").at(-1)[12], 1, "the cube moved");
  // Added and removed nodes.
  page.ws.deliver({ t: "patch", p: [sceneOp(2, { v: 2, upsert: [cube("#b", { pos: [-1, 0, 0] })], order: ["#cube", "#b"] })] });
  frames(page);
  assert.deepEqual(Array.from(st.player.frame(1, 1, false).draws, (d) => d.key), ["#cube", "#b"]);
  page.ws.deliver({ t: "patch", p: [sceneOp(2, { v: 2, remove: ["#cube"], order: ["#b"] })] });
  assert.deepEqual(Array.from(st.flat.nodes, (n) => n.key), ["#b"]);

  // Gone: its frame, observers and context are released.
  page.ws.deliver({ t: "patch", p: [{ op: "children", id: 1, children: [] }, { op: "destroy", id: 2 }] });
  assert.ok(gl.calls.some((c) => c[0] === "loseContext"));
  assert.equal(canvas._scene3d, null);
});

test("3D: animating scenes keep drawing; textures load, then draw", () => {
  const page = boot();
  const images = [];
  page.window.Image = class { constructor() { images.push(this); } };
  page.ws.open();
  const spinning = full({}, [cube("#a", { spin: [0, 90, 0], mat: { color: [1, 1, 1, 1], rough: 0.5, map: "/assets/wood.png", normal_map: "/assets/n.png" } })]);
  page.ws.deliver({ t: "reset", p: [create(1, "scene3d"), sceneOp(1, spinning), { op: "root", id: 1 }] });
  const canvas = page.node(1).querySelector("canvas");
  size(page, canvas, 100, 100);
  frames(page);
  assert.equal(page.frames.length, 1, "spinning: another frame");
  assert.deepEqual(images.map((i) => [i.src, i.crossOrigin]), [["/assets/wood.png", "anonymous"], ["/assets/n.png", "anonymous"]]);
  const gl = canvas._gl;
  assert.deepEqual(uniform(gl, "uniform1i", "u_has_texture"), [0], "untextured until it loads");
  images[0].onload();
  frames(page);
  assert.ok(gl.calls.some((c) => c[0] === "texImage2D" && c[6] === images[0]), "uploaded at the next frame");
  assert.ok(gl.calls.some((c) => c[0] === "generateMipmap"));
  assert.ok(gl.calls.some((c) => c[0] === "texParameteri" && c[2] === "TEXTURE_WRAP_S" && c[3] === "REPEAT"));
  assert.deepEqual(uniform(gl, "uniform1i", "u_has_texture"), [0, 1]);
  assert.deepEqual(uniform(gl, "uniform1i", "u_has_normal_map"), [0, 0]);
});

test("3D: blobs are fetched once per page, scenes draw when they arrive, then idle", async () => {
  const page = boot({ base: "/app" });
  const blobs = store();
  const mesh = blobs.put(encodeMesh({ positions: [0, 0, 0, 1, 0, 0, 0, 1, 0], normals: [0, 0, 1, 0, 0, 1, 0, 0, 1], indices: [0, 1, 2] }));
  const net = serve(page, blobs, { held: true });
  page.ws.open();
  const scene = full({ camera: camera([0, 0, 3]) }, [{ key: "#t", name: "t", mesh: { type: "blob", id: mesh, bounds: [0, 0, 0, 1, 1, 0] } }]);
  page.ws.deliver({ t: "reset", p: [create(1, "view"), create(2, "scene3d"), create(3, "scene3d"), sceneOp(2, scene), sceneOp(3, scene), { op: "children", id: 1, children: [2, 3] }, { op: "root", id: 1 }] });
  const canvases = [2, 3].map((id) => page.node(id).querySelector("canvas"));
  canvases.forEach((c) => size(page, c, 100, 100));
  frames(page);
  assert.deepEqual(net.asked, [`/app/_prism/3d/${mesh}`], "one fetch for both scenes");
  assert.deepEqual(canvases.map((c) => draws(c._gl)), [0, 0], "nothing to draw yet");
  assert.equal(page.frames.length, 0, "no frames while waiting");
  net.release();
  await settle();
  assert.equal(page.frames.length, 2, "both scenes draw again when it arrives");
  frames(page);
  assert.deepEqual(canvases.map((c) => draws(c._gl)), [1, 1]);
  assert.equal(page.frames.length, 0, "then idle");
  assert.equal(named_(canvases[0]._gl, "bufferData").length, 4, "positions, normals, uvs, indices: uploaded once");
  frames(page);
  assert.equal(net.asked.length, 1);
});

test("3D: instanced and skinned draws, shadow pass first, environment and sky", async () => {
  const page = boot();
  const blobs = store();
  const set = blobs.put(encodeInstances(Array.from({ length: 1000 }, (_, i) => TRS([(i % 10) - 5, Math.floor(i / 100) - 5, (Math.floor(i / 10) % 10) - 5])), null, [-5.5, -5.5, -5.5, 4.5, 4.5, 4.5]));
  const levels = [new Array(4 * 2 * 4).fill(0x3c00), new Array(2 * 1 * 4).fill(0x3c00)];
  const env = blobs.put(encodeEnv(4, 2, new Array(27).fill(0.5), levels));
  const model = skinnedModel(blobs);
  serve(page, blobs);
  page.ws.open();
  const scene = full({
    camera: camera([0, 3, 30]), env: { id: env, background: true },
    lights: [{ type: "directional", color: [1, 1, 1, 1], intensity: 1, dir: [-1, -2, -1], shadows: true }],
  }, [
    { key: "#floor", name: "floor", mesh: { type: "plane", size: [20, 20] }, pos: [0, -6, 0], cast: false },
    cube("#crowd", { mesh: { type: "box", size: [0.5, 0.5, 0.5] }, inst: { id: set, count: 1000, bounds: [-5.5, -5.5, -5.5, 4.5, 4.5, 4.5] } }),
    ...model.nodes({ clip: model.lift, loop: true }),
  ]);
  page.ws.deliver({ t: "reset", p: [create(1, "scene3d"), sceneOp(1, scene), { op: "root", id: 1 }] });
  const canvas = page.node(1).querySelector("canvas");
  size(page, canvas, 200, 100);
  await settle();
  await settle();
  frames(page);
  const gl = canvas._gl, calls = gl.calls.map((c) => c[0]);
  // The 1 000 cubes: one instanced draw, its matrices as 4 attribute columns of divisor 1.
  const instanced = named_(gl, "drawElementsInstanced");
  assert.equal(instanced.length, 2, "shadow pass and main pass");
  assert.equal(instanced[0][5], 1000);
  assert.deepEqual(named_(gl, "vertexAttribDivisor").map((c) => c[1]), [6, 7, 8, 9]);
  assert.ok(gl.calls.some((c) => c[0] === "shaderSource" && c[2].includes("#define INSTANCED\n#define DEPTH_ONLY\n")), "a depth-only instanced caster");
  assert.ok(gl.calls.some((c) => c[0] === "shaderSource" && c[2].includes("#define SKINNED\n")), "a skinned program");
  // The shadow pass (into the depth texture) before the frame.
  // (The first bind attaches the depth texture; the second draws into it.)
  const fbo = gl.calls.findLastIndex((c) => c[0] === "bindFramebuffer" && c[2] && c[2].of === "createFramebuffer");
  const back = gl.calls.findIndex((c, i) => i > fbo && c[0] === "bindFramebuffer" && c[2] === null);
  const firstDraw = calls.findIndex((c) => c.startsWith("drawElements"));
  assert.ok(fbo >= 0 && fbo < firstDraw && firstDraw < back, "casters drawn into the shadow map first");
  assert.ok(gl.calls.some((c) => c[0] === "texImage2D" && c[3] === "DEPTH_COMPONENT24"));
  assert.ok(gl.calls.some((c) => c[0] === "texParameteri" && c[2] === "TEXTURE_COMPARE_MODE" && c[3] === "COMPARE_REF_TO_TEXTURE"));
  const casterDraws = gl.calls.slice(fbo, back).filter((c) => c[0].startsWith("drawElements")).length;
  assert.equal(casterDraws, 2, "the crowd and the skinned triangle (the floor does not cast)");
  // The environment: RGBA16F half floats per level, its SH, the sky drawn first.
  const envLevels = gl.calls.filter((c) => c[0] === "texImage2D" && c[3] === "RGBA16F");
  assert.deepEqual(envLevels.map((c) => [c[2], c[4], c[5], c[8]]), [[0, 4, 2, "HALF_FLOAT"], [1, 2, 1, "HALF_FLOAT"]]);
  assert.ok(gl.calls.some((c) => c[0] === "drawArrays" && c[3] === 3), "the sky");
  assert.ok(calls.indexOf("drawArrays") < calls.lastIndexOf("drawElements"));
  assert.deepEqual(uniform(gl, "uniform1i", "u_has_env").slice(-1), [1]);
  assert.deepEqual(uniform(gl, "uniform1i", "u_has_shadow").slice(-1), [1]);
  // The bones: an RGBA32F texture, 4 texels per joint.
  assert.ok(gl.calls.some((c) => c[0] === "texImage2D" && c[3] === "RGBA32F" && c[4] === 4 && c[5] === 1));
  assert.ok(gl.calls.some((c) => c[0] === "vertexAttribIPointer" && c[1] === 4 && c[3] === "UNSIGNED_SHORT"));
  // Per-frame uniforms once per program: three programs drew the frame (plain, instanced, skinned).
  const viewProj = uniform(gl, "uniformMatrix4fv", "u_view_proj").length;
  assert.ok(viewProj <= 5, `u_view_proj set ${viewProj} times for 5 draws in 2 passes`);
  assert.equal(page.frames.length, 1, "the clip plays: another frame");
  frames(page);
  assert.deepEqual(gl.errors, [], "every vertex array keeps its element buffer");
});

test("3D: a blob that failed is asked again by the next update that refers to it", async () => {
  const page = boot();
  const blobs = store(), mesh = encodeMesh({ positions: [0, 0, 0, 1, 0, 0, 0, 1, 0], normals: [0, 0, 1, 0, 0, 1, 0, 0, 1], indices: [0, 1, 2] });
  const id = "a".repeat(32);
  const net = serve(page, blobs);
  const errors = [];
  page.window.console = { ...console, error: (...a) => errors.push(a) };
  page.ws.open();
  const scene = full({ camera: camera([0, 0, 3]) }, [{ key: "#t", name: "t", mesh: { type: "blob", id, bounds: [0, 0, 0, 1, 1, 0] } }]);
  page.ws.deliver({ t: "reset", p: [create(1, "scene3d"), sceneOp(1, scene), { op: "root", id: 1 }] });
  const canvas = page.node(1).querySelector("canvas");
  size(page, canvas, 100, 100);
  await settle();
  frames(page);
  frames(page);
  assert.equal(net.asked.length, 1, "a 404 is not asked again every frame");
  // The server has it now (the live session rendered the scene): the next update asks again.
  blobs.bytes.set(id, mesh);
  page.ws.deliver({ t: "reset", p: [create(1, "scene3d"), sceneOp(1, scene), { op: "root", id: 1 }] });
  await settle();
  frames(page);
  assert.equal(net.asked.length, 2);
  assert.equal(draws(canvas._gl), 1);
});

test("3D: a lost context draws nothing until it is restored", () => {
  const page = boot();
  page.ws.open();
  page.ws.deliver({ t: "reset", p: [create(1, "scene3d"), sceneOp(1, CUBE), { op: "root", id: 1 }] });
  const canvas = page.node(1).querySelector("canvas");
  size(page, canvas, 100, 100);
  frames(page);
  const st = canvas._scene3d, gl = canvas._gl;
  canvas.dispatchEvent({ type: "webglcontextlost", preventDefault() {} });
  assert.equal(st.gl, null);
  page.ws.deliver({ t: "patch", p: [sceneOp(1, { v: 2, upsert: [cube("#cube", { pos: [1, 0, 0] })] })] });
  assert.equal(page.frames.length, 0);
  canvas.dispatchEvent({ type: "webglcontextrestored" });
  frames(page);
  assert.equal(draws(gl), 2, "drawn again with fresh buffers");
  assert.equal(named_(gl, "createVertexArray").length, 2);
});

test("3D: without WebGL 2 the box stays empty and nothing throws", () => {
  webgl2 = false;
  try {
    const page = boot();
    page.ws.open();
    page.ws.deliver({ t: "reset", p: [create(1, "scene3d"), sceneOp(1, CUBE), { op: "root", id: 1 }] });
    const canvas = page.node(1).querySelector("canvas");
    size(page, canvas, 100, 100);
    assert.equal(page.frames.length, 0);
    pointer(canvas, "pointerdown", 50, 50);
    pointer(canvas, "pointerup", 50, 50);
    // Taps still pick (the scene is known, only not drawn).
    assert.equal(page.ws.sent.filter((m) => m.t === "scene_tap").length, 1);
    page.ws.deliver({ t: "patch", p: [{ op: "destroy", id: 1 }] });
  } finally {
    webgl2 = true;
  }
});
