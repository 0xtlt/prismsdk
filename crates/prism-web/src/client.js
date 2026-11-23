// Prism web client: applies server patches to the DOM, sends events back,
// mirrors navigation to the URL bar (History API), and implements the ARIA
// patterns (roles, keyboard, focus) for every node kind.
(() => {
  // Where the app is mounted (`/app` when nested in another server's routes).
  const BASE = ((document.currentScript && document.currentScript.dataset.base) || "").replace(/\/$/, "");
  // The id of this page's request: the first socket sends it back, so the
  // live session takes that request over (`cx.page_request()`); reconnects don't.
  let pageId = (document.currentScript && document.currentScript.dataset.page) || "";
  // App paths keep their query string (`/analytics?period=7d`).
  const urlFor = (path) => {
    const q = path.indexOf("?");
    const [p, query] = q < 0 ? [path, ""] : [path.slice(0, q), path.slice(q)];
    return (p === "/" && BASE ? BASE : BASE + p) + query;
  };
  const appPath = () => {
    const p = location.pathname;
    return (BASE && p.startsWith(BASE) ? p.slice(BASE.length) || "/" : p) + location.search;
  };
  const mount = document.getElementById("prism-root");
  // The host's CSP nonce (`prism_web::CspNonce`): `<style>` elements the
  // client adds carry it, so a strict `style-src` lets them apply.
  const styleNonce = (document.currentScript && document.currentScript.nonce)
    || (document.getElementById("prism-ssr") || {}).nonce || "";
  const newStyle = () => {
    const el = document.createElement("style");
    if (styleNonce) el.nonce = styleNonce;
    return el;
  };
  const baseTitle = document.title.split(" · ").pop();
  /** Unsaved changes (`cx.set_unsaved_changes`, #158): the browser asks
   *  before the page is closed, reloaded or left. */
  let unsaved = false;
  addEventListener("beforeunload", (e) => {
    if (!unsaved) return;
    e.preventDefault();
    e.returnValue = "";
  });
  /** The app's title (`cx.set_title`, #154) wins over the top screen's. */
  let appTitle = "", screenTitle = "";
  function showTitle() {
    const t = appTitle || screenTitle;
    document.title = t && t !== baseTitle ? `${t} · ${baseTitle}` : baseTitle;
  }
  // Pictures lose their loading background once loaded (`prism.css`), so a
  // transparent image shows its parent. `load` does not bubble: capture it.
  const markLoaded = (img) => { if (img.naturalWidth) img.dataset.loaded = "1"; };
  document.addEventListener("load", (e) => {
    const img = e.target;
    if (img.tagName === "IMG" && img.classList.contains("prism-picture")) markLoaded(img);
  }, true);
  // The first render's images may have loaded before this script ran.
  for (const img of document.querySelectorAll("img.prism-picture")) if (img.complete) markLoaded(img);
  const nodes = new Map();

  // Node styles: one rule per distinct style in a shared sheet, applied by a
  // class named after a short hash of the style (no `style` attribute: lean
  // markup, as in the server's first render, which uses the same names).
  // Rules no node uses any more are recycled for new styles.
  const styleSlots = new Map(); // css → { cls, rule }
  const slotClasses = new Set();
  const freeSlots = [];
  let styleSheet = null;
  let collectAt = 512;
  /** `p` + FNV-1a of the code points, base 36 (`prism_web::html::style_class`). */
  const styleHash = (css) => {
    let h = 0x811c9dc5;
    for (const ch of css) h = Math.imul(h ^ ch.codePointAt(0), 0x01000193);
    return "p" + (h >>> 0).toString(36);
  };
  function styleSlot(css) {
    let slot = styleSlots.get(css);
    if (slot) return slot;
    if (!styleSheet) {
      const el = newStyle();
      el.id = "prism-styles";
      document.head.appendChild(el);
      styleSheet = el.sheet;
    }
    if (styleSlots.size >= collectAt) collectStyles();
    // Two styles with one hash (rare): the second gets a suffix.
    let cls = styleHash(css);
    for (let n = 2; slotClasses.has(cls); n++) cls = `${styleHash(css)}-${n}`;
    // An id outranks Prism's own sheet, like an inline style did (`:is`
    // counts its most specific argument; `body` matches every node).
    const selector = `:is(#prism-root, body) .${cls}`;
    slot = freeSlots.pop();
    if (slot) {
      slot.rule.selectorText = selector;
    } else {
      const i = styleSheet.insertRule(`${selector}{}`, styleSheet.cssRules.length);
      slot = { rule: styleSheet.cssRules[i] };
    }
    slot.cls = cls;
    // Set through the CSSOM: a strict CSP allows it, and nothing in `css`
    // can end the rule.
    slot.rule.style.cssText = css;
    styleSlots.set(css, slot);
    slotClasses.add(cls);
    return slot;
  }
  /** Frees the rules of styles no element (attached or not) uses. */
  function collectStyles() {
    const used = new Set();
    for (const el of nodes.values()) {
      if (el._ps) used.add(el._ps);
      if (el._runPs) el._runPs.forEach((c) => used.add(c));
    }
    for (const [css, slot] of styleSlots) {
      if (used.has(slot.cls)) continue;
      styleSlots.delete(css);
      slotClasses.delete(slot.cls);
      slot.rule.style.cssText = "";
      freeSlots.push(slot);
    }
    collectAt = Math.max(512, styleSlots.size * 2);
  }
  /** Gives `el` the style `css` (a class of the shared sheet). */
  function setCss(el, css) {
    const cls = css ? styleSlot(css).cls : null;
    if (el._ps === cls) return;
    if (el._ps) el.classList.remove(el._ps);
    if (cls) el.classList.add(cls);
    else if (!el.className) el.removeAttribute("class");
    el._ps = cls;
  }
  const hiddenBy = (css) => /(^|;)display:none\b/.test(css || "");
  const tags = {
    view: "div", text: "div", button: "button", text_input: "input",
    stack: "div", screen: "section", toggle: "input", scroll: "div", tabs: "div", tab: "div",
    image: "span", progress: "progress", timer: "span", modal: "dialog", picture: "img",
    shape: "div", slider: "input", picker: "select", date_picker: "input", text_area: "textarea",
    spinner: "span", overlay: "div", checkbox: "label", radio_group: "div", scene3d: "div",
    rich_input: "div", video: "video",
    web_view: "iframe",
    table: "table", table_row: "tr", column_header: "th", table_cell: "td",
  };
  // SF Symbol names → glyphs (table generated from Rust: glyphs.rs).
  const glyphs = __GLYPHS__;
  let stacks = [];
  let ws;
  let userNavigated = false;

  const send = (msg) => ws && ws.readyState === 1 && ws.send(JSON.stringify(msg));

  function activate(el, id, e) {
    if (el.dataset.clickable !== "1" || el.getAttribute("aria-disabled") === "true") return;
    // The click that ends a drag is not a click.
    if (dragJustEnded) return;
    // Pointer clicks say where (keyboard activations have no position: detail 0).
    // With the keys held and the click's number in a quick series (#159),
    // only when they say something.
    if (e && e.detail > 0 && typeof e.clientX === "number") {
      const mods = (e.shiftKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.altKey ? 4 : 0) | (e.metaKey ? 8 : 0);
      send({ t: "click", id, ...local(el, e), ...(mods ? { mods } : {}), ...(e.detail > 1 ? { n: e.detail } : {}) });
    } else send({ t: "click", id });
  }

  // --- Pointer and drag (#17) -------------------------------------------------
  const round = (v) => Math.round(v * 10) / 10;
  /** The pointer in an element's own coordinates (from its top-left corner). */
  const local = (el, e) => {
    const r = el.getBoundingClientRect();
    return { x: round(e.clientX - r.left), y: round(e.clientY - r.top) };
  };
  const toggleClass = (el, cls, on) => {
    const list = el.className.split(/\s+/).filter((c) => c && c !== cls);
    if (on) list.push(cls);
    el.className = list.join(" ");
  };
  /** Set from a drag's end until the click it produces has been ignored. */
  let dragJustEnded = false;
  const modsOf = (e) => (e.shiftKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.altKey ? 4 : 0) | (e.metaKey ? 8 : 0);
  // The click a drag's release makes (#223): no handler, no link, hears it.
  document.addEventListener("click", (e) => {
    if (!dragJustEnded) return;
    dragJustEnded = false;
    e.preventDefault();
    e.stopImmediatePropagation();
  }, true);
  document.addEventListener("pointerdown", () => { dragJustEnded = false; }, true);
  /** One message per animation frame: the latest one wins. */
  function throttled() {
    let pending = null, queued = false;
    const flush = () => { queued = false; if (pending) { const m = pending; pending = null; send(m); } };
    return {
      queue(m) { pending = m; if (!queued) { queued = true; requestAnimationFrame(flush); } },
      flush,
      drop() { pending = null; },
    };
  }
  /** Hover (1), move (2) and drag (4) reports of a node. */
  function setPointer(el, id, mask) {
    el._pmask = mask;
    setGestures(el, id);
    // Whatever button drags it (#136, #142): no text selection, no browser
    // panning, so a left drag over a middle-button canvas selects nothing.
    toggleClass(el, "prism-draggable", (mask & (4 | 32 | 64)) !== 0);
    // A middle-button drag (#136): no autoscroll, no paste, no new tab.
    if ((mask & 32) && !el._middleBound) {
      el._middleBound = true;
      el.addEventListener("mousedown", (e) => { if (e.button === 1 && (el._pmask & 32)) e.preventDefault(); });
      el.addEventListener("auxclick", (e) => { if (e.button === 1 && (el._pmask & 32)) e.preventDefault(); });
    }
    // Middle, back and forward clicks (#215): reported, without the middle
    // button's autoscroll or the browser's back and forward.
    if ((mask & 128) && !el._auxBound) {
      el._auxBound = true;
      const aux = (e) => (el._pmask & 128) && e.button !== 0 && e.button !== 2;
      el.addEventListener("mousedown", (e) => { if (aux(e)) e.preventDefault(); });
      el.addEventListener("mouseup", (e) => { if (aux(e) && e.button > 2) e.preventDefault(); });
      el.addEventListener("auxclick", (e) => {
        if (!aux(e)) return;
        e.preventDefault();
        e.stopPropagation();
        const mods = (e.shiftKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.altKey ? 4 : 0) | (e.metaKey ? 8 : 0);
        const button = e.button === 1 ? "middle" : `other:${e.button}`;
        send({ t: "auxclick", id, button, mods, n: Math.max(1, e.detail || 1), ...local(el, e) });
      });
    }
    // After a secondary-button drag, no context menu (#136).
    if ((mask & 64) && !el._secondaryBound) {
      el._secondaryBound = true;
      el.addEventListener("contextmenu", (e) => { if (el._rightDragged) { el._rightDragged = false; e.preventDefault(); e.stopPropagation(); } }, true);
    }
    // A draggable link: the app's drag, not the browser's link drag.
    if (el.localName === "a") (mask & 4) ? el.setAttribute("draggable", "false") : el.removeAttribute("draggable");
    if (el._pointerBound) return;
    el._pointerBound = true;
    const moves = throttled(), drags = throttled();
    let drag = null;
    el.addEventListener("pointerenter", (e) => { if (el._pmask & 1) send({ t: "pointer", id, phase: "enter", ...local(el, e) }); });
    el.addEventListener("pointerleave", (e) => {
      if (!(el._pmask & 3)) return;
      moves.flush();
      send({ t: "pointer", id, phase: "leave", ...local(el, e) });
    });
    el.addEventListener("pointerdown", (e) => {
      // The innermost element the button may drag takes the drag: the
      // primary (4), middle (32) or secondary (64) button (#136).
      const bit = [4, 32, 64][e.button] || 0;
      if (!(el._pmask & bit) || e.prismDrag) return;
      e.prismDrag = true;
      if (e.button === 1) e.preventDefault();
      const at = local(el, e);
      // Only what differs from a plain primary drag goes up; the keys held
      // go with each report (#223).
      const extra = e.button ? { button: ["primary", "middle", "secondary"][e.button] } : {};
      drag = { pointer: e.pointerId, cx: e.clientX, cy: e.clientY, x: at.x, y: at.y, started: false, button: e.button, extra, mods: modsOf(e), dx: 0, dy: 0 };
      // A finger or a pen on a node that drags after a long press (#243):
      // held still that long, it lifts; a swipe before scrolls the page.
      if (el._dragDelay && e.pointerType !== "mouse") {
        const d = drag;
        d.timer = setTimeout(() => {
          if (drag !== d) return;
          d.started = true;
          d.lifted = true;
          el._lifted = true;
          try { el.setPointerCapture(d.pointer); } catch {}
          try { navigator.vibrate?.(10); } catch {}
          send({ t: "drag", id, x: d.x, y: d.y, dx: d.dx, dy: d.dy, ...d.extra, ...(d.mods ? { mods: d.mods } : {}), phase: "start" });
        }, el._dragDelay);
      }
      // Followed on the window until it is released (#194): a fast flick off
      // a small handle still starts the drag. The pointer is captured only
      // once it is a drag, so a click on a child stays the child's.
      addEventListener("pointermove", dragMove, true);
      addEventListener("pointerup", dragUp, true);
      addEventListener("pointercancel", dragCancel, true);
      addEventListener("keydown", dragKey, true);
      addEventListener("keyup", dragKey, true);
    });
    const dragMove = (e) => {
      if (!drag || e.pointerId !== drag.pointer) return;
      const dx = round(e.clientX - drag.cx), dy = round(e.clientY - drag.cy);
      drag.mods = modsOf(e);
      const msg = { t: "drag", id, x: drag.x, y: drag.y, dx, dy, ...drag.extra, ...(drag.mods ? { mods: drag.mods } : {}) };
      if (drag.timer && !drag.started) {
        // Moved before the long press (#243): a scroll, not a drag.
        if (Math.hypot(dx, dy) < 8) { drag.dx = dx; drag.dy = dy; return; }
        clearTimeout(drag.timer);
        drag = null;
        unfollow();
        return;
      }
      if (!drag.started) {
        // Past the slop: a drag, not a click.
        if (Math.hypot(dx, dy) < 4) return;
        drag.started = true;
        try { el.setPointerCapture(e.pointerId); } catch {}
        // A draggable's payload (#251): its preview, the target under it.
        if (el._dnd) payloadStart(el, drag, (m) => drags.queue({ ...m, t: "drag", id, x: drag.x, y: drag.y, dx: drag.dx, dy: drag.dy, ...drag.extra, ...(drag.mods ? { mods: drag.mods } : {}), phase: "move" }));
        send({ ...msg, ...payloadAt(el, e.clientX, e.clientY), phase: "start" });
      } else drags.queue({ ...msg, ...payloadAt(el, e.clientX, e.clientY), phase: "move" });
      drag.dx = dx;
      drag.dy = dy;
    };
    // ⇧, ⌥, ⌘ or ⌃ pressed or released alone during a drag (#223): reported
    // where the drag is, with the keys held now.
    const dragKey = (e) => {
      if (!drag || !drag.started || !["Shift", "Alt", "Meta", "Control"].includes(e.key)) return;
      const mods = modsOf(e);
      if (mods === drag.mods) return;
      drag.mods = mods;
      drags.queue({ t: "drag", id, x: drag.x, y: drag.y, dx: drag.dx, dy: drag.dy, ...drag.extra, ...(mods ? { mods } : {}), ...payloadLast(el), phase: "move" });
    };
    el.addEventListener("pointermove", (e) => {
      if (el._pmask & 2) moves.queue({ t: "pointer", id, phase: "move", ...local(el, e) });
    });
    const unfollow = () => {
      removeEventListener("pointermove", dragMove, true);
      removeEventListener("pointerup", dragUp, true);
      removeEventListener("pointercancel", dragCancel, true);
      removeEventListener("keydown", dragKey, true);
      removeEventListener("keyup", dragKey, true);
    };
    const finish = (e, phase) => {
      if (!drag || e.pointerId !== drag.pointer) return;
      const d = drag;
      drag = null;
      unfollow();
      clearTimeout(d.timer);
      el._lifted = false;
      if (!d.started) return;
      drags.drop();
      if (d.button === 2) el._rightDragged = true;
      const mods = modsOf(e);
      // Released over a target: dropped there (#251); cancelled: nowhere.
      const over = phase === "end" ? payloadAt(el, e.clientX, e.clientY) : {};
      payloadEnd(el);
      send({ t: "drag", id, phase, x: d.x, y: d.y, dx: round(e.clientX - d.cx), dy: round(e.clientY - d.cy), ...d.extra, ...(mods ? { mods } : {}), ...over });
      // The click this release makes is not a click, on the element or
      // around it (#223): swallowed before any handler, until the next press.
      dragJustEnded = true;
      setTimeout(() => { dragJustEnded = false; }, 500);
    };
    const dragUp = (e) => finish(e, "end");
    const dragCancel = (e) => finish(e, "cancel");
  }
  // --- Drag and drop between elements (#251) ------------------------------------
  // A draggable's drag is reported like any drag, with the drop target under
  // the pointer (`over`, and `ox`, `oy` in it, scrolling included). Pointer
  // events, not HTML5 drag and drop: the same drag as `on_drag` (the slop,
  // the buttons, live keys, no click after it) on touch screens too, a
  // preview the page draws itself, and `elementFromPoint` finds the target
  // with the browser's own hit-testing (scrolling, clipping, z-index).
  let payload = null;
  const PAYLOAD_EDGE = 44, PAYLOAD_STEP = 14;
  function setDnd(el, p) {
    el._dnd = p.kind ? { kind: p.kind, preview: !!p.preview } : null;
    el._accepts = p.accepts && p.accepts.length ? p.accepts : null;
    if (!el._accepts) el.removeAttribute("data-prism-drop");
  }
  /** Starts carrying `el`'s payload: a see-through copy follows the pointer. */
  function payloadStart(el, drag, resend) {
    payloadEnd();
    const r = el.getBoundingClientRect();
    let ghost = null;
    if (el._dnd.preview) {
      // The copy keeps `data-pid` for its styles; it is inert and hidden.
      ghost = el.cloneNode(true);
      ghost.removeAttribute("id");
      ghost.querySelectorAll("[id]").forEach((n) => n.removeAttribute("id"));
      ghost.setAttribute("aria-hidden", "true");
      ghost.inert = true;
      ghost.classList.add("prism-drag-preview");
      Object.assign(ghost.style, {
        position: "fixed", margin: "0", left: `${r.left}px`, top: `${r.top}px`, width: `${r.width}px`, height: `${r.height}px`,
        pointerEvents: "none", opacity: "0.85", zIndex: "2147483647", transition: "none", boxSizing: "border-box",
      });
      document.body.appendChild(ghost);
    }
    payload = { el, kind: el._dnd.kind, ghost, gx: drag.cx - r.left, gy: drag.cy - r.top, over: null, last: {}, at: [drag.cx, drag.cy], scroll: null, raf: 0, resend };
  }
  /** The target under (`cx`, `cy`) that takes the payload, and the pointer in it. */
  function payloadAt(el, cx, cy) {
    const p = payload;
    if (!p || p.el !== el) return {};
    p.at = [cx, cy];
    if (p.ghost) { p.ghost.style.left = `${cx - p.gx}px`; p.ghost.style.top = `${cy - p.gy}px`; }
    const hit = document.elementFromPoint(cx, cy);
    // Not the dragged element or what is inside it.
    let t = hit && el.contains(hit) ? el.parentElement : hit;
    while (t && !(t._accepts && t._accepts.includes(p.kind) && t.dataset.pid)) t = t.parentElement;
    if (t !== p.over) {
      p.over?.removeAttribute("data-prism-drop");
      t?.setAttribute("data-prism-drop", "");
      p.over = t;
    }
    payloadScroller(hit, cx, cy);
    if (!t) return (p.last = {});
    const r = t.getBoundingClientRect();
    p.last = { over: Number(t.dataset.pid), ox: round(cx - r.left - t.clientLeft + t.scrollLeft), oy: round(cy - r.top - t.clientTop + t.scrollTop) };
    return p.last;
  }
  const payloadLast = (el) => (payload && payload.el === el ? payload.last : {});
  /** The scroll view whose edge the pointer is near scrolls, on every frame. */
  function payloadScroller(hit, cx, cy) {
    const p = payload;
    const speed = (from, length) => {
      const edge = Math.min(PAYLOAD_EDGE, length / 3);
      if (from < edge) return -PAYLOAD_STEP * (1 - Math.max(0, from) / edge);
      if (from > length - edge) return PAYLOAD_STEP * (1 - Math.max(0, length - from) / edge);
      return 0;
    };
    p.scroll = null;
    for (let n = hit; n && n !== document.body; n = n.parentElement) {
      if (p.el.contains(n)) continue;
      const cs = getComputedStyle(n);
      const r = n.getBoundingClientRect();
      const sx = /(auto|scroll)/.test(cs.overflowX) && n.scrollWidth > n.clientWidth ? speed(cx - r.left, r.width) : 0;
      const sy = /(auto|scroll)/.test(cs.overflowY) && n.scrollHeight > n.clientHeight ? speed(cy - r.top, r.height) : 0;
      if (sx || sy) { p.scroll = { n, sx, sy }; break; }
    }
    if (p.scroll && !p.raf) p.raf = requestAnimationFrame(payloadTick);
  }
  function payloadTick() {
    const p = payload;
    if (!p) return;
    p.raf = 0;
    if (!p.scroll) return;
    const { n, sx, sy } = p.scroll;
    const before = [n.scrollLeft, n.scrollTop];
    n.scrollBy(sx, sy);
    if (n.scrollLeft === before[0] && n.scrollTop === before[1]) { p.scroll = null; return; }
    // The content moved under the pointer: where it is now.
    p.resend(payloadAt(p.el, p.at[0], p.at[1]));
    if (p.scroll && !p.raf) p.raf = requestAnimationFrame(payloadTick);
  }
  function payloadEnd(el) {
    const p = payload;
    if (!p || (el && p.el !== el)) return;
    payload = null;
    if (p.raf) cancelAnimationFrame(p.raf);
    p.ghost?.remove();
    p.over?.removeAttribute("data-prism-drop");
  }

  /**
   * A touch drag after a long press (#243): until the press is held still
   * for `ms`, the browser may pan what is around the node (`touch-action`);
   * once lifted, its touch moves stop scrolling and the long touch opens no
   * menu or callout. Mice drag on movement.
   */
  function setDragDelay(el, ms) {
    el._dragDelay = ms > 0 ? ms : 0;
    if (el._dragDelay) el.setAttribute("data-prism-hold", ""); else el.removeAttribute("data-prism-hold");
    if (!el._dragDelay || el._holdBound) return;
    el._holdBound = true;
    el.addEventListener("touchmove", (e) => { if (el._lifted && e.cancelable) e.preventDefault(); }, { passive: false });
    el.addEventListener("contextmenu", (e) => { if (el._dragDelay && (el._lifted || e.pointerType === "touch")) e.preventDefault(); });
  }
  /**
   * Wheel (8) and pinch (16) reports (#133): the element takes them, so
   * nothing behind it scrolls and the page does not zoom. Browsers report a
   * trackpad pinch as Ctrl + wheel (Safari: gesture events). Deltas add up
   * to one report per animation frame; the innermost listener wins.
   */
  function setGestures(el, id) {
    if (!(el._pmask & 24) || el._gestureBound) return;
    el._gestureBound = true;
    let wheel = null, pinch = null, gesture = 1;
    const flushWheel = () => { if (wheel) { send(wheel); wheel = null; } };
    const flushPinch = () => { if (pinch) { send(pinch); pinch = null; } };
    const magnify = (scale, at, phase) => {
      if (pinch && pinch.phase === phase) { pinch.scale *= scale; Object.assign(pinch, at); return; }
      flushPinch();
      pinch = { t: "magnify", id, scale, ...at, phase };
      requestAnimationFrame(flushPinch);
    };
    el.addEventListener("wheel", (e) => {
      const m = el._pmask;
      const zoom = e.ctrlKey && (m & 16);
      if (e.prismWheel || !(zoom || (m & 8))) return;
      e.prismWheel = true;
      e.preventDefault();
      const at = local(el, e);
      const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? (el.clientHeight || 800) : 1;
      if (zoom) return magnify(Math.exp(-e.deltaY * unit / 100), at, "none");
      const mods = (e.shiftKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.altKey ? 4 : 0) | (e.metaKey ? 8 : 0);
      if (wheel && wheel.mods === mods) {
        wheel.dx += e.deltaX * unit;
        wheel.dy += e.deltaY * unit;
        Object.assign(wheel, at);
        return;
      }
      flushWheel();
      wheel = { t: "wheel", id, dx: e.deltaX * unit, dy: e.deltaY * unit, ...at, mods, phase: "none", precise: e.deltaMode === 0 };
      requestAnimationFrame(flushWheel);
    }, { passive: false });
    const pinchEvent = (phase) => (e) => {
      if (!(el._pmask & 16) || e.prismWheel) return;
      e.prismWheel = true;
      e.preventDefault();
      // `e.scale` is since the start: report the change since the last event.
      const scale = phase === "start" ? 1 : e.scale / gesture;
      gesture = phase === "start" ? 1 : e.scale;
      magnify(scale, local(el, e), phase);
      if (phase !== "move") flushPinch();
    };
    el.addEventListener("gesturestart", pinchEvent("start"));
    el.addEventListener("gesturechange", pinchEvent("move"));
    el.addEventListener("gestureend", pinchEvent("end"));
  }

  // --- Keys (#17): focused elements and the whole window ----------------------
  let globalKeys = 0;
  const NATIVE_FOCUS = ["INPUT", "BUTTON", "SELECT", "TEXTAREA", "A"];
  function setKeys(el, local, global) {
    if (!!el._kGlobal !== global) globalKeys += global ? 1 : -1;
    el._kGlobal = global;
    el._kLocal = local;
    if (local) el.dataset.keys = "1"; else delete el.dataset.keys;
    // A view with key handlers can take the focus.
    if (local && !NATIVE_FOCUS.includes(el.tagName) && !el.hasAttribute("tabindex")) {
      el.setAttribute("tabindex", "0");
      el._kTab = true;
    } else if (!local && el._kTab) {
      el.removeAttribute("tabindex");
      el._kTab = false;
    }
  }
  const editable = (t) => !!t && (t.isContentEditable || t.tagName === "TEXTAREA"
    || (t.tagName === "INPUT" && !["checkbox", "radio", "range", "button", "submit", "color", "file"].includes(t.type)));
  const SCROLL_KEYS = ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", " ", "PageUp", "PageDown", "Home", "End"];
  // Keys an element takes from the focused field in it (`consume_keys`,
  // #181): reported first, in the capture phase, then kept from the field
  // (↑ moving the caret, ↵ submitting) and the page.
  // `consume_key_at` (#211): "ArrowUp@first-line" takes the key only while
  // the focused field's caret is there, measured now, before the key acts.
  const EDGES = { start: "s", end: "e", "first-line": "f", "last-line": "l" };
  function consumes(t, key) {
    let edges;
    for (let n = t; n; n = n.parentElement) {
      if (!n._consume) continue;
      if (n._consume.has(key)) return true;
      for (const k of n._consume) {
        const at = k.indexOf("@");
        if (at <= 0 || k.slice(0, at) !== key) continue;
        if (edges === undefined) edges = editable(t) ? caretEdges(t) : null;
        // No field: no caret to move, the key is the element's.
        if (edges == null || edges.includes(EDGES[k.slice(at + 1)])) return true;
      }
    }
    return false;
  }
  document.addEventListener("keydown", (e) => {
    if (e.isComposing || !e.key || e.prismShortcut) return;
    const t = e.target && e.target.closest ? e.target : null;
    if (!t || !consumes(t, e.key)) return;
    reportKey(e);
    e.preventDefault();
    e.stopImmediatePropagation();
  }, true);
  document.addEventListener("keydown", reportKey);
  // Focus changes (#224): a Prism element (a field, a focusable view) took
  // or lost the focus, the window's included.
  for (const [type, on] of [["focusin", true], ["focusout", false]]) {
    document.addEventListener(type, (e) => {
      const t = e.target;
      if (t && t.dataset && t.dataset.pid) send({ t: "focus", id: Number(t.dataset.pid), on });
    });
  }
  function reportKey(e) {
    if (e.isComposing || !e.key || e.prismShortcut) return;
    const t = e.target && e.target.closest ? e.target : null;
    const owner = t && t.closest("[data-keys]");
    const typing = editable(t);
    // In a text field, global listeners only hear keys that do not type.
    const typed = e.key.length === 1 && !e.ctrlKey && !e.metaKey;
    if (!owner && !(globalKeys > 0 && !(typing && typed))) return;
    const mods = (e.shiftKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.altKey ? 4 : 0) | (e.metaKey ? 8 : 0);
    const msg = { t: "key", id: owner ? Number(owner.dataset.pid) : 0, key: e.key, mods, repeat: !!e.repeat };
    // In a text field: its selection before the key acts (#86).
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t._rich) && typing) {
      if (t._selFlush) t._selFlush();
      const sel = fieldSelection(t);
      if (sel) msg.sel = sel;
      const edges = caretEdges(t);
      if (edges != null) msg.edges = edges;
    }
    send(msg);
    keysDown.set(e.key, msg.id);
    // The app handles arrows and space on its element: the page does not scroll.
    const control = typing || (t && ["SELECT", "INPUT"].includes(t.tagName));
    if (owner && !control && SCROLL_KEYS.includes(e.key)) e.preventDefault();
  }
  // The app menus' shortcuts (#161), `cmd` being ⌘ on Apple platforms and
  // Ctrl elsewhere: taken before the browser (⌘Z in a field, ⌘S, ⌘K) and
  // sent as the item's choice; the key is then not reported as a key.
  let shortcuts = new Map();
  const applePlatform = /Mac|iPhone|iPad|iPod/.test(navigator.platform || navigator.userAgent || "");
  function shortcutOf(e) {
    const cmd = applePlatform ? e.metaKey : e.ctrlKey;
    const ctrl = applePlatform && e.ctrlKey;
    const key = e.key === " " ? "space" : e.key.toLowerCase();
    return [cmd && "cmd", ctrl && "ctrl", e.altKey && "alt", e.shiftKey && "shift", key].filter(Boolean);
  }
  addEventListener("keydown", (e) => {
    if (!shortcuts.size || e.isComposing || !e.key) return;
    const parts = shortcutOf(e);
    for (const [keys, id] of shortcuts) {
      const want = keys.split("+").map((k) => k.trim()).map((k) => ({ command: "cmd", meta: "cmd", control: "ctrl", option: "alt", opt: "alt" })[k] || k);
      if (want.length !== parts.length || !want.every((k) => parts.includes(k))) continue;
      e.preventDefault();
      e.prismShortcut = true;
      send({ t: "menu", id });
      return;
    }
  }, true);
  // Releases (#159) of the keys whose press was reported, to the same
  // listeners: hold Space to pan, release it to stop. The window losing the
  // focus releases them all (no key-up arrives then).
  const keysDown = new Map();
  document.addEventListener("keyup", (e) => {
    if (!e.key || !keysDown.has(e.key)) return;
    const id = keysDown.get(e.key);
    keysDown.delete(e.key);
    const mods = (e.shiftKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.altKey ? 4 : 0) | (e.metaKey ? 8 : 0);
    send({ t: "keyup", id, key: e.key, mods });
  });
  addEventListener("blur", () => {
    keysDown.forEach((id, key) => send({ t: "keyup", id, key, mods: 0 }));
    keysDown.clear();
  });

  // --- Drawn radios (#84): one tab stop per group, arrows move the choice -------
  let radiosDirty = false;
  /** The operable radios of a `role=radiogroup` (not those of a nested group). */
  function groupRadios(group) {
    return [...group.querySelectorAll("[role=radio]")].filter((r) => r.closest("[role=radiogroup]") === group
      && r.getAttribute("aria-disabled") !== "true" && r.getAttribute("aria-hidden") !== "true" && !r.closest("[inert]"));
  }
  /** Roving tab index (ARIA radio group pattern): only the checked radio, else the first, is a tab stop. */
  function roveRadios() {
    radiosDirty = false;
    for (const group of document.querySelectorAll("[role=radiogroup]")) {
      const items = groupRadios(group);
      const current = items.find((r) => r.getAttribute("aria-checked") === "true")
        || items.find((r) => r === document.activeElement) || items[0];
      items.forEach((r) => r.setAttribute("tabindex", r === current ? "0" : "-1"));
    }
  }
  document.addEventListener("keydown", (e) => {
    const t = e.target;
    if (e.altKey || e.ctrlKey || e.metaKey || !t?.getAttribute || t.getAttribute("role") !== "radio") return;
    // Down and the reading direction's arrow go to the next choice.
    const rtl = document.documentElement.dir === "rtl";
    const step = { ArrowDown: 1, ArrowUp: -1, ArrowRight: rtl ? -1 : 1, ArrowLeft: rtl ? 1 : -1 }[e.key];
    const group = step && t.closest("[role=radiogroup]");
    const items = group ? groupRadios(group) : [];
    const i = items.indexOf(t);
    if (i < 0 || items.length < 2) return;
    e.preventDefault();
    // Moving the focus checks the radio it lands on, as native radios do.
    const next = items[(i + step + items.length) % items.length];
    items.forEach((r) => r.setAttribute("tabindex", r === next ? "0" : "-1"));
    next.focus();
    activate(next, Number(next.dataset.pid));
  });

  // --- Focus and scrolling from the app (#17) ----------------------------------
  /** The node the app focused in this batch (`cx.focus`, `.autofocus()`):
   *  a dialog opened in the same batch gives it the focus back, instead of
   *  its first control (`showModal`'s choice, #148). */
  let focusAsked = null;
  function focusNode(el) {
    const target = el._input || (el.dataset.kind === "radio_group"
      ? el.querySelector("input:checked") || el.querySelector("input") : el);
    target?.focus?.();
    // A rich input takes the selection the app set while it was not focused (#115).
    const sel = el._pendingSel;
    if (el._rich && sel) { delete el._pendingSel; applyFieldSelection(el, sel[0], sel[1]); }
  }
  function reveal(el) {
    el.scrollIntoView?.({ block: "nearest", inline: "nearest", behavior: reducedMotion?.matches ? "auto" : "smooth" });
  }

  // --- Layers (#25): above everything, the toaster ----------------------------
  let layerHost = document.getElementById("prism-layers");
  if (!layerHost) {
    layerHost = document.createElement("div");
    layerHost.id = "prism-layers";
    layerHost.setAttribute("popover", "manual");
    document.body.appendChild(layerHost);
  }
  /** Shown last in the top layer, so it stays over dialogs opened since. */
  function raiseLayers() {
    if (typeof layerHost.showPopover !== "function") return;
    try {
      if (layerHost.matches(":popover-open")) layerHost.hidePopover();
      layerHost.showPopover();
    } catch {}
  }

  // --- Overlays (#19): sheets, dialogs, popovers without a route ---------------
  /** Open popovers, and dialogs placed beside their anchor (#79): placed
   *  again when the window resizes or scrolls. */
  const placedOverlays = new Set();
  /** Open overlays not shown yet: inserted already open, their box is not
   *  in the document until the patch that places their parent (#70). */
  const pendingOverlays = new Set();
  /** Shows the overlays a batch of patches inserted already open. */
  function flushOverlays() {
    for (const el of [...pendingOverlays]) {
      if (!el._open || !nodes.has(Number(el.dataset.pid))) { pendingOverlays.delete(el); continue; }
      if (el._box && el._box.isConnected) { pendingOverlays.delete(el); showOverlay(el); }
    }
  }
  /** The user closed it: the app hears `back`, with how (#156): `escape`,
   *  `outside` (a click on the backdrop or around a popover), `unknown`. */
  function overlayDismissed(el, reason) {
    if (!el._open || el._closing) return;
    el._open = false;
    placedOverlays.delete(el);
    hideOverlay(el);
    send({ t: "back", id: Number(el.dataset.pid), depth: 0, reason: reason || "unknown" });
  }
  // The browser's light dismiss does not say why a popover closed: an
  // Escape just before it is the reason, otherwise a click outside.
  let escapeAt = -Infinity;
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") escapeAt = performance.now(); }, true);
  const lightDismissReason = () => (performance.now() - escapeAt < 500 ? "escape" : "outside");
  // A press around an open popover that leaves the focus to its anchor
  // closes it, and goes on to what it landed on (the field's caret, #182).
  document.addEventListener("pointerdown", (e) => {
    const t = e.target;
    for (const el of [...placedOverlays]) {
      if (el._boxKind !== "popover" || !el._keepFocus || el._manual || el._fixed || !el._open) continue;
      if (el._box && t && el._box.contains(t)) continue;
      overlayDismissed(el, "outside");
    }
  }, true);
  // --- Keyboard bars (#242): above the on-screen keyboard ---------------------
  // A keyboard overlay shows while a text field it serves has the focus, on
  // touch screens only (desktop browsers have no on-screen keyboard): its
  // own field (the node just before it, `field`), else any field in its
  // parent, the nearest bar winning. It sits on the bottom edge of the
  // visual viewport, which the keyboard shrinks, and presses on it keep the
  // focus in the field.
  const keyboardBars = new Set();
  const touchScreen = () => !!(window.matchMedia && window.matchMedia("(pointer: coarse)").matches);
  function keyboardBarOf(field) {
    let child = field, own = true;
    while (child && child.parentElement) {
      const parent = child.parentElement;
      if (own) {
        own = false;
        const next = child.nextElementSibling;
        if (next && next._boxKind === "keyboard" && next._kbField) return next;
      }
      for (const c of parent.children) if (c._boxKind === "keyboard" && !c._kbField) return c;
      child = parent;
    }
    return null;
  }
  function focusedField() {
    const a = document.activeElement;
    if (!a || !a.matches || !a.matches("input, textarea, [contenteditable]") || a.getAttribute("contenteditable") === "false") return null;
    return a.closest("[data-pid]") || a;
  }
  function syncKeyboardBars() {
    const a = document.activeElement;
    let bar = null;
    if (touchScreen()) {
      // Focus inside a bar (one of its own fields) keeps it.
      bar = [...keyboardBars].find((el) => el._box && a && el._box.contains(a)) || null;
      const field = bar ? null : focusedField();
      if (field) bar = keyboardBarOf(field);
    }
    for (const el of [...keyboardBars]) {
      if (el.isConnected) showKeyboardBar(el, el === bar && el._open);
    }
  }
  function showKeyboardBar(el, on) {
    const box = el._box;
    if (!box) return;
    const popover = typeof box.showPopover === "function";
    try {
      if (popover) {
        const open = box.matches(":popover-open");
        if (on && !open) box.showPopover();
        if (!on && open) box.hidePopover();
      } else box.style.display = on ? "" : "none";
    } catch {}
    if (on) placeKeyboardBar(box);
  }
  function placeKeyboardBar(box) {
    const vv = window.visualViewport;
    const left = vv ? vv.offsetLeft : 0;
    const width = vv ? vv.width : innerWidth;
    const bottom = vv ? vv.offsetTop + vv.height : innerHeight;
    box.style.left = left + "px";
    box.style.width = width + "px";
    box.style.top = Math.max(0, bottom - box.offsetHeight) + "px";
  }
  function placeKeyboardBars() {
    for (const el of keyboardBars) {
      const box = el._box;
      const shown = typeof box?.showPopover === "function" ? box.matches(":popover-open") : box && box.style.display !== "none";
      if (shown) placeKeyboardBar(box);
    }
  }
  document.addEventListener("focusin", syncKeyboardBars);
  // After the focus has moved on (to nothing, or another field).
  document.addEventListener("focusout", () => setTimeout(syncKeyboardBars, 0));
  if (window.visualViewport) {
    visualViewport.addEventListener("resize", placeKeyboardBars);
    visualViewport.addEventListener("scroll", placeKeyboardBars);
  }
  function keyboardBox(el, t) {
    el._kbField = t.includes("field");
    let box = el._box;
    if (!box || el._boxKind !== "keyboard") {
      const old = box;
      box = document.createElement("div");
      box.className = "prism-overlay-box prism-ov-keyboard";
      box.setAttribute("popover", "manual");
      if (typeof box.showPopover !== "function") box.style.display = "none";
      // Presses on the bar keep the focus (and the keyboard) in the field,
      // unless they land in a field of the bar.
      const keep = (e) => {
        const t = e.target;
        if (t && t.closest && t.closest("input, textarea, select, [contenteditable]")) return;
        e.preventDefault();
      };
      box.addEventListener("pointerdown", keep);
      box.addEventListener("mousedown", keep);
      if (old) { box.append(...old.childNodes); old.remove(); }
      el.appendChild(box);
      el._box = box;
      el._boxKind = "keyboard";
    }
    keyboardBars.add(el);
    syncKeyboardBars();
  }
  function overlayBox(el, config) {
    const t = config.split(" ");
    if (t[0] === "keyboard") return keyboardBox(el, t);
    const kind = ["popover", "dialog"].includes(t[0]) ? t[0] : "sheet";
    el._fixed = t.includes("fixed");
    // A popover a click outside does not close; one that leaves the focus
    // to its anchor (inline suggestion menus, #70).
    el._manual = t.includes("manual");
    el._keepFocus = kind === "popover" && t.includes("keepfocus");
    // The side of the anchor it opens on and how it lines up (#113).
    el._side = (t.find((x) => x.startsWith("side=")) || "side=bottom").slice(5);
    el._align = (t.find((x) => x.startsWith("align=")) || "align=start").slice(6);
    // A dialog beside its anchor instead of centered, the backdrop and the
    // size (#79).
    el._placed = kind === "dialog" && t.includes("placed");
    const opt = (name) => (t.find((x) => x.startsWith(`${name}=`)) || "").slice(name.length + 1);
    const [w, h] = opt("size").split("x").map(Number);
    let box = el._box;
    if (!box || el._boxKind !== kind) {
      const old = box;
      box = document.createElement(kind === "popover" ? "div" : "dialog");
      box.className = `prism-overlay-box prism-ov-${kind}`;
      if (kind === "popover") {
        box.addEventListener("toggle", (e) => { if (e.newState === "closed") overlayDismissed(el, lightDismissReason()); });
        // A manual popover hears Escape itself while the focus is inside.
        box.addEventListener("keydown", (e) => {
          if (e.key !== "Escape" || el._fixed || box.getAttribute("popover") !== "manual") return;
          e.stopPropagation();
          overlayDismissed(el, "escape");
        });
        // Clicks inside keep the focus where it is (the anchor's field),
        // unless they land in a field of the popover.
        box.addEventListener("mousedown", (e) => {
          if (!el._keepFocus) return;
          const t = e.target;
          if (t && t.closest && t.closest("input, textarea, select, [contenteditable]")) return;
          e.preventDefault();
        });
      } else {
        box.setAttribute("aria-modal", "true");
        box.addEventListener("cancel", (e) => {
          // Escape closes it, unless it is fixed (then the app offers a way out).
          if (el._fixed) e.preventDefault();
          else el._dismissReason = "escape";
        });
        box.addEventListener("close", () => {
          const reason = el._dismissReason;
          el._dismissReason = null;
          overlayDismissed(el, reason);
        });
        // A click on the backdrop (the dialog box itself, outside its content).
        box.addEventListener("click", (e) => {
          if (e.target !== box || el._fixed) return;
          const r = box.getBoundingClientRect();
          const inside = e.clientX >= r.left && e.clientX <= r.right && e.clientY >= r.top && e.clientY <= r.bottom;
          if (!inside || !box.firstElementChild) { el._closing = false; overlayDismissed(el, "outside"); }
        });
      }
      if (old) { box.append(...old.childNodes); old.remove(); }
      el.appendChild(box);
      el._box = box;
      el._boxKind = kind;
    }
    // Leaving the focus to the anchor: a manual popover (the browser's light
    // dismiss would swallow the press), closed by `pressedAround` (#182).
    if (kind === "popover") box.setAttribute("popover", el._fixed || el._manual || el._keepFocus ? "manual" : "auto");
    box.dataset.medium = t.includes("medium") ? "1" : "";
    box.dataset.prismBackdrop = ["clear", "dim", "blur"].includes(opt("backdrop")) ? opt("backdrop") : "";
    // A dialog or popover without Prism's box: the app draws the panel
    // (#166, #287).
    box.dataset.prismBare = kind !== "sheet" && t.includes("bare") ? "1" : "";
    for (const [axis, v] of [["w", w], ["h", h]]) {
      if (v > 0) { box.dataset[`prism${axis.toUpperCase()}`] = "1"; box.style.setProperty(`--prism-ov-${axis}`, `${v}px`); }
      else delete box.dataset[`prism${axis.toUpperCase()}`];
    }
    if (kind === "dialog" && !el._placed) unplace(el);
    if (el._open) showOverlay(el);
  }
  function showOverlay(el) {
    const box = el._box;
    if (!box) return;
    if (el._boxKind === "keyboard") return syncKeyboardBars();
    // Not in the document yet (inserted open with its parent): shown once
    // the batch has placed it (`flushOverlays`).
    if (!box.isConnected) { pendingOverlays.add(el); return; }
    pendingOverlays.delete(el);
    if (el._boxKind === "popover") {
      const focused = document.activeElement;
      if (typeof box.showPopover === "function") {
        try { if (!box.matches(":popover-open")) box.showPopover(); } catch {}
      } else box.dataset.open = "1";
      // The anchor's field keeps the focus.
      if (el._keepFocus && focused && focused !== document.activeElement && box.contains(document.activeElement)) focused.focus?.();
      placedOverlays.add(el);
      placeOverlay(el);
      requestAnimationFrame(() => placeOverlay(el));
    } else {
      if (!box.open && !box.hasAttribute("open")) {
        if (typeof box.showModal === "function" && box.isConnected) box.showModal();
        else box.setAttribute("open", "");
        userNavigated = true;
        if (focusAsked && focusAsked.isConnected && box.contains(focusAsked)) focusNode(focusAsked);
      }
      // Still modal, beside its anchor instead of centered.
      if (el._boxKind === "dialog" && el._placed) {
        placedOverlays.add(el);
        placeOverlay(el);
        requestAnimationFrame(() => placeOverlay(el));
      }
    }
    raiseLayers();
  }
  function hideOverlay(el) {
    pendingOverlays.delete(el);
    const box = el._box;
    if (!box) return;
    if (el._boxKind === "keyboard") return syncKeyboardBars();
    el._closing = true;
    try {
      placedOverlays.delete(el);
      if (el._boxKind === "popover") {
        if (typeof box.hidePopover === "function") { if (box.matches(":popover-open")) box.hidePopover(); }
        else delete box.dataset.open;
      } else if (typeof box.close === "function") box.close();
      else box.removeAttribute("open");
    } catch {}
    el._closing = false;
  }
  /** Where a box of `w` × `h` goes on `side` of the rectangle `a` (below
   *  by default, above, or beside it: trailing is the right in
   *  left-to-right pages), `gap` away: on the opposite side when there is
   *  no room, lined up with the anchor's start, center or end, kept inside
   *  the window less `m` (#113). `fits`: false when neither side has room.
   *  As `place_beside` in prism-core. */
  function placeBeside(a, w, h, sideName, align, rtl, gap, m) {
    const physical = { trailing: rtl ? "left" : "right", leading: rtl ? "right" : "left", top: "top" };
    let side = physical[sideName] || "bottom";
    const fits = {
      bottom: a.bottom + gap + h <= innerHeight - m,
      top: a.top - gap - h >= m,
      right: a.right + gap + w <= innerWidth - m,
      left: a.left - gap - w >= m,
    };
    const opposite = { bottom: "top", top: "bottom", right: "left", left: "right" };
    if (!fits[side] && fits[opposite[side]]) side = opposite[side];
    let left, top;
    if (side === "bottom" || side === "top") {
      top = side === "bottom" ? a.bottom + gap : a.top - gap - h;
      // Start is the leading edge: the right one in right-to-left pages.
      const start = rtl ? a.right - w : a.left, end = rtl ? a.left : a.right - w;
      left = align === "center" ? a.left + (a.width - w) / 2 : align === "end" ? end : start;
    } else {
      left = side === "right" ? a.right + gap : a.left - gap - w;
      top = align === "center" ? a.top + (a.height - h) / 2 : align === "end" ? a.bottom - h : a.top;
    }
    left = Math.max(m, Math.min(left, innerWidth - w - m));
    top = Math.max(m, Math.min(top, innerHeight - h - m));
    return { left, top, side, fits: fits[side] };
  }
  /** A popover on its side of its anchor (the overlay's parent); a placed
   *  dialog beside it, or centered when neither side has room (a phone,
   *  #79). */
  function placeOverlay(el) {
    const box = el._box, anchor = el.parentElement;
    if (!box || !anchor || !el._open) return;
    const popover = el._boxKind === "popover";
    if (!popover && !(el._boxKind === "dialog" && el._placed)) return;
    const a = anchor.getBoundingClientRect(), b = box.getBoundingClientRect();
    const rtl = document.documentElement.dir === "rtl";
    const { left, top, side, fits } = placeBeside(a, b.width, b.height, el._side, el._align, rtl, popover ? 6 : 8, 8);
    if (!popover) {
      if (!fits) return unplace(el);
      box.dataset.prismPlaced = "1";
    }
    box.style.left = `${round(left)}px`;
    box.style.top = `${round(top)}px`;
    box.dataset.side = side;
  }
  /** A dialog back in the middle of the window. */
  function unplace(el) {
    const box = el._box;
    if (!box || !box.dataset.prismPlaced) return;
    delete box.dataset.prismPlaced;
    delete box.dataset.side;
    box.style.left = "";
    box.style.top = "";
  }
  const replace = () => placedOverlays.forEach(placeOverlay);
  addEventListener("resize", replace);
  addEventListener("scroll", replace, true);

  // --- Choice controls (#18) ----------------------------------------------------
  function radioOptions(el, id, config) {
    const lines = config.split("\n");
    if (lines[0] === "row") el.dataset.row = "1"; else delete el.dataset.row;
    const options = lines.slice(1);
    el.replaceChildren(...options.map((text, i) => {
      const label = document.createElement("label");
      const input = document.createElement("input");
      input.type = "radio";
      input.name = `prism-radio-${id}`;
      input.value = String(i);
      input.checked = i === el._index;
      input.disabled = !!el._disabled;
      input.addEventListener("change", () => { if (input.checked) send({ t: "change", id, v: i }); });
      const span = document.createElement("span");
      span.textContent = text;
      label.append(input, span);
      return label;
    }));
  }
  const radios = (el) => el.querySelectorAll("input");

  /** A glyph drawn inside a text field, before the text. */
  function setFieldIcon(el, glyph) {
    toggleClass(el, "prism-icon-field", !!glyph);
    if (glyph) el.style.setProperty("--prism-icon", `url("data:image/svg+xml,${encodeURIComponent(`<svg xmlns='http://www.w3.org/2000/svg' width='16' height='16'><text x='8' y='14' font-size='16' text-anchor='middle' fill='gray' font-family='system-ui,sans-serif'>${glyph.replace(/[<&'"]/g, "")}</text></svg>`)}")`);
    else el.style.removeProperty("--prism-icon");
  }
  /** `<datalist>` of a text field's suggestions. */
  function setSuggestions(el, list) {
    if (!list.length) {
      if (el._list) { el._list.remove(); el._list = null; el.removeAttribute("list"); }
      return;
    }
    if (!el._list) {
      el._list = document.createElement("datalist");
      el._list.id = `prism-list-${el.dataset.pid}`;
    }
    el._list.replaceChildren(...list.map((v) => {
      const o = document.createElement("option");
      o.setAttribute("value", v);
      return o;
    }));
    if (!el._list.isConnected) document.body.appendChild(el._list);
    el.setAttribute("list", el._list.id);
  }

  // Shapes (prism-core shape.rs): paths in fractions of the box become an
  // SVG in pixels, drawn again whenever the box is resized.
  const svgNS = "http://www.w3.org/2000/svg";
  const shapeObserver = typeof ResizeObserver === "function"
    ? new ResizeObserver((entries) => entries.forEach((e) => drawShape(e.target)))
    : null;
  // Rules on an element's own width (`.when_width_at_least`): the server
  // sends the CSS for each width they switch at; the element takes the
  // widest one it reaches, as it is resized. Its first width is known once
  // it is laid out: the rule for it is part of its first style, which
  // `transition` does not animate (#50); later widths animate.
  const widthObserver = typeof ResizeObserver === "function"
    ? new ResizeObserver((entries) => entries.forEach((e) => {
      const box = e.borderBoxSize && e.borderBoxSize[0];
      const first = !e.target._widthSeen;
      e.target._widthSeen = true;
      applyOwnWidth(e.target, box ? box.inlineSize : e.contentRect.width, first);
    }))
    : null;
  function applyOwnWidth(el, width, first) {
    const v = el._ownWidth;
    if (!v) return;
    let css = v.base;
    for (const [min, variant] of v.cq) if (width >= min) css = variant;
    if (v.applied === css) return;
    v.applied = css;
    if (first) setCssAtOnce(el, css); else setCss(el, css);
  }
  /**
   * Gives `el` the style `css` without animating to it: CSS transitions
   * start when a computed style changes, so the change is computed with
   * `transition: none`, then the element's own transition is back for the
   * next ones.
   */
  function setCssAtOnce(el, css) {
    if (!el.isConnected || !el.style) return setCss(el, css);
    const inline = el.style.transition;
    el.style.transition = "none";
    setCss(el, css);
    void getComputedStyle(el).transitionProperty; // the style is computed now
    el.style.transition = inline;
    if (!el.getAttribute("style")) el.removeAttribute("style");
  }
  function ownWidth(el, css, cq) {
    if (!cq) {
      if (el._ownWidth && widthObserver && widthObserver.unobserve) widthObserver.unobserve(el);
      el._ownWidth = null;
      return;
    }
    el._ownWidth = { base: css, cq, applied: css };
    if (widthObserver) widthObserver.observe(el);
    applyOwnWidth(el, el.getBoundingClientRect ? el.getBoundingClientRect().width : 0);
  }

  const shapeColor = (s) => {
    if (!s) return "none";
    // `Color::current()` (an SVG's `currentColor`): `~` and its alpha.
    if (s[0] === "~") {
      const a = parseInt(s.slice(1), 16);
      if (isNaN(a) || a >= 255) return "currentColor";
      return `color-mix(in srgb, currentColor ${Math.round(a / 2.55 * 10) / 10}%, transparent)`;
    }
    const [light, dark] = s.split("/");
    return dark ? `light-dark(${light}, ${dark})` : light;
  };

  function drawShape(el) {
    const w = el.clientWidth, h = el.clientHeight;
    let ix = 0, iy = 0;
    const paths = [];
    for (const line of (el._drawing || "").split("\n")) {
      if (line[0] === "i") { [ix, iy] = line.slice(1).split(",").map(Number); continue; }
      const bar = line.indexOf("|");
      if (bar < 0) continue;
      const p = { fill: null, stroke: null, w: 1, round: false, dash: null, t: line.slice(bar + 1).trim().split(/\s+/) };
      for (const tok of line.slice(0, bar).trim().split(/\s+/)) {
        const v = tok.slice(1);
        if (tok[0] === "f") p.fill = v;
        else if (tok[0] === "s") p.stroke = v;
        else if (tok[0] === "w") p.w = +v;
        else if (tok[0] === "r") p.round = true;
        else if (tok[0] === "d") p.dash = v.replace(",", " ");
        else if (tok[0] === "e") p.evenOdd = true;
      }
      paths.push(p);
    }
    const iw = Math.max(0, w - 2 * ix), ih = Math.max(0, h - 2 * iy), side = Math.min(iw, ih);
    const X = (x) => ix + x * iw, Y = (y) => iy + y * ih;
    // The server's first paint may have drawn it already (#285).
    let svg = el._svg || (el._svg = el.querySelector(":scope > svg"));
    if (svg) svg.removeAttribute("preserveAspectRatio");
    if (!svg) {
      svg = el._svg = document.createElementNS(svgNS, "svg");
      svg.setAttribute("aria-hidden", "true");
      el.append(svg);
    }
    svg.setAttribute("viewBox", `0 0 ${w} ${h}`);
    svg.replaceChildren(...paths.map((p) => {
      const d = [], t = p.t;
      let open = false;
      for (let i = 0; i < t.length;) {
        const c = t[i++];
        const n = (k) => t.slice(i, (i += k)).map(Number);
        if (c === "M" || c === "L") {
          const [x, y] = n(2);
          d.push(`${open && c === "L" ? "L" : "M"}${X(x)} ${Y(y)}`);
          open = true;
        } else if (c === "C") {
          const v = n(6);
          if (!open) d.push(`M${X(v[0])} ${Y(v[1])}`);
          d.push(`C${X(v[0])} ${Y(v[1])} ${X(v[2])} ${Y(v[3])} ${X(v[4])} ${Y(v[5])}`);
          open = true;
        } else if (c === "A") {
          // Degrees clockwise from 12 o'clock; radius: fraction of the smaller side.
          const [cx0, cy0, r0, a0, a1] = n(5);
          const cx = X(cx0), cy = Y(cy0), r = r0 * side;
          const at = (a) => `${cx + r * Math.sin(a * Math.PI / 180)} ${cy - r * Math.cos(a * Math.PI / 180)}`;
          d.push(`${open ? "L" : "M"}${at(a0)}`);
          open = true;
          const sweep = Math.max(-360, Math.min(360, a1 - a0));
          // One SVG arc cannot draw a full circle: two halves at most.
          const parts = Math.abs(sweep) > 180 ? 2 : 1;
          for (let k = 1; k <= parts; k++) {
            d.push(`A${r} ${r} 0 0 ${sweep > 0 ? 1 : 0} ${at(a0 + (sweep / parts) * k)}`);
          }
        } else if (c === "Z") {
          d.push("Z");
          open = false;
        } else break;
      }
      const e = document.createElementNS(svgNS, "path");
      e.setAttribute("d", d.join(""));
      e.style.fill = shapeColor(p.fill);
      e.style.stroke = shapeColor(p.stroke);
      e.style.strokeWidth = p.w;
      if (p.round) { e.style.strokeLinecap = "round"; e.style.strokeLinejoin = "round"; }
      if (p.dash) e.style.strokeDasharray = p.dash;
      if (p.evenOdd) e.style.fillRule = "evenodd";
      return e;
    }));
  }

  // Rich text (prism-core spans.rs): runs separated by U+001E, fields by
  // U+001F (text first, then `k=v`): inline spans, links and symbols.
  function decodeRuns(config) {
    if (!config) return [];
    return config.split("\u001e").map((r) => {
      const [text, ...fields] = r.split("\u001f");
      const run = { text };
      for (const f of fields) {
        const i = f.indexOf("=");
        run[i < 0 ? f : f.slice(0, i)] = i < 0 ? true : f.slice(i + 1);
      }
      return run;
    });
  }
  // The same stacks as `FontFamily::css` (style.rs).
  const SYSTEM_FONT = '-apple-system,BlinkMacSystemFont,"Segoe UI",system-ui,sans-serif';
  const fam = {
    system: SYSTEM_FONT,
    mono: 'ui-monospace,SFMono-Regular,Menlo,Consolas,"Liberation Mono",monospace',
    serif: 'ui-serif,"New York",Georgia,"Times New Roman",serif',
    rounded: `ui-rounded,"SF Pro Rounded",${SYSTEM_FONT}`,
  };
  // An underline's own line (#221), as `css::underline_css`.
  const UNDERLINE = { thick: "text-decoration-thickness:max(2px,.12em)", double: "text-decoration-style:double", dotted: "text-decoration-style:dotted", dashed: "text-decoration-style:dashed" };
  const runCss = (r) => {
    const d = [r.u && "underline", r.s && "line-through"].filter(Boolean).join(" ");
    return [
      r.w && `font-weight:${r.w}`, r.i && "font-style:italic", d && `text-decoration-line:${d}`,
      r.u && r.uc && `text-decoration-color:${shapeColor(r.uc)}`, r.u && UNDERLINE[r.us],
      r.c && `color:${shapeColor(r.c)}`, r.b && `background-color:${shapeColor(r.b)};border-radius:3px`,
      r.f && `font-family:${fam[r.f] || `"${r.f.replace(/["\\<>]/g, "")}",${SYSTEM_FONT}`}`,
      r.z && `font-size:${r.z}em`,
    ].filter(Boolean).join(";");
  };
  function renderRuns(el) {
    // Inside a text that is a link itself, links are spans (they don't nest).
    const nested = el.localName === "a";
    const runs = decodeRuns(el._runs);
    el._runPs = null;
    if (!runs.length) { el.replaceChildren(document.createTextNode(el._text || "")); return; }
    el.replaceChildren(...runs.map((r) => {
      const css = runCss(r);
      let node;
      if (r.k) {
        // An inline object (#115): its icon, then its label.
        node = document.createElement("span");
        node.className = "prism-chip";
        if (r.l) node.dataset.link = r.l;
        if (r.m) {
          const g = document.createElement("span");
          g.className = "prism-symbol";
          g.setAttribute("aria-hidden", "true");
          g.textContent = glyphs[r.m] || "";
          node.appendChild(g);
        }
        node.appendChild(document.createTextNode(r.n || ""));
        if (css) { setCss(node, css); (el._runPs ||= []).push(node._ps); }
        return node;
      } else if (r.g) {
        node = document.createElement("span");
        node.className = "prism-symbol";
        node.setAttribute("aria-hidden", "true");
      } else if (r.l) {
        const url = r.h && !nested ? hrefFor(r.h) : null;
        const href = url != null;
        node = document.createElement(href ? "a" : "span");
        node.className = "prism-link";
        node.dataset.link = r.l;
        if (href) node.setAttribute("href", url);
        else if (!nested) { node.setAttribute("role", "link"); node.tabIndex = 0; }
      } else if (css) {
        node = document.createElement("span");
      } else {
        return document.createTextNode(r.text);
      }
      node.textContent = r.text;
      if (css) {
        setCss(node, css);
        (el._runPs ||= []).push(node._ps);
      }
      return node;
    }));
  }
  // A link inside a text: reported like a menu item (the app navigates).
  function activateRun(e) {
    const a = e.target.closest && e.target.closest("[data-link]");
    if (!a) return false;
    e.preventDefault();
    e.stopPropagation();
    userNavigated = true;
    send({ t: "menu", id: Number(a.dataset.link) });
    return true;
  }

  // --- Links: a view or a text made a link (`.link(path)`) -------------------
  // An `<a href>`, so the browser opens it in a new tab or window, copies it
  // and shows it on hover; a plain click still navigates in the app.
  const SCHEME = /^[a-z][a-z0-9+.-]*:/i;
  // Schemes that run script in the page or show content the app did not
  // serve: such a link gets no href and is never opened (html.rs's
  // `BLOCKED_SCHEMES`). The URL is read as the browser would: controls and
  // spaces trimmed at the ends, tabs and newlines dropped (`safe_url`).
  const BLOCKED = /^(?:javascript|vbscript|data|blob|filesystem):/i;
  const safeUrl = (url) => {
    const u = String(url).replace(/^[\u0000-\u0020]+|[\u0000-\u0020]+$/g, "").replace(/[\t\n\r]/g, "");
    return BLOCKED.test(u) ? null : u;
  };
  // A link's href (`link_href`): a URL as is, an app path under the mount
  // point, null for a blocked URL.
  const hrefFor = (path) => {
    const u = safeUrl(path);
    return u == null || SCHEME.test(u) ? u : urlFor(u);
  };
  // Sent right after "create" (on the web, a node that gains or loses its
  // link is a new node): the element becomes an `<a>`.
  function setLink(el, id, href) {
    const kind = el && el.dataset.kind;
    // Views, texts, and buttons (#230): an `<a>` with the button's look.
    if ((kind !== "view" && kind !== "text" && kind !== "button") || el._island) return;
    if ((href != null) !== (el.localName === "a")) {
      create(id, kind, href != null ? "a" : null);
      const fresh = nodes.get(id);
      while (el.firstChild) fresh.appendChild(el.firstChild);
      if (el.parentNode) { el.parentNode.insertBefore(fresh, el); el.remove(); }
      el = fresh;
    }
    if (href == null) return;
    const url = hrefFor(href);
    if (url == null) {
      el.removeAttribute("href");
      el.removeAttribute("rel");
      return;
    }
    el.setAttribute("href", url);
    if (SCHEME.test(url)) el.setAttribute("rel", "noopener"); else el.removeAttribute("rel");
  }
  // A click on a link node. With a modifier or another button (new tab, new
  // window, download) the browser follows it: true. A plain click stays in
  // the app (no page load): false, for the click handler to report it.
  function browserFollows(el, e) {
    if (el.localName !== "a") return false;
    if (el.getAttribute("aria-disabled") !== "true" && (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || (e.button || 0) !== 0)) {
      e.stopPropagation();
      return true;
    }
    e.preventDefault();
    return false;
  }

  function create(id, kind, tag) {
    const el = document.createElement(tag || tags[kind]);
    el.dataset.pid = id;
    el.dataset.kind = kind;
    // Form fields are named (browsers flag fields without an id or name, and
    // autofill reads it); a radio group names its inputs, a checkbox its box.
    if (["INPUT", "SELECT", "TEXTAREA"].includes(el.tagName)) el.name = `prism-${id}`;
    if (kind === "text_input") {
      el.type = "text";
      el.addEventListener("input", () => sendTyped(el, id));
      el.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && !e.isComposing) send({ t: "submit", id });
      });
      trackComposition(el);
      trackSelection(el, id);
    } else if (kind === "text_area") {
      el.className = "prism-textarea";
      el.rows = 3;
      el.addEventListener("input", () => { grow(el); sendTyped(el, id); });
      trackComposition(el);
      trackSelection(el, id);
    } else if (kind === "rich_input") {
      richSetup(el, id);
    } else if (kind === "slider") {
      el.type = "range";
      el.className = "prism-slider";
      el.addEventListener("input", () => send({ t: "change", id, v: Number(el.value) }));
    } else if (kind === "picker") {
      el.className = "prism-select";
      el._ph = document.createElement("option");
      Object.assign(el._ph, { disabled: true, hidden: true, value: "" });
      el._index = -1;
      showPicked(el);
      el.addEventListener("change", () => {
        el._index = el.selectedIndex - (el._ph.parentNode === el ? 1 : 0);
        showPicked(el);
        send({ t: "change", id, v: el._index });
      });
    } else if (kind === "date_picker") {
      el.type = "date";
      el.className = "prism-date";
      el.addEventListener("change", () => {
        const secs = dateSeconds(el);
        if (secs !== null) send({ t: "change", id, v: secs });
        // Cleared (#89): the browser's clear button, or every part erased.
        else if (el._optional && Number.isFinite(el._secs)) send({ t: "change", id, v: null });
      });
    } else if (kind === "spinner") {
      el.className = "prism-spinner";
    } else if (kind === "table") {
      // Rows of column headers go in its head, the others in its body (#284).
      el.className = "prism-table";
      el._head = el.appendChild(document.createElement("thead"));
      el._rows = el.appendChild(document.createElement("tbody"));
    } else if (kind === "column_header" || kind === "table_cell") {
      // A cell's content is laid out in its box (`prism.css`).
      if (kind === "column_header") el.scope = "col";
      el._cell = el.appendChild(document.createElement("div"));
      el._cell.className = "prism-cell";
    } else if (kind === "web_view") {
      webViewInit(el, id);
    } else if (kind === "toggle") {
      el.type = "checkbox";
      el.className = "prism-switch";
      el.setAttribute("role", "switch");
      el.addEventListener("change", () => send({ t: "change", id, v: el.checked ? 1 : 0 }));
    } else if (kind === "stack") {
      el.className = "prism-stack";
      el.innerHTML =
        '<div class="prism-header"><div class="prism-bar prism-bar-leading"><button class="prism-back" aria-label="Back" hidden>‹ Back</button></div>' +
        '<h1 class="prism-title" tabindex="-1"></h1><div class="prism-bar prism-bar-trailing"></div></div><div class="prism-body"></div>';
      el._body = el.lastChild;
      el._header = el.firstChild;
      el._back = el.querySelector(".prism-back");
      el._title = el.querySelector(".prism-title");
      el._leading = el.querySelector(".prism-bar-leading");
      el._trailing = el.querySelector(".prism-bar-trailing");
      el._screens = [];
      el._back.addEventListener("click", (e) => {
        e.stopPropagation();
        userNavigated = true;
        send({ t: "back", id, depth: el._screens.length - 1 });
      });
      stacks.push(el);
    } else if (kind === "tabs") {
      el.className = "prism-tabs";
      el.innerHTML = '<div class="prism-panels"></div><div class="prism-tabbar" role="tablist"></div>';
      el._panels = el.firstChild;
      el._bar = el.lastChild;
      el._tabs = [];
      el._selected = 0;
      el._bar.addEventListener("keydown", (e) => {
        const n = el._tabs.length;
        const moves = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: n - 1, ArrowUp: n - 1 };
        let next = null;
        if (e.key in moves) next = (el._selected + moves[e.key]) % n;
        if (e.key === "Home") next = 0;
        if (e.key === "End") next = n - 1;
        if (next === null) return;
        e.preventDefault();
        selectTab(el, next, true);
      });
    } else if (kind === "modal") {
      el.className = "prism-sheet";
      el.setAttribute("aria-modal", "true");
      // Escape: let the app decide (it removes the sheet).
      el.addEventListener("cancel", (e) => { e.preventDefault(); send({ t: "back", id, depth: 0, reason: "escape" }); });
    } else if (kind === "scroll") {
      el.className = "prism-scroll";
    } else if (kind === "video") {
      // The browser's player (#207): it says when it played to the end.
      el.className = "prism-video";
      el.playsInline = true;
      el.setAttribute("playsinline", "");
      el.preload = "metadata";
      el.addEventListener("ended", () => send({ t: "ended", id }));
    } else if (kind === "picture") {
      el.className = "prism-picture";
      el.loading = "lazy";
      el.decoding = "async";
      el.alt = "";
    } else if (kind === "shape") {
      el.className = "prism-shape";
      if (shapeObserver) shapeObserver.observe(el);
    } else if (kind === "scene3d") {
      // Its canvas and player come with its first update (`op: "scene"`).
      el.className = "prism-scene3d";
    } else if (kind === "image") {
      el.className = "prism-symbol";
    } else if (kind === "progress") {
      el.max = 1;
    } else if (kind === "timer") {
      el.className = "prism-timer";
    } else if (kind === "view") {
      el.addEventListener("click", (e) => {
        if (browserFollows(el, e) || el.dataset.clickable !== "1") return;
        e.stopPropagation();
        activate(el, id, e);
      });
      // Keyboard activation for custom buttons (ARIA button pattern).
      el.addEventListener("keydown", (e) => {
        if (el.dataset.clickable !== "1" || e.target !== el) return;
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          activate(el, id);
        }
      });
    } else if (kind === "text") {
      el.addEventListener("click", (e) => {
        if (activateRun(e) || browserFollows(el, e) || el.dataset.clickable !== "1") return;
        e.stopPropagation();
        activate(el, id);
      });
      el.addEventListener("keydown", (e) => {
        if (e.key !== "Enter" && e.key !== " ") return;
        // Anchors activate on Enter by themselves (a click event).
        if (e.target !== el && e.target.tagName !== "A" && activateRun(e)) return;
        if (el.dataset.clickable !== "1" || e.target !== el) return;
        e.preventDefault();
        activate(el, id);
      });
    } else if (kind === "button") {
      // A link button is an `<a>` (#230), with the look of one.
      if (el.localName === "a") el.className = "prism-link-button"; else el.type = "button";
      el.dataset.clickable = "1";
      el.addEventListener("click", (e) => {
        e.stopPropagation();
        // ⌘-click, a middle click: the browser opens the link (#230).
        if (browserFollows(el, e)) return;
        activate(el, id, e);
      });
    } else if (kind === "overlay") {
      el.className = "prism-overlay";
    } else if (kind === "checkbox") {
      el.className = "prism-check";
      el.innerHTML = '<input type="checkbox"><span></span>';
      el._input = el.firstChild;
      el._input.name = `prism-${id}`;
      el._label = el.lastChild;
      el._input.addEventListener("change", () => send({ t: "change", id, v: el._input.checked ? 1 : 0 }));
    } else if (kind === "radio_group") {
      el.className = "prism-radios";
      el.setAttribute("role", "radiogroup");
      el._index = -1;
    }
    nodes.set(id, el);
  }

  // --- Interaction states (prism-core `interaction`) ------------------------------
  // `.hover(|s| …)`, `.active(…)`, `.focus(…)`, `.focus_within(…)`: the
  // server sends each node's pseudo-class rules, gathered in one <style>
  // (written once per batch of patches, by `flushStates`).
  const stateRules = new Map();
  let statesSheet = null;
  let statesDirty = false;
  function setStates(id, rules) {
    if ((stateRules.get(id) || "") === (rules || "")) return;
    if (rules) stateRules.set(id, rules); else stateRules.delete(id);
    statesDirty = true;
  }
  function clearStates() {
    stateRules.clear();
    statesDirty = true;
  }
  function flushStates() {
    if (!statesDirty) return;
    statesDirty = false;
    if (!statesSheet) {
      statesSheet = newStyle();
      statesSheet.id = "prism-states";
      document.head.appendChild(statesSheet);
    }
    statesSheet.textContent = [...stateRules.values()].join("\n");
  }
  // iOS Safari only applies `:active` once a touch listener exists.
  document.addEventListener("touchstart", () => {}, { passive: true });

  // Orders Prism children; helper elements (no data-pid) are left in place.
  // --- Motion (prism-core `animation`) ------------------------------------------
  const reducedMotion = window.matchMedia ? matchMedia("(prefers-reduced-motion: reduce)") : null;
  /** Set while a reconnect rebuilds the page: what is shown does not enter again. */
  let quiet = false;
  const poseCss = (p) => {
    const d = document.documentElement.dir === "rtl" ? -1 : 1;
    const [x, y, s, r] = p.tf;
    if (p.tf.length > 4) {
      // Non-uniform scale, offsets in percent of the element's own size.
      const [, , , , sx, sy, px, py] = p.tf;
      return { opacity: p.o, transform: `translate(calc(${d * x}px + ${d * px}%),calc(${y}px + ${py}%)) rotate(${d * r}deg) scale(${s * sx},${s * sy})` };
    }
    return { opacity: p.o, transform: `translate(${d * x}px,${y}px) rotate(${d * r}deg) scale(${s})` };
  };
  const timing = (t, extra) => Object.assign({ duration: t.d * 1000, delay: t.delay * 1000, easing: t.ease }, extra);

  /** Enters (new, or shown again) and loops of an element. */
  function motion(el, m, wasHidden) {
    const first = !el._styled;
    el._styled = true;
    el._motion = m || null;
    const key = m && m.repeat ? JSON.stringify([m.rest, m.repeat]) : "";
    if (el._loopKey !== key) {
      if (el._loop) el._loop.cancel();
      el._loop = null;
      el._loopKey = key;
      if (key && !reducedMotion?.matches) {
        // Swings ease in and out; a loop that starts over keeps its curve
        // (rotate(0deg) to rotate(360deg) is a full turn).
        el._loop = el.animate([poseCss(m.rest), poseCss(m.repeat.to)],
          timing(m.repeat.t, m.repeat.restart ? { iterations: Infinity }
            : { iterations: Infinity, direction: "alternate", easing: "ease-in-out" }));
      }
    }
    if (!m || !m.enter || quiet || reducedMotion?.matches) return;
    if (!first && !(wasHidden && !hiddenBy(el._css))) return;
    el.animate([poseCss(m.enter), poseCss(m.rest)], timing(m.t, { fill: "backwards" }));
  }

  /** A copy of a removed element stays in place and animates to its exit pose. */
  function exit(parent, el) {
    const m = el._motion;
    if (!m || !m.exit || quiet || reducedMotion?.matches || !el.isConnected) return;
    const r = el.getBoundingClientRect(), pr = parent.getBoundingClientRect();
    if (!r.width || !r.height) return;
    if (getComputedStyle(parent).position === "static") parent.style.position = "relative";
    const ghost = el.cloneNode(true);
    ghost.removeAttribute("data-pid");
    ghost.querySelectorAll("[data-pid]").forEach((n) => n.removeAttribute("data-pid"));
    ghost.setAttribute("aria-hidden", "true");
    Object.assign(ghost.style, {
      position: "absolute", margin: "0", pointerEvents: "none", transition: "none",
      left: `${el.offsetLeft}px`, top: `${el.offsetTop}px`, width: `${el.offsetWidth}px`, height: `${el.offsetHeight}px`,
    });
    if (el.offsetParent !== parent) {
      ghost.style.left = `${r.left - pr.left - parent.clientLeft + parent.scrollLeft}px`;
      ghost.style.top = `${r.top - pr.top - parent.clientTop + parent.scrollTop}px`;
    }
    parent.appendChild(ghost);
    ghost.animate([poseCss(m.rest), poseCss(m.exit)], timing(m.xt, { fill: "forwards" })).finished
      .then(() => ghost.remove(), () => ghost.remove());
  }

  function setChildren(el, ids) {
    const kids = ids.map((i) => nodes.get(i)).filter(Boolean);
    const keep = new Set(kids);
    [...el.children].forEach((c) => { if (c.dataset.pid && !keep.has(c)) { exit(el, c); c.remove(); } });
    let anchor = [...el.children].find((c) => c.dataset.pid) || null;
    kids.forEach((kid) => {
      if (kid === anchor) {
        anchor = nextPrism(anchor);
      } else {
        el.insertBefore(kid, anchor);
      }
    });
  }
  const nextPrism = (n) => { let c = n.nextElementSibling; while (c && !c.dataset.pid) c = c.nextElementSibling; return c; };
  /** A table's rows (#284): rows of column headers in `<thead>`, the others in `<tbody>`. */
  function tableRows(table, ids) {
    table._ids = ids;
    const head = ids.filter((i) => nodes.get(i)?.firstElementChild?.localName === "th");
    setChildren(table._head, head);
    setChildren(table._rows, ids.filter((i) => !head.includes(i)));
  }


  // Header texts in the app's language (sent with the environment).
  let chrome = { back: "Back", close: "Close" };

  // --- Stacks: header (Back + title), only the top screen visible -------------
  function refreshStack(st) {
    const screens = st._screens.map((i) => nodes.get(i)).filter(Boolean);
    screens.forEach((s, i) => (s.hidden = i !== screens.length - 1));
    const top = screens[screens.length - 1];
    const title = (top && top._titleText) || "";
    // Stacks inside sheets show Close on their first screen.
    const closable = st._closable && screens.length <= 1;
    // No header bar: for the whole stack (`Router::header(false)`) or while a
    // bare screen is on top (`screen(..).bare()`).
    st._header.hidden = !!st._noHeader || !!(top && top._bare);
    st._back.hidden = screens.length <= 1 && !closable;
    st._back.textContent = closable ? chrome.close : `‹ ${chrome.back}`;
    st._back.setAttribute("aria-label", closable ? chrome.close : chrome.back);
    st._title.textContent = title;
    refreshBar(st, top);
    if (top) top.setAttribute("aria-label", title);
    if (isMainStack(st)) {
      screenTitle = title;
      showTitle();
    }
  }

  // --- Bar buttons of the top screen (#30) ----------------------------------------
  /** A screen's config: `large`/`inline`, then `L|T \t icon \t title \t label` per button. */
  function barItems(config) {
    return (config || "").split("\n").slice(1).map((line, index) => {
      const [side, icon = "", title = "", label = ""] = line.split("\t");
      return { index, trailing: side !== "L", icon, title, label };
    });
  }
  function refreshBar(st, top) {
    if (!st._leading) return;
    const items = barItems(top && top._barConfig);
    const fill = (box, trailing) => {
      box.querySelectorAll(":scope > .prism-bar-button").forEach((b) => b.remove());
      items.filter((it) => it.trailing === trailing).forEach((it) => {
        const b = document.createElement("button");
        b.type = "button";
        b.className = "prism-bar-button";
        b.dataset.bar = String(it.index);
        b.textContent = glyphs[it.icon] || it.title || (it.icon ? "•" : "");
        b.setAttribute("aria-label", it.label || it.title || it.icon);
        b.addEventListener("click", (e) => {
          e.stopPropagation();
          send({ t: "change", id: Number(top.dataset.pid), v: it.index });
        });
        box.appendChild(b);
      });
    };
    fill(st._leading, false);
    fill(st._trailing, true);
  }

  // --- Tabs: ARIA tablist with roving tabindex ----------------------------------
  function selectTab(tabs, index, fromUser) {
    if (fromUser) {
      userNavigated = true;
      send({ t: "change", id: Number(tabs.dataset.pid), v: index });
      if (index !== tabs._selected) awaitLazyTab(tabs, index);
    }
    tabs._selected = index;
    refreshTabs(tabs);
    if (fromUser) tabs._bar.children[index]?.focus();
  }

  function refreshTabs(tabs) {
    const panels = tabs._tabs.map((i) => nodes.get(i)).filter(Boolean);
    const bar = tabs._bar;
    while (bar.children.length > panels.length) bar.lastElementChild.remove();
    const shown = lazyTabShown(tabs, panels);
    panels.forEach((panel, i) => {
      let btn = bar.children[i];
      if (!btn) {
        btn = document.createElement("button");
        btn.type = "button";
        btn.setAttribute("role", "tab");
        btn.className = "prism-tab";
        btn.addEventListener("click", () => selectTab(tabs, [...bar.children].indexOf(btn), true));
        bar.appendChild(btn);
      }
      const pid = panel.dataset.pid;
      const selected = i === tabs._selected;
      btn.id = `prism-tab-${pid}`;
      btn.setAttribute("aria-controls", `prism-panel-${pid}`);
      btn.setAttribute("aria-selected", String(selected));
      btn.tabIndex = selected ? 0 : -1;
      const glyph = panel._glyph || glyphs[panel._icon] || "";
      btn.innerHTML = `<span class="prism-tab-icon" aria-hidden="true">${glyph}</span><span></span>`;
      btn.lastChild.textContent = panel._titleText || "";
      panel.id = `prism-panel-${pid}`;
      panel.setAttribute("role", "tabpanel");
      panel.setAttribute("aria-labelledby", btn.id);
      panel.hidden = i !== shown;
    });
    const s = navState();
    if (s && s.stack) refreshStack(s.stack);
  }

  // --- Lazy tabs: rendered by the server on their first selection ----------------
  /** A lazy tab never selected: its stack has no screen yet. */
  function lazyPanel(panel) {
    const st = panel && panel.firstElementChild;
    return !!st && st.dataset.kind === "stack" && !!st._screens && st._screens.length === 0;
  }
  /**
   * Picking a tab the server has not rendered yet: the panel shown keeps
   * showing until the server's answer (one round trip) instead of an empty
   * one. Its answer selects the tab (a "value" patch), which ends the wait;
   * so does a timeout, should it never come.
   */
  function awaitLazyTab(tabs, index) {
    if (!lazyPanel(nodes.get(tabs._tabs[index]))) return;
    tabs._awaiting = true;
    clearTimeout(tabs._awaitTimer);
    tabs._awaitTimer = setTimeout(() => { tabs._awaiting = false; refreshTabs(tabs); }, 1000);
  }
  function endLazyWait(tabs) {
    tabs._awaiting = false;
    clearTimeout(tabs._awaitTimer);
  }
  /** Index of the panel to show: the selected one, or the previous one while a lazy tab renders. */
  function lazyTabShown(tabs, panels) {
    const keep = tabs._awaiting && lazyPanel(panels[tabs._selected]) && panels[tabs._shown];
    tabs._shown = keep ? tabs._shown : tabs._selected;
    return tabs._shown;
  }

  // --- Form controls (prism-core `controls`) -------------------------------------
  /** A text area as tall as its text (its max-height then scrolls it). */
  // --- Text fields: echoes of the user's typing vs. values set by the app (#73) ---
  //
  // Every value typed is sent and remembered (`el._sent`) until the server
  // renders it back. A text patch equal to one of them is an echo: the field
  // already shows it (or has moved on since), so it is left alone, caret and
  // IME composition included. Any other value comes from the app (a Markdown
  // shortcut stripping "# ", clearing after a command, normalising): it is
  // applied, focused or not, keeping the caret at the same distance from the
  // end. During an IME composition it waits for `compositionend`.

  /** Sends what the user typed in field `el` (node `id`) and remembers it. */
  function sendTyped(el, id) {
    const sent = el._sent || (el._sent = []);
    sent.push(el.value);
    if (sent.length > 64) sent.shift();
    send({ t: "input", id, v: el.value });
  }

  function trackComposition(el) {
    el.addEventListener("compositionstart", () => { el._composing = true; });
    el.addEventListener("compositionend", () => {
      el._composing = false;
      const text = el._deferred, sel = el._deferredSel;
      delete el._deferred;
      delete el._deferredSel;
      if (text !== undefined) {
        applyFieldText(el, text);
        if (el.tagName === "TEXTAREA") grow(el);
      }
      if (sel) applyFieldSelection(el, sel[0], sel[1]);
    });
  }

  // --- Text fields: selection and caret (#86) -----------------------------------
  //
  // Prism counts characters (code points); the DOM counts UTF-16 units.
  // A field with `on_select` (`selev`) reports where its selection is after
  // anything that may move it (selecting, clicking, arrows, typing), at most
  // once per animation frame and only when it moved, after the `input` that
  // moved it. Key presses in a field carry its selection before the key
  // (`sel`). The app's `.selection(..)` (`sel` op) is applied unless the
  // user typed past the text it was computed for (#73's rule).

  /** The character offset of UTF-16 offset `units` in `text`. */
  const toChars = (text, units) => Array.from(text.slice(0, units)).length;
  /** The UTF-16 offset of character `chars` in `text` (clamped). */
  function toUnits(text, chars) {
    let units = 0, n = 0;
    for (const c of text) {
      if (n++ >= chars) break;
      units += c.length;
    }
    return units;
  }
  /** Field `el`'s selection in characters, `null` without a caret. */
  function fieldSelection(el) {
    if (el._rich) return richSelection(el);
    let s, e;
    try { s = el.selectionStart; e = el.selectionEnd; } catch { return null; /* email, number */ }
    if (s == null || e == null) return null;
    const v = el.value || "";
    return [toChars(v, Math.min(s, e)), toChars(v, Math.max(s, e))];
  }
  /**
   * Where field `el`'s caret is (#211): "s" at the start, "e" at the end,
   * "f" on the first visual line, "l" on the last (wrapping included); ""
   * while text is selected, null when it cannot tell.
   */
  function caretEdges(el) {
    const sel = fieldSelection(el);
    if (!sel) return null;
    if (sel[0] !== sel[1]) return "";
    const text = el._rich ? richText(el._model) : (el.value || "");
    const pos = sel[0];
    let lines = [true, true];
    if (el._rich || el.tagName === "TEXTAREA") lines = visualLines(el, text, pos) || logicalLines(text, pos);
    return (pos === 0 ? "s" : "") + (pos >= clen(text) ? "e" : "") + (lines[0] ? "f" : "") + (lines[1] ? "l" : "");
  }
  /** [first line, last line] counting line breaks only. */
  function logicalLines(text, pos) {
    const chars = [...text];
    return [!chars.slice(0, pos).includes("\n"), !chars.slice(pos).includes("\n")];
  }
  /** [first line, last line] as laid out, wrapping included; null without layout. */
  function visualLines(el, text, pos) {
    try {
      if (el._rich) {
        const s = document.getSelection();
        if (!s || !s.rangeCount) return null;
        const r = s.getRangeAt(0);
        let caret = r.getBoundingClientRect();
        if (!caret || !(caret.height > 0)) caret = [...r.getClientRects()].find((c) => c.height > 0);
        const all = document.createRange();
        all.selectNodeContents(el);
        const boxes = [...all.getClientRects()].filter((c) => c.height > 0);
        if (!caret || !boxes.length) return null;
        const mid = (caret.top + caret.bottom) / 2;
        return [mid < boxes[0].bottom, mid > boxes[boxes.length - 1].top];
      }
      // A textarea: a hidden copy laid out the same, with marks at the
      // start, the caret and the end.
      const cs = getComputedStyle(el);
      const m = document.createElement("div");
      for (const p of ["fontFamily", "fontSize", "fontWeight", "fontStyle", "letterSpacing", "wordSpacing",
        "lineHeight", "textIndent", "textTransform", "tabSize", "paddingLeft", "paddingRight"]) m.style[p] = cs[p];
      const pad = (parseFloat(cs.paddingLeft) || 0) + (parseFloat(cs.paddingRight) || 0);
      Object.assign(m.style, {
        position: "absolute", visibility: "hidden", left: "-9999px", top: "0", boxSizing: "content-box",
        width: Math.max(0, el.clientWidth - pad) + "px", whiteSpace: "pre-wrap", overflowWrap: "break-word",
      });
      const mark = () => { const b = document.createElement("span"); b.textContent = "\u200b"; return b; };
      const chars = [...text];
      const [a, c, z] = [mark(), mark(), mark()];
      m.append(a, chars.slice(0, pos).join(""), c, chars.slice(pos).join(""), z);
      document.body.appendChild(m);
      const [top, at, end, h] = [a.offsetTop, c.offsetTop, z.offsetTop, c.offsetHeight];
      m.remove();
      if (![top, at, end].every(Number.isFinite) || !(h > 0) || !(el.clientWidth > 0)) return null;
      return [at - top < h / 2, end - at < h / 2];
    } catch {
      return null;
    }
  }
  function trackSelection(el, id) {
    let queued = false;
    const report = () => {
      queued = false;
      if (!el._selev || document.activeElement !== el) return;
      const sel = fieldSelection(el);
      if (!sel) return;
      const last = el._selSent;
      if (last && last[0] === sel[0] && last[1] === sel[1]) return;
      el._selSent = sel;
      send({ t: "select", id, start: sel[0], end: sel[1] });
    };
    el._selCheck = () => {
      if (!el._selev || queued) return;
      queued = true;
      requestAnimationFrame(report);
    };
    // A key reports the selection it acts on first (its `sel`), so the app
    // sees the selection moves before the key.
    el._selFlush = () => { if (queued) report(); };
    for (const t of ["select", "selectionchange", "input", "keyup", "mouseup", "pointerup", "focus"]) {
      el.addEventListener(t, el._selCheck);
    }
  }
  // Browsers that fire `selectionchange` on the document only.
  document.addEventListener("selectionchange", () => {
    const a = document.activeElement;
    if (a && a._selCheck) a._selCheck();
    if (a && a._rectCheck) a._rectCheck();
    // A rich input's chips show they are selected (#115).
    const sel = a && a._rich ? richSelection(a) : null;
    if (sel) richMarkChips(a, sel[0], sel[1]);
  });
  /** The `selev` op: whether field `el` reports its selection. */
  function setSelectEvents(el, on) {
    el._selev = !!on;
    if (!on) delete el._selSent;
    else if (document.activeElement === el && el._selCheck) el._selCheck();
  }
  /** The `sel` op: the app's selection for field `el`, in characters. */
  function setFieldSelection(el, start, end) {
    // Typing in flight: the selection was computed for a text the field
    // has moved past since. Its echo (or the app's answer) comes next.
    if (el._sent && el._sent.length) return;
    if (el._composing && document.activeElement === el) { el._deferredSel = [start, end]; return; }
    applyFieldSelection(el, start, end);
  }
  function applyFieldSelection(el, start, end) {
    if (el._rich) {
      setRichSelection(el, start, end);
      if (el._selCheck) el._selCheck();
      return;
    }
    const v = el.value || "";
    try { el.setSelectionRange(toUnits(v, start), toUnits(v, Math.max(start, end))); } catch { /* no caret (email, number) */ }
    // Reported back (an echo the server recognises) if it moved.
    if (el._selCheck) el._selCheck();
  }

  // --- Other people's carets (#240) --------------------------------------------
  //
  // A layer next to the field (a sibling: an input holds no children, and a
  // rich input's children are its text), laid over it, never hit nor read
  // by assistive technologies. Each caret is a thin bar in its color with
  // a name tag, its selection tinted boxes. Offsets are characters, placed
  // with the field's own layout: ranges of a rich input's DOM, a hidden
  // copy of a textarea or input. Redrawn after patches, typing, scrolling
  // and resizing; the field's value, caret, selection and composition are
  // never touched.
  const caretFields = new Set();
  let caretsQueued = false;
  const caretObserver = typeof ResizeObserver === "function" ? new ResizeObserver(() => queueCarets()) : null;
  function queueCarets() {
    if (caretsQueued || !caretFields.size) return;
    caretsQueued = true;
    // Twice: an input scrolls to its caret after the frame that typed.
    requestAnimationFrame(() => {
      caretsQueued = false;
      caretFields.forEach(drawCarets);
      requestAnimationFrame(() => caretFields.forEach(drawCarets));
    });
  }
  addEventListener("resize", queueCarets);
  function dropCarets(el) {
    caretFields.delete(el);
    caretObserver?.unobserve(el);
    el._caretLayer?.remove();
    el._caretLayer = null;
  }
  function setRemoteCarets(el, carets) {
    el._carets = carets && carets.length ? carets : null;
    if (!el._carets) return dropCarets(el);
    if (!caretFields.has(el)) {
      caretFields.add(el);
      caretObserver?.observe(el);
    }
    if (!el._caretBound) {
      el._caretBound = true;
      for (const t of ["input", "beforeinput", "scroll", "compositionupdate", "keyup", "pointerup", "blur"]) el.addEventListener(t, queueCarets);
    }
    drawCarets(el);
  }
  function drawCarets(el) {
    if (nodes.get(Number(el.dataset.pid)) !== el) return dropCarets(el);
    let layer = el._caretLayer;
    if (!layer) {
      layer = el._caretLayer = document.createElement("div");
      layer.className = "prism-carets";
      layer.setAttribute("aria-hidden", "true");
    }
    // Right after the field, wherever it moved.
    if (el.parentNode && (layer.parentNode !== el.parentNode || layer.previousSibling !== el)) {
      el.parentNode.insertBefore(layer, el.nextSibling);
    }
    // One group per caret, kept across updates: a label fades, it doesn't blink.
    while (layer.childNodes.length > el._carets.length) layer.lastChild.remove();
    el._carets.forEach((c, i) => {
      let g = layer.childNodes[i];
      if (!g) {
        g = document.createElement("div");
        g.className = "prism-rcaret";
        g.innerHTML = '<div class="prism-rsel"></div><div class="prism-rbar"><span class="prism-rlabel"></span></div>';
        layer.appendChild(g);
      }
      g.style.setProperty("--prism-rc", c.color);
      const label = g.lastChild.firstChild;
      label.textContent = c.label || "";
      // The label's text (#267): white unless the app says.
      label.style.color = c.text || "";
      label.hidden = !c.label;
      if (c.idle) g.dataset.idle = ""; else delete g.dataset.idle;
    });
    placeCarets(el, layer);
  }
  /** Lays the layer over the field and its carets over its text. */
  function placeCarets(el, layer) {
    try {
      const op = layer.offsetParent;
      if (!op || !el.offsetWidth) return;
      const r = el.getBoundingClientRect(), o = op.getBoundingClientRect();
      Object.assign(layer.style, {
        left: `${r.left - o.left - op.clientLeft + op.scrollLeft}px`,
        top: `${r.top - o.top - op.clientTop + op.scrollTop}px`,
        width: `${r.width}px`, height: `${r.height}px`,
      });
      // What shows of the text: the padding box.
      const clip = { x: el.clientLeft, y: el.clientTop, w: el.clientWidth, h: el.clientHeight };
      const geo = el._rich ? richGeometry(el, r) : mirrorGeometry(el, r);
      el._carets.forEach((c, i) => {
        const g = layer.childNodes[i];
        if (!g) return;
        const { bar, boxes } = geo(Math.min(c.anchor, c.head), Math.max(c.anchor, c.head), c.head);
        const [sel, barEl] = [g.firstChild, g.lastChild];
        const shown = boxes.map((b) => {
          const x = Math.max(b.x, clip.x), y = Math.max(b.y, clip.y);
          return { x, y, w: Math.min(b.x + b.w, clip.x + clip.w) - x, h: Math.min(b.y + b.h, clip.y + clip.h) - y };
        }).filter((b) => b.w > 0 && b.h > 0);
        while (sel.childNodes.length > shown.length) sel.lastChild.remove();
        shown.forEach((b, j) => {
          const d = sel.childNodes[j] || sel.appendChild(document.createElement("div"));
          Object.assign(d.style, { left: `${b.x}px`, top: `${b.y}px`, width: `${b.w}px`, height: `${b.h}px` });
        });
        const visible = bar && bar.x >= clip.x - 1 && bar.x <= clip.x + clip.w + 1
          && bar.y + bar.h > clip.y && bar.y < clip.y + clip.h;
        barEl.style.display = visible ? "" : "none";
        if (visible) Object.assign(barEl.style, { left: `${bar.x}px`, top: `${bar.y}px`, height: `${bar.h}px` });
      });
    } catch { /* no layout yet */ }
  }
  const relBox = (b, r) => ({ x: b.left - r.left, y: b.top - r.top, w: b.width, h: b.height });
  /** A rich input's carets from ranges of its DOM, relative to its box `r`. */
  function richGeometry(el, r) {
    const len = clen(richText(el._model));
    const range = (a, b) => {
      const g = document.createRange();
      const [sn, so] = richPoint(el, Math.min(a, len), true);
      g.setStart(sn, so);
      if (b > a) { const [en, eo] = richPoint(el, Math.min(b, len), true); g.setEnd(en, eo); } else g.collapse(true);
      return g;
    };
    const rects = (g) => [...g.getClientRects()].filter((b) => b.height > 0);
    return (start, end, head) => {
      const boxes = end > start ? rects(range(start, end)).filter((b) => b.width > 0).map((b) => relBox(b, r)) : [];
      head = Math.min(head, len);
      let b = rects(range(head, head))[0], x = b && b.left;
      // At an edge the browser has no box for: beside the next or the previous character.
      if (!b && head < len) { b = rects(range(head, head + 1))[0]; x = b && b.left; }
      if (!b && head > 0) { const all = rects(range(head - 1, head)); b = all[all.length - 1]; x = b && b.right; }
      if (!b) {
        const cs = getComputedStyle(el);
        const h = parseFloat(cs.lineHeight) || (parseFloat(cs.fontSize) || 16) * 1.2;
        return { bar: { x: el.clientLeft + (parseFloat(cs.paddingLeft) || 0), y: el.clientTop + (parseFloat(cs.paddingTop) || 0), h }, boxes };
      }
      return { bar: { x: x - r.left, y: b.top - r.top, h: b.height }, boxes };
    };
  }
  /**
   * A textarea's or input's carets from a hidden copy laid out like it, over
   * it (its box `r`), with the field's scrolling taken off.
   */
  function mirrorGeometry(el, r) {
    const cs = getComputedStyle(el);
    const input = el.tagName === "INPUT";
    const chars = [...(el.value || "")];
    return (start, end, head) => {
      const m = document.createElement("div");
      for (const p of ["fontFamily", "fontSize", "fontWeight", "fontStyle", "letterSpacing", "wordSpacing", "lineHeight",
        "textIndent", "textTransform", "tabSize", "paddingLeft", "paddingRight", "paddingTop", "paddingBottom",
        "borderLeftWidth", "borderRightWidth", "borderTopWidth", "borderBottomWidth", "direction", "textAlign"]) m.style[p] = cs[p];
      Object.assign(m.style, {
        position: "fixed", visibility: "hidden", pointerEvents: "none", left: `${r.left}px`, top: `${r.top}px`,
        width: `${el.clientWidth + el.clientLeft * 2}px`, boxSizing: "border-box", borderStyle: "solid", borderColor: "transparent",
        whiteSpace: input ? "pre" : "pre-wrap", overflowWrap: input ? "normal" : "break-word",
      });
      const mark = document.createElement("span");
      mark.textContent = "​";
      const sel = document.createElement("span");
      sel.textContent = chars.slice(start, end).join("");
      const before = chars.slice(0, start).join(""), after = chars.slice(end).join("");
      if (head <= start) m.append(before, mark, sel, after, "​"); else m.append(before, sel, mark, after, "​");
      document.body.appendChild(m);
      const mr = m.getBoundingClientRect();
      const k = mark.getBoundingClientRect();
      const boxes = end > start ? [...sel.getClientRects()].filter((b) => b.width > 0 && b.height > 0) : [];
      m.remove();
      // An input centers its line; both scroll their text.
      const pad = (parseFloat(cs.paddingTop) || 0) + (parseFloat(cs.paddingBottom) || 0);
      const dy = input ? Math.max(0, (el.clientHeight - pad - k.height) / 2) : 0;
      const at = (b) => ({ x: b.left - mr.left - el.scrollLeft, y: b.top - mr.top - el.scrollTop + dy, w: b.width, h: b.height });
      const bar = at(k);
      return { bar: { x: bar.x, y: bar.y, h: bar.h }, boxes: boxes.map(at) };
    };
  }

  // --- Rich inputs (#115): an editable paragraph of runs and chips --------------
  //
  // A `contenteditable` drawn from a model the client owns: runs
  // `{ text, f, a, l }` (`f` the run's encoded fields without its link id,
  // `a` them decoded, `l` its link id). Every run is a `<span>` holding one
  // text node; a chip (`k=`, one U+FFFC of the value) is a non-editable
  // `<span class="prism-chip">`, so the caret, the selection and Backspace
  // treat it as one character. `beforeinput` edits the model (typing takes
  // the style of the run before the caret, except after a chip or a link,
  // like `prism_core::spans::typing_style`; pastes and drops are plain
  // text), then the DOM is redrawn and the runs sent (`input` with `runs`).
  // An IME composition is left to the browser; the DOM is read back into
  // runs when it ends. The app's runs (`rich` op) follow #73's rule: runs
  // equal to ones the user typed are echoes, others are applied (after a
  // composition), the caret keeping its distance from the end.

  const OBJ = "￼";
  const clen = (s) => { let n = 0; for (const _ of s) n++; return n; };
  /** The runs of `config`, each with its own fields and link id apart. */
  function richRuns(config) {
    if (!config) return [];
    return config.split("\u001e").map((r) => {
      const [text, ...fields] = r.split("\u001f");
      const own = fields.filter((f) => !f.startsWith("l="));
      const link = fields.find((f) => f.startsWith("l="));
      const a = {};
      for (const f of own) {
        const i = f.indexOf("=");
        a[i < 0 ? f : f.slice(0, i)] = i < 0 ? true : f.slice(i + 1);
      }
      const run = { text, f: own.join("\u001f"), a, l: link ? link.slice(2) : undefined };
      if (run.a.k || run.a.g) run.text = OBJ;
      return run;
    });
  }
  /** An inline object: a chip, or an icon (#221). One U+FFFC each. */
  const isChip = (r) => !!(r.a.k || r.a.g);
  /** Runs without empty ones, equal neighbours merged (`normalize_spans`). */
  function richNormalize(runs) {
    const out = [];
    for (const r of runs) {
      if (!r.text) continue;
      const last = out[out.length - 1];
      if (last && !isChip(last) && !isChip(r) && last.f === r.f && last.l === r.l) {
        out[out.length - 1] = { ...last, text: last.text + r.text };
      } else out.push({ ...r });
    }
    return out;
  }
  /** The runs as sent and compared: text then own fields, no link ids. */
  const richEncode = (runs) => richNormalize(runs).map((r) => (r.f ? `${r.text}\u001f${r.f}` : r.text)).join("\u001e");
  const richText = (runs) => runs.map((r) => r.text).join("");
  /** `runs` split at character `at`. */
  function richSplit(runs, at) {
    const before = [], after = [];
    let pos = 0;
    for (const r of runs) {
      const len = clen(r.text);
      if (pos + len <= at) before.push(r);
      else if (pos >= at) after.push(r);
      else {
        const cut = toUnits(r.text, at - pos);
        before.push({ ...r, text: r.text.slice(0, cut) });
        after.push({ ...r, text: r.text.slice(cut) });
      }
      pos += len;
    }
    return [before, after];
  }
  const continues = (r) => !!r && !isChip(r) && r.l === undefined && !r.a.h;
  /** The run whose style text typed at `at` (over `replaced` characters) takes. */
  function typingStyle(runs, at, replaced) {
    let left = null, right = null, pos = 0;
    for (const r of runs) {
      const len = clen(r.text);
      if (pos < at && at <= pos + len) left = r;
      if (!right && pos <= at && at < pos + len) right = r;
      pos += len;
    }
    let pick = null;
    if (left && left === right && !isChip(left)) pick = left;
    else if (replaced > 0 && continues(right)) pick = right;
    else if (continues(left)) pick = left;
    else if (continues(right)) pick = right;
    return pick ? { f: pick.f, a: pick.a, l: pick.l } : { f: "", a: {}, l: undefined };
  }

  function richSetup(el, id) {
    el.className = "prism-rich";
    el.setAttribute("contenteditable", "true");
    el.setAttribute("role", "textbox");
    el.setAttribute("aria-multiline", "true");
    el._rich = true;
    el._model = [];
    el._undo = [];
    el._redo = [];
    richDraw(el);
    el.addEventListener("beforeinput", (e) => richBeforeInput(el, id, e));
    el.addEventListener("input", (e) => {
      // The browser changed the DOM itself (a composition, an input type
      // not handled above): read it back once nothing is being composed.
      if (!el._composing && !e.isComposing) richReadBack(el, id);
    });
    el.addEventListener("keydown", (e) => {
      if (e.key !== "Enter" || e.isComposing || el._composing) return;
      e.preventDefault();
      // Shift+Enter: a line break; Enter: the app's (submit, split…).
      if (e.shiftKey) richEdit(el, id, ...richSelected(el), "\n");
      else send({ t: "submit", id });
    });
    el.addEventListener("compositionstart", () => { el._composing = true; });
    el.addEventListener("compositionend", () => {
      el._composing = false;
      richReadBack(el, id);
      const runs = el._deferred, sel = el._deferredSel;
      delete el._deferred;
      delete el._deferredSel;
      if (runs !== undefined) richApply(el, runs);
      if (sel) applyFieldSelection(el, sel[0], sel[1]);
    });
    // Copy and cut as plain text (a chip as its label, an icon its glyph).
    // With the app's `on_copy` / `on_cut` (#217) the plain text is set at
    // once and the app is asked: its content follows (`clip` op).
    const copy = (e, cut) => {
      const sel = richSelection(el);
      if (!sel || sel[0] === sel[1] || !e.clipboardData) return;
      e.preventDefault();
      const [, rest] = richSplit(el._model, sel[0]);
      const [mid] = richSplit(rest, sel[1] - sel[0]);
      const text = mid.map((r) => (r.a.k ? r.a.n || "" : r.a.g ? glyphs[r.a.g] || "" : r.text)).join("");
      e.clipboardData.setData("text/plain", text);
      if ((el._editMask || 0) & (cut ? EDIT_CUT : EDIT_COPY)) send({ t: "copy", id, cut, start: sel[0], end: sel[1], text });
      if (cut) richEdit(el, id, sel[0], sel[1], "");
    };
    el.addEventListener("copy", (e) => copy(e, false));
    el.addEventListener("cut", (e) => copy(e, true));
    el.addEventListener("paste", (e) => {
      e.preventDefault();
      const text = e.clipboardData ? e.clipboardData.getData("text/plain") : "";
      // The app's `on_paste` (#217): it gets the clipboard, and the text
      // only lands if it leaves it to the field (`instext` op).
      if ((el._editMask || 0) & EDIT_PASTE) {
        const [start, end] = richSelected(el);
        send({ t: "paste", id, start, end, ...pasteData(e.clipboardData, text) });
        return;
      }
      richEdit(el, id, ...richSelected(el), text);
    });
    // A clickable chip reports its click; a link, a click with ⌘ or Ctrl.
    // With the app's `on_span_click` (#217), a click with ⌘, Ctrl or ⌥ on
    // any run is the app's instead.
    el.addEventListener("click", (e) => {
      // Every click, with the character under it (#266); the field still
      // places its caret.
      if ((el._editMask || 0) & EDIT_CLICK) {
        const at = richClickOffset(el, e);
        if (at !== null) send({ t: "fieldclick", id, at, mods: modsOf(e), n: Math.max(1, e.detail || 1), ...local(el, e) });
      }
      if (((el._editMask || 0) & EDIT_SPAN_CLICK) && (e.metaKey || e.ctrlKey || e.altKey)) {
        const at = richClickOffset(el, e);
        if (at !== null) {
          e.preventDefault();
          send({ t: "spanclick", id, at, mods: modsOf(e) });
          return;
        }
      }
      const hit = e.target && e.target.closest && e.target.closest("[data-link]");
      if (!hit || !el.contains(hit)) return;
      if (!hit._chip && !hit.classList.contains("prism-chip") && !(e.metaKey || e.ctrlKey)) return;
      activateRun(e);
    });
    trackSelectionRect(el, id);
    // A selection the app set while the field was not focused lands when
    // the app focuses it; a click puts the caret where the user clicked.
    el.addEventListener("pointerdown", () => { delete el._pendingSel; });
    trackSelection(el, id);
  }

  // --- A rich input's editing hooks (#217) ------------------------------------
  // `editev` op bits: the selection's rectangle, copy, cut, paste,
  // modifier-clicks (prism_core::edit_events).
  const EDIT_RECT = 1, EDIT_COPY = 2, EDIT_CUT = 4, EDIT_PASTE = 8, EDIT_SPAN_CLICK = 16, EDIT_CLICK = 32;
  /** What a `paste` event's clipboard holds: text, HTML, types, other text data. */
  function pasteData(data, text) {
    const types = [], extra = [];
    let html = null;
    if (data) {
      for (const t of data.types || []) if (t !== "Files" && !types.includes(t)) types.push(t);
      for (const item of data.items || []) if (item.kind === "file" && item.type && !types.includes(item.type)) types.push(item.type);
      html = data.getData("text/html") || null;
      for (const t of types) {
        if (t === "text/plain" || t === "text/html" || !(t.startsWith("text/") || t.startsWith("application/"))) continue;
        const v = data.getData(t);
        if (v) extra.push([t, v]);
      }
    }
    if (text && !types.includes("text/plain")) types.unshift("text/plain");
    return { text, html, types, data: extra };
  }
  /** The `clip` op: what a copy writes, every type as one item. */
  function writeClipContent(items) {
    const text = (items.find(([t]) => t === "text/plain") || [, ""])[1];
    const cb = navigator.clipboard;
    if (!cb) return;
    const plain = () => (cb.writeText ? cb.writeText(text).catch(() => {}) : null);
    if (!cb.write || typeof ClipboardItem === "undefined" || items.length < 2) return plain();
    const supports = (t) => (typeof ClipboardItem.supports === "function" ? ClipboardItem.supports(t) : t === "text/plain" || t === "text/html");
    const parts = {};
    for (const [t, v] of items) {
      // Types the browser has no format for go as custom ones (`web …`).
      const name = supports(t) ? t : supports(`web ${t}`) ? `web ${t}` : null;
      if (name) parts[name] = new Blob([v], { type: t });
    }
    return cb.write([new ClipboardItem(parts)]).catch(plain);
  }
  /** The `instext` op: a paste the app left to the field. */
  function insertText(el, id, text) {
    if (el._rich) return richEdit(el, id, ...richSelected(el), text);
    if (typeof el.setRangeText === "function") {
      el.setRangeText(text, el.selectionStart, el.selectionEnd, "end");
      el.dispatchEvent(new Event("input", { bubbles: true }));
    }
  }
  /** The character a click on field `el` landed on, `null` outside its runs. */
  function richClickOffset(el, e) {
    let child = e.target;
    while (child && child.parentNode !== el) child = child.parentNode;
    if (!child || child._tail) return null;
    let n = 0;
    for (const c of el.childNodes) { if (c === child) break; n += richLen(c); }
    const len = richLen(child);
    if (!len) return null;
    if (child._chip) return n;
    // The caret the click puts down, kept inside the run clicked.
    let at = null;
    if (document.caretPositionFromPoint) {
      const p = document.caretPositionFromPoint(e.clientX, e.clientY);
      if (p) at = richOffset(el, p.offsetNode, p.offset);
    } else if (document.caretRangeFromPoint) {
      const r = document.caretRangeFromPoint(e.clientX, e.clientY);
      if (r) at = richOffset(el, r.startContainer, r.startOffset);
    }
    if (at === null) at = n;
    return Math.max(n, Math.min(n + len - 1, at));
  }
  /** Reports where field `el`'s caret or selection is (`selrect`), once per frame. */
  function trackSelectionRect(el, id) {
    let queued = false;
    const report = () => {
      queued = false;
      if (!((el._editMask || 0) & EDIT_RECT) || document.activeElement !== el) return;
      const sel = richSelection(el);
      const s = document.getSelection && document.getSelection();
      if (!sel || !s || !s.rangeCount) return;
      const range = s.getRangeAt(0);
      let r = range.getBoundingClientRect();
      if (!r.width && !r.height) {
        // A collapsed caret between two runs (or in an empty field) has
        // no box: the first of its client rects, else the run beside it.
        const rects = range.getClientRects();
        if (rects.length) r = rects[0];
        else {
          const [node] = richPoint(el, sel[0]);
          const box = (node.nodeType === 3 ? node.parentNode : node);
          const b = box && box !== el ? box.getBoundingClientRect() : null;
          const lh = parseFloat(getComputedStyle(el).lineHeight) || parseFloat(getComputedStyle(el).fontSize) * 1.2 || 16;
          const er = el.getBoundingClientRect();
          r = b ? { left: b.right, top: b.top, width: 0, height: b.height } : { left: er.left + (parseFloat(getComputedStyle(el).paddingLeft) || 0), top: er.top + (parseFloat(getComputedStyle(el).paddingTop) || 0), width: 0, height: lh };
        }
      }
      const er = el.getBoundingClientRect();
      const msg = { t: "selrect", id, start: sel[0], end: sel[1], x: r.left - er.left, y: r.top - er.top, w: sel[0] === sel[1] ? Math.max(1, r.width) : r.width, h: r.height };
      const key = JSON.stringify(msg);
      if (key === el._rectSent) return;
      el._rectSent = key;
      send(msg);
    };
    el._rectCheck = () => {
      if (!((el._editMask || 0) & EDIT_RECT) || queued) return;
      queued = true;
      requestAnimationFrame(report);
    };
    for (const t of ["input", "focus", "keyup", "pointerup"]) el.addEventListener(t, el._rectCheck);
    // The text wraps again: the rectangle moves.
    if (typeof ResizeObserver !== "undefined") new ResizeObserver(() => el._rectCheck()).observe(el);
  }

  /** The selection to edit: the current one, `[0, 0]` without one. */
  function richSelected(el) {
    return richSelection(el) || [0, 0];
  }

  /** `beforeinput`: the edits the client makes itself. */
  function richBeforeInput(el, id, e) {
    const type = e.inputType || "";
    if (e.isComposing || el._composing || type.includes("Composition")) return;
    // Formatting is the app's (⌘B is a key it handles); no browser styles.
    if (type.startsWith("format")) { e.preventDefault(); return; }
    if (type === "historyUndo" || type === "historyRedo") {
      e.preventDefault();
      richHistory(el, id, type === "historyUndo");
      return;
    }
    if (type === "insertParagraph") { e.preventDefault(); send({ t: "submit", id }); return; }
    let text = null;
    if (type === "insertText" || type === "insertReplacementText" || type === "insertFromPaste"
      || type === "insertFromDrop" || type === "insertFromYank") {
      text = e.data != null ? e.data : (e.dataTransfer ? e.dataTransfer.getData("text/plain") : "");
    } else if (type === "insertLineBreak") {
      text = "\n";
    } else if (type.startsWith("delete")) {
      text = "";
    } else {
      return; // Read back on `input`.
    }
    e.preventDefault();
    let [start, end] = richSelected(el);
    const target = e.getTargetRanges ? e.getTargetRanges()[0] : null;
    if (target) {
      const s = richOffset(el, target.startContainer, target.startOffset);
      const t = richOffset(el, target.endContainer, target.endOffset);
      if (s !== null && t !== null) [start, end] = [Math.min(s, t), Math.max(s, t)];
    }
    if (text === "" && start === end) {
      // A caret: one character (a chip is one).
      if (type.includes("Backward")) start = Math.max(0, start - 1);
      else end = Math.min(clen(richText(el._model)), end + 1);
    }
    richEdit(el, id, start, end, text, type);
  }

  /** Replaces characters `start..end` with `text` (plain), caret after it. */
  function richEdit(el, id, start, end, text, type) {
    text = (text || "").replace(/\r\n?/g, "\n").split(OBJ).join("");
    const model = el._model;
    const style = typingStyle(model, start, end - start);
    const [before, rest] = richSplit(model, start);
    const [, after] = richSplit(rest, end - start);
    const next = richNormalize([...before, ...(text ? [{ ...style, text }] : []), ...after]);
    if (richEncode(next) === richEncode(model)) { setRichSelection(el, start + clen(text), start + clen(text)); return; }
    // Undo: typing coalesces into one step until a space or another kind of edit.
    const coalesce = type === "insertText" && el._lastEdit === "insertText" && text !== " ";
    if (!coalesce) { el._undo.push({ model, sel: [start, end] }); if (el._undo.length > 100) el._undo.shift(); }
    el._redo.length = 0;
    el._lastEdit = type;
    el._model = next;
    richDraw(el);
    const caret = start + clen(text);
    setRichSelection(el, caret, caret);
    richSend(el, id);
    if (el._selCheck) el._selCheck();
  }

  function richHistory(el, id, undo) {
    const from = undo ? el._undo : el._redo, to = undo ? el._redo : el._undo;
    const step = from.pop();
    if (!step) return;
    const sel = richSelected(el);
    to.push({ model: el._model, sel });
    el._lastEdit = null;
    el._model = step.model;
    richDraw(el);
    setRichSelection(el, step.sel[0], step.sel[1]);
    richSend(el, id);
    if (el._selCheck) el._selCheck();
  }

  /** Sends the runs and remembers them (an echo of them is no change). */
  function richSend(el, id) {
    const runs = richEncode(el._model);
    const sent = el._sent || (el._sent = []);
    sent.push(runs);
    if (sent.length > 64) sent.shift();
    send({ t: "input", id, v: richText(el._model), runs });
  }

  /** The DOM's runs (after the browser edited it), sent if they changed. */
  function richReadBack(el, id) {
    const sel = richSelection(el);
    const out = [];
    const textOf = (n) => (n.nodeType === 3 ? n.data : n.localName === "br" ? "\n"
      : (["div", "p"].includes(n.localName) && n.previousSibling ? "\n" : "") + [...n.childNodes].map(textOf).join(""));
    for (const c of [...el.childNodes]) {
      if (c._tail) continue;
      if (c._chip) { out.push(c._run); continue; }
      const text = textOf(c).split(OBJ).join("");
      if (c._run) out.push({ ...c._run, text });
      else if (text) out.push({ f: "", a: {}, l: undefined, text });
    }
    const next = richNormalize(out);
    const changed = richEncode(next) !== richEncode(el._model);
    if (changed) {
      el._undo.push({ model: el._model, sel: sel || [0, 0] });
      el._redo.length = 0;
      el._lastEdit = null;
    }
    el._model = next;
    richDraw(el);
    if (sel) setRichSelection(el, sel[0], sel[1]);
    if (changed) richSend(el, id);
  }

  /** The `rich` op: the app's runs. */
  function setRichValue(el, runs) {
    const sent = el._sent;
    const canon = richEncode(richRuns(runs));
    const echo = sent ? sent.indexOf(canon) : -1;
    if (echo >= 0) {
      sent.splice(0, echo + 1);
      // The latest: what the field shows; take the new link ids.
      if (!sent.length && canon === richEncode(el._model)) {
        el._model = richNormalize(richRuns(runs));
        richDraw(el);
      }
      return;
    }
    if (el._composing && document.activeElement === el) { el._deferred = runs; return; }
    delete el._deferred;
    richApply(el, runs);
  }

  /** Shows the app's `runs`, keeping the caret's distance from the end. */
  function richApply(el, runs) {
    if (el._sent) el._sent.length = 0;
    const next = richNormalize(richRuns(runs));
    const focused = document.activeElement === el;
    const sel = focused ? richSelection(el) : null;
    const oldLen = clen(richText(el._model));
    el._model = next;
    el._undo.length = 0;
    el._redo.length = 0;
    richDraw(el);
    if (sel) {
      const caret = Math.max(0, clen(richText(next)) - (oldLen - sel[1]));
      setRichSelection(el, caret, caret);
    }
  }

  /** Draws the model, reusing the run elements that did not change. */
  function richDraw(el) {
    const old = el._runEls || [];
    const els = el._model.map((r, i) => {
      const prev = old[i];
      // The same style (a new link id at most): the node stays, so the
      // caret and an IME composition in it do too.
      if (!prev || prev._run.f !== r.f || !!prev._chip !== isChip(r)) return richNode(r);
      if (!isChip(r)) {
        const t = prev.firstChild && prev.firstChild.nodeType === 3 ? prev.firstChild : null;
        if (!t || prev.childNodes.length !== 1) prev.replaceChildren(document.createTextNode(r.text));
        else if (t.data !== r.text) t.data = r.text;
      }
      if (r.l !== undefined) prev.dataset.link = r.l; else delete prev.dataset.link;
      const link = !isChip(r) && (r.l !== undefined || !!r.a.h);
      if (!isChip(r) && prev.classList.contains("prism-link") !== link) toggleClass(prev, "prism-link", link);
      prev._run = r;
      return prev;
    });
    // A last line break needs a <br> to show its empty line; so does an
    // empty field (one line high, with its placeholder).
    const text = richText(el._model);
    if (!el._tail) { el._tail = document.createElement("br"); el._tail._tail = true; }
    const want = [...els, ...(!text || text.endsWith("\n") ? [el._tail] : [])];
    const same = want.length === el.childNodes.length && want.every((n, i) => el.childNodes[i] === n);
    if (!same) el.replaceChildren(...want);
    el._runEls = els;
    el._runPs = els.map((n) => n._ps).filter(Boolean);
    if (text) delete el.dataset.empty; else el.dataset.empty = "";
  }

  function richNode(r) {
    const node = document.createElement("span");
    node._run = r;
    if (r.l !== undefined) node.dataset.link = r.l;
    if (r.a.g && !r.a.k) {
      // An inline icon (#221): one character, its glyph.
      node._chip = true;
      node.className = "prism-symbol";
      node.setAttribute("contenteditable", "false");
      node.setAttribute("aria-hidden", "true");
      node.appendChild(document.createTextNode(glyphs[r.a.g] || ""));
    } else if (isChip(r)) {
      node._chip = true;
      node.className = "prism-chip";
      node.setAttribute("contenteditable", "false");
      if (r.a.m) {
        const g = document.createElement("span");
        g.className = "prism-symbol";
        g.setAttribute("aria-hidden", "true");
        g.textContent = glyphs[r.a.m] || "";
        node.appendChild(g);
      }
      node.appendChild(document.createTextNode(r.a.n || ""));
    } else {
      if (r.l !== undefined || r.a.h) node.className = "prism-link";
      node.appendChild(document.createTextNode(r.text));
    }
    setCss(node, runCss(r.a));
    return node;
  }

  /** How many characters of the value DOM node `c` (a child of the field) holds. */
  const richLen = (c) => (c._tail ? 0 : c._chip ? 1 : clen(c.nodeType === 3 ? c.data : c.textContent));

  /** The character offset of DOM point (`node`, `offset`) in field `el`, `null` outside it. */
  function richOffset(el, node, offset) {
    if (!node) return null;
    const kids = el.childNodes;
    if (node === el) {
      let n = 0;
      for (let i = 0; i < offset && i < kids.length; i++) n += richLen(kids[i]);
      return n;
    }
    let child = node;
    while (child && child.parentNode !== el) child = child.parentNode;
    if (!child) return null;
    let n = 0;
    for (const c of kids) { if (c === child) break; n += richLen(c); }
    if (child._tail) return n;
    if (child._chip) return n + (node === child && offset === 0 ? 0 : 1);
    if (node.nodeType === 3) {
      // In a run's text, or a text node the browser added.
      let before = 0;
      if (node !== child) for (const t of child.childNodes) { if (t === node) break; before += clen(t.textContent); }
      return n + before + toChars(node.data, offset);
    }
    // An element point: `offset` children in.
    let m = 0;
    for (let i = 0; i < offset && i < node.childNodes.length; i++) m += clen(node.childNodes[i].textContent);
    return n + m;
  }

  /** The text node of run element (or text node) `c`. */
  function textIn(c) {
    if (c.nodeType === 3) return c;
    if (!c.firstChild || c.firstChild.nodeType !== 3) c.prepend(document.createTextNode(""));
    return c.firstChild;
  }
  /**
   * The DOM point of character `at` in field `el`: inside a run's text, or
   * at the edge of two, in the one typing there continues (the run before,
   * unless a chip or a link, then the one after), else between them.
   */
  function richPoint(el, at, readOnly) {
    const kids = [...el.childNodes].filter((c) => !c._tail);
    const cont = (c) => !!c && !c._chip && (c.nodeType === 3 || !c._run || continues(c._run));
    // Read only (other people's carets, #240): never adds a text node, even
    // during a composition; a run without one is pointed at as an element.
    const text = (c) => (!readOnly ? textIn(c) : c.nodeType === 3 ? c : c.firstChild && c.firstChild.nodeType === 3 ? c.firstChild : null);
    const edge = (left, right, i) => {
      if (cont(left)) { const t = text(left); if (t) return [t, t.data.length]; }
      if (cont(right)) { const t = text(right); if (t) return [t, 0]; }
      return [el, i];
    };
    let n = 0;
    for (let i = 0; i < kids.length; i++) {
      const c = kids[i], len = richLen(c);
      if (at === n) return edge(kids[i - 1] || null, c, i);
      if (at < n + len && !c._chip) { const t = text(c); if (t) return [t, toUnits(t.data, at - n)]; }
      n += len;
    }
    return edge(kids[kids.length - 1] || null, null, kids.length);
  }

  /** Field `el`'s selection in characters, `null` when it is elsewhere. */
  function richSelection(el) {
    const s = document.getSelection && document.getSelection();
    if (!s || !s.anchorNode || !el.contains(s.anchorNode) || !el.contains(s.focusNode)) return null;
    const a = richOffset(el, s.anchorNode, s.anchorOffset);
    const f = richOffset(el, s.focusNode, s.focusOffset);
    if (a === null || f === null) return null;
    return [Math.min(a, f), Math.max(a, f)];
  }

  /** Selects characters `start..end` of field `el` (clamped); later if it is not focused. */
  function setRichSelection(el, start, end) {
    const len = clen(richText(el._model));
    start = Math.min(start, len);
    end = Math.min(Math.max(start, end), len);
    if (document.activeElement !== el) { el._pendingSel = [start, end]; return; }
    const s = document.getSelection && document.getSelection();
    if (!s) return;
    const [an, ao] = richPoint(el, start);
    const [fn, fo] = start === end ? [an, ao] : richPoint(el, end);
    s.setBaseAndExtent(an, ao, fn, fo);
    richMarkChips(el, start, end);
  }

  /** Chips inside the selection look selected (`data-selected`). */
  function richMarkChips(el, start, end) {
    let n = 0;
    for (const c of el.childNodes) {
      if (c._chip) {
        if (start < end && n >= start && n < end) c.dataset.selected = ""; else delete c.dataset.selected;
      }
      n += richLen(c);
    }
  }

  // --- One selection across several fields (#231) ------------------------------
  //
  // A selection group (`selgroup`, `data-selgroup`) makes its text fields
  // one selection space. The client extends it itself, as the user acts:
  // ⇧+arrows at a field's edge, then ⇧→/⇧← a character and ⇧↓/⇧↑ a field
  // at a time, and pointer drags from one field into another. The field
  // where it started keeps its own selection; the others draw their part
  // with the CSS Custom Highlight API (`::highlight(prism-across)`, rich
  // inputs) or look selected whole (`data-across`, a text area or input in
  // between). The server hears `across` (and draws nothing back), and
  // takes copies (`acrosscopy`, the plain text already written) and edits
  // (`acrossedit`) of the range.
  let across = null; // { g, a: [field, offset], h: [field, offset] }
  const highlights = new Map(); // field → [start, end]
  const isTextField = (el) => !!el && (el._rich || el.tagName === "TEXTAREA"
    || (el.tagName === "INPUT" && editable(el)));
  function selGroupOf(el) {
    if (!isTextField(el)) return null;
    const g = el.parentElement && el.parentElement.closest("[data-selgroup]");
    return g || null;
  }
  const groupFields = (g) => [...g.querySelectorAll("[data-pid]")].filter((f) => isTextField(f) && selGroupOf(f) === g);
  const fieldLen = (el) => clen(el._rich ? richText(el._model || []) : (el.value || ""));
  function fieldPart(el, start, end) {
    const text = el._rich ? richText(el._model || []).replace(/￼/g, "") : (el.value || "");
    return [...text].slice(start, end).join("");
  }
  const pid = (el) => Number(el.dataset.pid);
  /** Each field's part of anchor..head: [field, start, end], first to last. */
  function acrossParts(fields, a, h) {
    const ia = fields.indexOf(a[0]), ih = fields.indexOf(h[0]);
    if (ia < 0 || ih < 0) return [];
    const [lo, hi] = ia < ih || (ia === ih && a[1] <= h[1]) ? [[ia, a[1]], [ih, h[1]]] : [[ih, h[1]], [ia, a[1]]];
    const out = [];
    for (let i = lo[0]; i <= hi[0]; i++) {
      const len = fieldLen(fields[i]);
      out.push([fields[i], i === lo[0] ? Math.min(lo[1], len) : 0, i === hi[0] ? Math.min(hi[1], len) : len]);
    }
    return out;
  }
  function paintHighlights() {
    const H = globalThis.CSS && CSS.highlights;
    const ranges = [];
    for (const [el, [s, e]] of highlights) {
      if (!el.isConnected) { highlights.delete(el); continue; }
      // A text area or input can't highlight a part: whole, or nothing.
      if (!el._rich) { if (s === 0 && e >= fieldLen(el) && e > 0) el.dataset.across = ""; else delete el.dataset.across; continue; }
      delete el.dataset.across;
      if (s >= e || typeof Range === "undefined") continue;
      try {
        const r = document.createRange();
        const [sn, so] = richPoint(el, s);
        const [en, eo] = richPoint(el, e);
        r.setStart(sn, so);
        r.setEnd(en, eo);
        ranges.push(r);
      } catch { /* a field being redrawn */ }
    }
    for (const el of document.querySelectorAll("[data-across]")) if (!highlights.has(el)) delete el.dataset.across;
    if (H && typeof Highlight !== "undefined") {
      if (ranges.length) H.set("prism-across", new Highlight(...ranges)); else H.delete("prism-across");
    }
  }
  /** The `selhl` op: field `el`'s highlighted part (`start == end`: none). */
  function setHighlight(el, start, end) {
    if (start < end) highlights.set(el, [start, end]); else highlights.delete(el);
    paintHighlights();
  }
  /** Shows anchor..head in group `g` (the same field: given back to it). */
  function drawAcross(g, a, h) {
    const fields = groupFields(g);
    highlights.clear();
    if (a[0] === h[0]) {
      across = null;
      paintHighlights();
      applyFieldSelection(a[0], Math.min(a[1], h[1]), Math.max(a[1], h[1]));
      return;
    }
    across = { g, a, h };
    for (const [f, s, e] of acrossParts(fields, a, h)) {
      if (f === a[0]) applyFieldSelection(f, s, e); else highlights.set(f, [s, e]);
    }
    paintHighlights();
  }
  function reportAcross(g, a, h) {
    send(a ? { t: "across", id: pid(g), a: [pid(a[0]), a[1]], h: [pid(h[0]), h[1]] } : { t: "across", id: pid(g) });
  }
  function endAcross(report) {
    if (!across) return;
    const g = across.g;
    across = null;
    highlights.clear();
    paintHighlights();
    if (report) reportAcross(g, null);
  }
  /** The `across` op: the selection the app (or the runtime) set. */
  function setAcross(g, a, h) {
    const fa = a && nodes.get(a[0]), fh = h && nodes.get(h[0]);
    if (!fa || !fh || fa === fh) { across = null; return; }
    across = { g, a: [fa, a[1]], h: [fh, h[1]] };
  }
  /** Where ⇧-arrow `dir` (-2 ↑, -1 ←, 1 →, 2 ↓) takes `head`. */
  function acrossStep(fields, [f, off], dir) {
    const i = fields.indexOf(f), len = fieldLen(f);
    if (dir === 1) return off < len ? [f, off + 1] : fields[i + 1] ? [fields[i + 1], 0] : [f, off];
    if (dir === -1) return off > 0 ? [f, off - 1] : fields[i - 1] ? [fields[i - 1], fieldLen(fields[i - 1])] : [f, off];
    if (dir === 2) return off < len ? [f, len] : fields[i + 1] ? [fields[i + 1], fieldLen(fields[i + 1])] : [f, off];
    return off > 0 ? [f, 0] : fields[i - 1] ? [fields[i - 1], 0] : [f, off];
  }
  const ARROWS = { ArrowUp: -2, ArrowLeft: -1, ArrowRight: 1, ArrowDown: 2 };
  // Before every other listener (the window's capture phase).
  addEventListener("keydown", (e) => {
    if (e.isComposing || !e.key) return;
    const t = e.target;
    const g = selGroupOf(t);
    if (!g) return;
    const fields = groupFields(g);
    if (!fields.includes(t)) return;
    const dir = ARROWS[e.key];
    const command = e.metaKey || e.ctrlKey;
    const take = () => { e.preventDefault(); e.stopImmediatePropagation(); };
    if (across && across.g === g) {
      const { a, h } = across;
      if (dir !== undefined) {
        if (command || e.altKey) return;
        take();
        if (e.shiftKey) {
          const head = acrossStep(fields, h, dir);
          drawAcross(g, a, head);
          return reportAcross(g, a, head);
        }
        // An arrow ends it at one end: the caret goes there.
        const parts = acrossParts(fields, a, h);
        const [f, s, end] = dir < 0 ? parts[0] : parts[parts.length - 1];
        const at = dir < 0 ? s : end;
        endAcross(false);
        focusNode(f);
        applyFieldSelection(f, at, at);
        return reportAcross(g, [f, at], [f, at]);
      }
      if (e.key === "Escape") {
        take();
        endAcross(false);
        focusNode(h[0]);
        applyFieldSelection(h[0], h[1], h[1]);
        return reportAcross(g, h, h);
      }
      // Copy, cut and paste: their own events, below.
      if (command) return;
      const cause = e.key === "Backspace" ? "backspace" : e.key === "Delete" ? "delete"
        : e.key === "Enter" ? "enter" : e.key.length === 1 && !e.altKey ? "typing" : null;
      if (!cause) return;
      if (!g._acrossEdits) {
        // No `on_edit_across`: back into the field it started in, which acts.
        const part = acrossParts(fields, a, h).find(([f]) => f === a[0]);
        endAcross(false);
        if (part) {
          applyFieldSelection(part[0], part[1], part[2]);
          reportAcross(g, [part[0], part[1]], [part[0], part[2]]);
        }
        return;
      }
      take();
      endAcross(false);
      return send({ t: "acrossedit", id: pid(g), cause, text: cause === "typing" ? e.key : cause === "enter" ? "\n" : "" });
    }
    // A ⇧-arrow at the field's edge starts one.
    if (dir === undefined || !e.shiftKey || command || e.altKey) return;
    const sel = fieldSelection(t);
    if (!sel) return;
    const i = fields.indexOf(t), len = fieldLen(t), edges = caretEdges(t) || "";
    const selected = sel[0] !== sel[1];
    let anchor, head;
    if (dir > 0 && fields[i + 1]) {
      const atEnd = dir === 2 ? edges.includes("l") || (selected && sel[1] >= len) : edges.includes("e") || sel[1] >= len;
      if (!atEnd) return;
      anchor = [t, sel[0]];
      head = dir === 2 ? [fields[i + 1], fieldLen(fields[i + 1])] : [fields[i + 1], 0];
    } else if (dir < 0 && fields[i - 1]) {
      const atStart = dir === -2 ? edges.includes("f") || (selected && sel[0] === 0) : edges.includes("s") || sel[0] === 0;
      if (!atStart) return;
      anchor = [t, sel[1]];
      head = dir === -2 ? [fields[i - 1], 0] : [fields[i - 1], fieldLen(fields[i - 1])];
    } else return;
    take();
    drawAcross(g, anchor, head);
    reportAcross(g, anchor, head);
  }, true);
  // Copy and cut of the whole range: its plain text now, the app's content
  // next (`clip`); a cut's removal is the app's (`on_edit_across`).
  for (const cut of [false, true]) {
    document.addEventListener(cut ? "cut" : "copy", (e) => {
      if (!across || !across.g.contains(e.target)) return;
      const g = across.g;
      const text = acrossParts(groupFields(g), across.a, across.h).map(([f, s, end]) => fieldPart(f, s, end)).join("\n");
      e.preventDefault();
      e.stopImmediatePropagation();
      try { e.clipboardData.setData("text/plain", text); } catch { /* no clipboardData */ }
      send({ t: "acrosscopy", id: pid(g), cut });
      if (cut && g._acrossEdits) endAcross(false);
    }, true);
  }
  document.addEventListener("paste", (e) => {
    if (!across || !across.g.contains(e.target) || !across.g._acrossEdits) return;
    const g = across.g;
    e.preventDefault();
    e.stopImmediatePropagation();
    let text = "";
    try { text = e.clipboardData.getData("text/plain") || ""; } catch { /* none */ }
    endAcross(false);
    send({ t: "acrossedit", id: pid(g), cause: "paste", text });
  }, true);
  // Typing that no key announced (an on-screen keyboard, dictation).
  document.addEventListener("beforeinput", (e) => {
    if (!across || !across.g.contains(e.target) || !across.g._acrossEdits) return;
    const g = across.g;
    const cause = /^delete/.test(e.inputType) ? (/Backward/.test(e.inputType) ? "backspace" : "delete")
      : e.inputType === "insertParagraph" ? "enter" : e.inputType === "insertText" ? "typing" : null;
    if (!cause) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    endAcross(false);
    send({ t: "acrossedit", id: pid(g), cause, text: cause === "typing" ? e.data || "" : cause === "enter" ? "\n" : "" });
  }, true);
  /** The character offset under point (`x`, `y`) in field `el`. */
  function offsetAt(el, x, y) {
    if (el._rich) {
      let node = null, off = 0;
      if (document.caretPositionFromPoint) {
        const p = document.caretPositionFromPoint(x, y);
        if (p) { node = p.offsetNode; off = p.offset; }
      } else if (document.caretRangeFromPoint) {
        const r = document.caretRangeFromPoint(x, y);
        if (r) { node = r.startContainer; off = r.startOffset; }
      }
      const at = node && el.contains(node) ? richOffset(el, node, off) : null;
      if (at !== null) return at;
    }
    // A text area or input (or no caret there): its start above its
    // middle, its end below.
    const r = el.getBoundingClientRect();
    return y < r.top + r.height / 2 ? 0 : fieldLen(el);
  }
  // Pointer drags from one field of a group into another (mouse, pen).
  let acrossDrag = null; // { g, field, anchor }
  document.addEventListener("pointerdown", (e) => {
    const g = selGroupOf(e.target);
    let ended = false;
    if (across && (g !== across.g || e.button === 0)) {
      // A press elsewhere ends it: in the group, where the caret lands is
      // reported on release.
      ended = g === across.g;
      endAcross(!ended);
    }
    if (!g || e.button !== 0 || e.pointerType === "touch") return;
    acrossDrag = { g, field: e.target, anchor: null, backInside: ended };
  }, true);
  document.addEventListener("pointermove", (e) => {
    const d = acrossDrag;
    if (!d || !(e.buttons & 1)) return;
    // Where the drag started, as the browser placed it.
    if (d.anchor === null) {
      const sel = fieldSelection(d.field);
      d.anchor = sel ? sel[0] : offsetAt(d.field, e.clientX, e.clientY);
    }
    const under = document.elementFromPoint && document.elementFromPoint(e.clientX, e.clientY);
    let over = under && (isTextField(under) ? under : under.closest && [...groupFields(d.g)].find((f) => f.contains(under)));
    if (!over || selGroupOf(over) !== d.g) {
      // Between fields: the nearest one above or below.
      const fields = groupFields(d.g);
      over = fields.find((f) => f.getBoundingClientRect().bottom >= e.clientY) || fields[fields.length - 1];
    }
    if (!over) return;
    if (over === d.field) {
      if (across) { endAcross(false); d.backInside = true; }
      return;
    }
    const head = [over, offsetAt(over, e.clientX, e.clientY)];
    const a = [d.field, d.anchor];
    if (across && across.h[0] === head[0] && across.h[1] === head[1]) return;
    drawAcross(d.g, a, head);
    reportAcross(d.g, a, head);
    // The browser's own drag selection stays in the first field.
    e.preventDefault();
  }, true);
  document.addEventListener("pointerup", () => {
    const d = acrossDrag;
    acrossDrag = null;
    if (!d || across || !d.backInside) return;
    const sel = fieldSelection(d.field);
    if (sel) reportAcross(d.g, [d.field, sel[0]], [d.field, sel[1]]);
  }, true);

  /** A text patch for an input or a text area. */
  function setFieldText(el, text) {
    const sent = el._sent;
    const echo = sent ? sent.indexOf(text) : -1;
    // An echo: forget it and what was typed before it.
    if (echo >= 0) { sent.splice(0, echo + 1); return; }
    if (el._composing && document.activeElement === el) { el._deferred = text; return; }
    delete el._deferred;
    applyFieldText(el, text);
  }

  /** Writes the app's `text` into field `el`, keeping its caret. */
  function applyFieldText(el, text) {
    // The typing in flight was answered by the app's own value.
    if (el._sent) el._sent.length = 0;
    if (el.value === text) return;
    if (document.activeElement !== el) { el.value = text; return; }
    let fromEnd = 0;
    try { fromEnd = el.selectionEnd == null ? 0 : el.value.length - el.selectionEnd; } catch { /* no caret (email, number) */ }
    el.value = text;
    const caret = Math.max(0, text.length - fromEnd);
    try { el.setSelectionRange(caret, caret); } catch { /* no caret (email, number) */ }
  }

  /** Selects the picker's `_index`. The placeholder option is only there
   * while nothing is picked (#96): some browsers show a hidden option as a
   * blank row. */
  function showPicked(el) {
    const empty = el._index < 0 || el._index >= el.options.length - (el._ph.parentNode === el ? 1 : 0);
    if (empty && el._ph.parentNode !== el) el.insertBefore(el._ph, el.firstChild);
    if (!empty && el._ph.parentNode === el) el._ph.remove();
    el.selectedIndex = empty ? 0 : el._index;
  }

  function grow(el) {
    // Not in the page yet (just created): measure once it is.
    if (!el.isConnected) return requestAnimationFrame(() => el.isConnected && grow(el));
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight + el.offsetHeight - el.clientHeight}px`;
  }
  const pad = (n) => String(n).padStart(2, "0");
  /** Unix seconds as an `<input type=date|time|datetime-local>` value (local time); `""` when empty. */
  function dateString(secs, type, utc) {
    if (!Number.isFinite(secs)) return "";
    if (utc) {
      // A time of day (#18): seconds since midnight, no time zone.
      const m = Math.floor((((Math.round(secs) % 86400) + 86400) % 86400) / 60);
      return `${pad(Math.floor(m / 60))}:${pad(m % 60)}`;
    }
    const d = new Date(secs * 1000);
    const day = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
    const time = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
    return type === "date" ? day : type === "time" ? time : `${day}T${time}`;
  }
  /** The input's value as Unix seconds, keeping the part it does not show. */
  function dateSeconds(el) {
    const v = el.value;
    if (!v) return null;
    if (el._utc) {
      const [h, mi] = v.split(":").map(Number);
      return h * 3600 + mi * 60;
    }
    // An empty picker (#89) starts from now, to the minute (DateConfig::start).
    const base = Number.isFinite(el._secs) ? el._secs : Math.floor(Date.now() / 60000) * 60;
    const b = new Date(base * 1000);
    let [y, mo, d, h, mi, s] = [b.getFullYear(), b.getMonth(), b.getDate(), b.getHours(), b.getMinutes(), b.getSeconds()];
    if (el.type === "time") {
      [h, mi] = v.split(":").map(Number);
      s = 0;
    } else {
      const [day, time] = v.split("T");
      [y, mo, d] = day.split("-").map(Number);
      mo -= 1;
      if (time) { [h, mi] = time.split(":").map(Number); s = 0; }
    }
    return Math.round(new Date(y, mo, d, h, mi, s).getTime() / 1000);
  }
  /**
   * Shows `el._secs` (NaN: empty). The same text is not written again: a
   * date input being typed in shows "" until every part is filled, and
   * writing "" would erase the parts already typed.
   */
  function showDate(el) {
    const v = dateString(el._secs, el.type, el._utc);
    if (el.value !== v) el.value = v;
  }
  /** Keyboard, return key, capitalization, autocorrection (mirrors html.rs input_attrs). */
  function applyInputConfig(el, config) {
    const lines = config.split("\n");
    const t = new Set(lines[0].split(" ").filter(Boolean));
    const get = (k) => { for (const x of t) if (x.startsWith(k + "=")) return x.slice(k.length + 1); return null; };
    const secure = t.has("secure");
    const kb = get("keyboard");
    if (el.tagName === "INPUT") {
      el.type = secure ? "password" : { email: "email", url: "url", phone: "tel" }[kb] || "text";
      // The app's own `autocomplete` (`.attr`) wins over the default.
      if (secure && !(el._attrs && el._attrs.includes("autocomplete"))) el.setAttribute("autocomplete", "current-password");
    }
    const set = (name, v) => (v ? el.setAttribute(name, v) : el.removeAttribute(name));
    set("inputmode", { number: "numeric", decimal: "decimal" }[kb]);
    set("enterkeyhint", get("return"));
    set("autocapitalize", secure ? "none" : { none: "none", words: "words", all: "characters" }[get("capitalize")]);
    const noCorrect = secure || t.has("autocorrect=off");
    set("autocorrect", noCorrect ? "off" : null);
    el.spellcheck = !noCorrect;
    set("maxlength", get("max"));
    if (el.tagName === "INPUT") setSuggestions(el, lines.slice(1));
    // A text area's minimum lines (#235).
    if (el.tagName === "TEXTAREA") {
      const rows = Number(get("rows")) || 3;
      el.rows = rows;
      if (rows === 3) el.style.removeProperty("--prism-rows"); else el.style.setProperty("--prism-rows", rows);
      grow(el);
    }
  }
  function applyConfig(el, config) {
    const kind = el.dataset.kind;
    // A picture's focal point (#218): `object-position:…;`.
    if (kind === "picture") { el.style.objectPosition = config.replace(/^object-position:|;$/g, ""); return; }
    if (kind === "screen") {
      // Bar buttons and title size (#30): its stack shows them.
      el._barConfig = config;
      const owner = el.parentElement && el.parentElement.parentElement;
      if (owner && owner._screens) refreshStack(owner);
      return;
    }
    if (kind === "slider") {
      const [min, max, step] = config.split(" ").map(Number);
      Object.assign(el, { min, max, step: step > 0 ? step : "any" });
      if (el._v !== undefined) el.value = el._v;
    } else if (kind === "picker") {
      for (const o of [...el.options]) if (o !== el._ph) o.remove();
      (config ? config.split("\n") : []).forEach((text, i) => el.appendChild(new Option(text, String(i))));
      showPicked(el);
    } else if (kind === "date_picker") {
      const [mode, min, max, ...rest] = config.split(" ");
      el._utc = mode === "timeofday";
      // Only an optional picker may be emptied (#89): browsers hide their
      // clear button on required inputs.
      el._optional = rest.includes("optional");
      el.required = !el._optional;
      el.type = { time: "time", timeofday: "time", datetime: "datetime-local" }[mode] || "date";
      const bounded = mode !== "time" && !el._utc;
      el.min = bounded && min !== "-" ? dateString(Number(min), el.type) : "";
      el.max = bounded && max !== "-" ? dateString(Number(max), el.type) : "";
      if (el._secs !== undefined) showDate(el);
    } else if (kind === "text_input" || kind === "text_area" || kind === "rich_input") {
      applyInputConfig(el, config);
    } else if (kind === "text") {
      el._runs = config;
      renderRuns(el);
    } else if (kind === "overlay") {
      overlayBox(el, config);
    } else if (kind === "radio_group") {
      radioOptions(el, Number(el.dataset.pid), config);
    } else if (kind === "web_view") {
      webViewConfig(el, config);
    } else if (kind === "video") {
      // `controls autoplay loop muted poster=<url>` (#207).
      const [flags, poster] = config.split("poster=");
      const has = (t) => flags.split(/\s+/).includes(t);
      el.controls = has("controls");
      el.autoplay = has("autoplay");
      el.loop = has("loop");
      el.muted = has("muted");
      if (poster && poster.trim()) el.poster = poster.trim(); else el.removeAttribute("poster");
    }
  }

  /** App-defined attributes (`.attr`, #85): `pairs` is the full list, so
   * names set before and absent now are removed. The server only sends
   * `data-*` names Prism does not use and a safe list; this checks again. */
  const APP_ATTR = /^(data-[a-z0-9_.-]+|autocomplete|translate)$/;
  function setAttrs(el, pairs) {
    if (!el) return;
    const names = pairs.map(([n]) => n).filter((n) => APP_ATTR.test(n) && !n.startsWith("data-prism"));
    for (const n of el._attrs || []) {
      if (names.includes(n)) continue;
      el.removeAttribute(n);
      if (n === "autocomplete" && el.type === "password") el.setAttribute("autocomplete", "current-password");
    }
    for (const [n, v] of pairs) if (names.includes(n)) el.setAttribute(n, v);
    el._attrs = names;
  }

  // Tooltips (#216): the browser's own (`title`), or the full text, only
  // while it is cut short (checked as the pointer comes over it).
  function applyTip(el, a) {
    if (a.tip != null || el._tip != null) {
      el._tip = a.tip ?? null;
      if (typeof a.tip === "string") el.setAttribute("title", a.tip); else el.removeAttribute("title");
      if (a.tip === true && !el._tipBound) {
        el._tipBound = true;
        el.addEventListener("mouseenter", () => {
          if (el._tip !== true) return;
          const cut = el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1;
          if (cut) el.setAttribute("title", el.textContent); else el.removeAttribute("title");
        });
      }
    }
  }
  function applyA11y(el, a) {
    const kind = el.dataset.kind;
    const set = (name, v) =>
      v === undefined || v === null || v === false ? el.removeAttribute(name) : el.setAttribute(name, v === true ? "true" : v);
    set("aria-hidden", a.hidden || null);
    applyTip(el, a);
    if (kind === "stack" || kind === "tabs" || kind === "tab" || kind === "screen") return;
    if (kind === "overlay") return;
    // A table's parts (#284) have their roles by their tags; a cell is read
    // from its content (its label is for native screen readers).
    const tablePart = { table: "table", table_row: "row", column_header: "columnheader", table_cell: "cell" }[kind];
    if (tablePart) {
      set("role", a.role && a.role !== tablePart ? a.role : null);
      set("aria-label", kind === "table" || kind === "table_row" ? a.label || null : null);
      return;
    }
    if (kind === "checkbox") {
      // A native checkbox: its label is the visible text unless overridden.
      const input = el._input;
      const own = a.label && a.label !== el._label.textContent ? a.label : null;
      own ? input.setAttribute("aria-label", own) : input.removeAttribute("aria-label");
      a.hint ? input.setAttribute("aria-description", a.hint) : input.removeAttribute("aria-description");
      input.disabled = !!a.disabled;
      return;
    }
    if (kind === "radio_group") {
      el._disabled = !!a.disabled;
      radios(el).forEach((r) => { r.disabled = !!a.disabled; });
    }
    // An `<a href>` is a link by itself, and focusable.
    const link = el.localName === "a" && el.hasAttribute("href");
    const native = link ? "link" : { button: "button", toggle: "switch", slider: "slider", picker: "combobox" }[kind];
    const role = a.role;
    // A radio changed (or stopped being one): its group's tab stop is recomputed.
    if (role === "radio" || role === "radiogroup" || ["radio", "radiogroup"].includes(el.getAttribute("role"))) radiosDirty = true;
    if (role && role !== "text" && role !== native) {
      set("role", role);
      set("aria-level", role === "heading" ? String(a.level || 2) : null);
    } else if (kind !== "toggle") {
      set("role", null);
      set("aria-level", null);
    }
    set("aria-label", a.label || null);
    set("aria-description", a.hint || null);
    if (role === "switch" || role === "checkbox" || role === "radio") set("aria-checked", kind === "toggle" ? null : String(!!a.checked));
    else set("aria-checked", null);
    const selected = a.selected ?? null;
    set("aria-selected", role === "tab" ? String(!!(selected ?? a.checked)) : selected === null ? null : String(selected));
    // A link (its own `<a href>` or the role) is the current page (#144).
    set("aria-current", a.current ? (role === "link" || link ? "page" : "true") : null);
    // A disclosure (#291): whether it shows more, and the region it folds.
    set("aria-expanded", a.expanded === true || a.expanded === false ? String(a.expanded) : null);
    const target = a.controls != null ? nodes.get(a.controls) : null;
    if (target && !target.id) target.id = `prism-n${a.controls}`;
    set("aria-controls", target ? target.id : null);
    // A button with a state is a toggle button.
    set("aria-pressed", role === "button" && a.checked !== null && a.checked !== undefined ? String(a.checked) : null);
    set("aria-valuetext", role === "slider" ? a.value || null : null);
    // A polite live region: what it shows is announced.
    set("aria-live", role === "status" ? "polite" : null);
    set("aria-disabled", a.disabled || null);
    if (["INPUT", "BUTTON", "SELECT", "TEXTAREA"].includes(el.tagName)) el.disabled = !!a.disabled;
    if (el._rich) el.setAttribute("contenteditable", a.disabled ? "false" : "true");
    // Custom interactive elements must be reachable from the keyboard.
    const interactive = ["button", "link", "switch", "checkbox", "radio", "tab", "slider", "combobox"].includes(role);
    if (link) set("tabindex", a.hidden || a.disabled ? "-1" : null);
    else if (kind === "view" || kind === "text") set("tabindex", (interactive || el._kLocal) && !a.hidden && !a.disabled ? "0" : null);
  }

  function apply(p) {
    const el = p.id === undefined ? null : nodes.get(p.id);
    switch (p.op) {
      case "create": return create(p.id, p.kind);
      case "style": {
        const hidden = hiddenBy(el._css);
        // `like`: the style of a node styled earlier in this message.
        const css = p.like === undefined ? p.css : nodes.get(p.like)._css;
        el._css = css;
        setCss(el, css);
        if (el.dataset.kind === "scroll") {
          toggleClass(el, "prism-scroll-x", !!p.hx);
          // Overlay or hidden scroll bars (#132), styled by prism.css.
          if (p.sb) el.setAttribute("data-prism-scrollbar", p.sb); else el.removeAttribute("data-prism-scrollbar");
        }
        ownWidth(el, css, p.cq);
        setStates(p.id, p.states);
        if (p.group) el.dataset.group = ""; else delete el.dataset.group;
        // A named group (#212): `[data-gn="row"]:hover …` rules follow it.
        if (p.gn) el.dataset.gn = p.gn; else delete el.dataset.gn;
        if (p.inert) el.inert = true; else if (el.inert) el.inert = false;
        if (p.inert) el.setAttribute("inert", ""); else el.removeAttribute("inert");
        if (el.dataset.kind === "text_area") grow(el);
        motion(el, p.motion, hidden);
        return;
      }
      case "text": {
        const kind = el.dataset.kind;
        if (kind === "stack" || kind === "tabs") { el.dataset.path = p.text; return; }
        if (kind === "screen" || kind === "tab") {
          el._titleText = p.text;
          const owner = el.parentElement && el.parentElement.parentElement;
          if (owner && owner._screens) refreshStack(owner);
          if (owner && owner._tabs) refreshTabs(owner);
          return;
        }
        if (kind === "shape") {
          el._drawing = p.text;
          drawShape(el);
          return;
        }
        if (kind === "scene3d") return; // its scene comes as updates ("scene")
        if (kind === "web_view") return webViewSrc(el, p.text);
        if (kind === "video") {
          // The server resolves the URL (assets, the media proxy).
          if (el.getAttribute("src") !== p.text) el.src = p.text;
          return;
        }
        if (kind === "picture") {
          // The server resolves asset URLs (hashed, maybe on a CDN).
          const src = p.text;
          if (el.getAttribute("src") !== src) {
            delete el.dataset.loaded;
            el.src = src;
          }
          return;
        }
        if (kind === "checkbox") { el._label.textContent = p.text; return; }
        // A rich input's text comes with its runs (`rich`).
        if (kind === "rich_input") return;
        if (el.tagName === "INPUT" || el.tagName === "TEXTAREA") {
          setFieldText(el, p.text);
          if (el.tagName === "TEXTAREA") grow(el);
        } else if (kind === "text") {
          el._text = p.text;
          if (!el._runs) {
            if (el.childNodes.length !== 1 || el.firstChild.nodeType !== 3) el.replaceChildren(document.createTextNode(""));
            el.firstChild.data = p.text;
          }
        } else {
          let t = el.firstChild && el.firstChild.nodeType === 3 ? el.firstChild : null;
          if (!t) { t = document.createTextNode(""); el.prepend(t); }
          t.data = p.text;
        }
        return;
      }
      case "placeholder":
        // A countdown's text once ended (`timer(..).after`, #281).
        if (el.dataset.kind === "timer") { el.dataset.after = p.text; showTimer(el); return; }
        if (el._rich) el.dataset.placeholder = p.text;
        else if (el._ph) el._ph.textContent = p.text;
        else el.placeholder = p.text;
        return;
      case "config": return applyConfig(el, p.config);
      case "clickable":
        el.dataset.clickable = p.on ? "1" : "0";
        if (el.dataset.kind === "scroll") setRefreshable(el, p.id, p.on);
        return;
      case "scrollev": return setScrollEvents(el, p.id, p.on);
      case "swipe": el._swipe = { left: p.left, right: p.right }; return bindSwipe(el, p.id);
      case "value":
        if (el.dataset.kind === "toggle") el.checked = p.value !== 0;
        if (el.dataset.kind === "progress") el.value = p.value;
        if (el.dataset.kind === "slider") { el._v = p.value; el.value = p.value; }
        if (el.dataset.kind === "picker") { el._index = p.value; showPicked(el); }
        // `null`: an empty optional picker (#89).
        if (el.dataset.kind === "date_picker") { el._secs = p.value ?? NaN; showDate(el); }
        if (el.dataset.kind === "checkbox") el._input.checked = p.value !== 0;
        if (el.dataset.kind === "radio_group") { el._index = p.value; radios(el).forEach((r, i) => { r.checked = i === p.value; }); }
        if (el.dataset.kind === "overlay") {
          el._open = p.value === 1;
          if (el._open) showOverlay(el); else hideOverlay(el);
        }
        if (el.dataset.kind === "spinner") el.dataset.size = p.value === 1 ? "large" : "";
        if (el.dataset.kind === "picture") toggleClass(el, "prism-fit", p.value === 1);
        if (el.dataset.kind === "scroll") setRefreshing(el, p.value === 1);
        if (el.dataset.kind === "stack") { el._closable = (p.value & 1) === 1; el._noHeader = (p.value & 2) === 2; refreshStack(el); }
        if (el.dataset.kind === "screen") {
          el._bare = p.value === 1;
          const owner = el.parentElement && el.parentElement.parentElement;
          if (owner && owner._screens) refreshStack(owner);
        }
        if (el.dataset.kind === "timer") { el.dataset.end = p.value; showTimer(el); }
        if (el.dataset.kind === "tabs") { endLazyWait(el); el._selected = p.value; refreshTabs(el); }
        return;
      case "icon": {
        el._icon = p.icon;
        el._glyph = p.glyph;
        if (el.dataset.kind === "image") el.textContent = p.glyph;
        if (el.dataset.kind === "text_input") setFieldIcon(el, p.icon ? p.glyph : "");
        const owner = el.parentElement && el.parentElement.parentElement;
        if (owner && owner._tabs) refreshTabs(owner);
        return;
      }
      case "scene": return sceneUpdate(el, p.u);
      case "a11y":
        if (el.dataset.kind === "picture") { el.alt = p.a11y.hidden ? "" : p.a11y.label || ""; return applyTip(el, p.a11y); }
        // A frame is named by its title.
        if (el.dataset.kind === "web_view") { if (p.a11y.label) el.setAttribute("title", p.a11y.label); else el.removeAttribute("title"); }
        return applyA11y(el, p.a11y);
      case "menu": return setMenu(el, p.id, p.menu);
      case "appearance": document.documentElement.style.colorScheme = p.value; sceneAppearance(); return;
      case "env":
        document.documentElement.lang = p.lang;
        document.documentElement.dir = p.dir;
        chrome = { back: p.back, close: p.close };
        for (const n of nodes.values()) if (n._screens) refreshStack(n);
        return;
      case "children":
        if (el.dataset.kind === "stack" || el.dataset.kind === "tabs") {
          // Sheets attached to a root container are presented as dialogs.
          const sheets = p.children.filter((i) => nodes.get(i)?.dataset.kind === "modal");
          p.children = p.children.filter((i) => !sheets.includes(i));
          (el._sheets || []).filter((i) => !sheets.includes(i)).forEach((i) => nodes.get(i)?.remove());
          sheets.forEach((i) => {
            const d = nodes.get(i);
            if (!d.isConnected) { document.body.appendChild(d); d.showModal(); userNavigated = true; raiseLayers(); }
          });
          el._sheets = sheets;
        }
        if (el.dataset.kind === "stack") {
          const changed = el._screens.length !== p.children.length;
          el._screens = p.children;
          setChildren(el._body, p.children);
          refreshStack(el);
          // Move focus to the new screen's title so screen readers announce it.
          if (changed && userNavigated && isMainStack(el)) el._title.focus();
        } else if (el.dataset.kind === "tabs") {
          el._tabs = p.children;
          setChildren(el._panels, p.children);
          refreshTabs(el);
        } else if (el.dataset.kind === "overlay") {
          // The content goes in the dialog or popover box.
          if (!el._box) overlayBox(el, "sheet");
          setChildren(el._box, p.children);
          if (el._open) requestAnimationFrame(() => placeOverlay(el));
        } else if (el._head) {
          tableRows(el, p.children);
        } else if (el._cell) {
          setChildren(el._cell, p.children);
        } else {
          setChildren(el, p.children);
          // A row that became (or stopped being) a row of headers.
          const table = el.localName === "tr" && el.parentNode?.parentNode;
          if (table?._ids) tableRows(table, table._ids);
        }
        return;
      case "root": mount.replaceChildren(el); return retitle();
      // Pointer, keys, focus, layers (#17, #25).
      case "pointer": return setPointer(el, p.id, p.mask);
      case "dragdelay": return el && setDragDelay(el, p.ms);
      case "drop": return el && setDropTarget(el, p.id, p.types);
      case "dnd": return el && setDnd(el, p);
      case "keys": return setKeys(el, p.local, p.global);
      case "consume": el._consume = p.keys && p.keys.length ? new Set(p.keys) : null; return;
      case "layout": return setLayoutReports(el, p.on);
      // Text field selections (#86).
      case "sel": return el && setFieldSelection(el, p.start, p.end);
      // A rich input's runs (#115).
      case "rich": return el && el._rich && setRichValue(el, p.runs);
      case "selev": return el && setSelectEvents(el, p.on);
      // Other people's carets and selections (#240).
      case "carets": return el && setRemoteCarets(el, p.carets);
      // A rich input's editing hooks (#217).
      case "editev": if (el) { el._editMask = p.mask || 0; delete el._rectSent; if (el._rectCheck && document.activeElement === el) el._rectCheck(); } return;
      case "clip": return writeClipContent(p.items || []);
      case "instext": return el && insertText(el, p.id, p.text || "");
      // One selection across several fields (#231).
      case "selgroup":
        if (!el) return;
        if (p.on) el.dataset.selgroup = ""; else delete el.dataset.selgroup;
        el._acrossEdits = !!p.edits;
        return;
      case "selhl": return el && setHighlight(el, p.start || 0, p.end || 0);
      case "across": return el && setAcross(el, p.a, p.h);
      case "focus": if (el) { focusAsked = el; focusNode(el); } return;
      case "reveal": return el && reveal(el);
      // A pop-up menu (#247): at a point in the node (the page when it has
      // no id), or below it.
      case "popup": return popupMenu(p.id == null ? null : el, p.menu, p.at);
      // `cx.scroll_by` / `cx.scroll_to` (#193): the browser keeps it within the content.
      case "scrollto": {
        if (!el) return;
        const behavior = p.anim && !reducedMotion?.matches ? "smooth" : "auto";
        if (p.rel) el.scrollBy?.({ left: p.x, top: p.y, behavior });
        else el.scrollTo?.({ left: p.x, top: p.y, behavior });
        return;
      }
      // `cx.dismiss_keyboard()` (#29): the focused field lets go (phones
      // hide their keyboard).
      case "blur": {
        const active = document.activeElement;
        return active && active !== document.body && active.blur && active.blur();
      }
      case "layers":
        layerHost.replaceChildren(...p.ids.map((i) => nodes.get(i)).filter(Boolean));
        return raiseLayers();
      // Islands: client-side Web Components.
      case "island": return setIsland(p);
      case "props": return setIslandProps(el, p.props);
      case "link": return setLink(el, p.id, p.href);
      case "attrs": return setAttrs(el, p.attrs);
      // Web views (#62).
      case "webpost": return el && webPost(el, p.data);
      case "webreload": return el && webReload(el);
      case "destroy":
        if (el) {
          el.remove();
          if (el._scene) sceneStop(el._scene);
          if (el._kGlobal) globalKeys--;
          if (el._dropTypes) dropTargets--;
          if (el._list) el._list.remove();
          placedOverlays.delete(el);
          pendingOverlays.delete(el);
          if (keyboardBars.delete(el)) el._box?.remove();
          if (el._carets) dropCarets(el);
        }
        nodes.delete(p.id);
        setStates(p.id, null);
        stacks = stacks.filter((s) => s !== el);
        return;
    }
  }

  // --- 3D scenes (prism-core scene3d): WebGL 2 ---------------------------------------
  // A scene node is a `.prism-scene3d` box holding a canvas. Its scene comes
  // as updates (`{"op":"scene","id","u"}`: the first one full, then per-node
  // diffs; the server's first paint puts the first one in `data-scene`).
  // Heavy data (meshes, instance sets, clips, skins, environments, embedded
  // images) are content-addressed blobs, fetched once per page from
  // `<base>/_prism/3d/<id>`. Each scene keeps one ScenePlayer across updates
  // (the orbit, spins, transitions and clips carry on) and draws only when
  // something changed, arrived or moves (`frame.animating`), never offscreen
  // or in a hidden tab. Pages without scenes build none of this.

  // <scene3d-core> (tests/js/scene3d.test.mjs runs this function on its own)
  /**
   * The device side of scenes, ported line by line from prism-core's
   * `scene3d` (math.rs, geometry.rs, blobs.rs, env.rs's 'E' layout, wire.rs,
   * clip.rs, player.rs; the GLSL of shaders.rs verbatim), so every backend
   * moves, animates, draws and picks the same. Built the first time a page
   * shows a scene.
   */
  function scene3dCore() {
    // --- math.rs: column-major 4×4 matrices (OpenGL layout), OpenGL clip space.
    const IDENTITY = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
    const QUAT_IDENTITY = [0, 0, 0, 1];
    const rad = (d) => (d * Math.PI) / 180;
    const deg = (r) => (r * 180) / Math.PI;
    const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
    const add = (a, b) => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
    const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    const scale = (v, s) => [v[0] * s, v[1] * s, v[2] * s];
    const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    const length = (v) => Math.sqrt(dot(v, v));
    const normalize = (v) => { const l = length(v); return l === 0 ? v : scale(v, 1 / l); };
    const isZero = (v) => v[0] === 0 && v[1] === 0 && v[2] === 0;
    const lerp = (a, b, t) => a + (b - a) * t;
    const lerp3 = (a, b, t) => [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)];
    const srgbToLinear = (c) => (c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4));
    /** Same numbers (arrays, typed or not; `null`s). */
    const sameArray = (a, b) => {
      if (a === b) return true;
      if (!a || !b || a.length !== b.length) return false;
      for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
      return true;
    };
    /** `a · b` (`b` applies first). */
    function mul(a, b) {
      const out = new Array(16);
      for (let c = 0; c < 4; c++) {
        for (let r = 0; r < 4; r++) {
          out[c * 4 + r] = a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1] + a[8 + r] * b[c * 4 + 2] + a[12 + r] * b[c * 4 + 3];
        }
      }
      return out;
    }
    const translation = (v) => { const m = IDENTITY.slice(); m[12] = v[0]; m[13] = v[1]; m[14] = v[2]; return m; };
    const scaling = (v) => { const m = IDENTITY.slice(); m[0] = v[0]; m[5] = v[1]; m[10] = v[2]; return m; };
    const rotationX = (d) => { const s = Math.sin(rad(d)), c = Math.cos(rad(d)), m = IDENTITY.slice(); m[5] = c; m[6] = s; m[9] = -s; m[10] = c; return m; };
    const rotationY = (d) => { const s = Math.sin(rad(d)), c = Math.cos(rad(d)), m = IDENTITY.slice(); m[0] = c; m[2] = -s; m[8] = s; m[10] = c; return m; };
    const rotationZ = (d) => { const s = Math.sin(rad(d)), c = Math.cos(rad(d)), m = IDENTITY.slice(); m[0] = c; m[1] = s; m[4] = -s; m[5] = c; return m; };
    /** Euler angles in degrees: `Rx · Ry · Rz` (Z applies first). */
    const rotation = (d) => mul(rotationX(d[0]), mul(rotationY(d[1]), rotationZ(d[2])));
    /** A node's local matrix: `T · R · S`. */
    const compose = (p, r, s) => mul(translation(p), mul(rotation(r), scaling(s)));
    /** A node's local matrix from a quaternion: `T · R(q) · S`. */
    function composeQuat(p, q, s) {
      const m = quatToMat4(q);
      for (let c = 0; c < 3; c++) for (let r = 0; r < 3; r++) m[c * 4 + r] *= s[c];
      m[12] = p[0]; m[13] = p[1]; m[14] = p[2];
      return m;
    }
    /** The rotation matrix of a unit quaternion `[x, y, z, w]`. */
    function quatToMat4(q) {
      const [x, y, z, w] = q;
      const xx = x * x, yy = y * y, zz = z * z, xy = x * y, xz = x * z, yz = y * z, wx = w * x, wy = w * y, wz = w * z;
      return [
        1 - 2 * (yy + zz), 2 * (xy + wz), 2 * (xz - wy), 0,
        2 * (xy - wz), 1 - 2 * (xx + zz), 2 * (yz + wx), 0,
        2 * (xz + wy), 2 * (yz - wx), 1 - 2 * (xx + yy), 0,
        0, 0, 0, 1,
      ];
    }
    /** `a · b` (`b` applies first). */
    function quatMul(a, b) {
      const [ax, ay, az, aw] = a, [bx, by, bz, bw] = b;
      return [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
      ];
    }
    /** The quaternion of Euler angles in degrees, the same rotation as `rotation` (Z applies first). */
    function quatFromEuler(d) {
      const h = (x) => [Math.sin(rad(x) / 2), Math.cos(rad(x) / 2)];
      const [sx, cx] = h(d[0]), [sy, cy] = h(d[1]), [sz, cz] = h(d[2]);
      return quatMul([sx, 0, 0, cx], quatMul([0, sy, 0, cy], [0, 0, sz, cz]));
    }
    /** Unit length (the identity if `q` is zero or not finite). */
    function quatNormalize(q) {
      const l = Math.sqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]);
      return l > 1e-12 && Number.isFinite(l) ? [q[0] / l, q[1] / l, q[2] / l, q[3] / l] : QUAT_IDENTITY.slice();
    }
    /** Spherical interpolation from `a` to `b` the shorter way around (nlerp when nearly equal). */
    function slerp(a, b, t) {
      let d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
      if (d < 0) { d = -d; b = b.map((c) => -c); }
      if (d > 0.9995) return quatNormalize([0, 1, 2, 3].map((i) => lerp(a[i], b[i], t)));
      const theta = Math.acos(clamp(d, -1, 1)), s = Math.sin(theta);
      const wa = Math.sin((1 - t) * theta) / s, wb = Math.sin(t * theta) / s;
      return quatNormalize([0, 1, 2, 3].map((i) => a[i] * wa + b[i] * wb));
    }
    /** The box holding `b` once transformed by `m` (its 8 corners'). */
    function transformAabb(m, b) {
      const center = [(b[0] + b[3]) / 2, (b[1] + b[4]) / 2, (b[2] + b[5]) / 2];
      const half = [(b[3] - b[0]) / 2, (b[4] - b[1]) / 2, (b[5] - b[2]) / 2];
      const c = transformPoint(m, center);
      // Each axis' extent: |M| · half (the 3×3 part, absolute values).
      const e = [0, 1, 2].map((r) => Math.abs(m[r]) * half[0] + Math.abs(m[4 + r]) * half[1] + Math.abs(m[8 + r]) * half[2]);
      return [c[0] - e[0], c[1] - e[1], c[2] - e[2], c[0] + e[0], c[1] + e[1], c[2] + e[2]];
    }
    const aabbUnion = (a, b) => [
      Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.min(a[2], b[2]),
      Math.max(a[3], b[3]), Math.max(a[4], b[4]), Math.max(a[5], b[5]),
    ];
    /** The six planes (left, right, bottom, top, near, far) of `viewProj`'s clip volume (Gribb–Hartmann). */
    function frustumPlanes(vp) {
      const row = (r) => [vp[r], vp[4 + r], vp[8 + r], vp[12 + r]];
      const r0 = row(0), r1 = row(1), r2 = row(2), r3 = row(3);
      const plus = (a, b) => a.map((v, i) => v + b[i]), minus = (a, b) => a.map((v, i) => v - b[i]);
      return [plus(r3, r0), minus(r3, r0), plus(r3, r1), minus(r3, r1), plus(r3, r2), minus(r3, r2)];
    }
    /** Whether `b` is at least partly inside the frustum (conservative). */
    function aabbInFrustum(planes, b) {
      for (const p of planes) {
        // The corner farthest along the plane's normal.
        const x = p[0] >= 0 ? b[3] : b[0], y = p[1] >= 0 ? b[4] : b[1], z = p[2] >= 0 ? b[5] : b[2];
        if (!(p[0] * x + p[1] * y + p[2] * z + p[3] >= 0)) return false;
      }
      return true;
    }
    /** Where the ray `origin + t · dir` enters `b`: the smallest `t ≥ 0`, or null (the slab test). */
    function rayAabb(origin, dir, b) {
      let t0 = 0, t1 = Infinity;
      for (let a = 0; a < 3; a++) {
        const lo = b[a], hi = b[a + 3];
        if (Math.abs(dir[a]) < 1e-12) {
          if (origin[a] < lo || origin[a] > hi) return null;
          continue;
        }
        const inv = 1 / dir[a];
        let n = (lo - origin[a]) * inv, f = (hi - origin[a]) * inv;
        if (n > f) [n, f] = [f, n];
        t0 = Math.max(t0, n);
        t1 = Math.min(t1, f);
        if (t0 > t1) return null;
      }
      return t0;
    }
    function perspective(fov, aspect, near, far) {
      const f = 1 / Math.tan(rad(fov) / 2), nf = 1 / (near - far);
      const m = new Array(16).fill(0);
      m[0] = f / aspect;
      m[5] = f;
      m[10] = (far + near) * nf;
      m[11] = -1;
      m[14] = 2 * far * near * nf;
      return m;
    }
    function orthographic(height, aspect, near, far) {
      const t = height / 2, r = (height / 2) * aspect, m = IDENTITY.slice();
      m[0] = 1 / r;
      m[5] = 1 / t;
      m[10] = -2 / (far - near);
      m[14] = -(far + near) / (far - near);
      return m;
    }
    function lookAt(eye, target, up) {
      let f = normalize(sub(target, eye));
      if (length(f) === 0) f = [0, 0, -1];
      let s = cross(f, up);
      // Looking straight up or down: any horizontal side vector.
      if (length(s) < 1e-6) s = cross(f, [0, 0, -1]);
      s = normalize(s);
      const u = cross(s, f);
      return [s[0], u[0], -f[0], 0, s[1], u[1], -f[1], 0, s[2], u[2], -f[2], 0, -dot(s, eye), -dot(u, eye), dot(f, eye), 1];
    }
    /** The inverse (the identity if `m` is singular: zero determinant). */
    function invert(m) {
      const inv = new Array(16);
      inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15]
          + m[9] * m[7] * m[14]
          + m[13] * m[6] * m[11]
          - m[13] * m[7] * m[10];
      inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15]
          - m[8] * m[7] * m[14]
          - m[12] * m[6] * m[11]
          + m[12] * m[7] * m[10];
      inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15]
          + m[8] * m[7] * m[13]
          + m[12] * m[5] * m[11]
          - m[12] * m[7] * m[9];
      inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14]
          - m[8] * m[6] * m[13]
          - m[12] * m[5] * m[10]
          + m[12] * m[6] * m[9];
      inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15]
          - m[9] * m[3] * m[14]
          - m[13] * m[2] * m[11]
          + m[13] * m[3] * m[10];
      inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15]
          + m[8] * m[3] * m[14]
          + m[12] * m[2] * m[11]
          - m[12] * m[3] * m[10];
      inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15]
          - m[8] * m[3] * m[13]
          - m[12] * m[1] * m[11]
          + m[12] * m[3] * m[9];
      inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14]
          + m[8] * m[2] * m[13]
          + m[12] * m[1] * m[10]
          - m[12] * m[2] * m[9];
      inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15]
          + m[5] * m[3] * m[14]
          + m[13] * m[2] * m[7]
          - m[13] * m[3] * m[6];
      inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15]
          - m[4] * m[3] * m[14]
          - m[12] * m[2] * m[7]
          + m[12] * m[3] * m[6];
      inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15]
          + m[4] * m[3] * m[13]
          + m[12] * m[1] * m[7]
          - m[12] * m[3] * m[5];
      inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14]
          - m[4] * m[2] * m[13]
          - m[12] * m[1] * m[6]
          + m[12] * m[2] * m[5];
      inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11]
          - m[5] * m[3] * m[10]
          - m[9] * m[2] * m[7]
          + m[9] * m[3] * m[6];
      inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11]
          + m[4] * m[3] * m[10]
          + m[8] * m[2] * m[7]
          - m[8] * m[3] * m[6];
      inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11]
          - m[4] * m[3] * m[9]
          - m[8] * m[1] * m[7]
          + m[8] * m[3] * m[5];
      inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10]
          + m[4] * m[2] * m[9]
          + m[8] * m[1] * m[6]
          - m[8] * m[2] * m[5];
      const det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
      // Only a truly singular matrix: a centimeter rig (1/100 twice) has a fine inverse.
      if (det === 0 || !Number.isFinite(det)) return IDENTITY.slice();
      const out = inv.map((x) => x / det);
      return out.every(Number.isFinite) ? out : IDENTITY.slice();
    }
    /** The inverse transpose of the upper 3×3, column-major. */
    const normalMatrix = (m) => { const i = invert(m); return [i[0], i[4], i[8], i[1], i[5], i[9], i[2], i[6], i[10]]; };
    function transformPoint(m, p) {
      const x = m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12];
      const y = m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13];
      const z = m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14];
      const w = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15];
      return w !== 0 && w !== 1 ? [x / w, y / w, z / w] : [x, y, z];
    }
    const transformDir = (m, v) => [
      m[0] * v[0] + m[4] * v[1] + m[8] * v[2],
      m[1] * v[0] + m[5] * v[1] + m[9] * v[2],
      m[2] * v[0] + m[6] * v[1] + m[10] * v[2],
    ];

    // --- blobs.rs (+ env.rs' 'E'): little-endian, 4-byte aligned sections;
    // "P3D" + kind + u32 version 1. Decoders return null on anything malformed.
    const LITTLE = new Uint8Array(new Uint32Array([1]).buffer)[0] === 1;
    const BAD = {};
    function utf8(bytes) {
      if (typeof TextDecoder === "function") return new TextDecoder().decode(bytes);
      let s = "";
      for (let i = 0; i < bytes.length;) {
        const b = bytes[i++];
        let c = b;
        if (b >= 0xf0) { c = ((b & 7) << 18) | ((bytes[i++] & 63) << 12) | ((bytes[i++] & 63) << 6) | (bytes[i++] & 63); }
        else if (b >= 0xe0) { c = ((b & 15) << 12) | ((bytes[i++] & 63) << 6) | (bytes[i++] & 63); }
        else if (b >= 0xc0) { c = ((b & 31) << 6) | (bytes[i++] & 63); }
        s += String.fromCodePoint(c);
      }
      return s;
    }
    /** A reader of a `kind` blob (its header checked); every read past the end throws. */
    function reader(buf, kind) {
      if (!(buf instanceof ArrayBuffer)) buf = new Uint8Array(buf).slice().buffer;
      const view = new DataView(buf);
      let at = 0;
      const take = (n) => {
        if (!(n >= 0) || at + n > buf.byteLength) throw BAD;
        const s = at;
        at += n;
        return s;
      };
      /** `n` values as a typed array: a view when aligned (no copy), else read one by one. */
      const typed = (Type, n, get) => {
        const size = Type.BYTES_PER_ELEMENT, s = take(n * size);
        if (LITTLE && s % size === 0) return new Type(buf, s, n);
        const out = new Type(n);
        for (let i = 0; i < n; i++) out[i] = get(s + i * size);
        return out;
      };
      const r = {
        get at() { return at; },
        size: buf.byteLength,
        take,
        u8: () => view.getUint8(take(1)),
        u32: () => view.getUint32(take(4), true),
        f32: () => view.getFloat32(take(4), true),
        f32s: (n) => typed(Float32Array, n, (o) => view.getFloat32(o, true)),
        u32s: (n) => typed(Uint32Array, n, (o) => view.getUint32(o, true)),
        u16s: (n) => { const v = typed(Uint16Array, n, (o) => view.getUint16(o, true)); r.align(); return v; },
        str: () => { const len = r.u32(), s = take(len), v = utf8(new Uint8Array(buf, s, len)); r.align(); return v; },
        align: () => { at = Math.min(Math.ceil(at / 4) * 4, buf.byteLength); },
      };
      const magic = new Uint8Array(buf, take(4), 4);
      if (magic[0] !== 80 || magic[1] !== 51 || magic[2] !== 68 || magic[3] !== kind.charCodeAt(0) || r.u32() !== 1) throw BAD;
      return r;
    }
    const guard = (f) => { try { return f(); } catch (e) { if (e === BAD || e instanceof RangeError) return null; throw e; } };
    /** An 'M' blob: `{ count, positions, normals, uvs, joints, weights, colors, indices, bounds }` (absent attributes null). */
    const decodeMesh = (buf) => guard(() => {
      const r = reader(buf, "M");
      const n = r.u32(), m = r.u32(), flags = r.u32(), bounds = Array.from(r.f32s(6));
      const positions = r.f32s(n * 3), normals = r.f32s(n * 3);
      const uvs = flags & 1 ? r.f32s(n * 2) : null;
      let joints = null, weights = null;
      if (flags & 4) { joints = r.u16s(n * 4); weights = r.f32s(n * 4); }
      const colors = flags & 8 ? r.f32s(n * 4) : null;
      const indices = r.u32s(m);
      for (let i = 0; i < m; i++) if (indices[i] >= n) return null;
      return { count: n, positions, normals, uvs, joints, weights, colors, indices, bounds };
    });
    /** An 'I' blob, with each instance's matrix (`T · R(q) · S`, 16 floats). */
    const decodeInstances = (buf) => guard(() => {
      const r = reader(buf, "I");
      const count = r.u32(), flags = r.u32(), bounds = Array.from(r.f32s(6));
      const raw = r.f32s(count * 10);
      const colors = flags & 1 ? r.f32s(count * 4) : null;
      const matrices = new Float32Array(count * 16), transforms = [];
      for (let k = 0; k < count; k++) {
        const v = raw.subarray(k * 10, k * 10 + 10);
        const t = { position: [v[0], v[1], v[2]], rotation: [v[3], v[4], v[5], v[6]], scale: [v[7], v[8], v[9]] };
        transforms.push(t);
        matrices.set(composeQuat(t.position, t.rotation, t.scale), k * 16);
      }
      return { count, bounds, transforms, colors, matrices, matrix: (k) => matrices.subarray(k * 16, k * 16 + 16) };
    });
    /** An 'A' blob: `{ duration, channels: [{ target, property (0 T, 1 R, 2 S), interpolation (0 linear, 1 step, 2 cubic), times, values }] }`. */
    const decodeClip = (buf) => guard(() => {
      const r = reader(buf, "A");
      const count = r.u32(), duration = r.f32(), channels = [];
      for (let i = 0; i < count; i++) {
        const target = r.str(), property = r.u8(), interpolation = r.u8();
        if (property > 2 || interpolation > 2) return null;
        r.take(2);
        const k = r.u32(), times = r.f32s(k);
        const values = r.f32s(k * (property === 1 ? 4 : 3) * (interpolation === 2 ? 3 : 1));
        channels.push({ target, property, interpolation, times, values });
      }
      return { duration, channels };
    });
    /** A 'K' blob: `{ inverseBind: [mat4…], joints: ["0/1"…] }`. */
    const decodeSkin = (buf) => guard(() => {
      const r = reader(buf, "K");
      const n = r.u32(), raw = r.f32s(n * 16), inverseBind = [], joints = [];
      for (let i = 0; i < n; i++) inverseBind.push(Array.from(raw.subarray(i * 16, i * 16 + 16)));
      for (let i = 0; i < n; i++) joints.push(r.str());
      return { inverseBind, joints };
    });
    /** An 'E' blob: `{ width, height, sh (27), levels: [{ w, h, data (RGBA half floats) }] }`. */
    const decodeEnv = (buf) => guard(() => {
      const r = reader(buf, "E");
      const width = r.u32(), height = r.u32(), n = r.u32();
      if (!width || !height || !n || n > 32) return null;
      const sh = Array.from(r.f32s(27)), levels = [];
      for (let i = 0; i < n; i++) {
        const w = Math.max(width >>> i, 1), h = Math.max(height >>> i, 1);
        levels.push({ w, h, data: r.u16s(w * h * 4) });
      }
      return r.at === r.size ? { width, height, sh, levels } : null;
    });
    /** Decoders by blob kind ('P': an image's bytes, as they are). */
    const decode = { M: decodeMesh, I: decodeInstances, A: decodeClip, K: decodeSkin, E: decodeEnv, P: (buf) => buf };

    // --- clip.rs: the reference sampler.
    /** A channel's value at `t` seconds (`[x, y, z, 0]`, or a unit quaternion): the first key's before it, the last's after. */
    function sampleChannel(ch, t) {
      const c = ch.property === 1 ? 4 : 3, cubic = ch.interpolation === 2, stride = cubic ? 3 * c : c;
      const k = Math.min(ch.times.length, Math.floor(ch.values.length / stride));
      if (k === 0) return ch.property === 1 ? QUAT_IDENTITY.slice() : ch.property === 2 ? [1, 1, 1, 0] : [0, 0, 0, 0];
      // A key's value: the middle third of a cubic key.
      const value = (i) => { const at = i * stride + (cubic ? c : 0); return [0, 1, 2, 3].map((j) => (j < c ? ch.values[at + j] : 0)); };
      const finish = (v) => (ch.property === 1 ? quatNormalize(v) : v);
      if (t <= ch.times[0] || k === 1) return finish(value(0));
      if (t >= ch.times[k - 1]) return finish(value(k - 1));
      // The key before t (times increase: binary search).
      let lo = 0, hi = k;
      while (lo < hi) { const mid = (lo + hi) >> 1; if (ch.times[mid] <= t) lo = mid + 1; else hi = mid; }
      const i = Math.max(lo - 1, 0), t0 = ch.times[i], t1 = ch.times[i + 1], dt = t1 - t0;
      const s = dt > 0 ? (t - t0) / dt : 0;
      if (ch.interpolation === 1) return finish(value(i));
      if (ch.interpolation === 0) {
        if (ch.property === 1) return slerp(value(i), value(i + 1), s);
        const a = value(i), b = value(i + 1);
        return a.map((x, j) => lerp(x, b[j], s));
      }
      // glTF: p(s) = (2s³ − 3s² + 1)·v0 + (s³ − 2s² + s)·dt·b0 + (−2s³ + 3s²)·v1 + (s³ − s²)·dt·a1
      // (b0: key i's out-tangent, a1: key i+1's in-tangent).
      const s2 = s * s, s3 = s2 * s, v0 = i * stride + c, b0 = i * stride + 2 * c, a1 = (i + 1) * stride, v1 = (i + 1) * stride + c;
      const vs = ch.values;
      return finish([0, 1, 2, 3].map((j) => (j >= c ? 0
        : (2 * s3 - 3 * s2 + 1) * vs[v0 + j] + (s3 - 2 * s2 + s) * dt * vs[b0 + j]
          + (-2 * s3 + 3 * s2) * vs[v1 + j] + (s3 - s2) * dt * vs[a1 + j])));
    }

    // --- wire.rs: the update JSON, serde's defaults filled in.
    const num = (v, d) => (typeof v === "number" && Number.isFinite(v) ? v : d);
    const nums = (v, d) => d.map((x, i) => num(Array.isArray(v) ? v[i] : undefined, x));
    const obj = (v) => (v && typeof v === "object" && !Array.isArray(v) ? v : {});
    const str = (v) => (typeof v === "string" ? v : null);
    const flag = (v) => v === true;
    const blobId = (v) => (typeof v === "string" && /^[0-9a-fA-F]{32}$/.test(v) ? v.toLowerCase() : null);
    /** `[r, g, b, a]` or `[r, g, b, a, dr, dg, db, da]` (sRGB, dark variant). */
    function rgba(v, d) {
      if (!Array.isArray(v)) return d;
      return { light: nums(v.slice(0, 4), [0, 0, 0, 1]), dark: v.length >= 8 ? nums(v.slice(4, 8), [0, 0, 0, 1]) : null };
    }
    const resolve = (c, dark) => (dark && c.dark ? c.dark : c.light);
    const WHITE = { light: [1, 1, 1, 1], dark: null };
    const BLACK = { light: [0, 0, 0, 1], dark: null };
    /** A mesh ref, with its `key` (`Mesh::key`: GPU buffers are cached by it) and `bounds` (`Mesh::bounds`). */
    function mesh(m) {
      m = obj(m);
      const box = (x, y, z) => [-x, -y, -z, x, y, z];
      switch (m.type) {
        case "box": { const size = nums(m.size, [1, 1, 1]); return { type: "box", size, key: `box:${size}`, bounds: box(size[0] / 2, size[1] / 2, size[2] / 2) }; }
        case "sphere": { const r = num(m.radius, 1); return { type: "sphere", radius: r, key: `sphere:${r}`, bounds: box(r, r, r) }; }
        case "plane": { const size = nums(m.size, [1, 1]); return { type: "plane", size, key: `plane:${size}`, bounds: box(size[0] / 2, 0, size[1] / 2) }; }
        case "cylinder": case "cone": {
          const r = num(m.radius, 1), h = num(m.height, 1);
          return { type: m.type, radius: r, height: h, key: `${m.type}:${r},${h}`, bounds: box(r, h / 2, r) };
        }
        case "torus": {
          const r = num(m.radius, 1), t = num(m.tube, 0.25);
          return { type: "torus", radius: r, tube: t, key: `torus:${r},${t}`, bounds: box(r + t, t, r + t) };
        }
        case "blob": { const id = blobId(m.id); return id && { type: "blob", id, key: id, bounds: nums(m.bounds, [0, 0, 0, 0, 0, 0]) }; }
        default: return null;
      }
    }
    function material(m) {
      m = obj(m);
      return {
        color: rgba(m.color, { light: [0.8, 0.8, 0.8, 1], dark: null }),
        metal: num(m.metal, 0),
        rough: num(m.rough, 0.5),
        emissive: m.emissive ? rgba(m.emissive, null) : null,
        unlit: flag(m.unlit),
        double: flag(m.double),
        map: str(m.map),
        mr_map: str(m.mr_map),
        normal_map: str(m.normal_map),
        normal_scale: num(m.normal_scale, 1),
        emissive_map: str(m.emissive_map),
        ao_map: str(m.ao_map),
        alpha: m.alpha === "mask" || m.alpha === "opaque" ? m.alpha : "blend",
        cutoff: num(m.cutoff, 0),
        vcolor: flag(m.vcolor),
      };
    }
    /** A `FlatNode`: v1's props without children, plus key, parent, label, quat, inst, play, skin, cast, recv. */
    function flatNode(n) {
      n = obj(n);
      const inst = obj(n.inst), play = obj(n.play), skin = obj(n.skin);
      const instId = blobId(inst.id), clipId = blobId(play.clip), skinId = blobId(skin.id);
      return {
        key: typeof n.key === "string" ? n.key : String(n.key ?? ""),
        parent: str(n.parent),
        name: str(n.name),
        label: str(n.label),
        mesh: mesh(n.mesh),
        mat: material(n.mat),
        pos: nums(n.pos, [0, 0, 0]),
        rot: nums(n.rot, [0, 0, 0]),
        quat: Array.isArray(n.quat) ? nums(n.quat, [0, 0, 0, 1]) : null,
        scale: nums(n.scale, [1, 1, 1]),
        spin: nums(n.spin, [0, 0, 0]),
        hidden: flag(n.hidden),
        inst: instId && { id: instId, count: Math.max(0, Math.floor(num(inst.count, 0))), bounds: nums(inst.bounds, [0, 0, 0, 0, 0, 0]) },
        play: clipId && { clip: clipId, speed: num(play.speed, 1), loop: flag(play.loop), fade: num(play.fade, 0), run: num(play.run, 0) },
        skin: skinId && typeof skin.root === "string" ? { id: skinId, root: skin.root } : null,
        cast: n.cast !== false,
        recv: n.recv !== false,
      };
    }
    function light(l) {
      l = obj(l);
      return {
        type: l.type, color: rgba(l.color, WHITE), intensity: num(l.intensity, 1),
        pos: nums(l.pos, [0, 0, 0]), dir: nums(l.dir, [0, 0, 0]), angle: num(l.angle, 0), range: num(l.range, 0),
        shadows: flag(l.shadows),
      };
    }
    /** `SceneSettings`: everything about a scene but its nodes. */
    function settings(s) {
      s = obj(s);
      const c = obj(s.camera), o = s.orbit && typeof s.orbit === "object" ? s.orbit : null;
      const t = s.t && typeof s.t === "object" ? s.t : null, env = obj(s.env), envId = blobId(env.id);
      return {
        bg: s.bg ? rgba(s.bg, null) : null,
        camera: {
          pos: nums(c.pos, [0, 2, 6]), target: nums(c.target, [0, 0, 0]),
          fov: num(c.fov, 45), near: num(c.near, 0.1), far: num(c.far, 1000),
          ortho: typeof c.ortho === "number" ? c.ortho : null,
        },
        orbit: o && {
          rotate: flag(o.rotate), zoom: flag(o.zoom), pan: flag(o.pan),
          auto: num(o.auto, 0), min: num(o.min, 0), max: num(o.max, 0),
        },
        lights: Array.isArray(s.lights) ? s.lights.map(light) : [],
        t: t && { ms: num(t.ms, 0), delay: num(t.delay, 0), bezier: nums(t.bezier, [0, 0, 1, 1]) },
        tap: flag(s.tap),
        env: envId && { id: envId, intensity: num(env.intensity, 1), background: flag(env.background) },
        exposure: num(s.exposure, 1),
      };
    }
    /**
     * A `SceneUpdate` (JSON text or its value): `{ full, settings (null: unchanged), nodes (full: every
     * node in pre-order; diff: the upserts), remove, order (null: unchanged) }`; null unless version 2.
     */
    function decodeUpdate(u) {
      if (typeof u === "string") { try { u = JSON.parse(u); } catch (e) { return null; } }
      if (!u || typeof u !== "object" || u.v !== 2) return null;
      const list = (v) => (Array.isArray(v) ? v : []);
      const keys = (v) => list(v).filter((k) => typeof k === "string");
      if (u.full === true) return { full: true, settings: settings(u.settings), nodes: list(u.nodes).map(flatNode), remove: [], order: null };
      return {
        full: false,
        settings: u.settings && typeof u.settings === "object" ? settings(u.settings) : null,
        nodes: list(u.upsert).map(flatNode),
        remove: keys(u.remove),
        order: Array.isArray(u.order) ? keys(u.order) : null,
      };
    }
    /** `FlatScene::apply`: the scene `{ settings, nodes }` after `u` (drop removed keys, replace or insert upserts, reorder). */
    function applyFlat(flat, u) {
      if (u.full) return { settings: u.settings, nodes: u.nodes.slice() };
      let nodes = flat.nodes;
      if (u.remove.length) { const gone = new Set(u.remove); nodes = nodes.filter((n) => !gone.has(n.key)); }
      if (u.nodes.length) {
        nodes = nodes.slice();
        const at = new Map(nodes.map((n, i) => [n.key, i]));
        for (const n of u.nodes) {
          const i = at.get(n.key);
          if (i !== undefined) nodes[i] = n;
          else { at.set(n.key, nodes.length); nodes.push(n); }
        }
      }
      if (u.order) {
        const by = new Map(nodes.map((n) => [n.key, n]));
        nodes = [];
        for (const k of u.order) { const n = by.get(k); if (n) { by.delete(k); nodes.push(n); } }
      }
      return { settings: u.settings || flat.settings, nodes };
    }
    const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
    /** The app's lights, or (no lights, no environment) the default rig (`SceneSettings::effective_lights`). */
    const effectiveLights = (s) => (s.lights.length || s.env ? s.lights : [
      { type: "ambient", color: WHITE, intensity: 0.35, pos: [0, 0, 0], dir: [0, 0, 0], angle: 0, range: 0, shadows: false },
      { type: "directional", color: WHITE, intensity: 0.9, pos: [0, 0, 0], dir: [-0.5, -1, -0.6], angle: 0, range: 0, shadows: false },
    ]);

    // --- geometry.rs: CCW front faces, outward normals, uv (0, 0) top-left.
    const SEGMENTS = 48, RINGS = 24, TAU = Math.PI * 2;
    /** Vertex data: flat arrays (typed for blobs); colors, joints, weights null when absent. */
    function meshData() {
      return {
        positions: [], normals: [], uvs: [], colors: null, joints: null, weights: null, indices: [],
        vertex(p, n, uv) {
          const i = this.positions.length / 3;
          this.positions.push(p[0], p[1], p[2]);
          this.normals.push(n[0], n[1], n[2]);
          this.uvs.push(uv[0], uv[1]);
          return i;
        },
        triangle(a, b, c) { this.indices.push(a, b, c); },
        position(i) { return [this.positions[i * 3], this.positions[i * 3 + 1], this.positions[i * 3 + 2]]; },
        vertexCount() { return Math.floor(this.positions.length / 3); },
      };
    }
    /** A box centered on the origin: faces +X, −X, +Y, −Y, +Z, −Z, four vertices each. */
    function cuboid(size) {
      const half = size.map((s) => s / 2), m = meshData();
      // (normal, right, up) per face, seen from outside: right × up = normal.
      const faces = [
        [[1, 0, 0], [0, 0, -1], [0, 1, 0]],
        [[-1, 0, 0], [0, 0, 1], [0, 1, 0]],
        [[0, 1, 0], [1, 0, 0], [0, 0, -1]],
        [[0, -1, 0], [1, 0, 0], [0, 0, 1]],
        [[0, 0, 1], [1, 0, 0], [0, 1, 0]],
        [[0, 0, -1], [-1, 0, 0], [0, 1, 0]],
      ];
      const extent = (v) => Math.abs(v[0]) * half[0] + Math.abs(v[1]) * half[1] + Math.abs(v[2]) * half[2];
      for (const [n, r, u] of faces) {
        const dn = extent(n), dr = extent(r), du = extent(u);
        const at = (sr, su) => [0, 1, 2].map((k) => n[k] * dn + r[k] * dr * sr + u[k] * du * su);
        // Top-left, top-right, bottom-right, bottom-left.
        const a = m.vertex(at(-1, 1), n, [0, 0]);
        const b = m.vertex(at(1, 1), n, [1, 0]);
        const c = m.vertex(at(1, -1), n, [1, 1]);
        const d = m.vertex(at(-1, -1), n, [0, 1]);
        m.triangle(a, d, c);
        m.triangle(a, c, b);
      }
      return m;
    }
    /** A UV sphere: RINGS rings from the top pole (v = 0), SEGMENTS + 1 vertices around each. */
    function sphere(radius) {
      const m = meshData();
      for (let i = 0; i <= RINGS; i++) {
        const v = i / RINGS, theta = v * Math.PI;
        for (let j = 0; j <= SEGMENTS; j++) {
          const u = j / SEGMENTS, phi = u * TAU;
          const n = [-Math.sin(theta) * Math.sin(phi), Math.cos(theta), Math.sin(theta) * Math.cos(phi)];
          m.vertex(scale(n, radius), n, [u, v]);
        }
      }
      const row = SEGMENTS + 1;
      for (let i = 0; i < RINGS; i++) {
        for (let j = 0; j < SEGMENTS; j++) {
          const a = i * row + j, b = i * row + j + 1, c = (i + 1) * row + j, d = (i + 1) * row + j + 1;
          if (i !== 0) m.triangle(a, b, c);
          if (i !== RINGS - 1) m.triangle(b, d, c);
        }
      }
      return m;
    }
    /** A rectangle in the XZ plane facing +Y; u along +X, v along +Z. */
    function plane(size) {
      const hx = size[0] / 2, hz = size[1] / 2, m = meshData(), n = [0, 1, 0];
      const a = m.vertex([-hx, 0, -hz], n, [0, 0]);
      const b = m.vertex([hx, 0, -hz], n, [1, 0]);
      const c = m.vertex([hx, 0, hz], n, [1, 1]);
      const d = m.vertex([-hx, 0, hz], n, [0, 1]);
      m.triangle(a, d, c);
      m.triangle(a, c, b);
      return m;
    }
    /** A capped cylinder along Y (a cone when `top` is 0): side, top cap, bottom cap. */
    function cylinder(bottom, top, height) {
      const m = meshData(), h = height / 2;
      // The side's normal leans up by the slope (r_bottom - r_top) / height.
      const slope = height > 0 ? (bottom - top) / height : 0;
      const around = (j) => { const u = j / SEGMENTS, phi = u * TAU; return [u, Math.sin(phi), Math.cos(phi)]; };
      for (let j = 0; j <= SEGMENTS; j++) {
        const [u, s, c] = around(j), n = normalize([s, slope, c]);
        m.vertex([top * s, h, top * c], n, [u, 0]);
        m.vertex([bottom * s, -h, bottom * c], n, [u, 1]);
      }
      for (let j = 0; j < SEGMENTS; j++) {
        const t0 = j * 2, b0 = j * 2 + 1, t1 = j * 2 + 2, b1 = j * 2 + 3;
        if (top > 0) m.triangle(t0, b0, t1);
        m.triangle(t1, b0, b1);
      }
      for (const [radius, y, ny] of [[top, h, 1], [bottom, -h, -1]]) {
        if (radius <= 0) continue;
        const center = m.vertex([0, y, 0], [0, ny, 0], [0.5, 0.5]), first = center + 1;
        for (let j = 0; j <= SEGMENTS; j++) {
          const [, s, c] = around(j);
          m.vertex([radius * s, y, radius * c], [0, ny, 0], [0.5 + 0.5 * s, 0.5 + 0.5 * c * -ny]);
        }
        for (let j = 0; j < SEGMENTS; j++) {
          if (ny > 0) m.triangle(center, first + j, first + j + 1);
          else m.triangle(center, first + j + 1, first + j);
        }
      }
      return m;
    }
    /** A torus in the XZ plane: SEGMENTS around the ring, RINGS around the tube. */
    function torus(radius, tube) {
      const m = meshData();
      for (let i = 0; i <= SEGMENTS; i++) {
        const u = i / SEGMENTS, phi = u * TAU, s = Math.sin(phi), c = Math.cos(phi);
        for (let j = 0; j <= RINGS; j++) {
          const v = j / RINGS, theta = v * TAU, ts = Math.sin(theta), tc = Math.cos(theta);
          // Outward from the tube's center circle.
          m.vertex([s * (radius + tube * tc), tube * ts, c * (radius + tube * tc)], [s * tc, ts, c * tc], [u, v]);
        }
      }
      const row = RINGS + 1;
      for (let i = 0; i < SEGMENTS; i++) {
        for (let j = 0; j < RINGS; j++) {
          const a = i * row + j, b = i * row + j + 1, c = (i + 1) * row + j, d = (i + 1) * row + j + 1;
          m.triangle(a, c, b);
          m.triangle(b, c, d);
        }
      }
      return m;
    }
    /** The triangles of a mesh ref; a blob's from its 'M' blob (`blob(id, kind)`: decoded, or null while it has not arrived). */
    function generate(mesh, blob) {
      switch (mesh.type) {
        case "box": return cuboid(mesh.size);
        case "sphere": return sphere(mesh.radius);
        case "plane": return plane(mesh.size);
        case "cylinder": return cylinder(mesh.radius, mesh.radius, mesh.height);
        case "cone": return cylinder(mesh.radius, 0, mesh.height);
        case "torus": return torus(mesh.radius, mesh.tube);
        default: {
          const m = meshData(), b = blob ? blob(mesh.id, "M") : null;
          if (!b) return m;
          m.positions = b.positions;
          m.normals = b.normals;
          m.uvs = b.uvs || new Float32Array(b.count * 2);
          m.colors = b.colors;
          m.joints = b.joints;
          m.weights = b.weights;
          // `decodeMesh` checked the indices; whole triangles only.
          m.indices = b.indices.length % 3 ? b.indices.subarray(0, b.indices.length - (b.indices.length % 3)) : b.indices;
          return m;
        }
      }
    }
    /** Where the ray `origin + t · dir` crosses triangle `a b c` (either face): `t`, or null (Möller–Trumbore). */
    function rayTriangle(origin, dir, a, b, c) {
      const EDGE = 1e-5;
      const e1 = sub(b, a), e2 = sub(c, a), p = cross(dir, e2), det = dot(e1, p);
      if (Math.abs(det) < 1e-9) return null;
      const inv = 1 / det, s = sub(origin, a), u = dot(s, p) * inv;
      if (!(u >= -EDGE && u <= 1 + EDGE)) return null;
      const q = cross(s, e1), v = dot(dir, q) * inv;
      if (v < -EDGE || u + v > 1 + EDGE) return null;
      const t = dot(e2, q) * inv;
      return t > 1e-6 ? t : null;
    }

    /**
     * `rayTriangle` over every triangle of `ix` (flat positions `P`), inlined: the nearest `t` below
     * `limit`, or null. (A model has 100k triangles; taps test them all.)
     */
    function rayTriangles(o, d, P, ix, limit) {
      const EDGE = 1e-5;
      let best = null;
      for (let k = 0; k + 2 < ix.length; k += 3) {
        const a = ix[k] * 3, b = ix[k + 1] * 3, c = ix[k + 2] * 3, ax = P[a], ay = P[a + 1], az = P[a + 2];
        const e1x = P[b] - ax, e1y = P[b + 1] - ay, e1z = P[b + 2] - az, e2x = P[c] - ax, e2y = P[c + 1] - ay, e2z = P[c + 2] - az;
        const px = d[1] * e2z - d[2] * e2y, py = d[2] * e2x - d[0] * e2z, pz = d[0] * e2y - d[1] * e2x;
        const det = e1x * px + e1y * py + e1z * pz;
        if (Math.abs(det) < 1e-9) continue;
        const inv = 1 / det, sx = o[0] - ax, sy = o[1] - ay, sz = o[2] - az;
        const u = (sx * px + sy * py + sz * pz) * inv;
        if (!(u >= -EDGE && u <= 1 + EDGE)) continue;
        const qx = sy * e1z - sz * e1y, qy = sz * e1x - sx * e1z, qz = sx * e1y - sy * e1x;
        const v = (d[0] * qx + d[1] * qy + d[2] * qz) * inv;
        if (v < -EDGE || u + v > 1 + EDGE) continue;
        const t = (e2x * qx + e2y * qy + e2z * qz) * inv;
        if (t > 1e-6 && t < (best === null ? limit : best)) best = t;
      }
      return best;
    }

    // --- player.rs: the retained graph, orbit, spins, transitions, clips, skins, culling, shadows, hits.
    const DEGREES_PER_POINT = 0.5, MAX_PITCH = 89, TAP_SLOP = 6, MAX_LIGHTS = 8, SHADOW_MAP_SIZE = 2048;
    /** The eased progress at `p` of a CSS `cubic-bezier` `[x1, y1, x2, y2]`: x solved by bisection. */
    function ease(bezier, p) {
      p = clamp(p, 0, 1);
      if (p === 0 || p === 1) return p;
      const [x1, y1, x2, y2] = bezier;
      const at = (a, b, t) => { const u = 1 - t; return 3 * u * u * t * a + 3 * u * t * t * b + t * t * t; };
      let lo = 0, hi = 1;
      for (let i = 0; i < 24; i++) {
        const mid = (lo + hi) / 2;
        if (at(x1, x2, mid) < p) lo = mid; else hi = mid;
      }
      return at(y1, y2, (lo + hi) / 2);
    }
    const lerpRgba = (a, b, t) => {
      const mix = (x, y) => x.map((v, i) => lerp(v, y[i], t));
      return { light: mix(a.light, b.light), dark: a.dark || b.dark ? mix(resolve(a, true), resolve(b, true)) : null };
    };
    /** The animatable part of a node. */
    const poseOf = (n) => ({
      position: n.pos, rotation: n.rot, quat: n.quat, scale: n.scale, color: n.mat.color,
      emissive: n.mat.emissive || BLACK, metalness: n.mat.metal, roughness: n.mat.rough,
    });
    function poseLerp(a, b, t) {
      let rotation, quat;
      if (a.quat === null && b.quat === null) {
        // Degrees, like v1 and 2D `rotate`: every degree in between.
        rotation = lerp3(a.rotation, b.rotation, t);
        quat = null;
      } else {
        rotation = b.rotation;
        quat = slerp(a.quat || quatFromEuler(a.rotation), b.quat || quatFromEuler(b.rotation), t);
      }
      return {
        position: lerp3(a.position, b.position, t), rotation, quat, scale: lerp3(a.scale, b.scale, t),
        color: lerpRgba(a.color, b.color, t), emissive: lerpRgba(a.emissive, b.emissive, t),
        metalness: lerp(a.metalness, b.metalness, t), roughness: lerp(a.roughness, b.roughness, t),
      };
    }
    /** What clips set on a node (each part replaces the node's own; null: not set). */
    const clipPose = () => ({ translation: null, rotation: null, scale: null });
    const sameClipPose = (a, b) => !!a && !!b && sameArray(a.translation, b.translation) && sameArray(a.rotation, b.rotation) && sameArray(a.scale, b.scale);
    /** A clip playing on a node. */
    class Track {
      constructor(clip, now, speed, looping) {
        this.clip = clip;
        this.base = speed < 0 ? clip.duration : 0; // clip time at `anchor`
        this.anchor = now;
        this.speed = speed;
        this.looping = looping;
        this.targets = []; // the node each channel animates (resolved after every update)
      }
      /** Unbounded clip time at `now`. */
      rawTime(now) { return this.base + (now - this.anchor) * this.speed; }
      /** The time to sample at `now`. */
      time(now) {
        const d = this.clip.duration, t = this.rawTime(now);
        if (d <= 0) return 0;
        return this.looping ? ((t % d) + d) % d : clamp(t, 0, d);
      }
      /** Still moving at `now` (looping, or not past its end). */
      running(now) {
        const t = this.rawTime(now), d = this.clip.duration;
        return d > 0 && this.speed !== 0 && (this.looping || (this.speed > 0 && t < d) || (this.speed < 0 && t > 0));
      }
      /** Changes the rate without a jump in clip time. */
      setSpeed(speed, now) {
        if (speed !== this.speed) { this.base = this.rawTime(now); this.anchor = now; this.speed = speed; }
      }
      /** Adds this clip's values at `now` to `out` (by target node). */
      sample(now, out) {
        const t = this.time(now);
        this.clip.channels.forEach((c, k) => {
          const target = this.targets[k];
          if (target === null || target === undefined) return;
          const v = sampleChannel(c, t);
          let pose = out.get(target);
          if (!pose) out.set(target, (pose = clipPose()));
          if (c.property === 0) pose.translation = [v[0], v[1], v[2]];
          else if (c.property === 1) pose.rotation = v;
          else pose.scale = [v[0], v[1], v[2]];
        });
      }
    }
    /** The camera as an orbit around its target (yaw 0: on +Z of it). */
    function orbitOf(c) {
      const o = sub(c.pos, c.target), d = Math.max(length(o), 1e-4);
      return { yaw: deg(Math.atan2(o[0], o[2])), pitch: deg(Math.asin(clamp(o[1] / d, -1, 1))), distance: d, target: c.target, fov: c.fov, ortho: c.ortho };
    }
    function orbitEye(o) {
      const y = rad(o.yaw), p = rad(o.pitch);
      return add(o.target, [o.distance * Math.cos(p) * Math.sin(y), o.distance * Math.sin(p), o.distance * Math.cos(p) * Math.cos(y)]);
    }
    function orbitLerp(a, b, t) {
      // The shorter way around.
      const dyaw = ((((b.yaw - a.yaw + 540) % 360) + 360) % 360) - 180;
      return {
        yaw: a.yaw + dyaw * t, pitch: lerp(a.pitch, b.pitch, t), distance: lerp(a.distance, b.distance, t),
        target: lerp3(a.target, b.target, t), fov: lerp(a.fov, b.fov, t),
        ortho: a.ortho !== null && b.ortho !== null ? lerp(a.ortho, b.ortho, t) : b.ortho,
      };
    }
    /** Variant (instanced, skinned, depth only) as a number ordered like `Variant`'s `Ord`. */
    const variantOf = (d) => (d.instances ? 4 : 0) | (d.joints ? 2 : 0);
    const compareText = (a, b) => (a === b ? 0 : a === null ? -1 : b === null ? 1 : a < b ? -1 : 1);
    /** A row of the node table. */
    const newRow = (flat, now) => ({
      flat, parent: null, end: 0, children: [], named: null, part: null, visible: true,
      meshKey: flat.mesh ? flat.mesh.key : "", appeared: now, target: poseOf(flat), from: null,
      clipPose: null, playing: null, jointNodes: [], joints: [], local: IDENTITY, world: IDENTITY,
      normal: [1, 0, 0, 0, 1, 0, 0, 0, 1], bounds: [0, 0, 0, 0, 0, 0], moved: 0, dirty: true,
    });

    /**
     * The device side of one scene element (player.rs' `ScenePlayer`; times in seconds, sizes in
     * points). `blob(id, kind)` gives a decoded blob, or null while it has not arrived; when one
     * arrives, `blobsArrived(now)` resolves what waited for it (a clip that had not arrived starts
     * then) and the next frame shows it.
     */
    class ScenePlayer {
      constructor(now, blob) {
        this.blob = blob || (() => null);
        this.settings = settings({});
        this.fresh = true; // no update applied yet: the first one does not animate
        this.nodes = [];
        this.index = new Map();
        this.drawNodes = []; // visible nodes with a mesh
        this.transitioning = []; // nodes with a running transition
        this.spinning = []; // visible nodes that spin
        this.players = []; // nodes playing clips
        this.skinned = []; // visible nodes with a resolved skin
        this.orbit = orbitOf(this.settings.camera);
        this.cameraFrom = null; // { orbit, start }
        this.minDistance = 0;
        this.maxDistance = 0;
        this.now = now;
        this.last = null;
        this.dragging = false;
        this.moving = false; // something but the camera moves (spin, transition, clip)
        this.tick = 0;
        this.clips = new Map();
        this.skins = new Map();
        this.instances = new Map();
        this.geometryCache = new Map();
      }
      /** Row `i`'s FlatNode (a draw's `node`). */
      node(i) { return this.nodes[i] ? this.nodes[i].flat : null; }
      /**
       * Shows an update at `now`: changed nodes and camera animate from where they are to their
       * new values if the scene has a transition; a changed camera replaces where the user moved
       * it. Clips whose `play` changed start (or crossfade) here.
       */
      apply(update, now) {
        this.now = now;
        if (update.settings) this.setSettings(update.settings, now);
        const t = this.fresh ? null : this.settings.t;
        this.fresh = false;
        const oldOrder = this.nodes.map((n) => n.flat.key);
        const rows = new Map(this.nodes.map((n) => [n.flat.key, n]));
        // Keys in their new order (removed ones simply find no row).
        let order;
        if (update.full) order = update.nodes.map((n) => n.key);
        else {
          for (const k of update.remove) rows.delete(k);
          order = update.order || oldOrder.concat(update.nodes.map((n) => n.key));
        }
        for (const flat of update.nodes) {
          const row = rows.get(flat.key);
          if (row) this.updateRow(row, flat, now, t);
          else {
            const fresh = newRow(flat, now);
            this.startPlay(fresh, null, now);
            rows.set(flat.key, fresh);
          }
        }
        const seen = new Set();
        this.nodes = [];
        for (const k of order) {
          if (seen.has(k)) continue;
          seen.add(k);
          const row = rows.get(k);
          if (row) { rows.delete(k); this.nodes.push(row); }
        }
        this.relink();
        this.refresh(now, true);
      }
      setSettings(s, now) {
        if (this.fresh) {
          this.orbit = orbitOf(s.camera);
          this.setLimits(s);
        } else if (!same(s.camera, this.settings.camera)) {
          const to = orbitOf(s.camera), from = this.currentOrbit(now);
          this.orbit = to;
          this.cameraFrom = s.t && s.t.ms > 0 ? { orbit: from, start: now + s.t.delay / 1000 } : null;
          this.setLimits(s);
        } else if (!same(s.orbit, this.settings.orbit)) {
          this.setLimits(s);
        }
        this.settings = s;
      }
      setLimits(s) {
        const d = orbitOf(s.camera).distance, o = s.orbit || { min: 0, max: 0 };
        this.minDistance = o.min > 0 ? o.min : d * 0.25;
        this.maxDistance = o.max > 0 ? o.max : d * 4;
      }
      /** A node of the update replaces its row's props. */
      updateRow(row, flat, now, t) {
        const pose = poseOf(flat);
        if (!same(pose, row.target)) {
          const current = this.poseAt(row, now);
          row.from = t && t.ms > 0 && !same(current, pose) ? { pose: current, start: now + t.delay / 1000 } : null;
          row.target = pose;
        }
        if (!same(flat.mesh, row.flat.mesh)) row.meshKey = flat.mesh ? flat.mesh.key : "";
        const old = row.flat;
        row.flat = flat;
        this.startPlay(row, old.play, now);
      }
      /** Starts, restarts, crossfades, retimes or stops a row's clip after its `play` changed from `old`. */
      startPlay(row, old, now) {
        const p = row.flat.play;
        if (!p) { row.playing = null; return; }
        const clip = this.clip(p.clip);
        const unchanged = !!old && old.clip === p.clip && old.run === p.run;
        if (row.playing && unchanged) {
          const track = row.playing.current;
          if (track) { track.setSpeed(p.speed, now); track.looping = p.loop; }
        } else {
          const was = row.playing;
          const previous = was && p.fade > 0 && was.current ? { track: was.current, start: now, fade: p.fade } : null;
          // `current` is null while its blob has not arrived; `live`: sample once more after it stops.
          row.playing = { current: clip ? new Track(clip, now, p.speed, p.loop) : null, previous, live: true };
        }
      }
      /** The decoded clip `id` (from the cache, or its blob). */
      clip(id) { return this.clips.get(id) || this.blob(id, "A") || null; }
      /** Blobs arrived: clips that waited for theirs start now, skins and instance sets resolve. */
      blobsArrived(now) {
        for (const row of this.nodes) {
          const p = row.flat.play;
          if (!p || !row.playing || row.playing.current) continue;
          const clip = this.clip(p.clip);
          if (clip) { row.playing.current = new Track(clip, now, p.speed, p.loop); row.playing.live = true; }
        }
        this.relink();
        this.refresh(now, true);
      }
      /**
       * Rebuilds the table's links after an update: parents, subtrees, named and labeled
       * ancestors, visibility, the lists of moving nodes, clip targets and skin joints. O(nodes).
       */
      relink() {
        // Pre-order from the parent keys, whatever order the rows came in (rows whose parent is
        // missing go to the top level).
        let n = this.nodes.length;
        const keys = new Map();
        this.nodes.forEach((r, i) => keys.set(r.flat.key, i));
        const kids = this.nodes.map(() => []), roots = [];
        this.nodes.forEach((r, i) => {
          const p = r.flat.parent !== null ? keys.get(r.flat.parent) : undefined;
          if (p !== undefined && p !== i) kids[p].push(i); else roots.push(i);
        });
        const order = [], stack = roots.slice().reverse(), placed = new Array(n).fill(false);
        while (stack.length) {
          const i = stack.pop();
          if (placed[i]) continue;
          placed[i] = true;
          order.push(i);
          for (let k = kids[i].length - 1; k >= 0; k--) stack.push(kids[i][k]);
        }
        if (order.length !== n || order.some((a, b) => a !== b)) {
          const rows = this.nodes;
          this.nodes = order.map((i) => rows[i]);
          n = this.nodes.length;
        }
        this.index = new Map(this.nodes.map((r, i) => [r.flat.key, i]));
        for (let i = 0; i < n; i++) {
          const row = this.nodes[i];
          let parent = row.flat.parent !== null ? this.index.get(row.flat.parent) : undefined;
          if (parent === undefined || !(parent < i)) parent = null;
          const pn = parent !== null ? this.nodes[parent] : null;
          row.parent = parent;
          row.end = i + 1;
          row.children = [];
          row.named = row.flat.name !== null ? i : pn ? pn.named : null;
          row.part = row.flat.label !== null ? i : row.flat.name !== null ? null : pn ? pn.part : null;
          row.visible = (pn ? pn.visible : true) && !row.flat.hidden;
          row.clipPose = null;
          row.dirty = true;
        }
        for (let i = n - 1; i >= 0; i--) {
          const p = this.nodes[i].parent;
          if (p !== null) this.nodes[p].end = Math.max(this.nodes[p].end, this.nodes[i].end);
        }
        for (let i = 0; i < n; i++) {
          const p = this.nodes[i].parent;
          if (p !== null) this.nodes[p].children.push(i);
        }

        // Blobs in use (decoded once per page by the blob store).
        const clips = new Map(), skins = new Map(), instances = new Map();
        for (const row of this.nodes) {
          const p = row.flat.play, s = row.flat.skin, inst = row.flat.inst;
          if (p) { const c = this.clip(p.clip); if (c) clips.set(p.clip, c); }
          if (s) { const k = this.skins.get(s.id) || this.blob(s.id, "K"); if (k) skins.set(s.id, k); }
          if (inst) { const set = this.instances.get(inst.id) || this.blob(inst.id, "I"); if (set) instances.set(inst.id, set); }
        }
        this.clips = clips;
        this.skins = skins;
        this.instances = instances;
        const meshKeys = new Set(this.nodes.map((r) => r.meshKey));
        for (const k of [...this.geometryCache.keys()]) if (!meshKeys.has(k)) this.geometryCache.delete(k);

        // Clip targets and skin joints.
        for (let i = 0; i < n; i++) {
          const row = this.nodes[i];
          if (row.playing) {
            for (const track of [row.playing.current, row.playing.previous && row.playing.previous.track]) {
              if (track) track.targets = track.clip.channels.map((c) => this.resolve(i, c.target));
            }
          }
          const s = row.flat.skin;
          let joints = [];
          if (s) {
            const skin = this.skins.get(s.id), root = this.index.get(s.root);
            if (skin && root !== undefined) joints = skin.joints.map((path) => this.resolve(root, path));
          }
          row.jointNodes = joints;
        }
        const rows = this.nodes, all = rows.map((_, i) => i);
        this.drawNodes = all.filter((i) => rows[i].visible && rows[i].flat.mesh);
        this.spinning = all.filter((i) => rows[i].visible && !isZero(rows[i].flat.spin));
        this.transitioning = all.filter((i) => rows[i].from);
        this.players = all.filter((i) => rows[i].playing);
        this.skinned = all.filter((i) => rows[i].visible && rows[i].jointNodes.length > 0);
      }
      /** The node a clip target or joint path names, from node `from`: "" itself, "0/2" a child-index path, "#name" a named node. */
      resolve(from, target) {
        if (target.startsWith("#")) { const i = this.index.get(target); return i === undefined ? null : i; }
        let at = from;
        for (const step of target.split("/")) {
          if (!step) continue;
          if (!/^\d+$/.test(step)) return null;
          const child = this.nodes[at].children[Number(step)];
          if (child === undefined) return null;
          at = child;
        }
        return at;
      }
      transitionProgress(start, now) {
        const t = this.settings.t;
        if (!t || t.ms <= 0) return null;
        const p = ((now - start) * 1000) / t.ms;
        return p < 1 ? ease(t.bezier, Math.max(p, 0)) : null;
      }
      poseAt(n, now) {
        const p = n.from ? this.transitionProgress(n.from.start, now) : null;
        return p === null ? n.target : poseLerp(n.from.pose, n.target, p);
      }
      currentOrbit(now) {
        const p = this.cameraFrom ? this.transitionProgress(this.cameraFrom.start, now) : null;
        return p === null ? this.orbit : orbitLerp(this.cameraFrom.orbit, this.orbit, p);
      }
      /** The finger or mouse button went down (auto-rotation pauses). */
      pointerDown() { this.dragging = true; }
      /** Moved by `dx`, `dy` points while down: right turns the scene right, down shows more of its top. */
      drag(dx, dy) {
        if (!(this.settings.orbit && this.settings.orbit.rotate)) return;
        this.settleCamera();
        this.orbit.yaw -= dx * DEGREES_PER_POINT;
        this.orbit.pitch = clamp(this.orbit.pitch + dy * DEGREES_PER_POINT, -MAX_PITCH, MAX_PITCH);
      }
      pointerUp() { this.dragging = false; }
      /** Pinch or scroll: `factor` above 1 moves closer. */
      zoom(factor) {
        if (!(this.settings.orbit && this.settings.orbit.zoom) || !(factor > 0)) return;
        this.settleCamera();
        if (this.orbit.ortho !== null) this.orbit.ortho /= factor;
        else this.orbit.distance = clamp(this.orbit.distance / factor, this.minDistance, this.maxDistance);
      }
      /** Two-finger drag: moves the target with the content (`height`: the element's, points). */
      pan(dx, dy, height) {
        if (!(this.settings.orbit && this.settings.orbit.pan) || !(height > 0)) return;
        this.settleCamera();
        const o = this.orbit, eye = orbitEye(o);
        const f = normalize(sub(o.target, eye)), right = normalize(cross(f, [0, 1, 0])), up = cross(right, f);
        const visible = o.ortho !== null ? o.ortho : 2 * o.distance * Math.tan(rad(o.fov) / 2);
        const k = visible / height;
        o.target = add(o.target, add(scale(right, -dx * k), scale(up, dy * k)));
      }
      /** A gesture takes over from a running camera transition. */
      settleCamera() {
        if (this.cameraFrom) {
          this.orbit = { ...this.currentOrbit(this.now) };
          this.cameraFrom = null;
        }
      }
      /** Moves time to `now`: auto-rotation turns by the time since the last frame; spins, transitions and clips move the nodes. */
      advance(now) {
        const dt = this.last === null ? 0 : clamp(now - this.last, 0, 0.25);
        this.last = now;
        this.now = now;
        const o = this.settings.orbit;
        if (o && o.auto !== 0 && !this.dragging) {
          this.orbit.yaw += o.auto * dt;
          if (this.cameraFrom) this.cameraFrom.orbit = { ...this.cameraFrom.orbit, yaw: this.cameraFrom.orbit.yaw + o.auto * dt };
        }
        const cf = this.cameraFrom;
        if (cf && now >= cf.start && this.transitionProgress(cf.start, now) === null) this.cameraFrom = null;
        this.refresh(now, false);
      }
      /**
       * Recomputes what moved at `now`: clip poses, then the local matrices of moving nodes and
       * the worlds of their subtrees, then skins. `all`: every node (after an update).
       */
      refresh(now, all) {
        const tick = ++this.tick, nodes = this.nodes;
        let moving = false;
        // Rows whose local pose changes now (each frame: the moving ones only).
        let touched = [];
        // Fades that are over: the nodes only the old clip moved go back to their own pose.
        for (const p of this.players) {
          const playing = nodes[p].playing, prev = playing && playing.previous;
          if (!prev || !(now - prev.start >= prev.fade)) continue;
          playing.previous = null;
          playing.live = true;
          for (const t of prev.track.targets) {
            if (t === null) continue;
            nodes[t].clipPose = null;
            nodes[t].dirty = true;
            touched.push(t);
          }
        }
        // Clips: sample every running track (and, after an update, the finished ones too: their last pose holds).
        const poses = new Map(), live = [];
        for (const p of this.players) {
          const playing = nodes[p].playing;
          if (!playing) continue;
          const running = !!playing.previous || (!!playing.current && playing.current.running(now));
          live.push([p, running]);
          if (!running && !playing.live && !all) continue;
          moving = moving || running;
          const fresh = new Map();
          if (playing.current) playing.current.sample(now, fresh);
          const prev = playing.previous;
          if (prev) {
            const w = clamp((now - prev.start) / prev.fade, 0, 1), old = new Map();
            prev.track.sample(now, old);
            for (const [target, a] of old) {
              const b = fresh.get(target) || clipPose();
              fresh.delete(target);
              poses.set(target, this.blend(target, a, b, w, now));
            }
            for (const [target, b] of fresh) poses.set(target, this.blend(target, clipPose(), b, w, now));
          } else {
            for (const [target, pose] of fresh) poses.set(target, pose);
          }
        }
        for (const [p, running] of live) if (nodes[p].playing) nodes[p].playing.live = running;
        for (const [target, pose] of poses) {
          const row = nodes[target];
          if (!sameClipPose(row.clipPose, pose)) {
            row.clipPose = pose;
            row.dirty = true;
            touched.push(target);
          }
        }

        // Spins and transitions.
        for (const i of this.spinning) { nodes[i].dirty = true; touched.push(i); moving = true; }
        const done = [];
        for (const i of this.transitioning) {
          const row = nodes[i];
          row.dirty = true;
          touched.push(i);
          if (row.from && (now < row.from.start || this.transitionProgress(row.from.start, now) !== null)) moving = true;
          else done.push(i);
        }
        if (done.length) {
          for (const i of done) nodes[i].from = null;
          this.transitioning = this.transitioning.filter((i) => !done.includes(i));
        }

        // Locals, then the worlds of the subtrees of the touched rows (in pre-order, a subtree is
        // a range: one pass covers nested ones).
        if (all) touched = nodes.map((_, i) => i);
        else touched = [...new Set(touched)].sort((a, b) => a - b);
        let covered = 0;
        for (const i of touched) {
          if (i < covered) continue;
          covered = nodes[i].end;
          for (let j = i; j < covered; j++) {
            if (all || nodes[j].dirty) { nodes[j].local = this.local(j, now); nodes[j].dirty = false; }
            this.updateWorld(j, tick);
          }
        }

        // Skins whose joints or node moved.
        for (const s of this.skinned) {
          const row = nodes[s];
          const moved = all || row.moved === tick || row.jointNodes.some((j) => j !== null && nodes[j].moved === tick);
          if (!moved) continue;
          const skin = row.flat.skin && this.skins.get(row.flat.skin.id);
          if (!skin) continue;
          // Where each joint puts the mesh: world · J_i = world(joint i) · inverse_bind[i] (the node's own transform places nothing).
          const placed = row.jointNodes.map((j, k) => (j !== null ? mul(nodes[j].world, skin.inverseBind[k] || IDENTITY) : row.world));
          const inverse = invert(row.world);
          row.joints = placed.map((p) => mul(inverse, p));
          // Every skinned vertex is a weighted mean of `world · J_i · v`, so the union of the mesh box under each `world · J_i` holds them.
          if (row.flat.mesh && !row.flat.inst && placed.length) {
            const b = row.flat.mesh.bounds;
            let box = transformAabb(placed[0], b);
            for (let k = 1; k < placed.length; k++) box = aabbUnion(box, transformAabb(placed[k], b));
            row.bounds = box;
          }
        }
        this.moving = moving;
      }
      /** Blends two clips' poses of node `target` (`w`: the new one's weight); a part only one sets blends with the node's own pose. */
      blend(target, a, b, w, now) {
        const own = this.poseAt(this.nodes[target], now);
        const ownQ = own.quat || quatFromEuler(own.rotation);
        const mix3 = (x, y, base) => (x || y ? lerp3(x || base, y || base, w) : null);
        return {
          translation: mix3(a.translation, b.translation, own.position),
          rotation: a.rotation || b.rotation ? slerp(a.rotation || ownQ, b.rotation || ownQ, w) : null,
          scale: mix3(a.scale, b.scale, own.scale),
        };
      }
      /** Row `i`'s local matrix at `now`. */
      local(i, now) {
        const row = this.nodes[i], pose = this.poseAt(row, now), clip = row.clipPose || clipPose();
        const position = clip.translation || pose.position, size = clip.scale || pose.scale, spin = row.flat.spin;
        const turned = !isZero(spin) && row.visible ? scale(spin, now - row.appeared) : [0, 0, 0];
        const q = clip.rotation || pose.quat;
        if (q) return composeQuat(position, isZero(turned) ? q : quatMul(q, quatFromEuler(turned)), size);
        return compose(position, add(pose.rotation, turned), size);
      }
      /** Row `i`'s world matrix from its parent's, and what follows from it. */
      updateWorld(i, tick) {
        const row = this.nodes[i];
        const world = row.parent !== null ? mul(this.nodes[row.parent].world, row.local) : row.local;
        if (!sameArray(row.world, world) || row.moved === 0) {
          row.world = world;
          row.moved = tick;
          row.normal = normalMatrix(world);
        }
        // A skinned mesh's box follows its joints (set with them, after the worlds); until they resolve, its node places it.
        const skinned = row.jointNodes.length > 0 && !row.flat.inst;
        if (row.flat.mesh && !skinned) row.bounds = transformAabb(world, row.flat.inst ? row.flat.inst.bounds : row.flat.mesh.bounds);
      }
      /** The frame at the time of the last `advance`, for `width` × `height` points, dark or not. */
      frame(width, height, dark) {
        const now = this.now, cam = this.currentOrbit(now), eye = orbitEye(cam), c = this.settings.camera;
        const aspect = height > 0 ? width / height : 1;
        const projection = cam.ortho !== null ? orthographic(cam.ortho, aspect, c.near, c.far) : perspective(cam.fov, aspect, c.near, c.far);
        const view = lookAt(eye, cam.target, [0, 1, 0]);
        const viewProj = mul(projection, view);
        let clear = [0, 0, 0, 0];
        if (this.settings.bg) {
          const [r, g, b, a] = resolve(this.settings.bg, dark);
          clear = [r * a, g * a, b * a, a];
        }
        let ambient = [0, 0, 0], shadowLight = null;
        const lights = [];
        for (const l of effectiveLights(this.settings)) {
          const [r, g, b] = resolve(l.color, dark);
          const color = [r, g, b].map((v) => srgbToLinear(v) * l.intensity);
          const half = rad(Math.max(l.angle, 1) / 2);
          let kind, params;
          if (l.type === "ambient") { ambient = add(ambient, color); continue; }
          else if (l.type === "directional") { kind = 1; params = [0, 0, 0, 0]; }
          else if (l.type === "point") { kind = 2; params = [l.range, -1, -1, 0]; }
          else if (l.type === "spot") { kind = 3; params = [l.range, Math.cos(half), Math.cos(half * 0.75), 0]; }
          else continue;
          if (lights.length < MAX_LIGHTS) {
            if (kind === 1 && l.shadows && shadowLight === null) shadowLight = lights.length;
            const d = normalize(l.dir);
            lights.push({ kind, color, position: l.pos, direction: isZero(d) ? [0, -1, 0] : d, params });
          }
        }
        const planes = frustumPlanes(viewProj), opaque = [], transparent = [];
        for (const i of this.drawNodes) {
          const d = this.draw(i, dark);
          if (!d || !aabbInFrustum(planes, d.bounds)) continue;
          (d.transparent ? transparent : opaque).push(d);
        }
        // Fewer program, texture and buffer switches.
        opaque.sort((a, b) => variantOf(a) - variantOf(b) || compareText(a.material.texture, b.material.texture) || compareText(a.meshKey, b.meshKey));
        // Back to front: farthest from the camera first.
        const depth = (d) => { const b = d.bounds; return length(sub([(b[0] + b[3]) / 2, (b[1] + b[4]) / 2, (b[2] + b[5]) / 2], eye)); };
        transparent.sort((a, b) => depth(b) - depth(a));
        const draws = opaque.concat(transparent);
        const shadow = shadowLight !== null ? shadowPass(shadowLight, lights[shadowLight], draws) : null;
        const e = this.settings.env, o = this.settings.orbit;
        return {
          width, height, view, projection, viewProj, inverseViewProj: invert(viewProj), cameraPosition: eye, clear, ambient, lights,
          draws, shadow, env: e ? { id: e.id, intensity: e.intensity, background: e.background } : null,
          exposure: this.settings.exposure,
          animating: this.moving || !!this.cameraFrom || !!(o && o.auto !== 0 && !this.dragging),
        };
      }
      /** Row `i`'s draw (null: invisible, alpha 0). */
      draw(i, dark) {
        const row = this.nodes[i], mesh = row.flat.mesh;
        if (!mesh) return null;
        const m = row.flat.mat, pose = this.poseAt(row, this.now);
        const [r, g, b, ca] = resolve(pose.color, dark), [er, eg, eb] = resolve(pose.emissive, dark);
        const a = m.alpha === "opaque" ? 1 : ca;
        // Invisible: neither drawn nor hit.
        if (a <= 0) return null;
        const lin = srgbToLinear;
        return {
          node: i, key: row.flat.key, name: row.named !== null ? this.nodes[row.named].flat.name : null,
          mesh, meshKey: row.meshKey, world: row.world, normal: row.normal,
          material: {
            color: [lin(r), lin(g), lin(b), a], emissive: [er, eg, eb].map(lin), metalness: pose.metalness, roughness: pose.roughness,
            unlit: m.unlit, doubleSided: m.double, texture: m.map, mrMap: m.mr_map, normalMap: m.normal_map, normalScale: m.normal_scale,
            emissiveMap: m.emissive_map, aoMap: m.ao_map, alpha: m.alpha, alphaCutoff: m.cutoff,
          },
          transparent: m.alpha === "blend" && a < 1,
          instances: row.flat.inst ? { id: row.flat.inst.id, count: row.flat.inst.count } : null,
          joints: row.jointNodes.length > 0 && row.joints.length === row.jointNodes.length ? row.joints : null,
          castShadows: row.flat.cast, receiveShadows: row.flat.recv, bounds: row.bounds,
        };
      }
      /** The triangles of a draw as stored (unskinned; cached by mesh key; empty while a blob mesh has not arrived). */
      geometry(d) {
        const cached = this.geometryCache.get(d.meshKey);
        if (cached) return cached;
        const data = generate(d.mesh, this.blob);
        // A blob that has not arrived is not cached: it is retried.
        if (data.vertexCount() > 0 || d.mesh.type !== "blob") this.geometryCache.set(d.meshKey, data);
        return data;
      }
      /** What a tap at `(x, y)` (points from the element's top-left) hits: `{ node, part, instance, point }`. */
      hit(x, y, width, height, dark) {
        const none = { node: null, part: null, instance: null, point: null };
        if (!(width > 0) || !(height > 0)) return none;
        const frame = this.frame(width, height, dark);
        const nx = (2 * x) / width - 1, ny = 1 - (2 * y) / height;
        const near = transformPoint(frame.inverseViewProj, [nx, ny, -1]), far = transformPoint(frame.inverseViewProj, [nx, ny, 1]);
        const dir = sub(far, near);
        // Nearest so far: { t (ray parameter), node, instance }.
        let best = null;
        const closer = (t) => !best || t < best.t;
        for (const d of frame.draws) {
          const entry = rayAabb(near, dir, d.bounds);
          if (entry === null || !closer(entry)) continue;
          const g = this.geometry(d), localBox = d.mesh.bounds, ix = g.indices;
          const skin = d.joints && g.joints && g.joints.length === g.vertexCount() * 4 ? d.joints : null;
          // `m`: where the mesh is (its world, times an instance's matrix).
          const test = (m, instance) => {
            if (skin) {
              // Skinned: its triangles in the current pose, in the world.
              const t = rayTriangles(near, dir, skinnedPositions(g, skin.map((j) => mul(m, j)), m), ix, best ? best.t : Infinity);
              if (t !== null) best = { t, node: d.node, instance };
              return;
            }
            const inv = invert(m), o = transformPoint(inv, near), v = transformDir(inv, dir);
            const inside = rayAabb(o, v, localBox);
            if (inside === null || !closer(inside)) return;
            const t = rayTriangles(o, v, g.positions, ix, best ? best.t : Infinity);
            if (t !== null) best = { t, node: d.node, instance };
          };
          const set = d.instances ? this.instances.get(d.instances.id) : null;
          if (set) for (let k = 0; k < set.count; k++) test(mul(d.world, set.matrix(k)), k);
          else test(d.world, null);
        }
        if (!best) return none;
        const row = this.nodes[best.node];
        return {
          node: row.named !== null ? this.nodes[row.named].flat.name : null,
          part: row.part !== null ? this.nodes[row.part].flat.label : null,
          instance: best.instance,
          point: add(near, scale(dir, best.t)),
        };
      }
    }
    /**
     * The vertices of a skinned mesh in the current pose, flat: vertex `i` is `Σ w_k · placed[j_k] · v`
     * (by `world` alone if its weights are all zero). The joint matrices are affine (no divide by w).
     */
    function skinnedPositions(g, placed, world) {
      const n = g.vertexCount(), out = new Float64Array(n * 3), P = g.positions, J = g.joints, W = g.weights;
      for (let i = 0; i < n; i++) {
        const x = P[i * 3], y = P[i * 3 + 1], z = P[i * 3 + 2];
        let ox = 0, oy = 0, oz = 0, total = 0;
        for (let k = 0; k < 4; k++) {
          const w = W[i * 4 + k] || 0, m = placed[J[i * 4 + k]];
          if (!m || w === 0) continue;
          ox += w * (m[0] * x + m[4] * y + m[8] * z + m[12]);
          oy += w * (m[1] * x + m[5] * y + m[9] * z + m[13]);
          oz += w * (m[2] * x + m[6] * y + m[10] * z + m[14]);
          total += w;
        }
        if (total === 0) [ox, oy, oz] = transformPoint(world, [x, y, z]);
        out[i * 3] = ox;
        out[i * 3 + 1] = oy;
        out[i * 3 + 2] = oz;
      }
      return out;
    }
    /** The shadow map fitted to the visible draws, seen along `light`. */
    function shadowPass(index, light, draws) {
      if (!draws.length) return null;
      let union = draws[0].bounds;
      for (let i = 1; i < draws.length; i++) union = aabbUnion(union, draws[i].bounds);
      const casters = [];
      draws.forEach((d, i) => { if (d.castShadows) casters.push(i); });
      if (!casters.length) return null;
      const center = [(union[0] + union[3]) / 2, (union[1] + union[4]) / 2, (union[2] + union[5]) / 2];
      const extent = length([union[3] - union[0], union[4] - union[1], union[5] - union[2]]) / 2;
      // The radius in steps of 1/16 of its power of two, and the center on whole texels: the map
      // does not shimmer while the scene stays put.
      const step = Math.pow(2, Math.ceil(Math.log2(Math.max(extent, 1e-3))) - 4);
      const radius = Math.ceil(Math.max(extent, 1e-3) / step) * step;
      const size = SHADOW_MAP_SIZE, texel = (2 * radius) / size, dir = light.direction;
      const up = Math.abs(dir[1]) > 0.99 ? [0, 0, 1] : [0, 1, 0];
      const rotate = lookAt([0, 0, 0], dir, up);
      const c = transformPoint(rotate, center);
      const snap = (v) => Math.floor(v / texel) * texel;
      const view = mul(translation([-snap(c[0]), -snap(c[1]), 0]), rotate);
      // One texel of margin for the snap; depth around the center (view −Z).
      const half = radius + texel;
      const projection = orthographic(2 * half, 1, -c[2] - radius, -c[2] + radius);
      return { light: index, viewProj: mul(projection, view), mapSize: size, bias: 1.5 / size, normalBias: 1.5 * texel, casters };
    }

    // --- shaders.rs' header, defines and limits (scene3d.test.mjs checks them); the GLSL below.
    const ES_HEADER = "#version 300 es\n";
    /** `Variant::defines`: one line per flag, in this order. */
    const variantDefines = (v) => (v.instanced ? "#define INSTANCED\n" : "") + (v.skinned ? "#define SKINNED\n" : "") + (v.depthOnly ? "#define DEPTH_ONLY\n" : "");
    /** `program_source`: the header, the variant's defines, then the stage's body ("vertex" or "fragment"). */
    const programSource = (header, stage, variant) => header + variantDefines(variant) + (stage === "vertex" ? VERTEX : FRAGMENT);
    // prism-core's GLSL (scene3d/shaders.rs), as JSON strings: the server puts them in
    // (`client_js`), so the web draws with the same shaders as every OpenGL backend.
    const VERTEX = __SCENE3D_VERTEX__;
    const FRAGMENT = __SCENE3D_FRAGMENT__;
    const SKY_VERTEX = __SCENE3D_SKY_VERTEX__;
    const SKY_FRAGMENT = __SCENE3D_SKY_FRAGMENT__;

    return {
      decodeUpdate, applyFlat, ScenePlayer, ease, generate, sampleChannel, decode, TAP_SLOP, SHADOW_MAP_SIZE, MAX_LIGHTS,
      ES_HEADER, VERTEX, FRAGMENT, SKY_VERTEX, SKY_FRAGMENT, programSource, variantDefines,
      math: {
        IDENTITY, mul, compose, composeQuat, rotation, rotationY, translation, invert, transformPoint, transformDir, lookAt, perspective,
        length, quatToMat4, quatFromEuler, quatNormalize, slerp, transformAabb, rayAabb, frustumPlanes, aabbInFrustum,
      },
      geometry: { cuboid, sphere, plane, cylinder, torus, rayTriangle },
    };
  }
  // </scene3d-core>

  let scene3d = null;
  const s3 = () => scene3d || (scene3d = scene3dCore());
  const scenes = new Set();
  /** While a reset rebuilds the page: the scenes it had, by node id (the new nodes take them over). */
  let sceneOrphans = null;
  /** The observers and listeners every scene shares (made with the first one). */
  let sceneShared = null;
  const sceneNow = () => performance.now() / 1000;

  // --- Blobs and images: fetched once per page, shared by every scene ------------
  // (GPU objects cannot be shared between canvases: each scene's context
  // uploads a blob once, and draws every node using it from that one buffer.)
  /** Blob id → `{ buf, decoded (kind → value), type, failed }`; `buf` null while it is on its way (or failed). */
  const sceneBlobs = new Map();
  /** Blob `id` decoded as `kind` (`M`, `I`, `A`, `K`, `E`, `P`), or null until it arrives (fetched on first ask). */
  function sceneBlob(id, kind) {
    let e = sceneBlobs.get(id);
    if (!e) {
      e = { buf: null, decoded: new Map(), type: "", failed: false };
      sceneBlobs.set(id, e);
      sceneFetch(id, e);
    }
    if (e.retry) { e.retry = false; if (e.failed) sceneFetch(id, e); }
    if (!e.buf) return null;
    if (!e.decoded.has(kind)) e.decoded.set(kind, s3().decode[kind](e.buf));
    return e.decoded.get(kind);
  }
  function sceneFetch(id, e) {
    if (typeof fetch !== "function") return;
    e.failed = false;
    fetch(`${BASE}/_prism/3d/${id}`)
      .then((r) => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        e.type = (r.headers && r.headers.get && r.headers.get("content-type")) || "";
        return r.arrayBuffer();
      })
      .then((buf) => { e.buf = buf; sceneArrived(); }, (err) => {
        // Not asked again on every frame; the next update that refers to it does.
        e.failed = true;
        console.error("prism: 3D blob", id, err);
      });
  }
  /** Something a scene waits for arrived: each re-resolves it at its next frame. */
  function sceneArrived() {
    scenes.forEach((st) => { st.stale = true; sceneRequest(st); });
  }
  /**
   * Asks for every blob an update refers to now (in parallel), rather than when first drawn, and
   * again for those that failed: a blob of the first paint may only exist on the server once the
   * live session renders the scene again.
   */
  function scenePrefetch(update) {
    for (const e of sceneBlobs.values()) if (e.failed) e.retry = true;
    const s = update.settings;
    if (s && s.env) sceneBlob(s.env.id, "E");
    for (const n of update.nodes) {
      if (n.mesh && n.mesh.type === "blob") sceneBlob(n.mesh.id, "M");
      if (n.inst) sceneBlob(n.inst.id, "I");
      if (n.play) sceneBlob(n.play.clip, "A");
      if (n.skin) sceneBlob(n.skin.id, "K");
      for (const src of sceneSources(n)) sceneImage(src);
    }
  }
  /** A node's texture sources. */
  const sceneSources = (n) => [n.mat.map, n.mat.mr_map, n.mat.normal_map, n.mat.emissive_map, n.mat.ao_map].filter(Boolean);
  /** Texture images by source: `{ image, started }`, decoded once per page. */
  const sceneImages = new Map();
  /**
   * A texture's image, or null until it loads (the scenes draw again then). `p3d:<id>`: an image
   * blob, decoded unflipped with straight alpha and no color conversion (uv (0, 0) is its first
   * row); other sources are URLs the server resolved, like pictures'.
   */
  function sceneImage(src) {
    let e = sceneImages.get(src);
    if (!e) sceneImages.set(src, (e = { image: null, started: false }));
    if (e.image || e.started) return e.image;
    const done = (image) => { e.image = image; sceneArrived(); };
    const fail = (err) => console.error("prism: 3D texture", src, err);
    if (src.startsWith("p3d:")) {
      const id = src.slice(4).toLowerCase();
      if (!/^[0-9a-f]{32}$/.test(id)) { e.started = true; return null; }
      const buf = sceneBlob(id, "P");
      if (!buf) return null; // asked again once the blob is here
      e.started = true;
      const blob = new Blob([buf], { type: sceneBlobs.get(id).type || "" });
      const element = () => new Promise((ok, no) => {
        const img = new Image(), url = URL.createObjectURL(blob);
        img.onload = () => { URL.revokeObjectURL(url); ok(img); };
        img.onerror = () => { URL.revokeObjectURL(url); no(new Error("undecodable image")); };
        img.src = url;
      });
      (typeof createImageBitmap === "function"
        ? createImageBitmap(blob, { imageOrientation: "none", premultiplyAlpha: "none", colorSpaceConversion: "none" }).catch(element)
        : element()).then(done, fail);
      return null;
    }
    e.started = true;
    if (typeof Image !== "function") return null;
    const img = new Image();
    // CORS: a remote image that allows it can be a texture; one that does not fails to load (untextured).
    img.crossOrigin = "anonymous";
    img.onload = () => done(img);
    img.onerror = () => fail(new Error("not loaded"));
    img.src = src;
    return null;
  }

  function sceneWatch() {
    if (sceneShared) return sceneShared;
    const dark = window.matchMedia ? matchMedia("(prefers-color-scheme: dark)") : null;
    if (dark && dark.addEventListener) dark.addEventListener("change", sceneAppearance);
    document.addEventListener("visibilitychange", () => scenes.forEach(sceneRequest));
    sceneShared = {
      dark,
      // The canvas' size in points (its backing store is that × devicePixelRatio).
      resize: typeof ResizeObserver === "function" ? new ResizeObserver((entries) => entries.forEach((e) => {
        const st = e.target._scene3d;
        if (!st) return;
        st.w = e.contentRect.width;
        st.h = e.contentRect.height;
        sceneRequest(st);
      })) : null,
      // Offscreen scenes do not draw.
      view: typeof IntersectionObserver === "function" ? new IntersectionObserver((entries) => entries.forEach((e) => {
        const st = e.target._scene3d;
        if (!st) return;
        st.visible = e.isIntersecting;
        if (st.visible) sceneRequest(st); else sceneCancel(st);
      })) : null,
    };
    return sceneShared;
  }

  /**
   * Whether `el` is in the dark appearance: its `color-scheme` (the app's
   * forced appearance, or `light dark`: the system's).
   */
  function sceneDark(el) {
    const scheme = String((getComputedStyle(el) || {}).colorScheme || "");
    const light = /\blight\b/.test(scheme), dark = /\bdark\b/.test(scheme);
    return dark && (!light || !!(sceneWatch().dark && sceneWatch().dark.matches));
  }
  /** The appearance changed: scenes resolve their adaptive colors again. */
  function sceneAppearance() {
    scenes.forEach((st) => { st.dark = null; sceneRequest(st); });
  }

  /** A scene node's update (`{"op":"scene"}`): the first one starts the scene, later ones go to its player. */
  function sceneUpdate(el, u) {
    const update = s3().decodeUpdate(u);
    if (!update) return;
    const st = el._scene || sceneAdopt(el);
    if (!st) return sceneStart(el, update);
    sceneApply(st, update);
  }
  /** Shows an update. With reduced motion, the camera does not turn by itself. */
  function sceneApply(st, update) {
    const S = s3(), s = update.settings;
    if (s && s.orbit && reducedMotion && reducedMotion.matches) s.orbit.auto = 0;
    scenePrefetch(update);
    // The same scene again (the live tree taking over the first paint, a reconnect): nothing changes.
    if (update.full && st.flat && JSON.stringify([s, update.nodes]) === JSON.stringify([st.flat.settings, st.flat.nodes])) return;
    st.flat = S.applyFlat(st.flat || { settings: s || st.player.settings, nodes: [] }, update);
    st.player.apply(update, sceneNow());
    sceneConfigure(st);
    sceneRequest(st);
  }

  /** The scene `el` (a `.prism-scene3d`, new or server-rendered) shows, from now on: its first (full) update. */
  function sceneStart(el, update) {
    const shared = sceneWatch();
    const canvas = el.querySelector("canvas") || el.appendChild(document.createElement("canvas"));
    canvas.setAttribute("aria-hidden", "true");
    const st = {
      el, canvas, flat: null, player: new (s3().ScenePlayer)(sceneNow(), sceneBlob),
      gl: null, r: null, lost: false, raf: 0, visible: true, w: 0, h: 0, dark: null, stale: false,
      pointers: new Map(), press: null, swallowClick: false,
    };
    canvas._scene3d = st;
    el._scene = st;
    scenes.add(st);
    sceneInput(st);
    // A lost context (GPU reset, too many contexts) leaves the box empty until it is back.
    canvas.addEventListener("webglcontextlost", (e) => {
      e.preventDefault();
      sceneCancel(st);
      st.lost = true;
      st.gl = null;
      st.r = null;
    });
    canvas.addEventListener("webglcontextrestored", () => {
      st.lost = false;
      sceneGl(st);
      sceneRequest(st);
    });
    if (shared.resize) shared.resize.observe(canvas);
    if (shared.view) shared.view.observe(canvas);
    sceneGl(st);
    sceneApply(st, update);
  }

  /** After a reset: the new node `el` takes over the scene its id had (its canvas, camera, spins and clips). */
  function sceneAdopt(el) {
    const st = sceneOrphans && sceneOrphans.get(el.dataset.pid);
    if (!st) return null;
    sceneOrphans.delete(el.dataset.pid);
    el.appendChild(st.canvas);
    st.el = el;
    st.dark = null;
    el._scene = st;
    return st;
  }
  function sceneResetStart() {
    if (!scenes.size) return;
    sceneOrphans = new Map();
    for (const st of scenes) {
      sceneOrphans.set(st.el.dataset.pid, st);
      st.el._scene = null;
    }
  }
  function sceneResetEnd() {
    if (!sceneOrphans) return;
    sceneOrphans.forEach(sceneStop);
    sceneOrphans = null;
  }

  /** Releases a scene: its frame, observers and GPU resources. */
  function sceneStop(st) {
    sceneCancel(st);
    scenes.delete(st);
    if (sceneShared) {
      if (sceneShared.resize) sceneShared.resize.unobserve(st.canvas);
      if (sceneShared.view) sceneShared.view.unobserve(st.canvas);
    }
    const gl = st.gl;
    if (gl && st.r) {
      sceneFree(st, () => true);
      st.r.programs.forEach((p) => p && gl.deleteProgram(p.prog));
      if (st.r.shadow) { gl.deleteFramebuffer(st.r.shadow.fbo); gl.deleteTexture(st.r.shadow.tex); }
      if (st.r.empty) gl.deleteVertexArray(st.r.empty);
      // Browsers keep few live contexts: give this one back now.
      const lose = gl.getExtension("WEBGL_lose_context");
      if (lose) lose.loseContext();
    }
    st.gl = null;
    st.r = null;
    st.canvas._scene3d = null;
    if (st.el._scene === st) st.el._scene = null;
    // No scene left: the page's blobs and images can go (the HTTP cache keeps them).
    if (!scenes.size && !sceneOrphans) {
      for (const [id, e] of sceneBlobs) if (e.buf) sceneBlobs.delete(id);
      for (const [src, e] of sceneImages) if (e.image) sceneImages.delete(src);
    }
  }

  /** The WebGL 2 context (none when WebGL 2 is unavailable: the box stays empty) and its GPU caches. */
  function sceneGl(st) {
    st.gl = null;
    st.r = null;
    let gl = null;
    try {
      gl = st.canvas.getContext ? st.canvas.getContext("webgl2", { alpha: true, premultipliedAlpha: true, antialias: true }) : null;
    } catch (e) {
      gl = null;
    }
    if (!gl || gl.isContextLost()) return;
    const aniso = gl.getExtension("EXT_texture_filter_anisotropic");
    st.gl = gl;
    st.r = {
      programs: new Map(), // variant (or "sky") → { prog, u, frame } | null (failed)
      meshes: new Map(), // mesh key → vertex buffers
      instances: new Map(), // I blob id → instance buffers
      vaos: new Map(), // mesh key | I blob id → { vao, mesh, inst }
      textures: new Map(), // source → texture
      envs: new Map(), // E blob id → { tex, levels, sh }
      bones: new Map(), // node key → { tex, joints, rows }
      shadow: null, // { fbo, tex, size }
      empty: null, // the sky's (attribute-less) vertex array
      aniso: aniso ? Math.min(8, gl.getParameter(aniso.MAX_TEXTURE_MAX_ANISOTROPY_EXT) || 1) : 0,
      anisoName: aniso ? aniso.TEXTURE_MAX_ANISOTROPY_EXT : 0,
      frame: 0,
      program: null,
    };
  }

  /** Frees the GPU data `gone(kind, key)` says nothing uses any more. */
  function sceneFree(st, gone) {
    const gl = st.gl, r = st.r;
    if (!gl || !r) return;
    for (const [k, v] of r.vaos) if (gone("vao", v)) { gl.deleteVertexArray(v.vao); r.vaos.delete(k); }
    for (const [k, m] of r.meshes) if (gone("mesh", k)) { m.buffers.forEach((b) => gl.deleteBuffer(b)); r.meshes.delete(k); }
    for (const [k, b] of r.instances) if (gone("inst", k)) { b.buffers.forEach((x) => gl.deleteBuffer(x)); r.instances.delete(k); }
    for (const [k, t] of r.textures) if (gone("texture", k)) { gl.deleteTexture(t); r.textures.delete(k); }
    for (const [k, e] of r.envs) if (gone("env", k)) { gl.deleteTexture(e.tex); r.envs.delete(k); }
    for (const [k, b] of r.bones) if (gone("bones", k)) { gl.deleteTexture(b.tex); r.bones.delete(k); }
  }

  /** After an update: touch scrolling, and the GPU data nothing uses any more. */
  function sceneConfigure(st) {
    const o = st.player.settings.orbit;
    // Orbiting owns the touches (the page does not scroll); a still scene lets them scroll.
    st.canvas.style.touchAction = o && (o.rotate || o.zoom || o.pan) ? "none" : "";
    const used = { mesh: new Set(), inst: new Set(), texture: new Set(), env: new Set(), bones: new Set() };
    for (const n of st.flat.nodes) {
      if (n.mesh) used.mesh.add(n.mesh.key);
      if (n.inst) used.inst.add(n.inst.id);
      if (n.skin) used.bones.add(n.key);
      sceneSources(n).forEach((src) => used.texture.add(src));
    }
    if (st.flat.settings.env) used.env.add(st.flat.settings.env.id);
    sceneFree(st, (kind, k) => (kind === "vao" ? !used.mesh.has(k.mesh) || (k.inst !== null && !used.inst.has(k.inst)) : !used[kind].has(k)));
  }

  function sceneRequest(st) {
    if (st.raf || !st.gl || !st.visible || document.hidden) return;
    st.raf = requestAnimationFrame(() => {
      st.raf = 0;
      if (scenes.has(st) && sceneDraw(st)) sceneRequest(st);
    });
  }
  function sceneCancel(st) {
    if (st.raf && typeof cancelAnimationFrame === "function") cancelAnimationFrame(st.raf);
    st.raf = 0;
  }
  /** Blobs that arrived since the last frame: the player resolves them. */
  function sceneFresh(st) {
    if (!st.stale) return;
    st.stale = false;
    st.player.blobsArrived(sceneNow());
  }

  /** Gestures: drag turns, right or Shift drag and two fingers pan, pinch and wheel zoom, a tap picks. */
  function sceneInput(st) {
    const c = st.canvas, down = st.pointers;
    const orbit = () => st.player.settings.orbit;
    c.addEventListener("pointerdown", (e) => {
      if (e.pointerType === "mouse" && e.button !== 0 && e.button !== 2) return;
      down.set(e.pointerId, { x: e.clientX, y: e.clientY });
      try { c.setPointerCapture(e.pointerId); } catch (err) { /* not capturable: moves still arrive over the canvas */ }
      if (down.size === 1) {
        st.press = { x: e.clientX, y: e.clientY, moved: false, pan: e.button === 2 || e.shiftKey };
        st.player.pointerDown();
      } else if (st.press) {
        st.press.moved = true; // two fingers: not a tap
      }
      const o = orbit();
      // No text selection or focus change while orbiting.
      if (o && (o.rotate || o.zoom || o.pan)) e.preventDefault();
      sceneRequest(st);
    });
    c.addEventListener("pointermove", (e) => {
      const p = down.get(e.pointerId), press = st.press;
      if (!p || !press) return;
      const dx = e.clientX - p.x, dy = e.clientY - p.y, player = st.player;
      if (down.size === 2) {
        // Pinch: the fingers' distance; both moving together: pan by their midpoint.
        let other = null;
        for (const [id, q] of down) if (id !== e.pointerId) other = q;
        const before = Math.hypot(p.x - other.x, p.y - other.y), after = Math.hypot(e.clientX - other.x, e.clientY - other.y);
        if (before > 0 && after > 0) player.zoom(after / before);
        player.pan(dx / 2, dy / 2, st.h);
      } else if (down.size === 1) {
        if (Math.hypot(e.clientX - press.x, e.clientY - press.y) >= s3().TAP_SLOP) press.moved = true;
        const o = orbit();
        if (press.pan && o && o.pan) player.pan(dx, dy, st.h);
        else player.drag(dx, dy);
      }
      p.x = e.clientX;
      p.y = e.clientY;
      sceneRequest(st);
    });
    const up = (e) => {
      if (!down.delete(e.pointerId) || down.size) return;
      const press = st.press, tapping = st.player.settings.tap;
      st.press = null;
      st.player.pointerUp();
      const tap = e.type === "pointerup" && press && !press.moved;
      // The click after an orbit, or a tap the scene reports, is not the parent's.
      st.swallowClick = !!press && (press.moved || tapping);
      if (tap && tapping) {
        const r = c.getBoundingClientRect();
        const w = st.w || r.width, h = st.h || r.height;
        if (st.dark === null) st.dark = sceneDark(st.el);
        sceneFresh(st);
        const hit = st.player.hit(e.clientX - r.left, e.clientY - r.top, w, h, st.dark);
        const point = hit.point && hit.point.map((v) => Math.round(v * 1e4) / 1e4);
        send({ t: "scene_tap", id: Number(st.el.dataset.pid), node: hit.node, part: hit.part, instance: hit.instance, point });
      }
      sceneRequest(st);
    };
    c.addEventListener("pointerup", up);
    c.addEventListener("pointercancel", up);
    c.addEventListener("click", (e) => {
      if (st.swallowClick) e.stopPropagation();
      st.swallowClick = false;
    });
    c.addEventListener("wheel", (e) => {
      const o = orbit();
      if (!o || !o.zoom) return; // the page scrolls
      e.preventDefault();
      // Lines and pages in points (Firefox reports lines for mouse wheels).
      const dy = e.deltaY * (e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? st.h || 800 : 1);
      st.player.zoom(Math.exp(-dy * 0.002));
      sceneRequest(st);
    }, { passive: false });
    c.addEventListener("contextmenu", (e) => {
      const o = orbit();
      if (o && o.pan) e.preventDefault(); // right drag pans
    });
  }

  // --- Drawing (the programs, attributes, uniforms and texture units of shaders.rs) ---
  const SCENE_UNIFORMS = ["u_model", "u_normal_matrix", "u_view_proj", "u_camera", "u_ambient", "u_light_count", "u_light_kind",
    "u_light_color", "u_light_position", "u_light_direction", "u_light_params", "u_exposure", "u_color", "u_emissive", "u_metalness",
    "u_roughness", "u_unlit", "u_alpha_mode", "u_alpha_cutoff", "u_has_texture", "u_has_mr_map", "u_has_normal_map", "u_normal_scale",
    "u_has_emissive_map", "u_has_ao_map", "u_has_env", "u_env_levels", "u_env_sh", "u_env_intensity", "u_has_shadow", "u_receive_shadows",
    "u_shadow_view_proj", "u_shadow_light", "u_shadow_bias", "u_shadow_normal_bias", "u_shadow_texel", "u_inverse_view_proj"];
  /** Texture units: every sampler gets its own once (samplers of different types may not share one). */
  const SCENE_UNITS = { u_texture: 0, u_mr_map: 1, u_normal_map: 2, u_emissive_map: 3, u_ao_map: 4, u_env: 5, u_shadow_map: 6, u_bones: 7 };
  const SCENE_MAPS = [["texture", "u_texture", "u_has_texture"], ["mrMap", "u_mr_map", "u_has_mr_map"], ["normalMap", "u_normal_map", "u_has_normal_map"],
    ["emissiveMap", "u_emissive_map", "u_has_emissive_map"], ["aoMap", "u_ao_map", "u_has_ao_map"]];
  const SCENE_ALPHA = { blend: 0, mask: 1, opaque: 2 };

  /** The program of variant `key` (bits: 4 instanced, 2 skinned, 1 depth only; "sky": the sky), built once per context. */
  function sceneProgram(st, key) {
    const gl = st.gl, r = st.r;
    if (r.programs.has(key)) return r.programs.get(key);
    const S = s3();
    const variant = { instanced: !!(key & 4), skinned: !!(key & 2), depthOnly: !!(key & 1) };
    const vs = key === "sky" ? S.ES_HEADER + S.SKY_VERTEX : S.programSource(S.ES_HEADER, "vertex", variant);
    const fs = key === "sky" ? S.ES_HEADER + S.SKY_FRAGMENT : S.programSource(S.ES_HEADER, "fragment", variant);
    const shader = (type, source) => {
      const s = gl.createShader(type);
      gl.shaderSource(s, source);
      gl.compileShader(s);
      if (gl.getShaderParameter(s, gl.COMPILE_STATUS)) return s;
      console.error("prism: 3D shader", gl.getShaderInfoLog(s));
      gl.deleteShader(s);
      return null;
    };
    let p = null;
    const v = shader(gl.VERTEX_SHADER, vs), f = v && shader(gl.FRAGMENT_SHADER, fs);
    if (v && f) {
      const prog = gl.createProgram();
      gl.attachShader(prog, v);
      gl.attachShader(prog, f);
      gl.linkProgram(prog);
      if (gl.getProgramParameter(prog, gl.LINK_STATUS)) {
        const u = {};
        for (const name of SCENE_UNIFORMS) u[name] = gl.getUniformLocation(prog, name);
        gl.useProgram(prog);
        for (const name in SCENE_UNITS) {
          const at = gl.getUniformLocation(prog, name);
          if (at) gl.uniform1i(at, SCENE_UNITS[name]);
        }
        r.program = null;
        p = { prog, u, frame: -1 };
      } else {
        console.error("prism: 3D program", gl.getProgramInfoLog(prog));
        gl.deleteProgram(prog);
      }
    }
    if (v) gl.deleteShader(v);
    if (f) gl.deleteShader(f);
    r.programs.set(key, p); // a failed one is not retried every frame
    return p;
  }
  function sceneUse(st, p) {
    if (st.r.program !== p) { st.gl.useProgram(p.prog); st.r.program = p; }
  }
  /** A mesh's vertex buffers, uploaded once per context by mesh key (null while its blob has not arrived). */
  function sceneMesh(st, d) {
    const gl = st.gl, r = st.r;
    let m = r.meshes.get(d.meshKey);
    if (m) return m;
    const g = st.player.geometry(d);
    if (!g.vertexCount()) return null;
    // (The element buffer binding belongs to the bound vertex array: none while uploading.)
    gl.bindVertexArray(null);
    const buffers = [];
    const upload = (target, data) => { const b = gl.createBuffer(); gl.bindBuffer(target, b); gl.bufferData(target, data, gl.STATIC_DRAW); buffers.push(b); return b; };
    const f32 = (a) => (a instanceof Float32Array ? a : new Float32Array(a));
    const skin = g.joints && g.weights && g.joints.length === g.vertexCount() * 4;
    m = {
      position: upload(gl.ARRAY_BUFFER, f32(g.positions)),
      normal: upload(gl.ARRAY_BUFFER, f32(g.normals)),
      uv: upload(gl.ARRAY_BUFFER, f32(g.uvs)),
      color: g.colors ? upload(gl.ARRAY_BUFFER, f32(g.colors)) : null,
      joints: skin ? upload(gl.ARRAY_BUFFER, g.joints instanceof Uint16Array ? g.joints : new Uint16Array(g.joints)) : null,
      weights: skin ? upload(gl.ARRAY_BUFFER, f32(g.weights)) : null,
      index: upload(gl.ELEMENT_ARRAY_BUFFER, g.indices instanceof Uint32Array ? g.indices : new Uint32Array(g.indices)),
      count: g.indices.length,
      buffers,
    };
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, null);
    r.meshes.set(d.meshKey, m);
    return m;
  }
  /** An instance set's matrices (and colors) as attribute buffers, uploaded once per context by blob id. */
  function sceneInstanceBuffers(st, id) {
    const gl = st.gl, r = st.r;
    let b = r.instances.get(id);
    if (b) return b;
    const set = sceneBlob(id, "I");
    if (!set) return null;
    const upload = (data) => { const x = gl.createBuffer(); gl.bindBuffer(gl.ARRAY_BUFFER, x); gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW); return x; };
    const matrices = upload(set.matrices), colors = set.colors ? upload(set.colors) : null;
    b = { matrices, colors, count: set.count, buffers: colors ? [matrices, colors] : [matrices] };
    r.instances.set(id, b);
    return b;
  }
  /** The vertex array of a mesh (with an instance set's attributes), made once per pair. */
  function sceneVao(st, d, mesh, inst) {
    const gl = st.gl, r = st.r, key = inst ? `${d.meshKey}|${d.instances.id}` : d.meshKey;
    let v = r.vaos.get(key);
    if (v) return v.vao;
    const vao = gl.createVertexArray();
    gl.bindVertexArray(vao);
    const attribute = (location, buffer, size, divisor) => {
      gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
      gl.enableVertexAttribArray(location);
      gl.vertexAttribPointer(location, size, gl.FLOAT, false, 0, 0);
      if (divisor) gl.vertexAttribDivisor(location, 1);
    };
    attribute(0, mesh.position, 3);
    attribute(1, mesh.normal, 3);
    attribute(2, mesh.uv, 2);
    if (mesh.color) attribute(3, mesh.color, 4);
    if (mesh.joints) {
      gl.bindBuffer(gl.ARRAY_BUFFER, mesh.joints);
      gl.enableVertexAttribArray(4);
      gl.vertexAttribIPointer(4, 4, gl.UNSIGNED_SHORT, 0, 0);
      attribute(5, mesh.weights, 4);
    }
    if (inst) {
      // a_instance: a mat4, one column per location (6–9), one per instance.
      gl.bindBuffer(gl.ARRAY_BUFFER, inst.matrices);
      for (let c = 0; c < 4; c++) {
        gl.enableVertexAttribArray(6 + c);
        gl.vertexAttribPointer(6 + c, 4, gl.FLOAT, false, 64, c * 16);
        gl.vertexAttribDivisor(6 + c, 1);
      }
      if (inst.colors) attribute(10, inst.colors, 4, true);
    }
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, mesh.index);
    gl.bindVertexArray(null);
    r.vaos.set(key, { vao, mesh: d.meshKey, inst: inst ? d.instances.id : null });
    return vao;
  }
  /** Binds `tex` to texture unit `unit`. */
  function sceneBind(gl, unit, tex) {
    gl.activeTexture(gl.TEXTURE0 + unit);
    gl.bindTexture(gl.TEXTURE_2D, tex);
  }
  /** A material image as a mipmapped, repeating texture (uploaded once per context; null until it loads). */
  function sceneTexture(st, src, unit) {
    const gl = st.gl, r = st.r;
    let t = r.textures.get(src);
    if (t) return t;
    const img = sceneImage(src);
    if (!img) return null;
    t = gl.createTexture();
    sceneBind(gl, unit, t);
    // uv (0, 0) is the image's top-left: its first row, unflipped; straight alpha (the shader
    // premultiplies); the bytes as stored (normal and metal-rough maps are data, not colors).
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
    gl.pixelStorei(gl.UNPACK_COLORSPACE_CONVERSION_WEBGL, gl.NONE);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, img);
    gl.generateMipmap(gl.TEXTURE_2D);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.REPEAT);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.REPEAT);
    if (r.aniso > 1) gl.texParameterf(gl.TEXTURE_2D, r.anisoName, r.aniso);
    r.textures.set(src, t);
    return t;
  }
  /** An environment: its levels as the mipmaps of one RGBA16F texture, and its SH (null until its blob arrives). */
  function sceneEnv(st, id) {
    const gl = st.gl, r = st.r;
    let e = r.envs.get(id);
    if (e) return e;
    const env = sceneBlob(id, "E");
    if (!env) return null;
    const tex = gl.createTexture();
    sceneBind(gl, 5, tex);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    // Row 0 (top, +Y) first: as stored.
    env.levels.forEach((l, i) => gl.texImage2D(gl.TEXTURE_2D, i, gl.RGBA16F, l.w, l.h, 0, gl.RGBA, gl.HALF_FLOAT, l.data));
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_BASE_LEVEL, 0);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAX_LEVEL, env.levels.length - 1);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.REPEAT);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    e = { tex, levels: env.levels.length, sh: new Float32Array(env.sh) };
    r.envs.set(id, e);
    return e;
  }
  /** A skinned draw's joints as an RGBA32F texture (4 texels per joint: its columns), uploaded when they change. */
  function sceneBones(st, d) {
    const gl = st.gl, r = st.r;
    let b = r.bones.get(d.key);
    if (!b) { b = { tex: gl.createTexture(), joints: null, rows: 0 }; r.bones.set(d.key, b); }
    sceneBind(gl, 7, b.tex);
    if (b.joints === d.joints) return b;
    const n = d.joints.length, data = new Float32Array(n * 16);
    d.joints.forEach((m, i) => data.set(m, i * 16));
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
    if (b.rows === n) gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, 4, n, gl.RGBA, gl.FLOAT, data);
    else {
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA32F, 4, n, 0, gl.RGBA, gl.FLOAT, data);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      b.rows = n;
    }
    b.joints = d.joints;
    return b;
  }
  /** The shadow map: a depth texture compared on lookup (`sampler2DShadow`) and its framebuffer. */
  function sceneShadowMap(st, size) {
    const gl = st.gl, r = st.r;
    if (r.shadow && r.shadow.size === size) return r.shadow;
    if (r.shadow) { gl.deleteFramebuffer(r.shadow.fbo); gl.deleteTexture(r.shadow.tex); }
    const tex = gl.createTexture();
    sceneBind(gl, 6, tex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.DEPTH_COMPONENT24, size, size, 0, gl.DEPTH_COMPONENT, gl.UNSIGNED_INT, null);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_COMPARE_MODE, gl.COMPARE_REF_TO_TEXTURE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_COMPARE_FUNC, gl.LEQUAL);
    sceneBind(gl, 6, null);
    const fbo = gl.createFramebuffer();
    gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.DEPTH_ATTACHMENT, gl.TEXTURE_2D, tex, 0);
    gl.drawBuffers([gl.NONE]);
    gl.readBuffer(gl.NONE);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    r.shadow = { fbo, tex, size };
    return r.shadow;
  }

  /** Draws one frame; true while something moves (draw another). */
  function sceneDraw(st) {
    const { gl, canvas, player } = st;
    if (!sceneShared.resize) { st.w = canvas.clientWidth; st.h = canvas.clientHeight; }
    if (!gl || !st.r || !(st.w > 0) || !(st.h > 0)) return false;
    const dpr = window.devicePixelRatio || 1;
    const pw = Math.max(1, Math.min(4096, Math.round(st.w * dpr))), ph = Math.max(1, Math.min(4096, Math.round(st.h * dpr)));
    if (canvas.width !== pw) canvas.width = pw;
    if (canvas.height !== ph) canvas.height = ph;
    if (st.dark === null) st.dark = sceneDark(st.el);
    sceneFresh(st);
    player.advance(sceneNow());
    const f = player.frame(st.w, st.h, st.dark), r = st.r;
    r.frame++;
    r.program = null;
    // Constant attributes (context state, not vertex-array state): white without vertex or instance colors.
    gl.vertexAttrib4f(3, 1, 1, 1, 1);
    gl.vertexAttrib4f(10, 1, 1, 1, 1);
    gl.frontFace(gl.CCW);
    gl.cullFace(gl.BACK);
    gl.enable(gl.DEPTH_TEST);
    gl.depthFunc(gl.LEQUAL);
    gl.disable(gl.BLEND);
    gl.depthMask(true);
    const env = f.env ? sceneEnv(st, f.env.id) : null;

    // The shadow map: its casters, depth only, from the light.
    const shadow = f.shadow && f.shadow.casters.length ? sceneShadowMap(st, f.shadow.mapSize) : null;
    if (shadow) {
      sceneBind(gl, 6, null); // not sampled while drawn into
      gl.bindFramebuffer(gl.FRAMEBUFFER, shadow.fbo);
      gl.viewport(0, 0, shadow.size, shadow.size);
      gl.clearDepth(1);
      gl.clear(gl.DEPTH_BUFFER_BIT);
      const pass = { f, depth: true, viewProj: f.shadow.viewProj, env: null, shadow: null };
      for (const i of f.shadow.casters) sceneDrawMesh(st, f.draws[i], pass);
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    }

    gl.viewport(0, 0, pw, ph);
    gl.clearColor(f.clear[0], f.clear[1], f.clear[2], f.clear[3]);
    gl.clearDepth(1);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    sceneBind(gl, 5, env ? env.tex : null);
    sceneBind(gl, 6, shadow ? shadow.tex : null);
    // The environment behind everything.
    if (env && f.env.background) {
      const sky = sceneProgram(st, "sky");
      if (sky) {
        if (!r.empty) r.empty = gl.createVertexArray();
        sceneUse(st, sky);
        const u = sky.u;
        gl.uniformMatrix4fv(u.u_inverse_view_proj, false, f.inverseViewProj);
        gl.uniform3fv(u.u_camera, f.cameraPosition);
        gl.uniform1f(u.u_env_intensity, f.env.intensity);
        gl.uniform1f(u.u_exposure, f.exposure);
        gl.disable(gl.DEPTH_TEST);
        gl.depthMask(false);
        gl.disable(gl.CULL_FACE);
        gl.bindVertexArray(r.empty);
        gl.drawArrays(gl.TRIANGLES, 0, 3);
        gl.enable(gl.DEPTH_TEST);
        gl.depthMask(true);
      }
    }
    // Premultiplied output, over what is behind.
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    const pass = { f, depth: false, viewProj: f.viewProj, env, shadow, lights: null };
    for (const d of f.draws) sceneDrawMesh(st, d, pass);
    gl.bindVertexArray(null);
    gl.disable(gl.BLEND);
    gl.depthMask(true);
    return f.animating;
  }

  /** The uniforms a pass sets once per program: camera, lights, environment, shadow. */
  function sceneFrameUniforms(gl, u, pass) {
    gl.uniformMatrix4fv(u.u_view_proj, false, pass.viewProj);
    if (pass.depth) return;
    const f = pass.f;
    if (!pass.lights) {
      const kinds = new Int32Array(8), colors = new Float32Array(24), positions = new Float32Array(24);
      const directions = new Float32Array(24), params = new Float32Array(32);
      f.lights.forEach((l, i) => {
        kinds[i] = l.kind;
        colors.set(l.color, i * 3);
        positions.set(l.position, i * 3);
        directions.set(l.direction, i * 3);
        params.set(l.params, i * 4);
      });
      pass.lights = { kinds, colors, positions, directions, params };
    }
    const L = pass.lights;
    gl.uniform3fv(u.u_camera, f.cameraPosition);
    gl.uniform3fv(u.u_ambient, f.ambient);
    gl.uniform1i(u.u_light_count, f.lights.length);
    gl.uniform1iv(u.u_light_kind, L.kinds);
    gl.uniform3fv(u.u_light_color, L.colors);
    gl.uniform3fv(u.u_light_position, L.positions);
    gl.uniform3fv(u.u_light_direction, L.directions);
    gl.uniform4fv(u.u_light_params, L.params);
    gl.uniform1f(u.u_exposure, f.exposure);
    gl.uniform1i(u.u_has_env, pass.env ? 1 : 0);
    if (pass.env) {
      gl.uniform1f(u.u_env_levels, pass.env.levels);
      gl.uniform3fv(u.u_env_sh, pass.env.sh);
      gl.uniform1f(u.u_env_intensity, f.env.intensity);
    }
    gl.uniform1i(u.u_has_shadow, pass.shadow ? 1 : 0);
    if (pass.shadow) {
      const s = f.shadow;
      gl.uniformMatrix4fv(u.u_shadow_view_proj, false, s.viewProj);
      gl.uniform1i(u.u_shadow_light, s.light);
      gl.uniform1f(u.u_shadow_bias, s.bias);
      gl.uniform1f(u.u_shadow_normal_bias, s.normalBias);
      gl.uniform1f(u.u_shadow_texel, 1 / s.mapSize);
    } else {
      gl.uniform1i(u.u_shadow_light, -1);
    }
  }

  /** One draw of a pass (the shadow map's, depth only, or the frame's). */
  function sceneDrawMesh(st, d, pass) {
    const gl = st.gl, r = st.r;
    const mesh = sceneMesh(st, d);
    if (!mesh || !mesh.count) return;
    const inst = d.instances ? sceneInstanceBuffers(st, d.instances.id) : null;
    if (d.instances && !inst) return; // its instances have not arrived
    // Skinned only when the mesh has joints (a_joints has no constant value).
    const skinned = !!d.joints && !!mesh.joints;
    const p = sceneProgram(st, (inst ? 4 : 0) | (skinned ? 2 : 0) | (pass.depth ? 1 : 0));
    if (!p) return;
    sceneUse(st, p);
    const u = p.u;
    if (p.frame !== r.frame) { p.frame = r.frame; sceneFrameUniforms(gl, u, pass); }
    gl.uniformMatrix4fv(u.u_model, false, d.world);
    if (skinned) sceneBones(st, d);
    const m = d.material;
    if (m.doubleSided) gl.disable(gl.CULL_FACE); else gl.enable(gl.CULL_FACE);
    if (!pass.depth) {
      gl.uniformMatrix3fv(u.u_normal_matrix, false, d.normal);
      gl.uniform4fv(u.u_color, m.color);
      gl.uniform3fv(u.u_emissive, m.emissive);
      gl.uniform1f(u.u_metalness, m.metalness);
      gl.uniform1f(u.u_roughness, m.roughness);
      gl.uniform1i(u.u_unlit, m.unlit ? 1 : 0);
      gl.uniform1i(u.u_alpha_mode, SCENE_ALPHA[m.alpha]);
      gl.uniform1f(u.u_alpha_cutoff, m.alphaCutoff);
      gl.uniform1i(u.u_receive_shadows, d.receiveShadows ? 1 : 0);
      gl.uniform1f(u.u_normal_scale, m.normalScale);
      SCENE_MAPS.forEach(([field, sampler, has]) => {
        const unit = SCENE_UNITS[sampler], tex = m[field] ? sceneTexture(st, m[field], unit) : null;
        gl.uniform1i(u[has], tex ? 1 : 0);
        if (tex) sceneBind(gl, unit, tex);
      });
      // Transparent meshes (last, back to front) blend and keep the depth of the opaque ones.
      if (d.transparent) gl.enable(gl.BLEND); else gl.disable(gl.BLEND);
      gl.depthMask(!d.transparent);
    }
    gl.bindVertexArray(sceneVao(st, d, mesh, inst));
    if (inst) gl.drawElementsInstanced(gl.TRIANGLES, mesh.count, gl.UNSIGNED_INT, 0, Math.min(d.instances.count, inst.count));
    else gl.drawElements(gl.TRIANGLES, mesh.count, gl.UNSIGNED_INT, 0);
  }

  /** Scenes in the server's first paint start before the socket connects (the live tree takes them over). */
  function sceneBoot() {
    mount.querySelectorAll("[data-scene]").forEach((el) => {
      if (el.dataset.kind !== "scene3d") return;
      const update = s3().decodeUpdate(el.dataset.scene);
      if (update) sceneStart(el, update);
    });
  }

  // --- Web views (#62) ------------------------------------------------------------
  // An `<iframe>`. Its page posts to the app with `parent.postMessage(value,
  // "*")` and gets the app's messages as `message` events. Only the allowed
  // origins (the config's, else the URL's own) are heard and posted to;
  // messages the app posts before the page loaded wait for it.
  const absUrl = (u) => { try { return new URL(u, location.href).href; } catch { return u; } };
  const urlOrigin = (u) => { try { return new URL(u, location.href).origin; } catch { return null; } };
  const sandboxTokens = {
    scripts: "allow-scripts", forms: "allow-forms",
    popups: "allow-popups allow-popups-to-escape-sandbox", "same-origin": "allow-same-origin",
  };
  function webViewInit(el, id) {
    el.className = "prism-web-view";
    el._wq = [];
    el._wcfg = { messages: false, loads: false, origins: [] };
    el.addEventListener("load", () => {
      if (!el._src) return; // the blank page before a URL
      el._wloaded = true;
      el._wq.splice(0).forEach((data) => webDeliver(el, data));
      if (el._wcfg.loads) send({ t: "webload", id, url: webUrl(el) });
    });
  }
  /** Only web pages load in a frame: never `javascript:` (it would run in
   *  the app's page), `data:` or `blob:`. As `is_web_view_url` in prism-core. */
  const webPageUrl = (u) => { try { return ["http:", "https:"].includes(new URL(u, location.href).protocol); } catch { return false; } };
  function webViewSrc(el, src) {
    if (el._src === src) return;
    el._src = src;
    el._wloaded = false;
    if (src && webPageUrl(src)) el.src = src;
    else el.removeAttribute("src");
  }
  function webViewConfig(el, config) {
    const [first, ...origins] = config.split("\n");
    const tokens = first.split(" ").filter(Boolean);
    el._wcfg = {
      messages: tokens.includes("messages"),
      loads: tokens.includes("loads"),
      origins: origins.map((o) => o.trim()).filter(Boolean),
    };
    // Applies from the next page the frame loads (the browser's rule).
    const sandbox = tokens.find((t) => t.startsWith("sandbox="));
    if (sandbox === undefined) el.removeAttribute("sandbox");
    else el.setAttribute("sandbox", sandbox.slice(8).split(",").map((n) => sandboxTokens[n]).filter(Boolean).join(" "));
  }
  const webOrigins = (el) => (el._wcfg.origins.length ? el._wcfg.origins : [urlOrigin(el._src)].filter(Boolean));
  /** The frame's page: readable on the app's own origin, else the URL given. */
  function webUrl(el) {
    try {
      const href = el.contentWindow.location.href;
      if (href && href !== "about:blank") return href;
    } catch { /* another origin */ }
    return absUrl(el._src);
  }
  function webDeliver(el, data) {
    const w = el.contentWindow;
    if (!w) return;
    const origins = webOrigins(el);
    if (origins.includes("*")) return w.postMessage(data, "*");
    // The browser delivers it only if the frame shows that origin.
    for (const o of origins) {
      try { w.postMessage(data, o); } catch { /* not an origin */ }
    }
  }
  function webPost(el, data) {
    if (el._wloaded) webDeliver(el, data);
    else el._wq.push(data);
  }
  function webReload(el) {
    el._wloaded = false;
    // Another origin's page cannot be reloaded from here: load the URL again.
    try { el.contentWindow.location.reload(); } catch { if (el._src) el.src = el._src; }
  }
  addEventListener("message", (e) => {
    if (!e.source) return;
    for (const el of document.querySelectorAll('iframe[data-kind="web_view"]')) {
      if (el.contentWindow !== e.source) continue;
      if (!el._wcfg || !el._wcfg.messages) return;
      const origins = webOrigins(el);
      if (!origins.includes("*") && !origins.includes(e.origin)) return;
      try {
        send({ t: "webmsg", id: Number(el.dataset.pid), data: e.data === undefined ? null : e.data, origin: e.origin });
      } catch { /* not JSON (a cycle, a BigInt) */ }
      return;
    }
  });

  // --- Islands: client-side Web Components (WASM or JS) -------------------------
  // An island node is its custom element (`<prism-island-chart>`). Its light
  // DOM is the server's fallback, which this client keeps patching like any
  // view; the island renders into its shadow root, which this client never
  // touches. Its module is imported the first time a page shows it (pages
  // without islands load nothing more). Props: `el.prismProps` (all of them)
  // plus `el.prismUpdate(changed)` for WASM islands; properties (and
  // attributes for strings, numbers, booleans) for web components.
  const islandModules = new Map();
  const registry = () => (typeof customElements === "undefined" ? null : customElements);
  function importIsland(url, tag) {
    if (islandModules.has(url) || registry()?.get(tag)) return;
    islandModules.set(url, import(url).catch((e) => {
      islandModules.delete(url);
      console.error(`Prism: cannot load the island <${tag}> (${url})`, e);
    }));
  }
  function setIsland(p) {
    let el = nodes.get(p.id);
    if (!el) return;
    if (el.localName !== p.tag) {
      // Sent right after "create": the element becomes the custom element.
      const fresh = document.createElement(p.tag);
      fresh.dataset.pid = p.id;
      fresh.dataset.kind = el.dataset.kind;
      while (el.firstChild) fresh.appendChild(el.firstChild);
      if (el.parentNode) { el.parentNode.insertBefore(fresh, el); el.remove(); }
      nodes.set(p.id, fresh);
      el = fresh;
    }
    el._island = p;
    el.prismProps ||= Object.create(null);
    // The island's CustomEvents go to the server (its own, not bubbled ones).
    el._ibound ||= new Set();
    for (const name of p.events) {
      if (el._ibound.has(name)) continue;
      el._ibound.add(name);
      el.addEventListener(name, (e) => {
        if (e.target !== el || !el._island.events.includes(name)) return;
        send({ t: "island", id: p.id, name, detail: e.detail === undefined ? null : e.detail });
      });
    }
    if (p.module) importIsland(p.module, p.tag);
    else if (p.wasm) console.warn(`Prism: the island <${p.tag}> is not built; its fallback shows`);
    if (!p.wasm && !el._iwait && registry()) {
      // A web component gets its properties once defined (set before, they
      // would hide the class's own accessors).
      el._iwait = true;
      registry().whenDefined(p.tag).then(() => {
        registry().upgrade(el);
        el._idefined = true;
        for (const [k, v] of Object.entries(el.prismProps)) if (settableProp(el, k)) el[k] = v;
      });
    }
  }
  // What a web component's props may change (S20: names can come from
  // data). An attribute needs an ordinary name, never an event handler,
  // styles, the element's identity or Prism's own (`reflects_attr` in
  // html.rs). A property is one the component declares (a setter of its
  // class, a field of the instance) or one the element has no native
  // member for, never Prism's own (`_…`, `prism…`): not `innerHTML`, an
  // `on…` handler or `__proto__`.
  const ATTR_NAME = /^[a-zA-Z_][\w-]*$/;
  const NOT_ATTR = /^(on|xmlns)|^(style|class|id|is|slot|part|nonce|data-pid|data-kind|data-island)$/i;
  const reflectsAttr = (k) => ATTR_NAME.test(k) && !NOT_ATTR.test(k);
  function settableProp(el, k) {
    if (/^(_|prism)/.test(k)) return false;
    if (Object.prototype.hasOwnProperty.call(el, k)) return true;
    const native = typeof HTMLElement === "function" ? HTMLElement.prototype : null;
    for (let p = Object.getPrototypeOf(el); p && p !== native; p = Object.getPrototypeOf(p)) {
      const d = Object.getOwnPropertyDescriptor(p, k);
      if (d) return !!d.set;
    }
    return native !== null && !(k in el);
  }
  function setIslandProps(el, props) {
    if (!el) return;
    const all = (el.prismProps ||= Object.create(null));
    for (const [k, v] of Object.entries(props)) {
      if (v === null) delete all[k]; else all[k] = v;
    }
    if (el._island && el._island.wasm) {
      if (typeof el.prismUpdate === "function") el.prismUpdate(props);
      return;
    }
    for (const [k, v] of Object.entries(props)) {
      if (reflectsAttr(k)) {
        if (typeof v === "string" || typeof v === "number") el.setAttribute(k, String(v));
        else if (v === true) el.setAttribute(k, "");
        else if (v === false || v === null) el.removeAttribute(k);
      }
      if (el._idefined && settableProp(el, k)) el[k] = v === null ? undefined : v;
    }
  }

  // --- Navigation state & History API ------------------------------------------
  // The URL follows the visible stack: the root stack, or the selected tab's.
  function navState() {
    const root = mount.firstElementChild;
    // Server-rendered markup has no client state until the first "root" patch.
    if (!root || !nodes.has(Number(root.dataset.pid)) || nodes.get(Number(root.dataset.pid)) !== root) return null;
    let stack = root;
    let tab = 0;
    if (root.dataset.kind === "tabs") {
      tab = root._selected;
      const panel = nodes.get((root._tabs || [])[tab]);
      stack = panel && panel.querySelector(".prism-stack");
    }
    // A lazy tab not rendered yet has no screen to follow.
    if (!stack || !stack._screens || !stack._screens.length) return null;
    const path = root.dataset.kind === "tabs" ? root.dataset.path : stack.dataset.path;
    return { path: path || "/", depth: stack._screens.length, stack, tab };
  }

  const isMainStack = (st) => navState()?.stack === st;
  // --- Element frames (#192): measured by the page, sent when they change ---
  /** Elements that report their frame, with the frame last sent. */
  const layoutEls = new Map();
  const layoutObserver = typeof ResizeObserver === "function" ? new ResizeObserver(() => reportLayouts()) : null;
  function setLayoutReports(el, on) {
    if (!el) return;
    if (on && !layoutEls.has(el)) { layoutEls.set(el, null); layoutObserver?.observe(el); }
    if (!on && layoutEls.has(el)) { layoutEls.delete(el); layoutObserver?.unobserve(el); }
  }
  /** `el`'s frame in the page's layout coordinates before scrolling: the
   *  scroll offsets of its scrolling ancestors and of the window added back. */
  function layoutFrame(el) {
    const r = el.getBoundingClientRect();
    let x = r.left + (window.scrollX || 0), y = r.top + (window.scrollY || 0);
    for (let n = el.parentElement; n; n = n.parentElement) {
      x += n.scrollLeft || 0;
      y += n.scrollTop || 0;
    }
    return { x, y, w: r.width, h: r.height };
  }
  let layoutQueued = false;
  /** Sends the frames that changed, once the browser laid the page out. */
  function reportLayouts() {
    if (!layoutEls.size || layoutQueued) return;
    layoutQueued = true;
    const run = () => {
      layoutQueued = false;
      for (const [el, last] of layoutEls) {
        if (!el.isConnected) { layoutEls.delete(el); layoutObserver?.unobserve(el); continue; }
        const f = layoutFrame(el);
        const round = (v) => Math.round(v * 100) / 100;
        const now = { x: round(f.x), y: round(f.y), w: round(f.w), h: round(f.h) };
        if (last && last.x === now.x && last.y === now.y && last.w === now.w && last.h === now.h) continue;
        layoutEls.set(el, now);
        send({ t: "layout", id: Number(el.dataset.pid), ...now });
      }
    };
    if (typeof requestAnimationFrame === "function") requestAnimationFrame(run); else run();
  }
  addEventListener("resize", () => reportLayouts());
  /** The main stack's top screen names the page: its stack was built before
   *  the root that makes it the main one (the first tree, a reconnection). */
  function retitle() {
    const st = navState()?.stack;
    if (!st) return;
    const top = nodes.get(st._screens[st._screens.length - 1]);
    screenTitle = (top && top._titleText) || "";
    showTitle();
  }

  let hist = null; // { depth, tab } the browser history currently reflects
  let ignorePops = 0;

  function syncHistory() {
    const s = navState();
    if (!s) return;
    if (!hist && s.depth > 1) {
      // Deep link: give the browser entries for the screens underneath.
      history.replaceState({ prism: 1, tab: s.tab }, "", urlFor("/"));
      for (let d = 2; d <= s.depth; d++) history.pushState({ prism: d, tab: s.tab }, "", urlFor(s.path));
    } else if (!hist || hist.tab !== s.tab) {
      history.replaceState({ prism: s.depth, tab: s.tab }, "", urlFor(s.path));
    } else if (s.depth > hist.depth) {
      for (let d = hist.depth + 1; d <= s.depth; d++) history.pushState({ prism: d, tab: s.tab }, "", urlFor(s.path));
    } else if (s.depth < hist.depth) {
      ignorePops++;
      history.go(s.depth - hist.depth);
    } else if (appPath() !== s.path) {
      history.replaceState({ prism: s.depth, tab: s.tab }, "", urlFor(s.path));
    }
    hist = { depth: s.depth, tab: s.tab };
  }

  window.addEventListener("popstate", (e) => {
    const s = navState();
    if (ignorePops > 0) {
      ignorePops--;
      if (s && appPath() !== s.path) history.replaceState({ prism: hist.depth, tab: s.tab }, "", urlFor(s.path));
      return;
    }
    const depth = e.state && e.state.prism;
    if (!s || !depth) return location.reload();
    userNavigated = true;
    const id = Number(s.stack.dataset.pid);
    if (depth < hist.depth) {
      hist.depth = depth;
      send({ t: "back", id, depth });
    } else if (depth > hist.depth) {
      hist.depth = depth;
      send({ t: "navigate", id, path: appPath() });
    }
  });

  // --- Scroll events (virtualized lists) -------------------------------------
  function setScrollEvents(el, id, on) {
    el._scrollEvents = on;
    if (el._scrollBound) return;
    el._scrollBound = true;
    let queued = false;
    const report = () => {
      queued = false;
      if (el._scrollEvents) send({ t: "scroll", id, o: el.scrollTop, v: el.clientHeight });
    };
    el.addEventListener("scroll", () => { if (!queued) { queued = true; requestAnimationFrame(report); } }, { passive: true });
    // Report the real viewport once laid out.
    requestAnimationFrame(() => requestAnimationFrame(report));
  }

  // --- Pull to refresh: touch pull at the top, or a Refresh button ------------
  function setRefreshable(el, id, on) {
    el._refreshable = on;
    if (!on || el._refreshBound) return;
    el._refreshBound = true;
    const button = document.createElement("button");
    button.type = "button";
    button.className = "prism-refresh";
    button.textContent = "Refresh";
    button.addEventListener("click", () => send({ t: "click", id }));
    el.prepend(button);
    let startY = null;
    el.addEventListener("touchstart", (e) => { startY = el.scrollTop <= 0 ? e.touches[0].clientY : null; }, { passive: true });
    el.addEventListener("touchmove", (e) => {
      if (startY !== null && e.touches[0].clientY - startY > 80 && el._refreshable) {
        startY = null;
        send({ t: "click", id });
      }
    }, { passive: true });
  }

  function setRefreshing(el, on) {
    let bar = el.querySelector(":scope > .prism-refreshing");
    if (on && !bar) {
      bar = document.createElement("div");
      bar.className = "prism-refreshing";
      bar.setAttribute("role", "status");
      bar.textContent = "Refreshing…";
      el.prepend(bar);
    } else if (!on && bar) {
      bar.remove();
    }
  }

  // --- Swipes (touch and pen) -------------------------------------------------
  function bindSwipe(el, id) {
    if (el._swipeBound) return;
    el._swipeBound = true;
    let start = null;
    el.addEventListener("pointerdown", (e) => { if (e.pointerType !== "mouse") start = { x: e.clientX, y: e.clientY }; });
    el.addEventListener("pointerup", (e) => {
      if (!start) return;
      const dx = e.clientX - start.x, dy = e.clientY - start.y;
      start = null;
      if (Math.abs(dx) < 60 || Math.abs(dy) > 40) return;
      const dir = dx < 0 ? "left" : "right";
      if (el._swipe && el._swipe[dir]) send({ t: "swipe", id, dir });
    });
  }

  // --- Context menus: right-click, long touch, Menu key / Shift+F10 ----------
  let openMenu = null;
  function closeMenu(restore) {
    if (!openMenu) return;
    openMenu.el.remove();
    if (restore) openMenu.owner.focus?.();
    openMenu = null;
  }
  document.addEventListener("pointerdown", (e) => { if (openMenu && !openMenu.el.contains(e.target)) closeMenu(false); }, true);

  function showMenu(owner, spec, x, y, alignEnd) {
    closeMenu(false);
    const menu = document.createElement("div");
    menu.className = "prism-menu";
    menu.setAttribute("role", "menu");
    if (owner.getAttribute("aria-label")) menu.setAttribute("aria-label", owner.getAttribute("aria-label"));
    const add = (items, depth) => items.forEach((it) => {
      if (it.t === "sep") {
        const s = document.createElement("div");
        s.setAttribute("role", "separator");
        menu.appendChild(s);
      } else if (it.t === "sub") {
        const g = document.createElement("div");
        g.className = "prism-menu-group";
        g.textContent = it.title;
        menu.appendChild(g);
        add(it.menu.items, depth + 1);
      } else {
        const b = document.createElement("button");
        b.type = "button";
        b.setAttribute("role", it.checked === null ? "menuitem" : "menuitemcheckbox");
        if (it.checked !== null) b.setAttribute("aria-checked", String(it.checked));
        b.dataset.role = it.role;
        b.disabled = !it.enabled;
        b.tabIndex = -1;
        b.innerHTML = '<span aria-hidden="true"></span><span></span>';
        b.firstChild.textContent = it.icon ? glyphs[it.icon] || "" : it.checked ? "✓" : "";
        b.lastChild.textContent = it.title;
        // Its shortcut, bound or only a hint (#247).
        if (it.shortcut) {
          const k = document.createElement("span");
          k.className = "prism-menu-key";
          k.textContent = shortcutLabel(it.shortcut);
          b.setAttribute("aria-keyshortcuts", ariaShortcut(it.shortcut));
          b.appendChild(k);
        }
        if (depth) b.style.paddingLeft = 10 + depth * 12 + "px";
        b.addEventListener("click", () => { closeMenu(true); send({ t: "menu", id: it.id }); });
        menu.appendChild(b);
      }
    });
    add(spec.items, 0);
    menu.addEventListener("keydown", (e) => {
      const items = [...menu.querySelectorAll("[role^=menuitem]:not(:disabled)")];
      const i = items.indexOf(document.activeElement);
      if (e.key === "Escape") { e.preventDefault(); closeMenu(true); }
      else if (e.key === "ArrowDown") { e.preventDefault(); items[(i + 1) % items.length]?.focus(); }
      else if (e.key === "ArrowUp") { e.preventDefault(); items[(i - 1 + items.length) % items.length]?.focus(); }
      else if (e.key === "Tab") { closeMenu(false); }
    });
    document.body.appendChild(menu);
    const r = menu.getBoundingClientRect();
    // Below an element in a right-to-left page: its right edges aligned.
    if (alignEnd != null) x = alignEnd - r.width;
    menu.style.left = Math.max(8, Math.min(x, innerWidth - r.width - 8)) + "px";
    menu.style.top = Math.max(8, Math.min(y, innerHeight - r.height - 8)) + "px";
    openMenu = { el: menu, owner };
    menu.querySelector("[role^=menuitem]:not(:disabled)")?.focus();
  }

  // `"cmd+shift+b"` as the platform writes it: ⇧⌘B on Apple systems,
  // Ctrl+Shift+B elsewhere (#247).
  function shortcutLabel(keys) {
    const parts = keys.toLowerCase().split("+").map((k) => k.trim());
    const key = parts.pop() || "";
    const name = { space: "Space", enter: "↩", return: "↩", escape: "Esc", backspace: "⌫", delete: "⌦", tab: "⇥",
      arrowup: "↑", arrowdown: "↓", arrowleft: "←", arrowright: "→" }[key] || capitalized(key);
    const has = (...names) => parts.some((p) => names.includes(p));
    if (applePlatform) {
      return (has("ctrl", "control") ? "⌃" : "") + (has("alt", "option", "opt") ? "⌥" : "") + (has("shift") ? "⇧" : "") +
        (has("cmd", "command", "meta", "super") ? "⌘" : "") + name;
    }
    return [has("cmd", "command", "meta", "ctrl", "control") && "Ctrl", has("super") && "Super", has("alt", "option", "opt") && "Alt",
      has("shift") && "Shift", name].filter(Boolean).join("+");
  }
  // The same for `aria-keyshortcuts` ("Meta+Shift+B").
  function ariaShortcut(keys) {
    const parts = keys.toLowerCase().split("+").map((k) => k.trim());
    const key = parts.pop() || "";
    const mods = parts.map((p) => ({ cmd: applePlatform ? "Meta" : "Control", command: applePlatform ? "Meta" : "Control", meta: "Meta", super: "Meta",
      ctrl: "Control", control: "Control", alt: "Alt", option: "Alt", opt: "Alt", shift: "Shift" })[p]).filter(Boolean);
    return [...mods, capitalized(key)].join("+");
  }
  const capitalized = (k) => k.length === 1 ? k.toUpperCase() : k ? k[0].toUpperCase() + k.slice(1) : "";

  // A pop-up menu (#247): at `at` in `el` (the viewport without one), or
  // below `el`, aligned with its start edge.
  function popupMenu(el, spec, at) {
    if (!spec) return;
    const owner = el || document.activeElement || document.body;
    if (!el) return showMenu(owner, spec, at ? at[0] : 0, at ? at[1] : 0);
    const r = el.getBoundingClientRect();
    if (at) return showMenu(owner, spec, r.left + at[0], r.top + at[1]);
    const rtl = (el.closest?.("[dir]")?.getAttribute("dir") || document.documentElement.dir) === "rtl";
    showMenu(owner, spec, r.left, r.bottom + 4, rtl ? r.right : null);
  }

  function setMenu(el, id, spec) {
    el._menuSpec = spec;
    if (spec) el.setAttribute("aria-haspopup", "menu"); else el.removeAttribute("aria-haspopup");
    if (el._menuBound) return;
    el._menuBound = true;
    el.addEventListener("contextmenu", (e) => {
      if (!el._menuSpec || el._lifted) return;
      e.preventDefault();
      e.stopPropagation();
      showMenu(el, el._menuSpec, e.clientX, e.clientY);
    });
    el.addEventListener("keydown", (e) => {
      if (!el._menuSpec || e.target !== el) return;
      if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
        e.preventDefault();
        const r = el.getBoundingClientRect();
        showMenu(el, el._menuSpec, r.left + 12, r.bottom);
      }
    });
    // Long touch opens the menu too.
    let timer;
    el.addEventListener("pointerdown", (e) => {
      // A long-press drag (#243) owns the long touch.
      if (e.pointerType !== "touch" || !el._menuSpec || el._dragDelay) return;
      timer = setTimeout(() => showMenu(el, el._menuSpec, e.clientX, e.clientY), 500);
    });
    ["pointerup", "pointercancel", "pointermove"].forEach((t) => el.addEventListener(t, () => clearTimeout(timer)));
  }

  // --- Alerts: an accessible modal dialog --------------------------------------
  function showAlert(a) {
    const d = document.createElement("dialog");
    d.className = "prism-alert";
    d.setAttribute("role", "alertdialog");
    d.setAttribute("aria-labelledby", `prism-alert-${a.id}-t`);
    if (a.message) d.setAttribute("aria-describedby", `prism-alert-${a.id}-m`);
    d.innerHTML = `<h2 id="prism-alert-${a.id}-t"></h2>${a.message ? `<p id="prism-alert-${a.id}-m"></p>` : ""}<div class="prism-alert-actions"></div>`;
    d.querySelector("h2").textContent = a.title;
    if (a.message) d.querySelector("p").textContent = a.message;
    const actions = d.querySelector(".prism-alert-actions");
    const previous = document.activeElement;
    const close = (index) => {
      d.close();
      d.remove();
      previous?.focus?.();
      if (index !== null) send({ t: "alert", id: a.id, index });
    };
    a.actions.forEach((act, i) => {
      const b = document.createElement("button");
      b.type = "button";
      b.textContent = act.title;
      b.dataset.role = act.role;
      b.addEventListener("click", () => close(i));
      actions.appendChild(b);
    });
    const cancel = a.actions.findIndex((x) => x.role === "cancel");
    d.addEventListener("cancel", (e) => { e.preventDefault(); close(cancel >= 0 ? cancel : null); });
    document.body.appendChild(d);
    d.showModal();
    raiseLayers();
    // Safest default focus: Cancel when there is one.
    (cancel >= 0 ? actions.children[cancel] : actions.firstChild)?.focus();
  }

  // --- Timers: count down by themselves --------------------------------------
  function remaining(end) {
    const left = Math.max(0, Math.round(end - Date.now() / 1000));
    const h = Math.floor(left / 3600), m = Math.floor(left / 60) % 60, sec = left % 60;
    const pad = (n) => String(n).padStart(2, "0");
    return h ? `${h}:${pad(m)}:${pad(sec)}` : `${m}:${pad(sec)}`;
  }
  /** The time left, or the `after` text once it shows 0:00 (#281). */
  function timerText(end, after) {
    const text = remaining(end);
    return after && text === "0:00" ? after : text;
  }
  function showTimer(el) {
    if (el.dataset.end === undefined) return;
    const text = timerText(Number(el.dataset.end), el.dataset.after);
    if (el.dataset.kind === "timer") {
      let t = el.firstChild && el.firstChild.nodeType === 3 ? el.firstChild : null;
      if (!t) { t = document.createTextNode(""); el.prepend(t); }
      if (t.data !== text) t.data = text;
    } else if (el.textContent !== text) {
      el.textContent = text;
    }
  }
  function tick() {
    document.querySelectorAll("[data-end]").forEach(showTimer);
  }
  setInterval(tick, 1000);

  // --- Static snapshots (Live Activities on the web) --------------------------
  // Mirrors the SwiftUI renderer used by widgets and the Dynamic Island.
  const color = (c) => (Array.isArray(c) ? `light-dark(${c[0]}, ${c[1]})` : c);
  function snode(n) {
    const tag = { t: "span", i: "span", m: "span", p: "progress" }[n.k] || "div";
    const el = document.createElement(tag);
    const st = el.style;
    if (n.k === "v") { st.display = "flex"; st.flexDirection = n.d ? "row" : "column"; }
    else st.display = n.k === "p" ? "block" : "inline-block";
    if (n.g) st.gap = n.g + "px";
    if (n.pd) st.padding = n.pd.map((v) => v + "px").join(" ");
    if (n.m) st.margin = n.m.map((v) => v + "px").join(" ");
    if (n.k === "sh") {
      // A drawing (charts): drawn in SVG at the box's size, like `shape()`.
      el.className = "prism-shape";
      el._drawing = n.s || "";
      if (shapeObserver) shapeObserver.observe(el);
      requestAnimationFrame(() => drawShape(el));
    }
    if (n.w) st.width = n.w === "f" ? "100%" : n.w + "px";
    if (n.h) st.height = n.h === "f" ? "100%" : n.h + "px";
    if (n.f) st.flexGrow = n.f;
    if (n.ai) st.alignItems = { s: "flex-start", c: "center", e: "flex-end" }[n.ai];
    if (n.jc) st.justifyContent = { c: "center", e: "flex-end", b: "space-between", a: "space-around", v: "space-evenly" }[n.jc];
    if (n.bg) st.background = color(n.bg);
    if (n.fg) { st.color = color(n.fg); st.accentColor = color(n.fg); }
    if (n.bw) st.border = `${n.bw}px solid ${color(n.bc || "currentColor")}`;
    if (n.r) st.borderRadius = n.r + "px";
    if (n.fs) st.fontSize = n.fs + "px";
    if (n.fw) st.fontWeight = [400, 500, 600, 700][n.fw];
    if (n.ta) st.textAlign = n.ta === "c" ? "center" : "right";
    if (n.o !== undefined) st.opacity = n.o;
    if (n.k === "t") el.textContent = n.s || "";
    if (n.k === "i") { el.textContent = glyphs[n.s] || "•"; el.className = "prism-symbol"; }
    if (n.k === "m") {
      el.dataset.end = n.v;
      if (n.ph) el.dataset.after = n.ph;
      st.fontVariantNumeric = "tabular-nums";
      el.textContent = timerText(n.v, n.ph);
    }
    if (n.k === "p") { el.max = 1; el.value = n.v || 0; }
    if (n.al) el.setAttribute("aria-label", n.al);
    if (n.ah) el.setAttribute("aria-hidden", "true");
    if (n.hd) { el.setAttribute("role", "heading"); el.setAttribute("aria-level", "2"); }
    if (n.l) { st.cursor = "pointer"; el.addEventListener("click", (e) => { e.stopPropagation(); openLink(n.l); }); }
    (n.c || []).forEach((c) => el.appendChild(snode(c)));
    return el;
  }

  function openLink(path) {
    userNavigated = true;
    send({ t: "link", path });
  }

  // --- Live Activities: an island-like live banner ----------------------------
  const island = document.getElementById("prism-island");
  const activities = new Map();
  function showActivity(id, c) {
    let el = activities.get(id);
    if (!el) {
      el = document.createElement("button");
      el.type = "button";
      el.className = "prism-island";
      el.setAttribute("aria-expanded", "false");
      el.addEventListener("click", () => {
        const open = el.getAttribute("aria-expanded") !== "true";
        el.setAttribute("aria-expanded", String(open));
        render();
      });
      activities.set(id, el);
      island.appendChild(el);
    }
    const render = () => {
      el.replaceChildren();
      el.setAttribute("aria-label", el._c.sm || "Live activity");
      if (el.getAttribute("aria-expanded") === "true" && el._c.ls) {
        el.appendChild(snode(el._c.ls));
        if (el._c.l) {
          const open = document.createElement("div");
          open.textContent = "Open ›";
          open.style.cssText = "margin-top:8px;opacity:.8;font-size:13px";
          open.addEventListener("click", (e) => { e.stopPropagation(); openLink(el._c.l); });
          el.appendChild(open);
        }
      } else {
        const row = document.createElement("div");
        row.className = "prism-island-compact";
        if (el._c.cl) row.appendChild(snode(el._c.cl));
        if (el._c.ct) row.appendChild(snode(el._c.ct));
        el.appendChild(row);
      }
    };
    el._c = c;
    render();
  }

  // --- Notifications: browser notifications, or an in-page toast ---------------
  const toasts = document.getElementById("prism-toasts");
  const pending = new Map();
  // Reports a tap (action null) or an action button to the app.
  const respond = (n, action, text, link) =>
    send({ t: "notif", id: n.id, action, text, link: link ?? (action ? null : n.link) });
  function toast(n) {
    const actions = n.actions || [];
    const el = document.createElement(actions.length ? "div" : "button");
    el.className = "prism-toast";
    el.setAttribute("role", actions.length ? "group" : "button");
    if (!actions.length) el.type = "button";
    el.innerHTML = "<strong></strong><span></span>";
    el.children[0].textContent = n.title;
    el.children[1].textContent = [n.subtitle, n.body].filter(Boolean).join(" — ");
    el.setAttribute("aria-label", n.title);
    let timer = setTimeout(() => el.remove(), actions.length ? 20000 : 8000);
    const done = () => { clearTimeout(timer); el.remove(); };
    el.addEventListener("click", (e) => {
      if (e.target.closest(".prism-toast-actions")) return;
      done(); respond(n, null, null);
    });
    if (actions.length) {
      const bar = document.createElement("div");
      bar.className = "prism-toast-actions";
      for (const a of actions) {
        const b = document.createElement("button");
        b.type = "button";
        b.textContent = a.title;
        if (a.destructive) b.dataset.role = "destructive";
        b.addEventListener("click", () => {
          if (a.reply == null) { done(); return respond(n, a.id, null, a.link); }
          // Quick reply: swap the buttons for a text field.
          clearTimeout(timer);
          const input = document.createElement("input");
          input.name = "prism-reply";
          input.placeholder = a.reply;
          input.setAttribute("aria-label", a.title);
          const sendBtn = document.createElement("button");
          sendBtn.type = "button";
          sendBtn.textContent = a.title;
          const submit = () => { done(); respond(n, a.id, input.value, a.link); };
          sendBtn.addEventListener("click", submit);
          input.addEventListener("keydown", (e) => { if (e.key === "Enter") submit(); if (e.key === "Escape") done(); });
          bar.replaceChildren(input, sendBtn);
          input.focus();
        });
        bar.appendChild(b);
      }
      el.appendChild(bar);
    }
    toasts.appendChild(el);
  }
  function notify(n) {
    // Page notifications have no buttons: notifications with actions stay in-page.
    if ("Notification" in window && Notification.permission === "granted" && document.hidden && !(n.actions || []).length) {
      const sys = new Notification(n.title, { body: [n.subtitle, n.body].filter(Boolean).join("\n"), tag: n.id });
      sys.onclick = () => { window.focus(); respond(n, null, null); };
    } else {
      toast(n);
    }
  }

  // --- Remote push (Web Push): a service worker at <base>/prism-sw.js -----------
  const swURL = `${BASE}/prism-sw.js`;
  const pushQueue = [];
  // Messages for the app wait for the socket (a click can open the page).
  const sendPush = (msg) => (ws && ws.readyState === 1 ? send(msg) : pushQueue.push(msg));
  const b64ToBytes = (s) => {
    const raw = atob((s + "===".slice((s.length + 3) % 4)).replace(/-/g, "+").replace(/_/g, "/"));
    return Uint8Array.from(raw, (c) => c.charCodeAt(0));
  };
  async function pushRegister(key) {
    try {
      if (!("serviceWorker" in navigator) || !("PushManager" in window)) throw new Error("unavailable");
      // Safari wants the permission asked from a user gesture: the app's
      // button press is one (the message arrives right after the click).
      if ("Notification" in window && Notification.permission === "default") await Notification.requestPermission();
      if ("Notification" in window && Notification.permission === "denied") throw new Error("denied");
      const reg = await navigator.serviceWorker.register(swURL, { scope: `${BASE}/` });
      await navigator.serviceWorker.ready;
      let sub = await reg.pushManager.getSubscription();
      // A subscription made with another VAPID key cannot be reused.
      const current = sub && sub.options && sub.options.applicationServerKey;
      if (sub && current && btoa(String.fromCharCode(...new Uint8Array(current))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "") !== key) {
        await sub.unsubscribe();
        sub = null;
      }
      if (!sub) sub = await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: b64ToBytes(key) });
      sendPush({ t: "push_sub", sub: sub.toJSON() });
    } catch (e) {
      sendPush({ t: "push_err", err: String((e && e.message) || e) });
    }
  }
  async function pushUnregister() {
    const reg = "serviceWorker" in navigator && await navigator.serviceWorker.getRegistration(swURL);
    const sub = reg && await reg.pushManager.getSubscription();
    if (sub) await sub.unsubscribe();
  }
  if ("serviceWorker" in navigator) {
    navigator.serviceWorker.addEventListener("message", (e) => {
      const m = e.data || {};
      if (m.prismPush) sendPush({ t: "push", event: m.prismPush, launched: !!m.launched, payload: JSON.stringify(m.payload || {}) });
    });
  }
  // Connected: deliver queued pushes, and ask the worker for a click that
  // opened this page.
  const pushReady = async () => {
    pushQueue.splice(0).forEach(send);
    const reg = "serviceWorker" in navigator && await navigator.serviceWorker.getRegistration(swURL);
    if (reg && reg.active) reg.active.postMessage({ prismReady: true });
  };

  // Window size → responsive layouts (the server re-renders across breakpoints).
  // `cx.safe_area()` (#28): what the notch, bars and rounded corners cover,
  // as `env(safe-area-inset-*)` reports it (zero unless the page is laid
  // out under them, `viewport-fit=cover`).
  let safeProbe = null;
  const safeArea = () => {
    if (!safeProbe) {
      safeProbe = document.createElement("div");
      safeProbe.setAttribute("aria-hidden", "true");
      safeProbe.style.cssText = "position:fixed;left:0;top:0;width:0;height:0;visibility:hidden;pointer-events:none;" +
        "padding:env(safe-area-inset-top,0px) env(safe-area-inset-right,0px) env(safe-area-inset-bottom,0px) env(safe-area-inset-left,0px)";
      document.documentElement.appendChild(safeProbe);
    }
    const s = getComputedStyle(safeProbe);
    return ["paddingTop", "paddingRight", "paddingBottom", "paddingLeft"].map((k) => parseFloat(s[k]) || 0);
  };
  let resizeTimer = null;
  const reportViewport = () => {
    document.cookie = `prism_vw=${innerWidth}x${innerHeight}; Path=/; Max-Age=31536000; SameSite=Lax`;
    send({ t: "viewport", w: innerWidth, h: innerHeight, safe: safeArea() });
  };
  addEventListener("resize", () => { clearTimeout(resizeTimer); resizeTimer = setTimeout(reportViewport, 120); });
  document.cookie = `prism_vw=${innerWidth}x${innerHeight}; Path=/; Max-Age=31536000; SameSite=Lax`;

  // Page visibility → app lifecycle (background tasks keep running server-side).
  const reportLifecycle = () => send({ t: "lifecycle", state: document.hidden ? "background" : document.hasFocus() ? "active" : "inactive" });
  document.addEventListener("visibilitychange", reportLifecycle);
  window.addEventListener("focus", reportLifecycle);
  window.addEventListener("blur", reportLifecycle);

  // --- Device APIs: location, motion, photos, haptics, clipboard, share -----------
  // The browser's UTC offset (seconds east), for dates formatted on the server.
  const tz = -new Date().getTimezoneOffset() * 60;
  document.cookie = `prism_tz=${tz}; Path=/; Max-Age=31536000; SameSite=Lax`;
  const streams = new Map();
  const position = (p) => ({
    latitude: p.coords.latitude, longitude: p.coords.longitude, altitude: p.coords.altitude,
    accuracy: p.coords.accuracy, speed: p.coords.speed, heading: p.coords.heading, timestamp: p.timestamp / 1000,
  });
  const geoError = (e) => (e.code === 1 ? "denied" : e.message || "unavailable");
  const geoOptions = (msg) => ({ enableHighAccuracy: msg.accuracy === "best", timeout: 20000, maximumAge: msg.accuracy === "coarse" ? 600000 : 30000 });
  const query = async (name) => {
    try { return (await navigator.permissions.query({ name })).state; } catch { return null; }
  };
  const web = { granted: "granted", denied: "denied", prompt: "not_determined" };
  async function permissionState(name, ask) {
    switch (name) {
      case "location": case "location_always": {
        if (!navigator.geolocation) return "unavailable";
        if (ask) await new Promise((r) => navigator.geolocation.getCurrentPosition(r, r, { timeout: 30000 }));
        return web[await query("geolocation")] || "not_determined";
      }
      case "camera": case "microphone": {
        if (!navigator.mediaDevices) return "unavailable";
        if (ask) {
          try {
            const s = await navigator.mediaDevices.getUserMedia(name === "camera" ? { video: true } : { audio: true });
            s.getTracks().forEach((t) => t.stop());
          } catch (e) { if (e.name === "NotAllowedError") return "denied"; return "unavailable"; }
        }
        return web[await query(name)] || (ask ? "granted" : "not_determined");
      }
      case "notifications": {
        if (!("Notification" in window)) return "unavailable";
        if (ask && Notification.permission === "default") await Notification.requestPermission();
        return web[Notification.permission === "default" ? "prompt" : Notification.permission];
      }
      case "motion": {
        if (!("DeviceMotionEvent" in window)) return "unavailable";
        if (typeof DeviceMotionEvent.requestPermission === "function") {
          if (!ask) return "not_determined";
          try { return web[await DeviceMotionEvent.requestPermission()] || "denied"; } catch { return "denied"; }
        }
        return "granted";
      }
      // Picking files needs no permission on the web.
      case "photos": return "granted";
      default: return "unavailable";
    }
  }
  /**
   * Sends the chosen files to the server in chunks (#112): `upload_start`
   * (names, types, sizes), the bytes as `upload_chunk`s, then the reply. The
   * server writes them as they come and tells the app how far they are; a
   * `stop` for the call (`cx.cancel_upload`) ends it with « cancelled ».
   * Chunks wait while the socket still has a few of them to send, so the
   * progress follows the network and a cancel takes effect at once.
   */
  const CHUNK = 256 * 1024;
  async function upload(msg, files, reply) {
    let stopped = false;
    streams.set(msg.id, () => { stopped = true; });
    try {
      await sendFiles({ t: "upload_start", id: msg.id }, { id: msg.id }, files, () => stopped);
      reply(stopped ? { err: "cancelled" } : { uploaded: true });
    } catch (e) {
      reply({ err: String((e && e.message) || e) });
    } finally {
      streams.delete(msg.id);
    }
  }
  /** `start` with the files' names, types and sizes, then their chunks (`key` names the upload). */
  async function sendFiles(start, key, files, stopped) {
    // A folder's files (#225) go by their path in it.
    send({ ...start, files: files.map((f) => ({ name: f.webkitRelativePath || f.name, mime: f.type, size: f.size })) });
    for (let i = 0; i < files.length && !stopped(); i++) {
      for (let at = 0; at < files[i].size && !stopped(); at += CHUNK) {
        while (ws && ws.bufferedAmount > 4 * CHUNK && !stopped()) await new Promise((r) => setTimeout(r, 20));
        const bytes = new Uint8Array(await files[i].slice(at, at + CHUNK).arrayBuffer());
        if (!stopped()) send({ t: "upload_chunk", ...key, file: i, data: base64(bytes) });
      }
    }
  }

  // --- The clipboard's types (#246) ---------------------------------------------
  // Read: every type of the first item, uploaded like picked files, each
  // named by its type (a custom `web x/y` format comes back as `x/y`).
  // Without `clipboard.read` (or when the browser refuses it), the text.
  async function readClipboardItem(msg, reply) {
    const files = [];
    try {
      const [item] = await navigator.clipboard.read();
      for (const t of item ? item.types : []) {
        const type = t.startsWith("web ") ? t.slice(4) : t;
        const blob = await item.getType(t).catch(() => null);
        if (blob && !files.some((f) => f.name === type)) files.push(new File([blob], type, { type }));
      }
    } catch {
      const text = await navigator.clipboard?.readText?.().catch(() => null);
      if (text) files.push(new File([text], "text/plain", { type: "text/plain" }));
    }
    if (!files.length) return reply({ text: null });
    return upload(msg, files, reply);
  }
  // Write: text inline, other bytes fetched from the session's URL. The
  // browser takes text/plain, text/html and image/png (another image is
  // converted to PNG), other types as custom `web …` formats where it can;
  // what it refuses is left out, the text last of all.
  const CLIPBOARD_TYPES = ["text/plain", "text/html", "image/png"];
  async function toPng(blob) {
    const bitmap = await createImageBitmap(blob);
    const canvas = document.createElement("canvas");
    canvas.width = bitmap.width;
    canvas.height = bitmap.height;
    canvas.getContext("2d").drawImage(bitmap, 0, 0);
    return new Promise((resolve, reject) => canvas.toBlob((b) => (b ? resolve(b) : reject(new Error("png"))), "image/png"));
  }
  async function writeClipboard(entries) {
    const clipboard = navigator.clipboard;
    if (!clipboard) return;
    const text = entries.find((e) => e.type === "text/plain");
    const plain = () => (text ? clipboard.writeText(text.text ?? "").catch(() => {}) : undefined);
    if (!clipboard.write || typeof ClipboardItem === "undefined") return plain();
    // Each blob typed as its key (a custom format's too: `web x/y`).
    const blob = (e, type = e.type) => (e.url
      ? fetch(e.url, { credentials: "same-origin" }).then((r) => r.blob()).then((b) => new Blob([b], { type }))
      : Promise.resolve(new Blob([e.text ?? ""], { type })));
    const supports = (t) => typeof ClipboardItem.supports !== "function" || ClipboardItem.supports(t);
    const data = {};
    for (const e of entries) {
      if (e.type.startsWith("image/") && e.type !== "image/png") {
        if (!entries.some((x) => x.type === "image/png") && !data["image/png"]) data["image/png"] = blob(e).then(toPng);
        continue;
      }
      const type = CLIPBOARD_TYPES.includes(e.type) ? e.type : "web " + e.type;
      if (supports(type)) data[type] = blob(e, type);
    }
    const write = (types) => clipboard.write([new ClipboardItem(Object.fromEntries(types))]);
    const all = Object.entries(data);
    if (!all.length) return plain();
    try { await write(all); } catch {
      const basic = all.filter(([t]) => CLIPBOARD_TYPES.includes(t));
      try { if (basic.length < all.length && basic.length) await write(basic); else throw 0; } catch { await plain(); }
    }
  }

  // --- Drop targets (#111) -------------------------------------------------------
  // Files dragged from the system onto a node: `data-prism-drop` while they
  // are over it (the `drop_hover` state), then they go up like picked files
  // (`drop_start`, chunks, `drop_end`) and the server hands them to the app.
  // Only files the target accepts are sent; a drop elsewhere on a page with
  // drop targets does not make the browser open the file.
  let drops = 0, dropTargets = 0;
  // The drops uploading, each `true` once the server refused it (#290):
  // its bytes then stay in the browser.
  const dropping = new Map();
  const draggingFiles = (e) => !!e.dataTransfer && [...(e.dataTransfer.types || [])].includes("Files");
  /** `PickFiles::accepts`: MIME types (`image/*`) or extensions (`.pdf`); none: any file. */
  function accepts(types, name, mime) {
    return !types.length || types.some((t) => {
      t = t.trim().toLowerCase();
      if (t.startsWith(".")) return name.toLowerCase().endsWith(t);
      if (t.endsWith("/*")) return (mime || "").toLowerCase().startsWith(t.slice(0, -1));
      return (mime || "").toLowerCase() === t;
    });
  }
  function setDropTarget(el, id, types) {
    if (!!el._dropTypes !== !!types) dropTargets += types ? 1 : -1;
    el._dropTypes = types || null;
    if (!types) { el.removeAttribute("data-prism-drop"); return; }
    if (el._dropBound) return;
    el._dropBound = true;
    let depth = 0;
    const over = (on) => { if (on) el.setAttribute("data-prism-drop", ""); else el.removeAttribute("data-prism-drop"); };
    // Innermost targets win: the event stops at the first one.
    el.addEventListener("dragenter", (e) => {
      if (!el._dropTypes || !draggingFiles(e)) return;
      e.preventDefault(); e.stopPropagation();
      if (depth++ === 0) over(true);
    });
    el.addEventListener("dragover", (e) => {
      if (!el._dropTypes || !draggingFiles(e)) return;
      e.preventDefault(); e.stopPropagation();
      e.dataTransfer.dropEffect = "copy";
    });
    el.addEventListener("dragleave", (e) => {
      if (!el._dropTypes || !draggingFiles(e)) return;
      e.stopPropagation();
      if (--depth <= 0) { depth = 0; over(false); }
    });
    el.addEventListener("drop", (e) => {
      if (!el._dropTypes || !draggingFiles(e)) return;
      e.preventDefault(); e.stopPropagation();
      depth = 0; over(false);
      const all = [...(e.dataTransfer.files || [])];
      const files = all.filter((f) => accepts(el._dropTypes, f.name, f.type));
      if (all.length) dropFiles(id, files);
    });
  }
  async function dropFiles(node, files) {
    const key = { drop: ++drops };
    // None the target takes: the server tells the app (#290).
    if (!files.length) return send({ t: "drop_start", ...key, node, files: [] });
    dropping.set(key.drop, false);
    const stopped = () => dropping.get(key.drop) === true;
    try {
      await sendFiles({ t: "drop_start", ...key, node }, key, files, stopped);
      if (!stopped()) send({ t: "drop_end", ...key });
    } catch (e) {
      if (!stopped()) send({ t: "drop_end", ...key, err: String((e && e.message) || e) });
    }
    dropping.delete(key.drop);
  }
  for (const type of ["dragover", "drop"]) {
    addEventListener(type, (e) => {
      if (dropTargets > 0 && draggingFiles(e)) { e.preventDefault(); e.dataTransfer.dropEffect = "none"; }
    });
  }
  /** Standard base64 of `bytes`, in pieces (an argument list has a limit). */
  function base64(bytes) {
    let bin = "";
    for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    return btoa(bin);
  }
  function watchMotion(msg, event) {
    let last = 0, attitude = [0, 0, 0], seen = false;
    const rad = Math.PI / 180;
    const orient = (e) => { attitude = [(e.gamma || 0) * rad, (e.beta || 0) * rad, (e.alpha || 0) * rad]; };
    const motion = (e) => {
      seen = true;
      const now = performance.now();
      if (now - last < msg.interval) return;
      last = now;
      const a = e.acceleration || {}, g = e.accelerationIncludingGravity || {}, r = e.rotationRate || {};
      const G = 9.80665;
      event({
        acceleration: [(a.x || 0) / G, (a.y || 0) / G, (a.z || 0) / G],
        gravity: [((g.x || 0) - (a.x || 0)) / G, ((g.y || 0) - (a.y || 0)) / G, ((g.z || 0) - (a.z || 0)) / G],
        rotation_rate: [(r.beta || 0) * rad, (r.gamma || 0) * rad, (r.alpha || 0) * rad],
        attitude,
      });
    };
    addEventListener("devicemotion", motion);
    addEventListener("deviceorientation", orient);
    // Desktops have no sensors: say so instead of staying silent.
    setTimeout(() => { if (!seen && streams.has(msg.id)) event({ err: "unavailable" }); }, 1500);
    return () => { removeEventListener("devicemotion", motion); removeEventListener("deviceorientation", orient); };
  }
  // --- Sign-in popups and WebAuthn (checked on the server: security.rs) ---------
  const b64u = (buf) => btoa(String.fromCharCode(...new Uint8Array(buf))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  const unb64u = (s) => Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));
  const gestureText = () => ({
    fr: ["Continuer", "Annuler", "Vérifier votre identité"],
    ar: ["متابعة", "إلغاء", "تحقّق من هويتك"],
  })[(document.documentElement.lang || "en").slice(0, 2)] || ["Continue", "Cancel", "Verify it’s you"];
  // Popups and WebAuthn need a recent tap; after the server round trip it may
  // be gone. Then ask for one: `run` gets called from the button's click.
  function withGesture(title, run, cancel) {
    const [ok, no] = gestureText();
    const d = document.createElement("dialog");
    d.className = "prism-alert";
    d.setAttribute("role", "alertdialog");
    d.innerHTML = `<h2></h2><div class="prism-alert-actions"><button type="button" data-role="cancel"></button><button type="button" data-role="default"></button></div>`;
    d.querySelector("h2").textContent = title;
    const [bNo, bOk] = d.querySelectorAll("button");
    bNo.textContent = no;
    bOk.textContent = ok;
    const close = () => { d.close(); d.remove(); };
    bOk.addEventListener("click", () => { close(); run(); });
    bNo.addEventListener("click", () => { close(); cancel(); });
    d.addEventListener("cancel", (e) => { e.preventDefault(); close(); cancel(); });
    document.body.appendChild(d);
    d.showModal();
    bOk.focus();
  }
  function webAuth(msg, reply) {
    const url = msg.url == null ? null : safeUrl(msg.url);
    if (url == null) return reply({ err: "not a sign-in URL" });
    const open = () => {
      const w = Math.min(520, screen.width), h = Math.min(680, screen.height);
      const features = `popup,width=${w},height=${h},left=${Math.round((screen.width - w) / 2)},top=${Math.round((screen.height - h) / 2)}`;
      const popup = window.open(url, "prism-auth", features);
      if (!popup) return false;
      // The server answers when the provider redirects; a closed popup before
      // that is a cancel (the server ignores it once answered).
      const timer = setInterval(() => {
        if (popup.closed) { clearInterval(timer); setTimeout(() => reply({ err: "cancelled" }), 800); }
      }, 500);
      return true;
    };
    if (!open()) withGesture(msg.provider || location.host, () => { if (!open()) reply({ err: "popup blocked" }); }, () => reply({ err: "cancelled" }));
  }
  /** The pending conditional sign-in (#203), aborted by any other ceremony: it answers « cancelled ». */
  let conditional = null;
  function abortConditional() {
    if (!conditional) return;
    const { ctl, reply } = conditional;
    conditional = null;
    try { ctl.abort(); } catch {}
    reply({ err: "cancelled" });
  }
  /** An error as `name` U+001F message, when the platform named it (#189). */
  const named = (e) => {
    const message = String((e && e.message) || e);
    return e && typeof e.name === "string" && e.name && e.name !== "Error" ? `${e.name}\u001f${message}` : message;
  };
  async function webauthn(msg, reply) {
    if (!window.PublicKeyCredential || !navigator.credentials) return reply({ err: "unavailable" });
    abortConditional();
    const ceremony = async () => {
      if (msg.mode === "create") {
        const c = await navigator.credentials.create({ publicKey: {
          challenge: unb64u(msg.challenge),
          rp: { name: msg.rp || location.host },
          user: { id: unb64u(msg.user), name: msg.name || location.host, displayName: msg.name || location.host },
          pubKeyCredParams: [{ type: "public-key", alg: -7 }, { type: "public-key", alg: -257 }],
          authenticatorSelection: { authenticatorAttachment: "platform", userVerification: "required", residentKey: "discouraged" },
          attestation: "none", timeout: 60000,
        } });
        return { type: "create", id: b64u(c.rawId), client: b64u(c.response.clientDataJSON),
          auth: b64u(c.response.getAuthenticatorData()), key: b64u(c.response.getPublicKey()), alg: c.response.getPublicKeyAlgorithm() };
      }
      const c = await navigator.credentials.get({ publicKey: {
        challenge: unb64u(msg.challenge),
        allowCredentials: [{ type: "public-key", id: unb64u(msg.credential) }],
        userVerification: "required", timeout: 60000,
      } });
      return { type: "get", id: b64u(c.rawId), client: b64u(c.response.clientDataJSON),
        auth: b64u(c.response.authenticatorData), sig: b64u(c.response.signature) };
    };
    const error = (e) => ({ err: e && e.name === "NotAllowedError" ? "cancelled" : e && e.name === "SecurityError" ? "unavailable" : String((e && e.message) || e) });
    const started = Date.now();
    try {
      reply({ webauthn: await ceremony() });
    } catch (e) {
      // Refused at once: most likely no recent tap (Safari). Ask for one.
      if (e && e.name === "NotAllowedError" && Date.now() - started < 400) {
        withGesture(msg.reason || gestureText()[2], () => ceremony().then((r) => reply({ webauthn: r }), (e2) => reply(error(e2))), () => reply({ err: "cancelled" }));
      } else reply(error(e));
    }
  }
  /**
   * A ceremony of the app's own relying party (#94): the server's options
   * (WebAuthn JSON: binary fields in base64url), the credential back as JSON
   * (`toJSON()`), for the server to verify. Browsers that cannot parse or
   * serialize the JSON forms themselves get the same conversion here. As
   * with `authenticate`, a refusal at once (no recent tap) asks for a tap.
   */
  async function webauthnRp(msg, reply) {
    if (!window.PublicKeyCredential || !navigator.credentials || !msg.options) return reply({ err: "unavailable" });
    const PKC = window.PublicKeyCredential;
    const ids = (list) => (list || []).map((c) => ({ ...c, id: unb64u(c.id) }));
    const publicKey = msg.create
      ? (PKC.parseCreationOptionsFromJSON ? PKC.parseCreationOptionsFromJSON(msg.options) : {
        ...msg.options, challenge: unb64u(msg.options.challenge),
        user: { ...msg.options.user, id: unb64u(msg.options.user.id) },
        excludeCredentials: ids(msg.options.excludeCredentials),
      })
      : (PKC.parseRequestOptionsFromJSON ? PKC.parseRequestOptionsFromJSON(msg.options) : {
        ...msg.options, challenge: unb64u(msg.options.challenge),
        allowCredentials: ids(msg.options.allowCredentials),
      });
    const json = (c) => {
      if (typeof c.toJSON === "function") return c.toJSON();
      const r = c.response, out = { id: c.id, rawId: b64u(c.rawId), type: c.type,
        authenticatorAttachment: c.authenticatorAttachment || undefined,
        clientExtensionResults: c.getClientExtensionResults ? c.getClientExtensionResults() : {} };
      out.response = r.attestationObject
        ? { clientDataJSON: b64u(r.clientDataJSON), attestationObject: b64u(r.attestationObject),
          transports: r.getTransports ? r.getTransports() : undefined }
        : { clientDataJSON: b64u(r.clientDataJSON), authenticatorData: b64u(r.authenticatorData),
          signature: b64u(r.signature), userHandle: r.userHandle ? b64u(r.userHandle) : undefined };
      return out;
    };
    // A pending sign-in in a field's suggestions gives way (#203).
    abortConditional();
    if (msg.conditional && !msg.create) {
      // In the suggestions of a field marked `autocomplete="username webauthn"` (#203).
      const ok = typeof PKC.isConditionalMediationAvailable === "function" && await PKC.isConditionalMediationAvailable().catch(() => false);
      if (!ok || typeof AbortController !== "function") return reply({ err: "unavailable" });
      const ctl = new AbortController();
      conditional = { ctl, reply };
      try {
        const c = await navigator.credentials.get({ publicKey, mediation: "conditional", signal: ctl.signal });
        if (conditional && conditional.ctl === ctl) conditional = null;
        if (!c) return reply({ err: "cancelled" });
        reply({ text: JSON.stringify(json(c)) });
      } catch (e) {
        if (conditional && conditional.ctl === ctl) conditional = null;
        if (!ctl.signal.aborted) reply(e && e.name === "AbortError" ? { err: "cancelled" } : { err: named(e) });
      }
      return;
    }
    const ceremony = async () => {
      const c = msg.create ? await navigator.credentials.create({ publicKey }) : await navigator.credentials.get({ publicKey });
      if (!c) throw Object.assign(new Error("no credential"), { name: "NotAllowedError" });
      return JSON.stringify(json(c));
    };
    // Other errors keep their name (#189), as on Android and Windows:
    // `InvalidStateError` is a passkey already on this device,
    // `NotSupportedError` a request no authenticator here can do (not « no
    // passkeys here »).
    const error = (e) => ({ err: e && e.name === "NotAllowedError" ? "cancelled"
      : e && e.name === "SecurityError" ? "unavailable"
      : named(e) });
    const started = Date.now();
    try {
      reply({ text: await ceremony() });
    } catch (e) {
      if (e && e.name === "NotAllowedError" && Date.now() - started < 400) {
        withGesture(gestureText()[2], () => ceremony().then((t) => reply({ text: t }), (e2) => reply(error(e2))), () => reply({ err: "cancelled" }));
      } else reply(error(e));
    }
  }
  // Whether `authenticate` can work here (reported with the device).
  let uvpa = false;
  if (window.PublicKeyCredential && PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable) {
    PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable()
      .then((ok) => { uvpa = !!ok; if (ws && ws.readyState === 1) reportDevice(); })
      .catch(() => {});
  }

  async function device(msg) {
    const reply = (body) => send({ t: "device_reply", id: msg.id, ...body });
    const event = (body) => send({ t: "device_event", id: msg.id, ...body });
    try {
      switch (msg.call) {
        case "permission_status":
        case "request_permission":
          return reply({ permission: await permissionState(msg.permission, msg.call === "request_permission") });
        case "location":
          if (!navigator.geolocation) return reply({ err: "unavailable" });
          return navigator.geolocation.getCurrentPosition((p) => reply({ location: position(p) }), (e) => reply({ err: geoError(e) }), geoOptions(msg));
        case "watch_location": {
          if (!navigator.geolocation) return event({ err: "unavailable" });
          const w = navigator.geolocation.watchPosition((p) => event({ location: position(p) }), (e) => event({ err: geoError(e) }), geoOptions(msg));
          return streams.set(msg.id, () => navigator.geolocation.clearWatch(w));
        }
        case "watch_motion":
          if (!("DeviceMotionEvent" in window)) return event({ err: "unavailable" });
          streams.set(msg.id, null);
          return streams.set(msg.id, watchMotion(msg, event));
        case "stop": {
          const stop = streams.get(msg.id);
          streams.delete(msg.id);
          return stop && stop();
        }
        case "pick_images": {
          const input = document.createElement("input");
          input.type = "file";
          input.accept = "image/*";
          input.multiple = msg.limit !== 1;
          if (msg.camera) input.capture = "environment";
          input.addEventListener("cancel", () => reply({ err: "cancelled" }));
          input.addEventListener("change", () => {
            let files = [...input.files];
            if (msg.limit) files = files.slice(0, msg.limit);
            upload(msg, files, reply);
          });
          return input.click();
        }
        case "pick_files": {
          // Bytes go up over the socket in chunks, like photos (the server keeps them).
          const input = document.createElement("input");
          input.type = "file";
          input.accept = (msg.types || []).join(",");
          input.multiple = !!msg.multiple;
          input.addEventListener("cancel", () => reply({ err: "cancelled" }));
          input.addEventListener("change", () => {
            upload(msg, [...input.files].slice(0, msg.multiple ? Infinity : 1), reply);
          });
          return input.click();
        }
        case "pick_folder": {
          // A folder and its files (#225): uploaded like picked files, each
          // named by its path (`Export/notes/a.md`).
          const input = document.createElement("input");
          input.type = "file";
          input.webkitdirectory = true;
          input.addEventListener("cancel", () => reply({ err: "cancelled" }));
          input.addEventListener("change", () => upload(msg, [...input.files], reply));
          return input.click();
        }
        case "paste":
          return reply({ text: await navigator.clipboard?.readText().catch(() => null) ?? null });
        case "copy": return navigator.clipboard?.writeText(msg.text);
        case "read_clipboard_item": return readClipboardItem(msg, reply);
        case "write_clipboard": return writeClipboard(msg.entries || []);
        case "haptic":
          return navigator.vibrate?.({ selection: 5, light: 10, medium: 20, heavy: 35, success: [10, 40, 10], warning: [20, 60, 20], error: [30, 50, 30, 50, 30] }[msg.kind] || 10);
        case "share":
          if (navigator.share) return navigator.share({ text: msg.text || undefined, url: msg.url || undefined }).catch(() => {});
          return navigator.clipboard?.writeText([msg.text, msg.url].filter(Boolean).join(" "));
        case "open_url": return openUrl(msg.url, msg.target);
        // A file the app holds (#210): the server answered already, the
        // browser downloads it from the session's URL.
        case "save_file": {
          const a = document.createElement("a");
          a.href = msg.url;
          a.download = msg.name || "";
          a.rel = "noopener";
          a.style.display = "none";
          document.body.appendChild(a);
          a.click();
          return a.remove();
        }
        case "set_cookie": return setCookie(msg, reply);
        case "web_auth": return webAuth(msg, reply);
        case "webauthn": return webauthn(msg, reply);
        case "webauthn_rp": return webauthnRp(msg, reply);
        default: return reply({ err: "unavailable" });
      }
    } catch (e) {
      reply({ err: String((e && e.message) || e) });
    }
  }

  /**
   * `cx.open_url`: a URL of this page's origin replaces the page (a host's
   * sign-out page, a download); others open in a new tab. `target` ("same",
   * "new") overrides it (`cx.open_url_in`).
   */
  function openUrl(url, target) {
    url = url == null ? null : safeUrl(url);
    if (url == null) return;
    let same = false;
    try { same = new URL(url, location.href).origin === location.origin; } catch (e) { same = false; }
    if (target === "same" || (target !== "new" && same)) return location.assign(url);
    return window.open(url, "_blank", "noopener");
  }

  /**
   * `cx.set_cookie` (#91): the socket cannot set a cookie, so the page posts
   * the cookie's one-time ticket to the app (same origin, in a header), whose
   * answer sets it. One after the other, in the order the app set them;
   * `cookies` settles once the browser has them all. The server answers the
   * app when the ticket is redeemed: only failures are reported from here.
   */
  let cookies = Promise.resolve();
  function setCookie(msg, reply) {
    if (typeof fetch !== "function") return reply({ err: "unavailable" });
    cookies = cookies.then(() => fetch(msg.url, {
      method: "POST", credentials: "same-origin", cache: "no-store", redirect: "error",
      headers: { "X-Prism-Cookie": msg.ticket },
    })).then((r) => {
      if (r.status !== 204) reply({ err: `the cookie was refused (${r.status})` });
    }, (e) => reply({ err: String((e && e.message) || e) }));
    return cookies;
  }

  /**
   * S10: a sign-in gave the browser a new id. The page stores it with its
   * one-time ticket, in the order of the cookies (those asked for before it
   * are bound to the old id), and applies the server's next messages once
   * it has it: what they show (pictures, files of this browser) is fetched
   * with the new id. A fetch that never ends stops holding them after 10 s.
   */
  function renewBrowserId(msg) {
    if (typeof fetch !== "function") return;
    const signal = typeof AbortSignal === "function" && AbortSignal.timeout
      ? AbortSignal.timeout(10000) : undefined;
    cookies = cookies.then(() => fetch(msg.url, {
      method: "POST", credentials: "same-origin", cache: "no-store", redirect: "error", signal,
      headers: { "X-Prism-Cookie": msg.ticket },
    })).then((r) => {
      if (r.status !== 204) console.warn(`prism: the browser's new id was refused (${r.status})`);
    }, (e) => console.warn("prism: the browser's new id was not stored", e));
    const stored = cookies;
    const step = (inbox || Promise.resolve()).then(() => stored);
    inbox = step;
    step.then(() => { if (inbox === step) inbox = null; });
  }

  function service(msg) {
    switch (msg.t) {
      // The session ended for lack of activity: reconnect at the next one.
      case "idle": conn.idle = true; return;
      // The server refused the session (foreign origin, too many sessions).
      case "refused": conn.refused = msg.status || 0; return;
      // `cx.reconnect()`: once the cookies set before it are stored, a new
      // session at once (the host sees them in its handshake), no banner.
      case "reconnect": {
        const socket = ws;
        conn.renew = true;
        cookies.then(() => { if (socket && socket.readyState < 2) socket.close(); });
        return;
      }
      case "device": return device(msg);
      case "drop_stop": if (dropping.has(msg.drop)) dropping.set(msg.drop, true); return;
      case "browser_id": return renewBrowserId(msg);
      case "activity": return showActivity(msg.id, msg.c);
      case "activity_end": {
        // Show the final state for a moment, like the lock screen does.
        const remove = () => { activities.get(msg.id)?.remove(); activities.delete(msg.id); };
        if (msg.c && !msg.immediately && activities.has(msg.id)) {
          showActivity(msg.id, msg.c);
          setTimeout(remove, 4000);
        } else remove();
        return;
      }
      case "notify":
        clearTimeout(pending.get(msg.id));
        if (msg.delay) pending.set(msg.id, setTimeout(() => notify(msg), msg.delay));
        else notify(msg);
        return;
      case "notify_cancel": clearTimeout(pending.get(msg.id)); pending.delete(msg.id); return;
      case "alert": return showAlert(msg.a);
      case "notify_permission":
        if ("Notification" in window && Notification.permission === "default") Notification.requestPermission();
        return;
      // The app icon's badge (#239), where the browser has the Badging API.
      case "badge":
        if (msg.n > 0) navigator.setAppBadge?.(msg.n)?.catch?.(() => {});
        else navigator.clearAppBadge?.()?.catch?.(() => {});
        return;
      case "push_register": return pushRegister(msg.key);
      case "push_unregister": return pushUnregister();
      case "font": return loadFont(msg);
      case "sheets": return syncSheets(msg.urls || []);
      // `cx.set_title` (#154): the tab's title, over the screens'.
      case "title": appTitle = msg.title || ""; return showTitle();
      // The app menus' shortcuts (#161): taken before the browser.
      case "shortcuts": shortcuts = new Map((msg.list || []).map(([keys, id]) => [keys, id])); return;
      // `cx.set_unsaved_changes` (#158): leaving the page asks first.
      case "unsaved": unsaved = !!msg.on; return;
    }
  }

  // --- Run-time fonts and stylesheets (`cx.load_font`, `cx.add_stylesheet`) ----------
  const fontsLoaded = new Set();
  /** Registers a font face for this page: a URL or the file (base64url). */
  function loadFont(msg) {
    const key = JSON.stringify([msg.family, msg.url || msg.data, msg.weight, msg.style]);
    if (fontsLoaded.has(key) || !("FontFace" in window) || !document.fonts) return;
    fontsLoaded.add(key);
    const desc = { style: msg.style || "normal", display: "swap" };
    if (msg.weight) desc.weight = msg.weight;
    try {
      const source = msg.url ? `url(${JSON.stringify(msg.url)})` : unb64u(msg.data);
      const face = new FontFace(msg.family, source, desc);
      document.fonts.add(face);
      face.load().catch((err) => console.warn(`prism: font ${msg.family}`, err));
    } catch (err) {
      console.warn(`prism: font ${msg.family}`, err);
    }
  }
  /** Makes the page's app stylesheets exactly `urls` (the first paint's included). */
  function syncSheets(urls) {
    const want = new Set(urls);
    for (const link of document.querySelectorAll("link[data-prism-sheet]")) {
      if (want.has(link.getAttribute("href"))) want.delete(link.getAttribute("href"));
      else link.remove();
    }
    for (const url of want) {
      const link = document.createElement("link");
      link.setAttribute("rel", "stylesheet");
      link.setAttribute("href", url);
      link.setAttribute("data-prism-sheet", "");
      if (styleNonce) link.nonce = styleNonce;
      document.head.appendChild(link);
    }
  }

  // --- Device info: touch, installed app, screen, orientation, foldables ------------
  let hints = null;
  const media = (q) => (window.matchMedia ? matchMedia(q) : null);
  const standalone = () => !!(media("(display-mode: standalone)")?.matches
    || media("(display-mode: fullscreen)")?.matches || navigator.standalone === true);
  const segments = () => {
    const segs = (window.viewport && window.viewport.segments) || (window.getWindowSegments && window.getWindowSegments()) || null;
    return segs && segs.length > 1 ? Array.from(segs, (r) => [r.x ?? r.left, r.y ?? r.top, r.width, r.height]) : null;
  };
  const deviceReport = () => {
    const o = screen.orientation;
    const touch = navigator.maxTouchPoints > 0;
    const r = {
      t: "device", ua: navigator.userAgent, touch, standalone: standalone(),
      sw: screen.width, sh: screen.height, dpr: devicePixelRatio || 1,
      orientation: o ? o.type : (media("(orientation: landscape)")?.matches || screen.width > screen.height ? "landscape" : "portrait"),
      angle: o ? o.angle : (typeof window.orientation === "number" ? window.orientation : 0),
      posture: navigator.devicePosture ? navigator.devicePosture.type : null,
      segments: segments(),
      uvpa,
    };
    if (hints) r.hints = hints;
    document.cookie = `prism_dev=${touch ? 1 : 0}_${r.standalone ? 1 : 0}_${r.sw}_${r.sh}_${r.dpr}; Path=/; Max-Age=31536000; SameSite=Lax`;
    return r;
  };
  const reportDevice = () => send(deviceReport());
  if (navigator.userAgentData && navigator.userAgentData.getHighEntropyValues) {
    navigator.userAgentData.getHighEntropyValues(["platformVersion", "model", "fullVersionList"])
      .then((h) => { hints = { platform: h.platform, platformVersion: h.platformVersion, model: h.model, mobile: h.mobile, fullVersionList: h.fullVersionList }; reportDevice(); })
      .catch(() => {});
  }
  if (screen.orientation) screen.orientation.addEventListener("change", reportDevice);
  else addEventListener("orientationchange", reportDevice);
  if (navigator.devicePosture) navigator.devicePosture.addEventListener("change", reportDevice);
  media("(display-mode: standalone)")?.addEventListener?.("change", reportDevice);
  // Spanning a hinge or not changes with the window size.
  let lastSegments = JSON.stringify(segments());
  addEventListener("resize", () => {
    const now = JSON.stringify(segments());
    if (now !== lastSegments) { lastSegments = now; reportDevice(); }
  });
  deviceReport();

  // --- Connection: back-off, a visible state, the host's sign-in ---------------------
  // `<html data-prism-connection="connecting|online|reconnecting|offline|idle">`
  // styles the page (prism.css dims it and shows #prism-connection while
  // reconnecting); `prism:connection` events on window tell scripts.
  const conn = { attempt: 0, timer: null, idle: false, refused: 0, opened: false, renew: false };
  function setConnection(state) {
    const root = document.documentElement;
    if (root.getAttribute("data-prism-connection") === state) return;
    root.setAttribute("data-prism-connection", state);
    const banner = document.getElementById("prism-connection");
    if (banner) banner.hidden = state !== "reconnecting" && state !== "offline";
    if (typeof CustomEvent === "function" && window.dispatchEvent) {
      window.dispatchEvent(new CustomEvent("prism:connection", { detail: { state, attempt: conn.attempt } }));
    }
  }
  /** Retry delay: 0.5 s doubling to 30 s, ±25 % so pages do not reconnect in step. */
  const backoff = (attempt) => Math.min(30000, 500 * 2 ** Math.min(attempt - 1, 6)) * (0.75 + Math.random() / 2);
  /**
   * The socket keeps failing: is it the host refusing us (signed out: 401,
   * 403, a redirect to its sign-in page)? A WebSocket error cannot say, a
   * fetch of the page can; reload then, so the host's own answer shows.
   */
  function probe() {
    if (typeof fetch !== "function") return;
    fetch(location.href, { cache: "no-store", credentials: "same-origin", redirect: "manual", headers: { "X-Prism-Probe": "1" } })
      .then((r) => {
        if (r.type === "opaqueredirect" || r.redirected || r.status === 401 || r.status === 403) reload();
        else if (conn.attempt) setConnection("reconnecting");
      }, () => { if (conn.attempt) setConnection("offline"); });
  }
  /** Reloads, at most once every 30 s (a host that keeps refusing cannot loop us). */
  function reload() {
    let last = 0;
    try { last = Number(sessionStorage.getItem("prism-reload")) || 0; } catch (e) { last = 0; }
    if (Date.now() - last < 30000) return;
    try { sessionStorage.setItem("prism-reload", String(Date.now())); } catch (e) { /* private mode */ }
    location.reload();
  }
  function closed() {
    ws = null;
    if (conn.renew) {
      conn.renew = false;
      clearTimeout(conn.timer);
      return cookies.then(connect);
    }
    if (conn.idle) return setConnection("idle");
    conn.attempt++;
    setConnection(conn.opened || conn.attempt > 1 ? "reconnecting" : "connecting");
    if (conn.attempt >= 2 || conn.refused) probe();
    conn.refused = 0;
    clearTimeout(conn.timer);
    conn.timer = setTimeout(connect, backoff(conn.attempt));
  }
  /** Back online, shown again or touched after idling: try now. */
  function retryNow() {
    if (ws || (!conn.idle && !conn.attempt)) return;
    clearTimeout(conn.timer);
    conn.idle = false;
    connect();
  }
  addEventListener("online", retryNow);
  document.addEventListener("visibilitychange", () => { if (!document.hidden) retryNow(); });
  for (const t of ["pointerdown", "keydown", "focus"]) addEventListener(t, () => { if (conn.idle) retryNow(); }, true);

  // Compressed messages: with `DecompressionStream`, the page asks for large
  // messages as binary frames of raw DEFLATE (`prism_web::Frame`). Without
  // it, the server sends text only.
  const inflate = (() => {
    try {
      if (typeof DecompressionStream !== "function" || typeof Response !== "function") return null;
      new DecompressionStream("deflate-raw");
    } catch (e) {
      return null;
    }
    return (data) => new Response(new Response(data).body.pipeThrough(new DecompressionStream("deflate-raw"))).text();
  })();
  // Messages still being inflated, applied in arrival order: a small text
  // patch never overtakes a large compressed one sent before it.
  let inbox = null;
  function onFrame(socket, data) {
    if (typeof data === "string" && !inbox) return receive(data);
    // Inflating starts now, in parallel with the messages before it.
    const text = typeof data === "string" ? Promise.resolve(data)
      : inflate ? inflate(data) : Promise.reject(new Error("compressed message without DecompressionStream"));
    text.catch(() => {}); // handled below, once its turn comes
    const step = (inbox || Promise.resolve()).then(() => text).then(receive, (err) => {
      // A message is lost: the page no longer matches the server. Reconnect
      // for a fresh tree.
      console.error("prism: cannot read a message", err);
      if (socket.readyState < 2) socket.close();
    }).catch((err) => console.error(err));
    inbox = step;
    step.then(() => { if (inbox === step) inbox = null; });
  }
  /** Applies one message from the server: patches, or a service message. */
  function receive(text) {
    const msg = JSON.parse(text);
    if (!msg.p) return service(msg);
    if (msg.t === "reset") {
      sceneResetStart();
      nodes.clear(); clearStates(); stacks = []; hist = null; userNavigated = false;
      globalKeys = 0; placedOverlays.clear(); pendingOverlays.clear(); layerHost.replaceChildren();
      document.querySelectorAll("datalist[id^=prism-list-]").forEach((d) => d.remove());
      appTitle = msg.title || "";
      showTitle();
      unsaved = !!msg.unsaved;
    }
    quiet = msg.t === "reset";
    msg.p.forEach(apply);
    if (radiosDirty) roveRadios();
    flushOverlays();
    focusAsked = null;
    flushStates();
    reportLayouts();
    // Other people's carets follow the layout this batch changed (#240).
    queueCarets();
    if (msg.t === "reset") sceneResetEnd();
    quiet = false;
    // The live tree replaced the first paint: its style rules can go, and
    // the app's stylesheets are the session's.
    if (msg.t === "reset") {
      document.getElementById("prism-ssr")?.remove();
      syncSheets(msg.sheets || []);
    }
    syncHistory();
  }

  function connect() {
    const path = encodeURIComponent(appPath());
    const search = location.search ? `&search=${encodeURIComponent(location.search)}` : "";
    const compress = inflate ? "&compress=deflate-raw" : "";
    const page = pageId ? `&page=${encodeURIComponent(pageId)}` : "";
    pageId = "";
    const socket = ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}${BASE}/ws?path=${path}&tz=${tz}&w=${innerWidth}&h=${innerHeight}${search}${compress}${page}`);
    ws.binaryType = "arraybuffer";
    ws.onmessage = (e) => onFrame(socket, e.data);
    ws.onclose = closed;
    ws.addEventListener("open", () => { conn.attempt = 0; conn.opened = true; setConnection("online"); });
    ws.addEventListener("open", reportLifecycle);
    ws.addEventListener("open", reportDevice);
    // The insets are not in the query string: report them when there are any.
    ws.addEventListener("open", () => { if (safeArea().some((v) => v > 0)) reportViewport(); });
    ws.addEventListener("open", () => { pushReady().catch(() => {}); });
  }
  sceneBoot();
  setConnection("connecting");
  connect();
})();
