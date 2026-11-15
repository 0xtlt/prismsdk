// Prism's service worker (served at <base>/prism-sw.js, scope <base>/): shows
// Web Push messages as notifications, tells open pages about them, and
// opens or focuses the app when one is clicked. Payloads are Prism's JSON
// (`prism-push`): title, subtitle, body, link, id, data, silent.
const BASE = new URL(self.registration.scope).pathname.replace(/\/$/, "");
// A click that had to open a new window: told to the page once it connects.
let pendingOpen = null;

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

const windows = () => self.clients.matchAll({ type: "window", includeUncontrolled: true });

self.addEventListener("push", (event) => {
  let payload = {};
  try {
    payload = event.data ? event.data.json() : {};
  } catch {
    payload = { body: event.data ? event.data.text() : "" };
  }
  event.waitUntil((async () => {
    const pages = await windows();
    const visible = pages.some((c) => c.visibilityState === "visible");
    for (const c of pages) c.postMessage({ prismPush: "received", payload });
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

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const payload = event.notification.data || {};
  event.waitUntil((async () => {
    const pages = await windows();
    const page = pages.find((c) => new URL(c.url).pathname.startsWith(BASE || "/"));
    if (page) {
      await page.focus();
      page.postMessage({ prismPush: "opened", payload });
      return;
    }
    // No page open: open the app on the push's link; it learns about the
    // click when it connects (`prismReady`).
    pendingOpen = payload;
    const link = typeof payload.link === "string" && payload.link.startsWith("/") ? payload.link : "/";
    await self.clients.openWindow(BASE + link);
  })());
});

self.addEventListener("message", (event) => {
  if (event.data && event.data.prismReady && pendingOpen && event.source) {
    event.source.postMessage({ prismPush: "opened", payload: pendingOpen, launched: true });
    pendingOpen = null;
  }
});
