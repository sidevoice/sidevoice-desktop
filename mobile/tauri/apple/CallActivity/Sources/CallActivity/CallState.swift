import Foundation
#if canImport(ActivityKit)
import ActivityKit
#endif

/// The call's Live Activity: no static data (a conversation switch changes everything), only its state.
@available(iOS 16.1, *)
public struct CallActivityAttributes: ActivityAttributes {
    public typealias ContentState = CallState
    public init() {}
}

/// What the activity draws, as the app last told it (HANDOVER §6). Built by the app from the call's surface
/// (`CallActivityApp`); every word in it is the page's, in its language.
public struct CallState: Codable, Hashable {
    /// The app's own states (HANDOVER §0), plus "said", the few seconds after the person's turn is sent.
    public enum Phase: String, Codable, Hashable {
        case idle, listening, transcribing, said, working, speaking, holding, offline
    }

    /// Where the call's audio goes while headphones are connected (D-LA-ROUTE): none without headphones.
    public enum Route: String, Codable, Hashable {
        case headphones, speaker
    }

    /// A creature's still, by its file name in the App Group's stills folder, and `k`, how much of it the body takes.
    public struct Creature: Codable, Hashable {
        public var file: String
        public var k: Double
        public init(file: String, k: Double = 1) {
            self.file = file
            self.k = k
        }
    }

    /// The words the activity draws, resolved by the page in its language.
    public struct Words: Codable, Hashable {
        public var state = ""
        /// Always there: a stale activity draws the offline state with it (D-LA-OFFLINE).
        public var offline = ""
        public var mute = ""
        public var unmute = ""
        /// Skip's short label, and its accessible name.
        public var skip = ""
        public var skipName = ""
        public var cancelSend = ""
        public var hangUp = ""
        /// The audio switch's accessible name, `{route}` in it.
        public var route = ""
        public var speaker = ""
        /// The stack's accessible name, its count in it.
        public var conversations = ""
        public init() {}
    }

    public var phase: Phase = .idle
    /// While your words are held past the page's own 3 s (`TextPager`), `phase` is `.said` (the layout) and this is the
    /// phase the call really is in.
    public var heldPhase: Phase?
    public var title = ""
    /// "Opus 5.5", the page's wording of the model.
    public var model = ""
    public var machine = ""
    public var micOff = false
    public var canSkip = false
    public var canCancelSend = false
    /// When the working phase began, for a system timer; none when the page does not know it (never a made-up time).
    public var workingSince: Date?
    public var creature: Creature?
    /// Up to three other conversations, for the stack (a nil still draws an empty face).
    public var others: [Creature?] = []
    public var conversationCount = 0
    public var route: Route?
    /// The connected headphones' name, as the system gives it ("AirPods Pro").
    public var routeName = ""
    /// Speaking: the chunk of the reply the voice is in; said: a chunk of the person's words (`TextChunker`: built to fit,
    /// one update per chunk). No karaoke: all of it is drawn at full strength.
    public var text: String?
    public var words = Words()
    /// The page's language (BCP 47), for what the system words itself on the activity (the working timer on
    /// Always-On); empty: the device's.
    public var language = ""

    public init() {}

    /// The phase whose tone, glyph and timer the activity wears. Your words (`.said`) are not a state (D-LA-SAID-STATE): the
    /// call is working on them, and the header says so, with its timer, when the page says so (the working start is there);
    /// a page that does not (it says idle, no timer) is drawn as it says, never green "Resting".
    public var tonePhase: Phase {
        guard phase == .said else { return phase }
        // A phase that is a condition of its own (can't receive, offline) is what the words wear; idle or working is
        // decided by the page's working start, whichever of them the held phase was.
        if let heldPhase, heldPhase != .working, heldPhase != .idle { return heldPhase }
        return workingSince != nil || heldPhase == .working ? .working : .idle
    }

    /// The locale the system's own words on the activity follow: the page's language, else the device's.
    public var locale: Locale {
        language.isEmpty ? .current : Locale(identifier: language)
    }

    /// The state a stale activity draws (`context.isStale`: the app has stopped updating it): offline, with its word
    /// (D-LA-OFFLINE). There is no stale text of its own.
    public var stale: CallState {
        var state = self
        state.phase = .offline
        state.words.state = words.offline
        state.text = nil
        state.canSkip = false
        state.canCancelSend = false
        state.workingSince = nil
        return state
    }

    /// The audio switch's accessible name: the page's words with the output's name in them.
    public var routeLabel: String {
        let name = route == .speaker ? words.speaker : routeName
        return words.route.replacingOccurrences(of: "{route}", with: name)
    }
}

/// Where the app keeps the creatures' stills for the activity: the App Group's container, `call-stills/`
/// (mobile/tauri/plugin/src/call_surface.rs, `stills_dir`), shared with the widget extension.
public enum CallStills {
    public static let appGroup = "group.ai.sidevoice.mobile"
    static let folder = "call-stills"

    /// Overrides the folder (the render tests' fixtures).
    public static var directory: URL? = FileManager.default
        .containerURL(forSecurityApplicationGroupIdentifier: appGroup)?
        .appendingPathComponent(folder, isDirectory: true)

    static func url(_ file: String) -> URL? {
        guard !file.isEmpty, !file.contains("/") else { return nil }
        return directory?.appendingPathComponent(file)
    }
}
