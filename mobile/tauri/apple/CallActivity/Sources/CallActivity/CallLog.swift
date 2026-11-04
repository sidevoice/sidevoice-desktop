import os

/// The activity's diagnostics, in the unified log (`log show --predicate 'subsystem == "ai.sidevoice"'`), from the app and
/// from the widget extension. Public strings only for what holds no conversation: ids, sizes, names, lengths. NSLog's strings are redacted on a device, and the Rust side's log
/// is a file, so what happens to the creatures' stills and to the buttons is written here.
public enum CallLog {
    public static let subsystem = "ai.sidevoice"
    /// The creatures' stills: kept by the app, read by the extension.
    public static let stills = Logger(subsystem: subsystem, category: "stills")
    /// The buttons' App Intents.
    public static let intent = Logger(subsystem: subsystem, category: "intent")
    /// The words in the island.
    public static let text = Logger(subsystem: subsystem, category: "text")
    /// The app's own log lines (the Rust side's), by category.
    public static let app = Logger(subsystem: subsystem, category: "app")

    /// A line the Rust side hands over (`SidevoiceLog`): its category, as the unified log has it. The stills' and the intents'
    /// lines hold ids, sizes and names, never the conversation's words, and are public; the app's own lines may carry what
    /// the person said, and stay private (a device's log shows them as `<private>`).
    public static func write(_ category: String, _ message: String) {
        switch category {
        case "stills": stills.notice("\(message, privacy: .public)")
        case "intent": intent.notice("\(message, privacy: .public)")
        default: app.notice("\(message, privacy: .private)")
        }
    }
}
