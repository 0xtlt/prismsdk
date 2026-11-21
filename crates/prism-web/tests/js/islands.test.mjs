// client.js: islands, client-side Web Components (WASM or JS).
//
// Run: node --test crates/prism-web/tests/js/
import { test } from "node:test";
import assert from "node:assert/strict";
import { boot, Element } from "./dom.mjs";

const tick = () => new Promise((r) => setImmediate(r));
/** Objects made in the page's realm, as plain data. */
const plain = (v) => JSON.parse(JSON.stringify(v));
const sent = (page) => page.ws.sent.filter((m) => m.t !== "lifecycle" && m.t !== "device");
const root = (kids) => [{ op: "create", id: 1, kind: "view" }, { op: "children", id: 1, children: kids }, { op: "root", id: 1 }];

/** A generated WASM island element: records mounts and updates. */
function wasmIsland(log) {
  return class extends Element {
    connectedCallback() { log.push(["mount", { ...this.prismProps }]); }
    prismUpdate(changed) { log.push(["update", changed]); }
  };
}

/** The ops of a WASM island `chart` (id 2) with a fallback text (id 3). */
const chart = (props = { points: [1, 2] }) => [
  { op: "create", id: 2, kind: "view" },
  { op: "island", id: 2, tag: "prism-island-chart", module: "/_prism/islands/prism-island-chart.abc.js", wasm: true, events: ["select"] },
  { op: "props", id: 2, props },
  { op: "create", id: 3, kind: "text" }, { op: "text", id: 3, text: "2 points" },
  { op: "children", id: 2, children: [3] },
];

function live(patches, importModule) {
  const page = boot({ importModule });
  page.ws.open();
  page.ws.deliver({ t: "reset", p: patches });
  return page;
}

test("a page without islands imports nothing", async () => {
  const page = live([{ op: "create", id: 2, kind: "text" }, { op: "text", id: 2, text: "Hi" }, ...root([2])]);
  await tick();
  assert.deepEqual(page.imports, []);
});

test("an island is its custom element around the fallback, its module imported once", async () => {
  const log = [];
  const page = live(
    [...chart(), { op: "create", id: 4, kind: "view" },
      { op: "island", id: 4, tag: "prism-island-chart", module: "/_prism/islands/prism-island-chart.abc.js", wasm: true, events: [] },
      ...root([2, 4])],
    (url, registry) => registry.define("prism-island-chart", wasmIsland(log)),
  );
  const el = page.node(2);
  assert.equal(el.localName, "prism-island-chart");
  assert.equal(el.dataset.kind, "view");
  // The fallback is light DOM, patched like any view.
  assert.equal(el.textContent, "2 points");
  page.ws.deliver({ t: "patch", p: [{ op: "text", id: 3, text: "3 points" }] });
  assert.equal(page.node(2).textContent, "3 points");
  await tick();
  assert.deepEqual(page.imports, ["/_prism/islands/prism-island-chart.abc.js"]);
  assert.deepEqual(plain(el.prismProps), { points: [1, 2] });
});

test("prop patches update the live island with what changed", async () => {
  const log = [];
  const page = live([...chart({ points: [1, 2], color: "red" }), ...root([2])], (url, registry) => {
    registry.define("prism-island-chart", wasmIsland(log));
  });
  await tick();
  const el = page.node(2);
  // Defined after the element was created: upgraded when the class arrives
  // (a browser does it for connected elements).
  page.customElements.upgrade(el);
  page.ws.deliver({ t: "patch", p: [{ op: "props", id: 2, props: { points: [1, 2, 3], color: null } }] });
  assert.deepEqual(plain(log.at(-1)), ["update", { points: [1, 2, 3], color: null }]);
  assert.deepEqual(plain(el.prismProps), { points: [1, 2, 3] }, "all props, a removed one gone");
  assert.equal(page.node(2), el, "the same element: no re-mount");
  assert.equal(el.getAttribute("points"), null, "WASM islands get no attributes");
});

test("an island's events go to the server, other events do not", () => {
  const page = live([...chart(), ...root([2])], () => {});
  const el = page.node(2);
  el.dispatchEvent({ type: "select", detail: [1, 2.5] });
  assert.deepEqual(plain(sent(page).at(-1)), { t: "island", id: 2, name: "select", detail: [1, 2.5] });
  const before = sent(page).length;
  el.dispatchEvent({ type: "hover", detail: 1 });
  // Bubbling from inside (the fallback) is not the island's.
  page.node(3).dispatchEvent({ type: "select", detail: 9 });
  assert.equal(sent(page).length, before);
  el.dispatchEvent({ type: "select" });
  assert.deepEqual(sent(page).at(-1), { t: "island", id: 2, name: "select", detail: null });
});

test("a web component gets attributes now and properties once defined", async () => {
  const page = live(
    [{ op: "create", id: 2, kind: "view" },
      { op: "island", id: 2, tag: "time-ago", module: "/assets/time-ago.abc.js", wasm: false, events: [] },
      { op: "props", id: 2, props: { datetime: 1700000000000, label: "x", live: true, list: [1] } },
      ...root([2])],
    (url, registry) => registry.define("time-ago", class extends Element {}),
  );
  const el = page.node(2);
  assert.equal(el.getAttribute("datetime"), "1700000000000");
  assert.equal(el.getAttribute("label"), "x");
  assert.equal(el.getAttribute("live"), "");
  assert.equal(el.getAttribute("list"), null, "objects are properties only");
  assert.equal(Object.hasOwn(el, "datetime"), false, "no property before the class (it would hide its setter)");
  await tick();
  await tick();
  assert.equal(el.datetime, 1700000000000);
  assert.deepEqual(plain(el.list), [1]);
  page.ws.deliver({ t: "patch", p: [{ op: "props", id: 2, props: { datetime: 5, live: false } }] });
  assert.equal(el.getAttribute("datetime"), "5");
  assert.equal(el.datetime, 5);
  assert.equal(el.hasAttribute("live"), false);
});

test("a web component's props never become handlers, markup or Prism's own state (S20)", async () => {
  const set = [];
  // Props from data (a CMS): names chosen by whoever wrote it.
  const props = JSON.parse(`{
    "label": "ok", "onclick": "marker()", "OnFocus": "marker()", "style": "color: red",
    "data-pid": "99", "class": "x", "innerHTML": "<i data-marker></i>",
    "__proto__": { "marker": true }, "_island": { "events": ["click"] },
    "prismProps": 1, "render": "marker", "value": "v", "points": [1]
  }`);
  const page = live(
    [{ op: "create", id: 2, kind: "view" },
      { op: "island", id: 2, tag: "x-card", module: "/assets/x-card.js", wasm: false, events: [] },
      { op: "props", id: 2, props },
      { op: "create", id: 3, kind: "text" }, { op: "text", id: 3, text: "card" },
      { op: "children", id: 2, children: [3] },
      ...root([2])],
    (url, registry) => registry.define("x-card", class extends Element {
      // A declared property, and a method.
      set value(v) { set.push(v); }
      get value() { return set.at(-1); }
      render() { return "rendered"; }
    }),
  );
  const el = page.node(2);
  assert.equal(el.getAttribute("label"), "ok");
  for (const name of ["onclick", "onfocus", "OnFocus"]) {
    assert.equal(el.getAttribute(name), null, name);
  }
  // Prism's own style and classes stay.
  assert.ok(!String(el.getAttribute("style")).includes("red"));
  assert.notEqual(el.getAttribute("class"), "x");
  assert.equal(el.getAttribute("data-pid"), "2");
  await tick();
  await tick();
  // Declared and plain props arrive.
  assert.deepEqual(set, ["v"]);
  assert.deepEqual(plain(el.points), [1]);
  // Native members, Prism's state and the prototype are untouched.
  assert.equal(el.textContent, "card");
  assert.equal(el.querySelector("[data-marker]"), null);
  assert.equal(el.render(), "rendered");
  assert.equal(el.marker, undefined);
  assert.equal(Object.getPrototypeOf(el.prismProps), null);
  assert.equal(el.prismProps.marker, undefined);
  assert.deepEqual(plain(el._island.events), []);
  page.ws.deliver({ t: "patch", p: [{ op: "props", id: 2, props: { value: "w", onclick: "marker()" } }] });
  assert.deepEqual(set, ["v", "w"]);
  assert.equal(el.getAttribute("onclick"), null);
});

test("an island that was not built keeps its fallback and says so", async () => {
  const warnings = [];
  const page = boot();
  page.window.console = { ...console, warn: (m) => warnings.push(m) };
  page.ws.open();
  page.ws.deliver({ t: "reset", p: [
    { op: "create", id: 2, kind: "view" },
    { op: "island", id: 2, tag: "prism-island-map", module: null, wasm: true, events: [] },
    ...root([2])] });
  await tick();
  assert.deepEqual(page.imports, []);
  assert.equal(page.node(2).localName, "prism-island-map");
});
