// Renders Prism static snapshots (compact JSON from prism-core `SNode`) with
// SwiftUI. Used by widgets and Live Activities on iOS and macOS. Mirrors the
// web renderer (`snode()` in client.js) and prism-native's flexbox mapping.
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

private func weight(_ n: PNode) -> Font.Weight {
    switch num(n["fw"]) ?? 0 {
    case 1: return .medium
    case 2: return .semibold
    case 3: return .bold
    default: return .regular
    }
}

private func font(_ n: PNode) -> Font {
    .system(size: num(n["fs"]) ?? 16, weight: weight(n))
}

/// Renders a node, or nothing.
func prismView(_ node: Any?) -> AnyView {
    guard let n = node as? PNode else { return AnyView(EmptyView()) }
    return prismBuild(n)
}

func prismBuild(_ n: PNode) -> AnyView {
    let kind = n["k"] as? String ?? "v"
    let fg = prismColor(n["fg"])
    var view: AnyView
    switch kind {
    case "t":
        let align: TextAlignment = (n["ta"] as? String) == "c" ? .center : (n["ta"] as? String) == "r" ? .trailing : .leading
        view = AnyView(Text(n["s"] as? String ?? "").font(font(n)).foregroundStyle(fg ?? .primary)
            .multilineTextAlignment(align))
    case "i":
        view = AnyView(Image(systemName: n["s"] as? String ?? "questionmark")
            .font(.system(size: num(n["fs"]) ?? 16, weight: weight(n)))
            .foregroundStyle(fg ?? .primary))
    case "m":
        let end = Date(timeIntervalSince1970: num(n["v"]) ?? 0)
        let text = end > .now
            ? Text(timerInterval: Date.now...end, countsDown: true)
            : Text("0:00")
        view = AnyView(text.font(font(n)).monospacedDigit().foregroundStyle(fg ?? .primary))
    case "p":
        view = AnyView(ProgressView(value: min(max(num(n["v"]) ?? 0, 0), 1)).tint(fg))
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
            v = AnyView(v.frame(maxWidth: .infinity, alignment: .leading))
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
        let a: VerticalAlignment = ai == "s" ? .top : ai == "e" ? .bottom : .center
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
    if fill("w") || (num(n["f"]) ?? 0) > 0 { v = AnyView(v.frame(maxWidth: .infinity, alignment: .leading)) }
    if fill("h") { v = AnyView(v.frame(maxHeight: .infinity)) }
    let radius = num(n["r"]) ?? 0
    if let bg = prismColor(n["bg"]) {
        v = AnyView(v.background(RoundedRectangle(cornerRadius: radius, style: .continuous).fill(bg)))
    }
    if let bw = num(n["bw"]), bw > 0 {
        let c = prismColor(n["bc"]) ?? .secondary
        v = AnyView(v.overlay(RoundedRectangle(cornerRadius: radius, style: .continuous).strokeBorder(c, lineWidth: bw)))
    }
    if let o = num(n["o"]) { v = AnyView(v.opacity(o)) }
    if num(n["ah"]) == 1 {
        v = AnyView(v.accessibilityHidden(true))
    } else {
        if let label = n["al"] as? String { v = AnyView(v.accessibilityLabel(Text(label))) }
        if num(n["hd"]) == 1 { v = AnyView(v.accessibilityAddTraits(.isHeader)) }
    }
    if let url = prismURL(n["l"]) { v = AnyView(Link(destination: url) { v }) }
    return v
}
