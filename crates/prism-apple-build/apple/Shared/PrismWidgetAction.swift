// Widget buttons (`Div::widget_action`, #47): the App Intent a widget's or
// Live Activity's `Button(intent:)` runs. Compiled into the app (with the
// bridge) and into the widget extension: WidgetKit archives the intent in
// the extension, and because it is a LiveActivityIntent that the app also
// declares (its own Metadata.appintents), iOS runs `perform()` in the app's
// process, launching it in the background when it is not running. There
// the bridge hands the action to the app's Rust code
// (`App::on_widget_action`) and `perform()` returns when its job is done;
// the app then reloads its widgets.
//
// Wherever `perform()` runs without the app's code (the extension), the
// action is queued in the App Group (`prism-widgets/actions.json`) and the
// app runs it at its next launch or return to the foreground.
import AppIntents
import Foundation

/// Set by the app's bridge (`prism_bridge_init`): runs a queued-format
/// action (`k` kind, `a` action, `p` payload) in the app and calls the
/// closure when its job finished. Nil in the widget extension.
var prismWidgetActionRunner: (([String: String], @escaping () -> Void) -> Void)?

/// The App Group file holding actions the app has not run yet.
func prismWidgetActionsURL() -> URL? {
    guard let group = Bundle.main.object(forInfoDictionaryKey: "PrismAppGroup") as? String,
          let dir = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)
    else { return nil }
    return dir.appendingPathComponent("prism-widgets/actions.json")
}

/// Appends an action to the App Group queue (the app runs it later).
func prismQueueWidgetAction(_ action: [String: String]) {
    guard let url = prismWidgetActionsURL() else { return }
    var queue: [[String: String]] = []
    if let data = try? Data(contentsOf: url),
       let old = (try? JSONSerialization.jsonObject(with: data)) as? [[String: String]] { queue = old }
    queue.append(action)
    guard let data = try? JSONSerialization.data(withJSONObject: queue, options: [.sortedKeys]) else { return }
    try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    try? data.write(to: url, options: .atomic)
}

/// Runs the action in the app when this is the app (its bridge installed
/// the runner); false in the widget extension.
@MainActor
func prismRunWidgetActionInApp(_ action: [String: String]) async -> Bool {
    guard let run = prismWidgetActionRunner else { return false }
    await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in
        run(action) { done.resume() }
    }
    return true
}

#if os(iOS) || os(visionOS)
/// The intent of every widget button: which widget, which action, its
/// payload. Not offered in Shortcuts or Spotlight.
struct PrismWidgetActionIntent: LiveActivityIntent {
    static var title: LocalizedStringResource = "Widget action"
    static var isDiscoverable: Bool = false

    @Parameter(title: "Widget") var kind: String
    @Parameter(title: "Action") var action: String
    @Parameter(title: "Payload") var payload: String

    init() {}

    init(kind: String, action: String, payload: String) {
        self.kind = kind
        self.action = action
        self.payload = payload
    }

    func perform() async throws -> some IntentResult {
        let queued = ["k": kind, "a": action, "p": payload]
        if await !prismRunWidgetActionInApp(queued) {
            prismQueueWidgetAction(queued)
        }
        return .result()
    }
}
#endif
