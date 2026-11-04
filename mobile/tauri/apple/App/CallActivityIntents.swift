// The app target's own Swift (mobile/tauri/ios-project.yml). The call's Live Activity runs its buttons here: their App
// Intents are compiled into the app (mobile/tauri/apple/Intents), so that its App Intents metadata lists them as its own.
import CallActivityApp

/// The class the Rust side starts, updates and ends the activity through, named here so the linker keeps it.
let callActivityBridge: AnyClass = CallActivityBridge.self
