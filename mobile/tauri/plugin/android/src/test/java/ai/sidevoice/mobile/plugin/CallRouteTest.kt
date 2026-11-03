package ai.sidevoice.mobile.plugin

import android.media.AudioDeviceInfo
import androidx.core.telecom.CallEndpointCompat.Companion.TYPE_BLUETOOTH
import androidx.core.telecom.CallEndpointCompat.Companion.TYPE_EARPIECE
import androidx.core.telecom.CallEndpointCompat.Companion.TYPE_SPEAKER
import androidx.core.telecom.CallEndpointCompat.Companion.TYPE_WIRED_HEADSET
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class CallRouteTest {
    @Test
    fun theEarpieceGivesWayToTheLoudspeaker() {
        assertEquals(TYPE_SPEAKER, CallRoute.preferred(TYPE_EARPIECE, listOf(TYPE_EARPIECE, TYPE_SPEAKER)))
        // The audio waits for the change: it is not settled yet.
        assertFalse(CallRoute.settled(TYPE_EARPIECE, listOf(TYPE_EARPIECE, TYPE_SPEAKER)))
        assertTrue(CallRoute.settled(TYPE_SPEAKER, listOf(TYPE_EARPIECE, TYPE_SPEAKER)))
    }

    @Test
    fun aHeadsetOrBluetoothStaysChosen() {
        for (current in listOf(TYPE_WIRED_HEADSET, TYPE_BLUETOOTH)) {
            assertNull(CallRoute.preferred(current, listOf(TYPE_EARPIECE, TYPE_SPEAKER, current)))
            assertTrue(CallRoute.settled(current, listOf(TYPE_EARPIECE, TYPE_SPEAKER, current)))
        }
    }

    @Test
    fun anEarpieceWithNoLoudspeakerIsWhereItStays() {
        assertNull(CallRoute.preferred(TYPE_EARPIECE, listOf(TYPE_EARPIECE)))
        assertTrue(CallRoute.settled(TYPE_EARPIECE, listOf(TYPE_EARPIECE)))
    }

    // The app's own call (Telecom is not there to route it): the device its audio mode takes.
    private val earpiece = AudioDeviceInfo.TYPE_BUILTIN_EARPIECE
    private val loudspeaker = AudioDeviceInfo.TYPE_BUILTIN_SPEAKER

    @Test
    fun theOwnCallDefaultsToTheLoudspeakerWithNoHeadset() {
        val available = listOf(earpiece, loudspeaker)
        assertEquals(1, CallRoute.communicationDevice(available))
        assertNull("the earpiece alone is never taken", CallRoute.communicationDevice(listOf(earpiece)))
    }

    @Test
    fun theOwnCallTakesAConnectedHeadsetOverTheLoudspeaker() {
        val bluetooth = AudioDeviceInfo.TYPE_BLUETOOTH_SCO
        val wired = AudioDeviceInfo.TYPE_WIRED_HEADSET
        val usb = AudioDeviceInfo.TYPE_USB_HEADSET
        assertEquals(2, CallRoute.communicationDevice(listOf(earpiece, loudspeaker, wired)))
        assertEquals(2, CallRoute.communicationDevice(listOf(earpiece, loudspeaker, usb)))
        assertEquals(2, CallRoute.communicationDevice(listOf(earpiece, loudspeaker, bluetooth)))
        // Bluetooth before wired, as Telecom orders them.
        assertEquals(3, CallRoute.communicationDevice(listOf(wired, earpiece, loudspeaker, bluetooth)))
    }

    @Test
    fun aPickMadeInTheCallIsNotUndoneByTheDefault() {
        // The default is taken once, as the call opens, and by the earpiece only: a headset or the loudspeaker the person
        // switched to is where the audio stays (CallRoute.preferred).
        assertNull(CallRoute.preferred(TYPE_SPEAKER, listOf(TYPE_EARPIECE, TYPE_SPEAKER)))
        assertNull(CallRoute.preferred(TYPE_WIRED_HEADSET, listOf(TYPE_EARPIECE, TYPE_SPEAKER, TYPE_WIRED_HEADSET)))
    }
}
