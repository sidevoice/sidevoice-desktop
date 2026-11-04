import AVFoundation
import Foundation
import SwiftUI

// The activity's controls. The buttons themselves are App Intents, which are not here: the device's `linkd` finds no
// metadata for intents of this package, linked statically into the app and the extension (TestFlight build 26: every tap
// failed with "There is no metadata for SetMicOff"). They are declared in the app's and the extension's own code
// (mobile/tauri/apple/Intents), which hands the views their buttons through `CallControls`; without them (the renders, the
// tests) the views draw the same faces, display only.

/// A button of the activity, by what it does.
public enum CallControl {
    case skip, cancelSend, hangUp, route
}

/// The buttons of the activity's views, set by the widget extension, whose code declares their intents. None: the faces
/// are drawn display only.
public enum CallControls {
    /// The face of `control` made its button.
    public static var button: ((CallControl, AnyView) -> AnyView)?
    /// The mute switch: the face, drawn from the toggle's own `isOn` (the system flips it at once when the button is
    /// tapped, and puts it right again from the activity's next state), made the toggle of the mic's state `micOff`.
    public static var muteToggle: ((_ micOff: Bool, _ face: @escaping (Bool) -> AnyView) -> AnyView)?
}

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

/// A control of the activity: its button where the extension gave one (iOS 17), the same face only otherwise.
struct ActionButton<Label: View>: View {
    typealias Action = CallControl
    var action: Action
    var enabled = true
    @ViewBuilder var label: () -> Label

    var body: some View {
        if enabled, let button = CallControls.button {
            button(action, AnyView(label()))
        } else {
            label()
        }
    }
}

/// The mute switch where the extension gave one (iOS 17), the face of `micOff` only otherwise.
struct MuteToggle<Face: View>: View {
    var micOff: Bool
    @ViewBuilder var face: (Bool) -> Face

    var body: some View {
        if let toggle = CallControls.muteToggle {
            toggle(micOff) { AnyView(face($0)) }
        } else {
            face(micOff)
        }
    }
}

/// A toggle drawn as its face alone: the face draws both states.
public struct FaceToggle: ToggleStyle {
    var face: (Bool) -> AnyView

    public init(face: @escaping (Bool) -> AnyView) {
        self.face = face
    }

    public func makeBody(configuration: Configuration) -> some View {
        face(configuration.isOn)
    }
}
