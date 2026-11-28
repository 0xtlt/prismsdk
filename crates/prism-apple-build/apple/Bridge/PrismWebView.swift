// Web views (#62): a WKWebView in a host view, for the macOS and UIKit
// backends (iOS, iPadOS, visionOS). tvOS and watchOS have no WebKit: there
// `prism_web_view_new` returns null and the backends draw an empty box.
//
// The page talks to the app with `prismHost.postMessage(JSON.stringify(v))`
// (a script injected at document start, in the main frame) and receives the
// app's messages as `message` events on `window`. Only the allowed origins
// (the config's, else the URL's own) are heard and posted to; a link to
// another origin opens in the system browser.
//
// App schemes (#343): the schemes the app serves from Rust
// (`Application::web_scheme`) get a `WKURLSchemeHandler` on each web view's
// configuration; Rust answers each request on a background thread.
import Foundation

/// Implemented in Rust (prism-macos, prism-ios): kind 0 is a message (`a`:
/// JSON, `b`: the page's origin), kind 1 a finished load (`a`: the URL).
@_silgen_name("prism_web_view_event")
func prismRustWebViewEvent(_ id: UInt64, _ kind: Int32, _ a: UnsafePointer<CChar>, _ b: UnsafePointer<CChar>)

#if canImport(WebKit) && !os(tvOS) && !os(watchOS)
import WebKit
#if os(macOS)
import AppKit
typealias PrismWebBase = NSView
#else
import UIKit
typealias PrismWebBase = UIView
#endif

/// `scheme://host[:port]`, lower-case, without the scheme's default port.
func prismOrigin(_ url: URL?) -> String? {
    guard let url = url, let scheme = url.scheme?.lowercased(), let host = url.host?.lowercased(), !host.isEmpty else { return nil }
    let shown = host.contains(":") ? "[\(host)]" : host
    if let port = url.port, !((scheme == "http" && port == 80) || (scheme == "https" && port == 443)) {
        return "\(scheme)://\(shown):\(port)"
    }
    return "\(scheme)://\(shown)"
}

/// The script handing the page the app's message `json` (valid JSON, so a
/// JavaScript literal) as a `message` event, only if the page still shows
/// `origin` when it runs (it runs later: the view may have moved to another
/// document). `nil`: no origin to hold to (every origin allowed). The same
/// script as Rust's `prism_core::web_view::message_script`.
func prismMessageScript(_ json: String, _ origin: String?) -> String {
    let dispatch = "window.dispatchEvent(new MessageEvent(\"message\", { data: \(json) }));"
    guard let origin = origin else { return dispatch }
    guard let data = try? JSONSerialization.data(withJSONObject: origin, options: [.fragmentsAllowed]),
          let literal = String(data: data, encoding: .utf8) else { return "" }
    return "if (location.protocol + \"//\" + location.host === \(literal)) \(dispatch)"
}

/// Implemented in Rust (prism-native, `apple::web`): the app's schemes,
/// one per line, freed with `prism_web_string_free`.
@_silgen_name("prism_web_schemes")
func prismRustWebSchemes() -> UnsafeMutablePointer<CChar>?
@_silgen_name("prism_web_string_free")
func prismRustWebStringFree(_ p: UnsafeMutablePointer<CChar>?)

/// The answer to request `task`: the status then one `Name: value` header
/// per line, and the body. Called on a background thread; the pointers are
/// valid during the call only.
typealias PrismSchemeDone = @convention(c) (UInt64, UnsafePointer<CChar>?, UnsafePointer<UInt8>?, Int) -> Void

/// Implemented in Rust: runs the app's handler for a request on a
/// background thread, then calls `done` there.
@_silgen_name("prism_web_scheme_request")
func prismRustWebSchemeRequest(_ task: UInt64, _ method: UnsafePointer<CChar>, _ url: UnsafePointer<CChar>,
                               _ headers: UnsafePointer<CChar>, _ body: UnsafePointer<UInt8>?, _ len: Int,
                               _ done: PrismSchemeDone)

/// The app's schemes (#343): one handler for all of them, on every web
/// view's configuration. WebKit starts and stops tasks on the main thread;
/// the answers come back there too, and a task WebKit stopped is never
/// touched again (it would raise an exception).
final class PrismSchemeHandler: NSObject, WKURLSchemeHandler {
    static let shared = PrismSchemeHandler()

    /// The schemes the app serves, read once: they are registered before
    /// the first web view.
    static let schemes: [String] = {
        guard let p = prismRustWebSchemes() else { return [] }
        defer { prismRustWebStringFree(p) }
        return String(cString: p).split(separator: "\n").map(String.init)
    }()

    /// Running tasks by id, and their ids. Main thread only.
    private var tasks: [UInt64: WKURLSchemeTask] = [:]
    private var ids: [ObjectIdentifier: UInt64] = [:]
    private var next: UInt64 = 0

    /// Serves the app's schemes in web views made with `conf` (none: as
    /// before).
    static func install(_ conf: WKWebViewConfiguration) {
        for scheme in schemes where !WKWebView.handlesURLScheme(scheme) {
            conf.setURLSchemeHandler(shared, forURLScheme: scheme)
        }
    }

    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
        next += 1
        let id = next
        tasks[id] = task
        ids[ObjectIdentifier(task)] = id
        let request = task.request
        let method = request.httpMethod ?? "GET"
        let url = request.url?.absoluteString ?? ""
        var headers = ""
        for (name, value) in request.allHTTPHeaderFields ?? [:] {
            let clean = value.replacingOccurrences(of: "\r", with: " ").replacingOccurrences(of: "\n", with: " ")
            headers += "\(name): \(clean)\n"
        }
        let body = request.httpBody ?? PrismSchemeHandler.read(request.httpBodyStream)
        body.withUnsafeBytes { raw in
            let bytes = raw.bindMemory(to: UInt8.self)
            method.withCString { m in
                url.withCString { u in
                    headers.withCString { h in
                        prismRustWebSchemeRequest(id, m, u, h, bytes.baseAddress, bytes.count) { id, head, body, len in
                            let text = head.map { String(cString: $0) } ?? "500"
                            let data = body.map { Data(bytes: $0, count: len) } ?? Data()
                            DispatchQueue.main.async { PrismSchemeHandler.shared.finish(id, text, data) }
                        }
                    }
                }
            }
        }
    }

    func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {
        if let id = ids.removeValue(forKey: ObjectIdentifier(task)) {
            tasks[id] = nil
        }
    }

    /// The handler's answer: sent to the task if WebKit still waits for it.
    private func finish(_ id: UInt64, _ head: String, _ body: Data) {
        guard let task = tasks.removeValue(forKey: id) else { return }
        ids[ObjectIdentifier(task)] = nil
        guard let url = task.request.url else { return }
        var lines = head.components(separatedBy: "\n")
        let status = Int(lines.removeFirst()) ?? 500
        var fields: [String: String] = [:]
        for line in lines {
            guard let colon = line.firstIndex(of: ":") else { continue }
            let name = String(line[..<colon])
            let value = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
            fields[name] = fields[name].map { "\($0), \(value)" } ?? value
        }
        fields["Content-Length"] = String(body.count)
        guard let response = HTTPURLResponse(url: url, statusCode: status, httpVersion: "HTTP/1.1",
                                             headerFields: fields) else { return }
        task.didReceive(response)
        if !body.isEmpty { task.didReceive(body) }
        task.didFinish()
    }

    /// A request body WebKit gave as a stream.
    private static func read(_ stream: InputStream?) -> Data {
        guard let stream = stream else { return Data() }
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 64 * 1024)
        while true {
            let n = stream.read(&buffer, maxLength: buffer.count)
            if n <= 0 { break }
            data.append(buffer, count: n)
        }
        return data
    }
}

/// The script message handler, held weakly (WebKit retains its handlers).
private final class PrismWeakHandler: NSObject, WKScriptMessageHandler {
    weak var host: PrismWebHost?
    init(_ host: PrismWebHost) { self.host = host }
    func userContentController(_ c: WKUserContentController, didReceive message: WKScriptMessage) {
        host?.received(message)
    }
}

private let prismBridgeScript = """
(function () {
  var h = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.prismHost;
  if (!h || window.prismHost) return;
  Object.defineProperty(window, "prismHost", {
    value: Object.freeze({ postMessage: function (json) { h.postMessage(String(json)); } }),
  });
})();
"""

final class PrismWebHost: PrismWebBase, WKNavigationDelegate, WKUIDelegate {
    let nodeId: UInt64
    private var webView: WKWebView?
    private var urlString = ""
    private var messages = false
    private var loads = false
    /// `nil`: not sandboxed.
    private var sandbox: Set<String>?
    private var origins: [String] = []
    private var loaded = false
    private var queue: [String] = []
    private var loadPending = false

    init(id: UInt64) {
        nodeId = id
        super.init(frame: .zero)
        #if os(macOS)
        autoresizingMask = [.width, .height]
        #else
        autoresizingMask = [.flexibleWidth, .flexibleHeight]
        backgroundColor = .clear
        #endif
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    private var scripts: Bool { sandbox?.contains("scripts") ?? true }
    private var ephemeral: Bool { !(sandbox?.contains("same-origin") ?? true) }

    private var allowed: [String] {
        if !origins.isEmpty { return origins }
        return prismOrigin(URL(string: urlString)).map { [$0] } ?? []
    }

    private func allows(_ origin: String?) -> Bool {
        let list = allowed
        if list.contains("*") { return true }
        guard let origin = origin?.lowercased() else { return false }
        return list.contains(origin)
    }

    // MARK: From the app

    func load(_ url: String) {
        urlString = url
        loaded = false
        scheduleLoad()
    }

    /// The config comes after the URL in the same update: load on the next
    /// turn, with it.
    private func scheduleLoad() {
        guard !loadPending else { return }
        loadPending = true
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.loadPending = false
            self.performLoad()
        }
    }

    private func performLoad() {
        let view = webView ?? makeWebView()
        guard let url = URL(string: urlString), url.scheme != nil else { return }
        if url.isFileURL {
            view.loadFileURL(url, allowingReadAccessTo: url.deletingLastPathComponent())
        } else {
            view.load(URLRequest(url: url))
        }
    }

    func configure(_ config: String) {
        let lines = config.components(separatedBy: "\n")
        let tokens = (lines.first ?? "").split(separator: " ").map(String.init)
        let wasEphemeral = ephemeral
        let hadMessages = messages
        messages = tokens.contains("messages")
        loads = tokens.contains("loads")
        if let s = tokens.first(where: { $0.hasPrefix("sandbox=") }) {
            sandbox = Set(s.dropFirst("sandbox=".count).split(separator: ",").map(String.init))
        } else {
            sandbox = nil
        }
        origins = lines.dropFirst().map { $0.trimmingCharacters(in: .whitespaces).lowercased() }.filter { !$0.isEmpty }
        guard let view = webView else { return }
        if wasEphemeral != ephemeral {
            // Another data store: a new web view.
            teardown()
            if !urlString.isEmpty { scheduleLoad() }
        } else if hadMessages != messages {
            installBridge(view.configuration.userContentController)
        }
    }

    func post(_ json: String) {
        guard loaded, let view = webView else {
            queue.append(json)
            return
        }
        deliver(json, view)
    }

    func reload() {
        loaded = false
        if let view = webView, view.url != nil {
            view.reload()
        } else {
            scheduleLoad()
        }
    }

    func teardown() {
        guard let view = webView else { return }
        view.stopLoading()
        view.navigationDelegate = nil
        view.uiDelegate = nil
        view.configuration.userContentController.removeScriptMessageHandler(forName: "prismHost")
        view.removeFromSuperview()
        webView = nil
        loaded = false
    }

    // MARK: The web view

    private func makeWebView() -> WKWebView {
        let conf = WKWebViewConfiguration()
        if ephemeral { conf.websiteDataStore = .nonPersistent() }
        PrismSchemeHandler.install(conf)
        installBridge(conf.userContentController)
        let view = WKWebView(frame: bounds, configuration: conf)
        view.navigationDelegate = self
        view.uiDelegate = self
        #if os(macOS)
        view.autoresizingMask = [.width, .height]
        #else
        view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.isOpaque = false
        view.backgroundColor = .clear
        #endif
        addSubview(view)
        webView = view
        return view
    }

    private func installBridge(_ ucc: WKUserContentController) {
        ucc.removeScriptMessageHandler(forName: "prismHost")
        ucc.removeAllUserScripts()
        guard messages else { return }
        ucc.add(PrismWeakHandler(self), name: "prismHost")
        ucc.addUserScript(WKUserScript(source: prismBridgeScript, injectionTime: .atDocumentStart, forMainFrameOnly: true))
    }

    private func deliver(_ json: String, _ view: WKWebView) {
        let origin = prismOrigin(view.url)
        guard allows(origin) else { return }
        // `json` is valid JSON (checked by Prism), so a JavaScript literal.
        view.evaluateJavaScript(prismMessageScript(json, origin), completionHandler: nil)
    }

    fileprivate func received(_ message: WKScriptMessage) {
        // `webkit.messageHandlers` is visible to every frame: only the main
        // frame's page speaks for the web view.
        guard messages, message.frameInfo.isMainFrame else { return }
        let o = message.frameInfo.securityOrigin
        var origin = "\(o.protocol)://\(o.host)"
        if o.port != 0 && !((o.protocol == "http" && o.port == 80) || (o.protocol == "https" && o.port == 443)) {
            origin += ":\(o.port)"
        }
        guard allows(origin) else { return }
        let json: String
        if let s = message.body as? String {
            guard let data = s.data(using: .utf8),
                  (try? JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed])) != nil else { return }
            json = s
        } else if let data = try? JSONSerialization.data(withJSONObject: message.body, options: [.fragmentsAllowed]),
                  let s = String(data: data, encoding: .utf8) {
            json = s
        } else {
            return
        }
        json.withCString { a in origin.withCString { b in prismRustWebViewEvent(nodeId, 0, a, b) } }
    }

    private func openExternally(_ url: URL) {
        guard let scheme = url.scheme?.lowercased(), ["http", "https", "mailto", "tel"].contains(scheme) else { return }
        #if os(macOS)
        NSWorkspace.shared.open(url)
        #else
        UIApplication.shared.open(url)
        #endif
    }

    // MARK: WKNavigationDelegate

    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction, preferences: WKWebpagePreferences,
                 decisionHandler: @escaping (WKNavigationActionPolicy, WKWebpagePreferences) -> Void) {
        preferences.allowsContentJavaScript = scripts
        let user = action.navigationType == .linkActivated || action.navigationType == .formSubmitted
            || action.navigationType == .formResubmitted
        if let s = sandbox, !s.contains("forms"),
           action.navigationType == .formSubmitted || action.navigationType == .formResubmitted {
            decisionHandler(.cancel, preferences)
            return
        }
        // The main frame stays on the allowed origins: a link elsewhere opens
        // in the system browser.
        if user, action.targetFrame?.isMainFrame ?? false, let url = action.request.url,
           let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https",
           !allows(prismOrigin(url)) {
            openExternally(url)
            decisionHandler(.cancel, preferences)
            return
        }
        decisionHandler(.allow, preferences)
    }

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        loaded = false
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        loaded = true
        let pending = queue
        queue.removeAll()
        for json in pending { deliver(json, webView) }
        if loads {
            let url = webView.url?.absoluteString ?? urlString
            url.withCString { a in "".withCString { b in prismRustWebViewEvent(nodeId, 1, a, b) } }
        }
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        reload()
    }

    // MARK: WKUIDelegate

    /// `target="_blank"` and `window.open`: the system browser, if allowed.
    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                 for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        if sandbox?.contains("popups") ?? true, let url = action.request.url {
            openExternally(url)
        }
        return nil
    }
}

private func prismHost(_ p: UnsafeMutableRawPointer) -> PrismWebHost {
    Unmanaged<PrismWebHost>.fromOpaque(p).takeUnretainedValue()
}

private func prismStr(_ p: UnsafePointer<CChar>) -> String { String(cString: p) }

/// A new web view's host view for node `id`, retained (+1): the caller
/// releases it (`prism_web_view_release`).
@_cdecl("prism_web_view_new")
public func prismWebViewNew(_ id: UInt64) -> UnsafeMutableRawPointer? {
    Unmanaged.passRetained(PrismWebHost(id: id)).toOpaque()
}

@_cdecl("prism_web_view_load")
public func prismWebViewLoad(_ p: UnsafeMutableRawPointer, _ url: UnsafePointer<CChar>) {
    prismHost(p).load(prismStr(url))
}

@_cdecl("prism_web_view_config")
public func prismWebViewConfig(_ p: UnsafeMutableRawPointer, _ config: UnsafePointer<CChar>) {
    prismHost(p).configure(prismStr(config))
}

@_cdecl("prism_web_view_post")
public func prismWebViewPost(_ p: UnsafeMutableRawPointer, _ json: UnsafePointer<CChar>) {
    prismHost(p).post(prismStr(json))
}

@_cdecl("prism_web_view_reload")
public func prismWebViewReload(_ p: UnsafeMutableRawPointer) {
    prismHost(p).reload()
}

/// Stops the page and releases the host view (the +1 of `prism_web_view_new`).
@_cdecl("prism_web_view_release")
public func prismWebViewRelease(_ p: UnsafeMutableRawPointer) {
    let host = Unmanaged<PrismWebHost>.fromOpaque(p)
    host.takeUnretainedValue().teardown()
    host.release()
}

#elseif !os(watchOS)
// tvOS: no WebKit. The backend shows an empty box (`web_view: false`).
@_cdecl("prism_web_view_new")
public func prismWebViewNew(_ id: UInt64) -> UnsafeMutableRawPointer? { nil }
@_cdecl("prism_web_view_load")
public func prismWebViewLoad(_ p: UnsafeMutableRawPointer, _ url: UnsafePointer<CChar>) {}
@_cdecl("prism_web_view_config")
public func prismWebViewConfig(_ p: UnsafeMutableRawPointer, _ config: UnsafePointer<CChar>) {}
@_cdecl("prism_web_view_post")
public func prismWebViewPost(_ p: UnsafeMutableRawPointer, _ json: UnsafePointer<CChar>) {}
@_cdecl("prism_web_view_reload")
public func prismWebViewReload(_ p: UnsafeMutableRawPointer) {}
@_cdecl("prism_web_view_release")
public func prismWebViewRelease(_ p: UnsafeMutableRawPointer) {}
#endif
