// Security review S28 and S32 in real browser engines (Chromium and WebKit,
// through Playwright).
//
// S28: the Web Push service worker (crates/prism-web/src/sw.js, unmodified)
// tells only the app's own pages about a push: windows it controls, inside
// its mount. A sibling app on the same origin (/apple/ next to /app/) and a
// page outside the mount hear nothing, and another app's visible page no
// longer swallows the app's notification. The same run with sw.js from
// before the fix (a777a3d^) shows the leak the test catches.
//   Chromium: a real push event (DevTools ServiceWorker.deliverPushMessage).
//   WebKit: no push service in a test browser, so a few lines appended to
//   the served worker (marked TEST HARNESS) dispatch a push event when a page
//   asks; sw.js itself is unchanged.
//   Clicks need a real notification click, which a headless browser cannot
//   make: crates/prism-web/tests/js/sw.test.mjs covers them (run here too).
//
// S32: the script a native web view runs to hand the page the app's message
// (prism_core::web_view::message_script, built the same way by
// PrismWebView.swift and PrismWeb.java) dispatches only in a page that still
// shows the origin it was sent to. A page of another origin tries to fake
// `location` first; a positive control (the script without its origin
// check) proves the script did run in that page. The race: the allowed page
// navigates away before the script runs.
//
//   NODE_PATH=<dir>/node_modules node scripts/browser-push-messages.mjs [out dir]
//
// Needs playwright-core and its browsers (`npx playwright-core install webkit`).
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const { chromium, webkit } = require("playwright-core");
const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const out = path.resolve(process.argv[2] || ".");
const SW = {
  fixed: fs.readFileSync(path.join(repo, "crates/prism-web/src/sw.js"), "utf8"),
  before: execFileSync("git", ["-C", repo, "show", "a777a3d^:crates/prism-web/src/sw.js"], { encoding: "utf8" }),
};
const HARNESS = `
// TEST HARNESS (scripts/browser-push-messages.mjs, not part of sw.js): this
// browser has no push service, so a page asks and a push event is dispatched
// here, kept alive by the message event.
self.addEventListener("message", (event) => {
  if (!event.data || !event.data.s28push) return;
  const payload = event.data.s28push;
  const push = new ExtendableEvent("push");
  Object.defineProperty(push, "data", { value: { json: () => payload, text: () => JSON.stringify(payload) } });
  Object.defineProperty(push, "waitUntil", { value: (p) => event.waitUntil(p) });
  self.dispatchEvent(push);
});
`;

let failed = false;
const log = (s) => console.log(s);
const check = (ok, what) => {
  failed ||= !ok;
  log(`${ok ? "ok  " : "FAIL"} ${what}`);
};

// What the page shows: what it received, live.
const LISTENER = `
<pre id="got">received: none</pre>
<script>
  window.got = [];
  const show = () => document.getElementById("got").textContent =
    "received: " + (got.length ? got.map((m) => JSON.stringify(m)).join("\\n") : "none");
  const record = (e) => { got.push(e.data); show(); };
</script>`;

const page = (title, body) =>
  `<!doctype html><meta charset="utf-8"><title>${title}</title><body style="font:15px system-ui;margin:12px">` +
  `<h3 id="where"></h3><script>document.getElementById("where").textContent = ${JSON.stringify(title)} + " at " + location.href;</script>${body}</body>`;

const serve = (handler) =>
  new Promise((resolve) => {
    const server = http.createServer(handler).listen(0, "127.0.0.1", () => resolve(server));
  });

// Origin A: the app (S28's /app/, S32's allowed page). Origin B: another site.
let swMode = "fixed";
let harness = false;
const a = await serve((req, res) => {
  const url = new URL(req.url, "http://x");
  const send = (type, body) => {
    res.writeHead(200, { "Content-Type": type, "Cache-Control": "no-store" });
    res.end(body);
  };
  if (url.pathname === "/app/prism-sw.js") return send("text/javascript", SW[swMode] + (harness ? HARNESS : ""));
  if (url.pathname === "/app/" || url.pathname === "/app/inbox") {
    return send("text/html", page("The app (/app/, controlled)", LISTENER + `<script>
      navigator.serviceWorker.addEventListener("message", record);
      navigator.serviceWorker.register("/app/prism-sw.js", { scope: "/app/" });
    </script>`));
  }
  if (url.pathname === "/apple/" || url.pathname === "/other.html") {
    const title = url.pathname === "/apple/" ? "A sibling app (/apple/)" : "A page outside the mount (/other.html)";
    return send("text/html", page(title, LISTENER + `<script>
      navigator.serviceWorker.addEventListener("message", record);
      navigator.serviceWorker.startMessages && navigator.serviceWorker.startMessages();
    </script>`));
  }
  if (url.pathname === "/s32/app.html") {
    return send("text/html", page("The allowed page", LISTENER + `<script>addEventListener("message", record);</script>`));
  }
  res.writeHead(404);
  res.end();
});
const A = `http://127.0.0.1:${a.address().port}`;
const b = await serve((req, res) => {
  res.writeHead(200, { "Content-Type": "text/html", "Cache-Control": "no-store" });
  // Every way a page could make `location` read as A's.
  res.end(page("Another site", LISTENER + `<pre id="attempts"></pre>
<script>
  addEventListener("message", record);
  window.attempts = {};
  window.addEventListener("error", (e) => { attempts["let location = … (global lexical binding)"] = "refused: " + e.message; });
  const fake = { protocol: "http:", host: ${JSON.stringify(A.slice(7))} };
  const attempt = (name, f) => {
    try { f(); attempts[name] = "no error"; } catch (e) { attempts[name] = "refused: " + e.name; }
  };
  attempt("Object.defineProperty(window, 'location', …)", () => Object.defineProperty(window, "location", { get: () => fake }));
  attempt("window.__defineGetter__('location', …)", () => window.__defineGetter__("location", () => fake));
  attempt("Object.defineProperty(location, 'host', …)", () => Object.defineProperty(location, "host", { get: () => fake.host }));
  attempt("Object.defineProperty(location, 'protocol', …)", () => Object.defineProperty(location, "protocol", { get: () => fake.protocol }));
  attempt("delete location.host", () => { if (!delete location.host) throw new TypeError("not deleted"); });
  attempt("Location.prototype host and protocol getters", () => {
    Object.defineProperty(Location.prototype, "host", { get: () => fake.host, configurable: true });
    Object.defineProperty(Location.prototype, "protocol", { get: () => fake.protocol, configurable: true });
  });
  attempt("Object.setPrototypeOf(location, fake)", () => Object.setPrototypeOf(location, fake));
  attempt("Object.prototype.location getter", () => Object.defineProperty(Object.prototype, "location", { get: () => fake, configurable: true }));
</script>
<script>let location = { protocol: "http:", host: ${JSON.stringify(A.slice(7))} };</script>
<script>
  attempts["let location = … (global lexical binding)"] ??= "no error";
  attempts["=> location.protocol + '//' + location.host reads"] = location.protocol + "//" + location.host;
  document.getElementById("attempts").textContent = Object.entries(attempts).map(([k, v]) => k + ": " + v).join("\\n");
</script>`));
});
const B = `http://localhost:${b.address().port}`;

// prism_core::web_view::message_script (and its Swift and Java twins).
const messageScript = (json, origin) => {
  const dispatch = `window.dispatchEvent(new MessageEvent("message", { data: ${json} }));`;
  return origin == null ? dispatch : `if (location.protocol + "//" + location.host === ${JSON.stringify(origin)}) ${dispatch}`;
};
const GUARDED = messageScript(`{"s32":"for the allowed origin only"}`, A);
const CONTROL = messageScript(`{"s32":"control, no origin check"}`, null);
const got = (p) => p.evaluate("window.got");

async function s32(engine, browser) {
  log(`-- S32 ${engine}: the script handing the page the app's message, sent for ${A}`);
  const context = await browser.newContext();
  const p = await context.newPage();
  await p.goto(`${A}/s32/app.html`);
  await p.evaluate(GUARDED);
  check((await got(p)).length === 1, `the allowed page (${A}) gets it`);
  // The race: the message was sent while the allowed page showed, and its
  // script runs once the view has moved to another site.
  await Promise.all([p.waitForURL(`${B}/s32/evil.html`, { waitUntil: "load" }), p.evaluate(`location.href = ${JSON.stringify(`${B}/s32/evil.html`)}`)]);
  const attempts = await p.evaluate("window.attempts");
  for (const [k, v] of Object.entries(attempts)) log(`     ${k}: ${v}`);
  await p.evaluate(GUARDED);
  check((await got(p)).length === 0, `after navigating to ${B} (which tried to fake location), the script hands it nothing`);
  await p.evaluate(CONTROL);
  check((await got(p)).length === 1, "control: the same script without its origin check does run there (so the check is what refused)");
  await p.screenshot({ path: path.join(out, `browser-s32-${engine}.png`), fullPage: true });
  await p.goBack({ waitUntil: "load" });
  await p.evaluate(GUARDED);
  check((await got(p)).length === 1, "back on the allowed page, it gets it again");
  await context.close();
}

async function s28(engine, browser, mode) {
  swMode = mode;
  harness = engine !== "chromium";
  log(`-- S28 ${engine}: sw.js ${mode === "fixed" ? "(this tree)" : "from before the fix (a777a3d^)"}${harness ? ", push dispatched by the test harness" : ", real push events"}`);
  const context = await browser.newContext();
  let notifications = true;
  try {
    await context.grantPermissions(["notifications"], { origin: A });
  } catch {
    notifications = false;
  }
  const app = await context.newPage();
  await app.goto(`${A}/app/`);
  // Playwright's WebKit keeps the permission at "default" even when granted.
  notifications &&= (await app.evaluate("self.Notification ? Notification.permission : 'none'")) === "granted";
  await app.waitForFunction("navigator.serviceWorker.controller !== null", null, { timeout: 15000 });
  const sibling = await context.newPage();
  await sibling.goto(`${A}/apple/`);
  const outside = await context.newPage();
  await outside.goto(`${A}/other.html`);
  const state = async (p) => p.evaluate("document.visibilityState + (navigator.serviceWorker.controller ? ', controlled' : ', not controlled')");
  log(`     /app/: ${await state(app)}; /apple/: ${await state(sibling)}; /other.html: ${await state(outside)}`);

  let push;
  if (engine === "chromium") {
    const cdp = await context.newCDPSession(sibling);
    const registrations = [];
    cdp.on("ServiceWorker.workerRegistrationUpdated", (e) => registrations.push(...e.registrations));
    await cdp.send("ServiceWorker.enable");
    const deadline = Date.now() + 10000;
    while (!registrations.some((r) => r.scopeURL === `${A}/app/` && !r.isDeleted) && Date.now() < deadline) await sibling.waitForTimeout(100);
    const reg = registrations.find((r) => r.scopeURL === `${A}/app/` && !r.isDeleted);
    push = (payload) => cdp.send("ServiceWorker.deliverPushMessage", { origin: A, registrationId: reg.registrationId, data: JSON.stringify(payload) });
  } else {
    push = (payload) => sibling.evaluate(async (payload) => {
      const reg = await navigator.serviceWorker.getRegistration("/app/");
      reg.active.postMessage({ s28push: payload });
    }, payload);
  }
  const received = async (p) => (await got(p)).filter((m) => m && m.prismPush === "received").map((m) => m.payload.id);

  // 1. The app is open: only its page hears about the push.
  await push({ id: `${mode}-1`, title: "Private", body: "for the app's own pages only" });
  await app.waitForTimeout(1500);
  const r1 = { app: await received(app), sibling: await received(sibling), outside: await received(outside) };
  log(`     push 1, app open -> /app/ ${JSON.stringify(r1.app)}, /apple/ ${JSON.stringify(r1.sibling)}, /other.html ${JSON.stringify(r1.outside)}`);
  const leak1 = r1.sibling.length + r1.outside.length > 0;
  if (mode === "fixed") {
    check(r1.app.includes("fixed-1"), "the app's page gets the push");
    check(!leak1, "the sibling app and the page outside the mount get nothing");
  } else {
    check(leak1, "before the fix, the push leaked to the sibling app or the page outside the mount (what the test catches)");
  }
  await sibling.screenshot({ path: path.join(out, `browser-s28-${engine}-${mode}-sibling.png`) });

  // 2. The app is closed, another app's page is visible: the notification
  // still shows, and that page hears nothing.
  await app.close();
  await sibling.waitForTimeout(500);
  await push({ id: `${mode}-2`, title: "Private", body: "while the app is closed" });
  await sibling.waitForTimeout(1500);
  const r2 = await received(sibling);
  const shown = notifications
    ? await sibling.evaluate(async () => {
        const reg = await navigator.serviceWorker.getRegistration("/app/");
        return (await reg.getNotifications()).map((n) => n.tag);
      }).catch((e) => `n/a (${e.message})`)
    : "n/a (this browser does not grant the notification permission)";
  log(`     push 2, app closed, /apple/ visible -> /apple/ ${JSON.stringify(r2)}; notifications shown: ${JSON.stringify(shown)}`);
  if (mode === "fixed") {
    check(!r2.length, "the sibling app's visible page gets nothing");
    if (Array.isArray(shown)) check(shown.includes("fixed-2"), "the app's notification still shows");
  } else {
    check(r2.length > 0 || (Array.isArray(shown) && !shown.includes("before-2")), "before the fix, the sibling page got it or swallowed the notification");
  }
  await context.close();
}

for (const [engine, type] of [["chromium", chromium], ["webkit", webkit]]) {
  // Chromium: the full Chrome for Testing build (new headless), not the headless shell.
  const browser = await type.launch(engine === "chromium" ? { channel: "chromium" } : {});
  log(`== ${engine} ${browser.version()}`);
  await s32(engine, browser);
  for (const mode of ["fixed", "before"]) await s28(engine, browser, mode);
  await browser.close();
}
a.close();
b.close();

log("-- S28 clicks: crates/prism-web/tests/js/sw.test.mjs (node --test)");
try {
  const r = execFileSync("node", ["--test", "--test-reporter=tap", path.join(repo, "crates/prism-web/tests/js/sw.test.mjs")], { encoding: "utf8" });
  for (const line of r.split("\n").filter((l) => /^(ok|not ok|# (pass|fail))/.test(l.trim()))) log(`     ${line.trim()}`);
} catch (e) {
  failed = true;
  log(`FAIL sw.test.mjs:\n${e.stdout}`);
}
log(failed ? "browser-push-messages: FAIL" : "browser-push-messages: OK");
process.exit(failed ? 1 : 0);
