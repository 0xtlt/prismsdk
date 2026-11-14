// Prism web client: applies server patches to the DOM, sends events back,
// mirrors navigation to the URL bar (History API), and implements the ARIA
// patterns (roles, keyboard, focus) for every node kind.
(() => {
  const mount = document.getElementById("prism-root");
  const baseTitle = document.title.split(" · ").pop();
  const nodes = new Map();
  const tags = {
    view: "div", text: "div", button: "button", text_input: "input",
    stack: "div", screen: "section", toggle: "input", scroll: "div", tabs: "div", tab: "div",
  };
  // SF Symbol names → web glyphs (decorative; the tab title is the label).
  const glyphs = {
    house: "⌂", "house.fill": "⌂", gear: "⚙", gearshape: "⚙", checklist: "☑", "list.bullet": "☰",
    star: "★", "star.fill": "★", person: "☺", "person.crop.circle": "☺", magnifyingglass: "⌕",
    bell: "🔔", heart: "♥", "square.grid.2x2": "▦", paintbrush: "🖌", paintpalette: "🎨",
  };
  let stacks = [];
  let ws;
  let userNavigated = false;

  const send = (msg) => ws && ws.readyState === 1 && ws.send(JSON.stringify(msg));

  function activate(el, id) {
    if (el.dataset.clickable !== "1" || el.getAttribute("aria-disabled") === "true") return;
    send({ t: "click", id });
  }

  function create(id, kind) {
    const el = document.createElement(tags[kind]);
    el.dataset.pid = id;
    el.dataset.kind = kind;
    if (kind === "text_input") {
      el.type = "text";
      el.addEventListener("input", () => send({ t: "input", id, v: el.value }));
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
    } else if (kind === "scroll") {
      el.className = "prism-scroll";
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

  function setChildren(el, ids) {
    const kids = ids.map((i) => nodes.get(i)).filter(Boolean);
    kids.forEach((kid, i) => {
      if (el.children[i] !== kid) el.insertBefore(kid, el.children[i] || null);
    });
    while (el.children.length > kids.length) el.lastElementChild.remove();
  }

  // --- Stacks: header (Back + title), only the top screen visible -------------
  function refreshStack(st) {
    const screens = st._screens.map((i) => nodes.get(i)).filter(Boolean);
    screens.forEach((s, i) => (s.hidden = i !== screens.length - 1));
    const top = screens[screens.length - 1];
    const title = (top && top._titleText) || "";
    st._back.hidden = screens.length <= 1;
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
      const glyph = glyphs[panel._icon] || "";
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

  function applyA11y(el, a) {
    const kind = el.dataset.kind;
    const set = (name, v) =>
      v === undefined || v === null || v === false ? el.removeAttribute(name) : el.setAttribute(name, v === true ? "true" : v);
    set("aria-hidden", a.hidden || null);
    if (kind === "stack" || kind === "tabs" || kind === "tab" || kind === "screen") return;
    const native = { button: "button", toggle: "switch" }[kind];
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
    set("aria-disabled", a.disabled || null);
    if (el.tagName === "INPUT" || el.tagName === "BUTTON") el.disabled = !!a.disabled;
    // Custom interactive elements must be reachable from the keyboard.
    const interactive = ["button", "link", "switch", "checkbox", "tab"].includes(role);
    if (kind === "view") set("tabindex", interactive && !a.hidden && !a.disabled ? "0" : null);
  }

  function apply(p) {
    const el = p.id === undefined ? null : nodes.get(p.id);
    switch (p.op) {
      case "create": return create(p.id, p.kind);
      case "style":
        el.style.cssText = p.css;
        return;
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
        if (el.tagName === "INPUT") {
          // Never fight the user's typing: a focused field owns its value.
          if (el.value !== p.text && document.activeElement !== el) el.value = p.text;
        } else {
          let t = el.firstChild && el.firstChild.nodeType === 3 ? el.firstChild : null;
          if (!t) { t = document.createTextNode(""); el.prepend(t); }
          t.data = p.text;
        }
        return;
      }
      case "placeholder": el.placeholder = p.text; return;
      case "clickable":
        el.dataset.clickable = p.on ? "1" : "0";
        return;
      case "value":
        if (el.dataset.kind === "toggle") el.checked = p.value !== 0;
        if (el.dataset.kind === "tabs") { el._selected = p.value; refreshTabs(el); }
        return;
      case "icon": {
        el._icon = p.icon;
        const owner = el.parentElement && el.parentElement.parentElement;
        if (owner && owner._tabs) refreshTabs(owner);
        return;
      }
      case "a11y": return applyA11y(el, p.a11y);
      case "appearance": document.documentElement.style.colorScheme = p.value; return;
      case "children":
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
      history.replaceState({ prism: 1, tab: s.tab }, "", "/");
      for (let d = 2; d <= s.depth; d++) history.pushState({ prism: d, tab: s.tab }, "", s.path);
    } else if (!hist || hist.tab !== s.tab) {
      history.replaceState({ prism: s.depth, tab: s.tab }, "", s.path);
    } else if (s.depth > hist.depth) {
      for (let d = hist.depth + 1; d <= s.depth; d++) history.pushState({ prism: d, tab: s.tab }, "", s.path);
    } else if (s.depth < hist.depth) {
      ignorePops++;
      history.go(s.depth - hist.depth);
    } else if (location.pathname !== s.path) {
      history.replaceState({ prism: s.depth, tab: s.tab }, "", s.path);
    }
    hist = { depth: s.depth, tab: s.tab };
  }

  window.addEventListener("popstate", (e) => {
    const s = navState();
    if (ignorePops > 0) {
      ignorePops--;
      if (s && location.pathname !== s.path) history.replaceState({ prism: hist.depth, tab: s.tab }, "", s.path);
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
      send({ t: "navigate", id, path: location.pathname });
    }
  });

  function connect() {
    const path = encodeURIComponent(location.pathname);
    ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws?path=${path}`);
    ws.onmessage = (e) => {
      const msg = JSON.parse(e.data);
      if (msg.t === "reset") { nodes.clear(); stacks = []; hist = null; userNavigated = false; }
      msg.p.forEach(apply);
      syncHistory();
    };
    ws.onclose = () => setTimeout(connect, 1000);
  }
  connect();
})();
