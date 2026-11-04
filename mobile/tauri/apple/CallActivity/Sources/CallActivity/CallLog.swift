import os

/// The activity's diagnostics, in the unified log (`log show --predicate 'subsystem == "ai.sidevoice"'`), with public
/// strings, from the app and from the widget extension. NSLog's strings are redacted on a device, and the Rust side's log
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

    /// A line the Rust side hands over (`SidevoiceLog`): its category, as the unified log has it.
    public static func write(_ category: String, _ message: String) {
        let logger: Logger
        switch category {
        case "stills": logger = stills
        case "intent": logger = intent
        case "text": logger = text
        default: logger = app
        }
        logger.notice("\(message, privacy: .public)")
    }
}
