package ai.sidevoice.mobile.plugin

import android.Manifest
import android.app.Activity
import android.app.KeyguardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.content.ComponentCallbacks
import android.content.res.Configuration
import android.os.PowerManager
import android.util.TypedValue
import android.view.WindowManager
import android.webkit.JavascriptInterface
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
     * The page is an app, not a document: it does not pinch-zoom, and shows no zoom buttons.
     *
     * Text size: an Android WebView does not follow the system's font scale by itself (its `textZoom` is 100 until the app says
     * otherwise), so the page would not grow with the person's setting. It is kept at 100 here, so that text is never
     * inflated twice, and the app tells the page the scale instead ([TextScale], the page's `host.textScale`), which the
     * page applies to its type alone, as a native app does.
     */
    override fun load(webView: WebView) {
        webView.settings.apply {
            setSupportZoom(false)
            builtInZoomControls = false
            displayZoomControls = false
            textZoom = 100
        }
        webView.addJavascriptInterface(TextScale(activity), TextScale.NAME)
        activity.registerComponentCallbacks(object : ComponentCallbacks {
            override fun onConfigurationChanged(newConfig: Configuration) {
                webView.post { webView.evaluateJavascript(TextScale.CHANGED, null) }
            }

            override fun onLowMemory() {}
        })
        // The activity was made anew by the call card's stack (its process outlived it).
        openConversations(activity.intent)
    }

    /** The app's activity is brought back by an intent: the call card's stack, maybe. */
    override fun onNewIntent(intent: Intent) {
        openConversations(intent)
    }

    /**
     * The person tapped the other conversations on the call's card ([CallService.EXTRA_OPEN_CONVERSATIONS]): reported to the
     * app as their action, which opens the page's conversation list. Handled once:
     * - an intent from the recents list is the task's first intent replayed (FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY), so the
     *   extra it may still carry is not a tap;
     * - the activity's intent is replaced by one without the extra, so the activity being made again (a rotation) does not
     *   open it twice.
     */
    private fun openConversations(intent: Intent?) {
        if (intent?.hasExtra(CallService.EXTRA_OPEN_CONVERSATIONS) != true) return
        val tapped = intent.getBooleanExtra(CallService.EXTRA_OPEN_CONVERSATIONS, false) &&
            (intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY) == 0
        activity.intent = Intent(intent).apply { removeExtra(CallService.EXTRA_OPEN_CONVERSATIONS) }
        if (!tapped) return
        CallSession.current()?.let { CallSession.action(it, "open-conversations") }
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
     * Tells the running service what the call shows now: its title, the page's mute (the system's call and its microphone
     * mute follow it) and the call surface its card draws (passed on as the JSON it came in). Nothing when none runs.
     */
    @Command
    fun updateCallService(invoke: Invoke) {
        val args = invoke.parseArgs(UpdateCallServiceArgs::class.java)
        val surface = invoke.getArgs().optJSONObject("surface")?.toString()
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
                    .putExtra(CallService.EXTRA_SURFACE, surface)
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
     * Whether the screen is locked now (`host.screen.locked()`, the page's "only with the screen off"): the keyguard shows (a
     * swipe one too), or the screen is off, which counts as locked (a phone with no lock at all has no keyguard). The app in
     * the background with the screen on is neither. Answered at once: both services may be asked from any thread.
     */
    @Command
    fun screenLocked(invoke: Invoke) {
        val context = activity.applicationContext
        val keyguard = context.getSystemService(KeyguardManager::class.java)
        val power = context.getSystemService(PowerManager::class.java)
        val locked = keyguard?.isKeyguardLocked == true || power?.isInteractive == false
        invoke.resolve(JSObject().put("locked", locked))
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

/**
 * The system's text scale for the page (`window.SidevoiceTextScale`, which the mobile bridge offers as `host.textScale`): how
 * much bigger than its default the person has set the text, as a multiple (1 is the default). From Android 14 the scale is
 * nonlinear (large text grows less than the small), so it is measured, not read from `Configuration.fontScale`: what 17sp
 * comes to in px, over what 17dp would. Read every time it is asked, so it is always the current one.
 */
class TextScale(private val activity: Activity) {
    @JavascriptInterface
    fun get(): Double {
        val metrics = activity.resources.displayMetrics
        val scale = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, REFERENCE_SP, metrics) / metrics.density / REFERENCE_SP
        return if (scale.isFinite() && scale > 0f) scale.toDouble() else 1.0
    }

    companion object {
        const val NAME = "SidevoiceTextScale"
        /** What the page listens for: the scale may have changed, ask again. */
        const val CHANGED = "window.dispatchEvent(new Event('sidevoice-text-scale'))"
        private const val REFERENCE_SP = 17f
    }
}
