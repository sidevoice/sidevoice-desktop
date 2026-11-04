// The widget extension (mobile/tauri/ios-project.yml, target SidevoiceActivity): it draws the call's Live Activity, whose
// views and font are the CallActivity package's, and whose buttons are the intents compiled into it and into the app
// (mobile/tauri/apple/Intents; they run in the app).
import CallActivity
import SwiftUI
import WidgetKit

@main
struct SidevoiceActivityWidgets: WidgetBundle {
    init() {
        CallIntentControls.install()
    }

    var body: some Widget {
        CallActivityWidget()
    }
}
