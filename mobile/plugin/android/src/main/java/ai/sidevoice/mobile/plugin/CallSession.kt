package ai.sidevoice.mobile.plugin

/**
 * The call's service, as the process sees it: one session at a time, and one report to the app for each, whoever ends it.
 * The app's Kotlin plugin and the service are in the same process and meet here.
 *
 * Each start is its own [Session], and the service of that start carries it. The system ends a service on its own time,
 * after the app may have started the next call, so what a service does at its end (stop, report) is done to the session
 * it was started for, and is nothing when that session is no longer the current one.
 */
internal object CallSession {
    /** One start of the call's service, and where its reports go. */
    class Session internal constructor(val id: Long, private val sink: (Map<String, String>) -> Unit) {
        internal fun report(event: Map<String, String>) = sink(event)
    }

    private val lock = Any()
    private var current: Session? = null
    private var last = 0L

    /** The session of a new start, reporting to [sink]. Null when one is running. */
    fun begin(sink: (Map<String, String>) -> Unit): Session? = synchronized(lock) {
        if (current != null) null else Session(++last, sink).also { current = it }
    }

    /** The session that is running, if any. */
    fun current(): Session? = synchronized(lock) { current }

    /** The session numbered [id], when it is the one running: it may have ended, or been replaced, since it was started. */
    fun get(id: Long): Session? = synchronized(lock) { current?.takeIf { it.id == id } }

    /**
     * Ends [session] and reports `stopped` (unless [report] is false), once. False, and nothing done, when it is not the
     * session that is running: it already ended, or a later one has taken its place.
     */
    fun end(session: Session, reason: String, report: Boolean = true): Boolean {
        synchronized(lock) {
            if (current !== session) return false
            current = null
        }
        if (report) session.report(mapOf("type" to "stopped", "reason" to reason))
        return true
    }

    /** Reports that the audio focus of [session] changed (`gained`, `lost`, `lost-transient`, `duck`), if it is running. */
    fun focus(session: Session, state: String) {
        if (get(session.id) != null) session.report(mapOf("type" to "focus", "state" to state))
    }
}
