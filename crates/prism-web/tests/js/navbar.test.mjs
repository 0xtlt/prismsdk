// Bar buttons in the stack header and the tint of bars (#30).
import { test } from "node:test";
import assert from "node:assert/strict";
import { boot, cssOf } from "./dom.mjs";

const create = (id, kind) => ({ op: "create", id, kind });
const textOp = (id, text) => ({ op: "text", id, text });
const children = (id, ids) => ({ op: "children", id, children: ids });
const root = (id) => ({ op: "root", id });

function live(patches) {
  const page = boot();
  page.ws.open();
  page.ws.deliver({ t: "reset", p: patches });
  return page;
}

const bar = (page, side) =>
  page.node(1).querySelector(`.prism-bar-${side}`).querySelectorAll(":scope > .prism-bar-button").map((b) => [b.textContent, b.getAttribute("aria-label"), b.dataset.bar]);

test("the top screen's bar buttons sit in the header and report their index", () => {
  const todos = "inline\nL\t\tEdit\t\nT\tplus\t\tAdd a todo";
  const page = live([
    create(1, "stack"), create(2, "screen"), textOp(2, "Todos"), { op: "config", id: 2, config: todos },
    children(1, [2]), root(1),
  ]);
  assert.deepEqual(bar(page, "leading"), [["Edit", "Edit", "0"]]);
  // An icon with a glyph shows the glyph and is announced by its label.
  assert.deepEqual(bar(page, "trailing"), [["+", "Add a todo", "1"]]);
  // Back stays first on the leading side.
  assert.equal(page.node(1).querySelector(".prism-bar-leading").firstChild.className, "prism-back");
  page.node(1).querySelectorAll(".prism-bar-button")[1].click();
  assert.deepEqual(page.ws.sent.at(-1), { t: "change", id: 2, v: 1 });

  // A pushed screen without buttons: none; popping back brings them back.
  page.ws.deliver({ t: "patch", p: [create(3, "screen"), textOp(3, "Detail"), children(1, [2, 3])] });
  assert.deepEqual(bar(page, "leading"), []);
  assert.deepEqual(bar(page, "trailing"), []);
  page.ws.deliver({ t: "patch", p: [children(1, [2]), { op: "destroy", id: 3 }] });
  assert.equal(bar(page, "trailing").length, 1);

  // Changed buttons replace the old ones.
  page.ws.deliver({ t: "patch", p: [{ op: "config", id: 2, config: "\nT\tsparkles\tMagic\t" }] });
  assert.deepEqual(bar(page, "leading"), []);
  assert.deepEqual(bar(page, "trailing"), [["Magic", "Magic", "0"]], "no glyph: the title");
});

test("bar tints are CSS variables the stylesheet reads", () => {
  const page = live([create(1, "tabs"), { op: "style", id: 1, css: "display:flex;--prism-tint:rgba(255,0,0,1);" }, root(1)]);
  assert.match(cssOf(page.node(1)), /--prism-tint:rgba\(255,0,0,1\)/);
});
