package ai.sidevoice.mobile.plugin

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ScreenLockTest {
    @Test
    fun theAppBehindWithTheScreenOnIsNotLocked() {
        assertFalse(ScreenLock.locked(keyguardLocked = false, interactive = true))
    }

    @Test
    fun theScreenOffIsLockedTheMomentItGoesWithOrWithoutAKeyguard() {
        // Going to sleep: not interactive any more, the keyguard not up yet (or never, with no lock at all).
        assertTrue(ScreenLock.locked(keyguardLocked = false, interactive = false))
        assertTrue(ScreenLock.locked(keyguardLocked = true, interactive = false))
    }

    @Test
    fun theLockScreenShownIsLocked() {
        // Woken on the lock screen (a swipe one too), not unlocked yet.
        assertTrue(ScreenLock.locked(keyguardLocked = true, interactive = true))
    }

    @Test
    fun aServiceTheSystemDidNotGiveSaysNothing() {
        assertFalse(ScreenLock.locked(keyguardLocked = null, interactive = null))
        assertFalse(ScreenLock.locked(keyguardLocked = null, interactive = true))
        assertTrue(ScreenLock.locked(keyguardLocked = null, interactive = false))
    }
}
