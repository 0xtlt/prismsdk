// Generic WidgetKit plumbing: a timeline provider reading the snapshots the
// app wrote to the shared App Group container, the widget view, and the Live
// Activity configuration (lock screen + Dynamic Island).
import SwiftUI
import WidgetKit
#if os(iOS)
import ActivityKit
#endif

struct PrismEntry: TimelineEntry {
    let date: Date
    let doc: PNode?
}

func prismLoadWidget(_ kind: String) -> PNode? {
    guard let group = Bundle.main.object(forInfoDictionaryKey: "PrismAppGroup") as? String,
          let dir = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group),
          let json = try? String(contentsOf: dir.appendingPathComponent("prism-widgets/\(kind).json"), encoding: .utf8)
    else { return nil }
    return prismParse(json)
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
