package ai.sidevoice.mobile.plugin

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
}
