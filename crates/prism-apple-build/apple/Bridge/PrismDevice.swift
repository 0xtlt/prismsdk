// Device APIs for the Rust app (prism_core::device): permissions, location,
// motion, photo/camera pickers, haptics, clipboard, share sheet, URLs.
// Each call carries an id; answers go back through the C functions below.
import AVFoundation
import CoreLocation
import Foundation
#if canImport(Photos)
import Photos
#endif
import UniformTypeIdentifiers
import UserNotifications
#if canImport(CoreMotion)
import CoreMotion
#endif
#if os(iOS) || os(visionOS) || os(tvOS)
import PhotosUI
#endif
#if canImport(UIKit)
import UIKit
#else
import AppKit
#endif
#if os(watchOS)
import WatchKit

// Pickers and the share sheet are SwiftUI views on the watch: the host
// (apple/Watch) presents them.
@_silgen_name("prism_watch_pick_images")
func prismWatchPickImages(_ id: UInt64, _ limit: UInt32)
@_silgen_name("prism_watch_share")
func prismWatchShare(_ text: UnsafePointer<CChar>?, _ url: UnsafePointer<CChar>?)
#endif

// MARK: Answers (implemented in Rust)

@_silgen_name("prism_device_reply_permission")
func prismReplyPermission(_ id: UInt64, _ state: UnsafePointer<CChar>)
/// NaN for unknown altitude/speed/heading. `event`: a stream event.
@_silgen_name("prism_device_reply_location")
func prismReplyLocation(_ id: UInt64, _ event: Bool, _ lat: Double, _ lon: Double, _ alt: Double,
                        _ accuracy: Double, _ speed: Double, _ heading: Double, _ time: Double)
@_silgen_name("prism_device_motion")
func prismMotion(_ id: UInt64, _ ax: Double, _ ay: Double, _ az: Double, _ gx: Double, _ gy: Double, _ gz: Double,
                 _ rx: Double, _ ry: Double, _ rz: Double, _ roll: Double, _ pitch: Double, _ yaw: Double)
/// One file per line: path \t name \t mime \t size.
@_silgen_name("prism_device_reply_files")
func prismReplyFiles(_ id: UInt64, _ lines: UnsafePointer<CChar>)
@_silgen_name("prism_device_reply_text")
func prismReplyText(_ id: UInt64, _ text: UnsafePointer<CChar>?)
/// `denied`, `cancelled`, `unavailable` or a message.
@_silgen_name("prism_device_reply_error")
func prismReplyError(_ id: UInt64, _ event: Bool, _ message: UnsafePointer<CChar>)

private func fail(_ id: UInt64, _ message: String, event: Bool = false) {
    DispatchQueue.main.async { message.withCString { prismReplyError(id, event, $0) } }
}

private func permissionReply(_ id: UInt64, _ state: String) {
    DispatchQueue.main.async { state.withCString { prismReplyPermission(id, $0) } }
}

/// Info.plist must explain why the app asks; `prism` adds the texts from
/// `[permissions]` in Prism.toml.
private func usageMissing(_ id: UInt64, _ key: String, _ name: String, event: Bool = false) -> Bool {
    if Bundle.main.object(forInfoDictionaryKey: key) != nil { return false }
    fail(id, "add `\(name) = \"why the app needs it\"` under [permissions] in Prism.toml (Info.plist \(key))", event: event)
    return true
}

private func string(_ p: UnsafePointer<CChar>?) -> String? { p.map { String(cString: $0) } }

// MARK: Location

private let locationUsageKey: String = {
    #if os(iOS) || os(visionOS) || os(tvOS)
    return "NSLocationWhenInUseUsageDescription"
    #else
    return "NSLocationUsageDescription"
    #endif
}()

private func locationState(_ status: CLAuthorizationStatus, precise: Bool) -> String {
    switch status {
    case .notDetermined: return "not_determined"
    case .denied, .restricted: return "denied"
    default: return precise ? "granted" : "limited"
    }
}

/// One manager per call or stream; it answers through its delegate.
final class PrismLocation: NSObject, CLLocationManagerDelegate {
    static var active: [UInt64: PrismLocation] = [:]
    let id: UInt64
    let manager = CLLocationManager()
    /// once: one location; stream: updates; permission: just the prompt.
    enum Mode { case once, stream, permission(always: Bool) }
    let mode: Mode

    init(id: UInt64, mode: Mode, accuracy: Int32) {
        self.id = id
        self.mode = mode
        super.init()
        manager.delegate = self
        manager.desiredAccuracy = accuracy == 0 ? kCLLocationAccuracyKilometer
            : accuracy == 2 ? kCLLocationAccuracyBest : kCLLocationAccuracyHundredMeters
    }

    func start() {
        PrismLocation.active[id] = self
        switch manager.authorizationStatus {
        case .notDetermined:
            #if os(iOS)
            if case .permission(true) = mode { manager.requestAlwaysAuthorization() } else { manager.requestWhenInUseAuthorization() }
            #else
            manager.requestWhenInUseAuthorization()
            #endif
        case .denied, .restricted:
            if case .permission = mode { finishPermission() } else { error("denied") }
        default:
            proceed()
        }
    }

    private func proceed() {
        switch mode {
        case .once: manager.requestLocation()
        case .stream:
            #if os(tvOS)
            error("unavailable") // tvOS: one location at a time
            #else
            manager.startUpdatingLocation()
            #endif
        case .permission: finishPermission()
        }
    }

    private func finishPermission() {
        let precise = manager.accuracyAuthorization == .fullAccuracy
        permissionReply(id, locationState(manager.authorizationStatus, precise: precise))
        PrismLocation.active[id] = nil
    }

    private func error(_ message: String) {
        if case .stream = mode { fail(id, message, event: true) } else { fail(id, message); PrismLocation.active[id] = nil }
    }

    func locationManagerDidChangeAuthorization(_ m: CLLocationManager) {
        switch m.authorizationStatus {
        case .notDetermined: return
        case .denied, .restricted:
            if case .permission = mode { finishPermission() } else { error("denied") }
        default: proceed()
        }
    }

    func locationManager(_ m: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        guard let l = locations.last else { return }
        let event: Bool
        if case .stream = mode { event = true } else { event = false }
        prismReplyLocation(id, event, l.coordinate.latitude, l.coordinate.longitude,
                           l.verticalAccuracy >= 0 ? l.altitude : .nan, l.horizontalAccuracy,
                           l.speed >= 0 ? l.speed : .nan, l.course >= 0 ? l.course : .nan,
                           l.timestamp.timeIntervalSince1970)
        if !event { PrismLocation.active[id] = nil }
    }

    func locationManager(_ m: CLLocationManager, didFailWithError e: Error) {
        let code = (e as? CLError)?.code
        error(code == .denied ? "denied" : code == .locationUnknown ? "location unknown" : e.localizedDescription)
    }
}

// MARK: Calls (from Rust)

@_cdecl("prism_device_permission")
public func prismDevicePermission(_ id: UInt64, _ name: UnsafePointer<CChar>, _ ask: Bool) {
    let name = String(cString: name)
    switch name {
    case "location", "location_always":
        let manager = CLLocationManager()
        if !ask || manager.authorizationStatus != .notDetermined {
            return permissionReply(id, locationState(manager.authorizationStatus, precise: manager.accuracyAuthorization == .fullAccuracy))
        }
        if usageMissing(id, locationUsageKey, "location") { return }
        PrismLocation(id: id, mode: .permission(always: name == "location_always"), accuracy: 1).start()
    #if os(watchOS)
    case "camera":
        permissionReply(id, "unavailable") // no camera on the watch
    case "microphone":
        let reply = { (p: AVAudioApplication.recordPermission) in
            permissionReply(id, p == .granted ? "granted" : p == .undetermined ? "not_determined" : "denied")
        }
        let status = AVAudioApplication.shared.recordPermission
        guard ask, status == .undetermined else { return reply(status) }
        if usageMissing(id, "NSMicrophoneUsageDescription", name) { return }
        AVAudioApplication.requestRecordPermission { _ in reply(AVAudioApplication.shared.recordPermission) }
    #else
    case "camera", "microphone":
        let media: AVMediaType = name == "camera" ? .video : .audio
        let reply = { (s: AVAuthorizationStatus) in
            permissionReply(id, s == .authorized ? "granted" : s == .notDetermined ? "not_determined" : "denied")
        }
        let status = AVCaptureDevice.authorizationStatus(for: media)
        guard ask, status == .notDetermined else { return reply(status) }
        if usageMissing(id, name == "camera" ? "NSCameraUsageDescription" : "NSMicrophoneUsageDescription", name) { return }
        AVCaptureDevice.requestAccess(for: media) { _ in reply(AVCaptureDevice.authorizationStatus(for: media)) }
    #endif
    #if os(watchOS)
    case "photos":
        // The picker runs out of process: no library permission needed.
        permissionReply(id, "granted")
    #else
    case "photos":
        let reply = { (s: PHAuthorizationStatus) in
            permissionReply(id, s == .authorized ? "granted" : s == .limited ? "limited" : s == .notDetermined ? "not_determined" : "denied")
        }
        let status = PHPhotoLibrary.authorizationStatus(for: .readWrite)
        guard ask, status == .notDetermined else { return reply(status) }
        if usageMissing(id, "NSPhotoLibraryUsageDescription", "photos") { return }
        PHPhotoLibrary.requestAuthorization(for: .readWrite) { reply($0) }
    #endif
    case "motion":
        #if os(iOS) || os(visionOS) || os(watchOS)
        // Gyroscope and accelerometer need no permission (only activity does).
        permissionReply(id, CMMotionManager().isDeviceMotionAvailable ? "granted" : "unavailable")
        #else
        permissionReply(id, "unavailable")
        #endif
    case "notifications":
        // Outside an app bundle (`cargo run` on macOS) the center throws (#239).
        guard Bundle.main.bundleIdentifier != nil else { return permissionReply(id, "unavailable") }
        let center = UNUserNotificationCenter.current()
        let map = { (s: UNAuthorizationStatus) -> String in
            switch s {
            case .notDetermined: return "not_determined"
            case .denied: return "denied"
            case .provisional, .ephemeral: return "limited"
            default: return "granted"
            }
        }
        center.getNotificationSettings { settings in
            guard ask, settings.authorizationStatus == .notDetermined else { return permissionReply(id, map(settings.authorizationStatus)) }
            center.requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in
                center.getNotificationSettings { permissionReply(id, map($0.authorizationStatus)) }
            }
        }
    default:
        permissionReply(id, "unavailable")
    }
}

@_cdecl("prism_device_location")
public func prismDeviceLocation(_ id: UInt64, _ accuracy: Int32, _ stream: Bool) {
    if usageMissing(id, locationUsageKey, "location", event: stream) { return }
    PrismLocation(id: id, mode: stream ? .stream : .once, accuracy: accuracy).start()
}

#if os(iOS) || os(visionOS) || os(watchOS)
private var motionManagers: [UInt64: CMMotionManager] = [:]
#endif

@_cdecl("prism_device_motion_start")
public func prismDeviceMotionStart(_ id: UInt64, _ interval: Double) {
    #if os(iOS) || os(visionOS) || os(watchOS)
    let m = CMMotionManager()
    guard m.isDeviceMotionAvailable else { return fail(id, "unavailable", event: true) }
    m.deviceMotionUpdateInterval = max(interval, 0.01)
    motionManagers[id] = m
    m.startDeviceMotionUpdates(to: .main) { data, error in
        if let error { return fail(id, error.localizedDescription, event: true) }
        guard let d = data else { return }
        prismMotion(id, d.userAcceleration.x, d.userAcceleration.y, d.userAcceleration.z,
                    d.gravity.x, d.gravity.y, d.gravity.z,
                    d.rotationRate.x, d.rotationRate.y, d.rotationRate.z,
                    d.attitude.roll, d.attitude.pitch, d.attitude.yaw)
    }
    #else
    fail(id, "unavailable", event: true)
    #endif
}

@_cdecl("prism_device_stop")
public func prismDeviceStop(_ id: UInt64) {
    if let l = PrismLocation.active.removeValue(forKey: id) { l.manager.stopUpdatingLocation() }
    #if os(iOS) || os(visionOS) || os(watchOS)
    motionManagers.removeValue(forKey: id)?.stopDeviceMotionUpdates()
    #endif
}

// MARK: Photos and camera

/// Copies picked files where the app can read them (Caches/prism-picked).
func prismKeep(_ url: URL, ext: String? = nil) -> String? {
    let dir = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("prism-picked")
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    let dest = dir.appendingPathComponent(UUID().uuidString).appendingPathExtension(ext ?? url.pathExtension)
    do { try FileManager.default.copyItem(at: url, to: dest) } catch { return nil }
    return dest.path
}

func prismDescribe(_ path: String, name: String) -> String {
    let url = URL(fileURLWithPath: path)
    let size = (try? FileManager.default.attributesOfItem(atPath: path)[.size] as? NSNumber)?.int64Value ?? 0
    let mime = UTType(filenameExtension: url.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
    return [path, name, mime, String(size)].joined(separator: "\t")
}

private func replyFiles(_ id: UInt64, _ lines: [String]) {
    DispatchQueue.main.async {
        if lines.isEmpty { return "cancelled".withCString { prismReplyError(id, false, $0) } }
        lines.joined(separator: "\n").withCString { prismReplyFiles(id, $0) }
    }
}

#if os(iOS) || os(visionOS)
private var pickers: [UInt64: NSObject] = [:]

private func topController() -> UIViewController? {
    let windows = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.flatMap(\.windows)
    var top = (windows.first(where: \.isKeyWindow) ?? windows.first)?.rootViewController
    while let p = top?.presentedViewController { top = p }
    return top
}

final class PrismPhotoPicker: NSObject, PHPickerViewControllerDelegate {
    let id: UInt64
    init(id: UInt64) { self.id = id }

    func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
        picker.dismiss(animated: true)
        let group = DispatchGroup()
        var lines: [String] = []
        let lock = NSLock()
        for r in results {
            let provider = r.itemProvider
            guard let type = provider.registeredTypeIdentifiers.first(where: { UTType($0)?.conforms(to: .image) == true }) else { continue }
            group.enter()
            provider.loadFileRepresentation(forTypeIdentifier: type) { url, _ in
                if let url, let path = prismKeep(url, ext: UTType(type)?.preferredFilenameExtension) {
                    lock.lock(); lines.append(prismDescribe(path, name: provider.suggestedName ?? url.lastPathComponent)); lock.unlock()
                }
                group.leave()
            }
        }
        let id = self.id
        group.notify(queue: .main) { replyFiles(id, lines); pickers[id] = nil }
    }
}

final class PrismCamera: NSObject, UIImagePickerControllerDelegate, UINavigationControllerDelegate {
    let id: UInt64
    init(id: UInt64) { self.id = id }

    func imagePickerController(_ picker: UIImagePickerController, didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]) {
        picker.dismiss(animated: true)
        var lines: [String] = []
        if let image = info[.originalImage] as? UIImage, let data = image.jpegData(compressionQuality: 0.9) {
            let tmp = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".jpg")
            if (try? data.write(to: tmp)) != nil, let path = prismKeep(tmp) { lines.append(prismDescribe(path, name: "Photo.jpg")) }
        }
        replyFiles(id, lines)
        pickers[id] = nil
    }

    func imagePickerControllerDidCancel(_ picker: UIImagePickerController) {
        picker.dismiss(animated: true)
        replyFiles(id, [])
        pickers[id] = nil
    }
}
#endif

@_cdecl("prism_device_pick_images")
public func prismDevicePickImages(_ id: UInt64, _ limit: UInt32, _ camera: Bool) {
    #if os(watchOS)
    if camera { return fail(id, "unavailable") } // no camera on the watch
    prismWatchPickImages(id, limit)
    #elseif os(tvOS)
    fail(id, "unavailable") // no photo picker or camera on tvOS
    #elseif os(iOS) || os(visionOS)
    guard let top = topController() else { return fail(id, "no window to present the picker") }
    #if os(visionOS)
    if camera { return fail(id, "unavailable") } // UIImagePickerController has no camera on visionOS
    #else
    if camera {
        guard UIImagePickerController.isSourceTypeAvailable(.camera) else { return fail(id, "unavailable") }
        if usageMissing(id, "NSCameraUsageDescription", "camera") { return }
        let delegate = PrismCamera(id: id)
        pickers[id] = delegate
        let picker = UIImagePickerController()
        picker.sourceType = .camera
        picker.delegate = delegate
        top.present(picker, animated: true)
        return
    }
    #endif
    // PHPicker runs out of process: no photo library permission needed.
    var config = PHPickerConfiguration()
    config.filter = .images
    config.selectionLimit = Int(limit)
    let delegate = PrismPhotoPicker(id: id)
    pickers[id] = delegate
    let picker = PHPickerViewController(configuration: config)
    picker.delegate = delegate
    top.present(picker, animated: true)
    #else
    guard !camera else { return fail(id, "unavailable") }
    let panel = NSOpenPanel()
    panel.allowedContentTypes = [.image]
    panel.allowsMultipleSelection = limit != 1
    panel.canChooseDirectories = false
    panel.begin { response in
        guard response == .OK else { return replyFiles(id, []) }
        var urls = panel.urls
        if limit > 0 { urls = Array(urls.prefix(Int(limit))) }
        replyFiles(id, urls.compactMap { url in prismKeep(url).map { prismDescribe($0, name: url.lastPathComponent) } })
    }
    #endif
}

// MARK: Files

/// UTTypes for `types` (one per line: MIME types such as `image/*`, or
/// extensions such as `.pdf`). Empty: any file.
func prismContentTypes(_ types: String) -> [UTType] {
    let list = types.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces).lowercased() }.filter { !$0.isEmpty }
    if list.isEmpty { return [.item] }
    let found: [UTType] = list.compactMap { t in
        if t.hasPrefix(".") { return UTType(filenameExtension: String(t.dropFirst())) }
        if t.hasSuffix("/*") {
            switch t.dropLast(2) {
            case "image": return .image
            case "video": return .movie
            case "audio": return .audio
            case "text": return .text
            default: return nil
            }
        }
        return UTType(mimeType: t)
    }
    return found.isEmpty ? [.item] : found
}

#if os(iOS) || os(visionOS)
final class PrismDocumentPicker: NSObject, UIDocumentPickerDelegate {
    let id: UInt64
    init(id: UInt64) { self.id = id }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        // `asCopy`: the files are already in the app's sandbox (tmp/Inbox).
        replyFiles(id, urls.compactMap { url in prismKeep(url).map { prismDescribe($0, name: url.lastPathComponent) } })
        pickers[id] = nil
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        replyFiles(id, [])
        pickers[id] = nil
    }
}
#endif

@_cdecl("prism_device_pick_files")
public func prismDevicePickFiles(_ id: UInt64, _ types: UnsafePointer<CChar>, _ multiple: Bool) {
    let types = prismContentTypes(String(cString: types))
    #if os(watchOS) || os(tvOS)
    _ = types
    fail(id, "unavailable") // no document picker on the watch or the TV
    #elseif os(iOS) || os(visionOS)
    guard let top = topController() else { return fail(id, "no window to present the picker") }
    let picker = UIDocumentPickerViewController(forOpeningContentTypes: types, asCopy: true)
    picker.allowsMultipleSelection = multiple
    let delegate = PrismDocumentPicker(id: id)
    pickers[id] = delegate
    picker.delegate = delegate
    top.present(picker, animated: true)
    #else
    let panel = NSOpenPanel()
    panel.allowedContentTypes = types
    panel.allowsMultipleSelection = multiple
    panel.canChooseDirectories = false
    panel.canChooseFiles = true
    panel.begin { response in
        guard response == .OK else { return replyFiles(id, []) }
        replyFiles(id, panel.urls.compactMap { url in prismKeep(url).map { prismDescribe($0, name: url.lastPathComponent) } })
    }
    #endif
}

// MARK: Folders (#225)

/// The regular files under `root`, with their path in it (`/` between
/// folders); hidden files and package contents left out.
func prismFolderFiles(_ root: URL) -> [(URL, String)] {
    let keys: [URLResourceKey] = [.isRegularFileKey]
    guard let walk = FileManager.default.enumerator(at: root, includingPropertiesForKeys: keys,
                                                    options: [.skipsHiddenFiles, .skipsPackageDescendants]) else { return [] }
    let base = root.standardizedFileURL.path
    var out: [(URL, String)] = []
    for case let url as URL in walk {
        guard (try? url.resourceValues(forKeys: Set(keys)))?.isRegularFile == true else { continue }
        var rel = url.standardizedFileURL.path
        if rel.hasPrefix(base) { rel = String(rel.dropFirst(base.count)) }
        rel = rel.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        // Lines and fields of the answer stay apart.
        out.append((url, rel.replacingOccurrences(of: "\t", with: " ").replacingOccurrences(of: "\n", with: " ")))
    }
    return out
}

/// The answer's first line: the folder (`inode/directory`), its path when
/// its files are read in place.
func prismFolderLine(_ root: URL, inPlace: Bool) -> String {
    [inPlace ? root.path : "", root.lastPathComponent, "inode/directory", "0"].joined(separator: "\t")
}

#if os(iOS) || os(visionOS)
final class PrismFolderPicker: NSObject, UIDocumentPickerDelegate {
    let id: UInt64
    init(id: UInt64) { self.id = id }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        pickers[id] = nil
        guard let root = urls.first else { return replyFiles(id, []) }
        let id = self.id
        DispatchQueue.global(qos: .userInitiated).async {
            // The folder is the app's while it reads it: its files are copied.
            let scoped = root.startAccessingSecurityScopedResource()
            defer { if scoped { root.stopAccessingSecurityScopedResource() } }
            let files = prismFolderFiles(root).compactMap { url, rel in prismKeep(url).map { prismDescribe($0, name: rel) } }
            replyFiles(id, [prismFolderLine(root, inPlace: false)] + files)
        }
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        replyFiles(id, [])
        pickers[id] = nil
    }
}
#endif

@_cdecl("prism_device_pick_folder")
public func prismDevicePickFolder(_ id: UInt64, _ title: UnsafePointer<CChar>?) {
    let title = string(title)
    #if os(watchOS) || os(tvOS)
    _ = title
    fail(id, "unavailable") // no folders to pick on the watch or the TV
    #elseif os(iOS) || os(visionOS)
    _ = title
    guard let top = topController() else { return fail(id, "no window to present the picker") }
    let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.folder])
    let delegate = PrismFolderPicker(id: id)
    pickers[id] = delegate
    picker.delegate = delegate
    top.present(picker, animated: true)
    #else
    let panel = NSOpenPanel()
    panel.canChooseDirectories = true
    panel.canChooseFiles = false
    panel.allowsMultipleSelection = false
    if let t = title { panel.message = t; panel.prompt = t }
    panel.begin { response in
        guard response == .OK, let root = panel.url else { return replyFiles(id, []) }
        // Read in place: the panel gave the app this folder.
        DispatchQueue.global(qos: .userInitiated).async {
            let files = prismFolderFiles(root).map { url, rel in prismDescribe(url.path, name: rel) }
            replyFiles(id, [prismFolderLine(root, inPlace: true)] + files)
        }
    }
    #endif
}

// MARK: Haptics, clipboard, share, URLs

@_cdecl("prism_device_haptic")
public func prismDeviceHaptic(_ kind: UnsafePointer<CChar>) {
    let kind = String(cString: kind)
    #if os(watchOS)
    let haptic: WKHapticType
    switch kind {
    case "success": haptic = .success
    case "warning": haptic = .retry
    case "error": haptic = .failure
    case "heavy": haptic = .notification
    case "medium": haptic = .start
    default: haptic = .click // selection, light
    }
    WKInterfaceDevice.current().play(haptic)
    #elseif os(tvOS) || os(visionOS)
    _ = kind // no haptics on a TV or a headset (UIFeedbackGenerator is unavailable)
    #elseif os(iOS)
    switch kind {
    case "selection": UISelectionFeedbackGenerator().selectionChanged()
    case "success": UINotificationFeedbackGenerator().notificationOccurred(.success)
    case "warning": UINotificationFeedbackGenerator().notificationOccurred(.warning)
    case "error": UINotificationFeedbackGenerator().notificationOccurred(.error)
    case "light": UIImpactFeedbackGenerator(style: .light).impactOccurred()
    case "heavy": UIImpactFeedbackGenerator(style: .heavy).impactOccurred()
    default: UIImpactFeedbackGenerator(style: .medium).impactOccurred()
    }
    #else
    let pattern: NSHapticFeedbackManager.FeedbackPattern = kind == "selection" ? .alignment : kind == "light" ? .generic : .levelChange
    NSHapticFeedbackManager.defaultPerformer.perform(pattern, performanceTime: .now)
    #endif
}

@_cdecl("prism_device_copy")
public func prismDeviceCopy(_ text: UnsafePointer<CChar>) {
    let text = String(cString: text)
    #if os(tvOS) || os(watchOS)
    _ = text // no clipboard on tvOS or watchOS
    #elseif os(iOS) || os(visionOS)
    UIPasteboard.general.string = text
    #else
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(text, forType: .string)
    #endif
}

@_cdecl("prism_device_paste")
public func prismDevicePaste(_ id: UInt64) {
    #if os(tvOS) || os(watchOS)
    let text: String? = nil
    #elseif os(iOS) || os(visionOS)
    let text = UIPasteboard.general.string
    #else
    let text = NSPasteboard.general.string(forType: .string)
    #endif
    if let text { text.withCString { prismReplyText(id, $0) } } else { prismReplyText(id, nil) }
}

@_cdecl("prism_device_share")
public func prismDeviceShare(_ text: UnsafePointer<CChar>?, _ url: UnsafePointer<CChar>?, _ files: UnsafePointer<CChar>?) {
    var items: [Any] = []
    if let t = string(text) { items.append(t) }
    if let u = string(url), let link = URL(string: u) { items.append(link) }
    for path in (string(files) ?? "").split(separator: "\n") where !path.isEmpty {
        items.append(URL(fileURLWithPath: String(path)))
    }
    guard !items.isEmpty else { return }
    #if os(watchOS)
    // ShareLink shares text or a URL (files: their URLs).
    let shared = items.compactMap { $0 as? String }.first
    let link = items.compactMap { $0 as? URL }.first?.absoluteString
    withOptional(shared) { t in withOptional(link) { u in prismWatchShare(t, u) } }
    #elseif os(tvOS)
    return // no share sheet on tvOS
    #elseif os(iOS) || os(visionOS)
    guard let top = topController() else { return }
    let sheet = UIActivityViewController(activityItems: items, applicationActivities: nil)
    // iPad: a popover anchored to the middle of the screen.
    sheet.popoverPresentationController?.sourceView = top.view
    sheet.popoverPresentationController?.sourceRect = CGRect(x: top.view.bounds.midX, y: top.view.bounds.midY, width: 0, height: 0)
    top.present(sheet, animated: true)
    #else
    guard let view = NSApp.keyWindow?.contentView ?? NSApp.windows.first?.contentView else { return }
    NSSharingServicePicker(items: items).show(relativeTo: CGRect(x: view.bounds.midX, y: view.bounds.maxY - 40, width: 1, height: 1), of: view, preferredEdge: .minY)
    #endif
}

@_cdecl("prism_device_open_url")
public func prismDeviceOpenURL(_ url: UnsafePointer<CChar>) {
    guard let url = URL(string: String(cString: url)) else { return }
    #if os(watchOS)
    WKApplication.shared().openSystemURL(url)
    #elseif os(iOS) || os(visionOS) || os(tvOS)
    UIApplication.shared.open(url)
    #else
    NSWorkspace.shared.open(url)
    #endif
}

// MARK: Saving a file the app holds (#210)

/// Removes a copy Rust staged for the user (`prism_native::save`).
private func prismDiscardStaged(_ url: URL) {
    let folder = url.deletingLastPathComponent()
    if folder.lastPathComponent.hasPrefix("prism-save-") {
        try? FileManager.default.removeItem(at: folder)
    }
}

#if os(iOS) || os(visionOS)
final class PrismExportPicker: NSObject, UIDocumentPickerDelegate {
    let id: UInt64
    let source: URL
    init(id: UInt64, source: URL) { self.id = id; self.source = source }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        // The exported copy, where the user put it in Files.
        replyFiles(id, urls.prefix(1).map { prismDescribe($0.path, name: $0.lastPathComponent) })
        done()
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        replyFiles(id, [])
        done()
    }

    private func done() {
        prismDiscardStaged(source)
        pickers[id] = nil
    }
}
#endif

/// `path`: a file named as offered (staged by Rust when needed).
@_cdecl("prism_device_save_file")
public func prismDeviceSaveFile(_ id: UInt64, _ path: UnsafePointer<CChar>, _ name: UnsafePointer<CChar>) {
    let source = URL(fileURLWithPath: String(cString: path))
    let name = String(cString: name)
    #if os(watchOS) || os(tvOS)
    _ = name
    prismDiscardStaged(source)
    fail(id, "unavailable") // no Files app on the watch or the TV
    #elseif os(iOS) || os(visionOS)
    // "Save to Files": the user picks the folder (and may rename it).
    _ = name
    guard let top = topController() else {
        prismDiscardStaged(source)
        return fail(id, "no window to present the picker")
    }
    let picker = UIDocumentPickerViewController(forExporting: [source], asCopy: true)
    let delegate = PrismExportPicker(id: id, source: source)
    pickers[id] = delegate
    picker.delegate = delegate
    top.present(picker, animated: true)
    #else
    let panel = NSSavePanel()
    panel.nameFieldStringValue = name
    panel.canCreateDirectories = true
    panel.isExtensionHidden = false
    let finish: (NSApplication.ModalResponse) -> Void = { response in
        defer { prismDiscardStaged(source) }
        guard response == .OK, let dest = panel.url else { return replyFiles(id, []) }
        do {
            // The panel already asked before replacing a file.
            if FileManager.default.fileExists(atPath: dest.path) {
                try FileManager.default.removeItem(at: dest)
            }
            try FileManager.default.copyItem(at: source, to: dest)
            replyFiles(id, [prismDescribe(dest.path, name: dest.lastPathComponent)])
        } catch {
            fail(id, error.localizedDescription)
        }
    }
    if let window = NSApp.keyWindow ?? NSApp.mainWindow {
        panel.beginSheetModal(for: window, completionHandler: finish)
    } else {
        panel.begin(completionHandler: finish)
    }
    #endif
}

/// The Finder, with the file selected (#210). Nothing elsewhere.
@_cdecl("prism_device_reveal")
public func prismDeviceReveal(_ path: UnsafePointer<CChar>) {
    #if os(macOS)
    NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: String(cString: path))])
    #else
    _ = path
    #endif
}

private func withOptional<R>(_ s: String?, _ body: (UnsafePointer<CChar>?) -> R) -> R {
    guard let s else { return body(nil) }
    return s.withCString { body($0) }
}
