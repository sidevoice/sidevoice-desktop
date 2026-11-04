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
