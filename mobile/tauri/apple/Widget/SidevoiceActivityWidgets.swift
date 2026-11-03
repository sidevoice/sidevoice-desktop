// The widget extension (mobile/tauri/ios-project.yml, target SidevoiceActivity): it draws the call's Live Activity, whose
// views, intents and font are the CallActivity package's.
import AppIntents
import CallActivity
import SwiftUI
import WidgetKit

@main
struct SidevoiceActivityWidgets: WidgetBundle {
    var body: some Widget {
        CallActivityWidget()
    }
}

/// The intents the extension's buttons name (they run in the app).
@available(iOS 17.0, *)
struct SidevoiceActivityIntents: AppIntentsPackage {
    static var includedPackages: [any AppIntentsPackage.Type] { [CallActivityIntents.self] }
}
