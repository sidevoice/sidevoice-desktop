package ai.sidevoice.app

import android.Manifest
import android.app.Activity
import app.tauri.annotation.Permission
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Plugin

/**
 * Asks the person for the microphone. Nothing is written here: Tauri's Plugin class already has the `checkPermissions` and
 * `requestPermissions` commands for the permissions this annotation lists, holds the activity result, and remembers a
 * refusal for good. Rust calls them (platform/microphone/android.rs); the page never does.
 */
@TauriPlugin(
    permissions = [Permission(strings = [Manifest.permission.RECORD_AUDIO], alias = "microphone")]
)
class MicrophonePlugin(activity: Activity) : Plugin(activity)
