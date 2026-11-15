// C-callable bridge from the Rust app to Swift-only Apple APIs: WidgetKit
// (reload), ActivityKit (Live Activities) and UserNotifications. Linked into
// the app binary by prism-ios / prism-macos build scripts.
import Foundation
import UserNotifications
#if canImport(WidgetKit)
import WidgetKit
#endif
#if canImport(ActivityKit)
import ActivityKit
#endif

/// Implemented in Rust: a notification was tapped or one of its actions
/// chosen. `action`, `text` and `link` may be null.
@_silgen_name("prism_notification_response")
func prismRustNotificationResponse(_ id: UnsafePointer<CChar>, _ action: UnsafePointer<CChar>?,
                                   _ text: UnsafePointer<CChar>?, _ link: UnsafePointer<CChar>?)

private func str(_ p: UnsafePointer<CChar>?) -> String? { p.map { String(cString: $0) } }

private func info(_ key: String) -> String? { Bundle.main.object(forInfoDictionaryKey: key) as? String }

// MARK: Notifications

#if os(tvOS)
// tvOS notifications have no title or body (UNNotificationContent): only
// the badge. Capabilities say `notifications: false` there.
@_cdecl("prism_bridge_init")
public func prismBridgeInit() -> Bool { false }
@_cdecl("prism_notify_permission")
public func prismNotifyPermission() {}
@_cdecl("prism_notify")
public func prismNotify(_ json: UnsafePointer<CChar>) {}
@_cdecl("prism_notify_cancel")
public func prismNotifyCancel(_ id: UnsafePointer<CChar>) {}
#else
final class PrismNotificationDelegate: NSObject, UNUserNotificationCenterDelegate {
    static let shared = PrismNotificationDelegate()

    // Show banners even while the app is in the foreground.
    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) {
        let request = notification.request
        if request.trigger is UNPushNotificationTrigger {
            prismDeliverPush(0, request.content.userInfo, id: request.identifier)
        }
        done([.banner, .list, .sound])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                withCompletionHandler done: @escaping () -> Void) {
        let request = response.notification.request
        let info = request.content.userInfo
        let id = request.identifier
        var action: String? = response.actionIdentifier
        if action == UNNotificationDefaultActionIdentifier { action = nil }
        if action == UNNotificationDismissActionIdentifier { action = "dismiss" }
        let text = (response as? UNTextInputNotificationResponse)?.userText
        // A remote push: the app's push handler (its link opens there).
        if request.trigger is UNPushNotificationTrigger {
            DispatchQueue.main.async {
                prismDeliverPush(1, info, id: id, action: action, text: text)
                done()
            }
            return
        }
        let links = info["links"] as? [String: String] ?? [:]
        let link = action.flatMap { links[$0] } ?? (action == nil ? info["link"] as? String : nil)
        DispatchQueue.main.async {
            id.withCString { id in
                withOptionalCString(action) { action in
                    withOptionalCString(text) { text in
                        withOptionalCString(link) { link in
                            prismRustNotificationResponse(id, action, text, link)
                        }
                    }
                }
            }
            done()
        }
    }
}

private func withOptionalCString<R>(_ s: String?, _ body: (UnsafePointer<CChar>?) -> R) -> R {
    guard let s else { return body(nil) }
    return s.withCString { body($0) }
}

@_cdecl("prism_bridge_init")
public func prismBridgeInit() -> Bool {
    #if os(macOS)
    prismInstallMacDelegate()
    #endif
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
    var userInfo: [String: Any] = [:]
    if let l = n["link"] as? String { userInfo["link"] = l }
    // Action buttons live in a category registered alongside the request.
    let actions = (n["actions"] as? [[String: Any]] ?? []).map(makeAction)
    var category: UNNotificationCategory?
    if !actions.isEmpty {
        let id = "prism." + actions.map(\.identifier).joined(separator: "+")
        category = UNNotificationCategory(identifier: id, actions: actions, intentIdentifiers: [], options: [])
        content.categoryIdentifier = id
        var links: [String: String] = [:]
        for a in n["actions"] as? [[String: Any]] ?? [] {
            if let id = a["id"] as? String, let l = a["link"] as? String { links[id] = l }
        }
        userInfo["links"] = links
    }
    content.userInfo = userInfo
    let delay = (n["delay"] as? NSNumber)?.doubleValue ?? 0
    let trigger = delay > 0 ? UNTimeIntervalNotificationTrigger(timeInterval: delay, repeats: false) : nil
    let request = UNNotificationRequest(identifier: n["id"] as? String ?? UUID().uuidString, content: content, trigger: trigger)
    let center = UNUserNotificationCenter.current()
    center.requestAuthorization(options: [.alert, .sound, .badge]) { granted, _ in
        guard granted else { return }
        guard let category else { center.add(request); return }
        center.getNotificationCategories { existing in
            var all = existing.filter { $0.identifier != category.identifier }
            all.insert(category)
            center.setNotificationCategories(all)
            center.add(request)
        }
    }
}

private func makeAction(_ a: [String: Any]) -> UNNotificationAction {
    let id = a["id"] as? String ?? UUID().uuidString
    let title = a["title"] as? String ?? id
    var options: UNNotificationActionOptions = []
    if (a["destructive"] as? Bool) == true { options.insert(.destructive) }
    if (a["foreground"] as? Bool) == true { options.insert(.foreground) }
    if let placeholder = a["reply"] as? String {
        return UNTextInputNotificationAction(identifier: id, title: title, options: options,
                                             textInputButtonTitle: title, textInputPlaceholder: placeholder)
    }
    return UNNotificationAction(identifier: id, title: title, options: options)
}

@_cdecl("prism_notify_cancel")
public func prismNotifyCancel(_ id: UnsafePointer<CChar>) {
    let id = String(cString: id)
    UNUserNotificationCenter.current().removePendingNotificationRequests(withIdentifiers: [id])
    UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: [id])
}
#endif

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
    #if canImport(WidgetKit)
    WidgetCenter.shared.reloadTimelines(ofKind: kind)
    return true
    #else
    return false
    #endif
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
                               _ json: UnsafePointer<CChar>, _ stale: Double, _ push: Bool) -> Bool {
    do {
        // `push`: updates come from the app's server too (needs aps-environment).
        let activity = try Activity.request(attributes: PrismActivityAttributes(kind: String(cString: kind)),
                                            content: content(json, stale), pushType: push ? .token : nil)
        prismActivities[String(cString: key)] = activity
        if push { prismWatchActivityToken(String(cString: key), String(cString: kind), activity) }
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
#elseif os(tvOS) || os(visionOS)
// No Live Activities on tvOS (no ActivityKit).
@_cdecl("prism_activity_supported")
public func prismActivitySupported() -> Bool { false }
@_cdecl("prism_activity_start")
public func prismActivityStart(_ key: UnsafePointer<CChar>, _ kind: UnsafePointer<CChar>,
                               _ json: UnsafePointer<CChar>, _ stale: Double, _ push: Bool) -> Bool { false }
@_cdecl("prism_activity_update")
public func prismActivityUpdate(_ key: UnsafePointer<CChar>, _ json: UnsafePointer<CChar>, _ stale: Double) {}
@_cdecl("prism_activity_end")
public func prismActivityEnd(_ key: UnsafePointer<CChar>, _ json: UnsafePointer<CChar>?, _ immediately: Bool) {}
#endif

// MARK: Background tasks (iOS, tvOS: BGTaskScheduler)

/// Implemented in Rust: run / stop the app's background task `id` (full identifier).
@_silgen_name("prism_background_run")
func prismRustBackgroundRun(_ id: UnsafePointer<CChar>)
@_silgen_name("prism_background_expired")
func prismRustBackgroundExpired(_ id: UnsafePointer<CChar>)

#if os(iOS) || os(visionOS) || os(tvOS)
import BackgroundTasks

private var prismBackgroundTasks: [String: BGTask] = [:]
private var prismBackgroundRegistered = false

/// Registers a launch handler for every identifier in Info.plist
/// (BGTaskSchedulerPermittedIdentifiers). Must run before launch completes.
@_cdecl("prism_bg_register")
public func prismBgRegister() {
    guard !prismBackgroundRegistered else { return }
    prismBackgroundRegistered = true
    let ids = Bundle.main.object(forInfoDictionaryKey: "BGTaskSchedulerPermittedIdentifiers") as? [String] ?? []
    if ProcessInfo.processInfo.environment["PRISM_DEBUG_BACKGROUND"] != nil {
        NSLog("prism: registering background tasks %@", ids.description)
    }
    for id in ids {
        let ok = BGTaskScheduler.shared.register(forTaskWithIdentifier: id, using: .main) { task in
            prismBackgroundTasks[id] = task
            task.expirationHandler = { id.withCString { prismRustBackgroundExpired($0) } }
            id.withCString { prismRustBackgroundRun($0) }
        }
        if !ok { NSLog("prism: could not register background task %@", id) }
    }
}

@_cdecl("prism_bg_schedule")
public func prismBgSchedule(_ id: UnsafePointer<CChar>, _ seconds: Double, _ processing: Bool,
                            _ network: Bool, _ power: Bool) -> Bool {
    let id = String(cString: id)
    let request: BGTaskRequest
    if processing {
        let r = BGProcessingTaskRequest(identifier: id)
        r.requiresNetworkConnectivity = network
        r.requiresExternalPower = power
        request = r
    } else {
        request = BGAppRefreshTaskRequest(identifier: id)
    }
    request.earliestBeginDate = Date(timeIntervalSinceNow: seconds)
    do {
        try BGTaskScheduler.shared.submit(request)
        return true
    } catch {
        NSLog("prism: background task %@ not scheduled: %@", id, String(describing: error))
        return false
    }
}

@_cdecl("prism_bg_complete")
public func prismBgComplete(_ id: UnsafePointer<CChar>, _ success: Bool) {
    prismBackgroundTasks.removeValue(forKey: String(cString: id))?.setTaskCompleted(success: success)
}
#endif
