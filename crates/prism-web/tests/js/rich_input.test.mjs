// Rich inputs (#115): a contenteditable drawn from runs the client owns.
// Typing takes the style of the run before the caret (not a chip's or a
// link's), chips are one character, pastes are plain text, the app's runs
// follow the echo rules of text fields (#73) and selections are in
// characters (#86).
//
// Run: node --test crates/prism-web/tests/js/
import { test } from "node:test";
import assert from "node:assert/strict";
import { boot } from "./dom.mjs";

const RS = "\u001e", FS = "\u001f", OBJ = "￼";
/** Encoded runs: each a string or `[text, ...fields]`. */
const enc = (...runs) => runs.map((r) => (typeof r === "string" ? r : r.join(FS))).join(RS);
const BOLD = "w=700";
const CHIP = ["￼", "k=page:1", "n=Roadmap", "m=gear"];

/** A focused rich input showing `runs`, reporting its selection. */
function editor(runs, extra = []) {
  const page = boot();
  page.ws.open();
  page.ws.deliver({
    t: "reset",
    p: [{ op: "create", id: 1, kind: "rich_input" }, { op: "rich", id: 1, runs }, { op: "selev", id: 1, on: true }, ...extra, { op: "root", id: 1 }],
  });
  const el = page.node(1);
  el.focus();
  const patch = (...p) => page.ws.deliver({ t: "patch", p });
  // The server's echo of what was typed, then the app's selection (one
  // computed while typing is in flight is dropped, #86).
  const echo = () => { const m = page.ws.sent.filter((x) => x.t === "input").at(-1); if (m) patch({ op: "rich", id: 1, runs: m.runs }); };
  const select = (start, end = start) => { echo(); patch({ op: "sel", id: 1, start, end }); };
  const input = (inputType, data, more = {}) => el.dispatchEvent({ type: "beforeinput", inputType, data, ...more });
  const sent = () => page.ws.sent.filter((m) => m.t === "input");
  const last = () => sent().at(-1);
  const selection = () => {
    page.frames.splice(0).forEach((f) => f());
    const s = page.ws.sent.filter((m) => m.t === "select").at(-1);
    return s && [s.start, s.end];
  };
  return { page, el, patch, select, echo, input, sent, last, selection };
}

test("runs are drawn as spans, chips as one non-editable element (#115)", () => {
  const { el } = editor(enc("Hello ", ["world", BOLD], ["docs", "u", "l=9", "h=/docs"], CHIP));
  assert.equal(el.getAttribute("contenteditable"), "true");
  assert.equal(el.getAttribute("role"), "textbox");
  const [plain, bold, link, chip] = el.childNodes;
  assert.equal(plain.textContent, "Hello ");
  assert.equal(bold.textContent, "world");
  assert.match(el.ownerDocument.getElementById("prism-styles").sheet.cssRules.map((r) => r.style.cssText).join(), /font-weight:700/);
  assert.ok(link.classList.contains("prism-link"));
  assert.equal(link.dataset.link, "9");
  assert.equal(chip.className, "prism-chip");
  assert.equal(chip.getAttribute("contenteditable"), "false");
  assert.equal(chip.textContent, "⚙Roadmap", "its icon, then its label");
  assert.equal(el.dataset.empty, undefined);
  // Empty: one line (a <br>) and the placeholder.
  const empty = editor("");
  assert.equal(empty.el.dataset.empty, "");
  assert.equal(empty.el.childNodes.length, 1);
  assert.equal(empty.el.firstChild.localName, "br");
  empty.patch({ op: "placeholder", id: 1, text: "Write…" });
  assert.equal(empty.el.dataset.placeholder, "Write…");
});

test("typing takes the style of the run before the caret, not of a link or a chip (#115)", () => {
  const e = editor(enc("Hello ", ["world", BOLD]));
  e.select(11);
  e.input("insertText", "!");
  assert.deepEqual(e.last(), { t: "input", id: 1, v: "Hello world!", runs: enc("Hello ", ["world!", BOLD]) });
  assert.equal(e.el.childNodes[1].textContent, "world!", "drawn in the bold run");
  // At the start of the bold run: the plain run before it.
  e.select(6);
  e.input("insertText", "big ");
  assert.equal(e.last().runs, enc("Hello big ", ["world!", BOLD]));
  assert.deepEqual(e.selection(), [10, 10], "the caret after what was typed");
  // At the very start: the first run.
  const b = editor(enc(["bold", BOLD], " text"));
  b.select(0);
  b.input("insertText", "A");
  assert.equal(b.last().runs, enc(["Abold", BOLD], " text"));

  // After a link: plain text; inside it: the link.
  const l = editor(enc("See ", ["docs", "u", "l=3", "h=/docs"]));
  l.select(8);
  l.input("insertText", "!");
  assert.equal(l.last().runs, enc("See ", ["docs", "u", "h=/docs"], "!"));
  l.select(5);
  l.input("insertText", "x");
  assert.equal(l.last().runs, enc("See ", ["dxocs", "u", "h=/docs"], "!"));

  // After a chip: the run after it; between two chips: plain.
  const c = editor(enc(CHIP, ["next", "i"]));
  c.select(1);
  c.input("insertText", "a");
  assert.equal(c.last().runs, enc(CHIP, ["anext", "i"]));
  const cc = editor(enc(CHIP, CHIP));
  cc.select(1);
  cc.input("insertText", " ");
  assert.equal(cc.last().runs, enc(CHIP, " ", CHIP));
  // Over a selection: the style of its first character.
  const s = editor(enc("a", ["bold", BOLD], "z"));
  s.select(1, 5);
  s.input("insertText", "B");
  assert.equal(s.last().runs, enc("a", ["B", BOLD], "z"));
});

test("an icon is one non-editable character drawn as its glyph; underlines keep their line (#221)", () => {
  const BELL = [OBJ, "g=gear", "c=#E03131FF"];
  const DUE = ["Friday", "u", "uc=#E8A317FF", "us=thick"];
  const e = editor(enc("Due ", BELL, DUE));
  const [, icon, due] = e.el.childNodes;
  assert.ok(icon.classList.contains("prism-symbol"));
  assert.equal(icon.getAttribute("contenteditable"), "false");
  assert.equal(icon.textContent, "⚙", "its glyph");
  const css = e.el.ownerDocument.getElementById("prism-styles").sheet.cssRules.map((r) => r.style.cssText).join();
  assert.match(css, /text-decoration-color:\s*#E8A317FF/);
  assert.match(css, /text-decoration-thickness:\s*max\(2px,\s*\.12em\)/);
  // The caret counts it as one character; typing after it takes the run after.
  e.select(5);
  e.input("insertText", "x");
  assert.deepEqual(e.last(), { t: "input", id: 1, v: `Due ${OBJ}xFriday`, runs: enc("Due ", BELL, ["xFriday", ...DUE.slice(1)]) });
  // One Backspace deletes it.
  e.select(5);
  e.input("deleteContentBackward");
  assert.equal(e.last().v, "Due xFriday");
});

test("a chip is one character for the caret, the selection and deletion (#115)", () => {
  const e = editor(enc("a", CHIP, "b"));
  // Selection offsets count the chip as one.
  e.select(0, 2);
  assert.deepEqual(e.selection(), [0, 2]);
  assert.ok(e.el.childNodes[1].hasAttribute("data-selected"), "the chip looks selected");
  // The DOM selection is around the chip, not inside its label.
  const sel = e.page.document.getSelection();
  assert.ok(sel.focusNode === e.el || sel.focusNode === e.el.childNodes[2].firstChild);
  // Backspace after the chip deletes it whole.
  e.select(2);
  assert.ok(!e.el.childNodes[1].hasAttribute("data-selected"));
  e.input("deleteContentBackward");
  assert.deepEqual(e.last(), { t: "input", id: 1, v: "ab", runs: "ab" });
  assert.deepEqual(e.selection(), [1, 1]);
  // Delete before a chip deletes it too.
  const f = editor(enc("a", CHIP, "b"));
  f.select(1);
  f.input("deleteContentForward");
  assert.equal(f.last().runs, "ab");
  // A browser's own target range (a word, a grapheme) wins over one character.
  const w = editor(enc("one two"));
  w.select(7);
  const text = w.el.firstChild.firstChild;
  w.input("deleteWordBackward", null, { getTargetRanges: () => [{ startContainer: text, startOffset: 4, endContainer: text, endOffset: 7 }] });
  assert.equal(w.last().runs, "one ");
  // A point inside a chip's label counts as after it.
  const g = editor(enc("a", CHIP));
  const label = g.el.childNodes[1].lastChild;
  g.page.document.getSelection().setBaseAndExtent(label, 3, label, 3);
  g.el.dispatchEvent({ type: "keyup" });
  assert.deepEqual(g.selection(), [2, 2]);
});

test("pastes and drops are plain text; formatting commands do nothing (#115)", () => {
  const e = editor(enc(["Bold", BOLD]));
  e.select(4);
  const pasted = e.el.dispatchEvent({
    type: "paste",
    clipboardData: { getData: (t) => (t === "text/plain" ? ` and more${OBJ}\r\nlines` : "<b>x</b>") },
  });
  assert.equal(pasted, false, "the browser does not paste its HTML");
  assert.equal(e.last().runs, enc(["Bold and more\nlines", BOLD]), "plain, in the style where it lands, objects dropped");
  // A trailing line break shows its empty line.
  e.select(19);
  e.el.dispatchEvent({ type: "keydown", key: "Enter", shiftKey: true });
  assert.equal(e.last().v, "Bold and more\nlines\n");
  assert.equal(e.el.lastChild.localName, "br");
  const n = e.sent().length;
  assert.equal(e.input("formatBold"), false);
  assert.equal(e.input("formatItalic"), false);
  assert.equal(e.sent().length, n, "nothing sent");
  // Copy: plain text, a chip as its label.
  const c = editor(enc("See ", CHIP));
  c.select(0, 5);
  let copied = null;
  c.el.dispatchEvent({ type: "copy", clipboardData: { setData: (t, v) => { copied = [t, v]; } } });
  assert.deepEqual(copied, ["text/plain", "See Roadmap"]);
});

test("Enter submits and inserts nothing; Shift+Enter is a line break (#115)", () => {
  const e = editor(enc("Hi"));
  e.select(2);
  const enter = e.el.dispatchEvent({ type: "keydown", key: "Enter" });
  assert.equal(enter, false);
  assert.ok(e.page.ws.sent.some((m) => m.t === "submit" && m.id === 1));
  assert.equal(e.sent().length, 0);
  e.input("insertParagraph");
  assert.equal(e.page.ws.sent.filter((m) => m.t === "submit").length, 2, "a virtual keyboard's Enter too");
  e.el.dispatchEvent({ type: "keydown", key: "Enter", shiftKey: true });
  assert.equal(e.last().v, "Hi\n");
});

test("the app's runs: echoes leave the field alone, changes are applied (#73, #115)", () => {
  const e = editor("");
  e.select(0);
  e.input("insertText", "#");
  e.input("insertText", " ");
  const nodes = [...e.el.childNodes];
  // The echo of "#": the field has moved on, nothing is redrawn.
  e.patch({ op: "text", id: 1, text: "#" }, { op: "rich", id: 1, runs: "#" });
  assert.equal(e.el.textContent, "# ");
  assert.deepEqual([...e.el.childNodes], nodes);
  // The app turns "# " into a heading: an app change, applied while focused.
  e.patch({ op: "rich", id: 1, runs: enc(["", BOLD]) });
  assert.equal(e.el.textContent, "", "applied");
  e.input("insertText", "T");
  assert.equal(e.last().runs, "T");
  // The echo of the latest runs takes their new link ids, quietly.
  const l = editor(enc(["go", "l=1", "h=/a"]));
  l.select(2);
  l.input("insertText", "o", {});
  const linkNode = l.el.firstChild;
  assert.equal(linkNode.dataset.link, "1");
  l.patch({ op: "rich", id: 1, runs: enc(["goo", "l=7", "h=/a"]) });
  assert.equal(l.el.firstChild, linkNode, "the same node: the caret stays in it");
  assert.equal(linkNode.dataset.link, "7");
  assert.equal(l.el.textContent, "goo");
  // A value set by the app keeps the caret's distance from the end.
  const c = editor(enc("hello world"));
  c.select(5);
  c.patch({ op: "rich", id: 1, runs: enc(["Hello", BOLD], " world!") });
  assert.deepEqual(c.selection(), [6, 6], "one character more after it");
  assert.equal(c.el.childNodes[0].textContent, "Hello");
  // Its selection after its runs (`sel` follows `rich`).
  c.patch({ op: "rich", id: 1, runs: enc("abc") }, { op: "sel", id: 1, start: 0, end: 3 });
  assert.deepEqual(c.selection(), [0, 3]);
});

test("an IME composition is the browser's, read back when it ends (#115)", () => {
  const e = editor(enc(["ab", BOLD]));
  e.select(2);
  e.el.dispatchEvent({ type: "compositionstart" });
  // The browser composes in the bold run's text; nothing is sent meanwhile.
  assert.equal(e.input("insertCompositionText", "k"), true, "not prevented");
  const text = e.el.firstChild.firstChild;
  text.data = "abか";
  e.el.dispatchEvent({ type: "input", inputType: "insertCompositionText", isComposing: true });
  assert.equal(e.sent().length, 0);
  // An app value during the composition waits for its end.
  e.patch({ op: "rich", id: 1, runs: enc("app") });
  assert.equal(e.el.textContent, "abか");
  e.page.document.getSelection().setBaseAndExtent(text, 3, text, 3);
  e.el.dispatchEvent({ type: "compositionend" });
  assert.equal(e.sent()[0].runs, enc(["abか", BOLD]), "the composed text, in its run");
  assert.equal(e.el.textContent, "app", "then the app's value");
  // Text the browser put outside the runs is plain.
  const p = editor(enc(CHIP));
  p.el.insertBefore(p.page.document.createTextNode("x"), p.el.firstChild);
  p.page.document.getSelection().setBaseAndExtent(p.el, 1, p.el, 1);
  p.el.dispatchEvent({ type: "input", inputType: "insertText" });
  assert.equal(p.last().runs, enc("x", CHIP));
  assert.equal(p.el.firstChild.localName, "span", "redrawn as runs");
});

test("undo and redo step through the field's own edits (#115)", () => {
  const e = editor(enc("a"));
  e.select(1);
  e.input("insertText", "b");
  e.input("insertText", "c");
  e.input("insertText", " ");
  e.input("insertText", "d");
  e.input("historyUndo");
  assert.equal(e.last().v, "abc", "typing undoes a word at a time");
  e.input("historyUndo");
  assert.equal(e.last().v, "a");
  e.input("historyRedo");
  assert.equal(e.last().v, "abc");
  e.input("historyRedo");
  assert.equal(e.last().v, "abc d");
});

/** A key pressed in field `el`, as the browser sends it (window, document, field). */
function press(el, key, mods = {}) {
  const e = { type: "keydown", key, ...mods };
  el.dispatchEvent(e);
  return e;
}

test("the history survives the app's answer to an edit; new content starts it over (#338)", () => {
  // Backspace deletes the chip; the app answers with its own runs (it
  // normalises them): not an echo, but still the user's edit.
  const e = editor(enc("a", CHIP, "b"));
  e.select(2);
  e.input("deleteContentBackward");
  assert.equal(e.last().runs, enc("ab"));
  e.patch({ op: "rich", id: 1, runs: enc("a", ["b", BOLD]) });
  assert.equal(e.el.textContent, "ab");
  // Not an Apple platform here: ⌘ is Ctrl.
  const undo = press(e.el, "z", { ctrlKey: true });
  assert.equal(undo.defaultPrevented, true, "the browser's own undo does not run");
  assert.equal(e.last().runs, enc("a", CHIP, "b"), "the chip is back, and the app hears it");
  assert.deepEqual(e.selection(), [1, 2], "the deleted chip, selected");
  // The app's answer to the undo keeps the redo.
  e.patch({ op: "rich", id: 1, runs: enc("a", CHIP, ["b", BOLD]) });
  press(e.el, "Z", { ctrlKey: true, shiftKey: true });
  assert.equal(e.last().runs, enc("a", ["b", BOLD]), "redone, as the app showed it");
  press(e.el, "z", { ctrlKey: true });
  assert.equal(e.last().runs, enc("a", CHIP, ["b", BOLD]));
  press(e.el, "y", { ctrlKey: true });
  assert.equal(e.last().runs, enc("a", ["b", BOLD]), "Ctrl+Y redoes too");

  // An answer with other characters (a markdown shortcut) joins the edit.
  const m = editor("");
  m.select(0);
  m.input("insertText", "#");
  m.input("insertText", " ");
  m.patch({ op: "rich", id: 1, runs: enc(["", BOLD]) });
  assert.equal(m.el.textContent, "");
  m.input("insertText", "T");
  assert.equal(m.last().v, "T");
  press(m.el, "z", { ctrlKey: true });
  assert.equal(m.last().v, "", "what is typed after it is a step of its own");
  press(m.el, "z", { ctrlKey: true });
  assert.equal(m.last().runs, "#", "then back to before the space");

  // Runs from the app with no edit in flight are new content: no step
  // back into what was there.
  const n = editor(enc("draft"));
  n.select(5);
  n.input("insertText", "!");
  n.echo();
  n.patch({ op: "rich", id: 1, runs: enc("another note") });
  const before = n.sent().length;
  press(n.el, "z", { ctrlKey: true });
  assert.equal(n.sent().length, before, "nothing to undo");
  assert.equal(n.el.textContent, "another note");
  n.patch({ op: "sel", id: 1, start: 12, end: 12 });
  n.input("insertText", "s");
  assert.equal(n.last().v, "another notes");
  press(n.el, "z", { ctrlKey: true });
  assert.equal(n.last().runs, enc("another note"), "typing after it is undone alone");
});

test("one undo per ⌘Z: the app's when it consumes the keys, else the field's (#338)", () => {
  const page = boot();
  page.ws.open();
  page.ws.deliver({
    t: "reset",
    p: [
      { op: "create", id: 1, kind: "view" }, { op: "keys", id: 1, local: true, global: false },
      { op: "consume", id: 1, keys: ["cmd+z", "cmd+shift+z", "cmd+y"] },
      { op: "create", id: 2, kind: "rich_input" }, { op: "rich", id: 2, runs: enc("ab") },
      { op: "create", id: 3, kind: "view" }, { op: "keys", id: 3, local: false, global: true },
      { op: "children", id: 1, children: [2, 3] }, { op: "root", id: 1 },
    ],
  });
  const el = page.node(2);
  el.focus();
  page.ws.deliver({ t: "patch", p: [{ op: "sel", id: 2, start: 2, end: 2 }] });
  el.dispatchEvent({ type: "beforeinput", inputType: "insertText", data: "c" });
  const inputs = () => page.ws.sent.filter((m) => m.t === "input");
  const keys = () => page.ws.sent.filter((m) => m.t === "key");
  assert.equal(inputs().length, 1);
  // Consumed: the app's undo alone (its handler hears the key); the
  // browser's is prevented, the field's never runs.
  for (const [key, mods] of [["z", { ctrlKey: true }], ["Z", { ctrlKey: true, shiftKey: true }], ["y", { ctrlKey: true }]]) {
    const e = press(el, key, mods);
    assert.equal(e.defaultPrevented, true);
    assert.equal(keys().at(-1).key, key);
  }
  assert.equal(inputs().length, 1, "the field's history did not run");
  assert.equal(el.textContent, "abc");
  // Not consumed: the field's undo runs once, the browser's is prevented;
  // a global handler still hears the key.
  page.ws.deliver({ t: "patch", p: [{ op: "consume", id: 1, keys: [] }] });
  const e = press(el, "z", { ctrlKey: true });
  assert.equal(e.defaultPrevented, true);
  assert.equal(inputs().length, 2);
  assert.equal(inputs().at(-1).v, "ab");
  assert.equal(keys().at(-1).key, "z", "reported to the app");
  // An app menu's ⌘Z: the menu item's, not the field's.
  page.ws.deliver({ t: "shortcuts", list: [["cmd+shift+z", 9]] });
  assert.equal(press(el, "Z", { ctrlKey: true, shiftKey: true }).defaultPrevented, true);
  assert.deepEqual(page.ws.sent.filter((m) => m.t === "menu").at(-1), { t: "menu", id: 9 });
  assert.equal(inputs().length, 2, "no redo in the field");
  // Other chords with Z are not undo; neither is ⌘Z while composing.
  assert.equal(press(el, "z", { ctrlKey: true, altKey: true }).defaultPrevented, false);
  el.dispatchEvent({ type: "compositionstart" });
  assert.equal(press(el, "z", { ctrlKey: true, isComposing: true }).defaultPrevented, false);
  assert.equal(inputs().length, 2);
});

test("keys in a rich input carry its selection; on_select reports characters (#86, #115)", () => {
  const page = boot();
  page.ws.open();
  page.ws.deliver({
    t: "reset",
    p: [
      { op: "create", id: 1, kind: "view" }, { op: "keys", id: 1, local: true, global: false },
      { op: "create", id: 2, kind: "rich_input" }, { op: "rich", id: 2, runs: enc("a😀", CHIP, "b") },
      { op: "selev", id: 2, on: true }, { op: "children", id: 1, children: [2] }, { op: "root", id: 1 },
    ],
  });
  const el = page.node(2);
  el.focus();
  page.ws.deliver({ t: "patch", p: [{ op: "sel", id: 2, start: 3, end: 3 }] });
  el.dispatchEvent({ type: "keydown", key: "Backspace" });
  const key = page.ws.sent.find((m) => m.t === "key");
  assert.deepEqual(key.sel, [3, 3], "the emoji and the chip are one character each");
});

test("a read-only rich text shows a chip as its label", () => {
  const page = boot();
  page.ws.open();
  page.ws.deliver({ t: "reset", p: [{ op: "create", id: 1, kind: "text" }, { op: "config", id: 1, config: enc("See ", ["Roadmap", "k=page:1", "n=Roadmap", "l=5"]) }, { op: "root", id: 1 }] });
  const chip = page.node(1).childNodes[1];
  assert.equal(chip.className, "prism-chip");
  assert.equal(chip.textContent, "Roadmap");
  assert.equal(chip.dataset.link, "5");
});

test("a selection set while the field is not focused lands when the app focuses it (#86, #115)", () => {
  const page = boot();
  page.ws.open();
  page.ws.deliver({ t: "reset", p: [{ op: "create", id: 1, kind: "rich_input" }, { op: "rich", id: 1, runs: "hello" }, { op: "root", id: 1 }] });
  const el = page.node(1);
  const sel = page.document.getSelection();
  page.ws.deliver({ t: "patch", p: [{ op: "sel", id: 1, start: 2, end: 4 }] });
  assert.equal(sel.anchorNode, null, "the page's selection is left alone");
  page.ws.deliver({ t: "patch", p: [{ op: "focus", id: 1 }] });
  assert.equal(page.document.activeElement, el);
  assert.deepEqual([sel.anchorOffset, sel.focusOffset], [2, 4]);
  // A click drops a pending one: the caret goes where the user clicked.
  page.document.body.focus();
  page.ws.deliver({ t: "patch", p: [{ op: "sel", id: 1, start: 0, end: 0 }] });
  el.dispatchEvent({ type: "pointerdown" });
  page.ws.deliver({ t: "patch", p: [{ op: "focus", id: 1 }] });
  assert.deepEqual([sel.anchorOffset, sel.focusOffset], [2, 4]);
});

test("the app's copy, cut and paste: asked, then written or inserted (#217)", async () => {
  const e = editor(enc("Hello ", ["bold", BOLD], " world"), [{ op: "editev", id: 1, mask: 2 | 4 | 8 }]);
  // Copy: the plain text at once, then the app is asked.
  e.select(3, 12);
  let copied = null;
  e.el.dispatchEvent({ type: "copy", clipboardData: { setData: (t, v) => { copied = [t, v]; } } });
  assert.deepEqual(copied, ["text/plain", "lo bold w"]);
  assert.deepEqual(e.page.ws.sent.filter((m) => m.t === "copy").at(-1), { t: "copy", id: 1, cut: false, start: 3, end: 12, text: "lo bold w" });
  // Its answer is written as one item, a type the browser lacks as a custom one.
  const written = [];
  e.page.window.ClipboardItem = class { constructor(parts) { this.parts = parts; } static supports(t) { return t === "text/plain" || t === "text/html" || t.startsWith("web "); } };
  e.page.window.Blob = class { constructor(parts, opts) { this.parts = parts; this.type = opts.type; } };
  e.page.window.navigator.clipboard = { write: async (items) => { written.push(...items); }, writeText: async () => {} };
  e.patch({ op: "clip", id: 1, items: [["text/plain", "lo bold w"], ["text/markdown", "lo **bold** w"], ["text/html", "lo <b>bold</b> w"]] });
  await Promise.resolve();
  assert.deepEqual(Object.keys(written[0].parts), ["text/plain", "web text/markdown", "text/html"]);
  // Cut: asked too, then deleted.
  e.select(0, 6);
  e.el.dispatchEvent({ type: "cut", clipboardData: { setData() {} } });
  assert.equal(e.page.ws.sent.filter((m) => m.t === "copy").at(-1).cut, true);
  assert.equal(e.last().v, "bold world");
  // Paste: nothing inserted until the app leaves it to the field.
  e.select(4);
  const n = e.sent().length;
  const pasted = e.el.dispatchEvent({
    type: "paste",
    clipboardData: {
      types: ["text/plain", "text/html", "application/x-blocks", "Files"],
      items: [{ kind: "file", type: "image/png" }],
      getData: (t) => ({ "text/plain": "one\ntwo", "text/html": "<p>one</p>", "application/x-blocks": "[1,2]" })[t] || "",
    },
  });
  assert.equal(pasted, false);
  assert.equal(e.sent().length, n, "nothing inserted yet");
  assert.deepEqual(e.page.ws.sent.filter((m) => m.t === "paste").at(-1), {
    t: "paste", id: 1, start: 4, end: 4, text: "one\ntwo", html: "<p>one</p>",
    types: ["text/plain", "text/html", "application/x-blocks", "image/png"], data: [["application/x-blocks", "[1,2]"]],
  });
  e.patch({ op: "instext", id: 1, text: "one\ntwo" });
  assert.equal(e.last().v, "boldone\ntwo world");
  // Without the hooks: the field copies and pastes as before, asking nothing.
  const plain = editor(enc("abc"));
  plain.select(0, 3);
  plain.el.dispatchEvent({ type: "copy", clipboardData: { setData() {} } });
  plain.el.dispatchEvent({ type: "paste", clipboardData: { getData: () => "x" } });
  assert.equal(plain.page.ws.sent.filter((m) => m.t === "copy" || m.t === "paste").length, 0);
  assert.equal(plain.last().v, "x");
});

test("the selection's rectangle, in the field, once per frame (#217)", () => {
  const e = editor(enc("Hello world"), [{ op: "editev", id: 1, mask: 1 }]);
  e.el.clientWidth = 300;
  e.el.clientHeight = 40;
  const sel = e.page.document.getSelection();
  e.select(6, 11);
  sel.rect = { left: 60, top: 4, width: 40, height: 18, right: 100, bottom: 22 };
  e.el.dispatchEvent({ type: "keyup" });
  e.page.frames.splice(0).forEach((f) => f());
  const rects = () => e.page.ws.sent.filter((m) => m.t === "selrect");
  assert.deepEqual(rects().at(-1), { t: "selrect", id: 1, start: 6, end: 11, x: 60, y: 4, w: 40, h: 18 });
  // The same again: not sent twice.
  e.el.dispatchEvent({ type: "keyup" });
  e.page.frames.splice(0).forEach((f) => f());
  assert.equal(rects().length, 1);
  // A caret is one point wide.
  e.select(2);
  sel.rect = { left: 20, top: 4, width: 0, height: 18, right: 20, bottom: 22 };
  e.el.dispatchEvent({ type: "keyup" });
  e.page.frames.splice(0).forEach((f) => f());
  assert.deepEqual(rects().at(-1), { t: "selrect", id: 1, start: 2, end: 2, x: 20, y: 4, w: 1, h: 18 });
  // Without the hook: none.
  e.patch({ op: "editev", id: 1, mask: 0 });
  e.select(3);
  e.el.dispatchEvent({ type: "keyup" });
  e.page.frames.splice(0).forEach((f) => f());
  assert.equal(rects().length, 2);
});

test("a click with ⌘, Ctrl or ⌥ on a run is the app's on_span_click (#217)", () => {
  const e = editor(enc("See ", ["docs", "u", "l=9", "h=/docs"], " now"), [{ op: "editev", id: 1, mask: 16 }]);
  const link = e.el.childNodes[1];
  const clicked = e.el.dispatchEvent({ type: "click", target: link, metaKey: true, clientX: 0, clientY: 0 });
  assert.equal(clicked, false);
  assert.deepEqual(e.page.ws.sent.filter((m) => m.t === "spanclick").at(-1), { t: "spanclick", id: 1, at: 4, mods: 8 });
  assert.ok(!e.page.ws.sent.some((m) => m.t === "menu"), "instead of the link's own click");
  // A plain click places the caret: not reported.
  e.el.dispatchEvent({ type: "click", target: e.el.childNodes[2] });
  assert.equal(e.page.ws.sent.filter((m) => m.t === "spanclick").length, 1);
  // Without the hook, ⌘-click opens the link as before.
  const f = editor(enc("See ", ["docs", "u", "l=9", "h=/docs"]));
  f.el.dispatchEvent({ type: "click", target: f.el.childNodes[1], metaKey: true });
  assert.deepEqual(f.page.ws.sent.filter((m) => m.t === "menu").at(-1), { t: "menu", id: 9, x: 0, y: 0, w: 0, h: 0 });
});

test("a chip's click gives its box in the field, and a popover anchored to it sits under it (#327)", () => {
  const e = editor(enc("Hi ", [...CHIP, "l=65545"], " and ", ["￼", "k=date", "n=Date", "l=65546"]));
  const box = (left, top, width, height) => ({ left, top, width, height, right: left + width, bottom: top + height });
  e.el.getBoundingClientRect = () => box(100, 200, 400, 60);
  const date = e.el.childNodes[3];
  assert.equal(date.className, "prism-chip");
  date.getBoundingClientRect = () => box(260, 210, 50, 20);
  e.el.dispatchEvent({ type: "click", target: date });
  assert.deepEqual(e.page.ws.sent.filter((m) => m.t === "menu").at(-1), { t: "menu", id: 65546, x: 160, y: 10, w: 50, h: 20 });
  // A popover anchored to that chip (character 9), elsewhere in the tree.
  const open = (config) => {
    e.patch(
      { op: "create", id: 7, kind: "view" }, { op: "create", id: 8, kind: "overlay" },
      { op: "config", id: 8, config }, { op: "children", id: 7, children: [8] },
      { op: "value", id: 8, value: 1 },
      { op: "create", id: 9, kind: "view" }, { op: "children", id: 9, children: [1, 7] }, { op: "root", id: 9 },
    );
    return e.page.node(8);
  };
  // The overlay's parent is beside the field, not the anchor.
  const ov = open("popover anchor=text:1:9:10");
  const pop = ov.firstChild;
  pop.getBoundingClientRect = () => box(0, 0, 120, 80);
  e.page.ws.deliver({ t: "patch", p: [{ op: "value", id: 8, value: 1 }] });
  assert.deepEqual([pop.style.left, pop.style.top], ["260px", "236px"], "6 px under the chip, lined up with it");
  // The text wrapped: the chip moved; the user typing places it again.
  date.getBoundingClientRect = () => box(110, 240, 50, 20);
  e.page.document.dispatchEvent({ type: "input", target: e.el });
  e.page.frames.splice(0).forEach((f) => f());
  assert.deepEqual([pop.style.left, pop.style.top], ["110px", "266px"]);
  // The chip gone: anchored to the field, under it.
  e.patch({ op: "config", id: 8, config: "popover anchor=field:1" });
  e.page.frames.splice(0).forEach((f) => f());
  assert.deepEqual([pop.style.left, pop.style.top], ["100px", "266px"]);
});


test("every click in the field says which character, with on_click (#266)", () => {
  const e = editor(enc("See ", ["docs", "u"], " now"), [{ op: "editev", id: 1, mask: 32 }]);
  e.el.dispatchEvent({ type: "click", target: e.el.childNodes[1], shiftKey: true, detail: 1, clientX: 0, clientY: 0 });
  const c = e.page.ws.sent.filter((m) => m.t === "fieldclick").at(-1);
  assert.equal(c.at, 4);
  assert.equal(c.mods, 1);
  // Without the hook: none.
  e.patch({ op: "editev", id: 1, mask: 0 });
  e.el.dispatchEvent({ type: "click", target: e.el.childNodes[1], detail: 1 });
  assert.equal(e.page.ws.sent.filter((m) => m.t === "fieldclick").length, 1);
});

// A chip's options (#329): its box, tooltip, styled label runs, vector
// icon, state, test id and attributes.
const GS = "\u001d", US = "\u001c";
const STYLED = [
  OBJ, "k=user:7", "n=Ada · 3 tasks", "b=#FFD60A33",
  `nr=Ada${US}w=700${GS} · 3 tasks${US}i${US}c=#8E8E93FF`,
  "v=i0,0\nf~FF |M 0 0 L 1 0 L 1 1 Z \n", "vr=1.5",
  "tt=Ada Lovelace", "ring=1.5,#0A84FFFF", "rad=9", "ht=22", "mw=120",
  "st=open", "tid=mention-ada", `at=data-person${US}ada${GS}onclick${US}x`,
];
const rules = (el) => el.ownerDocument.getElementById("prism-styles").sheet.cssRules;
/** The CSS of `el`'s style class. */
const cssOf = (el) => {
  const r = [...rules(el)].find((r) => el._ps && r.selectorText.endsWith(`.${el._ps}`));
  return r ? r.style.cssText : "";
};

test("a chip draws its box, tooltip, label runs, vector icon and identity (#329)", () => {
  const { el } = editor(enc("To ", STYLED));
  const chip = el.childNodes[1];
  assert.ok(chip.classList.contains("prism-chip"));
  assert.ok(chip.classList.contains("prism-chip-open"), "its state class");
  assert.equal(chip.dataset.state, "open");
  assert.equal(chip.dataset.testid, "mention-ada");
  assert.equal(chip.getAttribute("data-person"), "ada");
  assert.equal(chip.getAttribute("onclick"), null, "only data-* names");
  assert.equal(chip.title, "Ada Lovelace");
  assert.equal(chip.getAttribute("contenteditable"), "false");
  const box = cssOf(chip);
  assert.match(box, /border-radius:9px/);
  assert.doesNotMatch(box, /border-radius:3px/, "a chip's background brings no radius of its own");
  assert.match(box, /box-shadow:inset 0 0 0 1.5px #0A84FFFF/);
  assert.match(box, /height:max\(22px,1.2em\)/);
  assert.match(box, /max-width:120px/);
  const [icon, label] = chip.childNodes;
  assert.equal(icon.localName, "svg");
  assert.equal(icon.getAttribute("class").split(" ")[0], "prism-chip-icon");
  assert.equal(icon.getAttribute("viewBox"), "0 0 150 100", "one em tall, 1.5 wide");
  assert.match(cssOf(icon), /width:1.5em/);
  assert.equal(icon.childNodes.length, 1);
  assert.match(cssOf(icon.childNodes[0]), /fill:currentColor/);
  assert.equal(label.className.split(" ")[0], "prism-chip-label");
  assert.match(cssOf(label), /text-overflow:ellipsis/, "cut at its largest width");
  const [name, summary] = label.childNodes;
  assert.equal(name.textContent, "Ada");
  assert.match(cssOf(name), /font-weight:700/);
  assert.equal(summary.textContent, " · 3 tasks");
  assert.match(cssOf(summary), /font-style:italic/);
  assert.match(cssOf(summary), /color:#8E8E93FF/);
  assert.equal(chip.textContent, "Ada · 3 tasks");
});

test("a styled chip stays one character: deleted whole, copied as its label (#329)", () => {
  const e = editor(enc("To ", STYLED, " now"));
  e.select(4);
  e.input("deleteContentBackward");
  assert.deepEqual(e.last(), { t: "input", id: 1, v: "To  now", runs: enc("To  now") });
  const f = editor(enc("To ", STYLED));
  f.select(0, 4);
  const data = new Map();
  f.el.dispatchEvent({ type: "copy", clipboardData: { setData: (k, v) => data.set(k, v) } });
  assert.equal(data.get("text/plain"), "To Ada · 3 tasks");
});

test("a chip's style classes are kept while it is shown (#329)", () => {
  const { page, el } = editor(enc("To ", STYLED));
  const chip = el.childNodes[1];
  const classes = [chip, ...chip.childNodes, ...chip.childNodes[1].childNodes].map((n) => n._ps).filter(Boolean);
  for (const c of classes) assert.ok(el._runPs.includes(c), `${c} is the field's`);
  void page;
});

test("a read-only rich text draws a chip's options too, and a run's tooltip (#329)", () => {
  const page = boot();
  page.ws.open();
  page.ws.deliver({ t: "reset", p: [{ op: "create", id: 1, kind: "text" }, { op: "config", id: 1, config: enc(["See", "tt=Look here"], ["Ada · 3 tasks", ...STYLED.slice(1)]) }, { op: "root", id: 1 }] });
  const [see, chip] = page.node(1).childNodes;
  assert.equal(see.title, "Look here");
  assert.ok(chip.classList.contains("prism-chip-open"));
  assert.equal(chip.dataset.testid, "mention-ada");
  assert.equal(chip.textContent, "Ada · 3 tasks");
});
