// watchOS host (compiled with apple/Shared and Bridge: device
// APIs and notifications come from there). watchOS apps have no UIKit views (UIView.h:
// API_UNAVAILABLE(watchos)), so Prism's tree is drawn with SwiftUI.
//
// Rust runs the app and sends patches as JSON (`prism_watch_apply`); this
// file keeps the node tree and renders it. Events go back as JSON
// (`prism_watch_event`). Layout is SwiftUI's: rows/columns, padding, sizes,
// grow, backgrounds and borders map to stacks and modifiers.
import Accessibility
import CoreText
import PhotosUI
import SceneKit
import SwiftUI
import UniformTypeIdentifiers
import WatchKit

// MARK: Rust side

@_silgen_name("prism_watch_ready")
func prismReady(_ width: Double, _ height: Double, _ scale: Double, _ zoom: Double, _ textScale: Double)
@_silgen_name("prism_watch_event")
func prismEvent(_ json: UnsafePointer<CChar>)

func send(_ event: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: event),
          let text = String(data: data, encoding: .utf8) else { return }
    text.withCString { prismEvent($0) }
}

// MARK: Measuring (called by Rust's layout)

private func uiWeight(_ w: Int32) -> UIFont.Weight {
    w >= 700 ? .bold : w >= 600 ? .semibold : w >= 500 ? .medium : .regular
}

/// Sizes of SwiftUI controls, measured once by rendering them.
private var controlSizes: [String: CGSize] = [:]

@MainActor
private func controlSize(_ kind: String) -> CGSize {
    if let s = controlSizes[kind] { return s }
    let view: AnyView
    switch kind {
    case "toggle": view = AnyView(Toggle("", isOn: .constant(true)).labelsHidden().fixedSize())
    // Full-width controls: their height at a typical width.
    case "slider":
        view = AnyView(Slider(value: .constant(0.5), in: 0...1, step: 0.1).frame(width: 160)
            .fixedSize(horizontal: false, vertical: true))
    case "spinner": view = AnyView(ProgressView().progressViewStyle(.circular).fixedSize())
    case "spinner_large": view = AnyView(ProgressView().progressViewStyle(.circular).controlSize(.large).fixedSize())
    default: view = AnyView(ProgressView(value: 0.5).frame(width: 100).fixedSize())
    }
    let size = ImageRenderer(content: view).uiImage?.size ?? .zero
    controlSizes[kind] = size
    return size
}

/// Space around a text area's text (the TextField platter).
enum TextAreaInset {
    static let horizontal = 10.0
    static let vertical = 8.0
}

/// A radio option: the circle, spacing, the row's height.
enum RadioChrome {
    static let width = 30.0
    static let minHeight = 32.0
    static let spacing = 8.0
}

/// A picker row around its value: padding and the chevron.
enum PickerChrome {
    static let width = 44.0
    static let vertical = 20.0
    static let minHeight = 44.0
}

@_cdecl("prism_watch_measure")
public func prismWatchMeasure(_ kind: UnsafePointer<CChar>, _ text: UnsafePointer<CChar>, _ font: Double,
                              _ weight: Int32, _ maxWidth: Double, _ spec: UnsafePointer<CChar>,
                              _ outW: UnsafeMutablePointer<Double>, _ outH: UnsafeMutablePointer<Double>) {
    let kind = String(cString: kind)
    // A styled text (typography or runs, see prism-native remote.rs).
    if kind == "text", spec.pointee != 0,
       let data = String(cString: spec).data(using: .utf8),
       let d = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] {
        let typo = PrismTypo(d, size: font > 0 ? font : 17, weight: Double(weight))
        let result = typo.measure(String(cString: text), runs: d["runs"] as? [[String: Any]] ?? [], maxWidth: maxWidth)
        outW.pointee = Double(result.width)
        outH.pointee = Double(result.height)
        return
    }
    let size = font > 0 ? font : 17
    let uiFont = UIFont.systemFont(ofSize: size, weight: uiWeight(weight))
    func textSize(_ s: String, width: Double = maxWidth) -> CGSize {
        let bounds = (s.isEmpty ? " " : s as NSString).boundingRect(
            with: CGSize(width: width.isFinite ? width : .greatestFiniteMagnitude, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: [.font: uiFont], context: nil)
        return CGSize(width: ceil(bounds.width), height: ceil(bounds.height))
    }
    var result: CGSize
    switch kind {
    case "baseline":
        // `items_baseline()`: `text` is the leaf's kind, `maxWidth` its
        // height; the answer is the distance from its top to the first
        // baseline. Text sits at the top of its frame; fields and picker
        // rows center one line.
        let height = maxWidth.isFinite ? maxWidth : 0
        switch String(cString: text) {
        case "text", "button", "timer":
            result = CGSize(width: 0, height: uiFont.ascender)
        case "text_input", "picker", "date_picker":
            result = CGSize(width: 0, height: max(0, (height - uiFont.lineHeight) / 2) + uiFont.ascender)
        default:
            result = .zero
        }
    case "text", "button", "timer":
        result = textSize(String(cString: text))
        if String(cString: text).isEmpty && kind != "timer" { result.width = 0 }
    case "text_input":
        let line = textSize("Ag")
        result = CGSize(width: maxWidth.isFinite ? maxWidth : 120, height: line.height + 16)
    case "text_area":
        // TextEditor is unavailable on watchOS: a vertical TextField, its
        // text wrapped inside the platter's insets, at least 3 lines.
        let width = maxWidth.isFinite ? maxWidth : 160
        let line = textSize("Ag").height
        let inner = max(width - TextAreaInset.horizontal * 2, 1)
        let content = textSize(String(cString: text), width: inner).height
        result = CGSize(width: width, height: max(content, line * 3) + TextAreaInset.vertical * 2)
    case "toggle", "progress":
        result = MainActor.assumeIsolated { controlSize(kind) }
        if kind == "progress", maxWidth.isFinite { result.width = maxWidth }
    case "slider":
        result = MainActor.assumeIsolated { controlSize(kind) }
        // Narrow screens: a small natural width (rows grow it with flex).
        result.width = maxWidth.isFinite ? min(maxWidth, 100) : 100
    case "picker":
        // A navigation-link row: the widest option and its chrome.
        let label = textSize(String(cString: text))
        result = CGSize(width: label.width + PickerChrome.width,
                        height: max(label.height + PickerChrome.vertical, PickerChrome.minHeight))
        if maxWidth.isFinite { result.width = min(result.width, maxWidth) }
    case "date_picker":
        // A button showing the value (a wide one: day 28, 23:58).
        let mode = String(cString: text).split(separator: " ").first.map(String.init) ?? "date"
        let lang = MainActor.assumeIsolated { PrismStore.shared.lang }
        let sample = PrismDatePicker.format(Date(timeIntervalSince1970: 1_798_498_680), mode, ControlEnvironment.locale(lang))
        let label = textSize(sample, width: .infinity)
        result = CGSize(width: label.width + PickerChrome.width,
                        height: max(label.height + PickerChrome.vertical, PickerChrome.minHeight))
        if maxWidth.isFinite { result.width = min(result.width, maxWidth) }
    case "spinner":
        result = MainActor.assumeIsolated { controlSize(String(cString: text) == "large" ? "spinner_large" : "spinner") }
    case "checkbox":
        // A labelled toggle (watchOS has no checkbox style).
        let toggle = MainActor.assumeIsolated { controlSize("toggle") }
        let label = textSize(String(cString: text), width: maxWidth.isFinite ? max(maxWidth - toggle.width - 8, 1) : .infinity)
        result = CGSize(width: label.width + toggle.width + 8, height: max(label.height, toggle.height) + 8)
        if maxWidth.isFinite { result.width = min(result.width, maxWidth) }
    case "radio_group":
        // One row per option (a circle and the label), side by side for `row`.
        let lines = String(cString: text).components(separatedBy: "\n")
        let row = lines.first == "row"
        let rows = lines.dropFirst().map { textSize($0, width: .infinity) }
        let item = { (s: CGSize) in CGSize(width: s.width + RadioChrome.width, height: max(s.height, RadioChrome.minHeight)) }
        if row {
            result = rows.map(item).reduce(CGSize.zero) { CGSize(width: $0.width + $1.width + RadioChrome.spacing, height: max($0.height, $1.height)) }
            result.width = max(result.width - RadioChrome.spacing, 0)
        } else {
            result = rows.map(item).reduce(CGSize.zero) { CGSize(width: max($0.width, $1.width), height: $0.height + $1.height) }
        }
        if maxWidth.isFinite { result.width = min(result.width, maxWidth) }
    case "image":
        let config = UIImage.SymbolConfiguration(pointSize: size)
        result = UIImage(systemName: String(cString: text), withConfiguration: config)?.size ?? CGSize(width: size, height: size)
    default:
        result = .zero
    }
    outW.pointee = Double(result.width)
    outH.pointee = Double(result.height)
}

// MARK: Typography (prism-core typography styles and rich-text runs)

/// UIFont weights at CSS 100, 200 … 900 (UIFontDescriptor.h); weights in
/// between (650) are interpolated: the system font is variable.
private let uiWeightSteps: [UIFont.Weight] = [.ultraLight, .thin, .light, .regular, .medium, .semibold, .bold, .heavy, .black]

func prismUIWeight(_ css: Double) -> UIFont.Weight {
    let w = min(max(css, 100), 900)
    let i = min(Int((w - 100) / 100), 7)
    let t = (w - 100 - Double(i) * 100) / 100
    let a = uiWeightSteps[i].rawValue, b = uiWeightSteps[i + 1].rawValue
    return UIFont.Weight(a + (b - a) * t)
}

/// The font of a run: family (`mono`, `serif`, `rounded`, a name), size,
/// CSS weight, italic, tabular digits. Drawing and measuring use the same one.
func prismUIFont(family: String?, size: Double, weight: Double, italic: Bool, tabular: Bool) -> UIFont {
    var font = UIFont.systemFont(ofSize: size, weight: prismUIWeight(weight))
    switch family {
    case nil, "", "system": break
    case "mono":
        font = UIFont.monospacedSystemFont(ofSize: size, weight: prismUIWeight(weight))
    case "serif", "rounded":
        if let d = font.fontDescriptor.withDesign(family == "serif" ? .serif : .rounded) {
            font = UIFont(descriptor: d, size: size)
        }
    case let name?:
        // An installed or bundled family (`assets/fonts`), its nearest face.
        if UIFont.familyNames.contains(name) {
            let d = UIFontDescriptor(fontAttributes: [
                .family: name,
                .traits: [UIFontDescriptor.TraitKey.weight: prismUIWeight(weight)],
            ])
            font = UIFont(descriptor: d, size: size)
        } else if let named = UIFont(name: name, size: size) {
            font = named
        }
    }
    var d = font.fontDescriptor
    if italic, let it = d.withSymbolicTraits(d.symbolicTraits.union(.traitItalic)) { d = it }
    if tabular {
        d = d.addingAttributes([.featureSettings: [[
            UIFontDescriptor.FeatureKey.type: kNumberSpacingType,
            UIFontDescriptor.FeatureKey.selector: kMonospacedNumbersSelector,
        ]]])
    }
    return italic || tabular ? UIFont(descriptor: d, size: size) : font
}

/// The text as drawn (`tt`): CSS `text-transform` (capitalize only raises
/// the first letter of each word).
func prismTransform(_ s: String, _ tt: String?) -> String {
    switch tt {
    case "upper": return s.uppercased()
    case "lower": return s.lowercased()
    case "capitalize":
        var out = "", start = true
        for c in s {
            out += start && (c.isLetter || c.isNumber) ? c.uppercased() : String(c)
            start = !(c.isLetter || c.isNumber) && c != "'" && c != "\u{2019}"
        }
        return out
    default: return s
    }
}

/// A text's typography: the style op's keys (`font`, `weight`, `ff`, `it`,
/// `ul`, `st`, `tn`, `tt`, `ls`, `lh`, `ml`, `nw`), or a measure spec.
struct PrismTypo {
    var size: Double
    var weight: Double
    var family: String?
    var italic = false, underline = false, strike = false, tabular = false, nowrap = false
    var transform: String?
    var letterSpacing = 0.0
    var lineHeight: Double?
    var maxLines: Int?

    init(_ d: [String: Any], size: Double, weight: Double) {
        func flag(_ k: String) -> Bool { d[k] as? Bool ?? false }
        func n(_ k: String) -> Double? { (d[k] as? NSNumber)?.doubleValue }
        self.size = size
        self.weight = weight
        family = d["ff"] as? String
        italic = flag("it"); underline = flag("ul"); strike = flag("st"); tabular = flag("tn"); nowrap = flag("nw")
        transform = d["tt"] as? String
        letterSpacing = n("ls") ?? 0
        lineHeight = n("lh")
        maxLines = n("ml").map { Int($0) }
    }

    init(_ n: Node) {
        self.init(n.style, size: n.num("font", 17), weight: n.num("weight", 400))
    }

    /// The node's own font.
    var font: UIFont { prismUIFont(family: family, size: size, weight: weight, italic: italic, tabular: tabular) }

    /// A run's font: its own family, size (`z` × the text's), weight and italic.
    func font(_ r: [String: Any]) -> UIFont {
        let z = (r["z"] as? NSNumber)?.doubleValue
        return prismUIFont(family: (r["ff"] as? String) ?? family,
                           size: z.map { max(($0 * size).rounded(), 1) } ?? size,
                           weight: (r["w"] as? NSNumber)?.doubleValue ?? weight,
                           italic: italic || (r["i"] as? Bool ?? false), tabular: tabular)
    }

    /// Extra space between lines for the line height (SwiftUI's `lineSpacing`
    /// cannot tighten lines below the font's own height).
    var lineSpacing: Double { lineHeight.map { max($0 - font.lineHeight, 0) } ?? 0 }

    /// The runs, or the plain text as one run.
    func runs(_ text: String, _ runs: [[String: Any]]) -> [[String: Any]] { runs.isEmpty ? [["t": text]] : runs }

    /// What `prism_watch_measure` returns: the attributed text's size, capped
    /// at `maxLines`; one line per paragraph when it never wraps.
    func measure(_ text: String, runs: [[String: Any]], maxWidth: Double) -> CGSize {
        let s = NSMutableAttributedString()
        var lineH = font.lineHeight
        for r in self.runs(text, runs) {
            let f = font(r)
            lineH = max(lineH, f.lineHeight)
            if let icon = r["icon"] as? String {
                let config = UIImage.SymbolConfiguration(pointSize: f.pointSize, weight: .regular)
                let attachment = NSTextAttachment()
                attachment.image = UIImage(systemName: icon, withConfiguration: config)
                s.append(NSAttributedString(attachment: attachment))
                continue
            }
            s.append(NSAttributedString(string: prismTransform(r["t"] as? String ?? "", transform),
                                        attributes: [.font: f, .kern: letterSpacing]))
        }
        if s.length == 0 { return .zero }
        let para = NSMutableParagraphStyle()
        para.lineSpacing = lineSpacing
        s.addAttribute(.paragraphStyle, value: para, range: NSRange(location: 0, length: s.length))
        let single = nowrap || maxLines == 1
        let width = single || !maxWidth.isFinite ? CGFloat.greatestFiniteMagnitude : maxWidth
        let r = s.boundingRect(with: CGSize(width: width, height: .greatestFiniteMagnitude),
                               options: [.usesLineFragmentOrigin, .usesFontLeading], context: nil)
        var size = CGSize(width: ceil(r.width), height: ceil(r.height))
        if let n = maxLines, n > 0 {
            size.height = min(size.height, ceil(Double(n) * lineH + Double(n - 1) * lineSpacing))
            if maxWidth.isFinite { size.width = min(size.width, maxWidth) }
        }
        return size
    }

    /// The SwiftUI text: one attributed run after another (inline symbols as
    /// images in the text), links as `prism-link://<id>` URLs.
    func text(_ text: String, runs: [[String: Any]], dark: Bool) -> Text {
        var out: Text?
        var pending = AttributedString()
        func flush() {
            guard !pending.characters.isEmpty else { return }
            let t = Text(pending)
            out = out.map { Text("\($0)\(t)") } ?? t
            pending = AttributedString()
        }
        for r in self.runs(text, runs) {
            let f = Font(font(r) as CTFont)
            let color = PrismColor(r["c"]).map { dark ? $0.dark : $0.light }
            if let icon = r["icon"] as? String {
                flush()
                var t = Text(Image(systemName: icon)).font(f)
                if let color { t = t.foregroundColor(color) }
                out = out.map { Text("\($0)\(t)") } ?? t
                continue
            }
            var a = AttributedString(prismTransform(r["t"] as? String ?? "", transform))
            a.font = f
            if let color { a.foregroundColor = color }
            if let bg = PrismColor(r["bg"]) { a.backgroundColor = dark ? bg.dark : bg.light }
            if underline || (r["u"] as? Bool ?? false) { a.underlineStyle = .single }
            if strike || (r["s"] as? Bool ?? false) { a.strikethroughStyle = .single }
            if letterSpacing != 0 { a.kern = letterSpacing }
            if let link = (r["link"] as? NSNumber)?.uint64Value { a.link = URL(string: "prism-link://\(link)") }
            pending += a
        }
        flush()
        return out ?? Text("")
    }
}

/// Activates a rich text's links (`prism-link://<id>`): reported like a menu item.
let prismOpenLink = OpenURLAction { url in
    guard url.scheme == "prism-link", let id = UInt64(url.host ?? "") else { return .systemAction }
    send(["t": "menu", "id": id])
    return .handled
}

/// Registers the fonts the app bundles in `assets/fonts` (`.ttf`, `.otf`,
/// `.ttc`), so `font_family("Name")` finds them.
func prismRegisterFonts() {
    let dir = URL(fileURLWithPath: Bundle.main.bundlePath).appendingPathComponent("assets/fonts")
    let files = (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)) ?? []
    for f in files where ["ttf", "otf", "ttc"].contains(f.pathExtension.lowercased()) {
        var error: Unmanaged<CFError>?
        if !CTFontManagerRegisterFontsForURL(f as CFURL, .process, &error) {
            NSLog("prism: font %@ not registered: %@", f.lastPathComponent, "\(String(describing: error?.takeRetainedValue()))")
        }
    }
}

// MARK: Tree

/// A Prism shape: paths resolved by Rust for the node's size. `d` is flat:
/// `0 x y` move, `1 x y` line, `2 x1 y1 x2 y2 x y` curve, `3` close.
struct PrismShape: View {
    let paths: [[String: Any]]
    let dark: Bool

    var body: some View {
        Canvas { ctx, _ in
            for p in paths {
                let path = Self.path(p["d"] as? [Double] ?? [])
                if let f = PrismColor(p["fill"]) {
                    ctx.fill(path, with: .color(dark ? f.dark : f.light), style: FillStyle(eoFill: p["eo"] as? Bool ?? false))
                }
                if let s = PrismColor(p["stroke"]) {
                    let round = p["round"] as? Bool ?? false
                    let style = StrokeStyle(
                        lineWidth: (p["w"] as? Double) ?? 1,
                        lineCap: round ? .round : .butt,
                        lineJoin: round ? .round : .miter,
                        dash: (p["dash"] as? [Double])?.map { CGFloat($0) } ?? []
                    )
                    ctx.stroke(path, with: .color(dark ? s.dark : s.light), style: style)
                }
            }
        }
    }

    static func path(_ d: [Double]) -> Path {
        var path = Path()
        var i = 0
        while i < d.count {
            let cmd = Int(d[i])
            i += 1
            let n = cmd == 2 ? 6 : cmd == 3 ? 0 : 2
            guard i + n <= d.count else { break }
            let v = Array(d[i..<(i + n)])
            i += n
            switch cmd {
            case 0: path.move(to: CGPoint(x: v[0], y: v[1]))
            case 1: path.addLine(to: CGPoint(x: v[0], y: v[1]))
            case 2:
                path.addCurve(to: CGPoint(x: v[4], y: v[5]),
                              control1: CGPoint(x: v[0], y: v[1]),
                              control2: CGPoint(x: v[2], y: v[3]))
            case 3: path.closeSubpath()
            default: return path
            }
        }
        return path
    }
}

struct PrismColor {
    var light: Color
    var dark: Color

    init?(_ v: Any?) {
        guard let a = v as? [Double], a.count >= 4 else { return nil }
        light = Color(.sRGB, red: a[0], green: a[1], blue: a[2], opacity: a[3])
        dark = a.count >= 8 ? Color(.sRGB, red: a[4], green: a[5], blue: a[6], opacity: a[7]) : light
    }
}

struct Node {
    var kind: String
    var children: [UInt64] = []
    var text = ""
    var placeholder = ""
    var icon = ""
    var value = 0.0
    var clickable = false
    var label: String?
    /// ARIA role name (`slider`, `status`…) and the read-out value.
    var role: String?
    var a11yValue: String?
    /// A control's config (`prism_core::controls`): slider `min max step`,
    /// picker options one per line, date `mode min max`, input tokens.
    var config = ""
    var a11yHidden = false
    var hidden = false
    var style: [String: Any] = [:]
    /// x, y, width, height in the parent (from Rust's layout).
    var frame = CGRect.zero
    /// Scroll views: the extent of their content.
    var content = CGSize.zero
    /// Context menu (long press): Prism's menu JSON.
    var menu: [String: Any]?
    var swipeLeft = false
    var swipeRight = false
    var scrollEvents = false
    /// Back and forth between the style's pose and this one.
    var loop: PrismLoop?
    /// A shape's paths, in points (see `PrismShape`).
    var paths: [[String: Any]] = []
    /// `.sticky()`: the scroll view it follows and its range there.
    var sticky: PrismSticky?
    /// A rich text's runs (`runs` op).
    var runs: [[String: Any]] = []
    /// Interaction states Rust wants reported (`states` op): `active`.
    var states: Set<String> = []
    /// Pointer events reported: 1 hover (none on a watch), 2 move, 4 drag.
    var pointerMask = 0
    /// A runtime layer's root: lets touches through where it has no child.
    var layer = false
    /// A 3D scene: a count of its updates (`scene` ops, applied to its
    /// `PrismSceneHost` as they come).
    var sceneVersion = 0

    /// `hit_test(false)`: the subtree takes no touch.
    var noHit: Bool { (style["hit"] as? Bool) == false }
    /// `inert`: no touch, no focus, hidden from VoiceOver.
    var inert: Bool { (style["inert"] as? Bool) == true }

    func num(_ k: String, _ d: Double = 0) -> Double { (style[k] as? NSNumber)?.doubleValue ?? d }
    /// The node's box: its radius on the corners it rounds (`corners` bits:
    /// 1 top left, 2 top right, 4 bottom right, 8 bottom left; Rust already
    /// mirrored them for right to left).
    func box() -> UnevenRoundedRectangle {
        let r = num("radius"), c = Int(num("corners", 15))
        func at(_ bit: Int) -> Double { c & bit != 0 ? r : 0 }
        return UnevenRoundedRectangle(topLeadingRadius: at(1), bottomLeadingRadius: at(8), bottomTrailingRadius: at(4), topTrailingRadius: at(2))
    }
    func str(_ k: String) -> String? { style[k] as? String }
    /// Padding (#14), `pad`: [top, right, bottom, left], left and right as
    /// drawn (Rust already mirrored them for right to left).
    var padding: EdgeInsets {
        guard let p = style["pad"] as? [NSNumber], p.count == 4 else { return EdgeInsets() }
        let v = p.map { CGFloat($0.doubleValue) }
        return EdgeInsets(top: v[0], leading: v[3], bottom: v[2], trailing: v[1])
    }
    func color(_ k: String, dark: Bool) -> Color? { PrismColor(style[k]).map { dark ? $0.dark : $0.light } }
}

final class PrismStore: ObservableObject {
    static let shared = PrismStore()
    @Published var nodes: [UInt64: Node] = [:]
    @Published var root: UInt64?
    /// Snapshots draw scroll views as plain views (ImageRenderer cannot).
    var snapshotting = false
    /// Presented over the app: an alert, a long-press menu, the photo
    /// picker, the share sheet.
    @Published var alert: [String: Any]?
    @Published var menuNode: UInt64?
    @Published var picker: (id: UInt64, limit: Int)?
    @Published var share: (text: String?, url: String?)?
    /// Pictures of removed views animating out, by node.
    @Published var ghosts: [UInt64: PrismGhost] = [:]
    /// The app's language direction and language (Rust's `env`): controls
    /// mirror and format dates in it. Layout is already mirrored by Rust.
    @Published var rtl = false
    @Published var lang = ""
    /// Runtime layers (the toaster), above everything.
    @Published var layers: [UInt64] = []
    /// `cx.focus` / `.autofocus()`: the node, and a count so asking again
    /// for the same node is a change.
    @Published var focus: (id: UInt64, n: Int) = (0, 0)
    /// `cx.dismiss_keyboard()`: a count, so asking again unfocuses again.
    @Published var blur = 0
    /// `cx.scroll_into_view`: the node, and a count.
    @Published var reveal: (id: UInt64, n: Int) = (0, 0)

    func apply(_ json: String) {
        guard let data = json.data(using: .utf8),
              let patches = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]] else { return }
        var nodes = self.nodes
        var root = self.root
        // Animated changes run once the rest is on screen, so they start from
        // what is shown (an entering view's first pose).
        var animated: [[String: Any]] = []
        var announcements: [String] = []
        for p in patches {
            let id = (p["id"] as? NSNumber)?.uint64Value ?? 0
            if p["anim"] is [String: Any], p["op"] as? String != "exit", p["op"] as? String != "repeat" {
                animated.append(p)
                continue
            }
            switch p["op"] as? String {
            case "exit": MainActor.assumeIsolated { exit(id, p) }
            case "repeat":
                nodes[id]?.loop = (p["to"] as? [String: Any]).map {
                    PrismLoop(from: PrismPose(p["from"]), to: PrismPose($0), anim: p["anim"] as? [String: Any] ?? [:],
                              restart: p["restart"] as? Bool ?? false)
                }
            case "create": nodes[id] = Node(kind: p["kind"] as? String ?? "view")
            case "destroy":
                nodes[id] = nil
                PrismSceneHost.forget(id)
            case "root": root = id
            case "children": nodes[id]?.children = (p["children"] as? [NSNumber] ?? []).map(\.uint64Value)
            case "text": nodes[id]?.text = p["text"] as? String ?? ""
            case "draw": nodes[id]?.paths = p["paths"] as? [[String: Any]] ?? []
            case "scene":
                // Updates are diffs: each one goes to the scene's player at once.
                PrismSceneHost.of(id).apply(p["u"])
                nodes[id]?.sceneVersion += 1
            case "placeholder": nodes[id]?.placeholder = p["text"] as? String ?? ""
            case "icon": nodes[id]?.icon = p["icon"] as? String ?? ""
            case "value": nodes[id]?.value = (p["value"] as? NSNumber)?.doubleValue ?? 0
            case "clickable": nodes[id]?.clickable = p["on"] as? Bool ?? false
            case "display": nodes[id]?.hidden = p["hidden"] as? Bool ?? false
            case "a11y":
                let role = p["role"] as? String
                let value = p["value"] as? String
                // A status region announces its new value (VoiceOver).
                if role == "status", let value, !value.isEmpty, value != nodes[id]?.a11yValue {
                    announcements.append(value)
                }
                nodes[id]?.label = p["label"] as? String
                nodes[id]?.a11yHidden = p["hidden"] as? Bool ?? false
                nodes[id]?.role = role
                nodes[id]?.a11yValue = value
            case "config": nodes[id]?.config = p["config"] as? String ?? ""
            case "runs": nodes[id]?.runs = p["runs"] as? [[String: Any]] ?? []
            case "env":
                rtl = p["rtl"] as? Bool ?? false
                lang = p["lang"] as? String ?? ""
            case "style": nodes[id]?.style = p["s"] as? [String: Any] ?? [:]
            case "frame":
                if let f = p["f"] as? [Double], f.count == 4 { nodes[id]?.frame = prismFrame(f) }
            case "content":
                if let c = p["size"] as? [Double], c.count == 2 { nodes[id]?.content = CGSize(width: c[0], height: c[1]) }
            case "menu": nodes[id]?.menu = p["menu"] as? [String: Any]
            case "swipe":
                nodes[id]?.swipeLeft = p["left"] as? Bool ?? false
                nodes[id]?.swipeRight = p["right"] as? Bool ?? false
            case "scrollev": nodes[id]?.scrollEvents = p["on"] as? Bool ?? false
            case "sticky":
                let sticky = PrismSticky(p)
                nodes[id]?.sticky = sticky
                if let sticky { PrismScrollState.shared.watched.insert(sticky.scroll) }
            case "states": nodes[id]?.states = Set(p["on"] as? [String] ?? [])
            case "alert": alert = p["alert"] as? [String: Any]
            case "pointer": nodes[id]?.pointerMask = (p["mask"] as? NSNumber)?.intValue ?? 0
            // No keyboard on a watch (and the Digital Crown scrolls): keys never come.
            case "keys": break
            case "focus": focus = (id, focus.n + 1)
            // `cx.dismiss_keyboard()` (#29): every field lets go.
            case "blur": blur += 1
            case "reveal": reveal = (id, reveal.n + 1)
            case "layers":
                let ids = (p["ids"] as? [NSNumber] ?? []).map(\.uint64Value)
                for l in layers where !ids.contains(l) { nodes[l]?.layer = false }
                for l in ids { nodes[l]?.layer = true }
                layers = ids
            default: break
            }
        }
        self.nodes = nodes
        self.root = root
        // Accessibility framework (watchOS 10); UIAccessibilityPostNotification
        // is API_UNAVAILABLE(watchos) in UIAccessibility.h.
        for a in announcements {
            AccessibilityNotification.Announcement(a).post()
            if ProcessInfo.processInfo.environment["PRISM_DEBUG_A11Y"] != nil { NSLog("prism-a11y announce %@", a) }
        }
        if !animated.isEmpty {
            DispatchQueue.main.async {
                for p in animated {
                    let id = (p["id"] as? NSNumber)?.uint64Value ?? 0
                    withAnimation(prismAnimation(p["anim"] as? [String: Any] ?? [:])) {
                        switch p["op"] as? String {
                        case "style": self.nodes[id]?.style = p["s"] as? [String: Any] ?? [:]
                        case "frame":
                            if let f = p["f"] as? [Double], f.count == 4 {
                                self.nodes[id]?.frame = prismFrame(f)
                            }
                        default: break
                        }
                    }
                }
            }
        }
        // `PRISM_DEBUG_LAYOUT=1`: log every text's frame.
        if ProcessInfo.processInfo.environment["PRISM_DEBUG_LAYOUT"] != nil {
            for (id, n) in nodes where !n.text.isEmpty {
                NSLog("prism-layout %llu %@ %@ %@", id, n.kind, n.text, NSCoder.string(for: n.frame))
            }
        }
    }
}

extension PrismStore {
    /// A picture of the view, left in its parent, animating to `to`.
    @MainActor
    func exit(_ id: UInt64, _ p: [String: Any]) {
        guard let n = nodes[id], n.frame.width > 0, n.frame.height > 0,
              let parent = (p["parent"] as? NSNumber)?.uint64Value else { return }
        let renderer = ImageRenderer(content: NodeView(id: id).environment(\.colorScheme, .dark))
        renderer.scale = WKInterfaceDevice.current().screenScale
        guard let image = renderer.uiImage else { return }
        let anim = p["anim"] as? [String: Any] ?? [:]
        ghosts[id] = PrismGhost(parent: parent, image: image, frame: n.frame,
                                from: PrismPose(["o": n.num("opacity", 1), "tf": n.style["tf"] ?? NSNull()]),
                                to: PrismPose(p["to"]), anim: anim)
        let total = (anim["d"] as? Double ?? 0.25) + (anim["delay"] as? Double ?? 0) + 0.05
        DispatchQueue.main.asyncAfter(deadline: .now() + total) { self.ghosts[id] = nil }
    }
}

/// `[x, y, width, height]` from Rust as a frame SwiftUI accepts: a size
/// that is negative or not finite (a degenerate absolutely positioned
/// overlay) would make `.frame(width:height:)` invalid and could take the
/// siblings drawn in the same stack (chart bars) down with it.
func prismFrame(_ f: [Double]) -> CGRect {
    let v = f.map { $0.isFinite ? $0 : 0 }
    return CGRect(x: v[0], y: v[1], width: max(0, v[2]), height: max(0, v[3]))
}

// MARK: Motion

/// Opacity and transform (offset, scale, rotation around the center).
struct PrismPose: Equatable {
    var o = 1.0, x = 0.0, y = 0.0, scale = 1.0, rotate = 0.0
    /// Size factors on each axis (multiplied with `scale`).
    var scaleX = 1.0, scaleY = 1.0

    init(_ v: Any?) {
        let d = v as? [String: Any] ?? [:]
        o = (d["o"] as? NSNumber)?.doubleValue ?? 1
        // `[x, y, scale, degrees]`, then `scale x, scale y`.
        if let tf = d["tf"] as? [Double], tf.count >= 4 { (x, y, scale, rotate) = (tf[0], tf[1], tf[2], tf[3]) }
        if let tf = d["tf"] as? [Double], tf.count >= 6 { (scaleX, scaleY) = (tf[4], tf[5]) }
    }

    init(opacity: Double, tf: Any?) { self.init(["o": opacity, "tf": tf ?? NSNull()]) }
}

struct PrismLoop: Equatable {
    var from: PrismPose
    var to: PrismPose
    var anim: [String: Any]
    /// Starts over from `from` instead of swinging back (spinners).
    var restart = false
    static func == (a: PrismLoop, b: PrismLoop) -> Bool {
        a.from == b.from && a.to == b.to && a.restart == b.restart && NSDictionary(dictionary: a.anim).isEqual(to: b.anim)
    }
}

struct PrismGhost {
    var parent: UInt64
    var image: UIImage
    var frame: CGRect
    var from: PrismPose
    var to: PrismPose
    var anim: [String: Any]
}

/// `{d, delay, ease: [x1, y1, x2, y2]}` as a SwiftUI animation.
func prismAnimation(_ a: [String: Any]) -> Animation {
    let e = a["ease"] as? [Double] ?? [0.42, 0, 0.58, 1]
    let curve = Animation.timingCurve(e[0], e[1], e[2], e[3], duration: a["d"] as? Double ?? 0.25)
    return curve.delay(a["delay"] as? Double ?? 0)
}

extension View {
    func prismPose(_ p: PrismPose) -> some View {
        scaleEffect(x: p.scale * p.scaleX, y: p.scale * p.scaleY).rotationEffect(.degrees(p.rotate)).offset(x: p.x, y: p.y).opacity(p.o)
    }
}

/// The style's pose, or a loop between two.
struct PrismPosed: ViewModifier {
    let pose: PrismPose
    let loop: PrismLoop?
    @State private var swing = false

    func body(content: Content) -> some View {
        content.prismPose(loop.map { swing ? $0.to : $0.from } ?? pose)
            .onAppear(perform: start)
            .onChange(of: loop) { _, _ in start() }
    }

    private func start() {
        var t = Transaction()
        t.disablesAnimations = true
        withTransaction(t) { swing = false }
        guard let loop else { return }
        let d = loop.anim["d"] as? Double ?? 0.6
        // A loop that starts over keeps its curve (linear for a spinner:
        // the angle animates as a number, so 0 to 360 degrees is a full turn);
        // swings, and curves that overshoot, ease in and out.
        let e = loop.anim["ease"] as? [Double] ?? []
        let overshoots = e.count < 4 || e.contains { $0 < 0 || $0 > 1 }
        let curve: Animation = loop.restart && !overshoots
            ? .timingCurve(e[0], e[1], e[2], e[3], duration: d) : .easeInOut(duration: d)
        DispatchQueue.main.async {
            withAnimation(curve.repeatForever(autoreverses: !loop.restart).delay(loop.anim["delay"] as? Double ?? 0)) {
                swing = true
            }
        }
    }
}

/// A removed view's picture, animating out.
struct GhostView: View {
    let ghost: PrismGhost
    @State private var gone = false

    var body: some View {
        Image(uiImage: ghost.image).resizable()
            .frame(width: ghost.frame.width, height: ghost.frame.height)
            .prismPose(gone ? ghost.to : ghost.from)
            .allowsHitTesting(false)
            .onAppear { withAnimation(prismAnimation(ghost.anim)) { gone = true } }
    }
}

@_cdecl("prism_watch_apply")
public func prismWatchApply(_ json: UnsafePointer<CChar>) {
    PrismStore.shared.apply(String(cString: json))
}

// MARK: Rendering

/// A node at its own size; its parent places it.
struct NodeView: View {
    @ObservedObject var store = PrismStore.shared
    @Environment(\.colorScheme) var scheme
    let id: UInt64
    /// A drag past the slop is under way.
    @State private var dragging = false

    var body: some View {
        if let n = store.nodes[id], !n.hidden {
            let dark = scheme == .dark
            // Snapshots show a scroll view's whole content.
            let height = store.snapshotting && n.kind == "scroll" ? max(n.content.height, n.frame.height) : n.frame.height
            let pressable = n.states.contains("active")
            content(n, dark: dark)
                // The content inside its padding; the box around it.
                .padding(n.padding)
                .frame(width: n.frame.width, height: height, alignment: .topLeading)
                .background(BoxFill(node: n, dark: dark))
                .overlay(BoxBorder(node: n, dark: dark))
                .modifier(RoundedClip(radius: n.num("radius"), clip: n.style["clip"] as? Bool ?? false, shape: n.box()))
                .background(DropShadows(node: n, dark: dark))
                .modifier(PrismPosed(pose: PrismPose(opacity: n.num("opacity", 1), tf: n.style["tf"]), loop: n.loop))
                .modifier(PressReport(id: id, on: pressable, clickable: n.clickable))
                // A layer's root lets touches through (only its children take them).
                .modifier(HitShape(on: !n.layer))
                // Taps report where they landed (ClickEvent::position).
                // (A pressable node's press report sends its click.)
                .onTapGesture(coordinateSpace: .local) { p in
                    if n.clickable && !pressable { send(["t": "click", "id": id, "x": p.x, "y": p.y]) }
                }
                // Moves while touching, and drags past the slop.
                .simultaneousGesture(DragGesture(minimumDistance: 0).onChanged { g in
                    if n.pointerMask & 2 != 0 { send(["t": "pointer", "id": id, "phase": "move", "x": g.location.x, "y": g.location.y]) }
                    guard n.pointerMask & 4 != 0 else { return }
                    let t = g.translation
                    let event: [String: Any] = ["t": "drag", "id": id, "x": g.startLocation.x, "y": g.startLocation.y,
                                                "dx": t.width, "dy": t.height]
                    if !dragging, t.width * t.width + t.height * t.height > 100 {
                        dragging = true
                        send(event.merging(["phase": "start"]) { $1 })
                    } else if dragging {
                        send(event.merging(["phase": "move"]) { $1 })
                    }
                }.onEnded { g in
                    guard dragging else { return }
                    dragging = false
                    send(["t": "drag", "id": id, "phase": "end", "x": g.startLocation.x, "y": g.startLocation.y,
                          "dx": g.translation.width, "dy": g.translation.height])
                }, including: n.pointerMask & 6 != 0 ? .all : .subviews)
                // Context menus: a long press opens them as a dialog (SwiftUI's
                // contextMenu is deprecated on watchOS).
                .onLongPressGesture { if n.menu != nil { store.menuNode = id } }
                .simultaneousGesture(DragGesture(minimumDistance: 24).onEnded { g in
                    let dx = g.translation.width
                    guard abs(dx) > abs(g.translation.height) * 2 else { return }
                    if dx < 0 && n.swipeLeft { send(["t": "swipe", "id": id, "dir": "left"]) }
                    if dx > 0 && n.swipeRight { send(["t": "swipe", "id": id, "dir": "right"]) }
                }, including: n.swipeLeft || n.swipeRight ? .all : .subviews)
                .accessibilityLabel(n.label ?? "")
                .accessibilityHidden(n.a11yHidden || n.inert)
                .allowsHitTesting(!n.noHit && !n.inert)
                .disabled(n.inert)
                .id(id)
        }
    }

    private func font(_ n: Node) -> Font {
        .system(size: n.num("font", 17), weight: swiftWeight(Int(n.num("weight", 400))))
    }

    @ViewBuilder
    private func content(_ n: Node, dark: Bool) -> some View {
        let fg = n.color("color", dark: dark) ?? .primary
        switch n.kind {
        case "text":
            // Lines align in the text's frame too (right-aligned in RTL: Rust
            // swaps left and right).
            let typo = PrismTypo(n)
            let linkColor = n.runs.first { $0["link"] != nil }.flatMap { PrismColor($0["c"]) }.map { dark ? $0.dark : $0.light }
            let t = typo.text(n.text, runs: n.runs, dark: dark)
                .foregroundStyle(fg).multilineTextAlignment(textAlign(n.str("talign")))
                .lineSpacing(typo.lineSpacing)
                .tint(linkColor ?? fg)
                .environment(\.openURL, prismOpenLink)
                .accessibilityAddTraits(n.clickable ? (n.role == "link" ? .isLink : .isButton) : [])
            if typo.nowrap {
                // One line per paragraph, cut at the frame's edge.
                t.fixedSize(horizontal: true, vertical: false)
                    .frame(maxWidth: .infinity, alignment: frameAlign(n.str("talign"))).clipped()
            } else {
                t.lineLimit(typo.maxLines).truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: frameAlign(n.str("talign")))
            }
        case "button":
            Button { send(["t": "click", "id": id]) } label: { Text(n.text).font(font(n)).foregroundStyle(fg) }
                .buttonStyle(.plain)
        case "toggle":
            Toggle(n.label ?? "", isOn: Binding(
                get: { n.value != 0 },
                set: { send(["t": "change", "id": id, "v": $0 ? 1 : 0]) }
            )).labelsHidden()
                .modifier(ControlEnvironment(rtl: store.rtl, lang: store.lang))
        case "text_input", "text_area":
            PrismTextField(id: id, node: n, font: font(n))
                .modifier(ControlEnvironment(rtl: store.rtl, lang: store.lang))
        case "slider":
            PrismSlider(id: id, node: n, tint: n.color("color", dark: dark))
                .modifier(ControlEnvironment(rtl: store.rtl, lang: store.lang))
        case "picker":
            PrismPicker(id: id, node: n, font: font(n), tint: n.color("color", dark: dark))
                .modifier(ControlEnvironment(rtl: store.rtl, lang: store.lang))
        case "date_picker":
            PrismDatePicker(id: id, node: n, tint: n.color("color", dark: dark))
                .modifier(ControlEnvironment(rtl: store.rtl, lang: store.lang))
        case "spinner":
            // Indeterminate: animates while on screen.
            ProgressView().progressViewStyle(.circular)
                .controlSize(n.value != 0 ? .large : .regular)
                .tint(n.color("color", dark: dark))
                .frame(width: n.frame.width, height: n.frame.height)
        case "progress":
            ProgressView(value: min(max(n.value, 0), 1))
                .modifier(ControlEnvironment(rtl: store.rtl, lang: store.lang))
        case "image":
            Image(systemName: n.icon.isEmpty ? "circle" : n.icon).font(font(n)).foregroundStyle(fg)
        case "timer":
            Text(Date(timeIntervalSince1970: n.value), style: .timer).font(font(n).monospacedDigit()).foregroundStyle(fg)
        case "picture":
            Picture(source: n.text, fit: n.value == 1)
        case "scene3d":
            PrismScene3d(id: id, node: n, dark: dark)
        case "shape":
            PrismShape(paths: n.paths, dark: dark)
        case "scroll":
            if store.snapshotting {
                placed(n.children)
            } else {
                let horizontal = n.style["hscroll"] as? Bool ?? false
                ScrollViewReader { proxy in
                    ScrollView(horizontal ? .horizontal : .vertical) {
                        placed(n.children)
                            .frame(width: n.content.width, height: n.content.height, alignment: .topLeading)
                            .background(GeometryReader { g in
                                Color.clear.onChange(of: g.frame(in: .named("s\(id)")).minY) { _, y in
                                    guard !horizontal else { return }
                                    PrismScrollState.shared.scrolled(id, -y)
                                    if n.scrollEvents { send(["t": "scroll", "id": id, "o": -y, "v": n.frame.height]) }
                                }
                            })
                            .overlay(alignment: .topLeading) { DevScrollMark(id: id) }
                    }
                    .coordinateSpace(name: "s\(id)")
                    .modifier(Refresh(id: id, on: n.clickable))
                    .onAppear { DevScrollMark.scroll(id, proxy) }
                    // `cx.scroll_into_view`: a no-op unless the node is in here.
                    .onChange(of: store.reveal.n) { _, _ in
                        withAnimation { proxy.scrollTo(store.reveal.id, anchor: .center) }
                    }
                }
            }
        case "checkbox":
            // watchOS has no checkbox style: a labelled toggle (read as a switch).
            Toggle(isOn: Binding(
                get: { n.value != 0 },
                set: { send(["t": "change", "id": id, "v": $0 ? 1 : 0]) }
            )) { Text(n.text).font(font(n)).foregroundStyle(fg) }
                .modifier(ControlEnvironment(rtl: store.rtl, lang: store.lang))
        case "radio_group":
            PrismRadioGroup(id: id, node: n, font: font(n), fg: fg, tint: n.color("color", dark: dark))
                .modifier(ControlEnvironment(rtl: store.rtl, lang: store.lang))
        case "overlay":
            // Takes no room; the content is presented in a sheet.
            PrismOverlay(id: id, node: n)
        case "stack":
            StackView(id: id, node: n)
        case "tabs":
            TabsView(id: id, node: n)
        default: // view, screen, tab
            placed(n.children.filter { store.nodes[$0]?.kind != "modal" })
                .sheet(isPresented: modalBinding(n)) {
                    if let m = modal(n) { NodeView(id: m) }
                }
        }
    }

    /// Children at the positions Rust's layout gave them.
    private func placed(_ children: [UInt64]) -> some View {
        ZStack(alignment: .topLeading) {
            Color.clear
            ForEach(children, id: \.self) { c in
                let f = store.nodes[c]?.frame ?? .zero
                if let sticky = store.nodes[c]?.sticky {
                    NodeView(id: c).offset(x: f.minX, y: f.minY).modifier(StickyShift(sticky: sticky)).zIndex(1)
                } else {
                    NodeView(id: c).offset(x: f.minX, y: f.minY)
                }
            }
            ForEach(store.ghosts.filter { $0.value.parent == id }.map(\.key), id: \.self) { g in
                if let ghost = store.ghosts[g] {
                    GhostView(ghost: ghost).offset(x: ghost.frame.minX, y: ghost.frame.minY)
                }
            }
        }
    }

    private func modal(_ n: Node) -> UInt64? { n.children.first { store.nodes[$0]?.kind == "modal" } }

    private func modalBinding(_ n: Node) -> Binding<Bool> {
        Binding(get: { modal(n) != nil }, set: { shown in
            // Swiped away: the modal's stack goes back to nothing.
            if !shown, let m = modal(n), let stack = store.nodes[m]?.children.first {
                send(["t": "back", "id": stack, "depth": 0])
            }
        })
    }
}

/// Where a `.sticky()` node sits in its scroll view (Rust's `sticky` op).
struct PrismSticky: Equatable {
    var scroll: UInt64
    var top: Double
    var inset: Double
    var max: Double

    init?(_ p: [String: Any]) {
        guard let scroll = (p["scroll"] as? NSNumber)?.uint64Value else { return nil }
        self.scroll = scroll
        top = (p["top"] as? NSNumber)?.doubleValue ?? 0
        inset = (p["inset"] as? NSNumber)?.doubleValue ?? 0
        max = (p["max"] as? NSNumber)?.doubleValue ?? 0
    }

    /// How far down it moves with the content scrolled by `y`.
    func offset(_ y: Double) -> Double { Swift.min(Swift.max(y + inset - top, 0), Swift.max(max, 0)) }
}

/// Scroll offsets of the scroll views that have sticky children: only the
/// sticky views observe them, so scrolling does not redraw the tree.
final class PrismScrollState: ObservableObject {
    static let shared = PrismScrollState()
    @Published var offsets: [UInt64: Double] = [:]
    var watched: Set<UInt64> = []

    func scrolled(_ id: UInt64, _ y: Double) {
        if watched.contains(id), offsets[id] != y { offsets[id] = y }
    }
}

/// Moves a sticky view down as its scroll view scrolls (its frame stays).
struct StickyShift: ViewModifier {
    @ObservedObject var scroll = PrismScrollState.shared
    let sticky: PrismSticky

    func body(content: Content) -> some View {
        content.offset(y: sticky.offset(scroll.offsets[sticky.scroll] ?? 0))
    }
}

/// Dev hook: `PRISM_SCROLL=<points>` scrolls scroll views that far down once
/// shown (the simulator has no Digital Crown input to script).
struct DevScrollMark: View {
    static let offset = ProcessInfo.processInfo.environment["PRISM_SCROLL"].flatMap(Double.init)
    let id: UInt64

    var body: some View {
        if let y = Self.offset {
            VStack(spacing: 0) {
                Color.clear.frame(width: 1, height: y)
                Color.clear.frame(width: 1, height: 1).id("prism-scroll-\(id)")
            }
        }
    }

    static func scroll(_ id: UInt64, _ proxy: ScrollViewProxy) {
        guard offset != nil else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { proxy.scrollTo("prism-scroll-\(id)", anchor: .top) }
    }
}

// MARK: Form controls

/// The app's direction and language for a native control (Rust already
/// mirrors the layout; this mirrors the control itself: a slider fills from
/// the right, text aligns to the right, dates read in the app's language).
struct ControlEnvironment: ViewModifier {
    let rtl: Bool
    let lang: String

    func body(content: Content) -> some View {
        content
            .environment(\.layoutDirection, rtl ? .rightToLeft : .leftToRight)
            .environment(\.locale, Self.locale(lang))
    }

    /// The device's locale, in the app's language (keeping the region).
    static func locale(_ lang: String) -> Locale {
        let current = Locale.current
        guard !lang.isEmpty, current.language.languageCode?.identifier != lang else { return current }
        return Locale(identifier: current.region.map { "\(lang)_\($0.identifier)" } ?? lang)
    }
}

/// `a11y.value` (e.g. "40 %") read out instead of the control's own value.
struct A11yValue: ViewModifier {
    let value: String?

    @ViewBuilder
    func body(content: Content) -> some View {
        if let value, !value.isEmpty { content.accessibilityValue(value) } else { content }
    }
}

/// A slider's config: `min max step` (step 0: continuous).
struct SliderRange {
    var min = 0.0, max = 1.0, step = 0.0

    init(_ config: String) {
        let v = config.split(separator: " ").map { Double($0) }
        if v.count > 0, let a = v[0] { min = a }
        if v.count > 1, let b = v[1] { max = Swift.max(b, min) }
        if v.count > 2, let s = v[2] { step = Swift.max(s, 0) }
    }

    /// Clamped and rounded to the step (`SliderConfig::snap`).
    func snap(_ value: Double) -> Double {
        let v = Swift.min(Swift.max(value, min), max)
        guard step > 0 else { return v }
        return Swift.min(Swift.max(min + ((v - min) / step).rounded() * step, min), max)
    }

    /// The Digital Crown / button step: the step, or a tenth of the range
    /// (`SliderConfig::a11y_step`). watchOS sliders always move by steps.
    var uiStep: Double { step > 0 ? step : Swift.max((max - min) / 10, .ulpOfOne) }
    var range: ClosedRange<Double> { min...(max > min ? max : min + uiStep) }
}

/// SwiftUI's slider: − / + buttons and the Digital Crown, adjustable by
/// VoiceOver natively.
struct PrismSlider: View {
    let id: UInt64
    let node: Node
    let tint: Color?

    var body: some View {
        let r = SliderRange(node.config)
        Slider(value: Binding(
            get: { Swift.min(Swift.max(node.value, r.min), r.max) },
            set: { v in
                let s = r.snap(v)
                guard s != node.value else { return }
                PrismStore.shared.nodes[id]?.value = s
                send(["t": "change", "id": id, "v": s])
            }
        ), in: r.range, step: r.uiStep)
            .tint(tint)
            .modifier(A11yValue(value: node.a11yValue))
    }
}

/// SwiftUI's picker as a navigation row (MenuPickerStyle is
/// `@available(watchOS, unavailable)`): the row opens the list of options.
/// Index -1 shows the placeholder.
struct PrismPicker: View {
    let id: UInt64
    let node: Node
    let font: Font
    let tint: Color?

    var body: some View {
        let options = node.config.isEmpty ? [] : node.config.components(separatedBy: "\n")
        let selected = Int(node.value)
        let chosen = options.indices.contains(selected) ? options[selected] : nil
        Picker(selection: Binding(
            get: { selected },
            set: { i in
                guard i != selected else { return }
                PrismStore.shared.nodes[id]?.value = Double(i)
                send(["t": "change", "id": id, "v": i])
            }
        )) {
            ForEach(Array(options.enumerated()), id: \.offset) { i, o in Text(o).tag(i) }
        } label: {
            // The row shows the label and the selected option: the
            // placeholder stands in while nothing is chosen.
            Text(chosen == nil ? node.placeholder : (node.label ?? "")).font(font)
                .foregroundStyle(chosen == nil ? .secondary : .primary)
        }
        .pickerStyle(.navigationLink)
        .tint(tint)
        .modifier(A11yValue(value: node.a11yValue ?? chosen))
    }
}

/// A date picker: a button showing the value (the compact form), opening
/// SwiftUI's DatePicker wheels (watchOS 10+) in a sheet. Inline, watchOS
/// draws the wheels at full size (CompactDatePickerStyle and
/// GraphicalDatePickerStyle are `@available(watchOS, unavailable)`).
/// `datetime` stacks the date and the time wheels. Values are Unix seconds,
/// shown in the device's time zone.
struct PrismDatePicker: View {
    let id: UInt64
    let node: Node
    let tint: Color?
    @State private var open = false
    @Environment(\.locale) private var locale

    var body: some View {
        let mode = node.config.split(separator: " ").first.map(String.init) ?? "date"
        let text = Self.format(Date(timeIntervalSince1970: node.value), mode, locale)
        let timeOnly = mode == "time" || mode == "timeofday"
        Button { open = true } label: {
            Text(text).lineLimit(1).minimumScaleFactor(0.6)
        }
        .tint(tint)
        .accessibilityValue(text)
        .sheet(isPresented: $open) {
            // The wheels share the screen (in a scroll view they collapse).
            VStack(spacing: 6) {
                if !timeOnly { wheels(.date).frame(maxHeight: .infinity) }
                if mode != "date" { wheels(.hourAndMinute).frame(maxHeight: .infinity) }
            }
            // A time of day is seconds since midnight: the same in every zone.
            .environment(\.timeZone, mode == "timeofday" ? TimeZone(identifier: "UTC")! : .current)
            .navigationTitle(node.label ?? "")
            .modifier(ControlEnvironment(rtl: PrismStore.shared.rtl, lang: PrismStore.shared.lang))
            .tint(tint)
        }
        .onAppear {
            // Dev hook: `PRISM_PRESENT=<label>` opens this picker's wheels.
            if let l = node.label, ProcessInfo.processInfo.environment["PRISM_PRESENT"] == l {
                DispatchQueue.main.asyncAfter(deadline: .now() + 2) { open = true }
            }
        }
    }

    @ViewBuilder
    private func wheels(_ components: DatePickerComponents) -> some View {
        let parts = node.config.split(separator: " ").map(String.init)
        let bounds = (1...2).map { i in
            parts.count > i ? Double(parts[i]).map { Date(timeIntervalSince1970: $0) } : nil
        }
        let selection = Binding<Date>(
            get: { Date(timeIntervalSince1970: PrismStore.shared.nodes[id]?.value ?? node.value) },
            set: { d in
                let v = d.timeIntervalSince1970
                guard v != PrismStore.shared.nodes[id]?.value else { return }
                PrismStore.shared.nodes[id]?.value = v
                send(["t": "change", "id": id, "v": v])
            }
        )
        let label = Text(node.label ?? "")
        Group {
            switch (bounds[0], bounds[1]) {
            case let (lo?, hi?) where lo <= hi:
                DatePicker(selection: selection, in: lo...hi, displayedComponents: components) { label }
            case let (lo?, _):
                DatePicker(selection: selection, in: lo..., displayedComponents: components) { label }
            case let (_, hi?):
                DatePicker(selection: selection, in: ...hi, displayedComponents: components) { label }
            default:
                DatePicker(selection: selection, displayedComponents: components) { label }
            }
        }
        .labelsHidden()
    }

    /// The value as the button shows it (and what it measures).
    static func format(_ date: Date, _ mode: String, _ locale: Locale) -> String {
        var style: Date.FormatStyle
        switch mode {
        case "time", "timeofday": style = .dateTime.hour().minute()
        case "datetime": style = .dateTime.day().month(.abbreviated).hour().minute()
        default: style = .dateTime.day().month(.abbreviated).year()
        }
        if mode == "timeofday", let utc = TimeZone(identifier: "UTC") { style.timeZone = utc }
        return date.formatted(style.locale(locale))
    }
}

/// A text input's config tokens (`prism_core::controls::InputConfig`).
struct InputTraits {
    var secure = false
    var keyboard = ""
    var returnKey = ""
    var capitalize = ""
    var autocorrect = true
    /// The most characters (`max=N`), if limited.
    var maxLength: Int?

    init(_ config: String) {
        // The first line: the next ones are suggestions (none on watchOS:
        // `textInputSuggestions` is `@available(watchOS, unavailable)`).
        let first = config.split(separator: "\n", omittingEmptySubsequences: false).first ?? ""
        for token in first.split(separator: " ") {
            let kv = token.split(separator: "=", maxSplits: 1).map(String.init)
            switch (kv[0], kv.count > 1 ? kv[1] : "") {
            case ("secure", _): secure = true
            case ("keyboard", let v): keyboard = v
            case ("return", let v): returnKey = v
            case ("capitalize", let v): capitalize = v
            case ("autocorrect", "off"): autocorrect = false
            case ("max", let v): maxLength = Int(v)
            default: break
            }
        }
    }

    /// SwiftUI has no `keyboardType` on watchOS (UIKeyboardType is
    /// API_UNAVAILABLE(watchos) in UITextInputTraits.h): the closest is
    /// WatchKit's text content type, which tunes the input sheet.
    /// Number and decimal have no equivalent.
    var contentType: WKTextContentType? {
        if secure { return .password }
        switch keyboard {
        case "email": return .emailAddress
        case "url": return .URL
        case "phone": return .telephoneNumber
        default: return nil
        }
    }

    var autocapitalization: TextInputAutocapitalization {
        switch capitalize {
        case "none": return .never
        case "words": return .words
        case "all": return .characters
        default: return .sentences
        }
    }

    var submitLabel: SubmitLabel {
        switch returnKey {
        case "done": return .done
        case "go": return .go
        case "next": return .next
        case "search": return .search
        case "send": return .send
        default: return .return
        }
    }
}

/// Text inputs and text areas. watchOS edits text in a system sheet
/// (dictation, scribble, keyboard); the field shows the value. TextEditor is
/// `@available(watchOS, unavailable)`, so a text area is a vertical
/// TextField (watchOS 9+) showing as many lines as its frame holds.
/// A text field without its platter (its node draws the box).
struct BareField: ViewModifier {
    let on: Bool
    func body(content: Content) -> some View {
        if on { content.textFieldStyle(.plain) } else { content }
    }
}

struct PrismTextField: View {
    @ObservedObject var store = PrismStore.shared
    let id: UInt64
    let node: Node
    let font: Font
    /// Reported to Rust: `.focus` / `.focus_within` refinements.
    @FocusState private var focused: Bool

    var body: some View {
        let traits = InputTraits(node.config)
        let text = Binding(get: { node.text }, set: { v in
            // InputConfig::max_length: longer text is cut.
            let v = traits.maxLength.map { String(v.prefix($0)) } ?? v
            if v != node.text { send(["t": "input", "id": id, "v": v]) }
        })
        HStack(spacing: 6) {
            // TextInput::leading_icon.
            if node.kind == "text_input", !node.icon.isEmpty {
                Image(systemName: node.icon).foregroundStyle(.secondary).accessibilityHidden(true)
            }
            field(traits, text)
        }
            .focused($focused)
            .onChange(of: store.focus.n) { _, _ in if store.focus.id == id { focused = true } }
            .onChange(of: store.blur) { _, _ in focused = false }
            .onAppear { if store.focus.id == id && store.focus.n > 0 { focused = true } }
            .font(font)
            .textContentType(traits.contentType)
            .textInputAutocapitalization(traits.secure ? .never : traits.autocapitalization)
            .autocorrectionDisabled(traits.secure || !traits.autocorrect)
            .submitLabel(traits.submitLabel)
            // Padding or `.border(0.)` (#14): the node's box, not the platter.
            .modifier(BareField(on: node.style["pad"] != nil || node.style["bnone"] as? Bool == true))
            .focused($focused)
            .onChange(of: focused) { _, on in send(["t": "state", "id": id, "s": "focus", "on": on]) }
    }

    @ViewBuilder
    private func field(_ traits: InputTraits, _ text: Binding<String>) -> some View {
        if node.kind == "text_area" {
            let line = UIFont.systemFont(ofSize: node.num("font", 17)).lineHeight
            let lines = max(Int((node.frame.height - TextAreaInset.vertical * 2) / max(line, 1)), 1)
            TextField(node.placeholder, text: text, axis: .vertical)
                .lineLimit(lines)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        } else if traits.secure {
            SecureField(node.placeholder, text: text)
                .onSubmit { send(["t": "submit", "id": id]) }
        } else {
            TextField(node.placeholder, text: text)
                .onSubmit { send(["t": "submit", "id": id]) }
        }
    }
}

/// The whole frame takes touches (a clickable card), except a layer's root.
struct HitShape: ViewModifier {
    let on: Bool

    @ViewBuilder
    func body(content: Content) -> some View {
        if on { content.contentShape(Rectangle()) } else { content }
    }
}

/// Radio buttons: one row per option, the chosen one checked. SwiftUI's
/// `RadioGroupPickerStyle` is `@available(watchOS, unavailable)`.
struct PrismRadioGroup: View {
    let id: UInt64
    let node: Node
    let font: Font
    let fg: Color
    let tint: Color?

    var body: some View {
        let lines = node.config.components(separatedBy: "\n")
        let options = Array(lines.dropFirst())
        let selected = Int(node.value)
        let layout = lines.first == "row" ? AnyLayout(HStackLayout(spacing: RadioChrome.spacing)) : AnyLayout(VStackLayout(alignment: .leading, spacing: 0))
        layout {
            ForEach(Array(options.enumerated()), id: \.offset) { i, o in
                Button {
                    guard i != selected else { return }
                    PrismStore.shared.nodes[id]?.value = Double(i)
                    send(["t": "change", "id": id, "v": i])
                } label: {
                    HStack(spacing: 6) {
                        Image(systemName: i == selected ? "largecircle.fill.circle" : "circle")
                            .foregroundStyle(tint ?? .accentColor)
                        Text(o).font(font).foregroundStyle(fg)
                    }
                    .frame(minHeight: RadioChrome.minHeight)
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(i == selected ? [.isSelected] : [])
                .accessibilityValue("\(i + 1) / \(options.count)")
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(node.label ?? "")
    }
}

/// A sheet, popover or dialog: on a watch all are full-screen sheets.
/// `fixed` cannot be swiped away; a swipe away is the overlay's dismissal.
struct PrismOverlay: View {
    @ObservedObject var store = PrismStore.shared
    let id: UInt64
    let node: Node

    var body: some View {
        let fixed = node.config.split(separator: " ").contains("fixed")
        Color.clear.frame(width: 0, height: 0)
            .sheet(isPresented: Binding(
                get: { node.value != 0 && !node.children.isEmpty },
                set: { shown in
                    if !shown, (store.nodes[id]?.value ?? 0) != 0 {
                        send(["t": "back", "id": id, "depth": 0])
                    }
                }
            )) {
                Color.clear
                    .overlay(alignment: .topLeading) {
                        if let c = node.children.first { NodeView(id: c) }
                    }
                    .modifier(ReportSize(event: ["t": "viewport", "id": id]))
                    .interactiveDismissDisabled(fixed)
            }
    }
}

/// Rounded corners clip their content; square views let it overflow unless
/// they clip (`clip`: CSS `overflow: hidden`).
struct RoundedClip: ViewModifier {
    let radius: Double
    var clip = false
    let shape: UnevenRoundedRectangle

    @ViewBuilder
    func body(content: Content) -> some View {
        if radius > 0 || clip { content.clipShape(shape) } else { content }
    }
}

// MARK: Box decorations (Style::shadows, border_sides, border_style,
// gradient, backdrop_blur): drawn like CSS draws them.

/// A dash pattern for a border `width` wide (prism_core `BorderStyle::dash`):
/// dashes 3w on, 2w off; dots are round caps 2w apart.
private func dashPattern(_ style: String?, _ width: Double) -> (dash: [CGFloat], cap: CGLineCap) {
    let w = max(width, 1)
    switch style {
    case "dashed": return ([3 * w, 2 * w], .butt)
    case "dotted": return ([0, 2 * w], .round)
    default: return ([], .butt)
    }
}

/// Background: backdrop material, color, gradient, then inner shadows.
struct BoxFill: View {
    let node: Node
    let dark: Bool

    var body: some View {
        let box = node.box()
        ZStack {
            let blur = node.num("blur")
            if blur > 0 {
                // A material blurs what is behind (watchOS 10+); the
                // background color tints it from above, like CSS.
                box.fill(blur < 10 ? .ultraThinMaterial : blur < 20 ? .thinMaterial : blur < 30 ? .regularMaterial : .thickMaterial)
            }
            if let bg = node.color("bg", dark: dark) { box.fill(bg) }
            if let g = node.style["grad"] as? [String: Any] { gradient(g, box) }
            // CSS paints the first shadow on top.
            ForEach(Array(shadows.enumerated().reversed()), id: \.offset) { _, sh in
                InnerShadow(shadow: sh, box: box, size: node.frame.size)
            }
        }
    }

    private var shadows: [PrismShadow] {
        (node.style["shadows"] as? [[String: Any]] ?? []).compactMap { PrismShadow($0, dark: dark) }.filter(\.inset)
    }

    @ViewBuilder
    private func gradient(_ g: [String: Any], _ box: UnevenRoundedRectangle) -> some View {
        let stops = (g["stops"] as? [[Any]] ?? []).compactMap { s -> Gradient.Stop? in
            guard s.count == 2, let c = PrismColor(s[0]), let at = (s[1] as? NSNumber)?.doubleValue else { return nil }
            return Gradient.Stop(color: dark ? c.dark : c.light, location: at)
        }
        let gradient = Gradient(stops: stops)
        if g["kind"] as? String == "radial" {
            // CSS: an ellipse the box's shape reaching the corners (0.5 is
            // the inscribed ellipse).
            box.fill(EllipticalGradient(gradient: gradient, center: .center, startRadiusFraction: 0, endRadiusFraction: 0.5 * 2.squareRoot()))
        } else {
            // The CSS gradient line (prism_core `Gradient::geometry`).
            let a = ((g["angle"] as? NSNumber)?.doubleValue ?? 180) * .pi / 180
            let (dx, dy) = (sin(a), -cos(a))
            let w = max(node.frame.width, 1), h = max(node.frame.height, 1)
            let half = (w * abs(dx) + h * abs(dy)) / 2
            let start = UnitPoint(x: (w / 2 - dx * half) / w, y: (h / 2 - dy * half) / h)
            let end = UnitPoint(x: (w / 2 + dx * half) / w, y: (h / 2 + dy * half) / h)
            box.fill(LinearGradient(gradient: gradient, startPoint: start, endPoint: end))
        }
    }
}

/// One `shadows` entry.
struct PrismShadow {
    var x, y, blur, spread: Double
    var color: Color
    var inset: Bool

    init?(_ v: [String: Any], dark: Bool) {
        guard let c = PrismColor(v["color"]) else { return nil }
        func n(_ k: String) -> Double { (v[k] as? NSNumber)?.doubleValue ?? 0 }
        x = n("x"); y = n("y"); blur = n("blur"); spread = n("spread")
        color = dark ? c.dark : c.light
        inset = v["inset"] as? Bool ?? false
    }
}

/// Drop shadows outside the box only (like CSS `box-shadow`): the box's
/// shape grown by the spread, offset and blurred, with the box cut out.
/// CSS blur radii are twice the Gaussian's: SwiftUI's `blur` takes half.
struct DropShadows: View {
    let node: Node
    let dark: Bool

    var body: some View {
        let shadows = (node.style["shadows"] as? [[String: Any]] ?? []).compactMap { PrismShadow($0, dark: dark) }.filter { !$0.inset }
        if !shadows.isEmpty {
            let box = node.box()
            ZStack {
                ForEach(Array(shadows.enumerated().reversed()), id: \.offset) { _, sh in
                    box.inset(by: -sh.spread).fill(sh.color)
                        .offset(x: sh.x, y: sh.y)
                        .blur(radius: sh.blur / 2)
                }
                box.fill(Color.black).blendMode(.destinationOut)
            }
            .compositingGroup()
            .allowsHitTesting(false)
        }
    }
}

/// An inner shadow: everything outside the box shrunk by the spread and
/// offset, blurred, kept inside the box.
struct InnerShadow: View {
    let shadow: PrismShadow
    let box: UnevenRoundedRectangle
    let size: CGSize

    var body: some View {
        let margin = shadow.blur + abs(shadow.x) + abs(shadow.y) + abs(shadow.spread) + 4
        ZStack {
            Rectangle().fill(shadow.color).padding(-margin)
            box.inset(by: shadow.spread).fill(Color.black)
                .offset(x: shadow.x, y: shadow.y)
                .blendMode(.destinationOut)
        }
        .compositingGroup()
        .blur(radius: shadow.blur / 2)
        .clipShape(box)
        .allowsHitTesting(false)
    }
}

/// The border: one outline (solid, dashed or dotted) or, with `sides`, each
/// side on its own (top, right, bottom, left; Rust mirrored them).
struct BoxBorder: View {
    let node: Node
    let dark: Bool

    var body: some View {
        let style = node.str("bstyle")
        if let sides = node.style["sides"] as? [[Any]], sides.count == 4 {
            let w = node.frame.width, h = node.frame.height
            ZStack {
                ForEach(0..<4, id: \.self) { i in
                    let width = (sides[i].first as? NSNumber)?.doubleValue ?? 0
                    if width > 0, let c = PrismColor(sides[i].count > 1 ? sides[i][1] : nil) {
                        let (dash, cap) = dashPattern(style, width)
                        let half = width / 2
                        Path { p in
                            switch i {
                            case 0: p.move(to: CGPoint(x: 0, y: half)); p.addLine(to: CGPoint(x: w, y: half))
                            case 1: p.move(to: CGPoint(x: w - half, y: 0)); p.addLine(to: CGPoint(x: w - half, y: h))
                            case 2: p.move(to: CGPoint(x: 0, y: h - half)); p.addLine(to: CGPoint(x: w, y: h - half))
                            default: p.move(to: CGPoint(x: half, y: 0)); p.addLine(to: CGPoint(x: half, y: h))
                            }
                        }
                        .stroke(dark ? c.dark : c.light, style: StrokeStyle(lineWidth: width, lineCap: cap, dash: dash))
                    }
                }
            }
            .allowsHitTesting(false)
        } else if let c = node.color("bc", dark: dark) {
            let width = node.num("bw", 1)
            let (dash, cap) = dashPattern(style, width)
            node.box().strokeBorder(c, style: StrokeStyle(lineWidth: width, lineCap: cap, dash: dash))
        }
    }
}

// MARK: Interaction states (`.active(|s| …)`)

/// Reports presses of a node Rust has `active` refinements for: the node
/// becomes a plain button, whose pressed state SwiftUI tracks (and cancels
/// when the crown or a finger scrolls). Its action is the node's click.
/// watchOS has no pointer (`onHover` is unavailable) and no keyboard focus
/// on custom views: only presses, and text fields' focus, are reported.
struct PressReport: ViewModifier {
    let id: UInt64
    let on: Bool
    let clickable: Bool

    @ViewBuilder
    func body(content: Content) -> some View {
        if on {
            Button { if clickable { send(["t": "click", "id": id]) } } label: { content }
                .buttonStyle(PressStyle(id: id))
        } else {
            content
        }
    }
}

struct PressStyle: ButtonStyle {
    let id: UInt64

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .onChange(of: configuration.isPressed) { _, pressed in
                send(["t": "state", "id": id, "s": "active", "on": pressed])
            }
    }
}

/// Pull to refresh (a "clickable" scroll view): the app sets the value
/// back to 0 when done.
struct Refresh: ViewModifier {
    let id: UInt64
    let on: Bool

    @ViewBuilder
    func body(content: Content) -> some View {
        if on {
            content.refreshable {
                send(["t": "click", "id": id])
                for _ in 0..<300 {
                    try? await Task.sleep(nanoseconds: 100_000_000)
                    if (PrismStore.shared.nodes[id]?.value ?? 0) == 0 { break }
                }
            }
        } else {
            content
        }
    }
}

/// Alerts, long-press menus, the photo picker and the share sheet.
struct Presented: ViewModifier {
    @ObservedObject var store = PrismStore.shared
    @State private var photos: [PhotosPickerItem] = []

    func body(content: Content) -> some View {
        content
            .alert(store.alert?["title"] as? String ?? "", isPresented: Binding(
                get: { store.alert != nil }, set: { if !$0 { store.alert = nil } }
            )) {
                ForEach(Array(actions.enumerated()), id: \.offset) { i, a in
                    Button(a["title"] as? String ?? "", role: role(a["role"] as? String)) {
                        send(["t": "alert", "id": store.alert?["id"] ?? 0, "index": i])
                        store.alert = nil
                    }
                }
            } message: {
                Text(store.alert?["message"] as? String ?? "")
            }
            .confirmationDialog(menu?["title"] as? String ?? "", isPresented: Binding(
                get: { store.menuNode != nil }, set: { if !$0 { store.menuNode = nil } }
            ), titleVisibility: .hidden) {
                ForEach(Array(menuActions.enumerated()), id: \.offset) { _, a in
                    Button(a["title"] as? String ?? "", role: role(a["role"] as? String)) {
                        send(["t": "menu", "id": a["id"] ?? 0])
                    }
                    .disabled((a["enabled"] as? Bool) == false)
                }
            }
            .sheet(isPresented: Binding(get: { store.picker != nil }, set: { if !$0 { cancelPicker() } })) {
                PhotosPicker(selection: $photos, maxSelectionCount: store.picker.map { $0.limit > 0 ? $0.limit : nil } ?? nil,
                             matching: .images) { Label("Photos", systemImage: "photo") }
                    .onChange(of: photos) { _, items in finishPicker(items) }
            }
            .sheet(isPresented: Binding(get: { store.share != nil }, set: { if !$0 { store.share = nil } })) {
                if let url = store.share?.url.flatMap(URL.init(string:)) {
                    ShareLink(item: url, message: store.share?.text.map { Text($0) })
                } else {
                    ShareLink(item: store.share?.text ?? "")
                }
            }
    }

    private var actions: [[String: Any]] { store.alert?["actions"] as? [[String: Any]] ?? [] }
    private var menu: [String: Any]? { store.menuNode.flatMap { store.nodes[$0]?.menu } }

    /// Actions of the menu, submenus flattened.
    private var menuActions: [[String: Any]] {
        func flatten(_ items: [[String: Any]]) -> [[String: Any]] {
            items.flatMap { i -> [[String: Any]] in
                switch i["t"] as? String {
                case "act": return [i]
                case "sub": return flatten((i["menu"] as? [String: Any])?["items"] as? [[String: Any]] ?? [])
                default: return []
                }
            }
        }
        return flatten(menu?["items"] as? [[String: Any]] ?? [])
    }

    private func role(_ r: String?) -> ButtonRole? {
        r == "destructive" ? .destructive : r == "cancel" ? .cancel : nil
    }

    private func cancelPicker() {
        guard let p = store.picker else { return }
        store.picker = nil
        "cancelled".withCString { prismReplyError(p.id, false, $0) }
    }

    /// Copies the picked photos where the app can read them.
    private func finishPicker(_ items: [PhotosPickerItem]) {
        guard let p = store.picker, !items.isEmpty else { return }
        store.picker = nil
        photos = []
        Task {
            var lines: [String] = []
            let dir = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("prism-picked")
            try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            for item in items {
                guard let data = try? await item.loadTransferable(type: Data.self) else { continue }
                let type = item.supportedContentTypes.first
                let file = dir.appendingPathComponent(UUID().uuidString).appendingPathExtension(type?.preferredFilenameExtension ?? "jpg")
                guard (try? data.write(to: file)) != nil else { continue }
                lines.append([file.path, file.lastPathComponent, type?.preferredMIMEType ?? "image/jpeg", String(data.count)].joined(separator: "\t"))
            }
            await MainActor.run {
                if lines.isEmpty { "cancelled".withCString { prismReplyError(p.id, false, $0) } }
                else { lines.joined(separator: "\n").withCString { prismReplyFiles(p.id, $0) } }
            }
        }
    }
}

@_cdecl("prism_watch_pick_images")
public func prismWatchPickImagesHost(_ id: UInt64, _ limit: UInt32) {
    DispatchQueue.main.async { PrismStore.shared.picker = (id, Int(limit)) }
}

@_cdecl("prism_watch_share")
public func prismWatchShareHost(_ text: UnsafePointer<CChar>?, _ url: UnsafePointer<CChar>?) {
    let t = text.map { String(cString: $0) }
    let u = url.map { String(cString: $0) }
    DispatchQueue.main.async { PrismStore.shared.share = (t, u) }
}

/// Reports the size SwiftUI gives a container's content, for Rust's layout.
struct ReportSize: ViewModifier {
    let event: [String: Any]

    func body(content: Content) -> some View {
        content.background(GeometryReader { g in
            Color.clear
                .onAppear { report(g.size, g.safeAreaInsets) }
                .onChange(of: g.size) { _, size in report(size, g.safeAreaInsets) }
                .onChange(of: g.safeAreaInsets) { _, safe in report(g.size, safe) }
        })
    }

    /// `safe`: what the screen's corners and the clock cover around it
    /// (top, right, bottom, left; `cx.safe_area()`, #28).
    private func report(_ size: CGSize, _ safe: EdgeInsets) {
        var e = event
        e["w"] = size.width
        e["h"] = size.height
        e["safe"] = [safe.top, safe.trailing, safe.bottom, safe.leading]
        send(e)
    }
}

/// A navigation stack: the first screen is the root, the others are pushed.
struct StackView: View {
    @ObservedObject var store = PrismStore.shared
    @Environment(\.colorScheme) private var scheme
    let id: UInt64
    let node: Node

    var body: some View {
        let screens = node.children
        NavigationStack(path: Binding(
            get: { Array(screens.dropFirst()) },
            set: { path in
                // Back (swipe or button): tell the app how deep it is now.
                if path.count + 1 < screens.count { send(["t": "back", "id": id, "depth": path.count + 1]) }
            }
        )) {
            if let first = screens.first {
                screen(first).navigationDestination(for: UInt64.self) { screen($0) }
            }
        }
        // The router's tint or the app's accent (#30): Back and bar buttons.
        .tint(node.color("tint", dark: scheme == .dark))
    }

    private func screen(_ s: UInt64) -> some View {
        Color.clear
            .overlay(alignment: .topLeading) { NodeView(id: s) }
            .modifier(ReportSize(event: ["t": "viewport", "id": id]))
            .navigationTitle(store.nodes[s]?.text ?? "")
            // No navigation bar: for the whole stack (value flag 2,
            // `Router::header(false)`) or a bare screen (value 1). Swiping
            // back still works; the larger area is reported as the viewport.
            .toolbar(barHidden(s) ? .hidden : .automatic, for: .navigationBar)
            // The screen's bar buttons (#30).
            .modifier(BarButtons(screen: s, config: store.nodes[s]?.config ?? ""))
    }

    private func barHidden(_ s: UInt64) -> Bool {
        (Int(node.value) & 2) != 0 || store.nodes[s]?.value == 1
    }
}

/// Tabs: pages scrolled with the Digital Crown.
struct TabsView: View {
    @ObservedObject var store = PrismStore.shared
    @Environment(\.colorScheme) private var scheme
    let id: UInt64
    let node: Node

    var body: some View {
        TabView(selection: Binding(
            get: { Int(node.value) },
            set: { send(["t": "change", "id": id, "v": $0]) }
        )) {
            ForEach(Array(node.children.enumerated()), id: \.element) { i, tab in
                Color.clear
                    .overlay(alignment: .topLeading) { NodeView(id: tab) }
                    .modifier(ReportSize(event: ["t": "viewport", "id": id]))
                    .tag(i)
            }
        }
        .tabViewStyle(.verticalPage)
        // The router's tint or the app's accent (#30).
        .tint(node.color("tint", dark: scheme == .dark))
    }
}

// --- Bar buttons (#30) ---

/// A screen's bar buttons as toolbar items, before and after the title.
/// Config: `large`/`inline` (watchOS has one title size), then one button
/// per line, `L|T`, icon, title and label separated by tabs. A tap reports
/// `change` on the screen with the button's index.
struct BarButtons: ViewModifier {
    let screen: UInt64
    let config: String

    struct Item {
        let index: Int
        let leading: Bool
        let icon: String
        let title: String
        let label: String
    }

    static func items(_ config: String) -> [Item] {
        config.split(separator: "\n", omittingEmptySubsequences: false).dropFirst().enumerated().map { i, line in
            let parts = line.split(separator: "\t", omittingEmptySubsequences: false).map(String.init)
            let part = { (k: Int) in k < parts.count ? parts[k] : "" }
            return Item(index: i, leading: part(0) == "L", icon: part(1), title: part(2), label: part(3))
        }
    }

    func body(content: Content) -> some View {
        let items = Self.items(config)
        if items.isEmpty {
            content
        } else {
            content.toolbar {
                ToolbarItemGroup(placement: .topBarLeading) {
                    ForEach(items.filter { $0.leading }, id: \.index) { button($0) }
                }
                ToolbarItemGroup(placement: .topBarTrailing) {
                    ForEach(items.filter { !$0.leading }, id: \.index) { button($0) }
                }
            }
        }
    }

    private func button(_ item: Item) -> some View {
        let spoken = [item.label, item.title, item.icon].first { !$0.isEmpty } ?? ""
        return Button {
            send(["t": "change", "id": screen, "v": item.index])
        } label: {
            if !item.icon.isEmpty {
                Image(systemName: item.icon)
            } else {
                Text(item.title)
            }
        }
        .accessibilityLabel(spoken)
    }
}

/// An app asset (`assets/` in the bundle) or a remote image.
struct Picture: View {
    let source: String
    let fit: Bool

    var body: some View {
        if source.hasPrefix("data:"), let image = Self.dataImage(source) {
            Image(uiImage: image).resizable().aspectRatio(contentMode: fit ? .fit : .fill)
        } else if source.contains("\n") {
            // A remote picture with headers (`Picture::header`).
            AuthorizedPicture(source: source, fit: fit)
        } else if source.hasPrefix("http://") || source.hasPrefix("https://"), let url = URL(string: source) {
            AsyncImage(url: url) { $0.resizable().aspectRatio(contentMode: fit ? .fit : .fill) } placeholder: { Color.gray.opacity(0.2) }
        } else if let image = UIImage(contentsOfFile: source.hasPrefix("/") ? source : Bundle.main.bundlePath + "/assets/" + source) {
            Image(uiImage: image).resizable().aspectRatio(contentMode: fit ? .fit : .fill)
        } else {
            Color.gray.opacity(0.2)
        }
    }

    /// Bytes in memory (`picture_data`): a base64 or percent-encoded `data:` URL.
    static func dataImage(_ source: String) -> UIImage? {
        guard let comma = source.firstIndex(of: ",") else { return nil }
        let meta = source[source.index(source.startIndex, offsetBy: 5)..<comma]
        let payload = String(source[source.index(after: comma)...])
        let data = meta.hasSuffix(";base64")
            ? Data(base64Encoded: payload, options: .ignoreUnknownCharacters)
            : payload.removingPercentEncoding.map { Data($0.utf8) }
        return data.flatMap(UIImage.init(data:))
    }
}

/// A remote picture loaded with the headers it carries (`Picture::header`):
/// the URL, then one `name: value` line per header. `AsyncImage` cannot send
/// headers, so this loads it with a `URLRequest`, outside the URL cache.
struct AuthorizedPicture: View {
    let source: String
    let fit: Bool
    @State private var image: UIImage?

    var body: some View {
        Group {
            if let image {
                Image(uiImage: image).resizable().aspectRatio(contentMode: fit ? .fit : .fill)
            } else {
                Color.gray.opacity(0.2)
            }
        }
        .task(id: source) { image = await Self.load(source) }
    }

    /// No cookies, no cache on disk: the images are private.
    static let session = URLSession(configuration: .ephemeral)

    static func load(_ source: String) async -> UIImage? {
        let lines = source.components(separatedBy: "\n")
        guard let url = URL(string: lines[0]) else { return nil }
        var request = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData)
        for line in lines.dropFirst() {
            guard let colon = line.firstIndex(of: ":") else { continue }
            let name = line[..<colon].trimmingCharacters(in: .whitespaces)
            let value = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
            request.setValue(value, forHTTPHeaderField: name)
        }
        guard let (data, response) = try? await session.data(for: request),
              (response as? HTTPURLResponse).map({ (200..<300).contains($0.statusCode) }) ?? false
        else { return nil }
        return UIImage(data: data)
    }
}

private func swiftWeight(_ w: Int) -> Font.Weight {
    w >= 700 ? .bold : w >= 600 ? .semibold : w >= 500 ? .medium : .regular
}

private func textAlign(_ a: String?) -> TextAlignment {
    a == "center" ? .center : a == "right" ? .trailing : .leading
}

private func frameAlign(_ a: String?) -> Alignment {
    a == "center" ? .center : a == "right" ? .trailing : .leading
}

// MARK: App

struct PrismRootView: View {
    @ObservedObject var store = PrismStore.shared
    @Environment(\.scenePhase) var phase

    var body: some View {
        Color.clear
            .overlay(alignment: .topLeading) {
                if let root = store.root { NodeView(id: root) } else { ProgressView() }
            }
            // Runtime layers (the toaster): over the app, touches go through
            // where they have no child. (Under a sheet that is open: sheets are
            // presented above the root view.)
            .overlay(alignment: .topLeading) {
                ForEach(store.layers, id: \.self) { l in
                    let f = store.nodes[l]?.frame ?? .zero
                    NodeView(id: l).offset(x: f.minX, y: f.minY)
                }
            }
            .modifier(ReportSize(event: ["t": "resize"]))
            .modifier(Presented())
            .onChange(of: phase) { _, p in
                send(["t": "lifecycle", "state": p == .active ? "active" : p == .inactive ? "inactive" : "background"])
            }
        .onAppear {
            // Prism's styles are phone-sized: zoom by the watch's body text
            // size over Prism's (17 pt), like tvOS.
            let bounds = WKInterfaceDevice.current().screenBounds
            let body = UIFont.preferredFont(forTextStyle: .body).pointSize
            prismReady(bounds.width, bounds.height, WKInterfaceDevice.current().screenScale, body / 17,
                       UIFontMetrics.default.scaledValue(for: 17) / 17)
            snapshotIfAsked(size: bounds.size)
        }
    }
}

/// Dev hook: `PRISM_SNAPSHOT=/abs/out.png` renders the tree to a PNG after
/// `PRISM_DELAY` seconds (default 2), then quits. (Navigation and tab
/// containers render their current content; ImageRenderer cannot draw
/// sliders, text fields, toggles and spinners, which are platform views on
/// watchOS: screenshot the simulator for those.)
/// `PRISM_CLICK="+1|Save"` presses buttons by title first (it also works
/// without a snapshot).
@MainActor
private func snapshotIfAsked(size: CGSize) {
    let env = ProcessInfo.processInfo.environment
    let path = env["PRISM_SNAPSHOT"]
    guard path != nil || env["PRISM_CLICK"] != nil else { return }
    let delay = Double(env["PRISM_DELAY"] ?? "") ?? 2
    for (i, title) in (env["PRISM_CLICK"] ?? "").split(separator: "|").enumerated() {
        DispatchQueue.main.asyncAfter(deadline: .now() + delay + 0.3 * Double(i)) {
            let store = PrismStore.shared
            // A native button, or a clickable view containing that text.
            let wanted = String(title)
            func titled(_ c: UInt64) -> Bool { store.nodes[c]?.text == wanted }
            var target: UInt64? = nil
            for (id, n) in store.nodes {
                if (n.kind == "button" && n.text == wanted) || (n.clickable && n.children.contains(where: titled)) {
                    target = id
                    break
                }
            }
            if let id = target {
                send(["t": "click", "id": id])
            } else {
                NSLog("prism-watchos: PRISM_CLICK: nothing titled %@", String(title))
            }
        }
    }
    let clicks = Double((env["PRISM_CLICK"] ?? "").split(separator: "|").count)
    guard let path else { return }
    DispatchQueue.main.asyncAfter(deadline: .now() + delay + 0.3 * clicks + 0.7) {
        let store = PrismStore.shared
        store.snapshotting = true
        // ImageRenderer cannot draw TabView or NavigationStack: render what
        // they show (the selected tab, the top screen).
        func visible(_ id: UInt64) -> UInt64 {
            guard let n = store.nodes[id] else { return id }
            switch n.kind {
            case "tabs" where Int(n.value) < n.children.count: return visible(n.children[Int(n.value)])
            case "stack" where !n.children.isEmpty: return visible(n.children[n.children.count - 1])
            case "tab" where n.children.count == 1: return visible(n.children[0])
            default: return id
            }
        }
        // The whole content (scrolled parts included), screen-wide.
        let shown = store.root.map(visible) ?? 0
        let first = store.nodes[shown]?.children.first.flatMap { store.nodes[$0] }
        let full = max(size.height, (first.map { $0.kind == "scroll" ? $0.content.height : $0.frame.height } ?? 0) + (first?.frame.minY ?? 0))
        // ImageRenderer fails on very tall images (long screens): render a
        // window of at most 1200 points, starting at PRISM_SCROLL.
        let top = min(max(0, CGFloat(DevScrollMark.offset ?? 0)), max(0, full - size.height))
        let height = min(full - top, 1200)
        let content = Color.clear
            .overlay(alignment: .topLeading) { NodeView(id: shown).offset(y: -top) }
            .frame(width: size.width, height: height, alignment: .topLeading)
            .clipped()
        .background(Color.black)
        .environment(\.colorScheme, .dark)
        let renderer = ImageRenderer(content: content)
        renderer.scale = WKInterfaceDevice.current().screenScale
        if let image = renderer.uiImage, let png = image.pngData() {
            try? png.write(to: URL(fileURLWithPath: path))
            NSLog("prism-watchos: snapshot written to %@", path)
        } else {
            NSLog("prism-watchos: snapshot failed")
        }
        exit(0)
    }
}

// MARK: Background refresh (WatchKit: BGTaskScheduler is unavailable on watchOS)

final class PrismWatchDelegate: NSObject, WKApplicationDelegate {
    static var running: [String: WKRefreshBackgroundTask] = [:]

    func handle(_ backgroundTasks: Set<WKRefreshBackgroundTask>) {
        for task in backgroundTasks {
            if let refresh = task as? WKApplicationRefreshBackgroundTask,
               let id = (refresh.userInfo as? [String: Any])?["prism"] as? String {
                PrismWatchDelegate.running[id] = refresh
                id.withCString { prismRustBackgroundRun($0) }
            } else {
                task.setTaskCompletedWithSnapshot(false)
            }
        }
    }

    // Remote push (PrismPush.swift): the token, and silent pushes.
    func didRegisterForRemoteNotifications(withDeviceToken deviceToken: Data) {
        prismHex(deviceToken).withCString { prismRustPushToken($0, nil) }
    }

    func didFailToRegisterForRemoteNotificationsWithError(_ error: Error) {
        error.localizedDescription.withCString { prismRustPushToken(nil, $0) }
    }

    func didReceiveRemoteNotification(_ userInfo: [AnyHashable: Any],
                                      fetchCompletionHandler completionHandler: @escaping (WKBackgroundFetchResult) -> Void) {
        prismWatchRemoteNotification(userInfo, completionHandler)
    }
}

/// Asks watchOS to wake the app for task `id` in `seconds` (at the earliest).
@_cdecl("prism_watch_bg_schedule")
public func prismWatchBgSchedule(_ id: UnsafePointer<CChar>, _ seconds: Double) {
    let id = String(cString: id)
    WKApplication.shared().scheduleBackgroundRefresh(
        withPreferredDate: Date(timeIntervalSinceNow: seconds),
        userInfo: ["prism": id] as NSDictionary
    ) { error in
        if let error { NSLog("prism: background refresh %@ not scheduled: %@", id, "\(error)") }
    }
}

@_cdecl("prism_watch_bg_complete")
public func prismWatchBgComplete(_ id: UnsafePointer<CChar>) {
    PrismWatchDelegate.running.removeValue(forKey: String(cString: id))?.setTaskCompletedWithSnapshot(false)
}

struct PrismWatchApp: App {
    @WKApplicationDelegateAdaptor(PrismWatchDelegate.self) var delegate

    var body: some Scene {
        WindowGroup { PrismRootView() }
    }
}

/// Called from Rust's `main`: runs the SwiftUI app (never returns).
// MARK: 3D scenes

/// A 3D scene (`scene3d`): SceneKit's SwiftUI view showing a
/// `PrismSceneRenderer` (Bridge/PrismScene.swift, the player every Apple
/// platform shares). A drag turns the camera; a tap reports what it hit.
struct PrismScene3d: View {
    let id: UInt64
    let node: Node
    let dark: Bool
    @ObservedObject private var host: PrismSceneHost
    /// The drag's translation so far, while a finger is down.
    @State private var last: CGSize?

    init(id: UInt64, node: Node, dark: Bool) {
        self.id = id
        self.node = node
        self.dark = dark
        host = PrismSceneHost.of(id)
    }

    var body: some View {
        let settings = host.renderer.settings
        let interactive = settings.orbit?.rotate == true || settings.tap
        SceneView(scene: host.renderer.scene, pointOfView: host.renderer.cameraNode,
                  options: host.animating ? [.rendersContinuously] : [], antialiasingMode: .multisampling4X)
            .gesture(DragGesture(minimumDistance: 0)
                .onChanged { g in
                    let prev = last ?? .zero
                    if last == nil { host.renderer.player.pointerDown() }
                    last = g.translation
                    host.renderer.player.drag(Float(g.translation.width - prev.width),
                                              Float(g.translation.height - prev.height))
                    host.redraw()
                }
                .onEnded { g in
                    last = nil
                    host.renderer.player.pointerUp()
                    let moved = hypot(g.translation.width, g.translation.height)
                    if moved < CGFloat(PrismScenePlayer.tapSlop), settings.tap {
                        var event = host.renderer.hit(x: Float(g.startLocation.x), y: Float(g.startLocation.y)).fields
                        event["t"] = "scene_tap"
                        event["id"] = id
                        send(event)
                    }
                    host.redraw()
                }, including: interactive ? .all : .subviews)
            .onAppear { host.show(size: node.frame.size, dark: dark) }
            .onChange(of: node.sceneVersion) { host.show(size: node.frame.size, dark: dark) }
            .onChange(of: node.frame.size) { host.show(size: node.frame.size, dark: dark) }
            .onChange(of: dark) { host.show(size: node.frame.size, dark: dark) }
            .onDisappear { host.stop() }
    }
}

/// A scene's player and SceneKit graph, kept by node id from its first
/// update to its `destroy` (updates are diffs: none may be missed). Moves
/// the SceneKit nodes on changes, and every 60th of a second while
/// something animates (no timer otherwise).
final class PrismSceneHost: ObservableObject {
    private static var hosts: [UInt64: PrismSceneHost] = [:]

    /// The host of scene node `id`.
    static func of(_ id: UInt64) -> PrismSceneHost {
        if let h = hosts[id] { return h }
        let h = PrismSceneHost()
        hosts[id] = h
        return h
    }

    /// Scene node `id` is gone.
    static func forget(_ id: UInt64) {
        hosts[id]?.stop()
        hosts[id] = nil
    }

    let renderer = PrismSceneRenderer()
    @Published private(set) var animating = false
    /// Counts the updates (redraws the view: its gestures follow the scene).
    @Published private(set) var version = 0
    private var size = CGSize.zero
    private var dark = false
    private var timer: Timer?
    /// Texture sources loading or loaded, for every scene.
    private static var requested = Set<String>()

    init() {
        renderer.paintsBackground = true
        renderer.changed = { [weak self] in self?.redraw() }
    }

    /// Applies a `SceneUpdate` (the `scene` op's `u`).
    func apply(_ update: Any?) {
        guard let d = update as? [String: Any], let update = PrismSceneUpdate(d) else { return }
        renderer.apply(update)
        load(update.textures)
        version += 1
        redraw()
    }

    func show(size: CGSize, dark: Bool) {
        self.size = size
        self.dark = dark
        redraw()
    }

    func redraw() {
        guard size.width > 0, size.height > 0 else { return }
        let moving = renderer.tick(width: Float(size.width), height: Float(size.height), dark: dark)
        if moving != animating { animating = moving }
        if moving, timer == nil {
            timer = Timer.scheduledTimer(withTimeInterval: 1.0 / 60, repeats: true) { [weak self] _ in self?.redraw() }
        } else if !moving {
            stop()
        }
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }

    /// Textures load like pictures (`Picture`): `data:` URLs, remote URLs
    /// (with headers), the bundle's assets. Embedded images (`p3d:`) come
    /// from the blob store.
    private func load(_ sources: [String]) {
        for src in sources where !src.hasPrefix("p3d:") && !Self.requested.contains(src) {
            Self.requested.insert(src)
            Task { @MainActor in
                if let image = await Self.image(src) { PrismSceneTextures.shared.add(src, image) }
            }
        }
    }

    @MainActor private static func image(_ src: String) async -> UIImage? {
        if src.hasPrefix("data:") { return Picture.dataImage(src) }
        if src.contains("\n") { return await AuthorizedPicture.load(src) }
        if src.hasPrefix("http://") || src.hasPrefix("https://") {
            guard let url = URL(string: src), let (data, _) = try? await URLSession.shared.data(from: url) else { return nil }
            return UIImage(data: data)
        }
        return UIImage(contentsOfFile: src.hasPrefix("/") ? src : Bundle.main.bundlePath + "/assets/" + src)
    }
}

@_cdecl("prism_watch_main")
public func prismWatchMain() {
    prismRegisterFonts()
    PrismWatchApp.main()
}
