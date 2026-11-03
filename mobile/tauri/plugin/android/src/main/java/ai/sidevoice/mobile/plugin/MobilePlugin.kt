package ai.sidevoice.mobile.plugin

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.view.WindowManager
import android.webkit.WebView
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import app.tauri.plugin.PluginManager

@InvokeArg
class StartCallServiceArgs {
    /** The conversation the call is on (empty for none): the system shows it as the call's other party. */
    var title: String = ""
    /** Whether the call is also a call of the system's Telecom (off: the app's own call, without Telecom). */
    var systemCall: Boolean = false
    /** Where the service reports (`CallEvent`, in Rust). */
    lateinit var onEvent: Channel
}

@InvokeArg
class UpdateCallServiceArgs {
    var title: String = ""
    /** Whether the page has the call muted. */
    var muted: Boolean = false
}

@InvokeArg
class KeepScreenOnArgs {
    /** Whether the screen is kept on (`true`) or may turn off again. */
    var on: Boolean = false
}

/** The app's Rust asks for the call's service ([CallService]) here; no page does. */
@TauriPlugin
class MobilePlugin(private val activity: Activity) : Plugin(activity) {
    /**
     * The page is an app, not a document: it does not pinch-zoom, and shows no zoom buttons. (Text size is the system's
     * font scale, which the page follows.)
     */
    override fun load(webView: WebView) {
        webView.settings.apply {
            setSupportZoom(false)
            builtInZoomControls = false
            displayZoomControls = false
        }
    }

    /**
     * Starts the service. Android lets a microphone service start only from a visible app and with the microphone
     * allowed, so a call that is not is refused here, by name, instead of by the system's exception. Once started,
     * every way it ends is reported on the channel, once.
     */
    @Command
    fun startCallService(invoke: Invoke) {
        val args = invoke.parseArgs(StartCallServiceArgs::class.java)
        val context = activity.applicationContext
        if (!visible()) {
            invoke.reject("The call can start only while the app is on screen", "not-visible")
            return
        }
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            invoke.reject("The microphone is not allowed", "microphone-permission")
            return
        }
        val session = CallSession.begin { event ->
            args.onEvent.send(JSObject().also { reported -> event.forEach { (key, value) -> reported.put(key, value) } })
        }
        if (session == null) {
            invoke.reject("A call's service is already running", "already-running")
            return
        }
        try {
            ContextCompat.startForegroundService(
                context,
                Intent(context, CallService::class.java)
                    .setAction(CallService.ACTION_START)
                    .putExtra(CallService.EXTRA_SESSION, session.id)
                    .putExtra(CallService.EXTRA_TITLE, args.title)
                    .putExtra(CallService.EXTRA_SYSTEM_CALL, args.systemCall)
            )
            invoke.resolve()
        } catch (error: RuntimeException) {
            // The system refused at once (ForegroundServiceStartNotAllowedException is one): nothing runs.
            CallSession.end(session, "system", report = false)
            invoke.reject(error.message ?: "The system refused the call's service", "not-allowed")
        }
    }

    /** Asks the running service to request the audio focus again; its answer comes as a `focus` report. */
    @Command
    fun requestCallFocus(invoke: Invoke) {
        val context = activity.applicationContext
        val session = CallSession.current()
        if (session == null) {
            invoke.reject("No call's service is running", "not-running")
            return
        }
        context.startService(
            Intent(context, CallService::class.java)
                .setAction(CallService.ACTION_REFOCUS)
                .putExtra(CallService.EXTRA_SESSION, session.id)
        )
        invoke.resolve()
    }

    /**
     * Tells the running service what the call shows now: its title, and the page's mute (the system's call and its
     * microphone mute follow it). Nothing when none runs.
     */
    @Command
    fun updateCallService(invoke: Invoke) {
        val args = invoke.parseArgs(UpdateCallServiceArgs::class.java)
        val context = activity.applicationContext
        val session = CallSession.current()
        if (session == null) {
            invoke.resolve()
            return
        }
        try {
            context.startService(
                Intent(context, CallService::class.java)
                    .setAction(CallService.ACTION_UPDATE)
                    .putExtra(CallService.EXTRA_SESSION, session.id)
                    .putExtra(CallService.EXTRA_TITLE, args.title)
                    .putExtra(CallService.EXTRA_MUTED, args.muted)
            )
            invoke.resolve()
        } catch (error: RuntimeException) {
            invoke.reject(error.message ?: "The call's service could not be reached", "not-allowed")
        }
    }

    /** Stops the service, if it runs: it reports `requested`, and the notification and the focus are gone. */
    @Command
    fun stopCallService(invoke: Invoke) {
        val context = activity.applicationContext
        val session = CallSession.current()
        if (session != null && CallSession.end(session, "requested")) {
            context.stopService(Intent(context, CallService::class.java))
        }
        invoke.resolve()
    }

    /**
     * Keeps the screen on (`FLAG_KEEP_SCREEN_ON` on the activity's window) or lets it turn off again. The window is the
     * UI thread's: the flag is set there, and the answer comes after.
     */
    @Command
    fun keepScreenOn(invoke: Invoke) {
        val args = invoke.parseArgs(KeepScreenOnArgs::class.java)
        activity.runOnUiThread {
            if (args.on) {
                activity.window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            } else {
                activity.window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
            invoke.resolve()
        }
    }

    /**
     * Whether the app's activity is on screen now (started: visible, whether or not it has the focus), as that activity
     * itself says. The process's own lifecycle reports a stop some time late, and a start just after the app went to the
     * background would pass it.
     */
    private fun visible(): Boolean {
        val current = PluginManager.activity ?: return false
        return current.lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)
    }
}
