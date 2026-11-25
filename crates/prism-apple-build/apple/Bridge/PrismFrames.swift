// Frame callbacks for the Rust app (#311, prism_core::frames): a display
// link runs while the app wants frames, and each refresh calls
// `prism_frame` (implemented by the backend) with how long ago the frame
// began and how long until it reaches the screen, in seconds.
//
// - iOS, iPadOS, tvOS, visionOS: CADisplayLink.
// - macOS 14+: the window's CADisplayLink (it follows the window's screen);
//   macOS 13: CVDisplayLink, whose callbacks run on its own thread.
// - watchOS has no display link: a timer in Rust stands in.
#if os(iOS) || os(tvOS) || os(visionOS) || os(macOS)
import Foundation
import QuartzCore
#if os(macOS)
import AppKit
import CoreVideo
#endif

@_silgen_name("prism_frame")
func prismFrame(_ ago: Double, _ ahead: Double)

final class PrismFrames: NSObject {
    static let shared = PrismFrames()
    private var link: CADisplayLink?
    #if os(macOS)
    private var cvLink: CVDisplayLink?
    /// A CVDisplayLink frame is queued on the main thread: later ones are
    /// dropped until it ran (a slow frame must not pile them up).
    private let queued = NSLock()
    private var pending = false
    #endif

    @objc func tick(_ link: CADisplayLink) {
        let now = CACurrentMediaTime()
        prismFrame(max(0, now - link.timestamp), max(0, link.targetTimestamp - now))
    }

    func start() {
        #if os(macOS)
        guard link == nil, cvLink == nil else { return }
        if #available(macOS 14.0, *) {
            let view = (NSApp.keyWindow ?? NSApp.windows.first)?.contentView
            let l = view.map { $0.displayLink(target: self, selector: #selector(tick(_:))) }
                ?? NSScreen.main?.displayLink(target: self, selector: #selector(tick(_:)))
            if let l {
                l.add(to: .main, forMode: .common)
                link = l
                return
            }
        }
        var cv: CVDisplayLink?
        guard CVDisplayLinkCreateWithActiveCGDisplays(&cv) == kCVReturnSuccess, let cv else { return }
        CVDisplayLinkSetOutputHandler(cv) { [weak self] _, now, output, _, _ in
            guard let self else { return kCVReturnSuccess }
            self.queued.lock()
            let skip = self.pending
            self.pending = true
            self.queued.unlock()
            if skip { return kCVReturnSuccess }
            let began = now.pointee.hostTime, shown = output.pointee.hostTime
            DispatchQueue.main.async {
                self.queued.lock()
                self.pending = false
                self.queued.unlock()
                guard self.cvLink != nil else { return }
                let t = mach_absolute_time()
                prismFrame(hostSeconds(t &- min(began, t)), hostSeconds(max(shown, t) &- t))
            }
            return kCVReturnSuccess
        }
        CVDisplayLinkStart(cv)
        cvLink = cv
        #else
        guard link == nil else { return }
        let l = CADisplayLink(target: self, selector: #selector(tick(_:)))
        l.add(to: .main, forMode: .common)
        link = l
        #endif
    }

    func stop() {
        link?.invalidate()
        link = nil
        #if os(macOS)
        if let cv = cvLink {
            CVDisplayLinkStop(cv)
            cvLink = nil
        }
        #endif
    }
}

#if os(macOS)
private let timebase: mach_timebase_info_data_t = {
    var info = mach_timebase_info_data_t()
    mach_timebase_info(&info)
    return info
}()

/// Mach host time units to seconds.
private func hostSeconds(_ t: UInt64) -> Double {
    Double(t) * Double(timebase.numer) / Double(timebase.denom) / 1e9
}
#endif

@_cdecl("prism_frames")
public func prismFrames(_ on: Bool) {
    if on { PrismFrames.shared.start() } else { PrismFrames.shared.stop() }
}
#endif
