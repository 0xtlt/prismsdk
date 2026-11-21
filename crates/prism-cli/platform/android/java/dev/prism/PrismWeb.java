package dev.prism;

import android.content.Intent;
import android.graphics.Color;
import android.net.Uri;
import android.os.Message;
import android.webkit.WebChromeClient;
import android.webkit.WebMessage;
import android.webkit.WebMessagePort;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import org.json.JSONException;
import org.json.JSONObject;
import org.json.JSONTokener;

/**
 * Web views (#62): an android.webkit.WebView per `web_view` node. Its text is
 * the URL, its config a WebViewConfig (`messages loads sandbox=…`, then the
 * allowed origins). The page posts with `prismHost.postMessage(JSON.stringify(v))`
 * and gets the app's messages as `message` events on `window`. Only the
 * allowed origins (the config's, else the URL's own) are heard and posted to;
 * a link to another origin opens in the browser.
 *
 * The page's messages come through a message port handed to its main frame
 * only (`postWebMessage` checks the frame's origin when it delivers): an
 * embedded frame of another origin can neither post as the page nor read
 * its messages, which `addJavascriptInterface` (injected in every frame)
 * allowed. `prismHost` is a script (SHIM) the main frame runs when its page
 * starts; it queues messages until the port arrives, after the page loads.
 * A navigation closes the port. Forms are not held back by the sandbox.
 */
final class PrismWeb {
    private PrismWeb() {}

    static final class State {
        String url = "";
        boolean messages, loads, loaded, loadPending;
        /** The app's end of the page's port, and the origin it was given to. */
        WebMessagePort port;
        String portOrigin;
        /** null: not sandboxed. */
        Set<String> sandbox;
        List<String> origins = new ArrayList<>();
        final List<String> queue = new ArrayList<>();
    }

    static final Map<Long, State> states = new HashMap<>();

    static WebView create(PrismHost.Node n) {
        WebView w = new WebView(PrismHost.activity);
        State s = new State();
        states.put(n.id, s);
        WebSettings ws = w.getSettings();
        ws.setJavaScriptEnabled(true);
        ws.setDomStorageEnabled(true);
        // `target="_blank"` and `window.open` reach onCreateWindow.
        ws.setSupportMultipleWindows(true);
        ws.setAllowFileAccess(false);
        ws.setAllowContentAccess(false);
        w.setBackgroundColor(Color.TRANSPARENT);
        final long id = n.id;
        w.setWebViewClient(new WebViewClient() {
            @Override
            public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest r) {
                // The main frame stays on the allowed origins: a link
                // elsewhere opens in the browser.
                if (!r.isForMainFrame() || !r.hasGesture()) return false;
                Uri u = r.getUrl();
                String scheme = u.getScheme() == null ? "" : u.getScheme().toLowerCase(Locale.ROOT);
                if (!scheme.equals("http") && !scheme.equals("https")) return false;
                if (allows(s, origin(u.toString()))) return false;
                openExternally(u);
                return true;
            }

            @Override
            public void onPageStarted(WebView view, String url, android.graphics.Bitmap icon) {
                s.loaded = false;
                // A new page: the old one's port is dead.
                close(s);
                if (s.messages) view.evaluateJavascript(SHIM, null);
            }

            @Override
            public void onPageCommitVisible(WebView view, String url) {
                if (s.messages) view.evaluateJavascript(SHIM, null);
            }

            @Override
            public void onPageFinished(WebView view, String url) {
                s.loaded = true;
                if (s.messages) connect(id, view, s);
                List<String> pending = new ArrayList<>(s.queue);
                s.queue.clear();
                for (String json : pending) deliver(view, s, json);
                if (s.loads) {
                    JSONObject e = PrismHost.event("webload", id);
                    try {
                        e.put("url", url == null ? s.url : url);
                    } catch (JSONException ignored) {
                    }
                    PrismHost.send(e);
                }
            }
        });
        w.setWebChromeClient(new WebChromeClient() {
            /** New windows: their first URL opens in the browser, if allowed. */
            @Override
            public boolean onCreateWindow(WebView view, boolean dialog, boolean gesture, Message result) {
                if (s.sandbox != null && !s.sandbox.contains("popups")) return false;
                WebView catcher = new WebView(view.getContext());
                catcher.setWebViewClient(new WebViewClient() {
                    @Override
                    public boolean shouldOverrideUrlLoading(WebView v, WebResourceRequest r) {
                        openExternally(r.getUrl());
                        v.destroy();
                        return true;
                    }
                });
                ((WebView.WebViewTransport) result.obj).setWebView(catcher);
                result.sendToTarget();
                return true;
            }
        });
        return w;
    }

    /** `scheme://host[:port]`, lower-case, without the scheme's default port. */
    static String origin(String url) {
        if (url == null) return null;
        Uri u = Uri.parse(url);
        String scheme = u.getScheme(), host = u.getHost();
        if (scheme == null || host == null || host.isEmpty()) return null;
        scheme = scheme.toLowerCase(Locale.ROOT);
        host = host.toLowerCase(Locale.ROOT);
        if (host.contains(":")) host = "[" + host + "]";
        int port = u.getPort();
        boolean usual = port == -1 || (scheme.equals("http") && port == 80) || (scheme.equals("https") && port == 443);
        return usual ? scheme + "://" + host : scheme + "://" + host + ":" + port;
    }

    static boolean allows(State s, String origin) {
        List<String> list = s.origins;
        if (list.isEmpty()) {
            String own = origin(s.url);
            list = new ArrayList<>();
            if (own != null) list.add(own);
        }
        if (list.contains("*")) return true;
        return origin != null && list.contains(origin.toLowerCase(Locale.ROOT));
    }

    static void openExternally(Uri u) {
        try {
            Intent i = new Intent(Intent.ACTION_VIEW, u);
            i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            PrismHost.activity.startActivity(i);
        } catch (Exception ignored) {
        }
    }

    /** The URL (`text`): loaded on the next turn, with the config of the same update. */
    static void load(PrismHost.Node n, String url) {
        State s = states.get(n.id);
        if (s == null) return;
        s.url = url;
        s.loaded = false;
        scheduleLoad(n, s);
    }

    private static void scheduleLoad(PrismHost.Node n, State s) {
        if (s.loadPending) return;
        s.loadPending = true;
        PrismHost.main.post(() -> {
            s.loadPending = false;
            if (states.get(n.id) != s || s.url.isEmpty()) return;
            ((WebView) n.view).loadUrl(s.url);
        });
    }

    static void config(PrismHost.Node n, String config) {
        State s = states.get(n.id);
        if (s == null) return;
        WebView w = (WebView) n.view;
        String[] lines = config.split("\n", -1);
        Set<String> tokens = new HashSet<>();
        String sandbox = null;
        for (String t : lines[0].split(" ")) {
            if (t.startsWith("sandbox=")) sandbox = t.substring(8);
            else if (!t.isEmpty()) tokens.add(t);
        }
        s.messages = tokens.contains("messages");
        s.loads = tokens.contains("loads");
        if (sandbox == null) {
            s.sandbox = null;
        } else {
            s.sandbox = new HashSet<>();
            for (String a : sandbox.split(",")) if (!a.isEmpty()) s.sandbox.add(a);
        }
        s.origins = new ArrayList<>();
        for (int i = 1; i < lines.length; i++) {
            String o = lines[i].trim().toLowerCase(Locale.ROOT);
            if (!o.isEmpty()) s.origins.add(o);
        }
        WebSettings ws = w.getSettings();
        ws.setJavaScriptEnabled(s.sandbox == null || s.sandbox.contains("scripts"));
        ws.setDomStorageEnabled(s.sandbox == null || s.sandbox.contains("same-origin"));
        // Without messages, no port; turned on, the loaded page gets one.
        if (!s.messages) close(s);
        else if (s.loaded && s.port == null) connect(n.id, w, s);
    }

    /**
     * `prismHost` in the main frame: messages queue (at most 256) until the
     * port comes, with `source === null` (posted by the app, not a frame)
     * and the nonce the app just gave. Installed once per page.
     */
    static final String SHIM = """
        (function () {
          if (window.prismHost && window.prismHost.__prism) return;
          var port = null, queue = [], expected = null;
          function deliver(e) {
            var data;
            try { data = JSON.parse(e.data); } catch (_) { return; }
            window.dispatchEvent(new MessageEvent("message", { data: data }));
          }
          window.addEventListener("message", function (e) {
            if (e.source !== null || expected === null || e.data !== "prism-port:" + expected
                || !e.ports || e.ports.length !== 1) return;
            e.stopImmediatePropagation();
            expected = null;
            if (port) port.close();
            port = e.ports[0];
            port.onmessage = deliver;
            queue.splice(0).forEach(function (m) { port.postMessage(m); });
          }, true);
          Object.defineProperty(window, "prismHost", {
            value: Object.freeze({
              __prism: true,
              postMessage: function (json) {
                var m = String(json);
                if (port) port.postMessage(m);
                else if (queue.length < 256) queue.push(m);
              }
            })
          });
          Object.defineProperty(window, "__prismExpect", { value: function (n) { expected = String(n); } });
          window.dispatchEvent(new Event("prismhost"));
        })();
        """;

    /** Hands the page's main frame a new port, if it shows an allowed origin. */
    static void connect(long id, WebView w, State s) {
        String origin = origin(w.getUrl());
        close(s);
        if (origin == null || !allows(s, origin)) return;
        WebMessagePort[] ports = w.createWebMessageChannel();
        WebMessagePort mine = ports[0];
        s.port = mine;
        s.portOrigin = origin;
        mine.setWebMessageCallback(new WebMessagePort.WebMessageCallback() {
            @Override
            public void onMessage(WebMessagePort p, WebMessage m) {
                if (states.get(id) == s && s.port == p) received(id, s, m.getData());
            }
        }, PrismHost.main);
        byte[] bytes = new byte[18];
        new java.security.SecureRandom().nextBytes(bytes);
        String nonce = android.util.Base64.encodeToString(bytes, android.util.Base64.NO_WRAP | android.util.Base64.URL_SAFE);
        // The shim (again, if the page started before it was asked), the
        // nonce, then the port: delivered to the main frame only, and only
        // while it shows `origin`.
        w.evaluateJavascript(SHIM + "window.__prismExpect(\"" + nonce + "\");", v -> {
            if (s.port != mine) {
                ports[1].close();
                return;
            }
            w.postWebMessage(new WebMessage("prism-port:" + nonce, new WebMessagePort[] {ports[1]}), Uri.parse(origin));
        });
    }

    static void close(State s) {
        if (s.port != null) s.port.close();
        s.port = null;
        s.portOrigin = null;
    }

    static void received(long id, State s, String json) {
        if (!s.messages || json == null || s.portOrigin == null) return;
        String origin = s.portOrigin;
        if (!allows(s, origin)) return;
        try {
            new JSONTokener(json).nextValue();
        } catch (JSONException e) {
            return; // not JSON
        }
        JSONObject e = PrismHost.event("webmsg", id);
        try {
            e.put("data", json);
            e.put("origin", origin);
        } catch (JSONException ignored) {
        }
        PrismHost.send(e);
    }

    static void post(PrismHost.Node n, String json) {
        State s = states.get(n.id);
        if (s == null) return;
        if (!s.loaded) {
            s.queue.add(json);
            return;
        }
        deliver((WebView) n.view, s, json);
    }

    private static void deliver(WebView w, State s, String json) {
        // Through the port when the page has one: it stays with the page
        // (and origin) it was given to.
        if (s.port != null) {
            if (allows(s, s.portOrigin)) s.port.postMessage(new WebMessage(json));
            return;
        }
        String origin = origin(w.getUrl());
        if (!allows(s, origin)) return;
        // `json` is valid JSON (checked by Prism), so a JavaScript literal.
        w.evaluateJavascript(messageScript(json, origin), null);
    }

    /**
     * The script handing the page the app's message as a `message` event, only if the page
     * still shows `origin` when it runs (it runs later: the view may have moved to another
     * document). null: no origin to hold to (every origin allowed). The same script as Rust's
     * `prism_core::web_view::message_script`.
     */
    static String messageScript(String json, String origin) {
        String dispatch = "window.dispatchEvent(new MessageEvent(\"message\", { data: " + json + " }));";
        if (origin == null) return dispatch;
        return "if (location.protocol + \"//\" + location.host === " + JSONObject.quote(origin) + ") " + dispatch;
    }

    static void reload(PrismHost.Node n) {
        State s = states.get(n.id);
        if (s == null) return;
        s.loaded = false;
        WebView w = (WebView) n.view;
        if (w.getUrl() == null) scheduleLoad(n, s);
        else w.reload();
    }

    static void destroy(PrismHost.Node n) {
        State s = states.remove(n.id);
        if (s == null) return;
        close(s);
        WebView w = (WebView) n.view;
        w.stopLoading();
        w.destroy();
    }
}
