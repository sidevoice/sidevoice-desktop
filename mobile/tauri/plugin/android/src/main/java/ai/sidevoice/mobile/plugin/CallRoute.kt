package ai.sidevoice.mobile.plugin

import androidx.core.telecom.CallEndpointCompat

/**
 * Where the call's audio goes while Telecom holds the call (by Core-Telecom's endpoint types): the loudspeaker rather than
 * the earpiece, as on iOS; a headset or a Bluetooth device the system chose stays chosen.
 */
internal object CallRoute {
    /** The endpoint type the call moves to from [current], with [available] to choose from; null where it stays. */
    fun preferred(current: Int, available: List<Int>): Int? =
        CallEndpointCompat.TYPE_SPEAKER.takeIf {
            current == CallEndpointCompat.TYPE_EARPIECE && it in available
        }

    /** Whether the route is where it stays: the call's audio may open on it, no endpoint change is coming. */
    fun settled(current: Int, available: List<Int>): Boolean = preferred(current, available) == null
}
