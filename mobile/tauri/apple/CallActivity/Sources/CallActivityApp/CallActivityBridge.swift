import ActivityKit
import AVFoundation
import CallActivity
import Foundation
import UIKit

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

    /// Whether the call's Live Activity is on the screens now: requested and not over. False before iOS 16.1, with Live
    /// Activities off, and while the app has not yet been in front to request it. Any thread: the media controls ask on
    /// every report, and give way to the activity only while it runs.
    @objc public static func running() -> Bool {
        guard #available(iOS 16.1, *) else { return false }
        return Activity<CallActivityAttributes>.activities.contains {
            $0.activityState != .ended && $0.activityState != .dismissed
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
        var language: String?
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
        state.language = surface.language ?? ""
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
/// HANDOVER §5). Each update moves `staleDate` three minutes ahead, and while the call lasts the activity is updated at
/// least every minute even when nothing changed (the working timer is the system's), so a live call never reads stale.
/// The updates and the end go to ActivityKit one after the other, in order.
///
/// An activity can be requested only while the app is in front: a call that starts in the background (a page reloaded
/// there) gets its activity when the app next comes to the front, if the call still lasts.
@available(iOS 16.1, *)
final class Controller {
    static let shared = Controller()

    private var activity: Activity<CallActivityAttributes>?
    /// The call lasts (started, not ended): what a refresh and a start in the foreground are for.
    private var wanted = false
    private var report: Report?
    private var sent: CallState?
    private var sentAt = Date.distantPast
    private var later: DispatchWorkItem?
    private var refresh: Timer?
    private var observers: [NSObjectProtocol] = []
    /// The last of ActivityKit's calls asked for: each waits for the one before.
    private var queue: Task<Void, Never>?

    static let captionInterval: TimeInterval = 2
    static let staleAfter: TimeInterval = 180
    static let refreshInterval: TimeInterval = 60

    init() {
        let center = NotificationCenter.default
        observers = [
            center.addObserver(forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main) { [weak self] note in
                AudioRoute.routeChanged(note)
                self?.publish()
            },
            center.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
                self?.requestIfWanted()
            },
        ]
    }

    func start(_ report: Report?) {
        end()
        wanted = true
        self.report = report
        requestIfWanted()
        refresh = Timer.scheduledTimer(withTimeInterval: Self.refreshInterval, repeats: true) { [weak self] _ in
            self?.publish(refresh: true)
        }
    }

    func show(_ report: Report?) {
        guard wanted else { return }
        self.report = report
        publish()
    }

    /// Ends the call's activity, and any other of the app's (one an earlier run left), at once.
    func end() {
        wanted = false
        later?.cancel()
        later = nil
        refresh?.invalidate()
        refresh = nil
        activity = nil
        sent = nil
        report = nil
        let all = Activity<CallActivityAttributes>.activities
        enqueue {
            for activity in all {
                if #available(iOS 16.2, *) {
                    await activity.end(nil, dismissalPolicy: .immediate)
                } else {
                    await activity.end(using: nil, dismissalPolicy: .immediate)
                }
            }
        }
    }

    /// Requests the activity for the call that lasts, if it has none yet and the app is in front.
    private func requestIfWanted() {
        guard wanted, activity == nil else { return }
        guard UIApplication.shared.applicationState == .active else {
            NSLog("Sidevoice: the call's Live Activity waits until the app is in front")
            return
        }
        guard ActivityAuthorizationInfo().areActivitiesEnabled else {
            NSLog("Sidevoice: Live Activities are off for the app")
            return
        }
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
    }

    private func current() -> CallState {
        (report ?? Report()).state(route: AudioRoute.current())
    }

    /// Sends the state when it changed; `refresh`: even when it did not, to move its `staleDate`.
    private func publish(refresh: Bool = false) {
        guard let activity else { return }
        let state = current()
        if state == sent && !refresh { return }
        let wait = Self.captionInterval - Date().timeIntervalSince(sentAt)
        if !refresh, let sent, state.sameButText(sent), wait > 0 {
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
        let stale = Date().addingTimeInterval(Self.staleAfter)
        enqueue {
            if #available(iOS 16.2, *) {
                await activity.update(ActivityContent(state: state, staleDate: stale))
            } else {
                await activity.update(using: state)
            }
        }
    }

    /// Runs `work` after every call asked for before it.
    private func enqueue(_ work: @escaping () async -> Void) {
        let previous = queue
        queue = Task {
            await previous?.value
            await work()
        }
    }
}
