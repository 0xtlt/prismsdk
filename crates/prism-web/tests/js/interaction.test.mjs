// client.js: pointer positions, hover / move / drag, keys, focus, layers,
// overlays and the choice controls (#17, #18, #19, #25).
//
// Run: node --test crates/prism-web/tests/js/
import { test } from "node:test";
import assert from "node:assert/strict";
import { boot } from "./dom.mjs";

function live(patches) {
  const page = boot();
  page.ws.open();
  page.ws.deliver({ t: "reset", p: patches });
  return page;
}
const sent = (page) => page.ws.sent.filter((m) => m.t !== "lifecycle" && m.t !== "device");
const lastSent = (page) => sent(page).at(-1);
const flushFrames = (page) => page.frames.splice(0).forEach((f) => f());
/** A root view holding `kids` (create patches included by the caller). */
const root = (kids) => [{ op: "create", id: 1, kind: "view" }, { op: "children", id: 1, children: kids }, { op: "root", id: 1 }];
/** An element whose box is 200 × 100 at (50, 20). */
const placed = (el) => { el.getBoundingClientRect = () => ({ left: 50, top: 20, width: 200, height: 100, right: 250, bottom: 120 }); return el; };

test("clicks say where they landed; keyboard activations do not", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" }, { op: "clickable", id: 2, on: true },
    ...root([2]),
  ]);
  const el = placed(page.node(2));
  el.dispatchEvent({ type: "click", detail: 1, clientX: 80, clientY: 45 });
  assert.deepEqual(lastSent(page), { t: "click", id: 2, x: 30, y: 25 });
  el.dispatchEvent({ type: "click", detail: 0, clientX: 0, clientY: 0 });
  assert.deepEqual(lastSent(page), { t: "click", id: 2 });
});

test("hover and moves are reported, moves once per frame", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" }, { op: "pointer", id: 2, mask: 3 },
    ...root([2]),
  ]);
  const el = placed(page.node(2));
  el.dispatchEvent({ type: "pointerenter", clientX: 50, clientY: 20, bubbles: false });
  assert.deepEqual(lastSent(page), { t: "pointer", id: 2, phase: "enter", x: 0, y: 0 });
  for (const x of [60, 70, 90]) el.dispatchEvent({ type: "pointermove", clientX: x, clientY: 30 });
  assert.equal(sent(page).filter((m) => m.phase === "move").length, 0, "throttled to the next frame");
  flushFrames(page);
  const moves = sent(page).filter((m) => m.phase === "move");
  assert.deepEqual(moves, [{ t: "pointer", id: 2, phase: "move", x: 40, y: 10 }]);
  el.dispatchEvent({ type: "pointerleave", clientX: 300, clientY: 30, bubbles: false });
  assert.equal(lastSent(page).phase, "leave");
});

test("drags start past the slop, move once per frame, and do not click", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" }, { op: "pointer", id: 2, mask: 4 }, { op: "clickable", id: 2, on: true },
    ...root([2]),
  ]);
  const el = placed(page.node(2));
  assert.match(el.className, /prism-draggable/);
  const p = (type, x, y) => el.dispatchEvent({ type, pointerId: 1, button: 0, clientX: x, clientY: y });
  p("pointerdown", 60, 30);
  p("pointermove", 62, 31);
  assert.equal(sent(page).filter((m) => m.t === "drag").length, 0, "within the slop");
  p("pointermove", 70, 30);
  assert.deepEqual(lastSent(page), { t: "drag", id: 2, x: 10, y: 10, dx: 10, dy: 0, phase: "start" });
  p("pointermove", 80, 30);
  p("pointermove", 90, 32);
  flushFrames(page);
  assert.deepEqual(lastSent(page), { t: "drag", id: 2, x: 10, y: 10, dx: 30, dy: 2, phase: "move" });
  p("pointerup", 95, 32);
  assert.deepEqual(lastSent(page), { t: "drag", id: 2, phase: "end", x: 10, y: 10, dx: 35, dy: 2 });
  const before = sent(page).length;
  el.dispatchEvent({ type: "click", detail: 1, clientX: 95, clientY: 32 });
  assert.equal(sent(page).length, before, "the drag's click is swallowed");
  // A plain tap still clicks once the drag is over.
  page.timers.splice(0).forEach((t) => t.fn());
  p("pointerdown", 60, 30);
  p("pointerup", 60, 30);
  el.dispatchEvent({ type: "click", detail: 1, clientX: 60, clientY: 30 });
  assert.equal(lastSent(page).t, "click");
});

test("keys reach the focused element, and global listeners skip typing", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" }, { op: "keys", id: 2, local: true, global: false },
    { op: "create", id: 3, kind: "view" },
    { op: "create", id: 4, kind: "text_input" },
    { op: "children", id: 2, children: [3] },
    ...root([2, 4]),
  ]);
  const pad = page.node(2), inner = page.node(3), field = page.node(4);
  assert.equal(pad.getAttribute("tabindex"), "0", "a view with key handlers can take the focus");
  const key = (target, k, extra = {}) => target.dispatchEvent({ type: "keydown", key: k, ...extra });
  key(inner, "ArrowUp", { shiftKey: true });
  assert.deepEqual(lastSent(page), { t: "key", id: 2, key: "ArrowUp", mods: 1, repeat: false });
  // Nothing listens anywhere else: nothing is sent.
  const n = sent(page).length;
  key(field, "Escape");
  assert.equal(sent(page).length, n);
  // A global listener: hears Escape in the field, not the letters typed there.
  page.ws.deliver({ t: "patch", p: [{ op: "create", id: 5, kind: "view" }, { op: "keys", id: 5, local: false, global: true }, { op: "children", id: 1, children: [2, 4, 5] }] });
  key(field, "a");
  assert.equal(sent(page).length, n);
  key(field, "Escape");
  assert.deepEqual(lastSent(page), { t: "key", id: 0, key: "Escape", mods: 0, repeat: false });
  key(field, "s", { metaKey: true });
  assert.equal(lastSent(page).mods, 8);
  key(page.document.body, "x");
  assert.equal(lastSent(page).key, "x");
  // Destroyed: unsubscribed.
  page.ws.deliver({ t: "patch", p: [{ op: "children", id: 1, children: [2, 4] }, { op: "destroy", id: 5 }] });
  const m = sent(page).length;
  key(page.document.body, "y");
  assert.equal(sent(page).length, m);
});

test("the app can focus a node and scroll it into view", () => {
  const page = live([
    { op: "create", id: 2, kind: "text_input" }, { op: "create", id: 3, kind: "checkbox" },
    ...root([2, 3]),
  ]);
  let revealed = null;
  page.node(2).scrollIntoView = (o) => { revealed = o; };
  page.ws.deliver({ t: "patch", p: [{ op: "reveal", id: 2 }, { op: "focus", id: 2 }] });
  assert.equal(revealed.block, "nearest");
  assert.equal(page.document.activeElement, page.node(2));
  page.ws.deliver({ t: "patch", p: [{ op: "focus", id: 3 }] });
  assert.equal(page.document.activeElement, page.node(3).firstChild, "a checkbox focuses its box");
});

test("layers are mounted above everything", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" }, { op: "create", id: 3, kind: "text" }, { op: "text", id: 3, text: "Saved" },
    { op: "children", id: 2, children: [3] },
    ...root([]),
    { op: "layers", ids: [2] },
  ]);
  const host = page.document.getElementById("prism-layers");
  assert.ok(host, "the client creates the layer host if the page has none");
  assert.equal(host.firstChild, page.node(2));
  assert.equal(host.textContent, "Saved");
  page.ws.deliver({ t: "patch", p: [{ op: "layers", ids: [] }] });
  assert.equal(host.childNodes.length, 0);
});

test("dialogs open, close on Escape (back) and stay open when fixed", () => {
  const page = live([
    { op: "create", id: 2, kind: "overlay" }, { op: "config", id: 2, config: "dialog" },
    { op: "value", id: 2, value: 0 },
    ...root([2]),
  ]);
  const ov = page.node(2);
  const box = ov.firstChild;
  assert.equal(box.localName, "dialog");
  assert.match(box.className, /prism-ov-dialog/);
  assert.equal(box.hasAttribute("open"), false);
  page.ws.deliver({ t: "patch", p: [
    { op: "value", id: 2, value: 1 },
    { op: "create", id: 3, kind: "text" }, { op: "text", id: 3, text: "Sure?" },
    { op: "children", id: 2, children: [3] },
  ] });
  assert.equal(box.hasAttribute("open"), true);
  assert.equal(box.textContent, "Sure?", "content goes in the dialog");
  // Escape: the browser closes it (`close`); the app hears back.
  box.dispatchEvent({ type: "cancel" });
  box.close();
  box.dispatchEvent({ type: "close" });
  assert.deepEqual(lastSent(page), { t: "back", id: 2, depth: 0 });
  // The app closing it says nothing back.
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 2, value: 1 }] });
  const n = sent(page).length;
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 2, value: 0 }] });
  box.dispatchEvent({ type: "close" });
  assert.equal(sent(page).length, n);
  // Fixed: Escape is refused.
  page.ws.deliver({ t: "patch", p: [{ op: "config", id: 2, config: "dialog fixed" }, { op: "value", id: 2, value: 1 }] });
  const cancel = { type: "cancel" };
  box.dispatchEvent(cancel);
  assert.equal(cancel.defaultPrevented, true);
});

test("popovers sit under their anchor and report light dismissal", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" },
    { op: "create", id: 3, kind: "overlay" }, { op: "config", id: 3, config: "popover" },
    { op: "children", id: 2, children: [3] },
    ...root([2]),
  ]);
  placed(page.node(2));
  const ov = page.node(3);
  const box = ov.firstChild;
  box.getBoundingClientRect = () => ({ left: 0, top: 0, width: 100, height: 60, right: 100, bottom: 60 });
  assert.equal(box.getAttribute("popover"), "auto");
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 3, value: 1 }] });
  assert.equal(box.style.top, "126px", "6 px under the anchor");
  assert.equal(box.style.left, "50px");
  box.dispatchEvent({ type: "toggle", newState: "closed" });
  assert.deepEqual(lastSent(page), { t: "back", id: 3, depth: 0 });
});

test("checkboxes and radio groups are native inputs", () => {
  const page = live([
    { op: "create", id: 2, kind: "checkbox" }, { op: "text", id: 2, text: "Remember me" }, { op: "value", id: 2, value: 1 },
    { op: "create", id: 3, kind: "radio_group" }, { op: "config", id: 3, config: "row\nDaily\nWeekly" }, { op: "value", id: 3, value: 0 },
    { op: "a11y", id: 3, a11y: { role: "radiogroup", label: "Frequency", checked: null, disabled: false, hidden: false } },
    ...root([2, 3]),
  ]);
  const cb = page.node(2);
  assert.equal(cb.localName, "label");
  assert.equal(cb.firstChild.type, "checkbox");
  assert.equal(cb.firstChild.checked, true);
  assert.equal(cb.textContent, "Remember me");
  cb.firstChild.checked = false;
  cb.firstChild.dispatchEvent({ type: "change" });
  assert.deepEqual(lastSent(page), { t: "change", id: 2, v: 0 });
  const rg = page.node(3);
  assert.equal(rg.getAttribute("role"), "radiogroup");
  assert.equal(rg.getAttribute("aria-label"), "Frequency");
  assert.equal(rg.dataset.row, "1");
  const inputs = rg.querySelectorAll("input");
  assert.deepEqual(inputs.map((i) => [i.type, i.checked]), [["radio", true], ["radio", false]]);
  assert.equal(inputs[0].getAttribute("name"), inputs[1].getAttribute("name"));
  inputs[1].checked = true;
  inputs[1].dispatchEvent({ type: "change" });
  assert.deepEqual(lastSent(page), { t: "change", id: 3, v: 1 });
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 3, value: 1 }] });
  assert.deepEqual(inputs.map((i) => i.checked), [false, true]);
});

test("text fields: max length, suggestions and a leading icon", () => {
  const page = live([
    { op: "create", id: 2, kind: "text_input" },
    { op: "style", id: 2, css: "color:red;" },
    { op: "config", id: 2, config: "max=24\nParis\nLyon" },
    { op: "icon", id: 2, icon: "magnifyingglass", glyph: "🔍" },
    ...root([2]),
  ]);
  const el = page.node(2);
  assert.equal(el.getAttribute("maxlength"), "24");
  const list = page.document.getElementById(el.getAttribute("list"));
  assert.equal(list.localName, "datalist");
  assert.deepEqual(list.children.map((o) => o.getAttribute("value")), ["Paris", "Lyon"]);
  assert.match(el.className, /prism-icon-field/);
  assert.match(el.style.cssText, /--prism-icon:url\("data:image\/svg\+xml,/);
  // A new style keeps the icon.
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 2, css: "color:blue;" }] });
  assert.match(el.style.cssText, /color:blue;.*--prism-icon/);
  // Suggestions removed: no list.
  page.ws.deliver({ t: "patch", p: [{ op: "config", id: 2, config: "" }] });
  assert.equal(el.getAttribute("list"), null);
  assert.equal(el.getAttribute("maxlength"), null);
});

test("a time of day ignores the time zone", () => {
  const page = live([
    { op: "create", id: 2, kind: "date_picker" }, { op: "config", id: 2, config: "timeofday - -" },
    { op: "value", id: 2, value: 27000 },
    ...root([2]),
  ]);
  const el = page.node(2);
  assert.equal(el.type, "time");
  assert.equal(el.value, "07:30");
  el.value = "22:05";
  el.dispatchEvent({ type: "change" });
  assert.deepEqual(lastSent(page), { t: "change", id: 2, v: 22 * 3600 + 5 * 60 });
});

test("inert styles and hit testing", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" }, { op: "style", id: 2, css: "pointer-events:none;", inert: true },
    ...root([2]),
  ]);
  const el = page.node(2);
  assert.equal(el.hasAttribute("inert"), true);
  assert.equal(el.style.pointerEvents, "none");
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 2, css: "" }] });
  assert.equal(el.hasAttribute("inert"), false);
});
