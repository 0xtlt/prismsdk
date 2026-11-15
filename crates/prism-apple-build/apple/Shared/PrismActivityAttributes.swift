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
    /// The activity's id (prism-core `ActivityId`, in decimal): the one
    /// `start_activity` returned, or the one a push-to-start push gave it
    /// (prism-push `Push::start_live_activity`). The app reports the
    /// activity's push token with it.
    public var id: String?
    public init(kind: String, id: String? = nil) { self.kind = kind; self.id = id }

    private enum CodingKeys: String, CodingKey { case kind, id }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        kind = try c.decode(String.self, forKey: .kind)
        // A push written by hand may give the id as a number.
        if let s = try? c.decodeIfPresent(String.self, forKey: .id) {
            id = s
        } else if let n = try? c.decodeIfPresent(UInt64.self, forKey: .id) {
            id = String(n)
        } else {
            id = nil
        }
    }
}
#endif
