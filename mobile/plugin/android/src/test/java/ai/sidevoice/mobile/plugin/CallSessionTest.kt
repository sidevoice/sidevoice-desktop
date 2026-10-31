package ai.sidevoice.mobile.plugin

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

class CallSessionTest {
    private val reports = mutableListOf<Pair<Long, Map<String, String>>>()

    private fun begin(): CallSession.Session {
        lateinit var session: CallSession.Session
        session = CallSession.begin { reports += session.id to it }!!
        return session
    }

    @Before
    fun noSessionIsLeftBehind() {
        CallSession.current()?.let { CallSession.end(it, "left over", report = false) }
        reports.clear()
    }

    @Test
    fun oneSessionRunsAtATime() {
        val first = begin()
        assertNull(CallSession.begin { })
        assertSame(first, CallSession.current())
        assertTrue(CallSession.end(first, "requested", report = false))
        assertNotNull(CallSession.begin { })
    }

    @Test
    fun anEndIsReportedOnce() {
        val session = begin()
        assertTrue(CallSession.end(session, "hang-up"))
        assertFalse(CallSession.end(session, "system"))
        assertEquals(listOf(session.id to mapOf("type" to "stopped", "reason" to "hang-up")), reports)
    }

    @Test
    fun anOldServiceCannotEndANewSession() {
        // The app stopped the first call and started a second at once; the system destroys the first service after.
        val old = begin()
        CallSession.end(old, "requested")
        val new = begin()
        assertNotEquals(old.id, new.id)
        assertFalse(CallSession.end(old, "system"))
        CallSession.focus(old, "lost")
        assertSame(new, CallSession.current())
        assertSame(new, CallSession.get(new.id))
        assertNull(CallSession.get(old.id))
        assertEquals(listOf(old.id to mapOf("type" to "stopped", "reason" to "requested")), reports)
        // The new one ends, and tells, as itself.
        assertTrue(CallSession.end(new, "hang-up"))
        assertEquals(new.id to mapOf("type" to "stopped", "reason" to "hang-up"), reports.last())
    }

    @Test
    fun eachSessionsHangUpHasItsOwnAddress() {
        val old = begin()
        CallSession.end(old, "requested", report = false)
        val new = begin()
        assertNotEquals(CallSession.hangUpAddress(old.id), CallSession.hangUpAddress(new.id))
        assertEquals("sidevoice-call://hang-up/${new.id}", CallSession.hangUpAddress(new.id))
    }

    @Test
    fun theFocusIsReportedOnlyWhileTheSessionRuns() {
        val session = begin()
        CallSession.focus(session, "lost-transient")
        CallSession.end(session, "requested", report = false)
        CallSession.focus(session, "gained")
        assertEquals(listOf(session.id to mapOf("type" to "focus", "state" to "lost-transient")), reports)
    }
}
