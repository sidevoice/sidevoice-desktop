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
        if #available(iOS 16.1, *) { Coming.now.set(true) }
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
        Coming.now.set(false)
        DispatchQueue.main.async {
            if #available(iOS 16.1, *) { Controller.shared.end() }
        }
    }

    /// Whether the call's Live Activity is on the screens now (requested and not over), or on its way: from `start` until
    /// it is requested, also while the app is not in front to request it. False before iOS 16.1, with Live Activities off,
    /// and once a request has failed. Any thread: the media controls ask on every report, and give way to the activity
    /// only while this holds. "On its way" counts because the first report comes before the main queue has requested the
    /// activity: Now Playing shown for that moment would leave the app the system's Now Playing app for the rest of the
    /// process, an empty card above the activity (HANDOVER-live-activity R2-6).
    @objc public static func running() -> Bool {
        guard #available(iOS 16.1, *) else { return false }
        return Coming.now.get() || Activity<CallActivityAttributes>.activities.contains {
            $0.activityState != .ended && $0.activityState != .dismissed
        }
    }
}

/// Whether the call's activity is on its way (`CallActivityBridge.running`): set at once by `start`, from the caller's
/// thread, and cleared by `end` and by the controller once the activity is requested or cannot be.
final class Coming: @unchecked Sendable {
    static let now = Coming()
    private let lock = NSLock()
    private var value = false

    func get() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return value
    }

    func set(_ new: Bool) {
        lock.lock()
        defer { lock.unlock() }
        value = new
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

    /// How much of the spoken page the voice has said (UTF-16 units, at a word's start): where the chunker's pager is.
    var heard: Int {
        surface?.page?.lit ?? 0
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

/// Starts, updates and ends the activity (main queue). Every change goes out at once; the words change once per chunk,
/// at least 2.5 s apart (`TextPager`), so they never come too often for iOS's throttle (HANDOVER §5). Each update moves `staleDate` three minutes ahead, and while the call lasts the activity is updated at
/// least every minute even when nothing changed (the working timer is the system's), so a live call never reads stale.
/// The updates and the end go to ActivityKit one after the other, in order.
///
/// An activity can be requested only while the app is in front: a call that starts in the background (a page reloaded
/// there) gets its activity when the app next comes to the front, if the call still lasts.
@available(iOS 16.1, *)
final class Controller {
    static let shared = Controller()

    /// The activity updates go to, while it lives; `lifecycle` says which it is and when it is over.
    private var activity: Activity<CallActivityAttributes>?
    private var lifecycle = ActivityLifecycle()
    private var report: Report?
    private var sent: CallState?
    private var sentAt = Date.distantPast
    /// The words in chunks (`TextPager`): one chunk per update, at least 2.5 s each, never ahead of the voice. The page's
    /// text is chunked here, where the updates are made and DM Sans is at hand; `wake` is when the next chunk is due.
    private var pager = TextPager(chunker: .dmSans())
    private var wake: DispatchWorkItem?
    private var refresh: Timer?
    private var observers: [NSObjectProtocol] = []
    /// When a request last failed: publishing does not ask again for `retryAfter` (a persistent error would otherwise be
    /// asked for on every state), while coming to the front always does.
    private var failedAt = Date.distantPast
    /// Watches the current activity's state for its end.
    private var watching: Task<Void, Never>?
    /// The last of ActivityKit's calls asked for: each waits for the one before.
    private var queue: Task<Void, Never>?

    static let staleAfter: TimeInterval = 180
    static let refreshInterval: TimeInterval = 60
    static let retryAfter: TimeInterval = 30

    init() {
        let center = NotificationCenter.default
        observers = [
            center.addObserver(forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main) { [weak self] note in
                AudioRoute.routeChanged(note)
                self?.publish()
            },
            center.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
                self?.dropIfOver()
                self?.endOrphans()
                self?.failedAt = .distantPast
                self?.requestIfWanted()
            },
        ]
    }

    func start(_ report: Report?) {
        end()
        lifecycle.start()
        self.report = report
        requestIfWanted()
        refresh = Timer.scheduledTimer(withTimeInterval: Self.refreshInterval, repeats: true) { [weak self] _ in
            self?.publish(refresh: true)
        }
    }

    func show(_ report: Report?) {
        guard lifecycle.wanted else { return }
        self.report = report
        publish()
    }

    /// Ends the call's activity, and any other of the app's (one an earlier run left), at once.
    func end() {
        lifecycle.end()
        wake?.cancel()
        wake = nil
        pager = TextPager(chunker: .dmSans())
        refresh?.invalidate()
        refresh = nil
        watching?.cancel()
        watching = nil
        activity = nil
        sent = nil
        report = nil
        let all = Activity<CallActivityAttributes>.activities
        NSLog("Sidevoice: Live Activity: ending %d", all.count)
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

    /// Ends the app's activities that are not the call's current one: what an earlier process (a crash, a kill) left,
    /// which nothing updates and which would show its last state for good. Also done by `end()` at launch, but ActivityKit
    /// may not list them yet that early, so it is done again whenever the app comes to the front.
    private func endOrphans() {
        let orphans = Activity<CallActivityAttributes>.activities.filter { $0.id != lifecycle.current }
        guard !orphans.isEmpty else { return }
        NSLog("Sidevoice: Live Activity: ending %d orphan(s)", orphans.count)
        enqueue {
            for activity in orphans {
                if #available(iOS 16.2, *) {
                    await activity.end(nil, dismissalPolicy: .immediate)
                } else {
                    await activity.end(using: nil, dismissalPolicy: .immediate)
                }
            }
        }
    }

    /// Forgets the activity if the system has ended it or the person has dismissed it, so a later `requestIfWanted` asks
    /// for another one instead of updating one that no longer exists.
    private func dropIfOver() {
        guard let activity else { return }
        drop(activity.id, Self.life(activity.activityState))
    }

    /// Drops activity `id` if it is the current one and over. The system may leave an ended activity on the Lock Screen
    /// for hours, next to the call's new one, so it is ended for good. Returns whether it was dropped.
    @discardableResult
    private func drop(_ id: String, _ life: ActivityLifecycle.Life) -> Bool {
        guard lifecycle.observed(id, life) else { return false }
        NSLog("Sidevoice: Live Activity: %@ is over; the call has none now", id)
        let over = activity
        watching?.cancel()
        watching = nil
        activity = nil
        sent = nil
        if let over {
            enqueue {
                if #available(iOS 16.2, *) {
                    await over.end(nil, dismissalPolicy: .immediate)
                } else {
                    await over.end(using: nil, dismissalPolicy: .immediate)
                }
            }
        }
        return true
    }

    private static func life(_ state: ActivityState) -> ActivityLifecycle.Life {
        switch state {
        case .ended, .dismissed: return .over
        default: return .alive
        }
    }

    /// Requests the activity for the call that lasts, if it has none and the app is in front.
    private func requestIfWanted() {
        dropIfOver()
        switch lifecycle.need(inFront: UIApplication.shared.applicationState == .active) {
        case .nothing:
            return
        case .wait:
            NSLog("Sidevoice: the call's Live Activity waits until the app is in front")
            return
        case .request:
            break
        }
        guard ActivityAuthorizationInfo().areActivitiesEnabled else {
            NSLog("Sidevoice: Live Activities are off for the app")
            Coming.now.set(false)
            return
        }
        let state = current()
        let new: Activity<CallActivityAttributes>
        do {
            if #available(iOS 16.2, *) {
                new = try Activity.request(
                    attributes: CallActivityAttributes(),
                    content: ActivityContent(state: state, staleDate: Date().addingTimeInterval(Self.staleAfter)),
                    pushType: nil)
            } else {
                new = try Activity.request(attributes: CallActivityAttributes(), contentState: state, pushType: nil)
            }
        } catch {
            failedAt = Date()
            Coming.now.set(false)
            NSLog("Sidevoice: the call's Live Activity did not start: \(error)")
            return
        }
        Coming.now.set(false)
        NSLog("Sidevoice: Live Activity: requested %@", new.id)
        activity = new
        lifecycle.adopt(new.id)
        sent = state
        sentAt = Date()
        watch(new)
    }

    /// Follows the activity's state, so that its end (by the system, or the person's swipe) is noticed when it happens,
    /// and the next state of the call asks for a new one.
    private func watch(_ activity: Activity<CallActivityAttributes>) {
        watching?.cancel()
        let id = activity.id
        watching = Task { [weak self] in
            for await state in activity.activityStateUpdates {
                let life = Self.life(state)
                await MainActor.run {
                    if self?.drop(id, life) == true { self?.requestIfWanted() }
                }
                if life == .over { break }
            }
        }
    }

    private func current() -> CallState {
        let report = report ?? Report()
        var state = report.state(route: AudioRoute.current())
        let kind: TextPager.Kind? = state.phase == .speaking ? .reply : state.phase == .said ? .said : nil
        // Your words are kept by conversation: another one's never show under this one's title.
        let scope = "\(state.creature?.file ?? "")|\(state.title)"
        let page = pager.page(
            kind: kind, text: state.text, heard: report.heard, holdable: TextPager.holds(state.phase), scope: scope, now: Date())
        state.text = page.text
        // Your words outlast the page's 3 s: they go on as your words, under the header of the state the call is really in.
        if page.held {
            state.heldPhase = state.phase
            state.phase = .said
        }
        due = page.wake
        return state
    }

    /// When the next chunk is due, as the last `current()` found.
    private var due: Date?

    /// Looks again when the next chunk is due (its predecessor has been up 2.5 s): the voice may be past it, and no report
    /// may come to say so.
    private func scheduleWake() {
        wake?.cancel()
        wake = nil
        guard let due else { return }
        let work = DispatchWorkItem { [weak self] in
            self?.wake = nil
            self?.publish()
        }
        wake = work
        DispatchQueue.main.asyncAfter(deadline: .now() + max(0, due.timeIntervalSinceNow), execute: work)
    }

    /// Sends the state when it changed; `refresh`: even when it did not, to move its `staleDate`.
    private func publish(refresh: Bool = false) {
        dropIfOver()
        if activity == nil, Date().timeIntervalSince(failedAt) > Self.retryAfter { requestIfWanted() }
        guard let activity else { return }
        let state = current()
        scheduleWake()
        if state == sent && !refresh { return }
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
