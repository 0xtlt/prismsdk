package dev.prism;

import android.content.Intent;
import android.graphics.Color;
import android.net.Uri;
import android.os.Message;
import android.webkit.JavascriptInterface;
import android.webkit.WebChromeClient;
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
 * Android cannot tell the bridge's frames apart: the main frame's URL decides
 * the origin of a message. Forms are not held back by the sandbox.
 */
final class PrismWeb {
    private PrismWeb() {}

    static final class State {
        String url = "";
        boolean messages, loads, loaded, loadPending, bridged;
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
            }

            @Override
            public void onPageFinished(WebView view, String url) {
                s.loaded = true;
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
        // The bridge: pages loaded from now on see `prismHost`.
        if (s.messages && !s.bridged) {
            w.addJavascriptInterface(new Bridge(n.id, w), "prismHost");
            s.bridged = true;
        } else if (!s.messages && s.bridged) {
            w.removeJavascriptInterface("prismHost");
            s.bridged = false;
        }
    }

    /** `prismHost` in the page. Called on a background thread. */
    static final class Bridge {
        final long id;
        final WebView view;

        Bridge(long id, WebView view) {
            this.id = id;
            this.view = view;
        }

        @JavascriptInterface
        public void postMessage(String json) {
            PrismHost.main.post(() -> received(id, view, json));
        }
    }

    static void received(long id, WebView w, String json) {
        State s = states.get(id);
        if (s == null || !s.messages || json == null) return;
        String origin = origin(w.getUrl());
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
        if (!allows(s, origin(w.getUrl()))) return;
        // `json` is valid JSON (checked by Prism), so a JavaScript literal.
        w.evaluateJavascript("window.dispatchEvent(new MessageEvent(\"message\", { data: " + json + " }));", null);
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
        if (states.remove(n.id) == null) return;
        WebView w = (WebView) n.view;
        w.stopLoading();
        w.destroy();
    }
}
