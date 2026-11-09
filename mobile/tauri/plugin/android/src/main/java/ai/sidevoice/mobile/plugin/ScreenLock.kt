package ai.sidevoice.mobile.plugin

/**
 * Whether the screen counts as locked (`host.screen.locked()`, the page's "only with the screen off"), from what the system
 * says: the keyguard shows (a swipe one too), or the device is not interactive. `PowerManager.isInteractive` turns false as
 * the device starts going to sleep, before the activity stops and the page is hidden, so the page's question from its own
 * `visibilitychange` already reads locked; a phone with no lock at all has no keyguard, and its screen off is still locked.
 * The app in the background with the screen on is neither. A service the system did not give says nothing (null): it does not
 * make the screen locked.
 */
internal object ScreenLock {
    fun locked(keyguardLocked: Boolean?, interactive: Boolean?): Boolean = keyguardLocked == true || interactive == false
}
