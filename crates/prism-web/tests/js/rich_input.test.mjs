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
