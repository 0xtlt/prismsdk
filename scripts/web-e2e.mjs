#!/usr/bin/env node
// End-to-end test of the web backend in headless Chromium, with no npm
// dependencies: Node 22+'s built-in WebSocket speaks the DevTools protocol.
//
// For each tab of the counter example it loads the page, waits for the live
// session (the "reset" frame on Prism's socket), checks the right tab is
// selected and the page has content; on the home tab it clicks "+1" with a
// real mouse event and waits for the counter to change through a "patch"
// frame. Any uncaught exception, console error or failed request fails it.
//
//   cargo build -p counter --no-default-features --features web --target-dir target/web
//   node scripts/web-e2e.mjs [--server target/web/debug/counter] [--url http://127.0.0.1:3000]
//
// Env: CHROME (browser binary), PRISM_E2E_TIMEOUT (seconds per step, default 20).
import { spawn } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import zlib from "node:zlib";

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : fallback;
};
const STEP = Number(process.env.PRISM_E2E_TIMEOUT || 20) * 1000;
const TABS = [
  { path: "/", index: 0 },
  { path: "/focus", index: 1 },
  { path: "/components", index: 2 },
  { path: "/settings", index: 3 },
];

const children = [];
const temp = fs.mkdtempSync(path.join(os.tmpdir(), "prism-e2e-"));
function cleanup() {
  for (const c of children) if (c.exitCode === null) c.kill("SIGKILL");
  fs.rmSync(temp, { recursive: true, force: true });
}
process.on("exit", cleanup);
for (const sig of ["SIGINT", "SIGTERM"]) process.on(sig, () => process.exit(130));

const log = (...a) => console.log("web-e2e:", ...a);
function fail(message) {
  console.error(`web-e2e: FAIL: ${message}`);
  process.exit(1);
}

/** Resolves `promise`, or fails after `ms` with `what`. */
function within(promise, what, ms = STEP) {
  let timer;
  return Promise.race([
    promise,
    new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`timed out: ${what}`)), ms); }),
  ]).finally(() => clearTimeout(timer));
}

async function until(what, check, ms = STEP) {
  const end = Date.now() + ms;
  for (;;) {
    const v = await check();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out: ${what}`);
    await new Promise((r) => setTimeout(r, 50));
  }
}

const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer().listen(0, "127.0.0.1", () => {
    const { port } = s.address();
    s.close(() => resolve(port));
  });
  s.on("error", reject);
});

function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  const candidates = [];
  const pw = path.join(os.homedir(), "Library/Caches/ms-playwright");
  if (fs.existsSync(pw)) {
    for (const dir of fs.readdirSync(pw).filter((d) => d.startsWith("chromium-")).sort().reverse()) {
      candidates.push(path.join(pw, dir, "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"));
      candidates.push(path.join(pw, dir, "chrome-mac/Chromium.app/Contents/MacOS/Chromium"));
    }
  }
  candidates.push("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
  for (const name of ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome"]) {
    for (const dir of (process.env.PATH || "").split(path.delimiter)) candidates.push(path.join(dir, name));
  }
  const found = candidates.find((c) => fs.existsSync(c));
  if (!found) fail("no Chrome/Chromium found: set CHROME");
  return found;
}

/** Starts the counter's web server on a free port; resolves its base URL. */
async function startServer(bin) {
  if (!fs.existsSync(bin)) fail(`${bin} not found: build it with cargo build -p counter --no-default-features --features web --target-dir target/web`);
  const port = await freePort();
  const server = spawn(bin, [], {
    cwd: path.join(ROOT, "examples/counter"),
    env: { ...process.env, PRISM_PORT: String(port), PRISM_HOST: "127.0.0.1", PRISM_DATA_DIR: path.join(temp, "data"), PRISM_LOCALE: "en" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  children.push(server);
  server.stderr.on("data", (d) => process.stderr.write(`[server] ${d}`));
  server.on("exit", (code, sig) => { if (!shuttingDown) fail(`the server exited (${code ?? sig})`); });
  const url = `http://127.0.0.1:${port}`;
  await until("the server answers", async () => {
    try { return (await fetch(url + "/")).ok; } catch { return false; }
  }, 60_000);
  return url;
}
let shuttingDown = false;

/** Launches headless Chrome; resolves the DevTools socket of its first page. */
async function startChrome() {
  const bin = findChrome();
  const port = await freePort();
  const flags = [
    "--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${path.join(temp, "chrome")}`,
    "--no-first-run", "--no-default-browser-check", "--disable-gpu", "--disable-extensions",
    "--disable-background-networking", "--disable-component-update", "--window-size=1280,800",
    // Straight to the local server, whatever the system proxy settings.
    "--no-proxy-server",
    // No "Chromium Safe Storage" keychain prompt on macOS.
    "--use-mock-keychain", "--password-store=basic",
  ];
  // Containers and CI runners without user namespaces.
  if (process.platform === "linux") flags.push("--no-sandbox", "--disable-dev-shm-usage");
  const chrome = spawn(bin, [...flags, "about:blank"], { stdio: ["ignore", "ignore", "pipe"] });
  children.push(chrome);
  let stderr = "";
  chrome.stderr.on("data", (d) => { stderr += d; });
  chrome.on("exit", (code, sig) => { if (!shuttingDown) fail(`Chrome exited (${code ?? sig}):\n${stderr}`); });
  const page = await until("Chrome's DevTools endpoint", async () => {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      return list.find((t) => t.type === "page");
    } catch { return null; }
  }, 30_000);
  log(`chrome: ${bin}`);
  return page.webSocketDebuggerUrl;
}

/** A DevTools protocol session over Node's built-in WebSocket. */
async function devtools(wsUrl) {
  const ws = new WebSocket(wsUrl);
  await within(new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; }), "DevTools socket");
  let nextId = 0;
  const pending = new Map();
  const listeners = [];
  ws.onmessage = (e) => {
    const msg = JSON.parse(e.data);
    if (msg.id !== undefined) {
      const p = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) p.reject(new Error(`${p.method}: ${msg.error.message}`));
      else p.resolve(msg.result);
    } else {
      for (const fn of listeners) fn(msg.method, msg.params);
    }
  };
  const send = (method, params = {}) => within(new Promise((resolve, reject) => {
    const id = ++nextId;
    pending.set(id, { resolve, reject, method });
    ws.send(JSON.stringify({ id, method, params }));
  }), method);
  return { send, on: (fn) => listeners.push(fn), close: () => ws.close() };
}

async function main() {
  if (typeof WebSocket !== "function") fail(`Node ${process.version} has no WebSocket: use Node 22+`);
  const base = opt("--url") || await startServer(path.resolve(ROOT, opt("--server", "target/web/debug/counter")));
  log(`server: ${base}`);
  const cdp = await devtools(await startChrome());

  // Everything that goes wrong in the page, and Prism's socket traffic.
  const errors = [];
  const frames = { sent: [], received: [] };
  cdp.on((method, p) => {
    if (method === "Runtime.exceptionThrown") {
      const d = p.exceptionDetails;
      errors.push(`exception: ${d.exception?.description || d.text} (${d.url || ""}:${d.lineNumber})`);
    } else if (method === "Runtime.consoleAPICalled" && (p.type === "error" || p.type === "assert")) {
      errors.push(`console.${p.type}: ${p.args.map((a) => a.value ?? a.description).join(" ")}`);
    } else if (method === "Log.entryAdded" && p.entry.level === "error") {
      // The server has no favicon (404 by design); anything else counts.
      if (/\/favicon\.ico$/.test(p.entry.url || "")) return;
      errors.push(`log: ${p.entry.text} ${p.entry.url || ""}`);
    } else if (method === "Network.webSocketFrameSent") {
      frames.sent.push(JSON.parse(p.response.payloadData));
    } else if (method === "Network.webSocketFrameReceived") {
      // Large messages come as binary frames of raw DEFLATE (base64 here).
      const r = p.response;
      const text = r.opcode === 2 ? zlib.inflateRawSync(Buffer.from(r.payloadData, "base64")).toString() : r.payloadData;
      frames.received.push(JSON.parse(text));
    }
  });
  await cdp.send("Runtime.enable");
  await cdp.send("Log.enable");
  await cdp.send("Network.enable");
  await cdp.send("Page.enable");

  const evaluate = async (expression) => {
    const r = await cdp.send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(`evaluate: ${r.exceptionDetails.exception?.description || r.exceptionDetails.text}`);
    return r.result.value;
  };

  for (const tab of TABS) {
    frames.sent.length = 0;
    frames.received.length = 0;
    const url = base + tab.path;
    const nav = await cdp.send("Page.navigate", { url });
    if (nav.errorText) fail(`${url}: ${nav.errorText}`);
    // The live session started: the server sent the page's tree.
    await until(`${tab.path}: the session's reset`, () => frames.received.some((m) => m.t === "reset"));
    const state = await until(`${tab.path}: the page shows tab ${tab.index}`, () => evaluate(`(() => {
      const tabs = [...document.querySelectorAll(".prism-tabbar [role=tab]")];
      const selected = tabs.findIndex((t) => t.getAttribute("aria-selected") === "true");
      const root = document.getElementById("prism-root");
      const panel = root.querySelector("[role=tabpanel]:not([hidden])");
      return tabs.length === ${TABS.length} && selected === ${tab.index} && panel && panel.textContent.trim().length > 0
        ? { selected, title: document.title, text: panel.textContent.trim().slice(0, 60), path: location.pathname }
        : null;
    })()`));
    if (state.path !== tab.path) fail(`${tab.path}: the URL became ${state.path}`);
    log(`${tab.path}: tab ${state.selected} "${state.title}" — ${JSON.stringify(state.text)}`);

    if (tab.path === "/") await clickPlusOne(cdp, evaluate, frames);
    if (tab.path === "/components") await richTextEditor(cdp, evaluate);
  }

  // Switching tabs by clicking the tab bar, live (we are on the last tab).
  frames.sent.length = 0;
  const box = await evaluate(`(() => {
    const r = document.querySelectorAll(".prism-tabbar [role=tab]")[0].getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  })()`);
  await click(cdp, box);
  await until("a tab click is sent as a change", () => frames.sent.some((m) => m.t === "change" && m.v === 0));
  await until("the URL follows the selected tab", () => evaluate("location.pathname === '/'"));
  await until("the home panel is shown", () => evaluate(
    `!document.querySelectorAll("#prism-root [role=tabpanel]")[0].hidden`));
  log("tab bar: click → change → /");

  await wheelOverHorizontalRow(cdp, evaluate);

  // Let late errors (async handlers) surface.
  await new Promise((r) => setTimeout(r, 300));
  if (errors.length) fail(`errors in the page:\n  ${errors.join("\n  ")}`);
  shuttingDown = true;
  cdp.close();
  log("ok");
  process.exit(0);
}

async function click(cdp, { x, y }) {
  for (const type of ["mousePressed", "mouseReleased"]) {
    await cdp.send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1 });
  }
}

/**
 * The rich text editor of the Forms card (#294), typed into like a person
 * would: Enter splits a block and moves the caret to the new one, ⌘B (Ctrl+B)
 * bolds the selection, Backspace at a block's start joins it to the one
 * before, "- " starts a list item, ⌘K links the selection (the example answers
 * with a link at once). Its Markdown, shown under it, follows each edit.
 */
async function richTextEditor(cdp, evaluate) {
  const ED = `[...document.querySelectorAll("[role=group]")].find((g) => (g.getAttribute("aria-label") || "").startsWith("Rich text"))`;
  const FIELDS = `[...(${ED}).querySelectorAll("[contenteditable=true]")]`;
  await until("the rich text editor", () => evaluate(`!!(${ED})`));
  const markdown = () => evaluate(`(() => {
    const texts = [...(${ED}).parentElement.querySelectorAll("*")].map((e) => e.textContent);
    return texts.find((t) => t.startsWith("## ")) || "";
  })()`);
  const focused = () => evaluate(`${FIELDS}.indexOf(document.activeElement)`);
  const caretIn = (i, at) => evaluate(`(() => {
    const f = ${FIELDS}[${i}];
    f.scrollIntoView({ block: "center" });
    f.focus();
    const off = ${at} < 0 ? f.textContent.length : ${at};
    const w = document.createTreeWalker(f, NodeFilter.SHOW_TEXT);
    let n, left = off, node = null, o = 0;
    while ((n = w.nextNode())) { if (left <= n.length) { node = n; o = left; break; } left -= n.length; }
    const r = document.createRange();
    if (node) r.setStart(node, o); else r.setStart(f, f.childNodes.length);
    r.collapse(true);
    getSelection().removeAllRanges();
    getSelection().addRange(r);
  })()`);
  const selectField = () => evaluate(`(() => {
    const r = document.createRange();
    r.selectNodeContents(document.activeElement);
    getSelection().removeAllRanges();
    getSelection().addRange(r);
  })()`);
  const press = async (key, code, modifiers = 0, text) => {
    const vk = { Enter: 13, Backspace: 8 }[key] ?? key.toUpperCase().charCodeAt(0);
    await cdp.send("Input.dispatchKeyEvent", { type: text ? "keyDown" : "rawKeyDown", key, code, windowsVirtualKeyCode: vk, modifiers, text });
    await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key, code, windowsVirtualKeyCode: vk, modifiers });
  };
  const type = (text) => cdp.send("Input.insertText", { text });
  const settle = () => new Promise((r) => setTimeout(r, 250));
  const command = (await evaluate(`/Mac|iPhone|iPad/.test(navigator.platform)`)) ? 4 : 2;
  const expect = (what, part) => until(what, async () => (await markdown()).includes(part));

  await caretIn(1, -1);
  await settle();
  await type(" ok");
  await expect("typing reaches the editor's Markdown", "prismsdk.dev). ok\n\n");
  await press("Enter", "Enter", 0, "\r");
  await until("Enter splits the block and focuses the new one", async () => (await focused()) === 2);
  await type("new");
  await expect("the new block holds the typed text", "ok\n\nnew\n\n");
  await selectField();
  await settle();
  await press("b", "KeyB", command);
  await expect("⌘B bolds the selection", "ok\n\n**new**\n\n");
  await caretIn(2, 0);
  await settle();
  await press("Backspace", "Backspace");
  await expect("Backspace at a block's start joins it to the one before", "ok**new**");
  await until("the caret stays at the join", async () => (await focused()) === 1);
  await press("Enter", "Enter", 0, "\r");
  await until("Enter at the join splits there", async () => (await focused()) === 2);
  await type("-");
  await settle();
  await type(" ");
  await expect("\"- \" starts a list item", "ok\n\n- **new**\n");
  await selectField();
  await settle();
  await press("k", "KeyK", command);
  await expect("⌘K links the selection", "- [**new**](https://prismsdk.dev)\n");
  log("rich text editor: type, Enter, ⌘B, Backspace join, \"- \" list, ⌘K link");
}

/**
 * A vertical wheel over a horizontal scroll view scrolls the vertical one
 * around it, and a horizontal one still scrolls the row (#293): the page's
 * own `.prism-scroll` / `.prism-scroll-x` rules on a fixture laid over it.
 */
async function wheelOverHorizontalRow(cdp, evaluate) {
  const at = await evaluate(`(() => {
    const outer = document.createElement("div");
    outer.className = "prism-scroll";
    outer.id = "e2e-wheel";
    outer.style.cssText = "position:fixed;left:0;top:0;width:400px;height:300px;z-index:2147483647;background:#fff;display:flex;flex-direction:column";
    const space = (h) => { const d = document.createElement("div"); d.style.cssText = "flex-shrink:0;height:" + h + "px"; return d; };
    const row = document.createElement("div");
    row.className = "prism-scroll prism-scroll-x";
    row.style.cssText = "display:flex;flex-direction:row;flex-shrink:0";
    const line = document.createElement("div");
    line.style.whiteSpace = "pre";
    line.textContent = "a very long line ".repeat(80);
    row.append(line);
    outer.append(space(600), row, space(1200));
    document.body.append(outer);
    outer.scrollTop = 500;
    const r = row.getBoundingClientRect();
    return { x: r.left + 100, y: r.top + r.height / 2, top: outer.scrollTop };
  })()`);
  const wheel = (deltaX, deltaY) => cdp.send("Input.dispatchMouseEvent", { type: "mouseWheel", x: at.x, y: at.y, deltaX, deltaY });
  await wheel(0, 200);
  await until("a vertical wheel over the row scrolls the view around it", () =>
    evaluate(`document.getElementById("e2e-wheel").scrollTop > ${at.top}`));
  // The row moved up with it: bring it back under the pointer.
  await evaluate(`document.getElementById("e2e-wheel").scrollTop = ${at.top}`);
  await wheel(200, 0);
  await until("a horizontal wheel over the row scrolls the row", () =>
    evaluate(`document.querySelector("#e2e-wheel .prism-scroll-x").scrollLeft > 0`));
  await evaluate(`document.getElementById("e2e-wheel").remove()`);
  log("wheel: vertical over a horizontal row → the outer view, horizontal → the row");
}

/** Clicks the counter's "+1" like a user and waits for the new count. */
async function clickPlusOne(cdp, evaluate, frames) {
  const target = await evaluate(`(() => {
    const plus = [...document.querySelectorAll("#prism-root [data-pid]")]
      .filter((e) => !e.closest("[hidden]"))
      .find((e) => (e.dataset.kind === "button" || e.getAttribute("role") === "button") && e.textContent.trim() === "+1");
    if (!plus) return null;
    plus.scrollIntoView({ block: "center" });
    // The count: the number in the same card.
    let card = plus.parentElement;
    let count = null;
    while (card && !count) {
      count = [...card.querySelectorAll("[data-kind=text]")].find((e) => /^-?\\d+$/.test(e.textContent.trim()));
      card = card.parentElement;
    }
    const r = plus.getBoundingClientRect();
    return count && { pid: count.dataset.pid, value: Number(count.textContent), x: r.left + r.width / 2, y: r.top + r.height / 2 };
  })()`);
  if (!target) fail("/: no visible +1 button next to a count");
  frames.sent.length = 0;
  frames.received.length = 0;
  await click(cdp, target);
  await until("+1 is sent as a click", () => frames.sent.some((m) => m.t === "click"));
  await until("the server answers with a patch", () => frames.received.some((m) => m.t === "patch"));
  const expected = target.value + 1;
  await until(`the count becomes ${expected}`, () =>
    evaluate(`document.querySelector('[data-pid="${target.pid}"]').textContent.trim() === "${expected}"`));
  log(`/: +1 → count ${target.value} → ${expected}`);
}

main().catch((e) => fail(e.stack || e.message));
