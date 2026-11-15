import os

/// The activity's lines for the unified log (subsystem `ai.sidevoice`, one category), from the app and from the widget
/// extension. The Sidevoice logging guideline applies (section 8): only warnings and errors are written, never an info or
/// debug line; each is a fixed sentence, so a literal (`StaticString`) is all a message can be, with at most one number as its
/// field. Nothing from outside goes in: no still or file name, no device or route name, no URL, no text, no system error
/// text. The unified log's privacy marks are not relied on: every piece written is public, because it is fixed.
public enum CallLog {
    public static let subsystem = "ai.sidevoice"
    private static let logger = Logger(subsystem: subsystem, category: "app")

    /// Something degraded, then recovered or went on without.
    public static func warning(_ message: StaticString, code: Int? = nil) {
        if let code {
            logger.warning("\(message.description, privacy: .public) code=\(code, privacy: .public)")
        } else {
            logger.warning("\(message.description, privacy: .public)")
        }
    }

    /// Something failed for good.
    public static func error(_ message: StaticString, code: Int? = nil) {
        if let code {
            logger.error("\(message.description, privacy: .public) code=\(code, privacy: .public)")
        } else {
            logger.error("\(message.description, privacy: .public)")
        }
    }
}
