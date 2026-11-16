// Compressed socket messages (`prism_web::Frame`): the page asks for them
// when it can inflate them, inflates binary frames with DecompressionStream,
// and applies every message in the order the server sent it.
//
// Run: node --test crates/prism-web/tests/js/
import { test } from "node:test";
import assert from "node:assert/strict";
import { boot } from "./dom.mjs";

const create = (id, kind) => ({ op: "create", id, kind });
const root = (id) => ({ op: "root", id });
const children = (id, kids) => ({ op: "children", id, children: kids });
const textOp = (id, text) => ({ op: "text", id, text });

/** A first tree with one text node (2) under a view (1). */
const tree = (text) => [create(1, "view"), create(2, "text"), textOp(2, text), children(1, [2]), root(1)];

/** Lets inflating (Node streams) and the promise chain run. */
const settle = async (page, done) => {
  for (let i = 0; i < 200; i++) {
    if (done()) return;
    await new Promise((r) => setTimeout(r, 2));
  }
  assert.fail("messages were not applied");
};

function connected(options) {
  const page = boot(options);
  page.ws.open();
  return page;
}

test("the socket asks for compressed frames only when the browser can inflate them", () => {
  const page = connected();
  assert.equal(new URL(page.ws.url).searchParams.get("compress"), "deflate-raw");
  assert.equal(page.ws.binaryType, "arraybuffer");
  const old = connected({ decompress: false });
  assert.equal(new URL(old.ws.url).searchParams.get("compress"), null);
  // Still path, tz, w, h as before.
  const u = new URL(old.ws.url);
  assert.ok(["path", "tz", "w", "h"].every((k) => u.searchParams.has(k)), u.search);
});

test("a compressed reset is inflated and applied", async () => {
  const page = connected();
  page.ws.deliverCompressed({ t: "reset", p: tree("hello") });
  // Inflating is asynchronous: nothing yet.
  assert.equal(page.node(2), null);
  await settle(page, () => page.node(2));
  assert.equal(page.node(2).textContent, "hello");
});

test("a small text patch never overtakes a compressed message sent before it", async () => {
  const page = connected();
  page.ws.deliverCompressed({ t: "reset", p: tree("first") });
  // Arrives while the reset is still inflating: applied after it, on its node.
  page.ws.deliver({ t: "patch", p: [textOp(2, "second")] });
  assert.equal(page.node(2), null, "queued behind the reset");
  page.ws.deliverCompressed({ t: "patch", p: [create(3, "text"), textOp(3, "third"), children(1, [2, 3])] });
  page.ws.deliver({ t: "patch", p: [textOp(3, "fourth")] });
  await settle(page, () => page.node(3) && page.node(3).textContent === "fourth");
  assert.equal(page.node(2).textContent, "second");
  assert.deepEqual(page.node(1).children.map((c) => c.textContent), ["second", "fourth"]);
  // Once the queue is empty, text messages apply at once again.
  await new Promise((r) => setTimeout(r, 5));
  page.ws.deliver({ t: "patch", p: [textOp(2, "now")] });
  assert.equal(page.node(2).textContent, "now");
});

test("text messages alone apply synchronously, as before", () => {
  const page = connected();
  page.ws.deliver({ t: "reset", p: tree("sync") });
  assert.equal(page.node(2).textContent, "sync");
});

test("a frame that cannot be inflated closes the socket to get a fresh tree", async () => {
  const page = connected();
  page.ws.deliver({ t: "reset", p: tree("kept") });
  const errors = [];
  page.window.console = { ...console, error: (...a) => errors.push(a.join(" ")) };
  page.ws.deliverBinary(new Uint8Array([0xff, 0xfe, 0xfd, 0x00, 0x13]));
  page.ws.deliver({ t: "patch", p: [textOp(2, "after")] });
  await settle(page, () => page.ws.closed);
  assert.match(errors.join("\n"), /cannot read a message/);
  // The messages after it still apply, in order.
  await settle(page, () => page.node(2).textContent === "after");
  // A reconnect is scheduled.
  assert.ok(page.timers.length > 0);
});

test("a browser without DecompressionStream that still gets a binary frame reconnects", async () => {
  const page = connected({ decompress: false });
  page.ws.deliver({ t: "reset", p: tree("plain") });
  page.window.console = { ...console, error: () => {} };
  page.ws.deliverCompressed({ t: "patch", p: [textOp(2, "lost")] });
  await settle(page, () => page.ws.closed);
  assert.equal(page.node(2).textContent, "plain");
});
