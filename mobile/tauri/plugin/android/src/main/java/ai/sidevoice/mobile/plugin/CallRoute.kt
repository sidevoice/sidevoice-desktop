package ai.sidevoice.mobile.plugin

import android.media.AudioDeviceInfo
import androidx.core.telecom.CallEndpointCompat

/**
 * Where the call's audio goes while Telecom holds the call (by Core-Telecom's endpoint types): the loudspeaker rather than
 * the earpiece, as on iOS; a headset or a Bluetooth device the system chose stays chosen.
 */
internal object CallRoute {
    /** The headsets a call is routed to, as Telecom does: Bluetooth (HFP, LE Audio) and hearing aids (`AudioDeviceInfo` types). */
    val HEADSET = setOf(
        AudioDeviceInfo.TYPE_BLUETOOTH_SCO,
        AudioDeviceInfo.TYPE_BLE_HEADSET,
        AudioDeviceInfo.TYPE_HEARING_AID,
    )

    /** The headphones the call moves to: wired, USB, Bluetooth (HFP, LE Audio) and hearing aids (`AudioDeviceInfo` types). */
    val HEADPHONES = HEADSET + setOf(
        AudioDeviceInfo.TYPE_WIRED_HEADSET,
        AudioDeviceInfo.TYPE_WIRED_HEADPHONES,
        AudioDeviceInfo.TYPE_USB_HEADSET,
    )

    /**
     * The device the app's own call takes from [available] (`AudioDeviceInfo` types), where Telecom is not there to route it:
     * a Bluetooth headset or a hearing aid, else wired or USB headphones, else the loudspeaker; never the earpiece, which the
     * communication mode would choose by itself. Null where none of them is offered: the system's route stays.
     * Returns the index in [available], since the caller holds the devices.
     */
    fun communicationDevice(available: List<Int>): Int? =
        available.indexOfFirst { it in HEADSET }.takeIf { it >= 0 }
            ?: available.indexOfFirst { it in HEADPHONES }.takeIf { it >= 0 }
            ?: available.indexOfFirst { it == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }.takeIf { it >= 0 }

    /** The endpoint type the call moves to from [current], with [available] to choose from; null where it stays. */
    fun preferred(current: Int, available: List<Int>): Int? =
        CallEndpointCompat.TYPE_SPEAKER.takeIf {
            current == CallEndpointCompat.TYPE_EARPIECE && it in available
        }

    /** Whether the route is where it stays: the call's audio may open on it, no endpoint change is coming. */
    fun settled(current: Int, available: List<Int>): Boolean = preferred(current, available) == null
}
