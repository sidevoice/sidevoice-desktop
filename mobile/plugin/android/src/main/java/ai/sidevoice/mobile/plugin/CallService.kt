package ai.sidevoice.mobile.plugin

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
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
 * It holds the call's audio focus for as long as it runs and says when it changes; what to do about that is the call's
 * decision. It is never restarted by the system: a call does not survive its process.
 */
class CallService : Service() {
    private var focusRequest: AudioFocusRequest? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> begin()
            ACTION_HANG_UP -> finish("hang-up")
            // Not asked by this app: a service that is not a call's has nothing to keep alive.
            else -> stopSelf(startId)
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
        CallSession.end("system")
        super.onDestroy()
    }

    private fun begin() {
        // Within seconds of a startForegroundService, even when the call already ended: Android crashes the app otherwise.
        try {
            ServiceCompat.startForeground(this, NOTIFICATION_ID, notification(), FOREGROUND_SERVICE_TYPES)
        } catch (error: RuntimeException) {
            // The system refused (a microphone service cannot start from the background, or without the microphone
            // allowed): nothing runs, and the app is told the service is gone.
            finish("system")
            return
        }
        if (!CallSession.running) {
            finish("requested")
            return
        }
        requestFocus()
    }

    /** Ends the call's service: the focus, the notification and the service itself, and one report. */
    private fun finish(reason: String) {
        abandonFocus()
        CallSession.end(reason)
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private fun notification(): Notification {
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
            Intent(this, CallService::class.java).setAction(ACTION_HANG_UP),
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
    private fun requestFocus() {
        val audio = getSystemService(AudioManager::class.java)
        val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build()
            )
            .setOnAudioFocusChangeListener({ change -> CallSession.focus(focusState(change)) }, Handler(Looper.getMainLooper()))
            .build()
        focusRequest = request
        if (audio.requestAudioFocus(request) != AudioManager.AUDIOFOCUS_REQUEST_GRANTED) {
            CallSession.focus("lost-transient")
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
        private const val CHANNEL_ID = "call"
        private const val NOTIFICATION_ID = 1
        // What the manifest declares for the service, both: the call listens and the agent speaks.
        private const val FOREGROUND_SERVICE_TYPES =
            ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE or ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK
    }
}
