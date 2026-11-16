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

/// The snapshot document with the families drawn for `chosen`: the default
/// ones, another rendered configuration's, or none (the view then asks to
/// open the app, which renders it).
func prismConfigured(_ doc: PNode?, _ chosen: [String: String]) -> PNode? {
    guard var doc else { return nil }
    if chosen == doc["default"] as? [String: String] ?? [:] { return doc }
    let config = (doc["configs"] as? [PNode])?.first { $0["values"] as? [String: String] == chosen }
    doc["families"] = config?["families"]
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
        PrismEntry(date: .now, doc: prismLoadWidget(Intent.prismKind))
    }
    func snapshot(for configuration: Intent, in context: Context) async -> PrismEntry {
        let doc = prismLoadWidget(Intent.prismKind)
        return PrismEntry(date: .now, doc: prismConfigured(doc, prismChosen(doc, configuration.prismValues)))
    }
    func timeline(for configuration: Intent, in context: Context) async -> Timeline<PrismEntry> {
        let doc = prismLoadWidget(Intent.prismKind)
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
        // The app reloads timelines whenever its state changes.
        return Timeline(entries: [PrismEntry(date: .now, doc: prismConfigured(doc, chosen))], policy: .never)
    }
}

struct PrismProvider: TimelineProvider {
    let kind: String
    func placeholder(in context: Context) -> PrismEntry { PrismEntry(date: .now, doc: prismLoadWidget(kind)) }
    func getSnapshot(in context: Context, completion: @escaping (PrismEntry) -> Void) {
        completion(placeholder(in: context))
    }
    func getTimeline(in context: Context, completion: @escaping (Timeline<PrismEntry>) -> Void) {
        // The app reloads timelines whenever its state changes.
        completion(Timeline(entries: [placeholder(in: context)], policy: .never))
    }
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
        .containerBackground(tile.map { AnyShapeStyle($0) } ?? AnyShapeStyle(.background), for: .widget)
        .widgetURL(prismURL(entry.doc?["link"]))
    }

    /// The family's root node, and its `bg` taken out: it fills the whole
    /// widget (the container background) instead of the area inside the
    /// system's margins.
    private func root() -> (PNode?, Color?) {
        guard var node = (entry.doc?["families"] as? PNode)?[prismFamilyName(family)] as? PNode else { return (nil, nil) }
        let tile = prismColor(node["bg"])
        if tile != nil { node["bg"] = nil }
        return (node, tile)
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
