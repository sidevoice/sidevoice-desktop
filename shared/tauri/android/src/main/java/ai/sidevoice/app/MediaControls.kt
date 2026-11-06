package ai.sidevoice.app

/**
 * The call's media controls as the shared plugin knows them: the call's state (from Rust, platform/media_controls/android.rs)
 * and where the controls' commands go (to Rust, which decides what they do to the call, by the same rule as on macOS
 * and iOS: play unmutes, pause toggles).
 *
 * The media session itself is the call's foreground service's (the mobile app's `CallService`): it exists only while the
 * call runs, follows [state] through [observe], and hands its play and pause, and its notification's mute button, to
 * [command].
 */
object MediaControls {
    /** The call as the controls show it: whether there is one, whether it plays (the microphone is open), its title. */
    data class State(val active: Boolean, val playing: Boolean, val title: String)

    private val lock = Any()
    private var send: ((String) -> Unit)? = null
    private var state = State(active = false, playing = false, title = "")
    private val listeners = mutableListOf<(State) -> Unit>()

    /** Where the commands go, once, when the app starts. */
    fun attach(send: (String) -> Unit) = synchronized(lock) { this.send = send }

    /** The call changed: every observer hears it. */
    fun update(active: Boolean, playing: Boolean, title: String) {
        val (now, observers) = synchronized(lock) {
            state = State(active, playing, title)
            state to listeners.toList()
        }
        observers.forEach { it(now) }
    }

    /** The call as it is now. */
    fun state(): State = synchronized(lock) { state }

    /** A command of the controls: `play` or `pause` (the single press), `next` (the double press). */
    fun command(name: String) {
        synchronized(lock) { send }?.invoke(name)
    }

    /** Hears every change of the call from now on, until the returned function is called. */
    fun observe(listener: (State) -> Unit): () -> Unit {
        synchronized(lock) { listeners.add(listener) }
        return { synchronized(lock) { listeners.remove(listener) } }
    }
}
