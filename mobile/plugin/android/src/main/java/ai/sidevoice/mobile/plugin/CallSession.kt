package ai.sidevoice.mobile.plugin

import app.tauri.plugin.JSObject

/**
 * The call's service, as the process sees it: one at a time, and one report to the app when it ends, whoever ends it.
 * The app's Kotlin plugin and the service are in the same process and meet here.
 */
internal object CallSession {
    private val lock = Any()
    private var sink: ((JSObject) -> Unit)? = null

    /** Whether a call's service has been started and has not ended. */
    val running: Boolean get() = synchronized(lock) { sink != null }

    /** Claims the session for a new call, reporting to [sink]. False when one is running. */
    fun begin(sink: (JSObject) -> Unit): Boolean = synchronized(lock) {
        if (this.sink != null) false else { this.sink = sink; true }
    }

    /** Ends the session and reports `stopped` (unless [report] is false), once. False when none was running. */
    fun end(reason: String, report: Boolean = true): Boolean {
        val last = synchronized(lock) {
            val last = sink ?: return false
            sink = null
            last
        }
        if (report) last(JSObject().put("type", "stopped").put("reason", reason))
        return true
    }

    /** Reports that the call's audio focus changed (`gained`, `lost`, `lost-transient`, `duck`), if a call is running. */
    fun focus(state: String) {
        val current = synchronized(lock) { sink } ?: return
        current(JSObject().put("type", "focus").put("state", state))
    }
}
