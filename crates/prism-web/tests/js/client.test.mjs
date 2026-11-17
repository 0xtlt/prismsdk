// client.js against a tiny DOM: patch application, events sent to the
// server, shapes, and the contract with the server's own rendering
// (fixtures/app.json is written by prism-web's `client_fixture_is_up_to_date`).
//
// Run: node --test crates/prism-web/tests/js/
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { boot, parseHTML, Document, cssOf, styleOf } from "./dom.mjs";

const fixture = JSON.parse(fs.readFileSync(new URL("./fixtures/app.json", import.meta.url), "utf8"));

/** A page connected to the server, after the first "reset". */
function live(patches, options) {
  const page = boot(options);
  page.ws.open();
  if (patches) page.ws.deliver({ t: "reset", p: patches });
  return page;
}

const sent = (page) => page.ws.sent.filter((m) => m.t !== "lifecycle" && m.t !== "device");
const lastSent = (page) => sent(page).at(-1);

/** What a Prism node looks like to a user and to assistive technology. */
function signature(el) {
  const attr = (n) => el.getAttribute(n);
  const own = el.childNodes.filter((n) => n.nodeType === 3).map((n) => n.data).join("");
  return {
    pid: attr("data-pid"),
    tag: el.localName,
    kind: attr("data-kind"),
    // Without the shared sheet's classes: `css` compares the style itself.
    class: attr("class") && attr("class").split(" ").filter((c) => !/^p[0-9a-z]+(-\d+)?$/.test(c)).join(" ") || null,
    role: attr("role"),
    label: attr("aria-label"),
    level: attr("aria-level"),
    selected: attr("aria-selected"),
    tabindex: attr("tabindex"),
    href: attr("href"),
    rel: attr("rel"),
    hidden: el.hidden,
    text: own,
    placeholder: attr("placeholder"),
    value: el.localName === "input" ? el.value : null,
    checked: el.localName === "input" ? el.checked : null,
    // App-defined attributes (`.attr`, #85).
    testid: attr("data-testid"),
    autocomplete: attr("autocomplete"),
    css: cssOf(el),
  };
}

/** Every Prism node under `root`, in document order. */
function prismNodes(root) {
  const out = [];
  const walk = (n) => n.children.forEach((c) => {
    if (c.hasAttribute("data-pid")) out.push(c);
    walk(c);
  });
  walk(root);
  return out;
}

test("the socket says where the page is and how big", () => {
  const page = live(null, { path: "/todos/1", base: "" });
  const url = new URL(page.ws.url);
  assert.equal(url.protocol, "ws:");
  assert.equal(url.pathname, "/ws");
  assert.equal(url.searchParams.get("path"), "/todos/1");
  assert.equal(url.searchParams.get("w"), "1280");
  assert.equal(url.searchParams.get("h"), "800");
  assert.match(url.searchParams.get("tz"), /^-?\d+$/);
  // On open: lifecycle and device reports.
  const kinds = page.ws.sent.map((m) => m.t);
  assert.ok(kinds.includes("lifecycle") && kinds.includes("device"), kinds.join());
  const device = page.ws.sent.find((m) => m.t === "device");
  assert.equal(device.sw, 1440);
  assert.equal(device.touch, false);
  // Mounted under a prefix: the socket and the app path follow it.
  const nested = live(null, { path: "/app/todos", base: "/app/" });
  const u = new URL(nested.ws.url);
  assert.equal(u.pathname, "/app/ws");
  assert.equal(u.searchParams.get("path"), "/todos");
});

test("the first socket hands the page's request over, reconnects don't", () => {
  const page = live([create(1, "text"), root(1)], { pageId: "0123abcd" });
  assert.equal(new URL(page.ws.url).searchParams.get("page"), "0123abcd");
  page.ws.onclose();
  page.timers.at(-1).fn();
  assert.equal(new URL(page.sockets.at(-1).url).searchParams.get("page"), null);
  // A page without one (a HEAD, an older server) sends none.
  assert.equal(new URL(live(null).ws.url).searchParams.get("page"), null);
});

test("the client builds the tree the server rendered", () => {
  const server = new Document();
  const ssr = parseHTML(server, fixture.ssr);
  const wrapper = server.createElement("main");
  wrapper.append(...ssr);
  const page = live(fixture.reset, { ssr: fixture.ssr });
  const want = prismNodes(wrapper).map(signature);
  const got = prismNodes(page.mount).map(signature);
  assert.deepEqual(got.map((n) => n.pid), want.map((n) => n.pid), "same nodes, same order");
  for (let i = 0; i < want.length; i++) {
    const { pid, kind } = want[i];
    assert.deepEqual(got[i], want[i], `node ${pid} (${kind}) differs between the server and the client`);
  }
  // The header and tab bar too.
  const text = (root, sel) => root.querySelectorAll(sel).map((e) => e.textContent);
  assert.deepEqual(text(page.mount, ".prism-title"), text(wrapper, ".prism-title"));
  const tab = (b) => ["id", "aria-controls", "aria-selected", "tabindex", "role"].map((a) => b.getAttribute(a)).concat(b.textContent);
  assert.deepEqual(page.mount.querySelectorAll(".prism-tab").map(tab), wrapper.querySelectorAll(".prism-tab").map(tab));
  assert.deepEqual(text(page.mount, ".prism-back"), text(wrapper, ".prism-back"));
});

test("app-defined attributes are set, replaced and removed live (#85)", () => {
  const page = live(fixture.reset);
  const field = page.mount.querySelector("[data-kind=text_input]");
  assert.equal(field.getAttribute("data-testid"), "name", "from the server's reset");
  assert.equal(field.getAttribute("autocomplete"), "given-name");
  const id = Number(field.dataset.pid);
  page.ws.deliver({ t: "patch", p: [{ op: "attrs", id, attrs: [["data-1p-ignore", ""], ["data-testid", "who"]] }] });
  assert.equal(field.getAttribute("data-1p-ignore"), "");
  assert.equal(field.getAttribute("data-testid"), "who");
  assert.equal(field.hasAttribute("autocomplete"), false, "absent from the list: removed");
  // Never a name outside data-* and the safe list, nor Prism's own.
  page.ws.deliver({ t: "patch", p: [{ op: "attrs", id, attrs: [["onclick", "x"], ["data-prism-x", "1"], ["data-ok", "1"]] }] });
  assert.equal(field.hasAttribute("onclick"), false);
  assert.equal(field.hasAttribute("data-prism-x"), false);
  assert.equal(field.getAttribute("data-ok"), "1");
  assert.equal(field.hasAttribute("data-testid"), false);
});

test("a secure field keeps the app's autocomplete over the default (#85)", () => {
  const field = { op: "create", id: 2, kind: "text_input" };
  const secure = { op: "config", id: 2, config: "secure" };
  const root = [{ op: "create", id: 1, kind: "view" }, { op: "children", id: 1, children: [2] }, { op: "root", id: 1 }];
  // Attributes before the config, and after.
  for (const order of [[field, { op: "attrs", id: 2, attrs: [["autocomplete", "new-password"]] }, secure],
                       [field, secure, { op: "attrs", id: 2, attrs: [["autocomplete", "new-password"]] }]]) {
    const page = live([...order, ...root]);
    const el = page.node(2);
    assert.equal(el.getAttribute("autocomplete"), "new-password");
    page.ws.deliver({ t: "patch", p: [secure] });
    assert.equal(el.getAttribute("autocomplete"), "new-password", "a new config keeps it");
    page.ws.deliver({ t: "patch", p: [{ op: "attrs", id: 2, attrs: [] }] });
    assert.equal(el.getAttribute("autocomplete"), "current-password", "the default comes back");
  }
});

test("interactions send the events the server expects, and its answers apply", () => {
  const page = live(fixture.reset);
  for (const { send, patches } of fixture.events) {
    const el = page.node(send.id);
    assert.ok(el, `node ${send.id} exists`);
    const before = page.ws.sent.length;
    if (send.t === "click") {
      el.click();
    } else if (send.t === "input") {
      el.value = send.v;
      el.dispatchEvent({ type: "input" });
    } else if (send.t === "change" && el.dataset.kind === "toggle") {
      el.checked = !!send.v;
      el.dispatchEvent({ type: "change" });
    } else if (send.t === "change" && el.dataset.kind === "tabs") {
      el.querySelector(".prism-tabbar").children[send.v].click();
    }
    assert.deepEqual(page.ws.sent.slice(before).filter((m) => m.t !== "lifecycle"), [send], `${JSON.stringify(send)} is sent once`);
    page.ws.deliver({ t: "patch", p: patches });
  }
  // The server's answers are on screen.
  const heading = page.mount.querySelector("[role=heading]");
  assert.equal(heading.textContent, "Count: 0");
  assert.equal(page.mount.querySelector("[data-kind=text_input]").value, "Ada");
  assert.equal(page.mount.querySelector("[data-kind=toggle]").checked, true);
  const tabs = page.mount.querySelector(".prism-tabbar").children;
  assert.deepEqual(tabs.map((t) => t.getAttribute("aria-selected")), ["false", "true"]);
  const panels = page.mount.querySelectorAll("[data-kind=tab]");
  assert.deepEqual(panels.map((p) => p.hidden), [true, false]);
  // The URL follows the selected tab.
  assert.equal(page.window.location.pathname, "/more");
});

// --- Patches, one op at a time ------------------------------------------------

const create = (id, kind) => ({ op: "create", id, kind });
const root = (id) => ({ op: "root", id });
const children = (id, kids) => ({ op: "children", id, children: kids });
const textOp = (id, text) => ({ op: "text", id, text });

test("children are reordered, kept and removed in place", () => {
  const page = live([
    create(1, "view"), create(2, "text"), textOp(2, "a"), create(3, "text"), textOp(3, "b"),
    create(4, "text"), textOp(4, "c"), children(1, [2, 3, 4]), root(1),
  ]);
  const order = () => page.node(1).children.map((c) => c.textContent);
  assert.deepEqual(order(), ["a", "b", "c"]);
  const b = page.node(3);
  page.ws.deliver({ t: "patch", p: [children(1, [4, 2, 3])] });
  assert.deepEqual(order(), ["c", "a", "b"]);
  assert.equal(page.node(3), b, "moved, not recreated");
  page.ws.deliver({ t: "patch", p: [create(5, "text"), textOp(5, "new"), children(1, [4, 5, 3]), { op: "destroy", id: 2 }] });
  assert.deepEqual(order(), ["c", "new", "b"]);
  assert.equal(page.node(2), null);
});

test("text updates keep child elements and never fight a focused field", () => {
  const page = live([create(1, "button"), create(2, "image"), children(1, [2]), textOp(1, "Save"), root(1)]);
  const btn = page.node(1);
  assert.equal(btn.firstChild.data, "Save");
  page.ws.deliver({ t: "patch", p: [textOp(1, "Saved")] });
  assert.equal(btn.childNodes.length, 2, "the icon stays");
  assert.equal(btn.firstChild.data, "Saved");

  const form = live([create(1, "text_input"), textOp(1, "a"), { op: "placeholder", id: 1, text: "Name" }, root(1)]);
  const input = form.node(1);
  assert.equal(input.value, "a");
  assert.equal(input.placeholder, "Name");
  input.focus();
  const type = (v) => { input.value = v; input.dispatchEvent({ type: "input" }); };
  type("ab");
  type("abc");
  form.ws.deliver({ t: "patch", p: [textOp(1, "ab")] });
  assert.equal(input.value, "abc", "a late echo never undoes the user's typing");
  input.blur();
  form.ws.deliver({ t: "patch", p: [textOp(1, "server")] });
  assert.equal(input.value, "server");
});

// --- Focused fields: echoes vs. values set by the app (#73) ---------------------

/** A focused field `kind` holding `text`, and a `type` helper sending input events. */
function focusedField(kind, text = "") {
  const page = live([create(1, kind), textOp(1, text), root(1)]);
  const field = page.node(1);
  field.focus();
  const type = (v, caret = v.length) => {
    field.value = v;
    field.setSelectionRange(caret, caret);
    field.dispatchEvent({ type: "input" });
  };
  const patch = (text) => page.ws.deliver({ t: "patch", p: [textOp(1, text)] });
  return { page, field, type, patch };
}

test("echoes of the user's typing leave a focused field and its caret alone (#73)", () => {
  for (const kind of ["text_input", "text_area"]) {
    const { page, field, type, patch } = focusedField(kind);
    type("Hello");
    type("Hello world");
    type("Hello, world", 6); // a comma typed in the middle: the caret after it
    assert.deepEqual(page.ws.sent.filter((m) => m.t === "input").map((m) => m.v), ["Hello", "Hello world", "Hello, world"]);
    const writes = field.valueWrites;
    for (const echo of ["Hello", "Hello world", "Hello, world"]) patch(echo);
    assert.equal(field.value, "Hello, world", kind);
    assert.equal(field.valueWrites, writes, `${kind}: the value is never rewritten`);
    assert.deepEqual([field.selectionStart, field.selectionEnd], [6, 6], `${kind}: the caret stays put`);
  }
});

test("a value the app sets reaches the focused field, caret kept from the end (#73)", () => {
  // A Markdown shortcut: "# " at the start is stripped.
  const md = focusedField("text_input");
  md.type("#");
  md.patch("#");
  md.type("# ");
  md.patch("");
  assert.equal(md.field.value, "", "the app's value replaces the field's");
  md.type("T");
  md.patch("T");
  assert.equal(md.field.value, "T", "and typing goes on from there");
  assert.deepEqual(md.page.ws.sent.filter((m) => m.t === "input").map((m) => m.v), ["#", "# ", "T"]);

  // Normalising: the caret keeps its distance from the end.
  const up = focusedField("text_area", "ab");
  up.type("abcd", 3);
  up.patch("ABCD");
  assert.equal(up.field.value, "ABCD");
  assert.deepEqual([up.field.selectionStart, up.field.selectionEnd], [3, 3]);
  up.type("ABCDe");
  up.patch("ABCDE");
  assert.equal(up.field.value, "ABCDE", "a later echo is not mistaken for typing");

  // Typing ahead of the server: a late echo is ignored, the app's answer applies.
  const ahead = focusedField("text_input");
  ahead.type("# ");
  ahead.type("# T");
  ahead.patch("");
  assert.equal(ahead.field.value, "", "the app answered '# ' by clearing");
  ahead.patch("T");
  assert.equal(ahead.field.value, "T", "then answered '# T' with 'T'");
});

test("an app value waits for the end of an IME composition (#73)", () => {
  const { field, type, patch } = focusedField("text_input");
  field.dispatchEvent({ type: "compositionstart" });
  type("k");
  patch("k");
  type("か");
  patch("");
  assert.equal(field.value, "か", "the composition is not disturbed");
  field.dispatchEvent({ type: "compositionend" });
  assert.equal(field.value, "", "the app's value lands once it ends");
});

// --- Text field selection and caret (#86) ---------------------------------------

const flush = (page) => page.frames.splice(0).forEach((f) => f());
const selects = (page) => page.ws.sent.filter((m) => m.t === "select").map((m) => [m.start, m.end]);

test("a field with on_select reports its selection in characters, once per frame (#86)", () => {
  const { page, field, type } = focusedField("text_input");
  // Without `on_select`, nothing is sent.
  type("a");
  flush(page);
  assert.deepEqual(selects(page), []);
  page.ws.deliver({ t: "patch", p: [{ op: "selev", id: 1, on: true }] });
  flush(page);
  assert.deepEqual(selects(page), [[1, 1]], "where the caret is when reporting starts");
  // "a😀b": the emoji is 2 UTF-16 units, 1 character.
  type("a😀b", 3);
  flush(page);
  const order = page.ws.sent.filter((m) => m.t === "input" || m.t === "select").map((m) => m.t);
  assert.deepEqual(order.slice(-2), ["input", "select"], "after the input that moved it");
  assert.deepEqual(selects(page).at(-1), [2, 2]);
  // Selecting backwards, then moving twice in one frame: one report, in order.
  field.setSelectionRange(4, 0);
  field.dispatchEvent({ type: "select" });
  field.setSelectionRange(1, 4);
  field.dispatchEvent({ type: "selectionchange" });
  page.document.dispatchEvent({ type: "selectionchange" });
  flush(page);
  assert.deepEqual(selects(page).slice(-1), [[1, 3]]);
  assert.equal(selects(page).length, 3);
  // Unmoved: not sent again.
  field.dispatchEvent({ type: "keyup" });
  flush(page);
  assert.equal(selects(page).length, 3);
});

test("key presses in a field carry its selection before the key (#86)", () => {
  const page = live([
    create(1, "view"), { op: "keys", id: 1, local: true, global: false },
    create(2, "text_input"), textOp(2, "héllo"), { op: "selev", id: 2, on: true },
    children(1, [2]), root(1),
  ]);
  const field = page.node(2);
  field.focus();
  field.setSelectionRange(2, 2);
  field.dispatchEvent({ type: "mouseup" });
  field.dispatchEvent({ type: "keydown", key: "Enter" });
  const key = page.ws.sent.find((m) => m.t === "key");
  assert.deepEqual(key.sel, [2, 2]);
  const i = page.ws.sent.indexOf(key);
  assert.deepEqual(page.ws.sent[i - 1], { t: "select", id: 2, start: 2, end: 2 }, "pending selection flushed first");
});

test("the app's selection is applied in characters, never against typing in flight (#86)", () => {
  const { page, field, type } = focusedField("text_area", "a😀bc");
  const patch = (p) => page.ws.deliver({ t: "patch", p });
  patch([{ op: "sel", id: 1, start: 1, end: 3 }]);
  assert.deepEqual([field.selectionStart, field.selectionEnd], [1, 4], "UTF-16 units");
  patch([{ op: "sel", id: 1, start: 9, end: 9 }]);
  assert.deepEqual([field.selectionStart, field.selectionEnd], [5, 5], "clamped to the end");

  // Typing ahead: the echo of "x" and a selection computed for it.
  type("x");
  type("xy");
  patch([textOp(1, "x"), { op: "sel", id: 1, start: 0, end: 1 }]);
  assert.equal(field.value, "xy");
  assert.deepEqual([field.selectionStart, field.selectionEnd], [2, 2], "the user's caret stays");
  // All echoed: the app's selection lands (bold the word, select it).
  patch([textOp(1, "xy"), { op: "sel", id: 1, start: 0, end: 2 }]);
  assert.deepEqual([field.selectionStart, field.selectionEnd], [0, 2]);
  // An app value with a caret: both apply, in order.
  patch([textOp(1, "Title"), { op: "sel", id: 1, start: 0, end: 0 }]);
  assert.equal(field.value, "Title");
  assert.deepEqual([field.selectionStart, field.selectionEnd], [0, 0]);
});

test("the app's selection waits for the end of an IME composition (#86)", () => {
  const { page, field, type } = focusedField("text_input", "ab");
  field.dispatchEvent({ type: "compositionstart" });
  type("abk");
  page.ws.deliver({ t: "patch", p: [textOp(1, "abk")] });
  page.ws.deliver({ t: "patch", p: [{ op: "sel", id: 1, start: 0, end: 0 }] });
  assert.deepEqual([field.selectionStart, field.selectionEnd], [3, 3]);
  field.dispatchEvent({ type: "compositionend" });
  assert.deepEqual([field.selectionStart, field.selectionEnd], [0, 0]);
});

test("accessibility patches set and clear ARIA", () => {
  const page = live([create(1, "view"), root(1)]);
  const el = page.node(1);
  const a11y = (a) => page.ws.deliver({ t: "patch", p: [{ op: "a11y", id: 1, a11y: { role: null, level: null, label: null, hint: null, value: null, checked: null, disabled: false, hidden: false, ...a } }] });
  a11y({ role: "button", label: "Open", hint: "Shows details" });
  assert.equal(el.getAttribute("role"), "button");
  assert.equal(el.getAttribute("aria-label"), "Open");
  assert.equal(el.getAttribute("aria-description"), "Shows details");
  assert.equal(el.getAttribute("tabindex"), "0");
  a11y({ role: "button", checked: true });
  assert.equal(el.getAttribute("aria-pressed"), "true");
  assert.equal(el.getAttribute("aria-label"), null, "cleared");
  a11y({ role: "heading", level: 3 });
  assert.equal(el.getAttribute("aria-level"), "3");
  assert.equal(el.getAttribute("tabindex"), null, "headings are not focusable");
  a11y({ role: "checkbox", checked: false, disabled: true });
  assert.equal(el.getAttribute("aria-checked"), "false");
  assert.equal(el.getAttribute("aria-disabled"), "true");
  assert.equal(el.getAttribute("tabindex"), null);
  a11y({ role: "tab", checked: true, hidden: true });
  assert.equal(el.getAttribute("aria-selected"), "true");
  assert.equal(el.getAttribute("aria-hidden"), "true");
  a11y({ role: "tab", checked: true, selected: false });
  assert.equal(el.getAttribute("aria-selected"), "false", "selected wins over checked");
  // The current destination of a navigation (#64).
  a11y({ role: "link", current: true });
  assert.equal(el.getAttribute("aria-current"), "page");
  assert.equal(el.getAttribute("aria-selected"), null);
  a11y({ role: "button", current: true, selected: true });
  assert.equal(el.getAttribute("aria-current"), "true");
  assert.equal(el.getAttribute("aria-selected"), "true");
  a11y({});
  for (const name of ["role", "aria-level", "aria-checked", "aria-selected", "aria-current", "aria-hidden", "aria-disabled"]) {
    assert.equal(el.getAttribute(name), null, name);
  }

  // Native controls: disabled is the element's own state.
  const btn = live([create(1, "button"), root(1)]);
  btn.ws.deliver({ t: "patch", p: [{ op: "a11y", id: 1, a11y: { role: "button", disabled: true } }] });
  assert.equal(btn.node(1).disabled, true);
  assert.equal(btn.node(1).getAttribute("role"), null, "implicit role");
  // Disabled clickable views send nothing.
  btn.node(1).click();
  assert.equal(lastSent(btn), undefined);
});

test("values, icons, pictures, appearance and environment", () => {
  const page = live([
    create(1, "view"), create(2, "toggle"), create(3, "progress"), create(4, "picture"), create(5, "image"),
    children(1, [2, 3, 4, 5]), root(1),
    { op: "value", id: 2, value: 1 }, { op: "value", id: 3, value: 0.25 },
    textOp(4, "/assets/logo.abc.png"), { op: "value", id: 4, value: 1 },
    { op: "a11y", id: 4, a11y: { role: "img", label: "Logo", hidden: false } },
    { op: "icon", id: 5, icon: "plus", glyph: "+" },
    { op: "appearance", value: "dark" },
    { op: "env", lang: "ar", dir: "rtl", back: "رجوع", close: "إغلاق" },
  ]);
  assert.equal(page.node(2).checked, true);
  assert.equal(page.node(2).getAttribute("role"), "switch");
  assert.equal(page.node(3).value, 0.25);
  assert.equal(page.node(4).src, "/assets/logo.abc.png");
  assert.equal(page.node(4).alt, "Logo");
  assert.ok(page.node(4).classList.contains("prism-fit"), "contain");
  assert.equal(page.node(5).textContent, "+");
  const html = page.document.documentElement;
  assert.equal(html.style.colorScheme, "dark");
  assert.equal(html.lang, "ar");
  assert.equal(html.dir, "rtl");
  // Hidden pictures have an empty alt (decorative).
  page.ws.deliver({ t: "patch", p: [{ op: "a11y", id: 4, a11y: { label: "Logo", hidden: true } }] });
  assert.equal(page.node(4).alt, "");
});

test("styles replace the inline CSS and play enter animations once", () => {
  const page = live([create(1, "view"), root(1)]);
  const motion = { rest: { o: 1, tf: [0, 0, 1, 0] }, enter: { o: 0, tf: [0, 20, 1, 0] }, exit: null, repeat: null, t: { d: 0.3, delay: 0, ease: "ease" }, xt: { d: 0.2, delay: 0, ease: "ease" } };
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 1, css: "display:flex;gap:4px;", motion }] });
  assert.equal(styleOf(page.node(1)).gap, "4px");
  const anims = () => (page.document.animations || []).filter((a) => a.el === page.node(1));
  assert.equal(anims().length, 1, "entered");
  assert.deepEqual(JSON.parse(JSON.stringify(anims()[0].keyframes[0])), { opacity: 0, transform: "translate(0px,20px) rotate(0deg) scale(1)" });
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 1, css: "display:flex;", motion }] });
  assert.equal(anims().length, 1, "a restyle does not enter again");
  assert.equal(styleOf(page.node(1)).gap, "");
  // A reset (reconnect) shows what is there without animating it.
  const again = live([create(1, "view"), root(1), { op: "style", id: 1, css: "", motion }]);
  assert.equal((again.document.animations || []).length, 0);
});

test("node styles: one shared rule per distinct style, no style attribute, unused rules recycled", () => {
  const ids = Array.from({ length: 600 }, (_, i) => i + 2);
  const page = live([
    create(1, "view"), ...ids.map((id) => create(id, "view")),
    children(1, ids),
    { op: "style", id: 2, css: "gap:4px;" }, { op: "style", id: 3, css: "gap:4px;" },
    root(1),
  ]);
  const sheet = page.document.getElementById("prism-styles").sheet;
  const a = page.node(2), b = page.node(3);
  assert.equal(a.className, b.className, "same style, same class");
  assert.equal(a.hasAttribute("style"), false);
  assert.equal(sheet.cssRules.length, 1);
  // Named after a short hash of the style, the server's `style_class`.
  assert.equal(a.className, "pihnx3i");
  assert.equal(sheet.cssRules[0].selectorText, ":is(#prism-root, body) .pihnx3i");
  // Many distinct styles over time (a value that keeps changing): rules no
  // node uses any more are reused, so the sheet stays small.
  for (let round = 0; round < 3; round++) {
    page.ws.deliver({ t: "patch", p: ids.map((id) => ({ op: "style", id, css: `width:${round * 1000 + id}px;` })) });
  }
  assert.ok(sheet.cssRules.length < 1300, `${sheet.cssRules.length} rules`);
  assert.equal(cssOf(page.node(2)), "width:2002px;");
  assert.equal(cssOf(page.node(601)), "width:2601px;");
});

test("loops swing back and forth, or start over (spinners), with non-uniform scale", () => {
  const page = live([create(1, "view"), root(1)]);
  const t = { d: 0.9, delay: 0, ease: "linear" };
  const base = { enter: null, exit: null, t, xt: t };
  const anims = () => (page.document.animations || []).filter((a) => a.el === page.node(1) && !a.anim.cancelled);
  const spin = { ...base, rest: { o: 1, tf: [0, 0, 1, 0] }, repeat: { to: { o: 1, tf: [0, 0, 1, 360] }, t, restart: true } };
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 1, css: "", motion: spin }] });
  assert.equal(anims().length, 1);
  assert.equal(anims()[0].options.iterations, Infinity);
  assert.equal(anims()[0].options.direction, undefined, "starts over");
  assert.equal(anims()[0].options.easing, "linear", "keeps its curve");
  assert.equal(anims()[0].keyframes[1].transform, "translate(0px,0px) rotate(360deg) scale(1)");
  // A blink: vertical scale only, a label offset in percent kept while looping.
  const blink = { ...base, rest: { o: 1, tf: [0, 0, 1, 0, 1, 1, -50, 0] }, repeat: { to: { o: 1, tf: [0, 0, 1, 0, 1, 0.1, -50, 0] }, t } };
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 1, css: "", motion: blink }] });
  assert.equal(anims().length, 1, "the spin stopped");
  assert.equal(anims()[0].options.direction, "alternate");
  assert.equal(anims()[0].options.easing, "ease-in-out");
  assert.equal(anims()[0].keyframes[1].transform, "translate(calc(0px + -50%),calc(0px + 0%)) rotate(0deg) scale(1,0.1)");
});

test("interaction states: one style sheet for all nodes, groups flagged, rules gone on destroy and reset", () => {
  const page = live([create(1, "view"), create(2, "view"), root(1)]);
  const sheet = () => page.document.getElementById("prism-states");
  const hover = '@media (hover:hover){[data-pid="1"]:hover{opacity:0.5 !important;}}';
  const active = '[data-pid="2"]:active{opacity:0.2 !important;}';
  page.ws.deliver({ t: "patch", p: [
    { op: "style", id: 1, css: "display:flex;", states: hover, group: true },
    { op: "style", id: 2, css: "", states: active },
  ] });
  assert.equal(page.document.head.children.filter((c) => c.localName === "style" && c.id === "prism-states").length, 1);
  assert.equal(sheet().textContent, `${hover}\n${active}`);
  assert.equal(page.node(1).dataset.group, "");
  // A restyle replaces the node's rules; no rules: none left, no group.
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 1, css: "", states: "[data-pid=\"1\"]:hover{}" }] });
  assert.equal(sheet().textContent, `[data-pid="1"]:hover{}\n${active}`);
  assert.equal(page.node(1).dataset.group, undefined);
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 1, css: "" }] });
  assert.equal(sheet().textContent, active);
  page.ws.deliver({ t: "patch", p: [{ op: "destroy", id: 2 }] });
  assert.equal(sheet().textContent, "");
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 1, css: "", states: hover }] });
  page.ws.deliver({ t: "reset", p: [create(1, "view"), root(1)] });
  assert.equal(sheet().textContent, "", "a reset starts over");
});

test("the client's style sheets carry the host's CSP nonce", () => {
  const page = live([create(1, "view"), root(1)], { nonce: "n0nce" });
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 1, css: "gap:2px;", states: '[data-pid="1"]:active{opacity:0.2 !important;}' }] });
  assert.equal(page.document.getElementById("prism-states").nonce, "n0nce");
  assert.equal(page.document.getElementById("prism-styles").nonce, "n0nce");
});

test("stacks show the top screen, its title and a Back button", () => {
  const page = live([
    create(1, "stack"), textOp(1, "/"), create(2, "screen"), textOp(2, "Home"), children(1, [2]), root(1),
  ]);
  const stack = page.node(1);
  const back = stack.querySelector(".prism-back");
  assert.equal(stack.querySelector(".prism-title").textContent, "Home");
  assert.equal(back.hidden, true);
  page.ws.deliver({ t: "patch", p: [create(3, "screen"), textOp(3, "Detail"), textOp(1, "/todos/1"), children(1, [2, 3])] });
  assert.equal(page.node(2).hidden, true);
  assert.equal(page.node(3).hidden, false);
  assert.equal(page.node(3).getAttribute("aria-label"), "Detail");
  assert.equal(back.hidden, false);
  assert.equal(page.document.title, "Detail · Prism");
  assert.equal(page.window.location.pathname, "/todos/1");
  assert.equal(page.history.entries.length, 2, "a pushed history entry");
  back.click();
  assert.deepEqual(lastSent(page), { t: "back", id: 1, depth: 1 });
  // The browser's Back button too.
  page.history.index = 0;
  page.fire("popstate", { state: { prism: 1, tab: 0 } });
  assert.deepEqual(lastSent(page), { t: "back", id: 1, depth: 1 });
  // Labels follow the app's language.
  page.ws.deliver({ t: "patch", p: [{ op: "env", lang: "fr", dir: "ltr", back: "Retour", close: "Fermer" }] });
  assert.equal(back.textContent, "‹ Retour");
  assert.equal(back.getAttribute("aria-label"), "Retour");
});

test("sheets open as modal dialogs and Escape asks the app", () => {
  const page = live([
    create(1, "stack"), create(2, "screen"), children(1, [2]), root(1),
  ]);
  page.ws.deliver({ t: "patch", p: [create(3, "modal"), create(4, "stack"), { op: "value", id: 4, value: 1 }, create(5, "screen"), textOp(5, "Sheet"), children(4, [5]), children(3, [4]), children(1, [2, 3])] });
  const dialog = page.node(3);
  assert.equal(dialog.localName, "dialog");
  assert.equal(dialog.parentNode, page.document.body);
  assert.ok(dialog.hasAttribute("open"));
  assert.equal(page.node(2).hidden, false, "the screen underneath stays");
  // A closable stack's first screen shows Close.
  assert.equal(page.node(4).querySelector(".prism-back").textContent, "Close");
  const event = { type: "cancel" };
  dialog.dispatchEvent(event);
  assert.ok(event.defaultPrevented);
  assert.deepEqual(lastSent(page), { t: "back", id: 3, depth: 0, reason: "escape" });
  page.ws.deliver({ t: "patch", p: [children(1, [2]), { op: "destroy", id: 3 }] });
  assert.equal(dialog.isConnected, false);
});

test("lazy tabs: the shown panel stays until the server renders the new one", () => {
  const page = live(fixture.reset);
  const tabs = page.mount.querySelector("[data-kind=tabs]");
  const bar = tabs.querySelector(".prism-tabbar");
  const panels = () => tabs.querySelectorAll("[data-kind=tab]");
  const stackOf = (panel) => panel.querySelector("[data-kind=stack]");
  // The server rendered the selected tab only: the other has an empty stack.
  assert.equal(stackOf(panels()[1])._screens.length, 0);
  assert.ok(stackOf(panels()[0])._screens.length > 0);
  bar.children[1].click();
  // The tab is selected at once, but no empty panel flashes meanwhile.
  assert.deepEqual(bar.children.map((b) => b.getAttribute("aria-selected")), ["false", "true"]);
  assert.deepEqual(panels().map((p) => p.hidden), [false, true]);
  // The server's answer (the tab, now rendered) shows it.
  const answer = fixture.events.find((e) => e.send.t === "change" && e.send.id === Number(tabs.dataset.pid));
  page.ws.deliver({ t: "patch", p: answer.patches });
  assert.deepEqual(panels().map((p) => p.hidden), [true, false]);
  assert.ok(stackOf(panels()[1])._screens.length > 0);
  // Back to a rendered tab: switches at once, its nodes kept.
  const first = stackOf(panels()[0])._screens[0];
  bar.children[0].click();
  assert.deepEqual(panels().map((p) => p.hidden), [false, true]);
  assert.equal(stackOf(panels()[0])._screens[0], first);
});

test("lazy tabs: without an answer, the selected tab shows after a while", () => {
  const page = live(fixture.reset);
  const tabs = page.mount.querySelector("[data-kind=tabs]");
  tabs.querySelector(".prism-tabbar").children[1].click();
  const panels = () => tabs.querySelectorAll("[data-kind=tab]");
  assert.deepEqual(panels().map((p) => p.hidden), [false, true]);
  page.timers.at(-1).fn();
  assert.deepEqual(panels().map((p) => p.hidden), [true, false]);
});

test("tabs: keyboard moves the selection", () => {
  const page = live(fixture.reset);
  const tabs = page.mount.querySelector("[data-kind=tabs]");
  const bar = tabs.querySelector(".prism-tabbar");
  bar.dispatchEvent({ type: "keydown", key: "ArrowRight" });
  assert.deepEqual(lastSent(page), { t: "change", id: Number(tabs.dataset.pid), v: 1 });
  assert.equal(page.document.activeElement, bar.children[1]);
  bar.dispatchEvent({ type: "keydown", key: "ArrowRight" });
  assert.deepEqual(lastSent(page).v, 0, "wraps around");
  bar.dispatchEvent({ type: "keydown", key: "End" });
  assert.equal(lastSent(page).v, 1);
  const before = page.ws.sent.length;
  bar.dispatchEvent({ type: "keydown", key: "a" });
  assert.equal(page.ws.sent.length, before);
});

// --- Events ---------------------------------------------------------------------

test("clickable views: click, Enter and Space; plain views: nothing", () => {
  const page = live([create(1, "view"), create(2, "view"), children(1, [2]), root(1), { op: "clickable", id: 2, on: true }]);
  page.node(2).click();
  assert.deepEqual(lastSent(page), { t: "click", id: 2 });
  for (const key of ["Enter", " "]) {
    page.ws.sent.length = 0;
    const e = { type: "keydown", key };
    e.target = page.node(2);
    page.node(2).dispatchEvent(e);
    assert.deepEqual(sent(page), [{ t: "click", id: 2 }], JSON.stringify(key));
    assert.ok(e.defaultPrevented);
  }
  page.ws.sent.length = 0;
  page.node(1).click();
  assert.deepEqual(sent(page), [], "the parent is not clickable");
  page.ws.deliver({ t: "patch", p: [{ op: "clickable", id: 2, on: false }] });
  page.node(2).click();
  assert.deepEqual(sent(page), []);
});

test("scroll views report their offset once per frame", () => {
  const page = live([create(1, "scroll"), root(1), { op: "scrollev", id: 1, on: true }]);
  const el = page.node(1);
  el.clientHeight = 600;
  page.frames.splice(0).forEach((f) => f());
  page.frames.splice(0).forEach((f) => f());
  assert.deepEqual(lastSent(page), { t: "scroll", id: 1, o: 0, v: 600 });
  el.scrollTop = 120;
  el.dispatchEvent({ type: "scroll" });
  el.dispatchEvent({ type: "scroll" });
  assert.equal(page.frames.length, 1, "coalesced");
  page.frames.splice(0).forEach((f) => f());
  assert.deepEqual(lastSent(page), { t: "scroll", id: 1, o: 120, v: 600 });
});

test("pull to refresh and the refreshing banner", () => {
  const page = live([create(1, "scroll"), root(1), { op: "clickable", id: 1, on: true }]);
  const el = page.node(1);
  el.querySelector(".prism-refresh").click();
  assert.deepEqual(lastSent(page), { t: "click", id: 1 });
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 1, value: 1 }] });
  assert.ok(el.querySelector(":scope > .prism-refreshing"));
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 1, value: 0 }] });
  assert.equal(el.querySelector(":scope > .prism-refreshing"), null);
});

test("swipes are reported for the directions the app listens to", () => {
  const page = live([create(1, "view"), root(1), { op: "swipe", id: 1, left: true, right: false }]);
  const el = page.node(1);
  const swipe = (dx, pointerType = "touch") => {
    el.dispatchEvent({ type: "pointerdown", pointerType, clientX: 200, clientY: 100 });
    el.dispatchEvent({ type: "pointerup", pointerType, clientX: 200 + dx, clientY: 110 });
  };
  swipe(-100);
  assert.deepEqual(lastSent(page), { t: "swipe", id: 1, dir: "left" });
  page.ws.sent.length = 0;
  swipe(100);
  swipe(-30);
  swipe(-100, "mouse");
  assert.deepEqual(sent(page), [], "right is not listened to; short and mouse drags are not swipes");
});

test("context menus open on right-click and report the chosen item", () => {
  const menu = { title: "", items: [
    { t: "act", id: 65537, title: "Rename", icon: null, shortcut: null, role: "normal", checked: null, enabled: true },
    { t: "sep" },
    { t: "act", id: 65538, title: "Delete", icon: null, shortcut: null, role: "destructive", checked: null, enabled: false },
    { t: "sub", title: "More", icon: null, menu: { title: "More", items: [
      { t: "act", id: 65539, title: "Pin", icon: null, shortcut: null, role: "normal", checked: true, enabled: true },
    ] } },
  ] };
  const page = live([create(1, "view"), root(1), { op: "menu", id: 1, menu }]);
  const el = page.node(1);
  assert.equal(el.getAttribute("aria-haspopup"), "menu");
  const e = { type: "contextmenu", clientX: 10, clientY: 10 };
  el.dispatchEvent(e);
  assert.ok(e.defaultPrevented);
  const shown = page.document.body.querySelector(".prism-menu");
  const items = shown.querySelectorAll("[role^=menuitem]");
  assert.deepEqual(items.map((b) => b.lastChild.textContent), ["Rename", "Delete", "Pin"]);
  assert.equal(items[1].disabled, true);
  assert.equal(items[2].getAttribute("role"), "menuitemcheckbox");
  assert.equal(items[2].getAttribute("aria-checked"), "true");
  assert.equal(page.document.activeElement, items[0], "the first enabled item has the focus");
  items[2].click();
  assert.deepEqual(lastSent(page), { t: "menu", id: 65539 });
  assert.equal(shown.isConnected, false);
  page.ws.deliver({ t: "patch", p: [{ op: "menu", id: 1, menu: null }] });
  assert.equal(el.getAttribute("aria-haspopup"), null);
});

test("alerts are dialogs; Escape picks Cancel", () => {
  const page = live([create(1, "view"), root(1)]);
  page.ws.deliver({ t: "alert", a: { id: 9, title: "Delete?", message: "Gone for good", actions: [{ title: "Delete", role: "destructive" }, { title: "Cancel", role: "cancel" }] } });
  const d = page.document.body.querySelector(".prism-alert");
  assert.equal(d.getAttribute("role"), "alertdialog");
  assert.equal(d.querySelector("h2").textContent, "Delete?");
  const buttons = d.querySelector(".prism-alert-actions").children;
  assert.equal(page.document.activeElement, buttons[1], "Cancel is focused");
  d.dispatchEvent({ type: "cancel" });
  assert.deepEqual(lastSent(page), { t: "alert", id: 9, index: 1 });
  assert.equal(d.isConnected, false);
  page.ws.deliver({ t: "alert", a: { id: 10, title: "OK?", message: null, actions: [{ title: "OK", role: "normal" }] } });
  page.document.body.querySelector(".prism-alert").querySelector("button").click();
  assert.deepEqual(lastSent(page), { t: "alert", id: 10, index: 0 });
});

test("notifications without browser permission become in-page toasts", () => {
  const page = live([create(1, "view"), root(1)]);
  page.ws.deliver({ t: "notify", id: "n1", title: "Hi", subtitle: null, body: "there", link: "/todos/1", delay: null, actions: [] });
  const toast = page.document.getElementById("prism-toasts").firstElementChild;
  assert.equal(toast.getAttribute("aria-label"), "Hi");
  assert.equal(toast.children[1].textContent, "there");
  toast.click();
  assert.deepEqual(lastSent(page), { t: "notif", id: "n1", action: null, text: null, link: "/todos/1" });
  page.ws.deliver({ t: "notify", id: "n2", title: "Task", subtitle: null, body: "", link: null, delay: null, actions: [{ id: "done", title: "Done", destructive: false, reply: null, link: null }] });
  page.document.querySelector(".prism-toast-actions").querySelector("button").click();
  assert.deepEqual(lastSent(page), { t: "notif", id: "n2", action: "done", text: null, link: null });
});

test("device calls answer with the request id", async () => {
  const page = live([create(1, "view"), root(1)]);
  page.window.navigator.clipboard = { readText: async () => "copied", writeText: async (t) => { page.window.copied = t; } };
  page.ws.deliver({ t: "device", id: 4, call: "paste" });
  page.ws.deliver({ t: "device", id: 5, call: "copy", text: "hello" });
  page.ws.deliver({ t: "device", id: 6, call: "permission_status", permission: "photos" });
  page.ws.deliver({ t: "device", id: 7, call: "location", accuracy: "best" });
  await new Promise((r) => setImmediate(r));
  const replies = page.ws.sent.filter((m) => m.t === "device_reply").sort((a, b) => a.id - b.id);
  assert.deepEqual(replies, [
    { t: "device_reply", id: 4, text: "copied" },
    { t: "device_reply", id: 6, permission: "granted" },
    { t: "device_reply", id: 7, err: "unavailable" },
  ]);
  assert.equal(page.window.copied, "hello");
});

/** A picked file of `bytes`; `hold` makes reading its chunks wait for `release()`. */
function fakeFile(name, type, bytes, hold) {
  const waiting = [];
  return {
    name, type, size: bytes.length,
    slice: (a, b) => ({
      arrayBuffer: () => new Promise((r) => {
        const done = () => r(bytes.slice(a, b).buffer);
        if (hold) waiting.push(done); else done();
      }),
    }),
    release: () => waiting.splice(0).forEach((f) => f()),
  };
}
/** The file inputs the client creates from now on. */
function fileInputs(page) {
  const inputs = [];
  const make = page.document.createElement.bind(page.document);
  page.document.createElement = (tag) => {
    const el = make(tag);
    if (tag === "input") inputs.push(el);
    return el;
  };
  return inputs;
}
const settle = async () => { for (let i = 0; i < 20; i++) await new Promise((r) => setImmediate(r)); };

test("picked files go up in chunks, then the reply (#112)", async () => {
  const page = live([create(1, "view"), root(1)]);
  const inputs = fileInputs(page);
  page.ws.deliver({ t: "device", id: 5, call: "pick_files", types: [], multiple: true });
  const big = new Uint8Array(300 * 1024).map((_, i) => i % 251);
  inputs[0].files = [fakeFile("big.bin", "application/octet-stream", big), fakeFile("a.txt", "text/plain", new TextEncoder().encode("hi"))];
  inputs[0].dispatchEvent({ type: "change" });
  await settle();
  const msgs = page.ws.sent.filter((m) => m.id === 5);
  assert.deepEqual(msgs.map((m) => m.t), ["upload_start", "upload_chunk", "upload_chunk", "upload_chunk", "device_reply"]);
  assert.deepEqual(msgs[0].files, [
    { name: "big.bin", mime: "application/octet-stream", size: 300 * 1024 },
    { name: "a.txt", mime: "text/plain", size: 2 },
  ]);
  const chunks = msgs.slice(1, 4);
  assert.deepEqual(chunks.map((c) => c.file), [0, 0, 1]);
  assert.equal(Buffer.from(chunks[0].data, "base64").length, 256 * 1024, "one chunk at most");
  const first = Buffer.concat(chunks.slice(0, 2).map((c) => Buffer.from(c.data, "base64")));
  assert.deepEqual([...first], [...big]);
  assert.equal(Buffer.from(chunks[2].data, "base64").toString(), "hi");
  assert.deepEqual(msgs[4], { t: "device_reply", id: 5, uploaded: true });
});

test("a cancelled upload stops sending and says so (#112)", async () => {
  const page = live([create(1, "view"), root(1)]);
  const inputs = fileInputs(page);
  page.ws.deliver({ t: "device", id: 6, call: "pick_images", limit: 1, camera: false });
  const photo = fakeFile("cat.jpg", "image/jpeg", new Uint8Array(600 * 1024), true);
  inputs[0].files = [photo];
  inputs[0].dispatchEvent({ type: "change" });
  await settle();
  photo.release();
  await settle();
  // `cx.cancel_upload`: a stop for the call, while the second chunk is read.
  page.ws.deliver({ t: "device", id: 6, call: "stop" });
  photo.release();
  await settle();
  const msgs = page.ws.sent.filter((m) => m.id === 6);
  assert.deepEqual(msgs.map((m) => m.t), ["upload_start", "upload_chunk", "device_reply"]);
  assert.deepEqual(msgs.at(-1), { t: "device_reply", id: 6, err: "cancelled" });
});

test("files dropped on a drop target go up, only those it accepts (#111)", async () => {
  const page = live([
    create(1, "view"), create(2, "view"), create(3, "view"),
    { op: "drop", id: 2, types: ["image/*"] },
    { op: "children", id: 1, children: [2, 3] }, root(1),
  ]);
  const zone = page.node(2);
  const png = fakeFile("cat.png", "image/png", new Uint8Array([1, 2, 3]));
  const txt = fakeFile("notes.txt", "text/plain", new TextEncoder().encode("hi"));
  const drag = (type, el, files = []) => {
    const e = { type, dataTransfer: { types: ["Files"], files, dropEffect: "" } };
    el.dispatchEvent(e);
    return e;
  };
  // Hovering files: the `drop_hover` state, kept over its children.
  assert.ok(drag("dragenter", zone).defaultPrevented);
  assert.equal(zone.getAttribute("data-prism-drop"), "");
  assert.equal(drag("dragover", zone).dataTransfer.dropEffect, "copy");
  drag("dragenter", zone);
  drag("dragleave", zone);
  assert.equal(zone.getAttribute("data-prism-drop"), "", "still over it");
  drag("dragleave", zone);
  assert.equal(zone.getAttribute("data-prism-drop"), null);
  // Text being dragged is not files: left to the browser.
  const text = { type: "dragenter", dataTransfer: { types: ["text/plain"] } };
  zone.dispatchEvent(text);
  assert.equal(text.defaultPrevented, false);
  // Elsewhere on the page: no drop target, nothing happens.
  drag("drop", page.node(3), [png]);
  assert.deepEqual(page.ws.sent.filter((m) => m.t === "drop_start"), []);
  // The drop: only the image goes up, then the end.
  drag("dragenter", zone);
  assert.ok(drag("drop", zone, [png, txt]).defaultPrevented);
  assert.equal(zone.getAttribute("data-prism-drop"), null);
  await settle();
  const msgs = page.ws.sent.filter((m) => m.t.startsWith("drop") || (m.t === "upload_chunk" && m.drop));
  assert.deepEqual(msgs, [
    { t: "drop_start", drop: 1, node: 2, files: [{ name: "cat.png", mime: "image/png", size: 3 }] },
    { t: "upload_chunk", drop: 1, file: 0, data: "AQID" },
    { t: "drop_end", drop: 1 },
  ]);
  // No longer a drop target: drops are left alone.
  page.ws.deliver({ t: "patch", p: [{ op: "drop", id: 2, types: null }] });
  assert.equal(drag("drop", zone, [png]).defaultPrevented, false);
});

test("passkeys of the app's relying party run with the server's options (#94)", async () => {
  const page = live([create(1, "view"), root(1)]);
  const seen = [];
  const buf = (...b) => new Uint8Array(b).buffer;
  page.window.PublicKeyCredential = function () {};
  page.window.navigator.credentials = {
    // A browser without toJSON / parse*FromJSON: Prism converts.
    create: async (o) => {
      seen.push(o.publicKey);
      return { id: "AQI", rawId: buf(1, 2), type: "public-key", authenticatorAttachment: "platform",
        getClientExtensionResults: () => ({}),
        response: { clientDataJSON: buf(123, 125), attestationObject: buf(9), getTransports: () => ["internal"] } };
    },
    // One with toJSON.
    get: async (o) => { seen.push(o.publicKey); return { toJSON: () => ({ id: "AQI", type: "public-key" }) }; },
  };
  page.ws.deliver({ t: "device", id: 4, call: "webauthn_rp", create: true, options: {
    challenge: "AQID", rp: { id: "example.com", name: "Example" },
    user: { id: "BAU", name: "ada", displayName: "Ada" },
    pubKeyCredParams: [{ type: "public-key", alg: -7 }],
    excludeCredentials: [{ type: "public-key", id: "Bg" }],
  } });
  page.ws.deliver({ t: "device", id: 5, call: "webauthn_rp", create: false, options: {
    challenge: "AQID", rpId: "example.com", allowCredentials: [{ type: "public-key", id: "Bg" }],
  } });
  await new Promise((r) => setImmediate(r));
  const [made, asked] = seen;
  assert.deepEqual([...new Uint8Array(made.challenge)], [1, 2, 3]);
  assert.deepEqual([...new Uint8Array(made.user.id)], [4, 5]);
  assert.equal(made.user.name, "ada");
  assert.deepEqual([...new Uint8Array(made.excludeCredentials[0].id)], [6]);
  assert.equal(made.rp.id, "example.com");
  assert.deepEqual([...new Uint8Array(asked.allowCredentials[0].id)], [6]);
  const replies = page.ws.sent.filter((m) => m.t === "device_reply").sort((a, b) => a.id - b.id);
  assert.deepEqual(JSON.parse(replies[0].text), {
    id: "AQI", rawId: "AQI", type: "public-key", authenticatorAttachment: "platform", clientExtensionResults: {},
    response: { clientDataJSON: "e30", attestationObject: "CQ", transports: ["internal"] },
  });
  assert.deepEqual(JSON.parse(replies[1].text), { id: "AQI", type: "public-key" });
  // A passkey already on the device: the error keeps its name (#189).
  page.window.navigator.credentials.create = async () => {
    throw Object.assign(new Error("The authenticator was previously registered"), { name: "InvalidStateError" });
  };
  page.ws.deliver({ t: "device", id: 7, call: "webauthn_rp", create: true, options: {
    challenge: "AQID", rp: { id: "example.com", name: "Example" }, user: { id: "BAU", name: "ada", displayName: "Ada" },
  } });
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(page.ws.sent.filter((m) => m.id === 7),
    [{ t: "device_reply", id: 7, err: "InvalidStateError\u001fThe authenticator was previously registered" }]);
  // Without WebAuthn: unavailable.
  delete page.window.PublicKeyCredential;
  page.ws.deliver({ t: "device", id: 6, call: "webauthn_rp", create: false, options: { challenge: "AQID" } });
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(page.ws.sent.filter((m) => m.id === 6), [{ t: "device_reply", id: 6, err: "unavailable" }]);
});

test("the current .link() of a navigation is the current page (#144)", () => {
  const page = live([create(2, "view"), { op: "link", id: 2, href: "/orders" }, { op: "a11y", id: 2, a11y: { current: true } }, root(2)]);
  assert.equal(page.node(2).localName, "a");
  assert.equal(page.node(2).getAttribute("aria-current"), "page");
});

test("the app's title wins over the screens', and a reset clears it (#154)", () => {
  const page = boot();
  page.ws.open();
  page.ws.deliver({ t: "reset", p: [create(1, "view"), root(1)], title: "Orders" });
  assert.equal(page.document.title, "Orders · Prism");
  page.ws.deliver({ t: "title", title: "Order 42" });
  assert.equal(page.document.title, "Order 42 · Prism");
  page.ws.deliver({ t: "title", title: null });
  assert.equal(page.document.title, "Prism");
  page.ws.deliver({ t: "title", title: "Orders" });
  page.ws.deliver({ t: "reset", p: [create(1, "view"), root(1)] });
  assert.equal(page.document.title, "Prism", "a session without a title drops the old one");
});

test("with unsaved changes, leaving the page asks first (#158)", () => {
  const page = live([create(1, "view"), root(1)]);
  const leave = () => {
    const e = { type: "beforeunload", returnValue: undefined, preventDefault() { this.defaultPrevented = true; } };
    page.window.dispatchEvent(e);
    return e;
  };
  assert.equal(leave().defaultPrevented, undefined, "nothing to lose: no question");
  page.ws.deliver({ t: "unsaved", on: true });
  const e = leave();
  assert.equal(e.defaultPrevented, true);
  assert.equal(e.returnValue, "");
  page.ws.deliver({ t: "unsaved", on: false });
  assert.equal(leave().defaultPrevented, undefined);
  // A new session says it again (a reconnect drops a stale state).
  page.ws.deliver({ t: "unsaved", on: true });
  page.ws.deliver({ t: "reset", p: [create(1, "view"), root(1)], unsaved: false });
  assert.equal(leave().defaultPrevented, undefined);
});

test("window resizes are reported once settled", () => {
  const page = live([create(1, "view"), root(1)]);
  page.window.innerWidth = 390;
  page.fire("resize");
  const timer = page.timers.at(-1);
  assert.equal(timer.ms, 120);
  timer.fn();
  assert.deepEqual(lastSent(page), { t: "viewport", w: 390, h: 800, safe: [0, 0, 0, 0] });
  assert.ok(page.document.cookies.some((c) => c.startsWith("prism_vw=390x800")));
});

test("resizes report the safe-area insets (#28)", () => {
  const page = live([create(1, "view"), root(1)]);
  const css = page.window.getComputedStyle;
  page.window.getComputedStyle = (el) => cssOf(el).includes("safe-area-inset")
    ? { paddingTop: "47px", paddingRight: "0px", paddingBottom: "34px", paddingLeft: "59px" }
    : css(el);
  page.fire("resize");
  page.timers.at(-1).fn();
  assert.deepEqual(lastSent(page).safe, [47, 0, 34, 59]);
});

test("a closed socket reconnects with back-off and the reset replaces the page", () => {
  const page = live([create(1, "text"), textOp(1, "old"), root(1)]);
  const html = page.document.documentElement;
  const banner = page.document.getElementById("prism-connection");
  const states = [];
  page.window.addEventListener("prism:connection", (e) => states.push(e.detail.state));
  assert.equal(html.getAttribute("data-prism-connection"), "online");
  assert.ok(banner.hidden);
  // Each failure waits about twice as long (±25 %), up to 30 s.
  const delays = [];
  for (let i = 0; i < 9; i++) {
    page.sockets.at(-1).onclose();
    delays.push(page.timers.at(-1).ms);
    page.timers.at(-1).fn();
  }
  const base = [500, 1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000];
  delays.forEach((d, i) => assert.ok(d >= base[i] * 0.75 && d <= base[i] * 1.25, `${i}: ${d}`));
  assert.equal(html.getAttribute("data-prism-connection"), "reconnecting");
  assert.ok(!banner.hidden, "the banner shows while reconnecting");
  assert.deepEqual(states, ["reconnecting"]);
  // Connected again: the counter restarts, the banner hides, the first
  // paint's style rules go once the live tree replaced it.
  const style = page.document.createElement("style");
  style.setAttribute("id", "prism-ssr");
  page.document.head.appendChild(style);
  const ws = page.sockets.at(-1);
  ws.open();
  ws.deliver({ t: "reset", p: [create(1, "text"), textOp(1, "new"), root(1)] });
  assert.equal(page.mount.textContent, "new");
  assert.equal(page.mount.children.length, 1);
  assert.equal(page.document.getElementById("prism-ssr"), null);
  assert.ok(banner.hidden);
  assert.deepEqual(states, ["reconnecting", "online"]);
  ws.onclose();
  assert.ok(page.timers.at(-1).ms <= 625, "back to the shortest delay");
});

test("failed reconnects fetch the page and reload when the host refuses it", async () => {
  for (const [answer, reloads] of [
    [{ status: 401, type: "basic" }, true],
    [{ status: 403, type: "basic" }, true],
    [{ status: 0, type: "opaqueredirect" }, true],
    [{ status: 200, type: "basic" }, false],
  ]) {
    const page = live([create(1, "text"), root(1)], { fetch: async () => answer, path: "/orders" });
    page.ws.onclose();
    assert.equal(page.window.fetches.length, 0, "one failure is a blip");
    page.timers.at(-1).fn();
    page.sockets.at(-1).onclose();
    assert.equal(page.window.fetches.length, 1);
    const { url, init } = page.window.fetches[0];
    assert.equal(url, "http://localhost:3000/orders");
    assert.equal(init.redirect, "manual");
    await new Promise((r) => setImmediate(r));
    assert.equal(!!page.window.reloaded, reloads, JSON.stringify(answer));
  }
  // The server unreachable: offline, still retrying.
  const page = live(null, { fetch: async () => { throw new Error("network"); } });
  page.ws.onclose();
  page.timers.at(-1).fn();
  page.sockets.at(-1).onclose();
  await new Promise((r) => setImmediate(r));
  assert.equal(page.document.documentElement.getAttribute("data-prism-connection"), "offline");
  assert.ok(!page.window.reloaded);
});

test("a session the server refused probes at once", async () => {
  const page = live(null, { fetch: async () => ({ status: 200, type: "basic" }) });
  page.ws.deliver({ t: "refused", status: 403 });
  page.ws.onclose();
  assert.equal(page.window.fetches.length, 1);
});

test("set_cookie posts each ticket in a header, one after the other, and reports refusals", async () => {
  const answers = [];
  const page = live(null, { fetch: (url, init) => new Promise((resolve) => answers.push({ url, init, resolve })) });
  const flush = () => new Promise((r) => setImmediate(r));
  page.ws.deliver({ t: "device", id: 7, call: "set_cookie", url: "/app/_prism/cookie", ticket: "T1" });
  page.ws.deliver({ t: "device", id: 8, call: "set_cookie", url: "/app/_prism/cookie", ticket: "T2" });
  await flush();
  // The second waits for the first: the browser stores them in order.
  assert.equal(answers.length, 1);
  const { url, init } = answers[0];
  assert.equal(url, "/app/_prism/cookie");
  assert.equal(init.method, "POST");
  assert.equal(init.credentials, "same-origin");
  assert.equal(init.cache, "no-store");
  assert.deepEqual(Object.entries(init.headers), [["X-Prism-Cookie", "T1"]]);
  answers[0].resolve({ status: 204 });
  await flush();
  assert.equal(answers.length, 2);
  assert.equal(answers[1].init.headers["X-Prism-Cookie"], "T2");
  answers[1].resolve({ status: 403 });
  await flush();
  // The server answers successes itself (it redeemed the ticket); the page
  // reports what failed.
  const replies = page.ws.sent.filter((m) => m.t === "device_reply");
  assert.deepEqual(replies, [{ t: "device_reply", id: 8, err: "the cookie was refused (403)" }]);
});

test("reconnect waits for the cookies, then opens a new session at once without the banner", async () => {
  const answers = [];
  const page = live([create(1, "text"), root(1)], { fetch: () => new Promise((resolve) => answers.push(resolve)) });
  const flush = () => new Promise((r) => setImmediate(r));
  const html = page.document.documentElement;
  const states = [];
  page.window.addEventListener("prism:connection", (e) => states.push(e.detail.state));
  page.ws.deliver({ t: "device", id: 3, call: "set_cookie", url: "/_prism/cookie", ticket: "T" });
  page.ws.deliver({ t: "reconnect" });
  await flush();
  assert.ok(!page.ws.closed, "the cookie is not stored yet");
  answers[0]({ status: 204 });
  await flush();
  assert.ok(page.ws.closed, "the page closes the old session");
  await flush();
  assert.equal(page.sockets.length, 2, "a new session at once");
  const timers = page.timers.length;
  assert.equal(html.getAttribute("data-prism-connection"), "online");
  assert.ok(page.document.getElementById("prism-connection").hidden);
  assert.deepEqual(states, []);
  page.sockets[1].open();
  assert.equal(page.timers.length, timers, "no retry scheduled");
  // The server closing first works the same.
  page.sockets[1].deliver({ t: "reconnect" });
  page.sockets[1].close();
  await flush();
  assert.equal(page.sockets.length, 3);
  assert.deepEqual(states, []);
});

test("an idle session waits for the user before reconnecting", () => {
  const page = live([create(1, "text"), root(1)]);
  page.ws.deliver({ t: "idle" });
  const timers = page.timers.length;
  page.ws.onclose();
  assert.equal(page.timers.length, timers, "no retry scheduled");
  assert.equal(page.document.documentElement.getAttribute("data-prism-connection"), "idle");
  assert.ok(page.document.getElementById("prism-connection").hidden);
  page.fire("pointerdown");
  assert.equal(page.sockets.length, 2);
});

test("the socket carries the page's query", () => {
  const page = live(null, { path: "/orders", search: "?page=2&q=a b" });
  const url = new URL(page.ws.url);
  assert.equal(url.searchParams.get("search"), "?page=2&q=a b");
});

test("open_url stays in the tab for this origin, opens a new one elsewhere", () => {
  const page = live([create(1, "text"), root(1)]);
  const open = (url, target) => page.ws.deliver({ t: "device", id: 1, call: "open_url", url, target });
  open("/logout");
  assert.equal(page.window.assigned, "/logout");
  open("http://localhost:3000/reports/1.pdf");
  assert.equal(page.window.assigned, "http://localhost:3000/reports/1.pdf");
  assert.equal(page.window.opened, undefined);
  open("https://example.com/");
  assert.deepEqual(page.window.opened, { url: "https://example.com/", target: "_blank", features: "noopener" });
  // An explicit target wins.
  page.window.opened = undefined;
  open("/help", "new");
  assert.equal(page.window.opened.url, "/help");
  open("https://example.com/sign-out", "same");
  assert.equal(page.window.assigned, "https://example.com/sign-out");
});

// --- Shapes ----------------------------------------------------------------------

/** The `d` of each path a drawing resolves to in a `w`×`h` box. */
function draw(drawing, w = 100, h = 50) {
  const page = live([create(1, "shape"), root(1)]);
  const el = page.node(1);
  el.clientWidth = w;
  el.clientHeight = h;
  page.ws.deliver({ t: "patch", p: [textOp(1, drawing)] });
  const svg = el.firstElementChild;
  return { page, el, svg, paths: svg.children };
}

test("shapes: fractions of the box become pixels", () => {
  const { svg, paths } = draw("i0,0\nf#FF0000FF |M 0 0 L 1 0 L 1 1 Z\n");
  assert.equal(svg.getAttribute("viewBox"), "0 0 100 50");
  assert.equal(svg.getAttribute("aria-hidden"), "true");
  assert.equal(paths.length, 1);
  assert.equal(paths[0].getAttribute("d"), "M0 0L100 0L100 50Z");
  assert.equal(paths[0].style.fill, "#FF0000FF");
  assert.equal(paths[0].style.stroke, "none");
});

test("shapes: insets, strokes, dashes, round caps and adaptive colors", () => {
  const { paths } = draw("i10,5\ns#000000FF/#FFFFFFFF w3 r d4,2 |M 0 0.5 L 1 0.5\n");
  assert.equal(paths[0].getAttribute("d"), "M10 25L90 25");
  assert.equal(paths[0].style.stroke, "light-dark(#000000FF, #FFFFFFFF)");
  assert.equal(paths[0].style.fill, "none");
  assert.equal(paths[0].style.strokeWidth, "3");
  assert.equal(paths[0].style.strokeLinecap, "round");
  assert.equal(paths[0].style.strokeDasharray, "4 2");
});

test("shapes: the even-odd fill rule (SVG pictures with holes)", () => {
  const { paths } = draw("i0,0\nf#000000FF e |M 0 0 L 1 0 L 1 1 Z M 0.2 0.2 L 0.8 0.2 L 0.8 0.8 Z\nf#000000FF |M 0 0 L 1 1\n");
  assert.equal(paths[0].style.fillRule, "evenodd");
  assert.ok(!paths[1].style.fillRule);
});

test("shapes: curves, and lines after Z start a new subpath", () => {
  const { paths } = draw("i0,0\n|C 0 0 0.5 1 1 0 Z L 0.5 0.5\n", 100, 100);
  // A curve without a start moves to its first control point; a line after
  // Z moves first (nothing is open).
  assert.equal(paths[0].getAttribute("d"), "M0 0C0 0 50 100 100 0ZM50 50");
});

test("shapes: arcs in degrees from 12 o'clock, full circles in two halves", () => {
  const near = (d) => d.replace(/-?\d+\.\d+(e-?\d+)?/g, (n) => String(Math.round(Number(n) * 1000) / 1000));
  // Radius 0.5 of the smaller side (50): 25px, centered.
  const half = draw("i0,0\n|A 0.5 0.5 0.5 0 180\n", 100, 50).paths[0].getAttribute("d");
  assert.equal(near(half), "M50 0A25 25 0 0 1 50 50");
  const circle = draw("i0,0\n|A 0.5 0.5 0.5 0 360\n", 100, 50).paths[0].getAttribute("d");
  assert.equal(near(circle), "M50 0A25 25 0 0 1 50 50A25 25 0 0 1 50 0");
  // Counter-clockwise sweeps use the other sweep flag.
  const ccw = draw("i0,0\n|A 0.5 0.5 0.5 90 0\n", 100, 50).paths[0].getAttribute("d");
  assert.equal(near(ccw), "M75 25A25 25 0 0 0 50 0");
  // Beyond a full turn is clamped to one.
  const many = draw("i0,0\n|A 0.5 0.5 0.5 0 1000\n", 100, 50).paths[0].getAttribute("d");
  assert.equal(near(many), near(circle));
});

test("shapes: unknown commands stop the path, bad lines are skipped, resizes redraw", () => {
  const { page, el, paths } = draw("i0,0\nno bar here\n|M 0 0 L 1 1 Q 1 1 L 0 0\n");
  assert.equal(paths.length, 1);
  assert.equal(paths[0].getAttribute("d"), "M0 0L100 50");
  // The resize observer redraws at the new size.
  el.clientWidth = 200;
  const ro = page.window.resizeObservers[0];
  assert.ok(ro.targets.includes(el));
  ro.cb([{ target: el }]);
  assert.equal(el.firstElementChild.children[0].getAttribute("d"), "M0 0L200 50");
  assert.equal(el.querySelectorAll("svg").length, 1, "one svg, reused");
});

test("timers count down by themselves", () => {
  const now = Math.floor(Date.now() / 1000);
  const page = live([create(1, "timer"), root(1), { op: "value", id: 1, value: now + 3725 }]);
  assert.match(page.node(1).textContent, /^1:02:0[45]$/);
  const short = live([create(1, "timer"), root(1), { op: "value", id: 1, value: now + 65 }]);
  assert.match(short.node(1).textContent, /^1:0[45]$/);
  const past = live([create(1, "timer"), root(1), { op: "value", id: 1, value: now - 10 }]);
  assert.equal(past.node(1).textContent, "0:00");
});

test("query strings: the socket, pushes, replaces and Back keep them", () => {
  const page = live(null, { path: "/app/analytics?period=7d&room=3", base: "/app/" });
  assert.equal(new URL(page.ws.url).searchParams.get("path"), "/analytics?period=7d&room=3");
  page.ws.deliver({ t: "reset", p: [
    create(1, "stack"), textOp(1, "/analytics?period=7d&room=3"), create(2, "screen"), textOp(2, "Analytics"),
    children(1, [2]), root(1),
  ] });
  assert.equal(page.window.location.pathname, "/app/analytics");
  assert.equal(page.window.location.search, "?period=7d&room=3");
  // `cx.replace_query`: same screen, the URL is replaced, not pushed.
  page.ws.deliver({ t: "patch", p: [textOp(1, "/analytics?period=30d")] });
  assert.equal(page.window.location.search, "?period=30d");
  assert.equal(page.history.entries.length, 1, "no new history entry");
  // `cx.navigate` to another query pushes a screen: Back undoes it.
  page.ws.deliver({ t: "patch", p: [create(3, "screen"), textOp(3, "Analytics"), textOp(1, "/analytics?period=1d"), children(1, [2, 3])] });
  assert.equal(page.history.entries.length, 2);
  assert.equal(page.history.entries[1].url, "/app/analytics?period=1d");
  page.history.index = 0;
  page.fire("popstate", { state: { prism: 1, tab: 0 } });
  assert.deepEqual(lastSent(page), { t: "back", id: 1, depth: 1 });
  // The root path with a query, mounted under a prefix.
  page.ws.deliver({ t: "patch", p: [textOp(1, "/?tab=2"), children(1, [2])] });
  assert.equal(page.window.location.pathname, "/app");
  assert.equal(page.window.location.search, "?tab=2");
  // Forward asks the app for the path, query included.
  page.history.entries.push({ state: { prism: 2, tab: 0 }, url: "/app/analytics?period=1d" });
  page.history.index = 1;
  page.window.location.pathname = "/app/analytics";
  page.window.location.search = "?period=1d";
  page.fire("popstate", { state: { prism: 2, tab: 0 } });
  assert.deepEqual(lastSent(page), { t: "navigate", id: 1, path: "/analytics?period=1d" });
});

test("the first screen names the page once live, header or not (#185)", () => {
  for (const value of [0, 2]) {
    const page = live([
      create(1, "stack"), { op: "value", id: 1, value }, textOp(1, "/login"),
      create(2, "screen"), textOp(2, "Sign in"), children(1, [2]), root(1),
    ]);
    assert.equal(page.document.title, "Sign in · Prism", `stack value ${value}`);
  }
});

test("frames of reporting elements are sent unscrolled, once per change (#192)", () => {
  const page = live([create(1, "scroll"), create(2, "view"), children(1, [2]), root(1)]);
  const list = page.node(1), row = page.node(2);
  row.clientWidth = 300; row.clientHeight = 24;
  page.ws.deliver({ t: "patch", p: [{ op: "layout", id: 2, on: true }] });
  flush(page);
  const layouts = () => page.ws.sent.filter((m) => m.t === "layout");
  assert.deepEqual(layouts(), [{ t: "layout", id: 2, x: 0, y: 0, w: 300, h: 24 }]);
  // Scrolling moves it on screen, not in layout coordinates: nothing sent.
  list.scrollTop = 80;
  row.getBoundingClientRect = () => ({ left: 0, top: -80, width: 300, height: 24 });
  page.ws.deliver({ t: "patch", p: [] });
  flush(page);
  assert.equal(layouts().length, 1);
  // A resize is.
  row.getBoundingClientRect = () => ({ left: 0, top: -80, width: 300, height: 48 });
  page.ws.deliver({ t: "patch", p: [] });
  flush(page);
  assert.deepEqual(layouts().at(-1), { t: "layout", id: 2, x: 0, y: 0, w: 300, h: 48 });
});

test("the app scrolls a scroll view by or to an offset (#193)", () => {
  const page = live([create(1, "scroll"), root(1)]);
  const list = page.node(1);
  const calls = [];
  list.scrollBy = (o) => calls.push(["by", o]);
  list.scrollTo = (o) => calls.push(["to", o]);
  page.ws.deliver({ t: "patch", p: [
    { op: "scrollto", id: 1, x: 0, y: 24, rel: true, anim: false },
    { op: "scrollto", id: 1, x: 0, y: 1e9, rel: false, anim: true },
  ] });
  assert.deepEqual(JSON.parse(JSON.stringify(calls)), [
    ["by", { left: 0, top: 24, behavior: "auto" }],
    ["to", { left: 0, top: 1e9, behavior: "smooth" }],
  ]);
});

test("the header bar hides for a whole stack or a bare screen", () => {
  const page = live([
    create(1, "stack"), { op: "value", id: 1, value: 0 }, create(2, "screen"), textOp(2, "Home"),
    { op: "value", id: 2, value: 0 }, children(1, [2]), root(1),
  ]);
  const header = page.node(1).querySelector(".prism-header");
  assert.equal(header.hidden, false);
  // A bare screen on top hides it; popping back shows it again.
  page.ws.deliver({ t: "patch", p: [create(3, "screen"), textOp(3, "Photo"), { op: "value", id: 3, value: 1 }, children(1, [2, 3])] });
  assert.equal(header.hidden, true);
  assert.equal(page.document.title, "Photo · Prism", "the title still names the page");
  page.ws.deliver({ t: "patch", p: [children(1, [2]), { op: "destroy", id: 3 }] });
  assert.equal(header.hidden, false);
  // A screen turning bare while on top.
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 2, value: 1 }] });
  assert.equal(header.hidden, true);
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 2, value: 0 }] });
  assert.equal(header.hidden, false);
  // `Router::header(false)`: flag 2, independent of Close (flag 1).
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 1, value: 2 }] });
  assert.equal(header.hidden, true);
  assert.equal(page.node(1).querySelector(".prism-back").hidden, true, "not closable");
  page.ws.deliver({ t: "patch", p: [{ op: "value", id: 1, value: 3 }] });
  assert.equal(header.hidden, true);
  assert.equal(page.node(1).querySelector(".prism-back").textContent, "Close");
});

test("own-width rules: the element takes the CSS of the widest threshold it reaches", () => {
  const cq = [[300, "display:flex;flex-direction:row;"], [600, "display:grid;"]];
  const page = live([create(1, "view"), root(1)]);
  const el = page.node(1);
  el.clientWidth = 400;
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 1, css: "display:flex;flex-direction:column;", cq }] });
  // Measured at once: 400 wide is past 300.
  assert.equal(cssOf(el), "display:flex;flex-direction:row;");
  const ro = page.window.resizeObservers.find((o) => o.targets.includes(el));
  assert.ok(ro, "observed");
  ro.cb([{ target: el, contentRect: { width: 200 } }]);
  assert.equal(cssOf(el), "display:flex;flex-direction:column;");
  ro.cb([{ target: el, borderBoxSize: [{ inlineSize: 800 }] }]);
  assert.equal(cssOf(el), "display:grid;");
  // No rules anymore: the plain CSS stays, resizes change nothing.
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 1, css: "display:flex;" }] });
  ro.cb([{ target: el, contentRect: { width: 900 } }]);
  assert.equal(cssOf(el), "display:flex;");
});

test("horizontal scroll views get their class from the style op", () => {
  const page = live([create(1, "scroll"), root(1), { op: "style", id: 1, css: "", hx: true }]);
  const el = page.node(1);
  assert.equal(el.getAttribute("class"), "prism-scroll prism-scroll-x");
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 1, css: "" }] });
  assert.equal(el.getAttribute("class"), "prism-scroll");
});

test("scroll bars follow the style op (#132)", () => {
  const page = live([create(1, "scroll"), root(1), { op: "style", id: 1, css: "", sb: "hidden" }]);
  const el = page.node(1);
  assert.equal(el.getAttribute("data-prism-scrollbar"), "hidden");
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 1, css: "", sb: "overlay" }] });
  assert.equal(el.getAttribute("data-prism-scrollbar"), "overlay");
  page.ws.deliver({ t: "patch", p: [{ op: "style", id: 1, css: "" }] });
  assert.equal(el.getAttribute("data-prism-scrollbar"), null);
});

// Rich text runs (prism-core spans.rs): U+001E between runs, U+001F between fields.
const RS = "\u001e", FS = "\u001f";
const runsConfig = [
  "Signed in as ",
  ["Ada", "w=700", "c=#FF0000FF/#00FF00FF", "z=1.5"].join(FS),
  ["settings", "u", "l=42", "h=/settings"].join(FS),
  ["★", "g=star.fill"].join(FS),
  ["tap", "l=43", "f=mono"].join(FS),
  ["docs", "l=44", "h=https://example.com/docs"].join(FS),
].join(RS);

test("rich text: runs become spans, links and symbols; links report their id", () => {
  const page = live([
    create(1, "view"), create(2, "text"), children(1, [2]), root(1),
    textOp(2, "Signed in as Adasettings★tapdocs"),
    { op: "config", id: 2, config: runsConfig },
  ], { base: "/app" });
  const el = page.node(2);
  assert.equal(el.textContent, "Signed in as Adasettings★tapdocs");
  const [plain, bold, link, symbol, run, external] = el.childNodes;
  assert.equal(plain.nodeType, 3);
  assert.equal(bold.localName, "span");
  assert.match(cssOf(bold), /font-weight:\s*700/);
  assert.match(cssOf(bold), /light-dark\(#FF0000FF, #00FF00FF\)/);
  assert.match(cssOf(bold), /font-size:\s*1.5em/);
  assert.equal(link.localName, "a");
  assert.equal(link.getAttribute("href"), "/app/settings", "app paths under the mount point");
  assert.equal(external.getAttribute("href"), "https://example.com/docs");
  assert.match(cssOf(link), /text-decoration-line:\s*underline/);
  assert.equal(symbol.getAttribute("aria-hidden"), "true");
  assert.equal(run.getAttribute("role"), "link");
  assert.equal(run.getAttribute("tabindex"), "0");

  // A click on a link: the app navigates (no page load).
  const click = { type: "click" };
  click.target = link;
  link.dispatchEvent(click);
  assert.ok(click.defaultPrevented);
  assert.deepEqual(lastSent(page), { t: "menu", id: 42 });
  // Enter on a clickable run without a path.
  const key = { type: "keydown", key: "Enter" };
  key.target = run;
  run.dispatchEvent(key);
  assert.deepEqual(lastSent(page), { t: "menu", id: 43 });
  assert.ok(key.defaultPrevented);
  // A text without click handler sends nothing for clicks outside links.
  page.ws.sent.length = 0;
  el.click();
  assert.deepEqual(sent(page), []);

  // A text update keeps the runs; clearing the runs shows the plain text.
  page.ws.deliver({ t: "patch", p: [textOp(2, "Signed in as Adasettings★tapdocs!")] });
  assert.equal(el.childNodes.length, 6);
  page.ws.deliver({ t: "patch", p: [{ op: "config", id: 2, config: "" }] });
  assert.equal(el.textContent, "Signed in as Adasettings★tapdocs!");
  assert.equal(el.childNodes.length, 1);
});

test("links are anchors: a plain click stays in the app, a modified one is the browser's (#59)", () => {
  const page = live([
    create(1, "view"),
    create(2, "view"), { op: "link", id: 2, href: "/orders/42" },
    { op: "clickable", id: 2, on: true }, { op: "a11y", id: 2, a11y: { role: "link" } },
    create(3, "text"), { op: "link", id: 3, href: "mailto:ada@example.com" }, textOp(3, "Mail"),
    { op: "clickable", id: 3, on: true }, { op: "a11y", id: 3, a11y: { role: "link" } },
    create(4, "text"), { op: "link", id: 4, href: "/profile" },
    { op: "config", id: 4, config: runsConfig },
    children(1, [2, 3, 4]), root(1),
  ], { base: "/app" });
  const row = page.node(2);
  assert.equal(row.localName, "a");
  assert.equal(row.getAttribute("href"), "/app/orders/42", "app paths under the mount point");
  assert.equal(row.getAttribute("rel"), null);
  // A link by itself: no role, focusable without a tabindex.
  assert.equal(row.getAttribute("role"), null);
  assert.equal(row.getAttribute("tabindex"), null);
  assert.equal(page.mount.firstChild.firstChild, row, "in its place");
  const mail = page.node(3);
  assert.equal(mail.localName, "a");
  assert.equal(mail.getAttribute("href"), "mailto:ada@example.com");
  assert.equal(mail.getAttribute("rel"), "noopener");
  assert.equal(mail.textContent, "Mail");

  // A plain click: the app navigates (no page load).
  const plain = { type: "click", detail: 0 };
  row.dispatchEvent(plain);
  assert.ok(plain.defaultPrevented);
  assert.deepEqual(lastSent(page), { t: "click", id: 2 });
  // Cmd/Ctrl/Shift-click, a middle click: the browser opens it (new tab,
  // window); the app hears nothing.
  page.ws.sent.length = 0;
  for (const mod of [{ metaKey: true }, { ctrlKey: true }, { shiftKey: true }, { button: 1 }]) {
    const e = { type: "click", detail: 1, ...mod };
    row.dispatchEvent(e);
    assert.ok(!e.defaultPrevented, JSON.stringify(mod));
  }
  assert.deepEqual(sent(page), []);
  // Disabled: neither follows it.
  page.ws.deliver({ t: "patch", p: [{ op: "a11y", id: 2, a11y: { role: "link", disabled: true } }] });
  assert.equal(row.getAttribute("tabindex"), "-1");
  const disabled = { type: "click", detail: 1, metaKey: true };
  row.dispatchEvent(disabled);
  assert.ok(disabled.defaultPrevented);
  assert.deepEqual(sent(page), []);

  // A text that is a link: its runs' links are spans (links don't nest),
  // still reported by their click.
  const linked = page.node(4);
  assert.equal(linked.localName, "a");
  assert.equal(linked.querySelectorAll("a").length, 0);
  const run = linked.querySelectorAll(".prism-link")[0];
  assert.equal(run.localName, "span");
  assert.equal(run.getAttribute("role"), null);
  const click = { type: "click" };
  run.dispatchEvent(click);
  assert.ok(click.defaultPrevented);
  assert.deepEqual(lastSent(page), { t: "menu", id: 42 });

  // A draggable link: the app's drag, not the browser's link drag.
  page.ws.deliver({ t: "patch", p: [{ op: "pointer", id: 2, mask: 4 }] });
  assert.equal(row.getAttribute("draggable"), "false");
});

test("clickable texts: click, Enter and Space, focusable as buttons", () => {
  const page = live([
    create(1, "view"), create(2, "text"), children(1, [2]), root(1), textOp(2, "Tap me"),
    { op: "clickable", id: 2, on: true },
    { op: "a11y", id: 2, a11y: { role: "button" } },
  ]);
  const el = page.node(2);
  assert.equal(el.getAttribute("role"), "button");
  assert.equal(el.getAttribute("tabindex"), "0");
  el.click();
  assert.deepEqual(lastSent(page), { t: "click", id: 2 });
  for (const k of ["Enter", " "]) {
    page.ws.sent.length = 0;
    const e = { type: "keydown", key: k };
    e.target = el;
    el.dispatchEvent(e);
    assert.deepEqual(sent(page), [{ t: "click", id: 2 }], JSON.stringify(k));
  }
  page.ws.deliver({ t: "patch", p: [{ op: "clickable", id: 2, on: false }] });
  page.ws.sent.length = 0;
  el.click();
  assert.deepEqual(sent(page), []);
});

// #50: a node's first style is not a change, so `transition` does not
// animate it; later changes do.
test("a new node is styled before it joins the page: its first style is not animated", () => {
  const page = live([create(1, "view"), root(1)]);
  const css = "padding:24px;background:#f28b6b;transition:all 0.42s ease-in-out 0s;";
  const order = [];
  const mount = page.node(1);
  const insert = mount.insertBefore.bind(mount);
  mount.insertBefore = (kid, ref) => { order.push(`insert ${kid.dataset.pid} ${cssOf(kid)}`); return insert(kid, ref); };
  page.window.getComputedStyle = (el) => { order.push(`computed ${el.dataset.pid}`); return { position: "static" }; };
  page.ws.deliver({ t: "patch", p: [create(2, "view"), { op: "style", id: 2, css }, children(1, [2])] });
  // Its class was there when it was inserted, and nothing computed its
  // style before: the browser's first style for it has the class.
  assert.deepEqual(order, [`insert 2 ${css}`]);
});

test("own-width rules: the rule for a new node's first width applies without a transition", () => {
  const cq = [[200, "padding:28px;transition:all 0.42s ease-in-out 0s;"]];
  const base = "padding:16px;transition:all 0.42s ease-in-out 0s;";
  const page = live([create(1, "view"), root(1)]);
  page.ws.deliver({ t: "patch", p: [create(2, "view"), { op: "style", id: 2, css: base, cq }, children(1, [2])] });
  const el = page.node(2);
  // Not laid out yet when styled: the base style.
  assert.equal(cssOf(el), base);
  const flushes = [];
  page.window.getComputedStyle = (n) => { flushes.push(cssOf(n)); return { position: "static" }; };
  const ro = page.window.resizeObservers.find((o) => o.targets.includes(el));

  // First width: the variant's style is computed with `transition: none`…
  ro.cb([{ target: el, borderBoxSize: [{ inlineSize: 386 }] }]);
  assert.deepEqual(flushes, [cq[0][1] + "transition:none;"]);
  // …then its own transition is back, and no `style` attribute is left.
  assert.equal(cssOf(el), cq[0][1]);
  assert.equal(el.hasAttribute("style"), false);

  // Later widths are changes: they animate (no flush, no inline style).
  ro.cb([{ target: el, borderBoxSize: [{ inlineSize: 120 }] }]);
  assert.equal(cssOf(el), base);
  ro.cb([{ target: el, borderBoxSize: [{ inlineSize: 300 }] }]);
  assert.equal(flushes.length, 1);
  assert.equal(cssOf(el), cq[0][1]);
});

test("own-width rules: an inline style the client set is kept", () => {
  const cq = [[200, "padding:28px;"]];
  const page = live([create(1, "view"), root(1)]);
  page.ws.deliver({ t: "patch", p: [create(2, "view"), { op: "style", id: 2, css: "padding:16px;", cq }, children(1, [2])] });
  const el = page.node(2);
  el.style.position = "relative";
  el.style.transition = "opacity 1s";
  const ro = page.window.resizeObservers.find((o) => o.targets.includes(el));
  ro.cb([{ target: el, contentRect: { width: 300 } }]);
  assert.equal(el.style.transition, "opacity 1s");
  assert.equal(el.style.position, "relative");
  assert.equal(cssOf(el), "padding:28px;position:relative;transition:opacity 1s;");
});

test("run-time fonts become FontFaces, once each (#81)", () => {
  const page = live([create(1, "view"), root(1)]);
  page.ws.deliver({ t: "font", family: "Lora", url: "https://fonts.example/lora.woff2", weight: "400 700", style: "normal" });
  page.ws.deliver({ t: "font", family: "Lora", url: "https://fonts.example/lora.woff2", weight: "400 700", style: "normal" });
  // "OTTO" in base64url, without padding.
  page.ws.deliver({ t: "font", family: "Brand", data: "T1RUTw", style: "italic" });
  const [lora, brand, ...rest] = page.document.fonts.added;
  assert.equal(rest.length, 0, "the same face is added once");
  assert.equal(lora.family, "Lora");
  assert.equal(lora.source, 'url("https://fonts.example/lora.woff2")');
  assert.deepEqual({ ...lora.descriptors }, { style: "normal", display: "swap", weight: "400 700" });
  assert.equal(brand.family, "Brand");
  assert.deepEqual([...brand.source], [0x4f, 0x54, 0x54, 0x4f]);
  assert.deepEqual({ ...brand.descriptors }, { style: "italic", display: "swap" });
});

test("app stylesheets follow the set the server sends, first paint's included (#81)", () => {
  const page = boot({ nonce: "n0nce" });
  const ssr = page.document.createElement("link");
  ssr.setAttribute("rel", "stylesheet");
  ssr.setAttribute("href", "/fonts.css");
  ssr.setAttribute("data-prism-sheet", "");
  page.document.head.appendChild(ssr);
  page.ws.open();
  const hrefs = () => page.document.querySelectorAll("link[data-prism-sheet]").map((l) => l.getAttribute("href"));
  page.ws.deliver({ t: "reset", p: [create(1, "view"), root(1)], sheets: ["/fonts.css", "/picker.css"] });
  assert.deepEqual(hrefs(), ["/fonts.css", "/picker.css"], "the first paint's link is kept");
  assert.equal(page.document.querySelector('link[href="/picker.css"]').nonce, "n0nce");
  page.ws.deliver({ t: "sheets", urls: ["/picker.css"] });
  assert.deepEqual(hrefs(), ["/picker.css"]);
  page.ws.deliver({ t: "sheets", urls: [] });
  assert.deepEqual(hrefs(), []);
  // A new session without stylesheets drops the previous one's.
  page.ws.deliver({ t: "sheets", urls: ["/old.css"] });
  page.ws.deliver({ t: "reset", p: [create(1, "view"), root(1)] });
  assert.deepEqual(hrefs(), []);
});
