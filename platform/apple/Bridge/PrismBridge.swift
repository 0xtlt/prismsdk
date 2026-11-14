// C-callable bridge from the Rust app to Swift-only Apple APIs: WidgetKit
// (reload), ActivityKit (Live Activities) and UserNotifications. Linked into
// the app binary by prism-ios / prism-macos build scripts.
import Foundation
import UserNotifications
import WidgetKit
#if os(iOS)
import ActivityKit
#endif

/// Implemented in Rust: opens an app path (notification tap).
@_silgen_name("prism_open_link")
func prismRustOpenLink(_ path: UnsafePointer<CChar>)

private func str(_ p: UnsafePointer<CChar>?) -> String? { p.map { String(cString: $0) } }

private func info(_ key: String) -> String? { Bundle.main.object(forInfoDictionaryKey: key) as? String }

// MARK: Notifications

final class PrismNotificationDelegate: NSObject, UNUserNotificationCenterDelegate {
    static let shared = PrismNotificationDelegate()

    // Show banners even while the app is in the foreground.
    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) {
        done([.banner, .list, .sound])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                withCompletionHandler done: @escaping () -> Void) {
        if let link = response.notification.request.content.userInfo["link"] as? String {
            DispatchQueue.main.async { link.withCString { prismRustOpenLink($0) } }
        }
        done()
    }
}

@_cdecl("prism_bridge_init")
public func prismBridgeInit() -> Bool {
    // Notifications need a bundled app (not `cargo run` on macOS).
    guard Bundle.main.bundleIdentifier != nil else { return false }
    UNUserNotificationCenter.current().delegate = PrismNotificationDelegate.shared
    return true
}

@_cdecl("prism_notify_permission")
public func prismNotifyPermission() {
    UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in }
}

@_cdecl("prism_notify")
public func prismNotify(_ json: UnsafePointer<CChar>) {
    guard let data = String(cString: json).data(using: .utf8),
          let n = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return }
    let content = UNMutableNotificationContent()
    content.title = n["title"] as? String ?? ""
    if let s = n["subtitle"] as? String { content.subtitle = s }
    content.body = n["body"] as? String ?? ""
    if (n["sound"] as? Bool) ?? true { content.sound = .default }
    if let b = n["badge"] as? NSNumber { content.badge = b }
    if let t = n["thread"] as? String { content.threadIdentifier = t }
    if let l = n["link"] as? String { content.userInfo = ["link": l] }
    let delay = (n["delay"] as? NSNumber)?.doubleValue ?? 0
    let trigger = delay > 0 ? UNTimeIntervalNotificationTrigger(timeInterval: delay, repeats: false) : nil
    let request = UNNotificationRequest(identifier: n["id"] as? String ?? UUID().uuidString, content: content, trigger: trigger)
    let center = UNUserNotificationCenter.current()
    center.requestAuthorization(options: [.alert, .sound, .badge]) { granted, _ in
        guard granted else { return }
        center.add(request)
    }
}

@_cdecl("prism_notify_cancel")
public func prismNotifyCancel(_ id: UnsafePointer<CChar>) {
    let id = String(cString: id)
    UNUserNotificationCenter.current().removePendingNotificationRequests(withIdentifiers: [id])
    UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: [id])
}

// MARK: Widgets

/// Writes a widget snapshot to the App Group container and reloads it.
@_cdecl("prism_widgets_write")
public func prismWidgetsWrite(_ kind: UnsafePointer<CChar>, _ json: UnsafePointer<CChar>) -> Bool {
    let kind = String(cString: kind)
    guard let group = info("PrismAppGroup"),
          let base = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)
    else { return false }
    let dir = base.appendingPathComponent("prism-widgets", isDirectory: true)
    do {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try String(cString: json).write(to: dir.appendingPathComponent("\(kind).json"), atomically: true, encoding: .utf8)
    } catch {
        NSLog("prism: cannot write widget %@: %@", kind, "\(error)")
        return false
    }
    WidgetCenter.shared.reloadTimelines(ofKind: kind)
    return true
}

// MARK: Live Activities

#if os(iOS)
private var prismActivities: [String: Activity<PrismActivityAttributes>] = [:]

private func content(_ json: UnsafePointer<CChar>, _ stale: Double) -> ActivityContent<PrismActivityAttributes.ContentState> {
    ActivityContent(state: .init(json: String(cString: json)),
                    staleDate: stale > 0 ? Date(timeIntervalSince1970: stale) : nil)
}

@_cdecl("prism_activity_supported")
public func prismActivitySupported() -> Bool { ActivityAuthorizationInfo().areActivitiesEnabled }

@_cdecl("prism_activity_start")
public func prismActivityStart(_ key: UnsafePointer<CChar>, _ kind: UnsafePointer<CChar>,
                               _ json: UnsafePointer<CChar>, _ stale: Double) -> Bool {
    do {
        let activity = try Activity.request(attributes: PrismActivityAttributes(kind: String(cString: kind)),
                                            content: content(json, stale), pushType: nil)
        prismActivities[String(cString: key)] = activity
        return true
    } catch {
        NSLog("prism: Live Activity failed: %@", "\(error)")
        return false
    }
}

@_cdecl("prism_activity_update")
public func prismActivityUpdate(_ key: UnsafePointer<CChar>, _ json: UnsafePointer<CChar>, _ stale: Double) {
    guard let activity = prismActivities[String(cString: key)] else { return }
    let c = content(json, stale)
    Task { await activity.update(c) }
}

@_cdecl("prism_activity_end")
public func prismActivityEnd(_ key: UnsafePointer<CChar>, _ json: UnsafePointer<CChar>?, _ immediately: Bool) {
    guard let activity = prismActivities.removeValue(forKey: String(cString: key)) else { return }
    let c = json.map { content($0, 0) }
    Task { await activity.end(c, dismissalPolicy: immediately ? .immediate : .default) }
}
#endif
