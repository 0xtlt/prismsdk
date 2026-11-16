// <time-ago datetime="1731744000000">: "12 seconds ago", ticking in the
// browser. A plain Web Component, shown from Rust with
// `web_component("time-ago").module("time-ago.js").prop("datetime", &ms)`:
// Prism sets `datetime` (an attribute and a property) and imports this file
// the first time a page shows the element. It renders into its shadow root,
// so the server's fallback text shows until then (and without JavaScript).
const STEPS = [[60, "second"], [60, "minute"], [24, "hour"], [7, "day"], [4.35, "week"], [12, "month"], [Infinity, "year"]];

class TimeAgo extends HTMLElement {
  static observedAttributes = ["datetime"];

  constructor() {
    super();
    this.attachShadow({ mode: "open" });
    this.out = document.createElement("span");
    this.shadowRoot.append(this.out);
  }

  /** Milliseconds since 1970 (also the `datetime` attribute). */
  get datetime() { return Number(this.getAttribute("datetime")); }
  set datetime(ms) { this.setAttribute("datetime", String(ms)); }

  connectedCallback() {
    this.render();
    this.timer = setInterval(() => this.render(), 1000);
  }

  disconnectedCallback() { clearInterval(this.timer); }

  attributeChangedCallback() { this.render(); }

  render() {
    const at = this.datetime;
    if (!Number.isFinite(at)) return;
    let delta = (at - Date.now()) / 1000;
    let unit = "second";
    for (const [size, name] of STEPS) {
      unit = name;
      if (Math.abs(delta) < size) break;
      delta /= size;
    }
    const lang = document.documentElement.lang || undefined;
    const text = new Intl.RelativeTimeFormat(lang, { numeric: "auto" }).format(Math.round(delta), unit);
    if (this.out.textContent !== text) this.out.textContent = text;
    this.title = new Date(at).toLocaleString(lang);
  }
}

if (!customElements.get("time-ago")) customElements.define("time-ago", TimeAgo);
