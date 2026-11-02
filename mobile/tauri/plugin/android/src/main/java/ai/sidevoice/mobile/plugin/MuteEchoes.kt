package ai.sidevoice.mobile.plugin

/**
 * The system's microphone mute while Telecom holds the call, as the call's service tells a mute from outside (a Bluetooth
 * or car kit, a watch) from its own coming back. The service sets the system's mute to the page's ([set]); Telecom tells
 * every change it sees ([observed]), the service's own among them, possibly late.
 *
 * Each mute the service sets waits until Telecom tells it back, oldest first; one told back acknowledges it and every
 * older one, whatever the observation before it was. What remains is a mute from outside only when it is a change and the
 * microphone still agrees: one the microphone no longer agrees with is late (a newer change came after it).
 */
internal class MuteEchoes(start: Boolean) {
    private val pending = ArrayDeque<Boolean>()
    /** The mute as last observed (the microphone's at the start). */
    private var known = start

    /** The service set the system's mute to [muted]. */
    fun set(muted: Boolean) = pending.addLast(muted)

    /** Telecom says the mute is [muted], with the microphone [now]: whether it is a mute from outside, to report. */
    fun observed(muted: Boolean, now: Boolean): Boolean {
        val own = pending.indexOf(muted)
        if (own >= 0) {
            repeat(own + 1) { pending.removeFirst() }
            known = muted
            return false
        }
        if (muted == known) return false
        known = muted
        return muted == now
    }
}
