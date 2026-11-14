// Shared by the app (to start activities) and the widget extension (to render
// them). ActivityKit matches the two by this type.
#if os(iOS)
import ActivityKit

public struct PrismActivityAttributes: ActivityAttributes {
    public struct ContentState: Codable, Hashable {
        /// The rendered regions as compact JSON (see prism-core `ActivityContent`).
        public var json: String
        public init(json: String) { self.json = json }
    }

    /// The app-defined activity kind (`"focus"`).
    public var kind: String
    public init(kind: String) { self.kind = kind }
}
#endif
