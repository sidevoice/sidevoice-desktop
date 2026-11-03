// The app target's own Swift (mobile/tauri/ios-project.yml): the call's Live Activity runs its buttons here.
import AppIntents
import CallActivity
import CallActivityApp

/// The App Intents the app runs: the Live Activity's buttons (`LiveActivityIntent`s of the CallActivity package), listed
/// so that the app's App Intents metadata has them.
@available(iOS 17.0, *)
struct SidevoiceAppIntents: AppIntentsPackage {
    static var includedPackages: [any AppIntentsPackage.Type] { [CallActivityIntents.self] }
}

/// The class the Rust side starts, updates and ends the activity through, named here so the linker keeps it.
let callActivityBridge: AnyClass = CallActivityBridge.self
