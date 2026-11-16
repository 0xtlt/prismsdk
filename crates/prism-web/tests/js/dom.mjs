// A tiny DOM, just enough to run client.js in Node (no dependencies): elements,
// text nodes, attributes, dataset, inline styles, events, a small HTML parser
// (the client's own `innerHTML` snippets and server-rendered pages) and simple
// selectors (`.class`, `tag`, `[attr]`, `[attr=v]`, `[attr^=v]`, `:not(:disabled)`,
// `:scope > …`).
import fs from "node:fs";
import vm from "node:vm";
import zlib from "node:zlib";

const VOID = new Set(["input", "img", "br", "meta", "link", "hr"]);
const kebab = (s) => s.replace(/[A-Z]/g, (c) => "-" + c.toLowerCase());
const camel = (s) => s.replace(/-([a-z])/g, (_, c) => c.toUpperCase());

class Node {
  constructor(doc) {
    this.ownerDocument = doc;
    this.parentNode = null;
    this.childNodes = [];
  }
  get parentElement() { return this.parentNode && this.parentNode.nodeType === 1 ? this.parentNode : null; }
  get firstChild() { return this.childNodes[0] || null; }
  get lastChild() { return this.childNodes[this.childNodes.length - 1] || null; }
  get nextSibling() {
    const s = this.parentNode && this.parentNode.childNodes;
    return s ? s[s.indexOf(this) + 1] || null : null;
  }
  get isConnected() {
    let n = this;
    while (n.parentNode) n = n.parentNode;
    return n === this.ownerDocument;
  }
  remove() {
    if (!this.parentNode) return;
    const s = this.parentNode.childNodes;
    s.splice(s.indexOf(this), 1);
    this.parentNode = null;
  }
  contains(n) {
    for (; n; n = n.parentNode) if (n === this) return true;
    return false;
  }
}

export class Text extends Node {
  constructor(doc, data) { super(doc); this.nodeType = 3; this.data = String(data); }
  get textContent() { return this.data; }
  set textContent(v) { this.data = String(v); }
  cloneNode() { return new Text(this.ownerDocument, this.data); }
}

/** `el.style`: a declaration list readable as properties and as `cssText`. */
function makeStyle() {
  const decls = new Map();
  const target = {};
  return new Proxy(target, {
    get(_, prop) {
      if (prop === "cssText") return [...decls].map(([k, v]) => `${k}:${v};`).join("");
      if (prop === "setProperty") return (k, v) => decls.set(k, String(v));
      if (prop === "getPropertyValue") return (k) => decls.get(k) || "";
      if (prop === "_decls") return decls;
      if (typeof prop !== "string") return undefined;
      return decls.get(kebab(prop)) || "";
    },
    set(_, prop, value) {
      if (prop === "cssText") {
        decls.clear();
        for (const d of String(value).split(";")) {
          const i = d.indexOf(":");
          if (i > 0) decls.set(d.slice(0, i).trim(), d.slice(i + 1).trim());
        }
      } else if (value === "" || value === null || value === undefined) {
        decls.delete(kebab(prop));
      } else {
        decls.set(kebab(prop), String(value));
      }
      return true;
    },
  });
}

export class Element extends Node {
  constructor(doc, tag, ns) {
    super(doc);
    this.nodeType = 1;
    this.localName = tag.toLowerCase();
    this.tagName = ns ? tag : tag.toUpperCase();
    this.namespaceURI = ns || "http://www.w3.org/1999/xhtml";
    this.attributes = new Map();
    this.style = makeStyle();
    this.listeners = new Map();
    // Form state (properties, not attributes, like the real DOM).
    this.value = "";
    this.checked = false;
    this.disabled = false;
    this.clientWidth = 0;
    this.clientHeight = 0;
    this.scrollTop = 0;
    const el = this;
    this.dataset = new Proxy({}, {
      get: (_, k) => (typeof k === "string" ? el.getAttribute("data-" + kebab(k)) ?? undefined : undefined),
      set: (_, k, v) => { el.setAttribute("data-" + kebab(k), String(v)); return true; },
      has: (_, k) => el.hasAttribute("data-" + kebab(k)),
      deleteProperty: (_, k) => { el.removeAttribute("data-" + kebab(k)); return true; },
    });
  }
  getAttribute(name) {
    if (name === "style") return this.style.cssText;
    return this.attributes.has(name) ? this.attributes.get(name) : null;
  }
  setAttribute(name, value) {
    if (name === "style") { this.style.cssText = value; return; }
    this.attributes.set(name, String(value));
  }
  hasAttribute(name) { return name === "style" ? this.style.cssText !== "" : this.attributes.has(name); }
  removeAttribute(name) { if (name === "style") this.style.cssText = ""; else this.attributes.delete(name); }
  // Reflected attributes.
  get id() { return this.getAttribute("id") || ""; }
  set id(v) { this.setAttribute("id", v); }
  get className() { return this.getAttribute("class") || ""; }
  set className(v) { this.setAttribute("class", v); }
  get classList() {
    const el = this;
    const list = () => el.className.split(/\s+/).filter(Boolean);
    return {
      contains: (c) => list().includes(c),
      add: (...cs) => { el.className = [...new Set([...list(), ...cs])].join(" "); },
      remove: (...cs) => { el.className = list().filter((c) => !cs.includes(c)).join(" "); },
      toggle: (c, on = !list().includes(c)) => {
        if (on) el.classList.add(c); else el.classList.remove(c);
        return on;
      },
    };
  }
  /** A `<style>`'s CSSOM sheet: rules added with `insertRule`. */
  get sheet() {
    if (this.localName !== "style") return null;
    if (!this._sheet) {
      const cssRules = [];
      this._sheet = {
        cssRules,
        insertRule(text, i = 0) {
          const at = text.indexOf("{");
          const rule = { selectorText: text.slice(0, at).trim(), style: makeStyle() };
          rule.style.cssText = text.slice(at + 1, text.lastIndexOf("}"));
          cssRules.splice(i, 0, rule);
          return i;
        },
      };
    }
    return this._sheet;
  }
  get hidden() { return this.hasAttribute("hidden"); }
  set hidden(v) { if (v) this.setAttribute("hidden", ""); else this.removeAttribute("hidden"); }
  get type() { return this.getAttribute("type") || ""; }
  set type(v) { this.setAttribute("type", v); }
  get placeholder() { return this.getAttribute("placeholder") || ""; }
  set placeholder(v) { this.setAttribute("placeholder", v); }
  get alt() { return this.getAttribute("alt") || ""; }
  set alt(v) { this.setAttribute("alt", v); }
  get src() { return this.getAttribute("src") || ""; }
  set src(v) { this.setAttribute("src", v); }
  get tabIndex() { const t = this.getAttribute("tabindex"); return t === null ? -1 : Number(t); }
  set tabIndex(v) { this.setAttribute("tabindex", String(v)); }
  get children() { return this.childNodes.filter((n) => n.nodeType === 1); }
  // A `<select>`: its options and the selected one (the first when none is).
  get options() { return this.children.filter((c) => c.tagName === "OPTION"); }
  get selectedIndex() {
    const o = this.options;
    const i = o.findIndex((c) => c._selected);
    return i >= 0 ? i : o.length ? 0 : -1;
  }
  set selectedIndex(v) { this.options.forEach((c, i) => { c._selected = i === v; }); }
  get firstElementChild() { return this.children[0] || null; }
  get lastElementChild() { const c = this.children; return c[c.length - 1] || null; }
  get nextElementSibling() {
    const s = this.parentNode ? this.parentNode.childNodes : [];
    for (let i = s.indexOf(this) + 1; i < s.length; i++) if (s[i].nodeType === 1) return s[i];
    return null;
  }
  get textContent() { return this.childNodes.map((n) => n.textContent).join(""); }
  set textContent(v) { this.replaceChildren(); if (v !== "" && v !== null && v !== undefined) this.append(String(v)); }
  set innerHTML(html) { this.replaceChildren(...parseHTML(this.ownerDocument, html)); }
  get innerHTML() { return this.childNodes.map(serialize).join(""); }
  get outerHTML() { return serialize(this); }

  _adopt(n) {
    if (typeof n === "string") n = new Text(this.ownerDocument, n);
    if (n.contains && n.contains(this)) throw new Error("HierarchyRequestError");
    n.remove();
    n.parentNode = this;
    return n;
  }
  appendChild(n) { this.childNodes.push(this._adopt(n)); return n; }
  append(...ns) { ns.forEach((n) => this.appendChild(n)); }
  prepend(...ns) { ns.reverse().forEach((n) => this.insertBefore(n, this.firstChild)); }
  insertBefore(n, ref) {
    if (!ref) return this.appendChild(n);
    if (n === ref) return n;
    const node = this._adopt(n);
    this.childNodes.splice(this.childNodes.indexOf(ref), 0, node);
    return n;
  }
  replaceChildren(...ns) {
    for (const c of [...this.childNodes]) c.remove();
    this.append(...ns);
  }
  cloneNode(deep) {
    const c = new Element(this.ownerDocument, this.tagName, this.namespaceURI === "http://www.w3.org/1999/xhtml" ? undefined : this.namespaceURI);
    for (const [k, v] of this.attributes) c.attributes.set(k, v);
    c.style.cssText = this.style.cssText;
    if (deep) this.childNodes.forEach((n) => c.appendChild(n.cloneNode(true)));
    return c;
  }

  addEventListener(type, fn) {
    if (!this.listeners.has(type)) this.listeners.set(type, []);
    this.listeners.get(type).push(fn);
  }
  removeEventListener(type, fn) {
    const l = this.listeners.get(type);
    if (l) l.splice(l.indexOf(fn) >>> 0, 1);
  }
  /** Dispatches `event` here, then bubbles it to the ancestors. */
  dispatchEvent(event) {
    event.target ??= this;
    event.defaultPrevented ??= false;
    let stopped = false;
    event.preventDefault = () => { event.defaultPrevented = true; };
    event.stopPropagation = () => { stopped = true; };
    for (let n = this; n && !stopped; n = n.parentNode) {
      event.currentTarget = n;
      for (const fn of [...((n.listeners && n.listeners.get(event.type)) || [])]) fn.call(n, event);
      if (event.bubbles === false) break;
    }
    return !event.defaultPrevented;
  }
  // A field's value and caret: writing the value puts the caret at its end,
  // like browsers do.
  get value() { return this._value; }
  set value(v) {
    this._value = v;
    if (typeof v === "string") this.selectionStart = this.selectionEnd = v.length;
    this.valueWrites = (this.valueWrites || 0) + 1;
  }
  setSelectionRange(start, end) { this.selectionStart = start; this.selectionEnd = end; }
  click() { this.dispatchEvent({ type: "click" }); }
  focus() { this.ownerDocument.activeElement = this; }
  blur() { if (this.ownerDocument.activeElement === this) this.ownerDocument.activeElement = this.ownerDocument.body; }
  getBoundingClientRect() { return { left: 0, top: 0, width: this.clientWidth, height: this.clientHeight, right: this.clientWidth, bottom: this.clientHeight }; }
  animate(keyframes, options) {
    const a = { keyframes, options, cancelled: false, cancel() { a.cancelled = true; }, finished: Promise.resolve() };
    (this.ownerDocument.animations ||= []).push({ el: this, ...a, anim: a });
    return a;
  }
  // Like browsers: a dialog or popover outside the document cannot be shown.
  showModal() {
    if (!this.isConnected) throw new Error("InvalidStateError: showModal on a disconnected dialog");
    this.setAttribute("open", "");
  }
  close() { this.removeAttribute("open"); }
  showPopover() {
    if (!this.hasAttribute("popover")) throw new Error("NotSupportedError: not a popover");
    if (!this.isConnected) throw new Error("InvalidStateError: showPopover on a disconnected element");
    this._popoverOpen = true;
  }
  hidePopover() { this._popoverOpen = false; }

  matches(selector) { return matches(this, selector); }
  closest(selector) {
    for (let n = this; n && n.nodeType === 1; n = n.parentNode) if (matches(n, selector)) return n;
    return null;
  }
  querySelectorAll(selector) {
    const out = [];
    const direct = selector.startsWith(":scope >");
    const sel = direct ? selector.slice(8).trim() : selector;
    const walk = (n) => n.children.forEach((c) => {
      if (matches(c, sel)) out.push(c);
      if (!direct) walk(c);
    });
    walk(this);
    return out;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
}

function matches(el, selector) {
  return selector.split(",").some((s) => matchCompound(el, s.trim()));
}

function matchCompound(el, sel) {
  const re = /([a-zA-Z][\w-]*)|\.([\w-]+)|\[([\w-]+)(?:(\^?=)([^\]]*))?\]|:not\(:disabled\)|:popover-open/g;
  let m, consumed = 0;
  while ((m = re.exec(sel))) {
    if (m.index !== consumed) throw new Error(`dom.mjs: unsupported selector ${sel}`);
    consumed = re.lastIndex;
    if (m[1] && el.localName !== m[1].toLowerCase()) return false;
    if (m[2] && !el.className.split(/\s+/).includes(m[2])) return false;
    if (m[3]) {
      const v = el.getAttribute(m[3]);
      if (v === null) return false;
      const want = m[5] && m[5].replace(/^["']|["']$/g, "");
      if (m[4] === "=" && v !== want) return false;
      if (m[4] === "^=" && !v.startsWith(want)) return false;
    }
    if (m[0] === ":not(:disabled)" && el.disabled) return false;
    if (m[0] === ":popover-open" && !el._popoverOpen) return false;
  }
  if (consumed !== sel.length) throw new Error(`dom.mjs: unsupported selector ${sel}`);
  return true;
}

const ENTITIES = { amp: "&", lt: "<", gt: ">", quot: '"', "#39": "'", nbsp: " " };
const decode = (s) => s.replace(/&(#?\w+);/g, (m, e) => ENTITIES[e] ?? m);
const escapeText = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** Parses well-formed markup (what Prism produces) into nodes. */
export function parseHTML(doc, html) {
  const root = new Element(doc, "template");
  const stack = [root];
  const re = /<\/([\w-]+)\s*>|<([\w-]+)((?:\s+[\w:-]+(?:="[^"]*")?)*)\s*\/?>|([^<]+)/g;
  let m;
  while ((m = re.exec(html))) {
    const top = stack[stack.length - 1];
    if (m[1]) {
      if (top.localName !== m[1].toLowerCase()) throw new Error(`dom.mjs: </${m[1]}> closes <${top.localName}>`);
      stack.pop();
    } else if (m[2]) {
      const el = new Element(doc, m[2]);
      for (const a of m[3].matchAll(/([\w:-]+)(?:="([^"]*)")?/g)) el.setAttribute(a[1], decode(a[2] ?? ""));
      if (el.localName === "input") {
        el.value = el.getAttribute("value") ?? "";
        el.checked = el.hasAttribute("checked");
      }
      top.appendChild(el);
      if (!VOID.has(el.localName)) stack.push(el);
    } else if (m[4]) {
      top.appendChild(new Text(doc, decode(m[4])));
    }
  }
  if (stack.length !== 1) throw new Error(`dom.mjs: unclosed <${stack[stack.length - 1].localName}>`);
  return [...root.childNodes];
}

function serialize(n) {
  if (n.nodeType === 3) return escapeText(n.data);
  const attrs = [...n.attributes].map(([k, v]) => ` ${k}="${v.replace(/"/g, "&quot;")}"`).join("");
  const style = n.style.cssText ? ` style="${n.style.cssText}"` : "";
  if (VOID.has(n.localName)) return `<${n.localName}${attrs}${style}>`;
  return `<${n.localName}${attrs}${style}>${n.childNodes.map(serialize).join("")}</${n.localName}>`;
}

export class Document extends Element {
  constructor() {
    super(null, "#document");
    this.ownerDocument = this;
    this.nodeType = 9;
    this.documentElement = new Element(this, "html");
    this.appendChild(this.documentElement);
    this.head = new Element(this, "head");
    this.body = new Element(this, "body");
    this.documentElement.append(this.head, this.body);
    this.activeElement = this.body;
    this.title = "";
    this.cookies = [];
    this.hidden = false;
    this.currentScript = null;
  }
  get cookie() { return this.cookies.map((c) => c.split(";")[0]).join("; "); }
  set cookie(v) { this.cookies.push(v); }
  hasFocus() { return true; }
  createElement(tag) {
    const el = new Element(this, tag);
    // A defined custom element is created upgraded.
    if (this.registry) this.registry.upgrade(el);
    return el;
  }
  createElementNS(ns, tag) { return new Element(this, tag, ns); }
  createTextNode(data) { return new Text(this, data); }
  getElementById(id) {
    const find = (n) => {
      for (const c of n.children) {
        if (c.getAttribute("id") === id) return c;
        const f = find(c);
        if (f) return f;
      }
      return null;
    };
    return find(this);
  }
}

/**
 * `customElements`: definitions, `whenDefined`, `upgrade` (an element of a
 * defined tag takes the class's prototype; classes extend `Element`).
 */
export class CustomElementRegistry {
  constructor() { this.defs = new Map(); this.waiting = new Map(); }
  get(tag) { return this.defs.get(tag); }
  define(tag, cls) {
    this.defs.set(tag, cls);
    (this.waiting.get(tag) || []).forEach((resolve) => resolve(cls));
    this.waiting.delete(tag);
  }
  whenDefined(tag) {
    if (this.defs.has(tag)) return Promise.resolve(this.defs.get(tag));
    return new Promise((resolve) => {
      if (!this.waiting.has(tag)) this.waiting.set(tag, []);
      this.waiting.get(tag).push(resolve);
    });
  }
  upgrade(el) {
    const cls = this.defs.get(el.localName);
    if (cls && !(el instanceof cls)) Object.setPrototypeOf(el, cls.prototype);
  }
}

/** A socket the tests drive: `deliver` plays the server, `sent` is what the page said. */
class FakeSocket {
  constructor(url) {
    this.url = url;
    this.readyState = 1;
    this.sent = [];
    this.listeners = {};
    FakeSocket.instances.push(this);
  }
  send(text) { this.sent.push(JSON.parse(text)); }
  addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); }
  open() { (this.listeners.open || []).forEach((fn) => fn({})); }
  deliver(msg) { this.onmessage({ data: JSON.stringify(msg) }); }
  /** A binary frame of raw DEFLATE, as `prism_web::Frame::Binary` (an ArrayBuffer, `binaryType = "arraybuffer"`). */
  deliverCompressed(msg) { this.deliverBinary(zlib.deflateRawSync(Buffer.from(JSON.stringify(msg)))); }
  deliverBinary(bytes) {
    const buf = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
    this.onmessage({ data: buf });
  }
  close() {
    if (this.readyState >= 2) return;
    this.readyState = 3;
    this.closed = true;
    if (this.onclose) this.onclose({});
  }
}
FakeSocket.instances = [];

const CLIENT = new URL("../../src/client.js", import.meta.url);

/**
 * Runs client.js on a page whose `<main id="prism-root">` holds `ssr`, and
 * returns the page, its window and the socket it opened.
 */
export function boot({ ssr = "", path = "/", base = "", glyphs = { plus: "+", gear: "⚙", house: "⌂" }, fetch = null, search = "", nonce = "", importModule = null, decompress = true, pageId = "" } = {}) {
  const document = new Document();
  const customElements = new CustomElementRegistry();
  document.registry = customElements;
  const imports = [];
  document.body.innerHTML =
    `<main id="prism-root">${ssr}</main><div id="prism-island"></div><div id="prism-toasts"></div>` +
    `<div id="prism-connection" role="status" hidden>Reconnecting…</div>`;
  const script = document.createElement("script");
  script.dataset.base = base;
  if (pageId) script.dataset.page = pageId;
  if (nonce) script.nonce = nonce;
  document.currentScript = script;
  document.title = "Prism";
  const timers = [];
  const frames = [];
  const history = {
    entries: [{ state: null, url: path }],
    index: 0,
    get state() { return this.entries[this.index].state; },
    pushState(state, _, url) { this.entries.splice(this.index + 1, Infinity, { state, url }); this.index++; go(url); },
    replaceState(state, _, url) { this.entries[this.index] = { state, url }; go(url); },
    go(n) { this.index += n; go(this.entries[this.index].url); },
  };
  // `location.pathname` and `location.search` of a URL like `/a?x=1`.
  const split = (url) => {
    const q = url.indexOf("?");
    return q < 0 ? { pathname: url, search: "" } : { pathname: url.slice(0, q), search: url.slice(q) };
  };
  const go = (url) => Object.assign(win.location, split(url));
  const winListeners = {};
  const sockets = [];
  class Socket extends FakeSocket {
    constructor(url) { super(url); sockets.push(this); }
  }
  const win = {
    document,
    location: {
      ...split(path), ...(search ? { search } : {}), protocol: "http:", host: "localhost:3000", origin: "http://localhost:3000",
      get href() { return `http://localhost:3000${this.pathname}${this.search}`; },
      reload() { win.reloaded = true; },
      assign(url) { win.assigned = url; },
    },
    open: (url, target, features) => { win.opened = { url, target, features }; return null; },
    fetch: fetch && ((url, init) => { win.fetches.push({ url, init }); return fetch(url, init); }),
    fetches: [],
    sessionStorage: { items: {}, getItem(k) { return this.items[k] ?? null; }, setItem(k, v) { this.items[k] = String(v); } },
    CustomEvent: class { constructor(type, init = {}) { this.type = type; this.detail = init.detail; } },
    dispatchEvent: (e) => { (winListeners[e.type] || []).forEach((fn) => fn(e)); return true; },
    history,
    innerWidth: 1280,
    innerHeight: 800,
    devicePixelRatio: 2,
    screen: { width: 1440, height: 900 },
    navigator: { userAgent: "node-test", maxTouchPoints: 0 },
    WebSocket: Socket,
    ResizeObserver: class { constructor(cb) { this.cb = cb; win.resizeObservers.push(this); this.targets = []; } observe(t) { this.targets.push(t); } },
    resizeObservers: [],
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    getComputedStyle: (el) => ({ position: el.style.position || "static" }),
    requestAnimationFrame: (fn) => frames.push(fn),
    setTimeout: (fn, ms) => timers.push({ fn, ms }) ,
    clearTimeout: () => {},
    setInterval: () => 0,
    performance: { now: () => 0 },
    addEventListener: (t, fn) => (winListeners[t] ||= []).push(fn),
    removeEventListener: () => {},
    console,
    customElements,
    Promise,
    JSON,
    Math,
    Date,
    Number,
    String,
    Map,
    Set,
    Array,
    Object,
    URL,
    encodeURIComponent,
    atob,
    // Run-time fonts (`cx.load_font`): faces are recorded in `document.fonts.added`.
    Option: function (text, value) {
      const o = document.createElement("option");
      o.textContent = text;
      if (value !== undefined) o.setAttribute("value", value);
      return o;
    },
    FontFace: class {
      constructor(family, source, descriptors) { Object.assign(this, { family, source, descriptors }); }
      load() { return Promise.resolve(this); }
    },
  };
  document.fonts = { added: [], add(face) { this.added.push(face); } };
  // Node's own streams: compressed frames (`decompress: false` plays a
  // browser without `DecompressionStream`).
  if (decompress) Object.assign(win, { DecompressionStream, Response });
  win.window = win;
  win.self = win;
  const context = vm.createContext(win);
  const source = fs.readFileSync(CLIENT, "utf8").replace("__GLYPHS__", JSON.stringify(glyphs));
  // `import()` of island modules (a vm script cannot import without a Node
  // flag): recorded, then `importModule(url, customElements)`.
  win.__testImport = async (url) => {
    imports.push(url);
    if (!importModule) throw new Error(`no module ${url}`);
    await importModule(url, customElements);
    return {};
  };
  vm.runInContext(source.replace(/\bimport\(/g, "__testImport("), context, { filename: "client.js" });
  const ws = sockets[0];
  return {
    window: win,
    document,
    ws,
    sockets,
    timers,
    frames,
    history,
    mount: document.getElementById("prism-root"),
    /** Modules the client imported (`import()`), in order. */
    imports,
    customElements,
    /** Window events (popstate…). */
    fire: (type, event = {}) => (winListeners[type] || []).forEach((fn) => fn({ type, ...event })),
    /** The element for Prism node `id`, found in the page. */
    node: (id) => document.querySelector(`[data-pid=${id}]`),
  };
}

/**
 * An element's own CSS: the rules of the client's shared sheet that its
 * classes select (`.psN`), then its inline style.
 */
export function cssOf(el) {
  const doc = el.ownerDocument;
  const classes = el.className.split(/\s+/).filter(Boolean);
  let css = "";
  const walk = (n) => n.children.forEach((c) => {
    if (c.localName === "style" && c._sheet) {
      for (const r of c._sheet.cssRules) {
        const m = /\.([\w-]+)$/.exec(r.selectorText);
        if (m && classes.includes(m[1])) css += r.style.cssText;
      }
    }
    walk(c);
  });
  walk(doc);
  return css + el.style.cssText;
}

/** `cssOf(el)` readable as properties (`styleOf(el).gap`). */
export function styleOf(el) {
  const st = makeStyle();
  st.cssText = cssOf(el);
  return st;
}
