#!/usr/bin/env node
// The web backend's default limits on a running app (security review S3,
// S16, S35): the counter's web server as it ships, driven over raw
// WebSockets (a client that can stop reading) and headless Chrome. No npm
// dependencies.
//
//   cargo build -p counter --no-default-features --features web --target-dir target/web
//   node scripts/web-limits.mjs [--server target/web/debug/counter] [--only s3,s16,s35] [--shots DIR]
//
// S3, sessions and messages:
// - 1,024 live sessions at most: the next handshake gets 503, and a closed
//   session frees its place;
// - a message of 8 MiB is read, one byte more ends the session;
// - a page that stops reading: its session ends once 32 MiB wait to be sent
//   (the secret note on /settings, read back: 2 MiB a round), and the
//   server's memory stays bounded;
// - a page that floods the session faster than it keeps up (32 MiB waiting);
// - a page that answers nothing (not even pings) is dropped after 75 s.
// S16, uploads (a pick on /components, a drop on /components/media):
// - 256 files in one drop are stored, 257 are refused before anything is;
// - more than 1 GiB per browser, counting the files it keeps, is refused;
//   files older than 24 hours no longer count (and are deleted);
// - more than 4 GiB being received server-wide is refused until an upload
//   ends;
// - a stored file is served only to its browser, sandboxed, never sniffed.
// S35: every response is nosniff, pages say `frame-ancestors`, and Chrome
// frames the app in its own pages only (screenshots with --shots).
//
// Nothing large is written: the big uploads are announced, held, then
// cancelled. Exit 1 if any check fails.
import { execFileSync, spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : fallback;
};
const ONLY = (opt("--only", "s3,s16,s35") || "").split(",");
const SHOTS = opt("--shots");
const STEP = 20_000;
const MiB = 1 << 20;
const GiB = 1 << 30;

const temp = fs.mkdtempSync(path.join(os.tmpdir(), "prism-limits-"));
const DATA = path.join(temp, "data");
const children = [];
let shuttingDown = false;
process.on("exit", () => {
  shuttingDown = true;
  for (const c of children) if (c.exitCode === null) c.kill("SIGKILL");
  fs.rmSync(temp, { recursive: true, force: true });
});
for (const sig of ["SIGINT", "SIGTERM"]) process.on(sig, () => process.exit(130));

const log = (...a) => console.log("web-limits:", ...a);
const results = [];
function check(name, ok, detail = "") {
  results.push({ name, ok });
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? `: ${detail}` : ""}`);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
function within(promise, what, ms = STEP) {
  let timer;
  return Promise.race([
    promise,
    new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`timed out: ${what}`)), ms); }),
  ]).finally(() => clearTimeout(timer));
}
async function until(what, test, ms = STEP) {
  const end = Date.now() + ms;
  for (;;) {
    const v = await test();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out: ${what}`);
    await sleep(50);
  }
}
const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer().listen(0, "127.0.0.1", () => {
    const { port } = s.address();
    s.close(() => resolve(port));
  });
  s.on("error", reject);
});
const rssMiB = (pid) => Math.round(Number(execFileSync("ps", ["-o", "rss=", "-p", String(pid)]).toString().trim()) / 1024);
const sid = () => crypto.randomBytes(16).toString("hex");

// --- The server -----------------------------------------------------------------------

let PORT;
let BASE;
let server;

async function startServer(bin) {
  if (!fs.existsSync(bin)) throw new Error(`${bin} not found: build it with cargo build -p counter --no-default-features --features web --target-dir target/web`);
  PORT = await freePort();
  BASE = `http://127.0.0.1:${PORT}`;
  // Room for 1,024 sockets and their sessions.
  server = spawn("/bin/sh", ["-c", 'ulimit -n 8192 2>/dev/null || ulimit -n "$(ulimit -Hn)"; exec "$0"', bin], {
    cwd: path.join(ROOT, "examples/counter"),
    env: { ...process.env, PRISM_PORT: String(PORT), PRISM_HOST: "127.0.0.1", PRISM_DATA_DIR: DATA, PRISM_LOCALE: "en" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  children.push(server);
  server.stderr.on("data", (d) => process.stderr.write(`[server] ${d}`));
  server.on("exit", (code, sig) => {
    if (!shuttingDown) { console.error(`web-limits: FAIL: the server exited (${code ?? sig})`); process.exit(1); }
  });
  await until("the server answers", async () => {
    try { return (await fetch(BASE + "/")).ok; } catch { return false; }
  }, 60_000);
}

// --- A raw WebSocket client: it can stop reading ------------------------------------

const socketPath = (page) => `/ws?path=${encodeURIComponent(page)}&tz=0&w=1280&h=900`;

/** Opens Prism's socket for `page`: `{status}`, and `ws` once upgraded (101). */
function openSocket(page, { cookie, parse = true } = {}) {
  return new Promise((resolve, reject) => {
    const sock = net.connect(PORT, "127.0.0.1");
    let head = Buffer.alloc(0);
    sock.once("error", reject);
    sock.on("connect", () => {
      const lines = [
        `GET ${socketPath(page)} HTTP/1.1`, `Host: 127.0.0.1:${PORT}`, "Upgrade: websocket", "Connection: Upgrade",
        `Sec-WebSocket-Key: ${crypto.randomBytes(16).toString("base64")}`, "Sec-WebSocket-Version: 13",
      ];
      if (cookie) lines.push(`Cookie: prism_sid=${cookie}`);
      sock.write(lines.join("\r\n") + "\r\n\r\n");
    });
    const onData = (d) => {
      head = Buffer.concat([head, d]);
      const end = head.indexOf("\r\n\r\n");
      if (end < 0) return;
      sock.off("data", onData);
      sock.off("error", reject);
      const status = Number(head.subarray(0, end).toString().split(" ")[1]);
      if (status !== 101) {
        sock.destroy();
        resolve({ status });
      } else {
        resolve({ status, ws: new Ws(sock, head.subarray(end + 4), parse) });
      }
    };
    sock.on("data", onData);
  });
}

class Ws {
  constructor(sock, rest, parse) {
    this.sock = sock;
    this.parse = parse;
    this.buf = Buffer.alloc(0);
    this.bytes = 0;
    this.messages = [];
    this.waiters = [];
    this.closeCode = null;
    this.ended = false;
    this.closed = new Promise((resolve) => sock.on("close", () => { this.ended = true; resolve(this.closeCode); }));
    sock.on("error", () => {});
    sock.on("data", (d) => this.feed(d));
    if (rest.length) this.feed(rest);
  }

  feed(d) {
    this.bytes += d.length;
    if (!this.parse) return;
    this.buf = this.buf.length ? Buffer.concat([this.buf, d]) : d;
    for (;;) {
      const b = this.buf;
      if (b.length < 2) return;
      const op = b[0] & 15;
      let len = b[1] & 127;
      let at = 2;
      if (len === 126) {
        if (b.length < 4) return;
        len = b.readUInt16BE(2);
        at = 4;
      } else if (len === 127) {
        if (b.length < 10) return;
        len = Number(b.readBigUInt64BE(2));
        at = 10;
      }
      if (b.length < at + len) return;
      const payload = b.subarray(at, at + len);
      this.buf = b.subarray(at + len);
      if (op === 1) this.text(payload.toString());
      else if (op === 8) this.closeCode = len >= 2 ? payload.readUInt16BE(0) : 1005;
      else if (op === 9) this.frame(10, payload); // pong
    }
  }

  text(t) {
    let m;
    try { m = JSON.parse(t); } catch { m = {}; }
    m._text = t;
    this.messages.push(m);
    this.waiters = this.waiters.filter((w) => {
      if (!w.test(m)) return true;
      w.resolve(m);
      return false;
    });
  }

  /** The first message from index `from` on that passes `test`. */
  next(test, what, from = 0, ms = STEP) {
    const hit = this.messages.slice(from).find(test);
    if (hit) return Promise.resolve(hit);
    return within(new Promise((resolve) => this.waiters.push({ test, resolve })), what, ms);
  }

  /** A masked frame (a zero mask: the server does not care, and 8 MiB need no XOR). */
  frame(op, payload) {
    const len = payload.length;
    let head;
    if (len < 126) {
      head = Buffer.alloc(6);
      head[1] = 0x80 | len;
    } else if (len < 65536) {
      head = Buffer.alloc(8);
      head[1] = 0x80 | 126;
      head.writeUInt16BE(len, 2);
    } else {
      head = Buffer.alloc(14);
      head[1] = 0x80 | 127;
      head.writeBigUInt64BE(BigInt(len), 2);
    }
    head[0] = 0x80 | op;
    if (this.sock.destroyed) return false;
    this.sock.write(head);
    return this.sock.write(payload);
  }

  send(msg) {
    return this.frame(1, Buffer.from(typeof msg === "string" ? msg : JSON.stringify(msg)));
  }

  close() { this.sock.destroy(); }
}

const isFrame = (m) => Array.isArray(m.p);
const texts = (msgs) => msgs.flatMap((m) => (m.p || []).filter((o) => o.op === "text").map((o) => o.text));

/** The clickable node holding the text `label`, from the page's frames. */
function clickable(ws, label) {
  const parent = new Map();
  const click = new Set();
  let node;
  for (const m of ws.messages.filter(isFrame)) {
    for (const o of m.p) {
      if (o.op === "children") for (const c of o.children) parent.set(c, o.id);
      else if (o.op === "clickable" && o.on) click.add(o.id);
      else if (o.op === "text" && o.text === label) node = o.id;
    }
  }
  while (node !== undefined && !click.has(node)) node = parent.get(node);
  if (node === undefined) throw new Error(`nothing clickable labelled ${label}`);
  return node;
}

/** An open session on `page`, its first frame received. */
async function session(page, opts) {
  const r = await openSocket(page, opts);
  if (r.status !== 101) throw new Error(`socket for ${page}: ${r.status}`);
  if (r.ws.parse) await r.ws.next(isFrame, `${page}: first frame`);
  else await until(`${page}: first frame`, () => r.ws.bytes > 0);
  return r.ws;
}

// --- S3 -------------------------------------------------------------------------------

async function s3Sessions() {
  const open = [];
  const statuses = {};
  while (open.length < 1100 && !statuses[503]) {
    const batch = await Promise.all(Array.from({ length: 32 }, () => openSocket("/", { parse: false })));
    for (const r of batch) {
      statuses[r.status] = (statuses[r.status] || 0) + 1;
      if (r.ws) open.push(r.ws);
    }
  }
  const extra = await openSocket("/", { parse: false });
  check("S3 sessions: 1,024 handshakes upgraded, then 503", open.length === 1024 && extra.status === 503,
    `${open.length} upgraded, statuses ${JSON.stringify(statuses)}, one more: ${extra.status}, server ${rssMiB(server.pid)} MiB`);
  open.pop().close();
  let again;
  await until("a place frees", async () => {
    again = await openSocket("/", { parse: false });
    if (again.ws) return true;
    await sleep(150);
    return false;
  }, 10_000).catch(() => {});
  check("S3 sessions: a closed session frees its place", again?.status === 101, `next handshake: ${again?.status}`);
  again?.ws?.close();
  for (const ws of open) ws.close();
  // Every session gone before the next test counts on room.
  await until("the sessions end", async () => (await openSocket("/", { parse: false }).then((r) => { r.ws?.close(); return r.status === 101; })), 30_000);
  await sleep(1000);
}

/** A message exactly `n` bytes long that the session reads and ignores. */
const padded = (n) => JSON.stringify({ t: "pad", x: "x".repeat(n - 18) });

async function s3MessageSize() {
  const ws = await session("/");
  ws.send(padded(8 * MiB));
  const from = ws.messages.length;
  ws.send({ t: "link", path: "/settings" });
  const after = await ws.next(isFrame, "a frame after the 8 MiB message", from).catch(() => null);
  check("S3 message size: 8 MiB read, the session goes on", !!after && !ws.ended, after ? `next frame ${after._text.length} bytes` : "no frame");
  ws.send(padded(8 * MiB + 1));
  const code = await within(ws.closed, "the socket closes", 10_000).catch(() => "open");
  check("S3 message size: 8 MiB + 1 byte ends the session", code !== "open", `socket closed (close code ${code ?? "none"})`);
}

async function s3SlowReader() {
  // The secret note on /settings, typed, saved and read back: about 2 MiB
  // of frames per round (the field's value, then « Your note: … »).
  const ws = await session("/settings");
  const field = ws.messages.filter(isFrame).flatMap((m) => m.p).find((o) => o.op === "placeholder" && o.text.startsWith("A token"))?.id;
  if (field === undefined) throw new Error("no note field on /settings");
  ws.send({ t: "input", id: field, v: "x" });
  await sleep(800);
  const save = clickable(ws, "Save");
  const read = clickable(ws, "Read");
  ws.parse = false;
  ws.sock.pause();
  const rss0 = rssMiB(server.pid);
  let peak = rss0;
  let rounds = 0;
  const start = Date.now();
  for (; rounds < 64 && !ws.ended; rounds++) {
    ws.send({ t: "input", id: field, v: String.fromCharCode(97 + (rounds % 26)).repeat(MiB) });
    ws.send({ t: "click", id: save, x: 2, y: 2 });
    ws.send({ t: "click", id: read, x: 2, y: 2 });
    await sleep(300);
    peak = Math.max(peak, rssMiB(server.pid));
  }
  await until("the session ends", () => ws.ended, 15_000).catch(() => {});
  peak = Math.max(peak, rssMiB(server.pid));
  check("S3 slow reader: its session ends once 32 MiB wait to be sent", ws.ended && rounds < 64,
    `${ws.ended ? "ended" : "still open"} after ${rounds} rounds of about 2 MiB (${((Date.now() - start) / 1000).toFixed(1)} s) while the page read nothing; `
    + `server ${rss0} MiB before, ${peak} MiB at most`);
  ws.close();
}

async function s3Flood() {
  const ws = await session("/", { parse: false });
  // 1 MiB messages that each make the session render a page: faster than
  // it keeps up.
  const pad = "x".repeat(MiB);
  const n = 128;
  let sent = 0;
  for (; sent < n && !ws.ended; sent++) {
    ws.send(JSON.stringify({ t: "link", path: sent % 2 ? "/" : "/components", pad }));
  }
  const code = await within(ws.closed, "the socket closes", 30_000).catch(() => "open");
  check("S3 flood: the session ends once 32 MiB wait to be handled", code !== "open",
    `${sent} messages of 1 MiB sent, socket ${code === "open" ? "still open" : "closed"}`);
  ws.close();
}

/** Started early, checked at the end: a page that never answers. */
async function s3Silent() {
  const ws = await session("/", { parse: false });
  ws.sock.pause();
  const start = Date.now();
  return async () => {
    ws.sock.resume();
    const code = await within(ws.closed, "the silent socket closes", Math.max(1000, 95_000 - (Date.now() - start))).catch(() => "open");
    const s = Math.round((Date.now() - start) / 1000);
    check("S3 silent page: dropped (75 s with no pong or message)", code !== "open", `${code === "open" ? "still open" : "closed"} after ${s} s at most`);
    ws.close();
  };
}

// --- S16 ------------------------------------------------------------------------------

const filesOf = (s) => {
  const dir = path.join(DATA, s, "files");
  return fs.existsSync(dir) ? fs.readdirSync(dir) : [];
};

/** Drops `n` files of one byte on the media page's any-file target. */
async function dropFiles(n) {
  const s = sid();
  const ws = await session("/components/media", { cookie: s });
  const node = ws.messages.filter(isFrame).flatMap((m) => m.p).find((o) => o.op === "drop" && o.types.length === 0)?.id;
  if (node === undefined) throw new Error("no drop target");
  const files = Array.from({ length: n }, (_, i) => ({ name: `f${i}.txt`, mime: "text/plain", size: 1 }));
  const from = ws.messages.length;
  ws.send({ t: "drop_start", drop: 1, node, files });
  for (let i = 0; i < n; i++) ws.send({ t: "upload_chunk", drop: 1, file: i, data: "YQ==" });
  ws.send({ t: "drop_end", drop: 1 });
  const shown = await ws.next((m) => texts([m]).some((t) => t.includes("f1.txt")), "the dropped files listed", from, 5000).catch(() => null);
  ws.close();
  return { listed: !!shown, stored: filesOf(s).length };
}

async function s16Count() {
  const ok = await dropFiles(256);
  check("S16 files: a drop of 256 files is stored", ok.listed && ok.stored === 256, `listed: ${ok.listed}, ${ok.stored} files stored`);
  const no = await dropFiles(257);
  check("S16 files: a drop of 257 files is refused, nothing stored", !no.listed && no.stored === 0, `listed: ${no.listed}, ${no.stored} files stored`);
}

/** The device demo's photo picker on /components (three at most), in the
 * session `ws`: refused uploads show why under it. */
async function pick(ws, files, chunks) {
  const button = clickable(ws, "Pick photos");
  let from = ws.messages.length;
  ws.send({ t: "click", id: button, x: 4, y: 4 });
  const call = await ws.next((m) => m.t === "device" && m.call && m.call !== "stop", "the pick call", from);
  from = ws.messages.length;
  ws.send({ t: "upload_start", id: call.id, files });
  const stop = await ws.next((m) => m.t === "device" && m.id === call.id && m.call === "stop", "a stop", from, 1500).catch(() => null);
  if (stop) {
    // The page stops and fails the call, as client.js does; the app shows why.
    from = ws.messages.length;
    ws.send({ t: "device_reply", id: call.id, err: "cancelled" });
    const why = await ws.next((m) => texts([m]).some((t) => /at most|too many|too large|receiving/.test(t)), "the reason shown", from, 5000).catch(() => null);
    return { refused: true, why: why ? texts([why]).find((t) => /at most|too many|too large|receiving/.test(t)) : "(not shown)" };
  }
  if (chunks) {
    for (const c of chunks) ws.send({ t: "upload_chunk", id: call.id, file: 0, data: c });
    ws.send({ t: "device_reply", id: call.id, uploaded: true });
    await sleep(500);
  }
  return { refused: false, id: call.id, cancel: () => ws.send({ t: "device_reply", id: call.id, err: "cancelled" }) };
}

const image = (size) => [{ name: "big.png", mime: "image/png", size }];

async function s16Bytes() {
  // Three files at most in this picker (the app's limit): the fourth is refused.
  {
    const ws = await session("/components");
    const r = await pick(ws, Array.from({ length: 4 }, (_, i) => ({ name: `p${i}.png`, mime: "image/png", size: 1 })));
    check("S16 files: a pick past the app's limit (3) is refused", r.refused, r.why);
    ws.close();
  }
  const b = sid();
  const ws = await session("/components", { cookie: b });
  // 1 MiB kept by this browser.
  const chunk = Buffer.alloc(256 * 1024, 7).toString("base64");
  const kept = await pick(ws, [{ name: "kept.png", mime: "image/png", size: MiB }], [chunk, chunk, chunk, chunk]);
  const stored = filesOf(b);
  check("S16 bytes: a 1 MiB upload is stored", !kept.refused && stored.length === 1 && fs.statSync(path.join(DATA, b, "files", stored[0])).size === MiB, stored.join(", "));
  await s16Served(b, stored[0]);

  const over = await pick(ws, image(GiB - MiB + 1));
  check("S16 bytes: 1 GiB per browser, counting what it keeps (1 MiB + 1 GiB - 1 MiB + 1)", over.refused, over.why);
  const fits = await pick(ws, image(GiB - MiB));
  check("S16 bytes: exactly 1 GiB with what it keeps is taken", !fits.refused);
  fits.cancel?.();
  await sleep(300);
  // The kept file, 25 hours old: expired, it no longer counts.
  const file = path.join(DATA, b, "files", stored[0]);
  const old = new Date(Date.now() - 25 * 3600 * 1000);
  fs.utimesSync(file, old, old);
  const full = await pick(ws, image(GiB));
  check("S16 kept 24 h: an expired file no longer counts, and is deleted", !full.refused && !fs.existsSync(file), `1 GiB pick ${full.refused ? "refused" : "taken"}, old file ${fs.existsSync(file) ? "still there" : "deleted"}`);
  full.cancel?.();
  await sleep(300);
  check("S16 bytes: a cancelled upload leaves nothing", filesOf(b).length === 0, filesOf(b).join(", "));
  ws.close();
  // A new browser, so the reason shown is new too.
  const fresh = await session("/components");
  const huge = await pick(fresh, image(GiB + 1));
  check("S16 bytes: 1 GiB + 1 in one pick is refused", huge.refused, huge.why);
  fresh.close();
}

async function s16Served(owner, name) {
  const url = `${BASE}/files/${name}`;
  const mine = await fetch(url, { headers: { Cookie: `prism_sid=${owner}` } });
  const theirs = await fetch(url, { headers: { Cookie: `prism_sid=${sid()}` } });
  const anon = await fetch(url);
  const h = mine.headers;
  check("S16 files: served to their browser only, nosniff and sandboxed",
    mine.status === 200 && theirs.status === 404 && anon.status === 404 && h.get("x-content-type-options") === "nosniff" && /sandbox/.test(h.get("content-security-policy") || ""),
    `owner ${mine.status}, another browser ${theirs.status}, no cookie ${anon.status}; ${h.get("x-content-type-options")}; CSP ${h.get("content-security-policy")}`);
}

async function s16ServerWide() {
  // Four browsers announce 1 GiB each and hold: 4 GiB being received.
  const held = [];
  for (let i = 0; i < 4; i++) {
    const ws = await session("/components");
    const r = await pick(ws, image(GiB));
    held.push({ ws, r });
  }
  check("S16 server-wide: four browsers receiving 1 GiB each", held.every((h) => !h.r.refused));
  const fifth = await session("/components");
  const no = await pick(fifth, image(1));
  check("S16 server-wide: a fifth upload (1 byte) is refused past 4 GiB", no.refused, no.why);
  held[0].r.cancel?.();
  await sleep(300);
  const yes = await pick(fifth, image(1));
  check("S16 server-wide: taken again once an upload ends", !yes.refused);
  yes.cancel?.();
  for (const h of held) { h.r.cancel?.(); h.ws.close(); }
  fifth.close();
}

// --- S35 ------------------------------------------------------------------------------

async function s35Headers() {
  const html = await (await fetch(BASE + "/")).text();
  const script = html.match(/src="(\/_prism\/client\.[^"]+\.js)"/)?.[1];
  for (const p of ["/", "/settings", "/components/media", script, "/favicon.ico", "/no/such/page", "/assets/../Cargo.toml"].filter(Boolean)) {
    const r = await fetch(BASE + p);
    const h = r.headers;
    const isHtml = (h.get("content-type") || "").startsWith("text/html");
    const csp = h.get("content-security-policy") || "";
    const ok = h.get("x-content-type-options") === "nosniff" && (!isHtml || (csp.includes(`frame-ancestors ${BASE}`) || csp.includes("frame-ancestors 'self'")) && h.get("x-frame-options") === "SAMEORIGIN");
    check(`S35 headers ${p}`, ok, `${r.status} ${h.get("content-type") || "-"}; nosniff: ${h.get("x-content-type-options")}; ${isHtml ? `frame-ancestors: ${csp.match(/frame-ancestors[^;,]*/)?.[0]}; X-Frame-Options: ${h.get("x-frame-options")}` : ""}`);
  }
}

function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  const pw = path.join(os.homedir(), "Library/Caches/ms-playwright");
  const candidates = [];
  if (fs.existsSync(pw)) {
    for (const dir of fs.readdirSync(pw).filter((d) => d.startsWith("chromium-")).sort().reverse()) {
      candidates.push(path.join(pw, dir, "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"));
      candidates.push(path.join(pw, dir, "chrome-mac/Chromium.app/Contents/MacOS/Chromium"));
      candidates.push(path.join(pw, dir, "chrome-linux/chrome"));
    }
  }
  candidates.push("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
  for (const name of ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"]) {
    for (const dir of (process.env.PATH || "").split(path.delimiter)) candidates.push(path.join(dir, name));
  }
  return candidates.find((c) => fs.existsSync(c));
}

async function devtools() {
  const bin = findChrome();
  if (!bin) throw new Error("no Chrome found: set CHROME");
  const port = await freePort();
  const flags = [
    "--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${path.join(temp, "chrome")}`,
    "--no-first-run", "--no-default-browser-check", "--disable-gpu", "--disable-extensions",
    "--disable-background-networking", "--disable-component-update", "--window-size=1100,800",
    "--no-proxy-server", "--use-mock-keychain", "--password-store=basic",
  ];
  if (process.platform === "linux") flags.push("--no-sandbox", "--disable-dev-shm-usage");
  const chrome = spawn(bin, [...flags, "about:blank"], { stdio: "ignore" });
  children.push(chrome);
  const page = await until("Chrome's DevTools endpoint", async () => {
    try {
      return (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === "page");
    } catch { return null; }
  }, 30_000);
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await within(new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; }), "DevTools socket");
  let id = 0;
  const pending = new Map();
  const events = [];
  ws.onmessage = (e) => {
    const m = JSON.parse(e.data);
    if (m.id !== undefined) {
      const p = pending.get(m.id);
      pending.delete(m.id);
      m.error ? p.reject(new Error(m.error.message)) : p.resolve(m.result);
    } else {
      events.push(m);
    }
  };
  const send = (method, params = {}) => within(new Promise((resolve, reject) => {
    pending.set(++id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  }), method);
  return { send, events, bin, close: () => { ws.close(); chrome.kill("SIGKILL"); } };
}

async function s35Framing() {
  // Another origin (another port) whose page frames the app.
  const otherPort = await freePort();
  const other = http.createServer((req, res) => {
    res.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
    res.end(`<!doctype html><body style="font:16px system-ui;margin:12px;background:#fde8e8"><p>Another site (${"127.0.0.1:" + otherPort}) framing ${BASE}/settings:</p><iframe id="f" src="${BASE}/settings" style="width:96%;height:560px;border:2px dashed #c33"></iframe></body>`);
  }).listen(otherPort, "127.0.0.1");
  const cdp = await devtools();
  log(`chrome: ${cdp.bin}`);
  await cdp.send("Page.enable");
  await cdp.send("Log.enable");
  await cdp.send("Runtime.enable");
  const load = async (url) => {
    const n = cdp.events.length;
    await cdp.send("Page.navigate", { url });
    await until(`${url} loads`, () => cdp.events.slice(n).some((e) => e.method === "Page.loadEventFired"));
  };
  const eval_ = async (expression) => (await cdp.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })).result.value;
  const shot = async (name) => {
    if (!SHOTS) return;
    const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
    fs.mkdirSync(SHOTS, { recursive: true });
    fs.writeFileSync(path.join(SHOTS, name), Buffer.from(data, "base64"));
    log(`screenshot ${path.join(SHOTS, name)}`);
  };

  // Framed by its own page: shown.
  await load(BASE + "/");
  const same = await eval_(`new Promise((resolve) => {
    const f = document.createElement("iframe");
    f.src = "/settings";
    f.style.cssText = "position:fixed;right:12px;bottom:12px;width:520px;height:420px;border:3px dashed #3a3;background:#fff;z-index:99999";
    f.onload = () => setTimeout(() => {
      let doc = null;
      try { doc = f.contentDocument; } catch (e) {}
      resolve({ href: doc ? doc.location.href : null, text: doc && doc.body ? doc.body.innerText.length : 0 });
    }, 2500);
    document.body.appendChild(f);
  })`);
  check("S35 framing: the app's own page frames it", !!same?.href && same.href.endsWith("/settings") && same.text > 0, JSON.stringify(same));
  await shot("web-s35-same-origin-frame.png");

  // Framed by another site: refused.
  const n = cdp.events.length;
  await load(`http://127.0.0.1:${otherPort}/`);
  await sleep(2500);
  const tree = await cdp.send("Page.getFrameTree");
  const child = tree.frameTree.childFrames?.[0]?.frame;
  const refusal = cdp.events.slice(n).map((e) => e.params?.entry?.text || e.params?.args?.map((a) => a.value).join(" ") || "").find((t) => /frame-ancestors|Refused to (display|frame)/i.test(t));
  // Chrome blocks it (an error page, out of the frame tree with site
  // isolation) and says why in the console.
  const blocked = !child || !!child.unreachableUrl || child.url.startsWith("chrome-error:");
  check("S35 framing: another site's frame is refused", blocked && !!refusal,
    `child frame ${child ? `${child.url} (unreachable ${child.unreachableUrl || "-"})` : "not loaded"}; console: ${refusal || "(nothing)"}`);
  await shot("web-s35-cross-origin-frame.png");
  cdp.close();
  other.close();
}

// --- Main -----------------------------------------------------------------------------

async function main() {
  if (typeof WebSocket !== "function") throw new Error(`Node ${process.version} has no WebSocket: use Node 22+`);
  await startServer(path.resolve(ROOT, opt("--server", "target/web/debug/counter")));
  log(`server ${BASE} (pid ${server.pid}), defaults: 1,024 sessions, 8 MiB messages, 32 MiB queued, 256 files, 1 GiB per browser, 4 GiB server-wide, 24 h`);
  // `--only s3SlowReader,s16Count`: those steps alone.
  const named = { s3Sessions, s3MessageSize, s3SlowReader, s3Flood, s16Count, s16Bytes, s16ServerWide, s35Headers, s35Framing };
  const steps = ONLY.filter((n) => named[n]).map((n) => named[n]);
  if (ONLY.includes("s35")) steps.push(s35Headers);
  if (ONLY.includes("s3")) steps.push(s3Sessions);
  let silent;
  if (ONLY.includes("s3")) steps.push(async () => { silent = await s3Silent(); }, s3MessageSize, s3SlowReader, s3Flood);
  if (ONLY.includes("s16")) steps.push(s16Count, s16Bytes, s16ServerWide);
  if (ONLY.includes("s35")) steps.push(s35Framing);
  for (const step of steps) {
    try {
      await step();
    } catch (e) {
      check(step.name || "step", false, e.message);
    }
  }
  if (silent) await silent();
  log(`server ${rssMiB(server.pid)} MiB at the end`);
  const failed = results.filter((r) => !r.ok);
  if (failed.length) {
    console.error(`web-limits: FAIL: ${failed.length} of ${results.length} checks`);
    process.exit(1);
  }
  log(`OK: ${results.length} checks`);
  process.exit(0);
}

main().catch((e) => {
  console.error(`web-limits: FAIL: ${e.stack || e}`);
  process.exit(1);
});
