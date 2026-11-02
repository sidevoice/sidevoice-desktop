package ai.sidevoice.mobile.plugin

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MuteEchoesTest {
    @Test
    fun aMuteFromOutsideIsReported() {
        val echoes = MuteEchoes(start = false)
        assertTrue(echoes.observed(muted = true, now = true))
        assertFalse(echoes.observed(muted = true, now = true), "no change")
        assertTrue(echoes.observed(muted = false, now = false))
    }

    @Test
    fun whatItIsAtTheStartSaysNothing() {
        assertFalse(MuteEchoes(start = true).observed(muted = true, now = true))
    }

    @Test
    fun theServicesOwnMutesComingBackAreNotReportedEvenLate() {
        // The page unmuted, then muted again, before Telecom told either back.
        val echoes = MuteEchoes(start = true)
        echoes.set(false)
        echoes.set(true)
        assertFalse(echoes.observed(muted = false, now = true), "the unmute, late")
        assertFalse(echoes.observed(muted = true, now = true), "the mute")
        // Then one from outside.
        assertTrue(echoes.observed(muted = false, now = false))
    }

    @Test
    fun anEchoToldBackAsNoChangeIsStillAcknowledged() {
        // The service muted, and Telecom's first word is that it is muted: no change from what it saw, but the echo is
        // acknowledged, so a later mute from outside to the same value is not taken for it.
        val echoes = MuteEchoes(start = false)
        echoes.set(true)
        assertFalse(echoes.observed(muted = true, now = true))
        assertTrue(echoes.observed(muted = false, now = false))
        assertTrue(echoes.observed(muted = true, now = true), "from outside, after the echo was told back")
    }

    @Test
    fun aLateObservationTheMicrophoneNoLongerAgreesWithSaysNothing() {
        val echoes = MuteEchoes(start = false)
        assertFalse(echoes.observed(muted = true, now = false))
    }

    @Test
    fun oneToldBackAcknowledgesEveryOlderOne() {
        val echoes = MuteEchoes(start = false)
        echoes.set(true)
        echoes.set(false)
        echoes.set(true)
        assertFalse(echoes.observed(muted = true, now = true), "the first, which acknowledges only itself")
        assertFalse(echoes.observed(muted = false, now = true), "the second, late")
        assertFalse(echoes.observed(muted = true, now = true), "the third")
        assertTrue(echoes.observed(muted = false, now = false), "nothing pending: from outside")
    }

    private fun assertFalse(value: Boolean, message: String) = assertFalse(message, value)
    private fun assertTrue(value: Boolean, message: String) = assertTrue(message, value)
}
