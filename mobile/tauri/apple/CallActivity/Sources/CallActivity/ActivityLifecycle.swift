import Foundation

/// Which activity the app is updating, and when it needs another: the bookkeeping of the call's Live Activity, apart from
/// ActivityKit so that it can be tested. The activity is the system's: it can end or be dismissed (the person swipes the
/// card away, the system ends it) without the app asking, and from then on updates to it go nowhere. So the activity is
/// held only while it lives, and a call that lasts and has none asks for a new one.
public struct ActivityLifecycle: Equatable {
    /// What ActivityKit says of an activity, reduced to what matters here.
    public enum Life: Equatable {
        /// Active, or stale (a stale activity is still on screen and takes updates).
        case alive
        /// Ended or dismissed: it takes no more updates and never returns.
        case over
    }

    /// What a call that lasts should do about its activity now.
    public enum Need: Equatable {
        /// Nothing: no call lasts, or its activity lives.
        case nothing
        /// Request one, now.
        case request
        /// It needs one, but the app is not in front (ActivityKit starts one only there): when it next is.
        case wait
    }

    /// The call lasts (started and not ended).
    public private(set) var wanted = false
    /// The identifier of the activity updates go to; nil if there is none (or the one there was is over).
    public private(set) var current: String?

    public init() {}

    /// A call starts: any activity of an earlier call is no longer ours.
    public mutating func start() {
        wanted = true
        current = nil
    }

    /// The call ends: nothing is wanted, and nothing is current.
    public mutating func end() {
        wanted = false
        current = nil
    }

    /// An activity was requested for the call.
    public mutating func adopt(_ id: String) {
        current = id
    }

    /// ActivityKit reported the state of activity `id`. Returns true if it was the current one and is over, so it was
    /// dropped (a report on an older activity, or on a live one, changes nothing).
    @discardableResult
    public mutating func observed(_ id: String, _ life: Life) -> Bool {
        guard life == .over, current == id else { return false }
        current = nil
        return true
    }

    public func need(inFront: Bool) -> Need {
        guard wanted, current == nil else { return .nothing }
        return inFront ? .request : .wait
    }
}

/// Whether the call's activity is on its way (`CallActivityBridge.running`): from the call's start until it is requested,
/// or found it cannot be. Set at once by the bridge's `start`, on the caller's thread, so the media controls asking on the
/// same thread right after already see it; cleared by `end` and by the controller once the activity is requested or
/// cannot be. Any thread.
public final class Coming: @unchecked Sendable {
    public static let now = Coming()
    private let lock = NSLock()
    private var value = false

    public init() {}

    /// A call starts. Its activity is on its way only if one can come: with Live Activities off for the app (`enabled`
    /// false) none ever will, and Now Playing must not wait for it, not even until the app is next in front (D2 = A's
    /// fallback: Now Playing and the headset's buttons from the call's first report).
    public func start(enabled: Bool) {
        set(enabled)
    }

    public func get() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return value
    }

    public func set(_ new: Bool) {
        lock.lock()
        defer { lock.unlock() }
        value = new
    }
}
