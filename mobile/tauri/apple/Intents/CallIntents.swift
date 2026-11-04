// The Live Activity's buttons (iOS 17), compiled into the app and into the widget extension alike
// (mobile/tauri/ios-project.yml), as Apple's Live Activity intents are: each bundle's own App Intents metadata lists them as
// its own, and the system runs a `LiveActivityIntent` in the app's process, where the call lives. They are not in the
// CallActivity package: the device found no metadata for the intents of a package linked statically (`linkd`:
// `aggregateMetadataIsEmpty`), so no button ran (`cargo xtask mobile check-ios` refuses that now).
//
// Each asks the app's Rust side for the call's action (`SidevoiceCallSurfaceActions`,
// mobile/tauri/plugin/src/platform/call_surface/ios.rs), except the audio route, which is the system's (AVAudioSession).
// None is offered to Shortcuts or Siri: they act on a running call only.
import AppIntents
import CallActivity
import Foundation
import SwiftUI

/// The call's actions, by the names `SurfaceAction` reads (`mute`, `unmute`, `skip`, `cancel-send`, `hang-up`).
enum CallActions {
    /// Hands the action to the app's Rust side; the hand-over is synchronous (the call acts after it, on its own threads).
    static func send(_ action: String, from intent: String) {
        let info = ProcessInfo.processInfo
        let process = Bundle.main.bundlePath.hasSuffix(".appex") ? "widget extension" : "app"
        CallLog.intent.notice(
            "intent \(intent, privacy: .public) performing: \(action, privacy: .public) in the \(process, privacy: .public) (\(info.processName, privacy: .public), pid \(info.processIdentifier))"
        )
        let handed = deliver(action)
        CallLog.intent.notice(
            "intent \(intent, privacy: .public): \(handed ? "handed to the call" : "found no call; the card is stale and is ended", privacy: .public)"
        )
    }

    /// The class is the app's (Rust, objc2), registered when the call's activity starts. Without it this process has no
    /// call (the system started the app for the intent after the call's process died, or this is not the app): the card
    /// that asked is stale, so it is ended rather than left to look alive.
    private static func deliver(_ action: String) -> Bool {
        guard let actions: AnyClass = NSClassFromString("SidevoiceCallSurfaceActions") else {
            if let bridge = NSClassFromString("SidevoiceCallActivity") as AnyObject? {
                _ = bridge.perform(NSSelectorFromString("end"))
            }
            return false
        }
        _ = (actions as AnyObject).perform(NSSelectorFromString("act:"), with: action as NSString)
        return true
    }
}

@available(iOS 17.0, *)
struct SetMicOff: SetValueIntent, LiveActivityIntent {
    static var title: LocalizedStringResource = "Mute the call"
    static var isDiscoverable = false
    @Parameter(title: "Muted") var value: Bool
    init() {}
    func perform() async throws -> some IntentResult {
        CallActions.send(value ? "mute" : "unmute", from: "SetMicOff")
        return .result()
    }
}

@available(iOS 17.0, *)
struct SkipReply: LiveActivityIntent {
    static var title: LocalizedStringResource = "Skip the reply"
    static var isDiscoverable = false
    init() {}
    func perform() async throws -> some IntentResult {
        CallActions.send("skip", from: "SkipReply")
        return .result()
    }
}

@available(iOS 17.0, *)
struct CancelSend: LiveActivityIntent {
    static var title: LocalizedStringResource = "Cancel sending"
    static var isDiscoverable = false
    init() {}
    func perform() async throws -> some IntentResult {
        CallActions.send("cancel-send", from: "CancelSend")
        return .result()
    }
}

/// Ends the call; the app ends the activity (`.immediate`) when the call ends.
@available(iOS 17.0, *)
struct HangUp: LiveActivityIntent {
    static var title: LocalizedStringResource = "Hang up"
    static var isDiscoverable = false
    init() {}
    func perform() async throws -> some IntentResult {
        CallActions.send("hang-up", from: "HangUp")
        return .result()
    }
}

/// Moves the call between the headphones and the speaker (D-LA-ROUTE); the mic follows the route.
@available(iOS 17.0, *)
struct ToggleRoute: LiveActivityIntent {
    static var title: LocalizedStringResource = "Switch the audio output"
    static var isDiscoverable = false
    init() {}
    func perform() async throws -> some IntentResult {
        CallLog.intent.notice("intent ToggleRoute performing (pid \(ProcessInfo.processInfo.processIdentifier))")
        AudioRoute.toggle()
        CallLog.intent.notice("intent ToggleRoute: switched the output (native; no call needed)")
        return .result()
    }
}

/// Gives the activity's views (the CallActivity package's) their buttons: the intents above. The widget extension, which
/// draws them, calls it once.
enum CallIntentControls {
    static func install() {
        guard #available(iOS 17.0, *) else { return }
        CallControls.button = { control, label in
            switch control {
            case .skip: return AnyView(Button(intent: SkipReply()) { label }.buttonStyle(.plain))
            case .cancelSend: return AnyView(Button(intent: CancelSend()) { label }.buttonStyle(.plain))
            case .hangUp: return AnyView(Button(intent: HangUp()) { label }.buttonStyle(.plain))
            case .route: return AnyView(Button(intent: ToggleRoute()) { label }.buttonStyle(.plain))
            }
        }
        CallControls.muteToggle = { micOff, face in
            AnyView(Toggle(isOn: micOff, intent: SetMicOff()) { EmptyView() }.toggleStyle(FaceToggle(face: face)))
        }
    }
}
