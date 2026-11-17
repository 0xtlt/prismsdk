// client.js: pointer positions, hover / move / drag, keys, focus, layers,
// overlays and the choice controls (#17, #18, #19, #25).
//
// Run: node --test crates/prism-web/tests/js/
import { test } from "node:test";
import assert from "node:assert/strict";
import { boot, cssOf, styleOf } from "./dom.mjs";

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

test("middle, back and forward clicks are reported, without autoscroll (#215)", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" }, { op: "pointer", id: 2, mask: 128 },
    ...root([2]),
  ]);
  const el = placed(page.node(2));
  const down = { type: "mousedown", button: 1 };
  el.dispatchEvent(down);
  assert.equal(down.defaultPrevented, true, "no autoscroll");
  const left = { type: "mousedown", button: 0 };
  el.dispatchEvent(left);
  assert.equal(!!left.defaultPrevented, false, "the primary button is untouched");
  el.dispatchEvent({ type: "auxclick", button: 1, detail: 1, clientX: 60, clientY: 30, ctrlKey: true });
  assert.deepEqual(lastSent(page), { t: "auxclick", id: 2, button: "middle", mods: 2, n: 1, x: 10, y: 10 });
  el.dispatchEvent({ type: "auxclick", button: 3, detail: 1, clientX: 20, clientY: 20 });
  assert.equal(lastSent(page).button, "other:3");
  const n = sent(page).length;
  el.dispatchEvent({ type: "auxclick", button: 2, detail: 1, clientX: 20, clientY: 20 });
  assert.equal(sent(page).length, n, "the secondary button is the context menu's");
  page.ws.deliver({ t: "patch", p: [{ op: "pointer", id: 2, mask: 0 }] });
  el.dispatchEvent({ type: "auxclick", button: 1, detail: 1, clientX: 20, clientY: 20 });
  assert.equal(sent(page).length, n, "no longer listening");
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

test("a fast drag off a small handle still starts, and clicks inside stay clicks (#194)", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" }, { op: "pointer", id: 2, mask: 4 },
    { op: "create", id: 3, kind: "view" }, { op: "clickable", id: 3, on: true },
    { op: "children", id: 2, children: [3] },
    ...root([2]),
  ]);
  const handle = placed(page.node(2));
  const body = page.document.body;
  const p = (target, type, x, y) => target.dispatchEvent({ type, pointerId: 7, button: 0, clientX: x, clientY: y });
  p(handle, "pointerdown", 60, 30);
  // The first move past the slop is already off the handle.
  p(body, "pointermove", 140, 30);
  assert.deepEqual(lastSent(page), { t: "drag", id: 2, x: 10, y: 10, dx: 80, dy: 0, phase: "start" });
  p(body, "pointerup", 150, 30);
  assert.deepEqual(lastSent(page), { t: "drag", id: 2, phase: "end", x: 10, y: 10, dx: 90, dy: 0 });
  // Released: moves elsewhere no longer drag it.
  const n = sent(page).length;
  p(body, "pointermove", 200, 30);
  assert.equal(sent(page).length, n);
  // A press on a child that does not move: no capture, the child clicks.
  page.timers.splice(0).forEach((t) => t.fn());
  p(page.node(3), "pointerdown", 61, 31);
  p(page.node(3), "pointerup", 61, 31);
  page.node(3).dispatchEvent({ type: "click", detail: 1, clientX: 61, clientY: 31 });
  assert.equal(lastSent(page).t, "click");
  assert.equal(lastSent(page).id, 3);
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

test("the middle and secondary buttons drag where allowed, with their keys (#136)", () => {
  const page = live([
    // Middle-button panning; the primary button does not drag here.
    { op: "create", id: 2, kind: "view" }, { op: "pointer", id: 2, mask: 32 },
    { op: "create", id: 3, kind: "view" }, { op: "pointer", id: 3, mask: 64 },
    ...root([2, 3]),
  ]);
  const canvas = placed(page.node(2));
  // Not selectable, whatever button drags it (#142).
  assert.match(canvas.className, /prism-draggable/);
  assert.match(page.node(3).className, /prism-draggable/);
  const p = (el, type, button, x, y, more = {}) => {
    const e = { type, pointerId: 1, button, clientX: x, clientY: y, ...more };
    el.dispatchEvent(e);
    return e;
  };
  const drags = () => sent(page).filter((m) => m.t === "drag");
  p(canvas, "pointerdown", 0, 60, 30);
  p(canvas, "pointermove", 0, 80, 30);
  p(canvas, "pointerup", 0, 80, 30);
  assert.equal(drags().length, 0, "the primary button selects");
  assert.ok(p(canvas, "pointerdown", 1, 60, 30, { shiftKey: true }).defaultPrevented, "no autoscroll");
  assert.ok(p(canvas, "mousedown", 1, 60, 30).defaultPrevented);
  p(canvas, "pointermove", 1, 70, 30);
  assert.deepEqual(lastSent(page), { t: "drag", id: 2, x: 10, y: 10, dx: 10, dy: 0, button: "middle", mods: 1, phase: "start" });
  p(canvas, "pointerup", 1, 75, 30);
  assert.deepEqual(lastSent(page), { t: "drag", id: 2, phase: "end", x: 10, y: 10, dx: 15, dy: 0, button: "middle", mods: 1 });
  assert.ok(p(canvas, "auxclick", 1, 75, 30).defaultPrevented, "no new tab, no paste");
  // The secondary button: a drag, then no context menu; a plain press keeps it.
  const pad = placed(page.node(3));
  p(pad, "pointerdown", 2, 60, 30);
  p(pad, "pointermove", 2, 60, 50);
  p(pad, "pointerup", 2, 60, 50);
  assert.equal(lastSent(page).button, "secondary");
  assert.ok(p(pad, "contextmenu", 2, 60, 50).defaultPrevented);
  assert.equal(p(pad, "contextmenu", 2, 60, 50).defaultPrevented, false);
});

test("clicks say their keys and count; released keys are reported (#159)", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" }, { op: "clickable", id: 2, on: true },
    { op: "create", id: 3, kind: "view" }, { op: "keys", id: 3, local: false, global: true },
    ...root([2, 3]),
  ]);
  const el = placed(page.node(2));
  el.dispatchEvent({ type: "click", detail: 2, clientX: 80, clientY: 45, metaKey: true });
  assert.deepEqual(lastSent(page), { t: "click", id: 2, x: 30, y: 25, mods: 8, n: 2 });
  el.dispatchEvent({ type: "click", detail: 1, clientX: 80, clientY: 45 });
  assert.deepEqual(lastSent(page), { t: "click", id: 2, x: 30, y: 25 }, "a plain click stays plain");
  // Hold Space: down, then up to the same listeners.
  page.document.body.dispatchEvent({ type: "keydown", key: " " });
  page.document.body.dispatchEvent({ type: "keyup", key: " " });
  assert.deepEqual(lastSent(page), { t: "keyup", id: 0, key: " ", mods: 0 });
  // A release whose press was not reported says nothing.
  const count = sent(page).length;
  page.document.body.dispatchEvent({ type: "keyup", key: "x" });
  assert.equal(sent(page).length, count);
  // Losing the window releases the keys still held.
  page.document.body.dispatchEvent({ type: "keydown", key: "Shift", shiftKey: true });
  page.window.dispatchEvent({ type: "blur" });
  assert.deepEqual(lastSent(page), { t: "keyup", id: 0, key: "Shift", mods: 0 });
});

test("the app menus' shortcuts are taken before the browser (#161)", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" }, { op: "keys", id: 2, local: false, global: true },
    ...root([2]),
  ]);
  page.ws.deliver({ t: "shortcuts", list: [["cmd+z", 7], ["cmd+shift+z", 8]] });
  // Not an Apple platform here: `cmd` is Ctrl.
  const key = (init) => {
    const e = { type: "keydown", key: "z", defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...init };
    page.window.dispatchEvent(e);
    if (!e.prismShortcut) page.document.body.dispatchEvent(e);
    return e;
  };
  const undo = key({ ctrlKey: true });
  assert.equal(undo.defaultPrevented, true);
  assert.deepEqual(lastSent(page), { t: "menu", id: 7 }, "the item runs, the key is not reported");
  key({ key: "Z", ctrlKey: true, shiftKey: true });
  assert.deepEqual(lastSent(page), { t: "menu", id: 8 });
  // Other keys go on as keys.
  const plain = key({ key: "z" });
  assert.equal(plain.defaultPrevented, false);
  assert.equal(lastSent(page).t, "key");
});

test("an app menu's real id comes back exact from its shortcut (#170)", () => {
  const page = live(root([]));
  // As the server sends it (the first app menu item, `app_menu_shortcut_ids_are_exact_in_the_page`):
  // the page parses the text, so an id above 2^53 would be rounded here.
  const id = "4503599627370497";
  page.ws.deliverText(`{"t":"shortcuts","list":[["cmd+k",${id}]]}`);
  const e = { type: "keydown", key: "k", ctrlKey: true, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; } };
  page.window.dispatchEvent(e);
  assert.equal(e.defaultPrevented, true);
  const out = page.ws.sentText.at(-1);
  assert.equal(out, `{"t":"menu","id":${id}}`, "the server finds the item's handler");
});

test("consumed keys are reported, then kept from the focused field (#181)", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" }, { op: "keys", id: 2, local: true, global: false },
    { op: "consume", id: 2, keys: ["ArrowDown", "Enter"] },
    { op: "create", id: 3, kind: "text_input" },
    { op: "children", id: 2, children: [3] },
    ...root([2]),
  ]);
  const field = page.node(3);
  // The field's own handler (submitting on Enter) must not run.
  let fieldSaw = 0;
  field.addEventListener("keydown", () => fieldSaw++);
  const key = (k) => {
    const e = { type: "keydown", key: k };
    field.dispatchEvent(e);
    return e;
  };
  const down = key("ArrowDown");
  assert.equal(down.defaultPrevented, true, "the caret does not move");
  assert.equal(lastSent(page).key, "ArrowDown");
  assert.equal(lastSent(page).id, 2);
  assert.equal(sent(page).filter((m) => m.t === "key").length, 1, "reported once");
  key("Enter");
  assert.equal(fieldSaw, 0, "the field never sees them");
  // Other keys type as usual.
  const a = key("ArrowLeft");
  assert.equal(a.defaultPrevented, false);
  assert.equal(fieldSaw, 1);
  // The menu closed: the field gets its keys back.
  page.ws.deliver({ t: "patch", p: [{ op: "consume", id: 2, keys: [] }] });
  assert.equal(key("ArrowDown").defaultPrevented, false);
  assert.equal(fieldSaw, 2);
});

test("keys are taken from a field only at the caret edges declared (#211)", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" }, { op: "keys", id: 2, local: true, global: false },
    { op: "consume", id: 2, keys: ["ArrowUp@first-line", "ArrowDown@last-line", "ArrowLeft@start", "ArrowRight@end"] },
    { op: "create", id: 3, kind: "text_area" },
    { op: "children", id: 2, children: [3] },
    ...root([2]),
  ]);
  const field = page.node(3);
  field.value = "one\ntwo";
  const key = (k, at, to = at) => {
    field.selectionStart = at;
    field.selectionEnd = to;
    const e = { type: "keydown", key: k };
    field.dispatchEvent(e);
    return e;
  };
  // The caret on the first line: ↑ is the app's, ↓ moves the caret.
  assert.equal(key("ArrowUp", 2).defaultPrevented, true);
  assert.equal(lastSent(page).edges, "f");
  assert.equal(key("ArrowDown", 2).defaultPrevented, false);
  // On the last line: the other way round.
  assert.equal(key("ArrowDown", 5).defaultPrevented, true);
  assert.equal(key("ArrowUp", 5).defaultPrevented, false);
  assert.equal(lastSent(page).edges, "l");
  // ← at the start, → at the end; not inside.
  assert.equal(key("ArrowLeft", 0).defaultPrevented, true);
  assert.equal(lastSent(page).edges, "sf");
  assert.equal(key("ArrowLeft", 1).defaultPrevented, false);
  assert.equal(key("ArrowRight", 7).defaultPrevented, true);
  assert.equal(lastSent(page).edges, "el");
  // Text selected: the key acts on the selection.
  assert.equal(key("ArrowUp", 0, 3).defaultPrevented, false);
  assert.equal(lastSent(page).edges, "");
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
  // In a field, with its selection (#86) and where its caret is (#211):
  // an empty field's caret is at every edge.
  assert.deepEqual(lastSent(page), { t: "key", id: 0, key: "Escape", mods: 0, repeat: false, sel: [0, 0], edges: "sefl" });
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

test("the app can dismiss the keyboard (#29)", () => {
  const page = live([{ op: "create", id: 2, kind: "text_input" }, ...root([2])]);
  page.ws.deliver({ t: "patch", p: [{ op: "focus", id: 2 }] });
  assert.equal(page.document.activeElement, page.node(2));
  page.ws.deliver({ t: "patch", p: [{ op: "blur" }] });
  assert.equal(page.document.activeElement, page.document.body);
  // Nothing focused: nothing to do.
  page.ws.deliver({ t: "patch", p: [{ op: "blur" }] });
  assert.equal(page.document.activeElement, page.document.body);
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

test("the first paint's layers stay until the live tree replaces them (#93)", () => {
  const page = boot({ layers: `<div data-pid="7" data-kind="view"><div data-pid="8" data-kind="text">Signed in</div></div>` });
  const host = page.document.getElementById("prism-layers");
  assert.equal(host.textContent, "Signed in", "shown before the socket connects");
  page.ws.open();
  page.ws.deliver({ t: "reset", p: [
    { op: "create", id: 2, kind: "view" }, { op: "create", id: 3, kind: "text" }, { op: "text", id: 3, text: "Signed in" },
    { op: "children", id: 2, children: [3] },
    ...root([]),
    { op: "layers", ids: [2] },
  ] });
  assert.equal(page.document.getElementById("prism-layers"), host, "the page's host is reused");
  assert.equal(host.childNodes.length, 1);
  assert.equal(host.firstChild, page.node(2));
  // A live session without layers clears the first paint's.
  const empty = boot({ layers: `<div data-pid="7" data-kind="view">Old</div>` });
  empty.ws.open();
  empty.ws.deliver({ t: "reset", p: root([]) });
  assert.equal(empty.document.getElementById("prism-layers").childNodes.length, 0);
});

test("a dialog keeps the focus the app asked for, not its first control (#148)", () => {
  // `dialog(…).open(true)` with a button, then an autofocused field, all
  // created in one render: the dialog opens at the end of the batch.
  const page = live([...root([])]);
  page.ws.deliver({ t: "patch", p: [
    { op: "create", id: 2, kind: "overlay" }, { op: "config", id: 2, config: "dialog" },
    { op: "value", id: 2, value: 1 },
    { op: "create", id: 3, kind: "button" }, { op: "text", id: 3, text: "x" },
    { op: "create", id: 4, kind: "text_input" },
    { op: "children", id: 2, children: [3, 4] },
    { op: "focus", id: 4 },
    { op: "children", id: 1, children: [2] },
  ] });
  assert.equal(page.node(2).firstChild.hasAttribute("open"), true);
  const field = page.node(4);
  assert.equal(page.document.activeElement, field._input || field);
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
  assert.deepEqual(lastSent(page), { t: "back", id: 2, depth: 0, reason: "escape" });
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
  assert.deepEqual(lastSent(page), { t: "back", id: 3, depth: 0, reason: "outside" });
  // Closed right after Escape: the reason is Escape (#156).
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 3, value: 1 }] });
  page.document.dispatchEvent({ type: "keydown", key: "Escape" });
  box.dispatchEvent({ type: "toggle", newState: "closed" });
  assert.deepEqual(lastSent(page), { t: "back", id: 3, depth: 0, reason: "escape" });
});

test("popovers open on the side and alignment they ask for, flipping without room (#113)", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" },
    { op: "create", id: 3, kind: "overlay" }, { op: "config", id: 3, config: "popover side=trailing" },
    { op: "children", id: 2, children: [3] },
    ...root([2]),
  ]);
  // The anchor: 200 × 100 at (50, 20); the popover: 100 × 60.
  placed(page.node(2));
  const box = page.node(3).firstChild;
  box.getBoundingClientRect = () => ({ left: 0, top: 0, width: 100, height: 60, right: 100, bottom: 60 });
  const at = (config) => {
    page.ws.deliver({ t: "patch", p: [{ op: "config", id: 3, config }, { op: "value", id: 3, value: 1 }] });
    return [box.style.left, box.style.top, box.dataset.side];
  };
  // A submenu: beside the anchor, tops lined up.
  assert.deepEqual(at("popover side=trailing"), ["256px", "20px", "right"]);
  assert.deepEqual(at("popover side=trailing align=center"), ["256px", "40px", "right"]);
  assert.deepEqual(at("popover side=trailing align=end"), ["256px", "60px", "right"]);
  // Below, trailing edges lined up: it grows towards the start.
  assert.deepEqual(at("popover align=end"), ["150px", "126px", "bottom"]);
  assert.deepEqual(at("popover align=center"), ["100px", "126px", "bottom"]);
  // Above: the window has room there (20 px above the anchor is too little,
  // so it flips below).
  assert.deepEqual(at("popover side=top"), ["50px", "126px", "bottom"]);
  // Leading: no room left of the anchor (50 px), so it flips to the right.
  assert.deepEqual(at("popover side=leading"), ["256px", "20px", "right"]);
  // Still works with light_dismiss(false) / take_focus(false) (#70).
  assert.deepEqual(at("popover manual keepfocus side=trailing"), ["256px", "20px", "right"]);
  assert.equal(box.getAttribute("popover"), "manual");
  // Near the window's right edge (1280 wide): a trailing submenu flips to
  // the leading side.
  page.node(2).getBoundingClientRect = () => ({ left: 1000, top: 300, width: 200, height: 30, right: 1200, bottom: 330 });
  assert.deepEqual(at("popover side=trailing"), ["894px", "300px", "left"]);
  // Room above: on top, ending at the anchor's top with the gap.
  assert.deepEqual(at("popover side=top align=end"), ["1100px", "234px", "top"]);
  // Right to left: trailing is the left side, start the right edge.
  page.document.documentElement.dir = "rtl";
  assert.deepEqual(at("popover side=trailing"), ["894px", "300px", "left"]);
  assert.deepEqual(at("popover"), ["1100px", "336px", "bottom"]);
  assert.deepEqual(at("popover align=end"), ["1000px", "336px", "bottom"]);
  page.document.documentElement.dir = "";
});

test("a popover inside an open popover is placed against its own anchor (#113)", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" },
    { op: "create", id: 3, kind: "overlay" }, { op: "config", id: 3, config: "popover" }, { op: "value", id: 3, value: 1 },
    { op: "create", id: 4, kind: "view" },
    { op: "create", id: 5, kind: "overlay" }, { op: "config", id: 5, config: "popover side=trailing" }, { op: "value", id: 5, value: 1 },
    { op: "children", id: 4, children: [5] },
    { op: "children", id: 3, children: [4] },
    { op: "children", id: 2, children: [3] },
    ...root([2]),
  ]);
  placed(page.node(2));
  const outer = page.node(3).firstChild, inner = page.node(5).firstChild;
  outer.getBoundingClientRect = () => ({ left: 0, top: 0, width: 220, height: 200, right: 220, bottom: 200 });
  // The item in the menu, where the menu sits (under the anchor).
  page.node(4).getBoundingClientRect = () => ({ left: 54, top: 130, width: 212, height: 28, right: 266, bottom: 158 });
  inner.getBoundingClientRect = () => ({ left: 0, top: 0, width: 160, height: 120, right: 160, bottom: 120 });
  flushFrames(page);
  assert.equal(outer.matches(":popover-open"), true);
  assert.equal(inner.matches(":popover-open"), true, "both stay open");
  assert.deepEqual([outer.style.left, outer.style.top], ["50px", "126px"]);
  assert.deepEqual([inner.style.left, inner.style.top], ["272px", "130px"], "beside its item, tops lined up");
});

test("a dialog opens beside its anchor, undimmed, at its size (#79)", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" },
    { op: "create", id: 3, kind: "overlay" },
    { op: "config", id: 3, config: "dialog side=trailing placed backdrop=clear size=320x0" },
    { op: "children", id: 2, children: [3] },
    ...root([2]),
  ]);
  placed(page.node(2));
  const box = page.node(3).firstChild;
  box.getBoundingClientRect = () => ({ left: 0, top: 0, width: 320, height: 400, right: 320, bottom: 400 });
  assert.equal(box.dataset.prismBackdrop, "clear", "no dim, no blur");
  assert.equal(box.dataset.prismW, "1");
  assert.equal(box.style.getPropertyValue("--prism-ov-w"), "320px");
  assert.equal(box.dataset.prismH, undefined, "the height fits the content");
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 3, value: 1 }] });
  assert.equal(box.hasAttribute("open"), true, "still a modal dialog");
  assert.equal(box.dataset.prismPlaced, "1");
  assert.equal(box.style.left, "258px", "8 px after the anchor");
  assert.equal(box.style.top, "20px", "aligned with its top");
  // Right to left: trailing is on the left, where there is no room (the
  // anchor starts at 50): the other side.
  page.document.documentElement.dir = "rtl";
  page.window.dispatchEvent({ type: "resize" });
  assert.equal(box.style.left, "258px");
  page.document.documentElement.dir = "";
  // A click outside it (on the clear backdrop) still closes it.
  box.dispatchEvent({ type: "click", target: box, clientX: 900, clientY: 700 });
  assert.deepEqual(lastSent(page), { t: "back", id: 3, depth: 0, reason: "outside" });
  // No room on either side (a phone): centered as usual.
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 3, value: 1 }] });
  page.window.innerWidth = 390;
  page.window.dispatchEvent({ type: "resize" });
  assert.equal(box.dataset.prismPlaced, undefined);
  assert.equal(box.style.left, "");
  // Without a placement it is a centered dialog again (a side alone does
  // not place a dialog: `placed` does).
  page.window.innerWidth = 1280;
  page.ws.deliver({ t: "patch", p: [{ op: "config", id: 3, config: "dialog side=trailing backdrop=dim" }] });
  page.window.dispatchEvent({ type: "resize" });
  assert.equal(box.dataset.prismPlaced, undefined);
  assert.equal(box.dataset.prismBackdrop, "dim");
  assert.equal(box.dataset.prismW, undefined);
});

test("a bare dialog drops Prism's box and stays modal (#166)", async () => {
  const page = live([
    { op: "create", id: 2, kind: "view" },
    { op: "create", id: 3, kind: "overlay" },
    { op: "config", id: 3, config: "dialog side=trailing placed backdrop=clear bare" },
    { op: "children", id: 2, children: [3] },
    ...root([2]),
  ]);
  placed(page.node(2));
  const box = page.node(3).firstChild;
  box.getBoundingClientRect = () => ({ left: 0, top: 0, width: 320, height: 400, right: 320, bottom: 400 });
  assert.equal(box.dataset.prismBare, "1");
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 3, value: 1 }] });
  assert.equal(box.hasAttribute("open"), true, "still a modal dialog");
  assert.equal(box.dataset.prismPlaced, "1");
  box.dispatchEvent({ type: "click", target: box, clientX: 900, clientY: 700 });
  assert.deepEqual(lastSent(page), { t: "back", id: 3, depth: 0, reason: "outside" });
  // Prism's box back without `bare`; popovers and sheets are never bare.
  page.ws.deliver({ t: "patch", p: [{ op: "config", id: 3, config: "dialog side=trailing placed" }] });
  assert.equal(box.dataset.prismBare, "");
  page.ws.deliver({ t: "patch", p: [{ op: "config", id: 3, config: "sheet bare" }] });
  assert.equal(page.node(3).firstChild.dataset.prismBare, "");
  // The stylesheet takes the box away: background, border, corners, shadow, animation.
  const { readFile } = await import("node:fs/promises");
  const css = await readFile(new URL("../../src/prism.css", import.meta.url), "utf8");
  const rule = css.split("\n").find((l) => l.startsWith('dialog.prism-overlay-box[data-prism-bare="1"] {'));
  assert.ok(rule, "a rule for bare dialogs");
  for (const d of ["background: transparent", "border: 0", "border-radius: 0", "box-shadow: none", "animation: none"]) assert.ok(rule.includes(d), d);
  assert.ok(css.indexOf(rule) > css.indexOf('[data-prism-backdrop="clear"] {'), "after the clear backdrop's border and shadow");
});

test("an overlay inserted already open is shown (#70)", () => {
  const page = live([...root([])]);
  // A menu that appears with its parent, open, in one patch.
  page.ws.deliver({ t: "patch", p: [
    { op: "create", id: 2, kind: "view" },
    { op: "create", id: 3, kind: "overlay" }, { op: "config", id: 3, config: "popover" }, { op: "value", id: 3, value: 1 },
    { op: "create", id: 4, kind: "text" }, { op: "text", id: 4, text: "/heading" },
    { op: "children", id: 3, children: [4] },
    { op: "children", id: 2, children: [3] },
    { op: "create", id: 5, kind: "overlay" }, { op: "config", id: 5, config: "dialog" }, { op: "value", id: 5, value: 1 },
    { op: "children", id: 1, children: [2, 5] },
  ] });
  const box = page.node(3).firstChild;
  assert.equal(box.isConnected, true);
  assert.equal(box.matches(":popover-open"), true, "the popover is shown");
  assert.equal(box.textContent, "/heading");
  const dialog = page.node(5).firstChild;
  assert.equal(dialog.hasAttribute("open"), true, "the dialog is shown (modal)");
  // Closing and reopening still works.
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 3, value: 0 }] });
  assert.equal(box.matches(":popover-open"), false);
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 3, value: 1 }] });
  assert.equal(box.matches(":popover-open"), true);
});

test("an overlay open on the first render is shown", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" },
    { op: "create", id: 3, kind: "overlay" }, { op: "config", id: 3, config: "popover" }, { op: "value", id: 3, value: 1 },
    { op: "children", id: 2, children: [3] },
    ...root([2]),
  ]);
  assert.equal(page.node(3).firstChild.matches(":popover-open"), true);
});

test("a popover can leave the focus to its anchor's field (#70)", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" },
    { op: "create", id: 3, kind: "text_input" },
    { op: "create", id: 4, kind: "overlay" }, { op: "config", id: 4, config: "popover manual keepfocus" },
    { op: "children", id: 2, children: [3, 4] },
    ...root([2]),
  ]);
  const field = page.node(3);
  field.focus();
  const box = page.node(4).firstChild;
  assert.equal(box.getAttribute("popover"), "manual", "no light dismiss");
  page.ws.deliver({ t: "patch", p: [
    { op: "value", id: 4, value: 1 },
    { op: "create", id: 5, kind: "view" }, { op: "clickable", id: 5, on: true }, { op: "text", id: 5, text: "Heading" },
    { op: "children", id: 4, children: [5] },
  ] });
  assert.equal(box.matches(":popover-open"), true);
  assert.equal(page.document.activeElement, field, "the field keeps the focus");
  // A press on an item does not take the focus; its click still runs.
  const item = page.node(5);
  const down = { type: "mousedown" };
  item.dispatchEvent(down);
  assert.equal(down.defaultPrevented, true);
  item.dispatchEvent({ type: "click", detail: 0 });
  assert.deepEqual(lastSent(page), { t: "click", id: 5 });
  // Escape inside a manual popover closes it (the app hears back).
  box.dispatchEvent({ type: "keydown", key: "Escape" });
  assert.deepEqual(lastSent(page), { t: "back", id: 4, depth: 0, reason: "escape" });
  // Leaving the focus: the browser's light dismiss stays off (#182 closes
  // it on presses around instead).
  page.ws.deliver({ t: "patch", p: [{ op: "config", id: 4, config: "popover keepfocus" }] });
  assert.equal(box.getAttribute("popover"), "manual");
  // Without `keepfocus` presses inside behave as usual.
  page.ws.deliver({ t: "patch", p: [{ op: "config", id: 4, config: "popover" }, { op: "value", id: 4, value: 1 }] });
  assert.equal(box.getAttribute("popover"), "auto");
  const plain = { type: "mousedown" };
  item.dispatchEvent(plain);
  assert.equal(plain.defaultPrevented, false);
});

test("a popover that leaves the focus closes on a press around it, which goes on (#182)", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" },
    { op: "create", id: 3, kind: "text_input" },
    { op: "create", id: 4, kind: "overlay" }, { op: "config", id: 4, config: "popover keepfocus" },
    { op: "create", id: 5, kind: "view" }, { op: "text", id: 5, text: "Heading" },
    { op: "children", id: 4, children: [5] },
    { op: "children", id: 2, children: [3, 4] },
    ...root([2]),
  ]);
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 4, value: 1 }] });
  const box = page.node(4).firstChild;
  assert.equal(box.matches(":popover-open"), true);
  const press = (target) => {
    const e = { type: "pointerdown" };
    target.dispatchEvent(e);
    return e;
  };
  const backs = () => sent(page).filter((m) => m.t === "back").length;
  // Inside: it stays.
  press(page.node(5));
  assert.equal(backs(), 0);
  // On the anchor's field: it closes, and the press is not held.
  const onField = press(page.node(3));
  assert.deepEqual(lastSent(page), { t: "back", id: 4, depth: 0, reason: "outside" });
  assert.equal(onField.defaultPrevented, false, "the caret still moves");
  assert.equal(box.matches(":popover-open"), false);
  // `light_dismiss(false)` keeps it open.
  page.ws.deliver({ t: "patch", p: [{ op: "config", id: 4, config: "popover manual keepfocus" }, { op: "value", id: 4, value: 1 }] });
  press(page.node(3));
  assert.equal(backs(), 1);
  assert.equal(box.matches(":popover-open"), true);
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

test("drawn radios: checked state, one tab stop per group, arrows move the choice (#84)", () => {
  const radio = (id, checked, extra = {}) =>
    ({ op: "a11y", id, a11y: { role: "radio", label: `Plan ${id}`, checked, disabled: false, hidden: false, ...extra } });
  const page = live([
    { op: "create", id: 2, kind: "view" },
    { op: "a11y", id: 2, a11y: { role: "radiogroup", label: "Plan", checked: null, disabled: false, hidden: false } },
    ...[3, 4, 5].flatMap((id) => [{ op: "create", id, kind: "view" }, { op: "clickable", id, on: true }]),
    radio(3, false), radio(4, false), radio(5, false, { disabled: true }),
    { op: "children", id: 2, children: [3, 4, 5] },
    ...root([2]),
  ]);
  const [a, b, c] = [3, 4, 5].map((id) => page.node(id));
  assert.equal(page.node(2).getAttribute("role"), "radiogroup");
  assert.equal(a.getAttribute("role"), "radio");
  assert.equal(a.getAttribute("aria-checked"), "false");
  // Nothing checked: the first operable radio is the group's tab stop.
  assert.deepEqual([a, b, c].map((r) => r.getAttribute("tabindex")), ["0", "-1", null]);
  page.ws.deliver({ t: "patch", p: [radio(4, true)] });
  assert.equal(b.getAttribute("aria-checked"), "true");
  assert.deepEqual([a, b].map((r) => r.getAttribute("tabindex")), ["-1", "0"]);
  // Arrows move the focus and check the radio they land on, skipping the
  // disabled one and wrapping around.
  b.focus();
  b.dispatchEvent({ type: "keydown", key: "ArrowDown" });
  assert.equal(page.document.activeElement, a);
  assert.deepEqual(lastSent(page), { t: "click", id: 3 });
  assert.deepEqual([a, b].map((r) => r.getAttribute("tabindex")), ["0", "-1"]);
  const left = { type: "keydown", key: "ArrowLeft" };
  a.dispatchEvent(left);
  assert.equal(left.defaultPrevented, true);
  assert.deepEqual(lastSent(page), { t: "click", id: 4 });
  // Right to left: the left arrow goes forward.
  page.document.documentElement.dir = "rtl";
  b.dispatchEvent({ type: "keydown", key: "ArrowLeft" });
  assert.deepEqual(lastSent(page), { t: "click", id: 3 });
  page.document.documentElement.dir = "";
  // Other keys and radios outside a group are left alone.
  const n = sent(page).length;
  a.dispatchEvent({ type: "keydown", key: "Tab" });
  page.ws.deliver({ t: "patch", p: [{ op: "create", id: 6, kind: "view" }, radio(6, false), ...root([2, 6]).slice(1, 2)] });
  page.node(6).dispatchEvent({ type: "keydown", key: "ArrowDown" });
  assert.equal(sent(page).length, n);
  assert.equal(page.node(6).getAttribute("tabindex"), "0");
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
  assert.match(cssOf(el), /color:blue;.*--prism-icon/);
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

test("an optional date picker can be empty and cleared (#89)", () => {
  const page = live([
    { op: "create", id: 2, kind: "date_picker" }, { op: "config", id: 2, config: "date - - optional" },
    { op: "value", id: 2, value: null },
    { op: "create", id: 3, kind: "date_picker" }, { op: "config", id: 3, config: "date - -" },
    { op: "value", id: 3, value: 0 },
    ...root([2, 3]),
  ]);
  const el = page.node(2);
  assert.equal(el.type, "date");
  assert.equal(el.value, "");
  // Browsers offer to clear optional inputs only.
  assert.equal(el.required, false);
  assert.equal(page.node(3).required, true);
  // Picking a day fills it.
  el.value = "2026-11-18";
  el.dispatchEvent({ type: "change" });
  const picked = lastSent(page).v;
  assert.equal(new Date(picked * 1000).getDate(), 18);
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 2, value: picked }] });
  assert.equal(el.value, "2026-11-18");
  // The browser's clear (or every part erased) reports null.
  el.value = "";
  el.dispatchEvent({ type: "change" });
  assert.deepEqual(lastSent(page), { t: "change", id: 2, v: null });
  // The app's empty value does not write over parts being typed.
  let writes = 0;
  let typed = "";
  Object.defineProperty(el, "value", { get: () => typed, set: (v) => { writes += 1; typed = v; }, configurable: true });
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 2, value: null }] });
  assert.equal(writes, 0);
  // A picker that is not optional never reports an empty value.
  const count = sent(page).length;
  page.node(3).value = "";
  page.node(3).dispatchEvent({ type: "change" });
  assert.equal(sent(page).length, count);
});

test("a picker with a value has no empty option (#96)", () => {
  const page = live([
    { op: "create", id: 2, kind: "picker" }, { op: "config", id: 2, config: "English\nFrançais" },
    { op: "value", id: 2, value: 0 },
    { op: "create", id: 3, kind: "picker" }, { op: "config", id: 3, config: "Small\nLarge" },
    { op: "value", id: 3, value: -1 }, { op: "placeholder", id: 3, text: "Size" },
    ...root([2, 3]),
  ]);
  const texts = (el) => el.options.map((o) => o.textContent);
  const picked = page.node(2);
  assert.deepEqual(texts(picked), ["English", "Français"]);
  assert.equal(picked.selectedIndex, 0);
  picked.selectedIndex = 1;
  picked.dispatchEvent({ type: "change" });
  assert.deepEqual(lastSent(page), { t: "change", id: 2, v: 1 });
  // New options keep the choice, without an empty row.
  page.ws.deliver({ t: "patch", p: [{ op: "config", id: 2, config: "English\nFrançais\nDeutsch" }] });
  assert.deepEqual(texts(picked), ["English", "Français", "Deutsch"]);
  assert.equal(picked.selectedIndex, 1);
  // Nothing picked: the placeholder shows, until a choice is made.
  const empty = page.node(3);
  assert.deepEqual(texts(empty), ["Size", "Small", "Large"]);
  assert.equal(empty.selectedIndex, 0);
  empty.selectedIndex = 2;
  empty.dispatchEvent({ type: "change" });
  assert.deepEqual(lastSent(page), { t: "change", id: 3, v: 1 });
  assert.deepEqual(texts(empty), ["Small", "Large"]);
  assert.equal(empty.selectedIndex, 1);
  // Back to empty from the app.
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 3, value: -1 }] });
  assert.deepEqual(texts(empty), ["Size", "Small", "Large"]);
});

test("wheel and pinch go to the element, one report per frame (#133)", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" }, { op: "pointer", id: 2, mask: 8 | 16 },
    { op: "create", id: 3, kind: "view" }, { op: "pointer", id: 3, mask: 8 },
    ...root([2, 3]),
  ]);
  const canvas = placed(page.node(2));
  const wheel = (el, init) => {
    const e = { type: "wheel", deltaX: 0, deltaY: 0, deltaMode: 0, clientX: 80, clientY: 45, ...init };
    el.dispatchEvent(e);
    return e;
  };
  // Two trackpad steps in one frame: one report, deltas added, scrolling taken.
  assert.ok(wheel(canvas, { deltaX: 3, deltaY: 10 }).defaultPrevented);
  wheel(canvas, { deltaY: 5, clientX: 90 });
  assert.equal(sent(page).filter((m) => m.t === "wheel").length, 0, "waits for the frame");
  flushFrames(page);
  assert.deepEqual(lastSent(page), { t: "wheel", id: 2, dx: 3, dy: 15, x: 40, y: 25, mods: 0, phase: "none", precise: true });
  // A mouse wheel in lines, with Shift: points, not precise.
  wheel(canvas, { deltaY: 3, deltaMode: 1, shiftKey: true });
  flushFrames(page);
  assert.deepEqual(lastSent(page), { t: "wheel", id: 2, dx: 0, dy: 48, x: 30, y: 25, mods: 1, phase: "none", precise: false });
  // A trackpad pinch (Ctrl + wheel): a scale, not a scroll.
  wheel(canvas, { deltaY: -10, ctrlKey: true });
  wheel(canvas, { deltaY: -10, ctrlKey: true });
  flushFrames(page);
  const pinch = lastSent(page);
  assert.equal(pinch.t, "magnify");
  assert.ok(Math.abs(pinch.scale - Math.exp(0.2)) < 1e-9, String(pinch.scale));
  // Safari's gesture events: the change since the last one.
  canvas.dispatchEvent({ type: "gesturestart", scale: 1, clientX: 80, clientY: 45 });
  assert.deepEqual(lastSent(page), { t: "magnify", id: 2, scale: 1, x: 30, y: 25, phase: "start" });
  canvas.dispatchEvent({ type: "gesturechange", scale: 2, clientX: 80, clientY: 45 });
  canvas.dispatchEvent({ type: "gestureend", scale: 3, clientX: 80, clientY: 45 });
  const [move, end] = sent(page).slice(-2);
  assert.deepEqual([move.phase, move.scale, end.phase, end.scale], ["move", 2, "end", 1.5]);
  // Wheel only: Ctrl + wheel is a wheel event with Ctrl held (the app zooms).
  const list = placed(page.node(3));
  assert.ok(wheel(list, { deltaY: -10, ctrlKey: true }).defaultPrevented);
  flushFrames(page);
  assert.deepEqual([lastSent(page).t, lastSent(page).id, lastSent(page).mods], ["wheel", 3, 2]);
  // Neither: the page scrolls.
  page.ws.deliver({ t: "patch", p: [{ op: "pointer", id: 3, mask: 0 }] });
  assert.equal(wheel(list, { deltaY: 10 }).defaultPrevented, false);
});

test("inert styles and hit testing", () => {
  const page = live([
    { op: "create", id: 2, kind: "view" }, { op: "style", id: 2, css: "pointer-events:none;", inert: true },
    ...root([2]),
  ]);
  const el = page.node(2);
  assert.equal(el.hasAttribute("inert"), true);
  assert.equal(styleOf(el).pointerEvents, "none");
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 2, css: "" }] });
  assert.equal(el.hasAttribute("inert"), false);
});
