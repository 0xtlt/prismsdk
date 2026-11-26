// Generic WidgetKit plumbing: a timeline provider reading the snapshots the
// app wrote to the shared App Group container, the widget view, and the Live
// Activity configuration (lock screen + Dynamic Island). Configurable widgets
// (parameters picked in the « Edit widget » sheet) get an App Intents
// provider: their intents, entities and queries are generated per kind by
// generate_widgets.py.
import AppIntents
import SwiftUI
import WidgetKit
#if os(iOS)
import ActivityKit
#endif

/// A drawing of a widget and the instant it is shown from: its `families`
/// are the ones drawn for that instant.
struct PrismEntry: TimelineEntry {
    let date: Date
    let doc: PNode?
}

/// The App Group's `prism-widgets` directory, shared with the app.
func prismWidgetsDir() -> URL? {
    guard let group = Bundle.main.object(forInfoDictionaryKey: "PrismAppGroup") as? String,
          let dir = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)
    else { return nil }
    return dir.appendingPathComponent("prism-widgets", isDirectory: true)
}

func prismLoadWidget(_ kind: String) -> PNode? {
    guard let dir = prismWidgetsDir(),
          let json = try? String(contentsOf: dir.appendingPathComponent("\(kind).json"), encoding: .utf8)
    else { return nil }
    return prismParse(json)
}

// MARK: Refresh without the app (#331)

#if PRISM_RUST_EXTENSION
/// Implemented in Rust (prism-ios / prism-macos): runs the refresh hook of
/// widget `kind` (`Widget::refresh`) in the app's code, linked into this
/// extension, writes its new drawings to the App Group, then calls
/// `prism_widget_refresh_done(token)`. Main thread.
@_silgen_name("prism_widget_refresh")
func prismRustWidgetRefresh(_ kind: UnsafePointer<CChar>, _ token: UInt64)

/// The timelines waiting for a refresh, by token. Main thread.
private var prismRefreshWaiting: [UInt64: () -> Void] = [:]
private var prismRefreshNext: UInt64 = 0

/// Called by Rust when the refresh of `token` ended (done, failed or timed
/// out: the drawings in the App Group are the ones to show either way).
@_cdecl("prism_widget_refresh_done")
public func prismWidgetRefreshDone(_ token: UInt64) {
    prismRefreshWaiting.removeValue(forKey: token)?()
}
#endif

/// Runs the refresh hook of `kind` when the app's code is in this
/// extension and the widget has one (`"refresh"` in its snapshot), then
/// calls `done`; at once otherwise. The hook has its own timeout
/// (`Widget::refresh_timeout`), so `done` always comes.
func prismRefresh(_ kind: String, _ done: @escaping () -> Void) {
    #if PRISM_RUST_EXTENSION
    // Before the app's first drawing there is no snapshot yet: the hook
    // runs too, and draws the widget for the first time.
    let doc = prismLoadWidget(kind)
    if let doc, doc["refresh"] as? Bool != true {
        done()
        return
    }
    DispatchQueue.main.async {
        prismRefreshNext += 1
        let token = prismRefreshNext
        prismRefreshWaiting[token] = done
        kind.withCString { prismRustWidgetRefresh($0, token) }
    }
    #else
    done()
    #endif
}

/// `prismRefresh`, awaited.
func prismRefresh(_ kind: String) async {
    await withCheckedContinuation { (finished: CheckedContinuation<Void, Never>) in
        prismRefresh(kind) { finished.resume() }
    }
}

// MARK: Configurable widgets

/// A configurable kind's intent (generated): its kind and the option picked
/// for each parameter, by parameter id (nothing for a parameter left unset).
protocol PrismConfigurationIntent: WidgetConfigurationIntent {
    static var prismKind: String { get }
    var prismValues: [String: String] { get }
}

/// A parameter's value, nil while it is unset. The generated parameters are
/// required (so the edit sheet starts on the default option, #49), and an
/// instance added before that has none: `valueState` says so where it
/// exists; before iOS 18.2 / macOS 15.2 the query's `defaultResult()` never
/// leaves one unset.
func prismValue<E>(_ parameter: IntentParameter<E>) -> E? {
    if #available(iOS 18.2, macOS 15.2, visionOS 2.2, *) {
        if case .set(let value) = parameter.valueState { return value }
        return nil
    }
    return parameter.wrappedValue
}

/// A parameter's options as the app wrote them at its last reload: what the
/// edit sheet lists.
func prismOptions(_ kind: String, _ parameter: String) -> [(id: String, title: String)] {
    prismParameter(prismLoadWidget(kind), parameter).options
}

/// The option picked until the user changes it.
func prismDefaultOption(_ kind: String, _ parameter: String) -> (id: String, title: String)? {
    let p = prismParameter(prismLoadWidget(kind), parameter)
    return p.options.first { $0.id == p.default }
}

/// The options for the ids WidgetKit resolves (`EntityQuery.entities(for:)`),
/// one per id, in its order (#277). A known id gets its option. An unknown
/// one (an option gone since, or the empty placeholder an instance keeps when
/// it was added before the app wrote its options) gets an entity of that id,
/// titled like the default option, or `placeholder` (the parameter's title)
/// without one: left out, the required parameter would be missing, WidgetKit
/// would never ask for a timeline and the widget would stay a grey
/// placeholder. `prismChosen` drops the unknown value, so the default is drawn.
func prismResolve(_ kind: String, _ parameter: String, _ ids: [String], placeholder: String) -> [(id: String, title: String)] {
    let p = prismParameter(prismLoadWidget(kind), parameter)
    let fallback = p.options.first { $0.id == p.default }?.title ?? placeholder
    return ids.map { id in p.options.first { $0.id == id } ?? (id, fallback) }
}

private func prismParameter(_ doc: PNode?, _ id: String) -> (default: String?, options: [(id: String, title: String)]) {
    let p = (doc?["parameters"] as? [PNode])?.first { $0["id"] as? String == id }
    let options = (p?["options"] as? [PNode] ?? []).compactMap { o -> (id: String, title: String)? in
        guard let id = o["id"] as? String else { return nil }
        return (id, o["title"] as? String ?? id)
    }
    return (p?["default"] as? String, options)
}

/// The full configuration of an instance: the default, with the options the
/// user picked that the app still offers.
func prismChosen(_ doc: PNode?, _ values: [String: String]) -> [String: String] {
    var chosen = doc?["default"] as? [String: String] ?? [:]
    for (id, value) in values where prismParameter(doc, id).options.contains(where: { $0.id == value }) {
        chosen[id] = value
    }
    return chosen
}

/// The snapshot document with the families drawn for `chosen`, and its
/// drawings ahead: the default ones, another rendered configuration's, or
/// none (the view then asks to open the app, which renders it).
func prismConfigured(_ doc: PNode?, _ chosen: [String: String]) -> PNode? {
    guard var doc else { return nil }
    if chosen == doc["default"] as? [String: String] ?? [:] { return doc }
    let config = (doc["configs"] as? [PNode])?.first { $0["values"] as? [String: String] == chosen }
    doc["families"] = config?["families"]
    doc["ahead"] = config?["ahead"]
    return doc
}

/// Records the configurations the system shows (the one asked for first),
/// so the app renders them on its next reload: `<kind>.configurations.json`.
/// Written only when it changes.
func prismRecordConfigurations(_ kind: String, _ configs: [[String: String]]) {
    guard let dir = prismWidgetsDir() else { return }
    var unique: [[String: String]] = []
    for c in configs where !unique.contains(c) { unique.append(c) }
    let url = dir.appendingPathComponent("\(kind).configurations.json")
    if let data = try? Data(contentsOf: url),
       let old = (try? JSONSerialization.jsonObject(with: data)) as? [[String: String]],
       old == unique { return }
    guard let data = try? JSONSerialization.data(withJSONObject: unique, options: [.sortedKeys]) else { return }
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    try? data.write(to: url, options: .atomic)
}

struct PrismIntentProvider<Intent: PrismConfigurationIntent>: AppIntentTimelineProvider {
    func placeholder(in context: Context) -> PrismEntry {
        prismEntryNow(prismLoadWidget(Intent.prismKind))
    }
    func snapshot(for configuration: Intent, in context: Context) async -> PrismEntry {
        let doc = prismLoadWidget(Intent.prismKind)
        return prismEntryNow(prismConfigured(doc, prismChosen(doc, configuration.prismValues)))
    }
    func timeline(for configuration: Intent, in context: Context) async -> Timeline<PrismEntry> {
        var doc = prismLoadWidget(Intent.prismKind)
        let chosen = prismChosen(doc, configuration.prismValues)
        // Every instance on screen, so the app keeps rendering what is used.
        var shown = [chosen]
        let infos = await withCheckedContinuation { done in
            WidgetCenter.shared.getCurrentConfigurations { done.resume(returning: try? $0.get()) }
        }
        if let infos {
            for info in infos where info.kind == Intent.prismKind {
                if let intent = info.widgetConfigurationIntent(of: Intent.self) {
                    shown.append(prismChosen(doc, intent.prismValues))
                }
            }
        }
        if doc != nil { prismRecordConfigurations(Intent.prismKind, shown) }
        // The refresh hook draws every configuration recorded above.
        await prismRefresh(Intent.prismKind)
        doc = prismLoadWidget(Intent.prismKind)
        return prismTimeline(prismConfigured(doc, prismChosen(doc, configuration.prismValues)))
    }
}

struct PrismProvider: TimelineProvider {
    let kind: String
    func placeholder(in context: Context) -> PrismEntry { prismEntryNow(prismLoadWidget(kind)) }
    func getSnapshot(in context: Context, completion: @escaping (PrismEntry) -> Void) {
        completion(placeholder(in: context))
    }
    func getTimeline(in context: Context, completion: @escaping (Timeline<PrismEntry>) -> Void) {
        let kind = kind
        prismRefresh(kind) { completion(prismTimeline(prismLoadWidget(kind))) }
    }
}

/// The most entries of a timeline: the drawing now, the drawings ahead (the
/// app writes at most 16, `Widget::MAX_AHEAD`) and the ends of timers with
/// an `after` text. Past it, WidgetKit asks for the rest at the last one.
let prismMaxEntries = 64

/// The timeline of a snapshot document (#281): its drawing now, each drawing
/// ahead (`"ahead"`, `Widget::ahead`) from its instant, and the same drawing
/// again when a timer with an `after` text ends, so the text replaces 0:00.
/// WidgetKit switches between them without the app. The policy is the
/// widget's (`"reload"`, #331: `Widget::reload`): `"never"`, `"end"`, or a
/// delay in seconds (`.after`, from now: a timeline the extension rebuilds
/// after a failed refresh asks again a delay later). Without one, `.atEnd`
/// while drawings are left after the last entry (WidgetKit then asks again
/// and the last one stays), `.never` otherwise: the app reloads timelines
/// whenever its state changes.
func prismTimeline(_ doc: PNode?, now: Date = .now) -> Timeline<PrismEntry> {
    let drawings = prismDrawings(doc, now: now)
    var entries: [PrismEntry] = []
    for (i, drawing) in drawings.enumerated() {
        entries.append(PrismEntry(date: drawing.from, doc: drawing.doc))
        let next = i + 1 < drawings.count ? drawings[i + 1].from : .distantFuture
        for end in prismTimerEnds(drawing.doc, after: drawing.from) where end < next {
            entries.append(PrismEntry(date: end, doc: drawing.doc))
        }
    }
    let more = entries.count > prismMaxEntries
    entries = Array(entries.prefix(prismMaxEntries))
    let ahead = drawings.count > 1 || more
    return Timeline(entries: entries, policy: prismReloadPolicy(doc?["reload"], ahead: ahead, now: now))
}

/// A snapshot's `"reload"` as WidgetKit's policy (`ahead`: drawings are
/// left after the last entry, for the default).
func prismReloadPolicy(_ reload: Any?, ahead: Bool, now: Date) -> TimelineReloadPolicy {
    switch reload {
    case let s as String where s == "never": return .never
    case let s as String where s == "end": return .atEnd
    case let n as NSNumber: return .after(now.addingTimeInterval(max(n.doubleValue, 0)))
    default: return ahead ? .atEnd : .never
    }
}

/// The drawing shown now (the system's placeholder and snapshot).
func prismEntryNow(_ doc: PNode?, now: Date = .now) -> PrismEntry {
    let drawing = prismDrawings(doc, now: now)[0]
    return PrismEntry(date: drawing.from, doc: drawing.doc)
}

/// The drawings of a document from now on, soonest first: the latest whose
/// instant has come (the families written, or a drawing ahead), from `now`,
/// then each one ahead from its instant. Always one at least.
func prismDrawings(_ doc: PNode?, now: Date) -> [(from: Date, doc: PNode?)] {
    guard var doc, let ahead = doc["ahead"] as? [PNode], !ahead.isEmpty else { return [(now, doc)] }
    doc["ahead"] = nil
    var drawings: [(from: Date, doc: PNode?)] = [(now, doc)]
    for e in ahead {
        guard let at = (e["at"] as? NSNumber)?.doubleValue, let families = e["families"] else { continue }
        var drawn = doc
        drawn["families"] = families
        let from = Date(timeIntervalSince1970: at)
        if from <= now {
            drawings[0] = (now, drawn)
        } else {
            drawings.append((from, drawn))
        }
    }
    return drawings
}

/// The ends after `now` of the timers that show an `after` text (`"ph"`), in
/// every family of `doc`, sorted.
func prismTimerEnds(_ doc: PNode?, after now: Date) -> [Date] {
    var ends = Set<Date>()
    func walk(_ n: PNode) {
        if n["k"] as? String == "m", let after = n["ph"] as? String, !after.isEmpty,
           let v = (n["v"] as? NSNumber)?.doubleValue {
            let end = Date(timeIntervalSince1970: v)
            if end > now { ends.insert(end) }
        }
        (n["c"] as? [PNode])?.forEach(walk)
    }
    (doc?["families"] as? PNode)?.values.compactMap { $0 as? PNode }.forEach(walk)
    return ends.sorted()
}

func prismFamilyName(_ f: WidgetFamily) -> String {
    switch f {
    case .systemSmall: return "systemSmall"
    case .systemMedium: return "systemMedium"
    case .systemLarge: return "systemLarge"
    case .systemExtraLarge: return "systemExtraLarge"
    #if os(iOS)
    case .accessoryCircular: return "accessoryCircular"
    case .accessoryRectangular: return "accessoryRectangular"
    case .accessoryInline: return "accessoryInline"
    #endif
    default: return "systemSmall"
    }
}

struct PrismWidgetView: View {
    @Environment(\.widgetFamily) private var family
    let entry: PrismEntry

    var body: some View {
        let (node, tile) = root()
        Group {
            if node != nil {
                prismView(node)
            } else {
                Text("Open the app to set up this widget.").font(.caption).foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .containerBackground(for: .widget) { PrismTile(color: tile.color, gradient: tile.gradient) }
        .widgetURL(prismURL(entry.doc?["link"]))
        // Timers compare their end with the entry's instant (#281).
        .environment(\.prismEntryDate, entry.date)
    }

    /// The family's root node, and its `bg` and gradient (`gr`, #330) taken
    /// out: they fill the whole widget (the container background) instead
    /// of the area inside the system's margins.
    private func root() -> (PNode?, (color: Color?, gradient: [String: Any]?)) {
        guard var node = (entry.doc?["families"] as? PNode)?[prismFamilyName(family)] as? PNode else {
            return (nil, (nil, nil))
        }
        let color = prismColor(node["bg"])
        let gradient = node["gr"] as? [String: Any]
        if color != nil { node["bg"] = nil }
        if gradient != nil { node["gr"] = nil }
        return (node, (color, gradient))
    }
}

/// A widget's container background: the root's color (the system's
/// background without one), under its gradient.
struct PrismTile: View {
    let color: Color?
    let gradient: [String: Any]?

    var body: some View {
        ZStack {
            if let color { Rectangle().fill(color) } else { Rectangle().fill(.background) }
            if let gradient { PrismGradientFill(spec: gradient, box: UnevenRoundedRectangle()) }
        }
    }
}

#if os(iOS)
struct PrismLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: PrismActivityAttributes.self) { context in
            let doc = prismParse(context.state.json)
            prismView(doc?["ls"])
                .activityBackgroundTint(prismColor(doc?["tn"]))
                .widgetURL(prismURL(doc?["l"]))
                .accessibilityLabel(Text(doc?["sm"] as? String ?? ""))
        } dynamicIsland: { context in
            let doc = prismParse(context.state.json)
            return DynamicIsland {
                DynamicIslandExpandedRegion(.leading) { prismView(doc?["el"]).padding(.leading, 6) }
                DynamicIslandExpandedRegion(.trailing) { prismView(doc?["et"]).padding(.trailing, 6) }
                DynamicIslandExpandedRegion(.center) { prismView(doc?["ec"]) }
                DynamicIslandExpandedRegion(.bottom) { prismView(doc?["eb"]) }
            } compactLeading: {
                prismView(doc?["cl"])
            } compactTrailing: {
                prismView(doc?["ct"])
            } minimal: {
                prismView(doc?["mn"])
            }
            .widgetURL(prismURL(doc?["l"]))
            .keylineTint(prismColor(doc?["tn"]))
        }
    }
}
#endif
