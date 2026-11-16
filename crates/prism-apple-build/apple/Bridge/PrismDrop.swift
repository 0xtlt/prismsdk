// Drop targets (#111): files dragged from the system onto a Prism view.
//
// macOS: the backend's views implement NSDraggingDestination and ask here
// whether the drag holds files they accept (`prism_drop_accepts`), then for
// the dropped files (`prism_drop_perform`). iOS, iPadOS, visionOS: a
// UIDropInteraction on the view (`prism_drop_target`). Either way the files
// are copied where the app can read them (like picked files) and reported
// with `prism_drop_event`. tvOS and watchOS have no drag and drop.
import Foundation
import UniformTypeIdentifiers

#if os(macOS) || os(iOS) || os(visionOS)
/// Implemented in Rust (prism-macos, prism-ios): kind 0 / 1, accepted files
/// left / came over the node; kind 2, files dropped (`path\tname\tmime\tsize`
/// per line).
@_silgen_name("prism_drop_event")
func prismRustDropEvent(_ node: UInt64, _ kind: Int32, _ files: UnsafePointer<CChar>?)

/// The type of a file at `url` (`.data` when its extension says nothing).
private func prismFileType(_ url: URL) -> UTType {
    UTType(filenameExtension: url.pathExtension) ?? .data
}

/// Reports the copies of the dropped files, on the main thread.
private func prismReportDrop(_ node: UInt64, _ lines: [String]) {
    DispatchQueue.main.async {
        if lines.isEmpty { return }
        lines.joined(separator: "\n").withCString { prismRustDropEvent(node, 2, $0) }
    }
}

#if os(macOS)
import AppKit

/// The dragged file URLs `types` accept (directories left out).
private func prismDroppedURLs(_ info: NSDraggingInfo, _ types: String) -> [URL] {
    let accepted = prismContentTypes(types)
    let urls = info.draggingPasteboard.readObjects(
        forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
    return urls.filter { url in
        !url.hasDirectoryPath && accepted.contains { prismFileType(url).conforms(to: $0) }
    }
}

@_cdecl("prism_drop_accepts")
public func prismDropAccepts(_ info: UnsafeRawPointer, _ types: UnsafePointer<CChar>) -> Bool {
    guard let info = Unmanaged<AnyObject>.fromOpaque(info).takeUnretainedValue() as? NSDraggingInfo else { return false }
    return !prismDroppedURLs(info, String(cString: types)).isEmpty
}

@_cdecl("prism_drop_perform")
public func prismDropPerform(_ node: UInt64, _ info: UnsafeRawPointer, _ types: UnsafePointer<CChar>) -> Bool {
    guard let info = Unmanaged<AnyObject>.fromOpaque(info).takeUnretainedValue() as? NSDraggingInfo else { return false }
    let urls = prismDroppedURLs(info, String(cString: types))
    if urls.isEmpty { return false }
    // Copying large files takes a while: not on the main thread.
    DispatchQueue.global(qos: .userInitiated).async {
        prismReportDrop(node, urls.compactMap { url in
            prismKeep(url).map { prismDescribe($0, name: url.lastPathComponent) }
        })
    }
    return true
}

#else
import UIKit

private final class PrismDropDelegate: NSObject, UIDropInteractionDelegate {
    let node: UInt64
    var types: [UTType]
    private var over = false

    init(node: UInt64, types: [UTType]) {
        self.node = node
        self.types = types
    }

    /// The first type of `provider` this target accepts.
    private func accepted(_ provider: NSItemProvider) -> String? {
        provider.registeredTypeIdentifiers.first { id in
            guard let t = UTType(id) else { return false }
            return types.contains { t.conforms(to: $0) }
        }
    }

    private func hover(_ on: Bool) {
        guard on != over else { return }
        over = on
        prismRustDropEvent(node, on ? 1 : 0, nil)
    }

    func dropInteraction(_ interaction: UIDropInteraction, canHandle session: UIDropSession) -> Bool {
        session.items.contains { accepted($0.itemProvider) != nil }
    }

    func dropInteraction(_ interaction: UIDropInteraction, sessionDidEnter session: UIDropSession) {
        hover(true)
    }

    func dropInteraction(_ interaction: UIDropInteraction, sessionDidUpdate session: UIDropSession) -> UIDropProposal {
        UIDropProposal(operation: .copy)
    }

    func dropInteraction(_ interaction: UIDropInteraction, sessionDidExit session: UIDropSession) {
        hover(false)
    }

    func dropInteraction(_ interaction: UIDropInteraction, sessionDidEnd session: UIDropSession) {
        hover(false)
    }

    func dropInteraction(_ interaction: UIDropInteraction, performDrop session: UIDropSession) {
        hover(false)
        let group = DispatchGroup()
        let lock = NSLock()
        var lines: [String] = []
        for item in session.items {
            let provider = item.itemProvider
            guard let type = accepted(provider) else { continue }
            group.enter()
            // The file is deleted when the handler returns: copy it there.
            provider.loadFileRepresentation(forTypeIdentifier: type) { url, _ in
                defer { group.leave() }
                guard let url else { return }
                let ext = url.pathExtension.isEmpty ? UTType(type)?.preferredFilenameExtension : nil
                if let path = prismKeep(url, ext: ext) {
                    let name = provider.suggestedName.map { n in
                        ext.map { "\(n).\($0)" } ?? (n.contains(".") ? n : url.lastPathComponent)
                    } ?? url.lastPathComponent
                    lock.lock(); lines.append(prismDescribe(path, name: name)); lock.unlock()
                }
            }
        }
        group.notify(queue: .global(qos: .userInitiated)) { prismReportDrop(self.node, lines) }
    }
}

private var prismDropKey: UInt8 = 0

/// Makes `view` a drop target of `types` (one per line) for `node`, or not
/// (`types` null). The interaction and its delegate live with the view.
@_cdecl("prism_drop_target")
public func prismDropTarget(_ view: UnsafeRawPointer, _ node: UInt64, _ types: UnsafePointer<CChar>?) {
    let view = Unmanaged<UIView>.fromOpaque(view).takeUnretainedValue()
    let current = objc_getAssociatedObject(view, &prismDropKey) as? PrismDropDelegate
    guard let types else {
        if current != nil {
            for i in view.interactions where i is UIDropInteraction { view.removeInteraction(i) }
            objc_setAssociatedObject(view, &prismDropKey, nil, .OBJC_ASSOCIATION_RETAIN_NONATOMIC)
        }
        return
    }
    let accepted = prismContentTypes(String(cString: types))
    if let current {
        current.types = accepted
        return
    }
    let delegate = PrismDropDelegate(node: node, types: accepted)
    objc_setAssociatedObject(view, &prismDropKey, delegate, .OBJC_ASSOCIATION_RETAIN_NONATOMIC)
    view.addInteraction(UIDropInteraction(delegate: delegate))
}
#endif
#endif
