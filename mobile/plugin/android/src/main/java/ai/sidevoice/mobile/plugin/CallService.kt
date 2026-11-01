package ai.sidevoice.mobile.plugin

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.Uri
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import ai.sidevoice.app.MediaControls
import android.support.v4.media.session.MediaSessionCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.media.app.NotificationCompat.MediaStyle

/**
 * The foreground service that keeps the process of a call alive while the screen is off or another app is in front:
 * microphone (the call listens) and media playback (the agent speaks). The app starts it while it is visible, which
 * Android requires of a microphone service, and ends it when the call ends ([CallSession] says which way it ended).
 *
 * Each instance serves the [CallSession.Session] it was started for, named in its intent, and only acts on that one: the
 * system destroys a service after the app may already have started the next call, and the end of the old one must not
 * end the new one.
 *
 * It holds the call's audio focus for as long as it runs and says when it changes; what to do about that is the call's
 * decision. It is never restarted by the system: a call does not survive its process.
 */
class CallService : Service() {
    private var session: CallSession.Session? = null
    private var focusRequest: AudioFocusRequest? = null
    /** The call's media session, this service's: it exists while the call runs, and goes with the service. */
    private var media: MediaSession? = null
    /** Stops following the call's state ([MediaControls.observe]). */
    private var unobserve: (() -> Unit)? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val id = intent?.getLongExtra(EXTRA_SESSION, NO_SESSION) ?: NO_SESSION
        when (intent?.action) {
            ACTION_START -> begin(id)
            // Hang up, from the notification of the session this instance serves; one of an older call's means nothing.
            ACTION_HANG_UP -> session?.takeIf { it.id == id }?.let { finish("hang-up") } ?: leaveIfIdle(startId)
            // Mute or unmute, from the notification's button (Android 12 and older), of this session alone.
            ACTION_TOGGLE_MUTE -> session?.takeIf { it.id == id }?.let { toggleMute() } ?: leaveIfIdle(startId)
            // Not asked by this app.
            else -> leaveIfIdle(startId)
        }
        return START_NOT_STICKY
    }

    /** The person removed the app from the recents list: the microphone does not outlive that. */
    override fun onTaskRemoved(rootIntent: Intent?) {
        finish("task-removed")
        super.onTaskRemoved(rootIntent)
    }

    /** Whoever ended it, the focus and the report are not left behind (the system may end a service on its own). */
    override fun onDestroy() {
        closeMedia()
        abandonFocus()
        session?.let { CallSession.end(it, "system") }
        session = null
        super.onDestroy()
    }

    private fun begin(id: Long) {
        // Within seconds of a startForegroundService, even when the call already ended: Android crashes the app otherwise.
        try {
            ServiceCompat.startForeground(this, NOTIFICATION_ID, notification(id), FOREGROUND_SERVICE_TYPES)
        } catch (error: RuntimeException) {
            // The system refused (a microphone service cannot start from the background, or without the microphone
            // allowed): nothing runs, and the app is told the service is gone.
            CallSession.get(id)?.let { CallSession.end(it, "system") }
            leave()
            return
        }
        val mine = CallSession.get(id)
        if (mine == null) {
            // Ended, or replaced, before this got to run: it is nobody's service.
            leave()
            return
        }
        abandonFocus()
        session = mine
        requestFocus(mine)
        openMedia(mine)
    }

    /** Ends the call's service: the focus, the notification and the service itself, and the one report of its session. */
    private fun finish(reason: String) {
        closeMedia()
        abandonFocus()
        session?.let { CallSession.end(it, reason) }
        session = null
        leave()
    }

    private fun leave() {
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    /** An intent that is not for the session this instance serves: when it serves none, there is nothing to keep. */
    private fun leaveIfIdle(startId: Int) {
        if (session == null) stopSelf(startId)
    }

    private fun notification(id: Long): Notification {
        val manager = getSystemService(NotificationManager::class.java)
        // Created each time, so a change of the system's language renames it.
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_ID, getString(R.string.call_channel_name), NotificationManager.IMPORTANCE_LOW)
        )
        val open = packageManager.getLaunchIntentForPackage(packageName)?.let {
            PendingIntent.getActivity(this, 0, it, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        }
        val hangUp = PendingIntent.getService(
            this,
            0,
            // The session is in the data too: PendingIntents are told apart by action, component and data, not by extras,
            // so one with only the extra would be this session's Hang up overwritten by the next session's.
            Intent(this, CallService::class.java)
                .setAction(ACTION_HANG_UP)
                .setData(Uri.parse(CallSession.hangUpAddress(id)))
                .putExtra(EXTRA_SESSION, id),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )
        val builder = NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_call_notification)
            .setContentTitle(getString(R.string.call_notification_title))
            .setContentText(getString(R.string.call_notification_text))
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .setContentIntent(open)
        val token = media?.sessionToken
        if (token == null || Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            // Android 13 and later draw a media notification's buttons from its session (the play/pause and the Hang up
            // custom action of `applyState`), and a notification without a session keeps its own Hang up.
            builder.addAction(R.drawable.ic_call_hang_up, getString(R.string.call_notification_hang_up), hangUp)
        } else {
            // Android 12 and older draw the notification's own buttons: mute or unmute, as the call is, and Hang up.
            val playing = MediaControls.state().playing
            val toggle = PendingIntent.getService(
                this,
                0,
                Intent(this, CallService::class.java)
                    .setAction(ACTION_TOGGLE_MUTE)
                    .setData(Uri.parse(CallSession.toggleMuteAddress(id)))
                    .putExtra(EXTRA_SESSION, id),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
            )
            builder
                .addAction(
                    if (playing) R.drawable.ic_call_mute else R.drawable.ic_call_unmute,
                    getString(if (playing) R.string.call_notification_mute else R.string.call_notification_unmute),
                    toggle
                )
                .addAction(R.drawable.ic_call_hang_up, getString(R.string.call_notification_hang_up), hangUp)
        }
        // The call's media session: the notification shows it, which puts the call's controls on the lock screen.
        token?.let {
            val style = MediaStyle().setMediaSession(MediaSessionCompat.Token.fromToken(it))
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) style.setShowActionsInCompactView(0, 1)
            builder.setStyle(style)
        }
        return builder.build()
    }

    /**
     * The call's media session, made for [mine] and only while it runs: Android hands it a headset's play/pause (a press
     * arrives as pause while the call plays, as play while it is muted), which go to the shared plugin
     * ([MediaControls.command]) and from there to the call; the lock screen's controls are its own and its Hang up custom
     * action (Android 13 and later), which ends this session's service as the notification's Hang up does.
     */
    private fun openMedia(mine: CallSession.Session) {
        closeMedia()
        val created = MediaSession(this, "sidevoice-call")
        created.setCallback(object : MediaSession.Callback() {
            override fun onPlay() = MediaControls.command("play")
            override fun onPause() = MediaControls.command("pause")
            override fun onCustomAction(action: String, extras: Bundle?) {
                if (action == CUSTOM_HANG_UP && session?.id == mine.id) finish("hang-up")
            }
        }, Handler(Looper.getMainLooper()))
        media = created
        applyState(created, MediaControls.state())
        created.isActive = true
        val main = Handler(Looper.getMainLooper())
        unobserve = MediaControls.observe { state ->
            main.post {
                val current = media ?: return@post
                if (session?.id != mine.id) return@post
                applyState(current, state)
                // The notification's mute button follows the call (Android 12 and older draw it themselves).
                getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification(mine.id))
            }
        }
        // Again, now with the session: the notification shows it.
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification(mine.id))
    }

    private fun closeMedia() {
        unobserve?.invoke()
        unobserve = null
        media?.let {
            it.isActive = false
            it.release()
        }
        media = null
    }

    /** The call, as the session says it: playing while the microphone is open, paused while muted; its title. */
    private fun applyState(media: MediaSession, state: MediaControls.State) {
        val metadata = MediaMetadata.Builder().putString(MediaMetadata.METADATA_KEY_TITLE, getString(R.string.call_notification_title))
        if (state.title.isNotEmpty()) metadata.putString(MediaMetadata.METADATA_KEY_ARTIST, state.title)
        media.setMetadata(metadata.build())
        media.setPlaybackState(
            PlaybackState.Builder()
                .setActions(PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE or PlaybackState.ACTION_PLAY_PAUSE)
                .addCustomAction(CUSTOM_HANG_UP, getString(R.string.call_notification_hang_up), R.drawable.ic_call_hang_up)
                .setState(
                    if (state.playing) PlaybackState.STATE_PLAYING else PlaybackState.STATE_PAUSED,
                    PlaybackState.PLAYBACK_POSITION_UNKNOWN,
                    1f
                )
                .build()
        )
    }

    /** The notification's mute button: the same as a headset's press. */
    private fun toggleMute() {
        MediaControls.command(if (MediaControls.state().playing) "pause" else "play")
    }

    /**
     * A call takes the audio focus for its length (other media pauses and comes back after), as the voice it is, and says
     * when it changes hands. Refused (a phone call is on), it is as if it had been taken at once.
     */
    private fun requestFocus(mine: CallSession.Session) {
        val audio = getSystemService(AudioManager::class.java)
        val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build()
            )
            .setOnAudioFocusChangeListener({ change -> CallSession.focus(mine, focusState(change)) }, Handler(Looper.getMainLooper()))
            .build()
        focusRequest = request
        if (audio.requestAudioFocus(request) != AudioManager.AUDIOFOCUS_REQUEST_GRANTED) {
            CallSession.focus(mine, "lost-transient")
        }
    }

    private fun abandonFocus() {
        focusRequest?.let { getSystemService(AudioManager::class.java).abandonAudioFocusRequest(it) }
        focusRequest = null
    }

    private fun focusState(change: Int): String = when (change) {
        AudioManager.AUDIOFOCUS_GAIN -> "gained"
        AudioManager.AUDIOFOCUS_LOSS -> "lost"
        AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> "lost-transient"
        AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> "duck"
        else -> "lost-transient"
    }

    companion object {
        const val ACTION_START = "ai.sidevoice.mobile.plugin.CALL_START"
        const val ACTION_HANG_UP = "ai.sidevoice.mobile.plugin.CALL_HANG_UP"
        const val ACTION_TOGGLE_MUTE = "ai.sidevoice.mobile.plugin.CALL_TOGGLE_MUTE"
        /** The session's custom action that ends the call (the media controls of Android 13 and later). */
        private const val CUSTOM_HANG_UP = "ai.sidevoice.mobile.plugin.HANG_UP"
        /** The session (a number) an intent is for. */
        const val EXTRA_SESSION = "ai.sidevoice.mobile.plugin.SESSION"
        private const val NO_SESSION = -1L
        private const val CHANNEL_ID = "call"
        private const val NOTIFICATION_ID = 1
        // What the manifest declares for the service, both: the call listens and the agent speaks.
        private const val FOREGROUND_SERVICE_TYPES =
            ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE or ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK
    }
}
