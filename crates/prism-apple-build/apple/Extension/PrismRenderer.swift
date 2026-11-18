// Renders Prism static snapshots (compact JSON from prism-core `SNode`) with
// SwiftUI. Used by widgets and Live Activities on iOS and macOS. Mirrors the
// web renderer (`snode()` in client.js) and prism-native's flexbox mapping.
import AppIntents
import SwiftUI
import WidgetKit
#if canImport(UIKit)
import UIKit
#else
import AppKit
#endif

typealias PNode = [String: Any]

func prismParse(_ json: String) -> PNode? {
    guard let data = json.data(using: .utf8) else { return nil }
    return (try? JSONSerialization.jsonObject(with: data)) as? PNode
}

/// The app's deep-link URL for a path (`prism-counter:/todos/1`).
func prismURL(_ path: Any?) -> URL? {
    guard let path = path as? String,
          let scheme = Bundle.main.object(forInfoDictionaryKey: "PrismURLScheme") as? String
    else { return nil }
    return URL(string: "\(scheme):\(path)")
}

private func rgba(_ hex: String) -> (Double, Double, Double, Double) {
    var s = hex.trimmingCharacters(in: CharacterSet(charactersIn: "#"))
    if s.count == 6 { s += "FF" }
    let v = UInt64(s, radix: 16) ?? 0
    return (Double((v >> 24) & 0xFF) / 255, Double((v >> 16) & 0xFF) / 255,
            Double((v >> 8) & 0xFF) / 255, Double(v & 0xFF) / 255)
}

/// `"#RRGGBBAA"`, or `["#light", "#dark"]` resolved by the system appearance.
func prismColor(_ value: Any?) -> Color? {
    if let s = value as? String {
        let c = rgba(s)
        return Color(.sRGB, red: c.0, green: c.1, blue: c.2, opacity: c.3)
    }
    guard let pair = value as? [String], pair.count == 2 else { return nil }
    let l = rgba(pair[0]), d = rgba(pair[1])
    #if canImport(UIKit)
    return Color(UIColor { traits in
        let c = traits.userInterfaceStyle == .dark ? d : l
        return UIColor(red: c.0, green: c.1, blue: c.2, alpha: c.3)
    })
    #else
    return Color(NSColor(name: nil) { appearance in
        let dark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        let c = dark ? d : l
        return NSColor(srgbRed: c.0, green: c.1, blue: c.2, alpha: c.3)
    })
    #endif
}

private func num(_ v: Any?) -> Double? { (v as? NSNumber)?.doubleValue }

/// `fw`: 1-3 medium, semibold, bold; larger values are CSS weights (100-900),
/// rounded to SwiftUI's nine weights (650 draws as bold).
private func weight(_ n: PNode) -> Font.Weight {
    let fw = Int(num(n["fw"]) ?? 0)
    switch fw {
    case 1: return .medium
    case 2: return .semibold
    case 3: return .bold
    case 100...900:
        let steps: [Font.Weight] = [.ultraLight, .thin, .light, .regular, .medium, .semibold, .bold, .heavy, .black]
        return steps[min(max((fw + 50) / 100 - 1, 0), 8)]
    default: return .regular
    }
}

private func font(_ n: PNode) -> Font {
    let size = num(n["fs"]) ?? 16
    var f: Font
    switch n["ff"] as? String {
    case "mono": f = .system(size: size, weight: weight(n), design: .monospaced)
    case "serif": f = .system(size: size, weight: weight(n), design: .serif)
    case "rounded": f = .system(size: size, weight: weight(n), design: .rounded)
    case let name? where !name.isEmpty && name != "system": f = .custom(name, fixedSize: size).weight(weight(n))
    default: f = .system(size: size, weight: weight(n))
    }
    if num(n["it"]) == 1 { f = f.italic() }
    if num(n["tn"]) == 1 { f = f.monospacedDigit() }
    return f
}

/// The text as drawn: `tt` u/l/c (upper, lower, capitalized words).
private func transformed(_ s: String, _ n: PNode) -> String {
    switch n["tt"] as? String {
    case "u": return s.uppercased()
    case "l": return s.lowercased()
    case "c":
        var out = "", start = true
        for c in s {
            out += start && (c.isLetter || c.isNumber) ? c.uppercased() : String(c)
            start = !(c.isLetter || c.isNumber) && c != "'" && c != "\u{2019}"
        }
        return out
    default: return s
    }
}

/// A text's or a timer's `ta`: its lines' alignment.
private func prismTextAlign(_ n: PNode) -> TextAlignment {
    (n["ta"] as? String) == "c" ? .center : (n["ta"] as? String) == "r" ? .trailing : .leading
}

/// Where a node sits in a frame wider than itself: a text or a timer follows
/// its `ta`, so a centered one stays centered when stretched, filling or cut
/// (#279); anything else sits leading.
private func prismFrameAlign(_ n: PNode) -> Alignment {
    let kind = n["k"] as? String
    guard kind == "t" || kind == "m" else { return .leading }
    let ta = n["ta"] as? String
    return ta == "c" ? .center : ta == "r" ? .trailing : .leading
}

/// A text node: typography keys `ff` `it` `ul` `st` `tn` `tt` `ls` `lh` `ml`
/// `nw` (see prism-core `write_typography`).
private func prismText(_ n: PNode, _ fg: Color?) -> AnyView {
    let align = prismTextAlign(n)
    var t = Text(transformed(n["s"] as? String ?? "", n)).font(font(n))
    if let ls = num(n["ls"]) { t = t.tracking(ls) }
    if num(n["ul"]) == 1 { t = t.underline() }
    if num(n["st"]) == 1 { t = t.strikethrough() }
    let size = num(n["fs"]) ?? 16
    // Line height: the extra space between lines (SwiftUI cannot tighten).
    let extra = num(n["lh"]).map { max($0 - size * 1.2, 0) } ?? 0
    var v = AnyView(t.foregroundStyle(fg ?? .primary).multilineTextAlignment(align).lineSpacing(extra))
    if let ml = num(n["ml"]), ml > 0 {
        v = AnyView(v.lineLimit(Int(ml)).truncationMode(.tail))
    } else if num(n["nw"]) == 1 {
        v = AnyView(v.fixedSize(horizontal: true, vertical: false).frame(maxWidth: .infinity, alignment: prismFrameAlign(n)).clipped())
    }
    return v
}

/// The instant a widget entry is drawn for (`PrismEntry.date`): WidgetKit
/// draws the entries of a timeline ahead of time, so a timer compares its
/// end with it, not with the moment it is drawn. Nil elsewhere (Live
/// Activities): now.
struct PrismEntryDateKey: EnvironmentKey {
    static let defaultValue: Date? = nil
}

extension EnvironmentValues {
    var prismEntryDate: Date? {
        get { self[PrismEntryDateKey.self] }
        set { self[PrismEntryDateKey.self] = newValue }
    }
}

/// A countdown: the time left until `end`, ticking by itself. Once ended,
/// its `after` text (`timer(..).after`, #281), else 0:00. A widget switches
/// to it with a timeline entry at `end` (see `prismTimerEnds`).
struct PrismTimerText: View {
    let end: Date
    let after: String?
    @Environment(\.prismEntryDate) private var entryDate

    var body: some View {
        let now = entryDate ?? .now
        if end > now {
            Text(timerInterval: now...end, countsDown: true)
        } else {
            Text(after.flatMap { $0.isEmpty ? nil : $0 } ?? "0:00")
        }
    }
}

/// Renders a node, or nothing.
func prismView(_ node: Any?) -> AnyView {
    guard let n = node as? PNode else { return AnyView(EmptyView()) }
    return prismBuild(n)
}

func prismBuild(_ node: PNode) -> AnyView {
    let n = prismLinkAroundActions(node)
    let kind = n["k"] as? String ?? "v"
    let fg = prismColor(n["fg"])
    var view: AnyView
    switch kind {
    case "t":
        view = prismText(n, fg)
    case "i":
        view = AnyView(Image(systemName: n["s"] as? String ?? "questionmark")
            .font(.system(size: num(n["fs"]) ?? 16, weight: weight(n)))
            .foregroundStyle(fg ?? .primary))
    case "m":
        let end = Date(timeIntervalSince1970: num(n["v"]) ?? 0)
        // A timer takes the width it is offered and lays its digits out by
        // its own alignment: centering it from outside does nothing (#279).
        view = AnyView(PrismTimerText(end: end, after: n["ph"] as? String)
            .font(font(n)).monospacedDigit().foregroundStyle(fg ?? .primary)
            .multilineTextAlignment(prismTextAlign(n)))
    case "p":
        view = AnyView(ProgressView(value: min(max(num(n["v"]) ?? 0, 0), 1)).tint(fg))
    case "sh":
        view = AnyView(PrismShapeView(drawing: PrismDrawing(n["s"] as? String ?? "")))
    default:
        view = prismContainer(n)
    }
    return prismModifiers(view, n)
}

private func prismContainer(_ n: PNode) -> AnyView {
    let kids = (n["c"] as? [PNode]) ?? []
    let row = num(n["d"]) == 1
    let spacing = num(n["g"]) ?? 0
    let ai = n["ai"] as? String
    let jc = n["jc"] as? String
    var items: [AnyView] = kids.map { child in
        var v = prismBuild(child)
        // Flexbox `align-items: stretch` (the default) in a column.
        if !row && ai == nil {
            v = AnyView(v.frame(maxWidth: .infinity, alignment: prismFrameAlign(child)))
        }
        return v
    }
    // `justify-content` via spacers.
    let spacer = AnyView(Spacer(minLength: 0))
    switch jc {
    case "e": items.insert(spacer, at: 0)
    case "c": items.insert(spacer, at: 0); items.append(spacer)
    case "b", "a", "v":
        var spaced: [AnyView] = []
        if jc != "b" { spaced.append(spacer) }
        for (i, item) in items.enumerated() {
            if i > 0 { spaced.append(spacer) }
            spaced.append(item)
        }
        if jc != "b" { spaced.append(spacer) }
        items = spaced
    default: break
    }
    let content = ForEach(0..<items.count, id: \.self) { items[$0] }
    if row {
        let a: VerticalAlignment = ai == "s" ? .top : ai == "e" ? .bottom : ai == "b" ? .firstTextBaseline : .center
        return AnyView(HStack(alignment: a, spacing: spacing) { content })
    }
    let a: HorizontalAlignment = ai == "c" ? .center : ai == "e" ? .trailing : .leading
    return AnyView(VStack(alignment: a, spacing: spacing) { content })
}

private func prismModifiers(_ base: AnyView, _ n: PNode) -> AnyView {
    var v = base
    if let pd = n["pd"] as? [NSNumber], pd.count == 4 {
        v = AnyView(v.padding(EdgeInsets(top: pd[0].doubleValue, leading: pd[3].doubleValue,
                                         bottom: pd[2].doubleValue, trailing: pd[1].doubleValue)))
    }
    let fill = { (key: String) -> Bool in (n[key] as? String) == "f" }
    if let w = num(n["w"]) { v = AnyView(v.frame(width: w)) }
    if let h = num(n["h"]) { v = AnyView(v.frame(height: h)) }
    if fill("w") || (num(n["f"]) ?? 0) > 0 { v = AnyView(v.frame(maxWidth: .infinity, alignment: prismFrameAlign(n))) }
    if fill("h") { v = AnyView(v.frame(maxHeight: .infinity)) }
    let radius = num(n["r"]) ?? 0
    // Rounded corners: all, or the `rc` bits (1 top leading, 2 top trailing,
    // 4 bottom trailing, 8 bottom leading; SwiftUI mirrors them right to left).
    let rc = Int(num(n["rc"]) ?? 15)
    let at = { (bit: Int) -> Double in rc & bit != 0 ? radius : 0 }
    let box = UnevenRoundedRectangle(topLeadingRadius: at(1), bottomLeadingRadius: at(8),
                                     bottomTrailingRadius: at(4), topTrailingRadius: at(2), style: .continuous)
    if let bg = prismColor(n["bg"]) {
        v = AnyView(v.background(box.fill(bg)))
    }
    if let bw = num(n["bw"]), bw > 0 {
        let c = prismColor(n["bc"]) ?? .secondary
        v = AnyView(v.overlay(box.strokeBorder(c, lineWidth: bw)))
    }
    // Margins, outside the background. Negative ones pull the node over its
    // previous sibling (a chart's label over its ring).
    if let m = n["m"] as? [NSNumber], m.count == 4 {
        v = AnyView(v.padding(EdgeInsets(top: m[0].doubleValue, leading: m[3].doubleValue,
                                         bottom: m[2].doubleValue, trailing: m[1].doubleValue)))
    }
    if let o = num(n["o"]) { v = AnyView(v.opacity(o)) }
    if num(n["ah"]) == 1 {
        v = AnyView(v.accessibilityHidden(true))
    } else {
        if let label = n["al"] as? String { v = AnyView(v.accessibilityLabel(Text(label))) }
        if num(n["hd"]) == 1 { v = AnyView(v.accessibilityAddTraits(.isHeader)) }
    }
    if let action = n["wa"] as? [String: Any] {
        v = prismActionButton(v, action)
    } else if let url = prismURL(n["l"]) {
        v = AnyView(Link(destination: url) { v })
    }
    return v
}

private func prismHasAction(_ n: PNode) -> Bool {
    n["wa"] != nil || ((n["c"] as? [PNode]) ?? []).contains(where: prismHasAction)
}

/// A link (`l`) around widget buttons would take their taps (WidgetKit
/// gives a tap to the outer `Link`): the link moves down to the children
/// that are not buttons, so the buttons keep theirs and the rest still
/// opens the link.
private func prismLinkAroundActions(_ n: PNode) -> PNode {
    guard let link = n["l"], n["wa"] == nil, prismHasAction(n) else { return n }
    var out = n
    out["l"] = nil
    out["c"] = ((n["c"] as? [PNode]) ?? []).map { child -> PNode in
        guard child["wa"] == nil, child["l"] == nil else { return child }
        var child = child
        child["l"] = link
        return child
    }
    return out
}

/// A widget button (`wa`: `k` kind, `a` action, `p` payload, #47). On iOS
/// and visionOS a `Button(intent:)`: the tap runs the action in the app
/// without opening it (PrismWidgetAction.swift); the widget is redrawn when
/// the app's job returns. No `invalidatableContent()` on the label: with it
/// the iOS 27 simulator stopped treating the button as one (taps fell
/// through to the widget's URL). Elsewhere (macOS widgets run intents in the
/// extension, which cannot run the app's code) the tap opens the app on the
/// action's link, and the app runs it there.
private func prismActionButton(_ content: AnyView, _ action: [String: Any]) -> AnyView {
    let kind = action["k"] as? String ?? ""
    let name = action["a"] as? String ?? ""
    let payload = action["p"] as? String ?? ""
    #if os(iOS) || os(visionOS)
    let intent = PrismWidgetActionIntent(kind: kind, action: name, payload: payload)
    return AnyView(Button(intent: intent) { content }.buttonStyle(.plain))
    #else
    // prism-core `WidgetAction::to_link`.
    let safe = CharacterSet(charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")
    let q = { (s: String) in s.addingPercentEncoding(withAllowedCharacters: safe) ?? "" }
    let path = "/__prism/widget-action?kind=\(q(kind))&action=\(q(name))&payload=\(q(payload))"
    guard let url = prismURL(path) else { return content }
    return AnyView(Link(destination: url) { content })
    #endif
}

// --- Shapes and charts ----------------------------------------------------------------

/// A prism-core `Drawing` (text form, see `Drawing::encode` and
/// `compact_drawing`): an inset line `i<x>,<y>`, then one line per path:
/// paint tokens (`f`/`s` color, `w` width, `r` round, `d` dash), `|`, and
/// segments in fractions of the frame (`M x y`, `L x y`, `C …6`, `A cx cy r a0 a1`
/// in degrees clockwise from 12 o'clock, `Z`).
struct PrismDrawing {
    enum Segment {
        case move(Double, Double), line(Double, Double)
        case cubic(Double, Double, Double, Double, Double, Double)
        case arc(Double, Double, Double, Double, Double)
        case close
    }
    struct Layer {
        var segments: [Segment] = []
        var fill: String?
        var stroke: String?
        var width = 1.0
        var round = false
        var dash: [Double] = []
        /// Even-odd fill rule (`e`): overlapping subpaths cut holes.
        var evenOdd = false
    }
    var inset = (0.0, 0.0)
    var layers: [Layer] = []

    init(_ text: String) {
        for line in text.split(separator: "\n") {
            if line.hasPrefix("i") {
                let v = line.dropFirst().split(separator: ",").compactMap { Double($0) }
                inset = (v.first ?? 0, v.count > 1 ? v[1] : 0)
                continue
            }
            guard let bar = line.firstIndex(of: "|") else { continue }
            var layer = Layer()
            for token in line[..<bar].split(separator: " ") {
                let value = String(token.dropFirst())
                switch token.first {
                case "f": layer.fill = value
                case "s": layer.stroke = value
                case "w": layer.width = Double(value) ?? 1
                case "r": layer.round = true
                case "e": layer.evenOdd = true
                case "d": layer.dash = value.split(separator: ",").compactMap { Double($0) }
                default: break
                }
            }
            let t = line[line.index(after: bar)...].split(separator: " ")
            var i = 0
            func take(_ n: Int) -> [Double]? {
                guard i + n <= t.count else { return nil }
                let v = t[i..<i + n].compactMap { Double($0) }
                i += n
                return v.count == n ? v : nil
            }
            parse: while i < t.count {
                let cmd = t[i]
                i += 1
                switch cmd {
                case "M": guard let v = take(2) else { break parse }; layer.segments.append(.move(v[0], v[1]))
                case "L": guard let v = take(2) else { break parse }; layer.segments.append(.line(v[0], v[1]))
                case "C":
                    guard let v = take(6) else { break parse }
                    layer.segments.append(.cubic(v[0], v[1], v[2], v[3], v[4], v[5]))
                case "A":
                    guard let v = take(5) else { break parse }
                    layer.segments.append(.arc(v[0], v[1], v[2], v[3], v[4]))
                case "Z": layer.segments.append(.close)
                default: break parse
                }
            }
            layers.append(layer)
        }
    }
}

/// One path of a drawing, resolved for the frame SwiftUI gives it (like
/// prism-core `Drawing::resolve`): fractions become points inside the
/// inset, radii are fractions of the smaller side, arcs become curves.
struct PrismPathShape: Shape {
    let segments: [PrismDrawing.Segment]
    let inset: (Double, Double)
    /// Right to left: flipped horizontally, like the layout around it.
    let mirrored: Bool

    func path(in rect: CGRect) -> Path {
        let (ix, iy) = inset
        let iw = max(0, rect.width - 2 * ix), ih = max(0, rect.height - 2 * iy)
        let side = min(iw, ih)
        func pt(_ x: Double, _ y: Double) -> CGPoint {
            CGPoint(x: rect.minX + ix + x * iw, y: rect.minY + iy + y * ih)
        }
        var p = Path()
        var open = false
        for s in segments {
            switch s {
            case let .move(x, y):
                p.move(to: pt(x, y))
                open = true
            case let .line(x, y):
                if open { p.addLine(to: pt(x, y)) } else { p.move(to: pt(x, y)) }
                open = true
            case let .cubic(a, b, c, d, e, f):
                if !open { p.move(to: pt(a, b)) }
                p.addCurve(to: pt(e, f), control1: pt(a, b), control2: pt(c, d))
                open = true
            case let .arc(cx, cy, r, start, end):
                let c = pt(cx, cy), radius = r * side
                func at(_ deg: Double) -> CGPoint {
                    let a = deg * .pi / 180
                    return CGPoint(x: c.x + radius * sin(a), y: c.y - radius * cos(a))
                }
                if open { p.addLine(to: at(start)) } else { p.move(to: at(start)) }
                open = true
                // Cubic Béziers of at most 90° each (prism-core `arc`).
                let sweep = min(max(end - start, -360), 360)
                let n = max(1, Int((abs(sweep) / 90).rounded(.up)))
                let step = sweep / Double(n)
                let k = 4 / 3 * tan(step * .pi / 180 / 4) * radius
                for j in 0..<n {
                    let a0 = start + step * Double(j), a1 = a0 + step
                    let p0 = at(a0), p1 = at(a1)
                    let r0 = a0 * .pi / 180, r1 = a1 * .pi / 180
                    p.addCurve(to: p1,
                               control1: CGPoint(x: p0.x + k * cos(r0), y: p0.y + k * sin(r0)),
                               control2: CGPoint(x: p1.x - k * cos(r1), y: p1.y - k * sin(r1)))
                }
            case .close:
                p.closeSubpath()
                open = false
            }
        }
        if mirrored {
            p = p.applying(CGAffineTransform(a: -1, b: 0, c: 0, d: 1, tx: 2 * rect.minX + rect.width, ty: 0))
        }
        return p
    }
}

/// Draws a drawing at the node's frame. Colors follow light/dark mode. On
/// the lock screen (vibrant) and in tinted widgets (accented), colors become
/// white with their opacity: colored paths (series) stay prominent and take
/// the accent, neutral ones (grid lines, tracks) recede.
struct PrismShapeView: View {
    let drawing: PrismDrawing
    @Environment(\.layoutDirection) private var direction
    @Environment(\.widgetRenderingMode) private var mode

    var body: some View {
        ZStack {
            ForEach(0..<drawing.layers.count, id: \.self) { i in
                layer(drawing.layers[i])
            }
        }
    }

    private func layer(_ l: PrismDrawing.Layer) -> AnyView {
        let shape = PrismPathShape(segments: l.segments, inset: drawing.inset,
                                   mirrored: direction == .rightToLeft)
        let style = StrokeStyle(lineWidth: l.width,
                                lineCap: l.round ? .round : .butt,
                                lineJoin: l.round ? .round : .miter,
                                dash: l.dash.count == 2 ? l.dash.map { CGFloat($0) } : [])
        let fill: AnyView? = paint(l.fill).map { p in
            AnyView(shape.fill(p.color, style: FillStyle(eoFill: l.evenOdd)).widgetAccentable(p.accent))
        }
        let stroke: AnyView? = paint(l.stroke).map { p in
            AnyView(shape.stroke(p.color, style: style).widgetAccentable(p.accent))
        }
        return AnyView(ZStack {
            if let fill { fill }
            if let stroke { stroke }
        })
    }

    /// `#RRGGBB[AA]` or `#light/#dark`, for the current rendering mode.
    private func paint(_ spec: String?) -> (color: Color, accent: Bool)? {
        guard let spec else { return nil }
        let parts = spec.split(separator: "/").map(String.init)
        guard let first = parts.first else { return nil }
        if mode == .fullColor {
            guard let c = prismColor(parts.count == 2 ? parts as Any : first as Any) else { return nil }
            return (c, false)
        }
        let (r, g, b, a) = rgba(first)
        let colored = max(r, g, b) - min(r, g, b) > 0.15
        return (Color.white.opacity(a * (colored ? 1 : 0.35)), colored)
    }
}
