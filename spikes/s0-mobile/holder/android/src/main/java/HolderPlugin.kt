package ai.sidevoice.spike.holder

import android.Manifest
import android.app.Activity
import android.content.Intent
import androidx.core.content.ContextCompat
import app.tauri.PermissionState
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class StartArgs {
    var dir: String? = null
}

/** S0 spike: run N's native audio on Android, called only by the app's Rust (`run_mobile_plugin`). */
@TauriPlugin(
    permissions = [
        Permission(strings = [Manifest.permission.RECORD_AUDIO], alias = "microphone"),
        Permission(strings = [Manifest.permission.POST_NOTIFICATIONS], alias = "notifications"),
    ]
)
class HolderPlugin(private val activity: Activity) : Plugin(activity) {
    /** Where the run logs go: the app's external files directory, which `adb pull` reads without root. */
    @Command
    fun logDir(invoke: Invoke) {
        val dir = activity.getExternalFilesDir(null) ?: activity.filesDir
        invoke.resolve(JSObject().put("path", dir.absolutePath))
    }

    /** Starts logging what the phone does (lock, screen, network) into the run directory, in both runs. */
    @Command
    fun watch(invoke: Invoke) {
        val dir = invoke.parseArgs(StartArgs::class.java).dir ?: return invoke.reject("no run directory", "no-dir")
        SystemWatch.start(activity, dir)
        invoke.resolve(JSObject().put("watching", true))
    }

    @Command
    fun unwatch(invoke: Invoke) {
        SystemWatch.stop()
        invoke.resolve(JSObject().put("watching", false))
    }

    /** Starts the foreground service (microphone type) while the app is visible, asking for the microphone first. */
    @Command
    fun start(invoke: Invoke) {
        if (getPermissionState("microphone") != PermissionState.GRANTED ||
            getPermissionState("notifications") == PermissionState.PROMPT
        ) {
            requestPermissionForAliases(arrayOf("microphone", "notifications"), invoke, "startAfterPermission")
            return
        }
        startNow(invoke)
    }

    @PermissionCallback
    private fun startAfterPermission(invoke: Invoke) {
        if (getPermissionState("microphone") != PermissionState.GRANTED) {
            invoke.reject("microphone permission denied", "microphone-denied")
            return
        }
        startNow(invoke)
    }

    private fun startNow(invoke: Invoke) {
        val args = invoke.parseArgs(StartArgs::class.java)
        val dir = args.dir ?: return invoke.reject("no run directory", "no-dir")
        try {
            val intent = Intent(activity, HolderService::class.java).putExtra(HolderService.EXTRA_DIR, dir)
            ContextCompat.startForegroundService(activity, intent)
            invoke.resolve(JSObject().put("started", true))
        } catch (e: Exception) {
            invoke.reject("the holder did not start: ${e.message}", "start-failed")
        }
    }

    @Command
    fun stop(invoke: Invoke) {
        activity.stopService(Intent(activity, HolderService::class.java))
        invoke.resolve(JSObject().put("stopped", true))
    }

    @Command
    fun status(invoke: Invoke) {
        invoke.resolve(HolderService.status())
    }
}
