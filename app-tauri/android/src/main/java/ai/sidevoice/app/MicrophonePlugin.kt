package ai.sidevoice.app

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import app.tauri.annotation.Command
import app.tauri.annotation.Permission
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin

/**
 * Asks the person for the microphone. Tauri's Plugin class already has the `checkPermissions` and `requestPermissions`
 * commands for the permissions this annotation lists, holds the activity result, and remembers a refusal for good. Rust
 * calls them (platform/microphone/android.rs); the page never does.
 *
 * The one command written here is `openSettings`: after a refusal for good the system no longer shows the prompt, and the
 * person can only undo it on the app's own page of the system's settings. That page is an intent, which the opener plugin
 * (an `ACTION_VIEW` of a URI) cannot send.
 */
@TauriPlugin(
    permissions = [Permission(strings = [Manifest.permission.RECORD_AUDIO], alias = "microphone")]
)
class MicrophonePlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun openSettings(invoke: Invoke) {
        try {
            activity.startActivity(
                Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", activity.packageName, null))
            )
            invoke.resolve()
        } catch (ex: Exception) {
            invoke.reject(ex.message)
        }
    }
}
