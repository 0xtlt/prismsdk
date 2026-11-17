// The app icon's badge (#239), set on its own: the Dock tile on macOS,
// `setBadgeCount` (needs the notification permission with badges) on iOS,
// iPadOS, tvOS and visionOS. Nothing on watchOS.
import Foundation
#if os(macOS)
import AppKit
#elseif !os(watchOS)
import UIKit
import UserNotifications
#endif

/// `n` shown on the app icon; 0 or less clears it.
@_cdecl("prism_set_badge")
public func prismSetBadge(_ n: Int32) {
    DispatchQueue.main.async {
        #if os(macOS)
        NSApplication.shared.dockTile.badgeLabel = n > 0 ? String(n) : nil
        #elseif os(watchOS)
        _ = n
        #else
        // Outside an app bundle there is no notification center.
        guard Bundle.main.bundleIdentifier != nil else { return }
        let count = max(0, Int(n))
        if #available(iOS 16.0, tvOS 16.0, *) {
            UNUserNotificationCenter.current().setBadgeCount(count) { _ in }
        } else {
            UIApplication.shared.applicationIconBadgeNumber = count
        }
        #endif
    }
}
