//! S7 (security review): on Android, a web view's page talks to the app
//! through a message port handed to its main frame only, for an allowed
//! origin. `addJavascriptInterface` reached every frame (an embedded ad
//! could post as the page). The port handshake runs in the page as a
//! script (`SHIM` in `PrismWeb.java`): exercised here with Node, against
//! forged handshakes from frames. Delivery to the main frame only, and only
//! at the allowed origin, is `WebView.postWebMessage`'s (device-only).

use std::path::Path;
use std::process::Command;

fn java() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("platform/android/java/dev/prism/PrismWeb.java"),
    )
    .unwrap()
}

/// The script the bridge runs in the page.
fn shim() -> String {
    let java = java();
    let start = java.find("SHIM = \"\"\"").expect("a SHIM text block") + "SHIM = \"\"\"".len();
    let end = start + java[start..].find("\"\"\";").unwrap();
    java[start..end].to_string()
}

#[test]
fn no_frame_gets_a_javascript_interface() {
    let java = java();
    assert!(
        !java.contains("addJavascriptInterface("),
        "every frame would see it"
    );
    assert!(java.contains("postWebMessage("));
    assert!(java.contains("createWebMessageChannel()"));
}

/// A window with `message` listeners, `MessageChannel`-like ports and the
/// events a page can see; then the shim, then `scenario`.
const HARNESS: &str = r#"
const listeners = { message: [], prismhost: [] };
const received = [];
class Port {
  constructor() { this.sent = []; this.onmessage = null; this.closed = false; }
  postMessage(m) { this.sent.push(m); }
  close() { this.closed = true; }
}
const window = globalThis;
window.addEventListener = (type, f, capture) => { (listeners[type] ||= []).push(f); };
window.dispatchEvent = (e) => {
  let stopped = false;
  e.stopImmediatePropagation = () => { stopped = true; };
  for (const f of listeners[e.type] || []) { if (stopped) break; f(e); }
  return true;
};
globalThis.MessageEvent = class { constructor(type, init) { this.type = type; Object.assign(this, init); } };
globalThis.Event = class { constructor(type) { this.type = type; } };
const post = (data, source, ports) =>
  window.dispatchEvent(Object.assign(new MessageEvent("message", { data, ports: ports || [] }), { source }));
listeners.message.push((e) => { if (e.source === undefined) received.push(e.data); });
const check = (ok, what) => { if (!ok) { console.error("FAILED: " + what); process.exit(1); } };
"#;

fn run(scenario: &str) {
    let Ok(out) = Command::new("node")
        .arg("-e")
        .arg(format!("{HARNESS}\n{}\n{scenario}", shim()))
        .output()
    else {
        eprintln!("node not found: skipped");
        return;
    };
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn only_the_native_handshake_connects_the_page() {
    run(r#"
check(typeof prismHost.postMessage === "function", "prismHost before the port");
prismHost.postMessage('{"early":1}');
// A frame posts a port of its own, with and without the nonce.
const evil = new Port();
post("prism-port:n0nce", { frame: true }, [evil]);
__prismExpect("n0nce");
post("prism-port:n0nce", { frame: true }, [evil]);
post("prism-port:guess", null, [evil]);
prismHost.postMessage('{"secret":1}');
check(evil.sent.length === 0, "a frame's port got messages: " + evil.sent);
// The native handshake: source null, the nonce the app set.
const port = new Port();
post("prism-port:n0nce", null, [port]);
check(JSON.stringify(port.sent) === JSON.stringify(['{"early":1}', '{"secret":1}']), "queue flushed: " + port.sent);
prismHost.postMessage('{"later":2}');
check(port.sent.length === 3, "sent through the port");
// The app's messages arrive as parsed `message` events.
port.onmessage({ data: '{"hello":"page"}' });
check(received.length === 1 && received[0].hello === "page", "delivered: " + JSON.stringify(received));
// A second handshake with the used nonce is refused.
const again = new Port();
post("prism-port:n0nce", null, [again]);
prismHost.postMessage("x");
check(again.sent.length === 0 && port.sent.length === 4, "nonce reused");
// The page cannot swap the bridge.
try { window.prismHost = { postMessage() {} }; } catch (_) {}
check(typeof prismHost.__prism === "boolean", "prismHost replaced");
"#);
}

#[test]
fn the_shim_installs_once_and_bounds_its_queue() {
    run(r#"
const first = prismHost;
eval(SHIM_AGAIN);
check(prismHost === first, "installed twice");
for (let i = 0; i < 10000; i++) prismHost.postMessage("m" + i);
__prismExpect("k");
const port = new Port();
post("prism-port:k", null, [port]);
check(port.sent.length <= 256, "queue unbounded: " + port.sent.length);
"#
    .replace("SHIM_AGAIN", &format!("{:?}", shim()))
    .as_str());
}
