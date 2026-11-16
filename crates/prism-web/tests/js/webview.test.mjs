// client.js: web views (#62): the frame, messages both ways (allowed
// origins only, queued until the page loads), loads, reload, sandbox.
//
// Run: node --test crates/prism-web/tests/js/
import { test } from "node:test";
import assert from "node:assert/strict";
import { boot } from "./dom.mjs";

const root = (kids) => [{ op: "create", id: 1, kind: "view" }, { op: "children", id: 1, children: kids }, { op: "root", id: 1 }];
const sent = (page) => page.ws.sent.filter((m) => m.t === "webmsg" || m.t === "webload");

/** A page with a web view (node 2) showing `url`, its frame given a window. */
function withWebView(config, url = "https://preview.example.com/page/1") {
  const page = boot();
  page.ws.open();
  page.ws.deliver({ t: "reset", p: [
    { op: "create", id: 2, kind: "web_view" },
    { op: "text", id: 2, text: url },
    { op: "config", id: 2, config },
    { op: "a11y", id: 2, a11y: { label: "Preview" } },
    ...root([2]),
  ] });
  const frame = page.node(2);
  const inbox = [];
  const reloads = [];
  // The frame's page: another origin (its location cannot be read).
  frame.contentWindow = {
    postMessage: (data, origin) => inbox.push({ data, origin }),
    location: { get href() { throw new Error("SecurityError"); }, reload() { throw new Error("SecurityError"); } },
  };
  return { page, frame, inbox, reloads };
}

test("a web view is a titled iframe of its URL, sandboxed when asked", () => {
  const { frame } = withWebView("messages sandbox=scripts,same-origin");
  assert.equal(frame.localName, "iframe");
  assert.equal(frame.getAttribute("src"), "https://preview.example.com/page/1");
  assert.equal(frame.getAttribute("title"), "Preview");
  assert.equal(frame.getAttribute("sandbox"), "allow-scripts allow-same-origin");
  assert.match(frame.className, /prism-web-view/);
  const plain = withWebView("messages").frame;
  assert.equal(plain.getAttribute("sandbox"), null, "not sandboxed by default");
});

test("messages to the page wait for it to load, then go to its origin only", () => {
  const { page, frame, inbox } = withWebView("messages");
  page.ws.deliver({ t: "patch", p: [{ op: "webpost", id: 2, data: { select: "s1" } }] });
  assert.equal(inbox.length, 0, "not loaded yet");
  frame.dispatchEvent({ type: "load", bubbles: false });
  assert.deepEqual(inbox, [{ data: { select: "s1" }, origin: "https://preview.example.com" }]);
  page.ws.deliver({ t: "patch", p: [{ op: "webpost", id: 2, data: 7 }] });
  assert.deepEqual(inbox.at(-1), { data: 7, origin: "https://preview.example.com" });
});

test("the page's messages reach the app from allowed origins only", () => {
  const { page, frame } = withWebView("messages\nhttps://preview.example.com\nhttps://cdn.example.com");
  const post = (data, origin, source = frame.contentWindow) => page.fire("message", { data, origin, source });
  post({ selected: "s2" }, "https://preview.example.com");
  post("hi", "https://cdn.example.com");
  post({ evil: true }, "https://evil.example.com");
  post({ other: true }, "https://preview.example.com", {});
  assert.deepEqual(sent(page), [
    { t: "webmsg", id: 2, data: { selected: "s2" }, origin: "https://preview.example.com" },
    { t: "webmsg", id: 2, data: "hi", origin: "https://cdn.example.com" },
  ]);
});

test("without on_message nothing is forwarded; '*' allows any origin", () => {
  const quiet = withWebView("");
  quiet.page.fire("message", { data: 1, origin: "https://preview.example.com", source: quiet.frame.contentWindow });
  assert.deepEqual(sent(quiet.page), []);
  const any = withWebView("messages\n*");
  any.page.fire("message", { data: 2, origin: "https://elsewhere.example", source: any.frame.contentWindow });
  assert.deepEqual(sent(any.page), [{ t: "webmsg", id: 2, data: 2, origin: "https://elsewhere.example" }]);
  any.frame.dispatchEvent({ type: "load", bubbles: false });
  any.page.ws.deliver({ t: "patch", p: [{ op: "webpost", id: 2, data: "x" }] });
  assert.deepEqual(any.inbox, [{ data: "x", origin: "*" }]);
});

test("loads are reported with the URL; a reload loads the URL again", () => {
  const { page, frame } = withWebView("loads");
  frame.dispatchEvent({ type: "load", bubbles: false });
  assert.deepEqual(sent(page), [{ t: "webload", id: 2, url: "https://preview.example.com/page/1" }]);
  frame.removeAttribute("src");
  page.ws.deliver({ t: "patch", p: [{ op: "webreload", id: 2 }] });
  assert.equal(frame.getAttribute("src"), "https://preview.example.com/page/1", "another origin: the URL again");
  // A new URL from the app navigates; the same one does not.
  page.ws.deliver({ t: "patch", p: [{ op: "text", id: 2, text: "https://preview.example.com/page/2" }] });
  assert.equal(frame.getAttribute("src"), "https://preview.example.com/page/2");
});

test("a page of the app's own origin reloads in place", () => {
  const { page, frame } = withWebView("loads", "/preview/1");
  let reloaded = 0;
  frame.contentWindow.location = { href: "http://localhost:3000/preview/1?x=1", reload() { reloaded++; } };
  frame.dispatchEvent({ type: "load", bubbles: false });
  assert.deepEqual(sent(page), [{ t: "webload", id: 2, url: "http://localhost:3000/preview/1?x=1" }]);
  page.ws.deliver({ t: "patch", p: [{ op: "webreload", id: 2 }] });
  assert.equal(reloaded, 1);
});
