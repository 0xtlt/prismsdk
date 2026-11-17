// Videos (#207): an AVPlayer in a host view, with the platform's controls:
// `AVPlayerView` on macOS, `AVPlayerViewController`'s view on UIKit (iOS,
// iPadOS, tvOS, visionOS). Headers go with every request of the asset
// (`AVURLAssetHTTPHeaderFieldsKey`). watchOS: `prism_video_new` returns nil.
import Foundation

/// Implemented in Rust (prism-macos, prism-ios): kind 0 the video's size is
/// known (lay out again), kind 1 the playback reached the end.
@_silgen_name("prism_video_event")
func prismRustVideoEvent(_ id: UInt64, _ kind: Int32)

#if canImport(AVKit) && !os(watchOS)
import AVFoundation
import AVKit
#if os(macOS)
import AppKit
typealias PrismVideoBase = NSView
#else
import UIKit
typealias PrismVideoBase = UIView
#endif

final class PrismVideoHost: PrismVideoBase {
    let nodeId: UInt64
    let player = AVPlayer()
    private var source = ""
    private var looping = false
    private var autoplay = false
    private var sizeObserver: NSKeyValueObservation?
    private var endObserver: NSObjectProtocol?
    private(set) var naturalSize = CGSize.zero
    #if os(macOS)
    private let playerView = AVPlayerView()
    #else
    private let controller = AVPlayerViewController()
    #endif

    init(id: UInt64) {
        nodeId = id
        super.init(frame: .zero)
        // The host fills Prism's container, which layout resizes.
        #if os(macOS)
        autoresizingMask = [.width, .height]
        #else
        autoresizingMask = [.flexibleWidth, .flexibleHeight]
        #endif
        #if os(macOS)
        playerView.player = player
        playerView.controlsStyle = .inline
        playerView.frame = bounds
        playerView.autoresizingMask = [.width, .height]
        addSubview(playerView)
        #else
        controller.player = player
        controller.view.frame = bounds
        controller.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        addSubview(controller.view)
        #endif
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    #if !os(macOS)
    /// The player's controller joins the nearest controller (full screen and
    /// picture in picture need it).
    override func didMoveToWindow() {
        super.didMoveToWindow()
        guard window != nil, controller.parent == nil else { return }
        var r: UIResponder? = self
        while let next = r?.next {
            if let vc = next as? UIViewController {
                vc.addChild(controller)
                controller.didMove(toParent: vc)
                return
            }
            r = next
        }
    }
    #endif

    /// `src`: the URL, then one `name: value` header per line.
    func load(_ src: String) {
        guard src != source else { return }
        source = src
        let lines = src.components(separatedBy: "\n")
        guard let first = lines.first, let url = URL(string: first) ?? URL(fileURLWithPath: first) as URL? else { return }
        var headers: [String: String] = [:]
        for line in lines.dropFirst() {
            guard let colon = line.firstIndex(of: ":") else { continue }
            let name = line[..<colon].trimmingCharacters(in: .whitespaces)
            let value = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
            if !name.isEmpty { headers[name] = value }
        }
        let options: [String: Any] = headers.isEmpty ? [:] : ["AVURLAssetHTTPHeaderFieldsKey": headers]
        let item = AVPlayerItem(asset: AVURLAsset(url: url, options: options))
        naturalSize = .zero
        sizeObserver = item.observe(\.presentationSize, options: [.new]) { [weak self] item, _ in
            guard let self = self else { return }
            let size = item.presentationSize
            if size.width > 0, size.height > 0, size != self.naturalSize {
                self.naturalSize = size
                prismRustVideoEvent(self.nodeId, 0)
            }
        }
        if let old = endObserver { NotificationCenter.default.removeObserver(old) }
        endObserver = NotificationCenter.default.addObserver(
            forName: .AVPlayerItemDidPlayToEndTime, object: item, queue: .main
        ) { [weak self] _ in
            guard let self = self else { return }
            if self.looping {
                self.player.seek(to: .zero)
                self.player.play()
            } else {
                prismRustVideoEvent(self.nodeId, 1)
            }
        }
        player.replaceCurrentItem(with: item)
        if autoplay { player.play() }
    }

    func configure(controls: Bool, autoplay: Bool, looping: Bool, muted: Bool) {
        self.looping = looping
        player.isMuted = muted
        #if os(macOS)
        playerView.controlsStyle = controls ? .inline : .none
        #else
        controller.showsPlaybackControls = controls
        #endif
        if autoplay && !self.autoplay && player.currentItem != nil { player.play() }
        self.autoplay = autoplay
    }

    func teardown() {
        player.pause()
        player.replaceCurrentItem(with: nil)
        sizeObserver = nil
        if let old = endObserver { NotificationCenter.default.removeObserver(old) }
        endObserver = nil
        #if !os(macOS)
        controller.willMove(toParent: nil)
        controller.removeFromParent()
        #endif
    }
}

private func prismVideo(_ p: UnsafeMutableRawPointer) -> PrismVideoHost {
    Unmanaged<PrismVideoHost>.fromOpaque(p).takeUnretainedValue()
}

/// A new video's host view for node `id`, retained (+1): the caller
/// releases it (`prism_video_release`).
@_cdecl("prism_video_new")
public func prismVideoNew(_ id: UInt64) -> UnsafeMutableRawPointer? {
    Unmanaged.passRetained(PrismVideoHost(id: id)).toOpaque()
}

@_cdecl("prism_video_load")
public func prismVideoLoad(_ p: UnsafeMutableRawPointer, _ src: UnsafePointer<CChar>) {
    prismVideo(p).load(String(cString: src))
}

@_cdecl("prism_video_config")
public func prismVideoConfig(_ p: UnsafeMutableRawPointer, _ controls: Bool, _ autoplay: Bool, _ looping: Bool, _ muted: Bool) {
    prismVideo(p).configure(controls: controls, autoplay: autoplay, looping: looping, muted: muted)
}

/// The video's size in points, once known (false before).
@_cdecl("prism_video_size")
public func prismVideoSize(_ p: UnsafeMutableRawPointer, _ w: UnsafeMutablePointer<Double>, _ h: UnsafeMutablePointer<Double>) -> Bool {
    let s = prismVideo(p).naturalSize
    guard s.width > 0, s.height > 0 else { return false }
    w.pointee = Double(s.width)
    h.pointee = Double(s.height)
    return true
}

/// Stops the playback and releases the host view.
@_cdecl("prism_video_release")
public func prismVideoRelease(_ p: UnsafeMutableRawPointer) {
    let host = Unmanaged<PrismVideoHost>.fromOpaque(p)
    host.takeUnretainedValue().teardown()
    host.release()
}

#elseif !os(watchOS)
@_cdecl("prism_video_new")
public func prismVideoNew(_ id: UInt64) -> UnsafeMutableRawPointer? { nil }
@_cdecl("prism_video_load")
public func prismVideoLoad(_ p: UnsafeMutableRawPointer, _ src: UnsafePointer<CChar>) {}
@_cdecl("prism_video_config")
public func prismVideoConfig(_ p: UnsafeMutableRawPointer, _ controls: Bool, _ autoplay: Bool, _ looping: Bool, _ muted: Bool) {}
@_cdecl("prism_video_size")
public func prismVideoSize(_ p: UnsafeMutableRawPointer, _ w: UnsafeMutablePointer<Double>, _ h: UnsafeMutablePointer<Double>) -> Bool { false }
@_cdecl("prism_video_release")
public func prismVideoRelease(_ p: UnsafeMutableRawPointer) {}
#endif
