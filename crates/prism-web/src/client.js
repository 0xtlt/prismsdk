// Prism web client: applies server patches to the DOM, sends events back,
// mirrors navigation to the URL bar (History API), and implements the ARIA
// patterns (roles, keyboard, focus) for every node kind.
(() => {
  // Where the app is mounted (`/app` when nested in another server's routes).
  const BASE = ((document.currentScript && document.currentScript.dataset.base) || "").replace(/\/$/, "");
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
  const baseTitle = document.title.split(" · ").pop();
  const nodes = new Map();
  const tags = {
    view: "div", text: "div", button: "button", text_input: "input",
    stack: "div", screen: "section", toggle: "input", scroll: "div", tabs: "div", tab: "div",
    image: "span", progress: "progress", timer: "span", modal: "dialog", picture: "img",
    shape: "div", slider: "input", picker: "select", date_picker: "input", text_area: "textarea",
    spinner: "span",
  };
  // SF Symbol names → glyphs (table generated from Rust: glyphs.rs).
  const glyphs = __GLYPHS__;
  let stacks = [];
  let ws;
  let userNavigated = false;

  const send = (msg) => ws && ws.readyState === 1 && ws.send(JSON.stringify(msg));

  function activate(el, id) {
    if (el.dataset.clickable !== "1" || el.getAttribute("aria-disabled") === "true") return;
    send({ t: "click", id });
  }

  // Shapes (prism-core shape.rs): paths in fractions of the box become an
  // SVG in pixels, drawn again whenever the box is resized.
  const svgNS = "http://www.w3.org/2000/svg";
  const shapeObserver = typeof ResizeObserver === "function"
    ? new ResizeObserver((entries) => entries.forEach((e) => drawShape(e.target)))
    : null;
  const shapeColor = (s) => {
    if (!s) return "none";
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
      }
      paths.push(p);
    }
    const iw = Math.max(0, w - 2 * ix), ih = Math.max(0, h - 2 * iy), side = Math.min(iw, ih);
    const X = (x) => ix + x * iw, Y = (y) => iy + y * ih;
    let svg = el._svg;
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
      return e;
    }));
  }

  function create(id, kind) {
    const el = document.createElement(tags[kind]);
    el.dataset.pid = id;
    el.dataset.kind = kind;
    if (kind === "text_input") {
      el.type = "text";
      el.addEventListener("input", () => send({ t: "input", id, v: el.value }));
      el.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && !e.isComposing) send({ t: "submit", id });
      });
    } else if (kind === "text_area") {
      el.className = "prism-textarea";
      el.rows = 3;
      el.addEventListener("input", () => { grow(el); send({ t: "input", id, v: el.value }); });
    } else if (kind === "slider") {
      el.type = "range";
      el.className = "prism-slider";
      el.addEventListener("input", () => send({ t: "change", id, v: Number(el.value) }));
    } else if (kind === "picker") {
      el.className = "prism-select";
      el._ph = document.createElement("option");
      Object.assign(el._ph, { disabled: true, hidden: true, value: "" });
      el.appendChild(el._ph);
      el._index = -1;
      el.addEventListener("change", () => send({ t: "change", id, v: el.selectedIndex - 1 }));
    } else if (kind === "date_picker") {
      el.type = "date";
      el.className = "prism-date";
      el.addEventListener("change", () => {
        const secs = dateSeconds(el);
        if (secs !== null) send({ t: "change", id, v: secs });
      });
    } else if (kind === "spinner") {
      el.className = "prism-spinner";
    } else if (kind === "toggle") {
      el.type = "checkbox";
      el.className = "prism-switch";
      el.setAttribute("role", "switch");
      el.addEventListener("change", () => send({ t: "change", id, v: el.checked ? 1 : 0 }));
    } else if (kind === "stack") {
      el.className = "prism-stack";
      el.innerHTML =
        '<div class="prism-header"><button class="prism-back" aria-label="Back" hidden>‹ Back</button>' +
        '<h1 class="prism-title" tabindex="-1"></h1></div><div class="prism-body"></div>';
      el._body = el.lastChild;
      el._header = el.firstChild;
      el._back = el.querySelector(".prism-back");
      el._title = el.querySelector(".prism-title");
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
      el.addEventListener("cancel", (e) => { e.preventDefault(); send({ t: "back", id, depth: 0 }); });
    } else if (kind === "scroll") {
      el.className = "prism-scroll";
    } else if (kind === "picture") {
      el.className = "prism-picture";
      el.loading = "lazy";
      el.decoding = "async";
      el.alt = "";
    } else if (kind === "shape") {
      el.className = "prism-shape";
      if (shapeObserver) shapeObserver.observe(el);
    } else if (kind === "image") {
      el.className = "prism-symbol";
    } else if (kind === "progress") {
      el.max = 1;
    } else if (kind === "timer") {
      el.className = "prism-timer";
    } else if (kind === "view") {
      el.addEventListener("click", (e) => {
        if (el.dataset.clickable !== "1") return;
        e.stopPropagation();
        activate(el, id);
      });
      // Keyboard activation for custom buttons (ARIA button pattern).
      el.addEventListener("keydown", (e) => {
        if (el.dataset.clickable !== "1" || e.target !== el) return;
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          activate(el, id);
        }
      });
    } else if (kind === "button") {
      el.type = "button";
      el.dataset.clickable = "1";
      el.addEventListener("click", (e) => {
        e.stopPropagation();
        activate(el, id);
      });
    }
    nodes.set(id, el);
  }

  // Orders Prism children; helper elements (no data-pid) are left in place.
  // --- Motion (prism-core `animation`) ------------------------------------------
  const reducedMotion = window.matchMedia ? matchMedia("(prefers-reduced-motion: reduce)") : null;
  /** Set while a reconnect rebuilds the page: what is shown does not enter again. */
  let quiet = false;
  const poseCss = (p) => {
    const d = document.documentElement.dir === "rtl" ? -1 : 1;
    const [x, y, s, r] = p.tf;
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
        el._loop = el.animate([poseCss(m.rest), poseCss(m.repeat.to)],
          timing(m.repeat.t, { iterations: Infinity, direction: "alternate", easing: "ease-in-out" }));
      }
    }
    if (!m || !m.enter || quiet || reducedMotion?.matches) return;
    if (!first && !(wasHidden && el.style.display !== "none")) return;
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
    if (top) top.setAttribute("aria-label", title);
    if (isMainStack(st)) {
      document.title = title && title !== baseTitle ? `${title} · ${baseTitle}` : baseTitle;
    }
  }

  // --- Tabs: ARIA tablist with roving tabindex ----------------------------------
  function selectTab(tabs, index, fromUser) {
    if (fromUser) {
      userNavigated = true;
      send({ t: "change", id: Number(tabs.dataset.pid), v: index });
    }
    tabs._selected = index;
    refreshTabs(tabs);
    if (fromUser) tabs._bar.children[index]?.focus();
  }

  function refreshTabs(tabs) {
    const panels = tabs._tabs.map((i) => nodes.get(i)).filter(Boolean);
    const bar = tabs._bar;
    while (bar.children.length > panels.length) bar.lastElementChild.remove();
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
      panel.hidden = !selected;
    });
    const s = navState();
    if (s && s.stack) refreshStack(s.stack);
  }

  // --- Form controls (prism-core `controls`) -------------------------------------
  /** A text area as tall as its text (its max-height then scrolls it). */
  function grow(el) {
    // Not in the page yet (just created): measure once it is.
    if (!el.isConnected) return requestAnimationFrame(() => el.isConnected && grow(el));
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight + el.offsetHeight - el.clientHeight}px`;
  }
  const pad = (n) => String(n).padStart(2, "0");
  /** Unix seconds as an `<input type=date|time|datetime-local>` value (local time). */
  function dateString(secs, type) {
    const d = new Date(secs * 1000);
    const day = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
    const time = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
    return type === "date" ? day : type === "time" ? time : `${day}T${time}`;
  }
  /** The input's value as Unix seconds, keeping the part it does not show. */
  function dateSeconds(el) {
    const v = el.value;
    if (!v) return null;
    const b = new Date((el._secs || 0) * 1000);
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
  /** Keyboard, return key, capitalization, autocorrection (mirrors html.rs input_attrs). */
  function applyInputConfig(el, config) {
    const t = new Set(config.split(" ").filter(Boolean));
    const get = (k) => { for (const x of t) if (x.startsWith(k + "=")) return x.slice(k.length + 1); return null; };
    const secure = t.has("secure");
    const kb = get("keyboard");
    if (el.tagName === "INPUT") {
      el.type = secure ? "password" : { email: "email", url: "url", phone: "tel" }[kb] || "text";
      if (secure) el.autocomplete = "current-password";
    }
    const set = (name, v) => (v ? el.setAttribute(name, v) : el.removeAttribute(name));
    set("inputmode", { number: "numeric", decimal: "decimal" }[kb]);
    set("enterkeyhint", get("return"));
    set("autocapitalize", secure ? "none" : { none: "none", words: "words", all: "characters" }[get("capitalize")]);
    const noCorrect = secure || t.has("autocorrect=off");
    set("autocorrect", noCorrect ? "off" : null);
    el.spellcheck = !noCorrect;
  }
  function applyConfig(el, config) {
    const kind = el.dataset.kind;
    if (kind === "slider") {
      const [min, max, step] = config.split(" ").map(Number);
      Object.assign(el, { min, max, step: step > 0 ? step : "any" });
      if (el._v !== undefined) el.value = el._v;
    } else if (kind === "picker") {
      while (el.options.length > 1) el.lastChild.remove();
      (config ? config.split("\n") : []).forEach((text, i) => el.appendChild(new Option(text, String(i))));
      el.selectedIndex = el._index + 1;
    } else if (kind === "date_picker") {
      const [mode, min, max] = config.split(" ");
      el.type = { time: "time", datetime: "datetime-local" }[mode] || "date";
      el.min = mode !== "time" && min !== "-" ? dateString(Number(min), el.type) : "";
      el.max = mode !== "time" && max !== "-" ? dateString(Number(max), el.type) : "";
      if (el._secs !== undefined) el.value = dateString(el._secs, el.type);
    } else if (kind === "text_input" || kind === "text_area") {
      applyInputConfig(el, config);
    }
  }

  function applyA11y(el, a) {
    const kind = el.dataset.kind;
    const set = (name, v) =>
      v === undefined || v === null || v === false ? el.removeAttribute(name) : el.setAttribute(name, v === true ? "true" : v);
    set("aria-hidden", a.hidden || null);
    if (kind === "stack" || kind === "tabs" || kind === "tab" || kind === "screen") return;
    const native = { button: "button", toggle: "switch", slider: "slider", picker: "combobox" }[kind];
    const role = a.role;
    if (role && role !== "text" && role !== native) {
      set("role", role);
      set("aria-level", role === "heading" ? String(a.level || 2) : null);
    } else if (kind !== "toggle") {
      set("role", null);
      set("aria-level", null);
    }
    set("aria-label", a.label || null);
    set("aria-description", a.hint || null);
    if (role === "switch" || role === "checkbox") set("aria-checked", kind === "toggle" ? null : String(!!a.checked));
    else set("aria-checked", null);
    set("aria-selected", role === "tab" ? String(!!a.checked) : null);
    // A button with a state is a toggle button.
    set("aria-pressed", role === "button" && a.checked !== null && a.checked !== undefined ? String(a.checked) : null);
    set("aria-valuetext", role === "slider" ? a.value || null : null);
    // A polite live region: what it shows is announced.
    set("aria-live", role === "status" ? "polite" : null);
    set("aria-disabled", a.disabled || null);
    if (["INPUT", "BUTTON", "SELECT", "TEXTAREA"].includes(el.tagName)) el.disabled = !!a.disabled;
    // Custom interactive elements must be reachable from the keyboard.
    const interactive = ["button", "link", "switch", "checkbox", "tab", "slider", "combobox"].includes(role);
    if (kind === "view") set("tabindex", interactive && !a.hidden && !a.disabled ? "0" : null);
  }

  function apply(p) {
    const el = p.id === undefined ? null : nodes.get(p.id);
    switch (p.op) {
      case "create": return create(p.id, p.kind);
      case "style": {
        const hidden = el.style.display === "none";
        el.style.cssText = p.css;
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
        if (kind === "picture") {
          // The server resolves asset URLs (hashed, maybe on a CDN).
          const src = p.text;
          if (el.getAttribute("src") !== src) el.src = src;
          return;
        }
        if (el.tagName === "INPUT" || el.tagName === "TEXTAREA") {
          // Never fight the user's typing: a focused field owns its value.
          if (el.value !== p.text && document.activeElement !== el) el.value = p.text;
          if (el.tagName === "TEXTAREA") grow(el);
        } else {
          let t = el.firstChild && el.firstChild.nodeType === 3 ? el.firstChild : null;
          if (!t) { t = document.createTextNode(""); el.prepend(t); }
          t.data = p.text;
        }
        return;
      }
      case "placeholder":
        if (el._ph) el._ph.textContent = p.text;
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
        if (el.dataset.kind === "picker") { el._index = p.value; el.selectedIndex = p.value + 1; }
        if (el.dataset.kind === "date_picker") { el._secs = p.value; el.value = dateString(p.value, el.type); }
        if (el.dataset.kind === "spinner") el.dataset.size = p.value === 1 ? "large" : "";
        if (el.dataset.kind === "picture") el.style.objectFit = p.value === 1 ? "contain" : "cover";
        if (el.dataset.kind === "scroll") setRefreshing(el, p.value === 1);
        if (el.dataset.kind === "stack") { el._closable = (p.value & 1) === 1; el._noHeader = (p.value & 2) === 2; refreshStack(el); }
        if (el.dataset.kind === "screen") {
          el._bare = p.value === 1;
          const owner = el.parentElement && el.parentElement.parentElement;
          if (owner && owner._screens) refreshStack(owner);
        }
        if (el.dataset.kind === "timer") { el.dataset.end = p.value; tick(); }
        if (el.dataset.kind === "tabs") { el._selected = p.value; refreshTabs(el); }
        return;
      case "icon": {
        el._icon = p.icon;
        el._glyph = p.glyph;
        if (el.dataset.kind === "image") el.textContent = p.glyph;
        const owner = el.parentElement && el.parentElement.parentElement;
        if (owner && owner._tabs) refreshTabs(owner);
        return;
      }
      case "a11y":
        if (el.dataset.kind === "picture") { el.alt = p.a11y.hidden ? "" : p.a11y.label || ""; return; }
        return applyA11y(el, p.a11y);
      case "menu": return setMenu(el, p.id, p.menu);
      case "appearance": document.documentElement.style.colorScheme = p.value; return;
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
            if (!d.isConnected) { document.body.appendChild(d); d.showModal(); userNavigated = true; }
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
        } else {
          setChildren(el, p.children);
        }
        return;
      case "root": return mount.replaceChildren(el);
      case "destroy":
        if (el) el.remove();
        nodes.delete(p.id);
        stacks = stacks.filter((s) => s !== el);
        return;
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
    if (!stack || !stack._screens) return null;
    const path = root.dataset.kind === "tabs" ? root.dataset.path : stack.dataset.path;
    return { path: path || "/", depth: stack._screens.length, stack, tab };
  }

  const isMainStack = (st) => navState()?.stack === st;

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

  function showMenu(owner, spec, x, y) {
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
    menu.style.left = Math.max(8, Math.min(x, innerWidth - r.width - 8)) + "px";
    menu.style.top = Math.max(8, Math.min(y, innerHeight - r.height - 8)) + "px";
    openMenu = { el: menu, owner };
    menu.querySelector("[role^=menuitem]:not(:disabled)")?.focus();
  }

  function setMenu(el, id, spec) {
    el._menuSpec = spec;
    if (spec) el.setAttribute("aria-haspopup", "menu"); else el.removeAttribute("aria-haspopup");
    if (el._menuBound) return;
    el._menuBound = true;
    el.addEventListener("contextmenu", (e) => {
      if (!el._menuSpec) return;
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
      if (e.pointerType !== "touch" || !el._menuSpec) return;
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
  function tick() {
    document.querySelectorAll("[data-end]").forEach((el) => {
      const text = remaining(Number(el.dataset.end));
      if (el.dataset.kind === "timer") {
        let t = el.firstChild && el.firstChild.nodeType === 3 ? el.firstChild : null;
        if (!t) { t = document.createTextNode(""); el.prepend(t); }
        if (t.data !== text) t.data = text;
      } else if (el.textContent !== text) {
        el.textContent = text;
      }
    });
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
    if (n.k === "m") { el.dataset.end = n.v; st.fontVariantNumeric = "tabular-nums"; el.textContent = remaining(n.v); }
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
  let resizeTimer = null;
  const reportViewport = () => {
    document.cookie = `prism_vw=${innerWidth}x${innerHeight}; Path=/; Max-Age=31536000; SameSite=Lax`;
    send({ t: "viewport", w: innerWidth, h: innerHeight });
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
  const readFile = (f) => new Promise((resolve) => {
    const r = new FileReader();
    r.onload = () => resolve({ name: f.name, mime: f.type, data: String(r.result).split(",")[1] || "" });
    r.onerror = () => resolve(null);
    r.readAsDataURL(f);
  });
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
    const open = () => {
      const w = Math.min(520, screen.width), h = Math.min(680, screen.height);
      const features = `popup,width=${w},height=${h},left=${Math.round((screen.width - w) / 2)},top=${Math.round((screen.height - h) / 2)}`;
      const popup = window.open(msg.url, "prism-auth", features);
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
  async function webauthn(msg, reply) {
    if (!window.PublicKeyCredential || !navigator.credentials) return reply({ err: "unavailable" });
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
          input.addEventListener("change", async () => {
            let files = [...input.files];
            if (msg.limit) files = files.slice(0, msg.limit);
            reply({ files: (await Promise.all(files.map(readFile))).filter(Boolean) });
          });
          return input.click();
        }
        case "paste":
          return reply({ text: await navigator.clipboard?.readText().catch(() => null) ?? null });
        case "copy": return navigator.clipboard?.writeText(msg.text);
        case "haptic":
          return navigator.vibrate?.({ selection: 5, light: 10, medium: 20, heavy: 35, success: [10, 40, 10], warning: [20, 60, 20], error: [30, 50, 30, 50, 30] }[msg.kind] || 10);
        case "share":
          if (navigator.share) return navigator.share({ text: msg.text || undefined, url: msg.url || undefined }).catch(() => {});
          return navigator.clipboard?.writeText([msg.text, msg.url].filter(Boolean).join(" "));
        case "open_url": return window.open(msg.url, "_blank", "noopener");
        case "web_auth": return webAuth(msg, reply);
        case "webauthn": return webauthn(msg, reply);
        default: return reply({ err: "unavailable" });
      }
    } catch (e) {
      reply({ err: String((e && e.message) || e) });
    }
  }

  function service(msg) {
    switch (msg.t) {
      case "device": return device(msg);
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
      case "push_register": return pushRegister(msg.key);
      case "push_unregister": return pushUnregister();
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

  function connect() {
    const path = encodeURIComponent(appPath());
    ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}${BASE}/ws?path=${path}&tz=${tz}&w=${innerWidth}&h=${innerHeight}`);
    ws.onmessage = (e) => {
      const msg = JSON.parse(e.data);
      if (!msg.p) return service(msg);
      if (msg.t === "reset") { nodes.clear(); stacks = []; hist = null; userNavigated = false; }
      quiet = msg.t === "reset";
      msg.p.forEach(apply);
      quiet = false;
      syncHistory();
    };
    ws.onclose = () => setTimeout(connect, 1000);
    ws.addEventListener("open", reportLifecycle);
    ws.addEventListener("open", reportDevice);
    ws.addEventListener("open", () => { pushReady().catch(() => {}); });
  }
  connect();
})();
