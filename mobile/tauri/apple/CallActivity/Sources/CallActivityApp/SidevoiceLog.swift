import CallActivity
import Foundation

/// The app's log for the unified log, which the Rust side finds by name (mobile/tauri/plugin/src/platform/call_surface/ios.rs,
/// `os_log`): every line of the app's log (`log::info!`, tauri-plugin-log) goes to the unified log through here, subsystem
/// `ai.sidevoice`, so that `log show` and a sysdiagnose have the app's own diagnostics.
@objc(SidevoiceLog)
public final class SidevoiceLog: NSObject {
    @objc public static func log(_ category: String, message: String) {
        CallLog.write(category, message)
    }
}
