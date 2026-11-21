// The Web Push service worker (`sw.js`, S28 of the security review): pushes
// and clicks reach only the app's own pages, inside its mount, and a click
// opens only a page of the app.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const SOURCE = readFileSync(new URL("../../src/sw.js", import.meta.url), "utf8");

let ids = 0;
// A window. `controlled`: this worker controls it (a page loaded in its
// scope); pages of other apps on the origin never are.
function page(url, { controlled = false, visible = false } = {}) {
  return {
    id: `client-${++ids}`,
    type: "window",
    url,
    controlled,
    visibilityState: visible ? "visible" : "hidden",
    got: [],
    focused: false,
    postMessage(m) {
      this.got.push(m);
    },
    async focus() {
      this.focused = true;
    },
  };
}

// Runs `sw.js` with the scope `scope` and the windows `windows`.
// `onOpen(url)` runs before `openWindow` returns the window it made.
function worker(scope, windows, { onOpen = async () => {}, opens = true } = {}) {
  const listeners = {};
  const shown = [];
  const opened = [];
  const self = {
    registration: { scope, showNotification: async (title, options) => shown.push({ title, options }) },
    clients: {
      matchAll: async ({ includeUncontrolled = false } = {}) => windows.filter((w) => includeUncontrolled || w.controlled),
      openWindow: async (url) => {
        opened.push(url);
        const w = page(new URL(url, scope).href, { controlled: true });
        windows.push(w);
        await onOpen(w);
        return opens ? w : null;
      },
      claim: async () => {},
    },
    skipWaiting() {},
    addEventListener(type, f) {
      listeners[type] = f;
    },
  };
  new Function("self", SOURCE)(self);
  const fire = async (type, event) => {
    let work = Promise.resolve();
    listeners[type]({ ...event, waitUntil: (p) => (work = p) });
    await work;
  };
  return {
    shown,
    opened,
    push: (payload) => fire("push", { data: { json: () => payload, text: () => JSON.stringify(payload) } }),
    click: (payload) => fire("notificationclick", { notification: { data: payload, close() {} } }),
    ready: (from) => fire("message", { data: { prismReady: true }, source: from }),
  };
}

const PUSH = { title: "New message", body: "from Ada", link: "/inbox?id=3" };

test("a push reaches only the app's pages inside its mount", async () => {
  const app = page("https://site.test/app/inbox", { controlled: true });
  const sibling = page("https://site.test/apple/", { visible: true });
  const outside = page("https://site.test/blog");
  const sw = worker("https://site.test/app/", [sibling, outside, app]);
  await sw.push(PUSH);
  assert.deepEqual(app.got, [{ prismPush: "received", payload: PUSH }]);
  assert.deepEqual(sibling.got, []);
  assert.deepEqual(outside.got, []);
  // Another app's visible page does not hide this app's notification.
  assert.equal(sw.shown.length, 1);
  assert.equal(sw.shown[0].title, "New message");
});

test("a visible page of the app shows the push itself", async () => {
  const app = page("https://site.test/app", { controlled: true, visible: true });
  const sw = worker("https://site.test/app/", [app]);
  await sw.push(PUSH);
  assert.equal(app.got.length, 1);
  assert.deepEqual(sw.shown, []);
});

test("a click focuses a page of the app, never a similarly named sibling", async () => {
  const sibling = page("https://site.test/apple/");
  const sw = worker("https://site.test/app/", [sibling]);
  await sw.click(PUSH);
  assert.equal(sibling.focused, false);
  assert.deepEqual(sibling.got, []);
  assert.deepEqual(sw.opened.map((u) => new URL(u, "https://site.test/app/").href), ["https://site.test/app/inbox?id=3"]);

  const app = page("https://site.test/app/settings", { controlled: true });
  const sw2 = worker("https://site.test/app/", [sibling, app]);
  await sw2.click(PUSH);
  assert.equal(app.focused, true);
  assert.deepEqual(app.got, [{ prismPush: "opened", payload: PUSH }]);
  assert.deepEqual(sw2.opened, []);
});

test("a click opens only a page of the app", async () => {
  for (const [scope, link] of [
    ["https://site.test/", "//evil.test/login"],
    ["https://site.test/", "/\\evil.test/login"],
    ["https://site.test/", "https://evil.test/"],
    ["https://site.test/app/", "/../admin"],
    ["https://site.test/app/", "/%2e%2e/admin"],
    ["https://site.test/app/", "javascript:alert(1)"],
  ]) {
    const sw = worker(scope, []);
    await sw.click({ link });
    assert.equal(sw.opened.length, 1, link);
    assert.equal(new URL(sw.opened[0], scope).href, scope, `${scope} ${link}`);
  }
  const sw = worker("https://site.test/", []);
  await sw.click({ link: "/chat/42#latest" });
  assert.equal(new URL(sw.opened[0], "https://site.test/").href, "https://site.test/chat/42#latest");
});

test("only the window a click opened learns about the click", async () => {
  const sibling = page("https://site.test/apple/");
  const outside = page("https://site.test/blog");
  const other = page("https://site.test/app/other", { controlled: true });
  const windows = [sibling, outside];
  let launched = null;
  const sw = worker("https://site.test/app/", windows, { onOpen: async (w) => (launched = w) });
  await sw.click(PUSH);
  windows.push(other);
  // Pages that are not the opened one ask first: they get nothing.
  for (const w of [sibling, outside, other]) await sw.ready(w);
  for (const w of [sibling, outside, other]) assert.deepEqual(w.got, [], w.url);
  await sw.ready(launched);
  assert.deepEqual(launched.got, [{ prismPush: "opened", payload: PUSH, launched: true }]);
  // Told once.
  await sw.ready(launched);
  await sw.ready(other);
  assert.equal(launched.got.length, 1);
  assert.deepEqual(other.got, []);
});

test("a window that connects before openWindow returns is told once", async () => {
  const sibling = page("https://site.test/apple/");
  let launched = null;
  const sw = worker("https://site.test/app/", [sibling], {
    onOpen: async (w) => {
      launched = w;
      await sw.ready(sibling);
      await sw.ready(w);
      await sw.ready(w);
    },
  });
  await sw.click(PUSH);
  assert.deepEqual(launched.got, [{ prismPush: "opened", payload: PUSH, launched: true }]);
  assert.deepEqual(sibling.got, []);
});

test("when the browser does not say which window it opened, nobody is told", async () => {
  const other = page("https://site.test/app/", { controlled: true });
  const sw = worker("https://site.test/app/", [], { opens: false });
  await sw.click(PUSH);
  await sw.ready(other);
  assert.deepEqual(other.got, []);
});
