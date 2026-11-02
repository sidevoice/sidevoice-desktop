package ai.sidevoice.app

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class AttachMediaControlsArgs {
    /** Where the session's commands go (`{command: "play" | "pause"}`; platform/media_controls/android.rs). */
    lateinit var onCommand: Channel
}

@InvokeArg
class UpdateMediaControlsArgs {
    var active: Boolean = false
    var playing: Boolean = false
    var title: String = ""
}

/**
 * The shared plugin's Kotlin class (Tauri keeps one per plugin; platform/kotlin.rs). Rust calls it; the page never does.
 *
 * **The microphone.** It asks the person for the microphone. Tauri's Plugin class already has the `checkPermissions` and `requestPermissions`
 * commands for the permissions this annotation lists, holds the activity result, and remembers a refusal for good. Rust
 * calls them (platform/microphone/android.rs); the page never does.
 *
 * The one command written here is `openSettings`: after a refusal for good the system no longer shows the prompt, and the
 * person can only undo it on the app's own page of the system's settings. That page is an intent, which the opener plugin
 * (an `ACTION_VIEW` of a URI) cannot send.
 *
 * **The media controls** ([MediaControls]): where their commands go, once, then the call as it changes
 * (platform/media_controls/android.rs). The media session is the call's service's.
 */
@TauriPlugin(
    permissions = [Permission(strings = [Manifest.permission.RECORD_AUDIO], alias = "microphone")]
)
class AppPlugin(private val activity: Activity) : Plugin(activity) {
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

    @Command
    fun attachMediaControls(invoke: Invoke) {
        val args = invoke.parseArgs(AttachMediaControlsArgs::class.java)
        MediaControls.attach { command -> args.onCommand.send(JSObject().also { it.put("command", command) }) }
        invoke.resolve()
    }

    @Command
    fun updateMediaControls(invoke: Invoke) {
        val args = invoke.parseArgs(UpdateMediaControlsArgs::class.java)
        MediaControls.update(args.active, args.playing, args.title)
        invoke.resolve()
    }
}
