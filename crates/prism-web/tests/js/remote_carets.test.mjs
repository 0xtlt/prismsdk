// Other people's carets and selections (#240): a layer next to the field,
// one group per caret (a colored bar with a name tag, tinted boxes), kept
// across updates so an idle label fades; the field's value and selection
// are never touched.
//
// Run: node --test crates/prism-web/tests/js/
import { test } from "node:test";
import assert from "node:assert/strict";
import { boot } from "./dom.mjs";

const camille = { anchor: 12, head: 12, color: "rgba(232,89,12,1)", label: "Camille", idle: false };
const range = { anchor: 3, head: 9, color: "rgba(28,126,214,1)", label: null, idle: false };

function field(kind, carets) {
  const page = boot();
  page.ws.open();
  page.ws.deliver({
    t: "reset",
    p: [
      { op: "create", id: 1, kind: "view" },
      { op: "create", id: 2, kind },
      { op: "text", id: 2, text: "Hello, world. Again." },
      { op: "carets", id: 2, carets },
      { op: "children", id: 1, children: [2] },
      { op: "root", id: 1 },
    ],
  });
  return { page, root: page.node(1), el: page.node(2), patch: (...p) => page.ws.deliver({ t: "patch", p }) };
}

for (const kind of ["text_area", "text_input", "rich_input"]) {
  test(`a ${kind} shows other people's carets in a layer beside it (#240)`, () => {
    const { page, root, el, patch } = field(kind, [camille, range]);
    // Placed once the batch laid the field out.
    page.frames.splice(0).forEach((f) => f());
    const layer = el.nextSibling;
    assert.ok(layer && layer.className === "prism-carets", "the layer follows the field");
    assert.equal(layer.getAttribute("aria-hidden"), "true");
    assert.equal(root.children.length, 2, "the field and its layer");
    assert.ok(!el.contains(layer), "never inside the field's text");
    const [a, b] = layer.children;
    assert.equal(a.style.getPropertyValue("--prism-rc"), camille.color);
    assert.equal(a.querySelector(".prism-rlabel").textContent, "Camille");
    assert.equal(b.querySelector(".prism-rlabel").hidden, true, "no label, no tag");

    // Idle: the same group fades its label (kept, so CSS transitions it).
    patch({ op: "carets", id: 2, carets: [{ ...camille, idle: true }] });
    assert.equal(layer.children.length, 1);
    assert.equal(layer.children[0], a, "the group is kept");
    assert.ok("idle" in a.dataset);
    patch({ op: "carets", id: 2, carets: [{ ...camille, head: 14, anchor: 14 }] });
    assert.ok(!("idle" in a.dataset));

    // The field's own value and selection are left alone.
    if (kind !== "rich_input") assert.equal(el.value, "Hello, world. Again.");

    // None: the layer goes.
    patch({ op: "carets", id: 2, carets: [] });
    assert.equal(el.nextSibling, null);
    assert.equal(root.children.length, 1);
  });
}

test("the layer follows its field when it moves and goes with it (#240)", () => {
  const { page, root, el, patch } = field("text_area", [camille]);
  patch({ op: "create", id: 3, kind: "view" }, { op: "children", id: 3, children: [2] }, { op: "children", id: 1, children: [3] });
  page.frames.splice(0).forEach((f) => f());
  assert.equal(el.parentNode, page.node(3));
  assert.equal(el.nextSibling?.className, "prism-carets", "moved with it");
  assert.ok(!root.children.some((c) => c.className === "prism-carets"));
  const layer = el.nextSibling;
  patch({ op: "children", id: 3, children: [] }, { op: "destroy", id: 2 });
  assert.equal(layer.parentNode, null, "destroyed with the field");
});
