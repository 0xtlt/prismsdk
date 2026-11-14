// Prism web client: applies server patches to the DOM, sends events back.
(() => {
  const mount = document.getElementById("prism-root");
  const nodes = new Map();
  const tags = { view: "div", text: "div", button: "button", text_input: "input" };
  let ws;

  const send = (msg) => ws && ws.readyState === 1 && ws.send(JSON.stringify(msg));

  function create(id, kind) {
    const el = document.createElement(tags[kind]);
    el.dataset.pid = id;
    el.dataset.kind = kind;
    if (kind === "text_input") {
      el.type = "text";
      el.addEventListener("input", () => send({ t: "input", id, v: el.value }));
    } else {
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

  function apply(p) {
    const el = nodes.get(p.id);
    switch (p.op) {
      case "create": return create(p.id, p.kind);
      case "style":
        el.style.cssText = p.css + (el.dataset.clickable === "1" && el.dataset.kind === "view" ? "cursor:pointer;" : "");
        return;
      case "text":
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
      case "children": return setChildren(el, p.children);
      case "root": return mount.replaceChildren(el);
      case "destroy": el && el.remove(); nodes.delete(p.id); return;
    }
  }

  function connect() {
    ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`);
    ws.onmessage = (e) => {
      const msg = JSON.parse(e.data);
      if (msg.t === "reset") nodes.clear();
      msg.p.forEach(apply);
    };
    ws.onclose = () => setTimeout(connect, 1000);
  }
  connect();
})();
