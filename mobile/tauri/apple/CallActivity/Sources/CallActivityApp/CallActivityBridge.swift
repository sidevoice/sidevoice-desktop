import ActivityKit
import AVFoundation
import CallActivity
import Foundation

/// The Live Activity's door for the app's Rust side (mobile/tauri/plugin/src/platform/call_surface/ios.rs), which finds it
/// by name: `start` when the app's own call starts, `show` on every change of the call's surface, `end` when it ends (and
/// once at launch, for an activity an earlier run left). Each takes the call's details as JSON (`CallDetails`: `title`,
/// `muted`, `surface`), from any thread; the work is done on the main queue, in order.
///
/// Nothing runs before iOS 16.1, or while the person has Live Activities off for the app.
@objc(SidevoiceCallActivity)
public final class CallActivityBridge: NSObject {
    @objc public static func start(_ json: String) {
        let report = Report.read(json)
        DispatchQueue.main.async {
            if #available(iOS 16.1, *) { Controller.shared.start(report) }
        }
    }

    @objc public static func show(_ json: String) {
        let report = Report.read(json)
        DispatchQueue.main.async {
            if #available(iOS 16.1, *) { Controller.shared.show(report) }
        }
    }

    @objc public static func end() {
        DispatchQueue.main.async {
            if #available(iOS 16.1, *) { Controller.shared.end() }
        }
    }
}

/// The call's details as the Rust side serialises them (`CallDetails`, its `surface` in camelCase), every field optional.
struct Report: Decodable {
    struct Still: Decodable {
        var file: String?
        var k: Double?
    }

    struct Other: Decodable {
        var still: Still?
    }

    struct Page: Decodable {
        var text: String?
        var lit: Int?
    }

    struct Words: Decodable {
        var state, offline, mute, unmute, skip, skipName, cancelSend, hangUp, route, speaker, conversations: String?
    }

    struct Surface: Decodable {
        var phase: String?
        var canSkip: Bool?
        var canCancelSend: Bool?
        var model: String?
        var machine: String?
        var workingSince: Double?
        var still: Still?
        var others: [Other]?
        var conversationCount: Int?
        var page: Page?
        var said: String?
        var words: Words?
    }

    var title: String?
    var muted: Bool?
    var surface: Surface?

    static func read(_ json: String) -> Report? {
        try? JSONDecoder().decode(Report.self, from: Data(json.utf8))
    }

    /// The activity's state: the page's, with the audio route the system has now.
    func state(route: (route: CallState.Route, name: String)?) -> CallState {
        var state = CallState()
        let surface = surface ?? Surface()
        state.phase = surface.phase.flatMap(CallState.Phase.init(rawValue:)) ?? .idle
        // The title is cut by the app (HANDOVER §6): the activity's data stays small.
        state.title = String((title ?? "").prefix(64))
        state.model = surface.model ?? ""
        state.machine = surface.machine ?? ""
        state.micOff = muted ?? false
        state.canSkip = surface.canSkip ?? false
        state.canCancelSend = surface.canCancelSend ?? false
        state.workingSince = surface.workingSince.map { Date(timeIntervalSince1970: $0 / 1000) }
        state.creature = surface.still.flatMap(Self.creature)
        state.others = (surface.others ?? []).prefix(3).map { $0.still.flatMap(Self.creature) }
        state.conversationCount = surface.conversationCount ?? 0
        state.route = route?.route
        state.routeName = route?.name ?? ""
        switch state.phase {
        case .speaking:
            state.text = surface.page?.text
            state.lit = surface.page?.lit ?? 0
        case .said:
            state.text = surface.said
        default:
            break
        }
        let words = surface.words ?? Words()
        state.words.state = words.state ?? ""
        state.words.offline = words.offline ?? ""
        state.words.mute = words.mute ?? ""
        state.words.unmute = words.unmute ?? ""
        state.words.skip = words.skip ?? ""
        state.words.skipName = words.skipName ?? ""
        state.words.cancelSend = words.cancelSend ?? ""
        state.words.hangUp = words.hangUp ?? ""
        state.words.route = words.route ?? ""
        state.words.speaker = words.speaker ?? ""
        state.words.conversations = words.conversations ?? ""
        return state
    }

    private static func creature(_ still: Still) -> CallState.Creature? {
        guard let file = still.file, !file.isEmpty else { return nil }
        return CallState.Creature(file: file, k: still.k ?? 1)
    }
}

/// Starts, updates and ends the activity (main queue). Every change goes out at once, except the captions: a change of
/// the reply's page alone waits until ~2 s after the last update (iOS throttles local updates that come too often;
/// HANDOVER §5). `staleDate` is three minutes ahead, moved on every update.
@available(iOS 16.1, *)
final class Controller {
    static let shared = Controller()

    private var activity: Activity<CallActivityAttributes>?
    private var report: Report?
    private var sent: CallState?
    private var sentAt = Date.distantPast
    private var later: DispatchWorkItem?
    private var routeChanges: NSObjectProtocol?

    static let captionInterval: TimeInterval = 2
    static let staleAfter: TimeInterval = 180

    func start(_ report: Report?) {
        end()
        guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }
        self.report = report
        let state = current()
        do {
            if #available(iOS 16.2, *) {
                activity = try Activity.request(
                    attributes: CallActivityAttributes(),
                    content: ActivityContent(state: state, staleDate: Date().addingTimeInterval(Self.staleAfter)),
                    pushType: nil)
            } else {
                activity = try Activity.request(attributes: CallActivityAttributes(), contentState: state, pushType: nil)
            }
        } catch {
            NSLog("Sidevoice: the call's Live Activity did not start: \(error)")
            return
        }
        sent = state
        sentAt = Date()
        routeChanges = NotificationCenter.default.addObserver(
            forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            self?.publish()
        }
    }

    func show(_ report: Report?) {
        guard activity != nil else { return }
        self.report = report
        publish()
    }

    /// Ends the call's activity, and any other of the app's (one an earlier run left), at once.
    func end() {
        later?.cancel()
        later = nil
        if let routeChanges { NotificationCenter.default.removeObserver(routeChanges) }
        routeChanges = nil
        activity = nil
        sent = nil
        report = nil
        for activity in Activity<CallActivityAttributes>.activities {
            Task {
                if #available(iOS 16.2, *) {
                    await activity.end(nil, dismissalPolicy: .immediate)
                } else {
                    await activity.end(using: nil, dismissalPolicy: .immediate)
                }
            }
        }
    }

    private func current() -> CallState {
        (report ?? Report()).state(route: AudioRoute.current())
    }

    private func publish() {
        guard let activity else { return }
        let state = current()
        guard state != sent else { return }
        let wait = Self.captionInterval - Date().timeIntervalSince(sentAt)
        if let sent, state.sameButText(sent), wait > 0 {
            if later == nil {
                let work = DispatchWorkItem { [weak self] in
                    self?.later = nil
                    self?.publish()
                }
                later = work
                DispatchQueue.main.asyncAfter(deadline: .now() + wait, execute: work)
            }
            return
        }
        later?.cancel()
        later = nil
        sent = state
        sentAt = Date()
        Task {
            if #available(iOS 16.2, *) {
                await activity.update(ActivityContent(state: state, staleDate: Date().addingTimeInterval(Self.staleAfter)))
            } else {
                await activity.update(using: state)
            }
        }
    }
}

