// The Live Activity harness's host app (../project.yml), for the Simulator tests only. Started by the UI tests with a
// fixture of the call's details in `LA_FIXTURE` (the JSON the Rust side hands `SidevoiceCallActivity`: `title`, `muted`,
// `surface`), it does what the app does with one: it writes the creatures' stills where the app writes them (the App
// Group's `call-stills/`, each under the file name the Rust side gives it), then hands the details to the app's own bridge
// (`CallActivityBridge.start`), whose controller requests and updates the activity. Its status line says when the activity
// runs. The call's stand-in (CallStandIn.swift) answers the activity's buttons, with the details `LA_ANSWERS` gives by
// action name, and `harness.actions` says what they asked. The fixture mode is compiled in Debug only, and this app is
// never part of the app that ships.
import ActivityKit
import CallActivity
import CallActivityApp
import SwiftUI

@main
struct HarnessApp: App {
    @StateObject private var harness = Harness()

    var body: some Scene {
        WindowGroup {
            VStack {
                Text(harness.status)
                    .accessibilityIdentifier("harness.status")
                Text(harness.actions)
                    .accessibilityIdentifier("harness.actions")
            }
            .task { await harness.run() }
        }
    }
}

@MainActor
final class Harness: ObservableObject {
    @Published var status = "waiting"
    /// What the activity's buttons asked the call's stand-in, in order, comma-separated; "none" before any.
    @Published var actions = "none"

    #if DEBUG
    func run() async {
        let environment = ProcessInfo.processInfo.environment
        guard let fixture = environment["LA_FIXTURE"] else {
            status = "no fixture"
            return
        }
        // ActivityKit starts an activity only for an app in front, as the app's controller knows.
        while UIApplication.shared.applicationState != .active {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        guard let details = Self.object(fixture) else {
            status = "failed: LA_FIXTURE is not a JSON object"
            return
        }
        CallStandIn.details = details
        CallStandIn.answers = (environment["LA_ANSWERS"].flatMap(Self.object) ?? [:]).compactMapValues {
            $0 as? [String: Any]
        }
        CallStandIn.heard = { [weak self] said in
            guard let self else { return }
            actions = actions == "none" ? said : "\(actions), \(said)"
        }
        guard writeStills() else {
            status = "failed: no App Group container"
            return
        }
        // The controller ends what an earlier fixture left before it requests its own.
        let before = Set(Activity<CallActivityAttributes>.activities.map(\.id))
        CallActivityBridge.start(fixture)
        guard await running(besides: before) != nil else {
            status = "failed: the activity did not start"
            return
        }
        guard await ended(before) else {
            status = "failed: an earlier fixture's activity did not end"
            return
        }
        status = "started"
    }

    private static func object(_ json: String) -> [String: Any]? {
        (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any]
    }
    #else
    func run() async {
        status = "fixtures are Debug only"
    }
    #endif

    /// The Rust side's file name of still `id` (`Still::file_name`): the id's bytes in hex.
    static func file(_ id: String) -> String {
        id.utf8.map { String(format: "%02x", $0) }.joined() + ".png"
    }

    /// Copies the bundled stills into the App Group's stills folder under the Rust side's file names, the id being the
    /// PNG's base name. False without the container.
    private func writeStills() -> Bool {
        guard let directory = CallStills.directory else { return false }
        let manager = FileManager.default
        try? manager.createDirectory(at: directory, withIntermediateDirectories: true)
        let stills = Bundle.main.urls(forResourcesWithExtension: "png", subdirectory: "Fixtures") ?? []
        for still in stills {
            let target = directory.appendingPathComponent(Self.file(still.deletingPathExtension().lastPathComponent))
            try? manager.removeItem(at: target)
            guard (try? manager.copyItem(at: still, to: target)) != nil else { return false }
        }
        return !stills.isEmpty
    }

    /// The id of an active activity not in `besides`, once there is one (up to 10 s).
    private func running(besides: Set<String>) async -> String? {
        for _ in 0..<100 {
            if let activity = Activity<CallActivityAttributes>.activities.first(where: {
                $0.activityState == .active && !besides.contains($0.id)
            }) {
                return activity.id
            }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        return nil
    }

    /// Whether no activity in `ids` is active any more, once so (up to 10 s).
    private func ended(_ ids: Set<String>) async -> Bool {
        for _ in 0..<100 {
            if !Activity<CallActivityAttributes>.activities.contains(where: {
                ids.contains($0.id) && $0.activityState == .active
            }) {
                return true
            }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        return false
    }
}
