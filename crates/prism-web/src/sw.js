// Prism's service worker (served at <base>/prism-sw.js, scope <base>/): shows
// Web Push messages as notifications, tells open pages about them, and
// opens or focuses the app when one is clicked. Payloads are Prism's JSON
// (`prism-push`): title, subtitle, body, link, id, data, silent.
const SCOPE = new URL(self.registration.scope);
const BASE = SCOPE.pathname.replace(/\/$/, "");
// A click that had to open a new window: told to that window once its page
// connects (`prismReady`).
let pendingOpen = null;

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

// Inside the app's mount: same origin, `<base>` or under `<base>/` (never a
// sibling such as `<base>le/`).
const inMount = (href) => {
  try {
    const url = new URL(href, SCOPE);
    return url.origin === SCOPE.origin && (url.pathname === BASE || url.pathname.startsWith(`${BASE}/`));
  } catch {
    return false;
  }
};

// The app's open pages: windows this worker controls, inside the mount.
// Other pages of the origin (another app, a tab outside the mount) are told
// nothing.
const pages = async () => (await self.clients.matchAll({ type: "window" })).filter((c) => inMount(c.url));

// Where a click opens: the push's link under the mount, else the app's
// start (a link to another origin, `//host`, or out of the mount by `..`).
const destination = (link) => {
  if (typeof link === "string" && link.startsWith("/")) {
    try {
      const url = new URL(BASE + link, SCOPE);
      if (inMount(url.href)) return url.href;
    } catch {
      // Not a URL: the start.
    }
  }
  return SCOPE.href;
};

self.addEventListener("push", (event) => {
  let payload = {};
  try {
    payload = event.data ? event.data.json() : {};
  } catch {
    payload = { body: event.data ? event.data.text() : "" };
  }
  event.waitUntil((async () => {
    const open = await pages();
    const visible = open.some((c) => c.visibilityState === "visible");
    for (const c of open) c.postMessage({ prismPush: "received", payload });
    // A visible page shows it itself (the app's on_push). Otherwise browsers
    // require a notification for every push (userVisibleOnly), silent or not.
    if (visible) return;
    const title = payload.title || (payload.silent ? self.registration.scope : "");
    const body = [payload.subtitle, payload.body].filter(Boolean).join("\n");
    await self.registration.showNotification(title || body || " ", {
      body: title ? body : "",
      tag: payload.id || undefined,
      data: payload,
      silent: !!payload.silent,
    });
  })());
});

const tellOpened = (client, pending) => {
  if (pendingOpen !== pending) return;
  pendingOpen = null;
  client.postMessage({ prismPush: "opened", payload: pending.payload, launched: true });
};

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const payload = event.notification.data || {};
  event.waitUntil((async () => {
    const [page] = await pages();
    if (page) {
      await page.focus();
      page.postMessage({ prismPush: "opened", payload });
      return;
    }
    // No page open: open the app on the push's link. Only the window opened
    // here learns about the click, when its page connects; one that connects
    // before `openWindow` returns waits in `early`.
    const pending = { payload, client: undefined, early: [] };
    pendingOpen = pending;
    const opened = await self.clients.openWindow(destination(payload.link));
    if (pendingOpen !== pending) return;
    if (!opened) {
      // The browser did not say which window it opened: nobody is told.
      pendingOpen = null;
      return;
    }
    pending.client = opened.id;
    const early = pending.early.find((c) => c.id === opened.id);
    if (early) tellOpened(early, pending);
  })());
});

self.addEventListener("message", (event) => {
  const pending = pendingOpen;
  const source = event.source;
  if (!(event.data && event.data.prismReady && pending && source && source.type === "window" && inMount(source.url))) return;
  if (pending.client === undefined) pending.early.push(source);
  else if (source.id === pending.client) tellOpened(source, pending);
});
