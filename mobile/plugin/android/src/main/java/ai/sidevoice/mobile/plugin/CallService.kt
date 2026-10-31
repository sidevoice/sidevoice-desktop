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
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat

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

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val id = intent?.getLongExtra(EXTRA_SESSION, NO_SESSION) ?: NO_SESSION
        when (intent?.action) {
            ACTION_START -> begin(id)
            // Hang up, from the notification of the session this instance serves; one of an older call's means nothing.
            ACTION_HANG_UP -> session?.takeIf { it.id == id }?.let { finish("hang-up") } ?: leaveIfIdle(startId)
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
    }

    /** Ends the call's service: the focus, the notification and the service itself, and the one report of its session. */
    private fun finish(reason: String) {
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
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_call_notification)
            .setContentTitle(getString(R.string.call_notification_title))
            .setContentText(getString(R.string.call_notification_text))
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .setContentIntent(open)
            .addAction(0, getString(R.string.call_notification_hang_up), hangUp)
            .build()
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
