// Prism web client: applies server patches to the DOM, sends events back,
// and mirrors the root navigation stack to the URL bar (History API).
(() => {
  const mount = document.getElementById("prism-root");
  const baseTitle = document.title.split(" · ").pop();
  const nodes = new Map();
  const tags = { view: "div", text: "div", button: "button", text_input: "input", stack: "div", screen: "div" };
  let stacks = [];
  let ws;

  const send = (msg) => ws && ws.readyState === 1 && ws.send(JSON.stringify(msg));

  function create(id, kind) {
    const el = document.createElement(tags[kind]);
    el.dataset.pid = id;
    el.dataset.kind = kind;
    if (kind === "text_input") {
      el.type = "text";
      el.addEventListener("input", () => send({ t: "input", id, v: el.value }));
    } else if (kind === "stack") {
      el.className = "prism-stack";
      el.innerHTML =
        '<div class="prism-header"><button class="prism-back" hidden>‹ Back</button>' +
        '<div class="prism-title"></div></div><div class="prism-body"></div>';
      el._body = el.lastChild;
      el._back = el.querySelector(".prism-back");
      el._title = el.querySelector(".prism-title");
      el._screens = [];
      el._back.addEventListener("click", (e) => {
        e.stopPropagation();
        send({ t: "back", id, depth: el._screens.length - 1 });
      });
      stacks.push(el);
    } else if (kind !== "screen") {
      el.addEventListener("click", (e) => {
        if (el.dataset.clickable !== "1") return;
        e.stopPropagation();
        send({ t: "click", id });
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

  // Shows the top screen, its title, and Back when there is somewhere to go.
  function refreshStack(st) {
    const screens = st._screens.map((i) => nodes.get(i)).filter(Boolean);
    screens.forEach((s, i) => (s.hidden = i !== screens.length - 1));
    const top = screens[screens.length - 1];
    st._back.hidden = screens.length <= 1;
    st._title.textContent = (top && top._titleText) || "";
    if (st === rootStack()) {
      const t = top && top._titleText;
      document.title = t && t !== baseTitle ? `${t} · ${baseTitle}` : baseTitle;
    }
  }

  const rootStack = () => stacks.find((s) => s.isConnected);

  function apply(p) {
    const el = nodes.get(p.id);
    switch (p.op) {
      case "create": return create(p.id, p.kind);
      case "style":
        el.style.cssText = p.css + (el.dataset.clickable === "1" && el.dataset.kind === "view" ? "cursor:pointer;" : "");
        return;
      case "text":
        if (el.dataset.kind === "stack") { el.dataset.path = p.text; return; }
        if (el.dataset.kind === "screen") {
          el._titleText = p.text;
          const st = el.parentElement && el.parentElement.parentElement;
          if (st && st._screens) refreshStack(st);
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
      case "placeholder": el.placeholder = p.text; return;
      case "clickable":
        el.dataset.clickable = p.on ? "1" : "0";
        if (el.dataset.kind === "view") el.style.cursor = p.on ? "pointer" : "";
        return;
      case "children":
        if (el.dataset.kind === "stack") {
          el._screens = p.children;
          setChildren(el._body, p.children);
          refreshStack(el);
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

  // --- History: one browser entry per screen of the root stack ---------------
  let histDepth = null; // depth the browser history currently reflects
  let ignorePops = 0; // pops we caused ourselves via history.go()

  function syncHistory() {
    const st = rootStack();
    if (!st) return;
    const depth = st._screens.length;
    const path = st.dataset.path || "/";
    if (histDepth === null) {
      // Deep link: the router put the home screen underneath, give the
      // browser matching entries so its Back button works too.
      history.replaceState({ prism: 1 }, "", depth > 1 ? "/" : path);
      for (let d = 2; d <= depth; d++) history.pushState({ prism: d }, "", path);
    } else if (depth > histDepth) {
      for (let d = histDepth + 1; d <= depth; d++) history.pushState({ prism: d }, "", path);
    } else if (depth < histDepth) {
      ignorePops++;
      history.go(depth - histDepth);
    } else if (location.pathname !== path) {
      history.replaceState({ prism: depth }, "", path);
    }
    histDepth = depth;
  }

  window.addEventListener("popstate", (e) => {
    const st = rootStack();
    if (ignorePops > 0) {
      ignorePops--;
      if (st && location.pathname !== st.dataset.path) {
        history.replaceState({ prism: histDepth }, "", st.dataset.path);
      }
      return;
    }
    const depth = e.state && e.state.prism;
    if (!st || !depth) return location.reload();
    const id = Number(st.dataset.pid);
    if (depth < histDepth) {
      histDepth = depth;
      send({ t: "back", id, depth });
    } else if (depth > histDepth) {
      histDepth = depth;
      send({ t: "navigate", id, path: location.pathname });
    }
  });

  function connect() {
    const path = encodeURIComponent(location.pathname);
    ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws?path=${path}`);
    ws.onmessage = (e) => {
      const msg = JSON.parse(e.data);
      if (msg.t === "reset") { nodes.clear(); stacks = []; histDepth = null; }
      msg.p.forEach(apply);
      syncHistory();
    };
    ws.onclose = () => setTimeout(connect, 1000);
  }
  connect();
})();
