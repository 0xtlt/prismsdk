// watchOS host (compiled with platform/apple/Shared and Bridge: device
// APIs and notifications come from there). watchOS apps have no UIKit views (UIView.h:
// API_UNAVAILABLE(watchos)), so Prism's tree is drawn with SwiftUI.
//
// Rust runs the app and sends patches as JSON (`prism_watch_apply`); this
// file keeps the node tree and renders it. Events go back as JSON
// (`prism_watch_event`). Layout is SwiftUI's: rows/columns, padding, sizes,
// grow, backgrounds and borders map to stacks and modifiers.
import PhotosUI
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
    let view: AnyView = kind == "toggle"
        ? AnyView(Toggle("", isOn: .constant(true)).labelsHidden().fixedSize())
        : AnyView(ProgressView(value: 0.5).frame(width: 100).fixedSize())
    let size = ImageRenderer(content: view).uiImage?.size ?? .zero
    controlSizes[kind] = size
    return size
}

@_cdecl("prism_watch_measure")
public func prismWatchMeasure(_ kind: UnsafePointer<CChar>, _ text: UnsafePointer<CChar>, _ font: Double,
                              _ weight: Int32, _ maxWidth: Double,
                              _ outW: UnsafeMutablePointer<Double>, _ outH: UnsafeMutablePointer<Double>) {
    let kind = String(cString: kind)
    let size = font > 0 ? font : 17
    let uiFont = UIFont.systemFont(ofSize: size, weight: uiWeight(weight))
    func textSize(_ s: String) -> CGSize {
        let bounds = (s.isEmpty ? " " : s as NSString).boundingRect(
            with: CGSize(width: maxWidth.isFinite ? maxWidth : .greatestFiniteMagnitude, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: [.font: uiFont], context: nil)
        return CGSize(width: ceil(bounds.width), height: ceil(bounds.height))
    }
    var result: CGSize
    switch kind {
    case "text", "button", "timer":
        result = textSize(String(cString: text))
        if String(cString: text).isEmpty && kind != "timer" { result.width = 0 }
    case "text_input":
        let line = textSize("Ag")
        result = CGSize(width: maxWidth.isFinite ? maxWidth : 120, height: line.height + 16)
    case "toggle", "progress":
        result = MainActor.assumeIsolated { controlSize(kind) }
        if kind == "progress", maxWidth.isFinite { result.width = maxWidth }
    case "image":
        let config = UIImage.SymbolConfiguration(pointSize: size)
        result = UIImage(systemName: String(cString: text), withConfiguration: config)?.size ?? CGSize(width: size, height: size)
    default:
        result = .zero
    }
    outW.pointee = Double(result.width)
    outH.pointee = Double(result.height)
}

// MARK: Tree

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

    func num(_ k: String, _ d: Double = 0) -> Double { (style[k] as? NSNumber)?.doubleValue ?? d }
    func str(_ k: String) -> String? { style[k] as? String }
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

    func apply(_ json: String) {
        guard let data = json.data(using: .utf8),
              let patches = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]] else { return }
        var nodes = self.nodes
        var root = self.root
        for p in patches {
            let id = (p["id"] as? NSNumber)?.uint64Value ?? 0
            switch p["op"] as? String {
            case "create": nodes[id] = Node(kind: p["kind"] as? String ?? "view")
            case "destroy": nodes[id] = nil
            case "root": root = id
            case "children": nodes[id]?.children = (p["children"] as? [NSNumber] ?? []).map(\.uint64Value)
            case "text": nodes[id]?.text = p["text"] as? String ?? ""
            case "placeholder": nodes[id]?.placeholder = p["text"] as? String ?? ""
            case "icon": nodes[id]?.icon = p["icon"] as? String ?? ""
            case "value": nodes[id]?.value = (p["value"] as? NSNumber)?.doubleValue ?? 0
            case "clickable": nodes[id]?.clickable = p["on"] as? Bool ?? false
            case "display": nodes[id]?.hidden = p["hidden"] as? Bool ?? false
            case "a11y":
                nodes[id]?.label = p["label"] as? String
                nodes[id]?.a11yHidden = p["hidden"] as? Bool ?? false
            case "style": nodes[id]?.style = p["s"] as? [String: Any] ?? [:]
            case "frame":
                if let f = p["f"] as? [Double], f.count == 4 { nodes[id]?.frame = CGRect(x: f[0], y: f[1], width: f[2], height: f[3]) }
            case "content":
                if let c = p["size"] as? [Double], c.count == 2 { nodes[id]?.content = CGSize(width: c[0], height: c[1]) }
            case "menu": nodes[id]?.menu = p["menu"] as? [String: Any]
            case "swipe":
                nodes[id]?.swipeLeft = p["left"] as? Bool ?? false
                nodes[id]?.swipeRight = p["right"] as? Bool ?? false
            case "scrollev": nodes[id]?.scrollEvents = p["on"] as? Bool ?? false
            case "alert": alert = p["alert"] as? [String: Any]
            default: break
            }
        }
        self.nodes = nodes
        self.root = root
        // `PRISM_DEBUG_LAYOUT=1`: log every text's frame.
        if ProcessInfo.processInfo.environment["PRISM_DEBUG_LAYOUT"] != nil {
            for (id, n) in nodes where !n.text.isEmpty {
                NSLog("prism-layout %llu %@ %@ %@", id, n.kind, n.text, NSCoder.string(for: n.frame))
            }
        }
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

    var body: some View {
        if let n = store.nodes[id], !n.hidden {
            let dark = scheme == .dark
            // Snapshots show a scroll view's whole content.
            let height = store.snapshotting && n.kind == "scroll" ? max(n.content.height, n.frame.height) : n.frame.height
            content(n, dark: dark)
                .frame(width: n.frame.width, height: height, alignment: .topLeading)
                .background(n.color("bg", dark: dark).map { RoundedRectangle(cornerRadius: n.num("radius")).fill($0) })
                .overlay(n.color("bc", dark: dark).map {
                    RoundedRectangle(cornerRadius: n.num("radius")).strokeBorder($0, lineWidth: n.num("bw", 1))
                })
                .modifier(RoundedClip(radius: n.num("radius")))
                .opacity(n.num("opacity", 1))
                .contentShape(Rectangle())
                .onTapGesture { if n.clickable { send(["t": "click", "id": id]) } }
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
                .accessibilityHidden(n.a11yHidden)
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
            Text(n.text).font(font(n)).foregroundStyle(fg).multilineTextAlignment(textAlign(n.str("talign")))
        case "button":
            Button { send(["t": "click", "id": id]) } label: { Text(n.text).font(font(n)).foregroundStyle(fg) }
                .buttonStyle(.plain)
        case "toggle":
            Toggle(n.label ?? "", isOn: Binding(
                get: { n.value != 0 },
                set: { send(["t": "change", "id": id, "v": $0 ? 1 : 0]) }
            )).labelsHidden()
        case "text_input":
            TextField(n.placeholder, text: Binding(get: { n.text }, set: { send(["t": "input", "id": id, "v": $0]) }))
                .font(font(n))
        case "progress":
            ProgressView(value: min(max(n.value, 0), 1))
        case "image":
            Image(systemName: n.icon.isEmpty ? "circle" : n.icon).font(font(n)).foregroundStyle(fg)
        case "timer":
            Text(Date(timeIntervalSince1970: n.value), style: .timer).font(font(n).monospacedDigit()).foregroundStyle(fg)
        case "picture":
            Picture(source: n.text, fit: n.value == 1)
        case "scroll":
            if store.snapshotting {
                placed(n.children)
            } else {
                ScrollView {
                    placed(n.children)
                        .frame(width: n.content.width, height: n.content.height, alignment: .topLeading)
                        .background(GeometryReader { g in
                            Color.clear.onChange(of: g.frame(in: .named("s\(id)")).minY) { _, y in
                                if n.scrollEvents { send(["t": "scroll", "id": id, "o": -y, "v": n.frame.height]) }
                            }
                        })
                }
                .coordinateSpace(name: "s\(id)")
                .modifier(Refresh(id: id, on: n.clickable))
            }
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
                NodeView(id: c).offset(x: f.minX, y: f.minY)
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

/// Rounded corners clip their content; square views let it overflow (like
/// the other backends without `overflow: hidden`).
struct RoundedClip: ViewModifier {
    let radius: Double

    @ViewBuilder
    func body(content: Content) -> some View {
        if radius > 0 { content.clipShape(RoundedRectangle(cornerRadius: radius)) } else { content }
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
                .onAppear { report(g.size) }
                .onChange(of: g.size) { _, size in report(size) }
        })
    }

    private func report(_ size: CGSize) {
        var e = event
        e["w"] = size.width
        e["h"] = size.height
        send(e)
    }
}

/// A navigation stack: the first screen is the root, the others are pushed.
struct StackView: View {
    @ObservedObject var store = PrismStore.shared
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
    }

    private func screen(_ s: UInt64) -> some View {
        Color.clear
            .overlay(alignment: .topLeading) { NodeView(id: s) }
            .modifier(ReportSize(event: ["t": "viewport", "id": id]))
            .navigationTitle(store.nodes[s]?.text ?? "")
    }
}

/// Tabs: pages scrolled with the Digital Crown.
struct TabsView: View {
    @ObservedObject var store = PrismStore.shared
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
    }
}

/// An app asset (`assets/` in the bundle) or a remote image.
struct Picture: View {
    let source: String
    let fit: Bool

    var body: some View {
        if source.hasPrefix("http://") || source.hasPrefix("https://"), let url = URL(string: source) {
            AsyncImage(url: url) { $0.resizable().aspectRatio(contentMode: fit ? .fit : .fill) } placeholder: { Color.gray.opacity(0.2) }
        } else if let image = UIImage(contentsOfFile: source.hasPrefix("/") ? source : Bundle.main.bundlePath + "/assets/" + source) {
            Image(uiImage: image).resizable().aspectRatio(contentMode: fit ? .fit : .fill)
        } else {
            Color.gray.opacity(0.2)
        }
    }
}

private func swiftWeight(_ w: Int) -> Font.Weight {
    w >= 700 ? .bold : w >= 600 ? .semibold : w >= 500 ? .medium : .regular
}

private func textAlign(_ a: String?) -> TextAlignment {
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
/// containers render their current content.)
@MainActor
private func snapshotIfAsked(size: CGSize) {
    let env = ProcessInfo.processInfo.environment
    guard let path = env["PRISM_SNAPSHOT"] else { return }
    let delay = Double(env["PRISM_DELAY"] ?? "") ?? 2
    // `PRISM_CLICK="+1|Save"`: press buttons by title first.
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
        let height = max(size.height, (first.map { $0.kind == "scroll" ? $0.content.height : $0.frame.height } ?? 0) + (first?.frame.minY ?? 0))
        let content = Color.clear
            .overlay(alignment: .topLeading) { NodeView(id: shown) }
            .frame(width: size.width, height: height, alignment: .topLeading)
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
@_cdecl("prism_watch_main")
public func prismWatchMain() {
    PrismWatchApp.main()
}
