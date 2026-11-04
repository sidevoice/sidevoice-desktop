import AppIntents
import AVFoundation
import Foundation
import SwiftUI

// The activity's buttons (iOS 17): App Intents compiled into the app and the extension alike, run by the system in the
// app's process (`LiveActivityIntent`), where the call lives. Each asks the app's Rust side for the page's own action
// (`SidevoiceCallSurfaceActions`, mobile/tauri/plugin/src/platform/call_surface/ios.rs), except the audio route, which is
// the system's (AVAudioSession) and not the page's. None is offered to Shortcuts or Siri: they act on a running call only.

/// The page's actions, by the names `SurfaceAction` reads (`mute`, `unmute`, `skip`, `cancel-send`, `hang-up`).
enum CallActions {
    /// Hands the action to the app's Rust side, and returns when it has been handed over (the page acts after that, on
    /// the call's own threads). The app is kept running for the hand-over, whatever state the phone is in.
    static func send(_ action: String, from intent: String) async {
        NSLog("Sidevoice: intent %@ performing (pid %d)", intent, ProcessInfo.processInfo.processIdentifier)
        let delivered: Bool = await withCheckedContinuation { done in
            ProcessInfo.processInfo.performExpiringActivity(withReason: "Sidevoice: the call's \(action) button") { expired in
                guard !expired else {
                    done.resume(returning: false)
                    return
                }
                done.resume(returning: deliver(action))
            }
        }
        NSLog("Sidevoice: intent %@ %@", intent, delivered ? "handed to the call" : "found no call")
    }

    /// The class is the app's (Rust, objc2); the extension has none and never runs an intent. Without it this process has
    /// no call (the system started the app for the intent, after the call's process died): the card that asked is stale,
    /// so it is ended rather than left to look alive.
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
public struct SetMicOff: SetValueIntent, LiveActivityIntent {
    public static var title: LocalizedStringResource = "Mute the call"
    public static var isDiscoverable = false
    @Parameter(title: "Muted") public var value: Bool
    public init() {}
    public func perform() async throws -> some IntentResult {
        await CallActions.send(value ? "mute" : "unmute", from: "SetMicOff")
        return .result()
    }
}

@available(iOS 17.0, *)
public struct SkipReply: LiveActivityIntent {
    public static var title: LocalizedStringResource = "Skip the reply"
    public static var isDiscoverable = false
    public init() {}
    public func perform() async throws -> some IntentResult {
        await CallActions.send("skip", from: "SkipReply")
        return .result()
    }
}

@available(iOS 17.0, *)
public struct CancelSend: LiveActivityIntent {
    public static var title: LocalizedStringResource = "Cancel sending"
    public static var isDiscoverable = false
    public init() {}
    public func perform() async throws -> some IntentResult {
        await CallActions.send("cancel-send", from: "CancelSend")
        return .result()
    }
}

/// Ends the call through the page, like its red button; the app ends the activity (`.immediate`) when the call ends.
@available(iOS 17.0, *)
public struct HangUp: LiveActivityIntent {
    public static var title: LocalizedStringResource = "Hang up"
    public static var isDiscoverable = false
    public init() {}
    public func perform() async throws -> some IntentResult {
        await CallActions.send("hang-up", from: "HangUp")
        return .result()
    }
}

/// Moves the call between the headphones and the speaker (D-LA-ROUTE); the mic follows the route.
@available(iOS 17.0, *)
public struct ToggleRoute: LiveActivityIntent {
    public static var title: LocalizedStringResource = "Switch the audio output"
    public static var isDiscoverable = false
    public init() {}
    public func perform() async throws -> some IntentResult {
        NSLog("Sidevoice: intent ToggleRoute performing (pid %d)", ProcessInfo.processInfo.processIdentifier)
        AudioRoute.toggle()
        return .result()
    }
}

/// The intents of this package, for the app's and the extension's App Intents metadata.
@available(iOS 17.0, *)
public struct CallActivityIntents: AppIntentsPackage {}

/// The call's audio output while headphones are connected: the headphones, or the speaker by the app's override
/// (D-LA-ROUTE). The switch stays while the headphones do, whether or not they have a microphone: the ones moved away from
/// are remembered until they go (`headphonesGone()`, on the route change that says so).
public enum AudioRoute {
    static let headphonePorts: Set<AVAudioSession.Port> = [
        .headphones, .bluetoothA2DP, .bluetoothHFP, .bluetoothLE, .usbAudio, .carAudio,
    ]

    /// The headphones' name while the call is moved off them to the speaker (main queue; the intents run in the app).
    static var movedFrom: String?

    /// Headphones the call can use, and their name: the current output, or the speaker while the app has moved the call
    /// off headphones that are still there.
    public static func current() -> (route: CallState.Route, name: String)? {
        let session = AVAudioSession.sharedInstance()
        if let port = session.currentRoute.outputs.first(where: { headphonePorts.contains($0.portType) }) {
            movedFrom = nil
            return (.headphones, port.portName)
        }
        guard session.currentRoute.outputs.contains(where: { $0.portType == .builtInSpeaker }) else { return nil }
        if let movedFrom {
            return (.speaker, movedFrom)
        }
        // A headset's microphone still offered: its headphones are there.
        let headset = session.availableInputs?.first {
            $0.portType == .bluetoothHFP || $0.portType == .headsetMic || $0.portType == .bluetoothLE
        }
        return headset.map { (.speaker, $0.portName) }
    }

    /// One tap: from the headphones to the speaker, or back.
    public static func toggle() {
        let session = AVAudioSession.sharedInstance()
        if let port = session.currentRoute.outputs.first(where: { headphonePorts.contains($0.portType) }) {
            movedFrom = port.portName
            try? session.overrideOutputAudioPort(.speaker)
        } else {
            movedFrom = nil
            try? session.overrideOutputAudioPort(.none)
        }
    }

    /// The route changed because a device went away: if it was the headphones moved off, the switch goes with them.
    public static func routeChanged(_ notification: Notification) {
        let raw = notification.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt
        if raw.flatMap(AVAudioSession.RouteChangeReason.init(rawValue:)) == .oldDeviceUnavailable {
            movedFrom = nil
        }
    }
}

/// A control of the activity: the intent's button from iOS 17, the same face only before (display only).
struct ActionButton<Label: View>: View {
    enum Action { case skip, cancelSend, hangUp, route }
    var action: Action
    var enabled = true
    @ViewBuilder var label: () -> Label

    var body: some View {
        if enabled {
            if #available(iOS 17.0, *) {
                switch action {
                case .skip: Button(intent: SkipReply(), label: label).buttonStyle(.plain)
                case .cancelSend: Button(intent: CancelSend(), label: label).buttonStyle(.plain)
                case .hangUp: Button(intent: HangUp(), label: label).buttonStyle(.plain)
                case .route: Button(intent: ToggleRoute(), label: label).buttonStyle(.plain)
                }
            } else {
                label()
            }
        } else {
            label()
        }
    }
}

/// The mute switch (a `Toggle` on `SetMicOff`) from iOS 17. The face is drawn from the toggle's own `isOn`, which the
/// system flips at once when the button is tapped (and puts right again from the activity's next state), not from the
/// activity's state alone: the state changes only after the page's round trip.
struct MuteToggle<Face: View>: View {
    var micOff: Bool
    @ViewBuilder var face: (Bool) -> Face

    var body: some View {
        if #available(iOS 17.0, *) {
            Toggle(isOn: micOff, intent: SetMicOff()) { EmptyView() }
                .toggleStyle(FaceToggle(face: face))
        } else {
            face(micOff)
        }
    }
}

/// A toggle drawn as its face alone: the face draws both states.
struct FaceToggle<Face: View>: ToggleStyle {
    var face: (Bool) -> Face

    func makeBody(configuration: Configuration) -> some View {
        face(configuration.isOn)
    }
}
