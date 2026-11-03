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
    class Session internal constructor(val id: Long, private val sink: (Map<String, Any>) -> Unit) {
        internal fun report(event: Map<String, Any>) = sink(event)
    }

    private val lock = Any()
    private var current: Session? = null
    private var last = 0L

    /** The session of a new start, reporting to [sink]. Null when one is running. */
    fun begin(sink: (Map<String, Any>) -> Unit): Session? = synchronized(lock) {
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

    /**
     * What names one session's Hang up: the data of its PendingIntent. Android tells PendingIntents apart by action,
     * component and data and not by extras, so the sessions' must differ in the data, or the notification a session left
     * behind would carry the next one's number.
     */
    fun hangUpAddress(id: Long): String = "sidevoice-call://hang-up/$id"

    /** What names one session's button [action] on the card (mute, unmute, skip, route), as [hangUpAddress] for its Hang up. */
    fun surfaceAddress(id: Long, action: String): String = "sidevoice-call://$action/$id"

    /**
     * Reports that the person asked for [action] on the call's card (`mute`, `unmute`, `skip`, `open-conversations`: Rust
     * `SurfaceAction`), if [session] is running; the app runs it on the page.
     */
    fun action(session: Session, action: String) {
        if (get(session.id) != null) session.report(mapOf("type" to "action", "action" to action))
    }

    /** Reports that the audio focus of [session] changed (`gained`, `lost`, `lost-transient`, `duck`, `refused`), if it is running. */
    fun focus(session: Session, state: String) {
        if (get(session.id) != null) session.report(mapOf("type" to "focus", "state" to state))
    }

    /** Reports that the system's call of [session] was muted or unmuted from outside the app, if it is running. */
    fun muted(session: Session, muted: Boolean) {
        if (get(session.id) != null) session.report(mapOf("type" to "muted", "muted" to muted))
    }

    /** Reports that the system put the call of [session] on hold, or took it off, if it is running. */
    fun held(session: Session, held: Boolean) {
        if (get(session.id) != null) session.report(mapOf("type" to "held", "held" to held))
    }

    /**
     * Reports that the call's audio of [session] may open now (Telecom made the call active and its route settled, or the
     * call stays the app's own), if it is running.
     */
    fun audioReady(session: Session) {
        if (get(session.id) != null) session.report(mapOf("type" to "audio", "active" to true))
    }

    /** What names one session's call to the system's Telecom (its address). */
    fun callAddress(id: Long): String = "sidevoice-call://call/$id"
}
