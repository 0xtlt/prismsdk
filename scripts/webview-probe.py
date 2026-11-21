#!/usr/bin/env python3
"""Test pages for native web views (security review S7, S32, S41), served on
two origins, with every report the pages make logged.

    python3 scripts/webview-probe.py [--main-port 8001] [--frame-port 8002]
                                     [--frame-host localhost] [--log FILE]
    python3 scripts/webview-probe.py --dump DIR   # the main page alone, for file://

Point the counter's web view at the main page (`PRISM_WEB_URL`, read at build
time on Android) and open `/components/media`. On the Android emulator, after
`adb reverse tcp:8001 tcp:8001` and `adb reverse tcp:8002 tcp:8002`, the main
page is `http://127.0.0.1:8001/main.html` and its frame is on another origin,
`http://localhost:8002`. Build with `PRISM_HTTP_HOSTS=127.0.0.1,localhost` so
Android lets the web view use plain HTTP.

The main page (the app's page):
- posts `main:ready` to the app through `prismHost`, and `main:got <data>`
  for each message the app sends it ("Send a message"), reported as
  `who=main got=…`;
- opens two windows by script, with no tap (`window.open` after a timer): a
  web page and an `intent:` URL. Neither may leave the app (S41);
- shows links that open a new window when tapped: https, mailto, tel (may
  open their app) and sms, market, intent (must not: not web, mail or phone);
- embeds a frame of another origin.

The frame (another origin, S7) tries to post as the page: `prismHost` in its
own window, the parent's `prismHost`, a forged port handshake to the top
window, every 400 ms. It reports what it has (`who=frame has-prismHost=…`)
and any message it receives (`who=frame got=…`): the app's messages must
never reach it, and its posts must never reach the app (the counter shows
the last message from the page under the web view).
"""
import argparse
import datetime
import json
import os
import sys
import threading
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MAIN = r"""<!doctype html>
<meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>Probe main</title>
<style>
  body { font: 14px system-ui, sans-serif; margin: 6px; background: #eef3fb; color: #111; }
  a { display: inline-block; margin: 2px 4px 2px 0; padding: 6px 9px; background: #d7def0; color: #003; }
  #big { display: block; padding: 18px 9px; background: #4b4bd8; color: #fff; font-weight: 600; }
  iframe { border: 1px dashed #c33; width: 96%; height: 54px; }
  #log { font: 11px monospace; white-space: pre-wrap; }
</style>
<div>Main frame: <b id="origin"></b></div>
<a id="big" href="https://example.com/?from=tapped-link" target="_blank">Tap: https link in a new window</a>
<a href="mailto:probe@example.com" target="_blank">mailto</a><a href="tel:5550100" target="_blank">tel</a><a href="sms:5550100" target="_blank">sms</a><a href="market://details?id=dev.prism.counter" target="_blank">market</a><a href="intent://probe#Intent;scheme=prismprobe;end" target="_blank">intent</a>
<iframe src="FRAME_URL"></iframe>
<div id="log"></div>
<script>
  document.getElementById("origin").textContent = location.origin;
  var lines = [];
  function show(s) { lines.push(s); document.getElementById("log").textContent = lines.slice(-6).join("\n"); }
  function report(key, value) {
    show(key + "=" + value);
    if (location.protocol.indexOf("http") === 0)
      fetch("/log?who=main&" + key + "=" + encodeURIComponent(value)).catch(function () {});
  }
  function post(value) {
    var json = JSON.stringify(value);
    if (window.prismHost) prismHost.postMessage(json);
    else window.addEventListener("prismhost", function () { prismHost.postMessage(json); }, { once: true });
  }
  // The app's messages: synthetic events, or the port's (no source window).
  window.addEventListener("message", function (e) {
    if (e.source) { report("from-frame-window", JSON.stringify(e.data)); return; }
    report("got", JSON.stringify(e.data));
    post("main:got " + JSON.stringify(e.data));
  });
  report("has-prismHost", typeof window.prismHost);
  post("main:ready");
  // Windows opened by script, no tap.
  setTimeout(function () {
    var w = window.open("https://example.com/?from=script-popup", "_blank");
    report("script-popup", w ? "window" : "null");
    var v = window.open("intent://probe#Intent;scheme=prismprobe;end", "_blank");
    report("script-intent", v ? "window" : "null");
  }, 1500);
</script>
"""

FRAME = r"""<!doctype html>
<meta charset="utf-8">
<style>body { font: 11px monospace; margin: 2px; background: #fde8e8; }</style>
<div id="s">Frame (another origin)</div>
<script>
  function report(key, value) {
    document.getElementById("s").textContent = "Frame " + location.origin + ": " + key + "=" + value;
    fetch("/log?who=frame&" + key + "=" + encodeURIComponent(value)).catch(function () {});
  }
  report("has-prismHost", typeof window.prismHost);
  window.addEventListener("message", function (e) { report("got", JSON.stringify(e.data)); });
  window.addEventListener("prismhost", function () { report("prismhost-event", "fired"); });
  var n = 0;
  setInterval(function () {
    n++;
    if (window.prismHost) { try { prismHost.postMessage(JSON.stringify("frame:evil-own " + n)); report("posted-own", n); } catch (e) { report("own-error", e.name); } }
    try { parent.prismHost.postMessage(JSON.stringify("frame:evil-parent " + n)); report("posted-parent", n); }
    catch (e) { if (n === 1) report("parent-prismHost", e.name); }
    try { top.postMessage("prism-port:guess", "*"); } catch (e) {}
    if (window.chrome && chrome.webview) { chrome.webview.postMessage(JSON.stringify("frame:evil-webview " + n)); if (n === 1) report("chrome-webview", "posted"); }
  }, 400);
</script>
"""

OTHER = r"""<!doctype html>
<meta charset="utf-8"><meta name="viewport" content="width=device-width">
<div>Other page: <b id="o"></b></div>
<script>
  document.getElementById("o").textContent = location.origin;
  window.addEventListener("message", function (e) {
    fetch("/log?who=other&got=" + encodeURIComponent(JSON.stringify(e.data))).catch(function () {});
  });
</script>
"""

LOCK = threading.Lock()
OPTS = argparse.Namespace(log=None)


def log(line):
    stamp = datetime.datetime.now().strftime("%H:%M:%S.%f")[:-3]
    with LOCK:
        print(f"{stamp} {line}", flush=True)
        if OPTS.log:
            with open(OPTS.log, "a", encoding="utf-8") as f:
                f.write(f"{stamp} {line}\n")


def handler(pages):
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            url = urllib.parse.urlsplit(self.path)
            port = self.server.server_address[1]
            if url.path == "/log":
                q = urllib.parse.parse_qsl(url.query, keep_blank_values=True)
                log(f"port={port} " + " ".join(f"{k}={json.dumps(v)}" for k, v in q))
                body, kind = b"ok", "text/plain"
            elif url.path in pages:
                log(f"port={port} GET {url.path} from {self.headers.get('User-Agent', '?')[:60]}")
                body, kind = pages[url.path].encode(), "text/html; charset=utf-8"
            else:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", kind)
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    return Handler


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--main-port", type=int, default=8001)
    p.add_argument("--frame-port", type=int, default=8002)
    p.add_argument("--frame-host", default="localhost")
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--log")
    p.add_argument("--dump", help="write the main page (no frame) to DIR/main.html and exit")
    a = p.parse_args(namespace=OPTS)
    frame_url = f"http://{a.frame_host}:{a.frame_port}/frame.html"
    main_page = MAIN.replace("FRAME_URL", frame_url)
    if a.dump:
        os.makedirs(a.dump, exist_ok=True)
        with open(os.path.join(a.dump, "main.html"), "w", encoding="utf-8") as f:
            f.write(MAIN.replace('<iframe src="FRAME_URL"></iframe>', ""))
        print(os.path.join(a.dump, "main.html"))
        return
    servers = [
        ThreadingHTTPServer((a.host, a.main_port), handler({"/main.html": main_page, "/other.html": OTHER})),
        ThreadingHTTPServer((a.host, a.frame_port), handler({"/frame.html": FRAME, "/other.html": OTHER})),
    ]
    for s in servers:
        threading.Thread(target=s.serve_forever, daemon=True).start()
    log(f"main http://{a.host}:{a.main_port}/main.html, frame {frame_url}")
    try:
        threading.Event().wait()
    except KeyboardInterrupt:
        sys.exit(0)


if __name__ == "__main__":
    main()
