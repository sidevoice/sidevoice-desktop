package ai.sidevoice.app

import android.content.Context
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Handler
import android.os.Looper

/**
 * The call's media session, one for the process. While there is a call it is active and says the call: playing while the
 * microphone is open, paused while it is muted. Android hands the active session a headset's play/pause (a single press
 * of a wired or Bluetooth headset arrives as pause while playing and as play while paused) and the controls of the
 * notification that shows it; each arrives here as `play` or `pause`, and goes to Rust, which decides what it does to the
 * call (platform/media_controls/android.rs, the same rule as on macOS and iOS).
 *
 * The call's foreground service shows the session in its notification ([token]), which is what puts the call's
 * controls on the lock screen.
 */
object MediaControls {
    private val lock = Any()
    private var session: MediaSession? = null

    /** The session's token, once it is made: the call's service gives it to its notification. */
    fun token(): MediaSession.Token? = synchronized(lock) { session?.sessionToken }

    /** Makes the session, once, with where its commands go. Inactive until there is a call. */
    fun attach(context: Context, send: (String) -> Unit) = synchronized(lock) {
        if (session != null) return@synchronized
        session = MediaSession(context, "sidevoice-call").apply {
            setCallback(object : MediaSession.Callback() {
                override fun onPlay() = send("play")
                override fun onPause() = send("pause")
            }, Handler(Looper.getMainLooper()))
        }
    }

    /** Follows the call: [active] while there is one, [playing] while its microphone is open, [title] its conversation's. */
    fun update(active: Boolean, playing: Boolean, title: String) = synchronized(lock) {
        val session = session ?: return@synchronized
        if (!active) {
            session.setPlaybackState(state(PlaybackState.STATE_STOPPED))
            session.isActive = false
            return@synchronized
        }
        val metadata = MediaMetadata.Builder().putString(MediaMetadata.METADATA_KEY_TITLE, "Sidevoice")
        if (title.isNotEmpty()) metadata.putString(MediaMetadata.METADATA_KEY_ARTIST, title)
        session.setMetadata(metadata.build())
        session.setPlaybackState(state(if (playing) PlaybackState.STATE_PLAYING else PlaybackState.STATE_PAUSED))
        session.isActive = true
    }

    private fun state(state: Int): PlaybackState = PlaybackState.Builder()
        .setActions(PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE or PlaybackState.ACTION_PLAY_PAUSE)
        .setState(state, PlaybackState.PLAYBACK_POSITION_UNKNOWN, 1f)
        .build()
}
