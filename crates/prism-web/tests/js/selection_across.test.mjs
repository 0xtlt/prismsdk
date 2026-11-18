// One selection across several fields (#231): the client extends it over
// a group's fields with ⇧+arrows and drags, draws the other fields' parts,
// and sends copies and edits of the range to the server.
//
// Run: node --test crates/prism-web/tests/js/
import { test } from "node:test";
import assert from "node:assert/strict";
import { boot } from "./dom.mjs";

/** A group (2) of three text areas (3, 4, 5): "Hello", "big", "world". */
function doc({ edits = true } = {}) {
  const page = boot();
  page.ws.open();
  page.ws.deliver({
    t: "reset",
    p: [
      { op: "create", id: 1, kind: "view" },
      { op: "create", id: 2, kind: "view" },
      { op: "selgroup", id: 2, on: true, edits },
      { op: "create", id: 3, kind: "text_area" },
      { op: "create", id: 4, kind: "text_area" },
      { op: "create", id: 5, kind: "text_area" },
      { op: "children", id: 2, children: [3, 4, 5] },
      { op: "children", id: 1, children: [2] },
      { op: "root", id: 1 },
    ],
  });
  const [a, b, c] = [3, 4, 5].map((i) => page.node(i));
  a.value = "Hello";
  b.value = "big";
  c.value = "world";
  const key = (field, k, init = {}) => {
    const e = { type: "keydown", key: k, ...init };
    field.dispatchEvent(e);
    return e;
  };
  const sent = (t) => page.ws.sent.filter((m) => m.t === t);
  return { page, a, b, c, key, sent };
}

test("⇧↓ at the end of a field carries the selection into the next (#231)", () => {
  const { page, a, b, c, key, sent } = doc();
  assert.equal(page.node(2).dataset.selgroup, "");
  a.setSelectionRange(2, 2);
  // Not at the edge yet: ⇧→ is the field's.
  assert.equal(key(a, "ArrowRight", { shiftKey: true }).defaultPrevented, false);
  assert.equal(sent("across").length, 0);
  // A one-line text area: the caret is on its last line.
  const e = key(a, "ArrowDown", { shiftKey: true });
  assert.equal(e.defaultPrevented, true);
  assert.deepEqual(sent("across").at(-1), { t: "across", id: 2, a: [3, 2], h: [4, 3] });
  // The first field keeps its part selected; the next, whole, looks selected.
  assert.deepEqual([a.selectionStart, a.selectionEnd], [2, 5]);
  assert.equal(b.dataset.across, "");
  assert.equal(c.dataset.across, undefined);
  // ⇧→ goes on into the third field; ⇧← back.
  key(a, "ArrowRight", { shiftKey: true });
  assert.deepEqual(sent("across").at(-1).h, [5, 0]);
  key(a, "ArrowRight", { shiftKey: true });
  assert.deepEqual(sent("across").at(-1).h, [5, 1]);
  // The third field holds only a part: a text area can't show it.
  assert.equal(c.dataset.across, undefined);
  // ⇧↑ a field at a time, back into the first: given back to it.
  key(a, "ArrowUp", { shiftKey: true });
  key(a, "ArrowUp", { shiftKey: true });
  key(a, "ArrowUp", { shiftKey: true });
  assert.deepEqual(sent("across").at(-1), { t: "across", id: 2, a: [3, 2], h: [3, 0] });
  assert.deepEqual([a.selectionStart, a.selectionEnd], [0, 2]);
  assert.equal(b.dataset.across, undefined);
});

test("typing, copying and pasting over it go to the server (#231)", () => {
  const { a, b, key, sent } = doc();
  a.setSelectionRange(2, 2);
  key(a, "ArrowDown", { shiftKey: true });
  // Copy: the plain text of the whole range now, the app's content later.
  const data = {};
  const copy = { type: "copy", clipboardData: { setData: (t, v) => { data[t] = v; } } };
  a.dispatchEvent(copy);
  assert.equal(copy.defaultPrevented, true);
  assert.equal(data["text/plain"], "llo\nbig");
  assert.deepEqual(sent("acrosscopy").at(-1), { t: "acrosscopy", id: 2, cut: false });
  // Typing replaces the range: the app's.
  const x = key(a, "x");
  assert.equal(x.defaultPrevented, true);
  assert.deepEqual(sent("acrossedit").at(-1), { t: "acrossedit", id: 2, cause: "typing", text: "x" });
  assert.equal(b.dataset.across, undefined, "ended");
  // A paste over a new one.
  a.setSelectionRange(5, 5);
  key(a, "ArrowRight", { shiftKey: true });
  const paste = { type: "paste", clipboardData: { getData: () => "pasted" } };
  a.dispatchEvent(paste);
  assert.equal(paste.defaultPrevented, true);
  assert.deepEqual(sent("acrossedit").at(-1), { t: "acrossedit", id: 2, cause: "paste", text: "pasted" });
});

test("without on_edit_across, a key goes back to the first field (#231)", () => {
  const { a, b, key, sent } = doc({ edits: false });
  a.setSelectionRange(2, 2);
  key(a, "ArrowDown", { shiftKey: true });
  const e = key(a, "Backspace");
  assert.equal(e.defaultPrevented, false, "the field deletes its part");
  assert.deepEqual([a.selectionStart, a.selectionEnd], [2, 5]);
  assert.equal(b.dataset.across, undefined);
  assert.equal(sent("acrossedit").length, 0);
});

test("an arrow ends it at one end; the server's selection is drawn (#231)", () => {
  const { page, a, b, c, key, sent } = doc();
  a.setSelectionRange(2, 2);
  key(a, "ArrowDown", { shiftKey: true });
  const e = key(a, "ArrowDown");
  assert.equal(e.defaultPrevented, true);
  assert.deepEqual(sent("across").at(-1), { t: "across", id: 2, a: [4, 3], h: [4, 3] });
  assert.deepEqual([b.selectionStart, b.selectionEnd], [3, 3]);
  // The app selects from the first to the third field.
  page.ws.deliver({
    t: "patch",
    p: [
      { op: "sel", id: 3, start: 1, end: 5 },
      { op: "selhl", id: 4, start: 0, end: 3 },
      { op: "selhl", id: 5, start: 0, end: 5 },
      { op: "across", id: 2, a: [3, 1], h: [5, 5] },
    ],
  });
  assert.equal(b.dataset.across, "");
  assert.equal(c.dataset.across, "");
  // It extends from there.
  key(a, "ArrowLeft", { shiftKey: true });
  assert.deepEqual(sent("across").at(-1), { t: "across", id: 2, a: [3, 1], h: [5, 4] });
  assert.equal(c.dataset.across, undefined);
  // The server ends it.
  page.ws.deliver({ t: "patch", p: [{ op: "selhl", id: 4, start: 0, end: 0 }, { op: "across", id: 2 }] });
  assert.equal(b.dataset.across, undefined);
  assert.equal(key(a, "x").defaultPrevented, false);
});
