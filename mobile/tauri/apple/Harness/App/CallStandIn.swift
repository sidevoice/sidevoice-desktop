// The call's stand-in in the harness app (../project.yml), for the Simulator tests of the Live Activity's buttons only: it is
// `SidevoiceCallSurfaceActions`, the class the app's Rust side registers (mobile/tauri/plugin/src/platform/call_surface/ios.rs),
// so a tap goes the real way: the system runs the button's intent in this app (../../Intents, `CallActions.send`), which
// finds this class by name and asks it for the action. It answers as the app's call would, with the call's details given
// again to the app's own bridge: a mute sets the mic, a hang-up ends the activity, and any other action shows the details
// the UI test gave for it. What it was asked is the test's hook (`Harness.heard`, on screen as `harness.actions`) and a line
// of the system log (`LA harness stand-in: …`). Debug only, like the fixture mode: no class of this name in a Release build.
#if DEBUG
import CallActivityApp
import Foundation

@objc(SidevoiceCallSurfaceActions)
final class CallStandIn: NSObject {
    /// The call's details as last shown (`title`, `muted`, `surface`), nil once it is hung up (main queue).
    @MainActor static var details: [String: Any]?
    /// The details to show after each action other than the mute's and the hang-up's, by the action's name (main queue).
    @MainActor static var answers: [String: [String: Any]] = [:]
    /// Told what was asked, and what the call did: `mute muted=true`, `skip`, … (main queue).
    @MainActor static var heard: @MainActor (String) -> Void = { _ in }

    /// A button of the Live Activity asked for `action` (from the intent's `perform`, on any thread), as the Rust side's
    /// `act:` is asked.
    @objc(act:)
    static func act(_ action: NSString) {
        let action = action as String
        NSLog("LA harness stand-in: %@ asked for by the Live Activity", action)
        Task { @MainActor in answer(action) }
    }

    @MainActor private static func answer(_ action: String) {
        guard var details else {
            NSLog("LA harness stand-in: %@ asked for with no call running", action)
            heard("\(action) no call")
            return
        }
        switch action {
        case "hang-up":
            self.details = nil
            CallActivityBridge.end()
            heard(action)
            return
        // `toggle-mute`: the other of the mute the card last showed (desktop#430's rule: a tap does what the card showed).
        case "mute", "unmute", "toggle-mute":
            let muted = action == "toggle-mute" ? !(details["muted"] as? Bool ?? false) : action == "mute"
            details["muted"] = muted
            heard("\(action) muted=\(muted)")
        default:
            guard let next = answers[action] else {
                heard("\(action) unanswered")
                return
            }
            details = next
            heard(action)
        }
        self.details = details
        // `refresh`: drawn again even if the activity has this state (the toggle flipped its own face); ignored where the
        // bridge does not read it.
        details["refresh"] = true
        guard let data = try? JSONSerialization.data(withJSONObject: details) else { return }
        CallActivityBridge.show(String(decoding: data, as: UTF8.self))
        NSLog("LA harness stand-in: %@ answered", action)
    }
}
#endif
