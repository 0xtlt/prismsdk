// Remote push notifications (APNs) for the Rust app: registration, the
// device token, pushes received in the foreground, tapped, or silent
// (content-available), and Live Activity push tokens.
//
// iOS, tvOS and visionOS: the app delegate is Rust's (prism-ios), which
// forwards the token and silent pushes; payloads are read here
// (`prism_push_payload`). macOS and watchOS: the delegates below.
import Foundation
import UserNotifications
#if os(watchOS)
import WatchKit
#elseif canImport(UIKit)
import UIKit
#else
import AppKit
#endif
#if canImport(ActivityKit)
import ActivityKit
#endif

/// Implemented in Rust: the device token (hex), or why registration failed.
@_silgen_name("prism_push_token")
func prismRustPushToken(_ token: UnsafePointer<CChar>?, _ error: UnsafePointer<CChar>?)

/// Implemented in Rust: a push arrived. `kind`: 0 received in the
/// foreground, 1 tapped, 2 silent (call `prism_push_complete(completion)`
/// when done; 0 = nothing to call).
@_silgen_name("prism_push_received")
func prismRustPushReceived(_ kind: Int32, _ json: UnsafePointer<CChar>, _ action: UnsafePointer<CChar>?,
                           _ text: UnsafePointer<CChar>?, _ completion: UInt64)

/// Implemented in Rust: a Live Activity push token (`key` nil: push-to-start).
@_silgen_name("prism_activity_push_token")
func prismRustActivityPushToken(_ key: UnsafePointer<CChar>?, _ kind: UnsafePointer<CChar>, _ token: UnsafePointer<CChar>)

func prismHex(_ data: Data) -> String { data.map { String(format: "%02x", $0) }.joined() }

/// A push's userInfo as Prism's payload JSON: `title`, `subtitle`, `body`
/// (from `aps.alert`), `link`, `id`, and the other top-level keys in `data`.
func prismPushJSON(_ info: [AnyHashable: Any], id: String?) -> String {
    var out: [String: Any] = [:]
    var data: [String: Any] = [:]
    for (key, value) in info {
        guard let key = key as? String, key != "aps" else { continue }
        if key == "link", let link = value as? String { out["link"] = link } else { data[key] = value }
    }
    if let aps = info["aps"] as? [String: Any] {
        if let alert = aps["alert"] as? [String: Any] {
            for k in ["title", "subtitle", "body"] { if let v = alert[k] as? String { out[k] = v } }
        } else if let alert = aps["alert"] as? String {
            out["body"] = alert
        }
        if (aps["content-available"] as? NSNumber)?.intValue == 1, out["title"] == nil, out["body"] == nil {
            out["silent"] = true
        }
    }
    if let id { out["id"] = id }
    if JSONSerialization.isValidJSONObject(data) { out["data"] = data }
    guard let json = try? JSONSerialization.data(withJSONObject: out),
          let text = String(data: json, encoding: .utf8) else { return "{}" }
    return text
}

/// For Rust (iOS app delegate): a userInfo dictionary → payload JSON
/// (malloc'd: free it).
@_cdecl("prism_push_payload")
public func prismPushPayload(_ info: UnsafeRawPointer) -> UnsafeMutablePointer<CChar>? {
    let dict = Unmanaged<NSDictionary>.fromOpaque(info).takeUnretainedValue()
    return strdup(prismPushJSON(dict as? [AnyHashable: Any] ?? [:], id: nil))
}

/// Calls Rust with a push.
func prismDeliverPush(_ kind: Int32, _ info: [AnyHashable: Any], id: String?, action: String? = nil,
                      text: String? = nil, completion: UInt64 = 0) {
    let json = prismPushJSON(info, id: id)
    json.withCString { json in
        prismOptionalCString(action) { action in
            prismOptionalCString(text) { text in prismRustPushReceived(kind, json, action, text, completion) }
        }
    }
}

func prismOptionalCString<R>(_ s: String?, _ body: (UnsafePointer<CChar>?) -> R) -> R {
    guard let s else { return body(nil) }
    return s.withCString { body($0) }
}

/// The aps-environment the build declared (`PrismApsEnvironment` in
/// Info.plist: "development" or "production"), for the token's sandbox flag.
@_cdecl("prism_push_environment")
public func prismPushEnvironment() -> UnsafeMutablePointer<CChar>? {
    guard let env = Bundle.main.object(forInfoDictionaryKey: "PrismApsEnvironment") as? String else { return nil }
    return strdup(env)
}

private func prismRegisterNow() {
    #if os(watchOS)
    WKApplication.shared().registerForRemoteNotifications()
    #elseif canImport(UIKit)
    UIApplication.shared.registerForRemoteNotifications()
    #else
    if NSApplication.shared.delegate == nil { NSApplication.shared.delegate = PrismMacAppDelegate.shared }
    NSApplication.shared.registerForRemoteNotifications()
    #endif
}

/// Asks to show notifications (badges only on tvOS), then registers with
/// APNs. The token arrives in the app delegate.
@_cdecl("prism_push_register")
public func prismPushRegister() {
    guard Bundle.main.bundleIdentifier != nil else {
        "push needs a bundled app".withCString { prismRustPushToken(nil, $0) }
        return
    }
    #if os(tvOS)
    let options: UNAuthorizationOptions = [.badge]
    #else
    let options: UNAuthorizationOptions = [.alert, .sound, .badge]
    #endif
    UNUserNotificationCenter.current().requestAuthorization(options: options) { _, _ in
        // Silent pushes work without the user's permission: register anyway.
        DispatchQueue.main.async { prismRegisterNow() }
    }
}

@_cdecl("prism_push_unregister")
public func prismPushUnregister() {
    DispatchQueue.main.async {
        #if os(watchOS)
        WKApplication.shared().unregisterForRemoteNotifications()
        #elseif canImport(UIKit)
        UIApplication.shared.unregisterForRemoteNotifications()
        #else
        NSApplication.shared.unregisterForRemoteNotifications()
        #endif
    }
}

// MARK: Silent pushes waiting for the app (macOS has no completion handler)

#if os(watchOS)
private var prismPushCompletions: [UInt64: (WKBackgroundFetchResult) -> Void] = [:]
private var prismNextCompletion: UInt64 = 0

/// Rust finished handling silent push `id`.
@_cdecl("prism_push_complete")
public func prismPushComplete(_ id: UInt64, _ newData: Bool) {
    prismPushCompletions.removeValue(forKey: id)?(newData ? .newData : .noData)
}

/// Called by the watch app's delegate (PrismWatch.swift).
func prismWatchRemoteNotification(_ info: [AnyHashable: Any], _ done: @escaping (WKBackgroundFetchResult) -> Void) {
    prismNextCompletion += 1
    let id = prismNextCompletion
    prismPushCompletions[id] = done
    prismDeliverPush(2, info, id: nil, completion: id)
}
#else
@_cdecl("prism_push_complete")
public func prismPushComplete(_ id: UInt64, _ newData: Bool) {}
#endif

#if os(macOS)
/// NSApplication's delegate (prism-macos sets none): the APNs callbacks.
final class PrismMacAppDelegate: NSObject, NSApplicationDelegate {
    static let shared = PrismMacAppDelegate()

    func application(_ application: NSApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        prismHex(deviceToken).withCString { prismRustPushToken($0, nil) }
    }

    func application(_ application: NSApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        error.localizedDescription.withCString { prismRustPushToken(nil, $0) }
    }

    func application(_ application: NSApplication, didReceiveRemoteNotification userInfo: [String: Any]) {
        prismDeliverPush(2, userInfo, id: nil)
    }

    /// The app's URL scheme (`prism-counter:/todos/1`): a widget's link, or a
    /// widget button's action link (`/__prism/widget-action?…`, which the
    /// runtime runs instead of navigating).
    func application(_ application: NSApplication, open urls: [URL]) {
        for url in urls {
            let s = url.absoluteString
            guard let colon = s.firstIndex(of: ":") else { continue }
            var path = String(s[s.index(after: colon)...])
            while path.hasPrefix("//") { path.removeFirst() }
            if !path.hasPrefix("/") { path = "/" + path }
            path.withCString { prismRustOpenLink($0) }
        }
    }
}

/// Implemented in Rust (prism-macos): opens an app path.
@_silgen_name("prism_open_link")
func prismRustOpenLink(_ path: UnsafePointer<CChar>)

/// Installs the delegate before launch finishes (called by prism_bridge_init).
func prismInstallMacDelegate() {
    if NSApplication.shared.delegate == nil { NSApplication.shared.delegate = PrismMacAppDelegate.shared }
}
#endif

// MARK: Live Activity push tokens (iOS 16.1+; push-to-start 17.2+)

#if os(iOS)
/// Reports `activity`'s update token as `key`: the current one (an activity
/// adopted after it started may have it already), then each new one.
func prismWatchActivityToken(_ key: String, _ kind: String, _ activity: Activity<PrismActivityAttributes>) {
    Task {
        var last = activity.pushToken
        if let data = last { await prismReportActivityToken(key, kind, data) }
        for await data in activity.pushTokenUpdates where data != last {
            last = data
            await prismReportActivityToken(key, kind, data)
        }
    }
}

private func prismReportActivityToken(_ key: String, _ kind: String, _ data: Data) async {
    let hex = prismHex(data)
    await MainActor.run {
        key.withCString { key in kind.withCString { kind in hex.withCString { prismRustActivityPushToken(key, kind, $0) } } }
    }
}

private var prismWatchingStartTokens = false

@_cdecl("prism_activity_watch_start_tokens")
public func prismActivityWatchStartTokens() {
    guard !prismWatchingStartTokens else { return }
    prismWatchingStartTokens = true
    if #available(iOS 17.2, *) {
        Task {
            for await data in Activity<PrismActivityAttributes>.pushToStartTokenUpdates {
                let hex = prismHex(data)
                await MainActor.run { "".withCString { kind in hex.withCString { prismRustActivityPushToken(nil, kind, $0) } } }
            }
        }
    }
}
#else
@_cdecl("prism_activity_watch_start_tokens")
public func prismActivityWatchStartTokens() {}
#endif
