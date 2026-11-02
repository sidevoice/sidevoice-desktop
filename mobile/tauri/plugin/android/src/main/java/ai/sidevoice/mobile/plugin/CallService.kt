package ai.sidevoice.mobile.plugin

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.telecom.DisconnectCause
import android.util.Log
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
import androidx.core.app.Person
import androidx.core.app.ServiceCompat
import androidx.core.telecom.CallAttributesCompat
import androidx.core.telecom.CallControlResult
import androidx.core.telecom.CallControlScope
import androidx.core.telecom.CallEndpointCompat
import androidx.core.telecom.CallsManager
import androidx.media.app.NotificationCompat.MediaStyle
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The foreground service that keeps the process of a call alive while the screen is off or another app is in front:
 * microphone (the call listens) and media playback (the agent speaks). The app starts it while it is visible, which
 * Android requires of a microphone service, and ends it when the call ends ([CallSession] says which way it ended).
 *
 * Each instance serves the [CallSession.Session] it was started for, named in its intent, and only acts on that one: the
 * system destroys a service after the app may already have started the next call, and the end of the old one must not
 * end the new one.
 *
 * It holds the call's audio focus and says when it changes; what to do about that is the call's decision. It is never
 * restarted by the system: a call does not survive its process.
 *
 * **The system's call.** For its length the call is also a self-managed call of the system's Telecom (Core-Telecom: an
 * outgoing audio call, with the conversation as its name), so a Bluetooth or car kit, a watch and the system's call
 * notification show it as the phone call it is. Telecom holds the call's audio from then (the mode, the focus, the route):
 * the service gives its own focus request up, and Telecom's hold (another call taking this one's place) is reported
 * instead (`held`); a hang-up from outside the app ends the service (`hang-up`), and a mute from outside is reported
 * (`muted`), while the page's mute is given to the system's microphone mute. The loudspeaker is chosen over the earpiece,
 * as on iOS. The notification is a call's (CallStyle) while Telecom holds the call. Telecom refusing the call leaves it the
 * app's own, as before: the service's focus and media notification.
 */
class CallService : Service() {
    private var session: CallSession.Session? = null
    private var focusRequest: AudioFocusRequest? = null
    /** The call's media session, this service's: it exists while the call runs, and goes with the service. */
    private var media: MediaSession? = null
    /** Stops following the call's state ([MediaControls.observe]). */
    private var unobserve: (() -> Unit)? = null
    /** The conversation the call is on (empty for none). */
    private var title = ""
    /** Telecom's coroutines: the call's, and their ends. Never cancelled as a whole: a call is disconnected before its own go. */
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    /** The coroutine that adds the call to Telecom and runs for its length. */
    private var telecom: Job? = null
    /** The call as Telecom gave it (added), until it is disconnected. */
    private var registered: CallControlScope? = null
    /** The call, once Telecom holds it (active). */
    private var control: CallControlScope? = null
    /** This service's call is over: a call Telecom gives after that is disconnected at once, so none is left behind. */
    private var telecomEnded = false
    /**
     * The microphone mutes the service set itself and Telecom has not told back yet, oldest first: told back, they are no
     * mute from outside.
     */
    private val echoes = ArrayDeque<Boolean>()
    /** The page's mute, as last told ([ACTION_UPDATE]). */
    private var pageMuted: Boolean? = null
    /** Whether the service turned the system's microphone mute on: it is turned off again at the end. */
    private var mutedTheSystem = false
    /** Whether the audio focus was last reported lost (or refused): given back as gained when Telecom takes the call's audio. */
    private var focusLost = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val id = intent?.getLongExtra(EXTRA_SESSION, NO_SESSION) ?: NO_SESSION
        when (intent?.action) {
            ACTION_START -> begin(id, intent.getStringExtra(EXTRA_TITLE) ?: "")
            // What the call shows now (its title) and the page's mute, for the session this instance serves.
            ACTION_UPDATE -> session?.takeIf { it.id == id }?.let {
                update(it, intent.getStringExtra(EXTRA_TITLE) ?: "", intent.getBooleanExtra(EXTRA_MUTED, false))
            } ?: leaveIfIdle(startId)
            // Hang up, from the notification of the session this instance serves; one of an older call's means nothing.
            ACTION_HANG_UP -> session?.takeIf { it.id == id }?.let { finish("hang-up") } ?: leaveIfIdle(startId)
            // Ask for the audio focus again (it was lost for good, or refused), for the session this instance serves.
            ACTION_REFOCUS -> session?.takeIf { it.id == id }?.let { refocus(it) } ?: leaveIfIdle(startId)
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
        endTelecom()
        closeMedia()
        abandonFocus()
        session?.let { CallSession.end(it, "system") }
        session = null
        super.onDestroy()
    }

    private fun begin(id: Long, title: String) {
        this.title = title
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
        telecomEnded = false
        addTelecomCall(mine)
    }

    /** Ends the call's service: the focus, the notification and the service itself, and the one report of its session. */
    private fun finish(reason: String) {
        endTelecom()
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
        if (control != null) {
            // Telecom holds the call: the notification is a call's (the system's call chip and its Hang up), with the mute
            // button beside it. The media session goes on taking a headset's buttons, without a card of its own.
            val person = Person.Builder().setName(displayName()).setImportant(true).build()
            return builder
                .setStyle(NotificationCompat.CallStyle.forOngoingCall(person, hangUp))
                .addAction(muteAction(id))
                .build()
        }
        if (token == null || Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            // Android 13 and later draw a media notification's buttons from its session (the play/pause and the Hang up
            // custom action of `applyState`), and a notification without a session keeps its own Hang up.
            builder.addAction(R.drawable.ic_call_hang_up, getString(R.string.call_notification_hang_up), hangUp)
        } else {
            // Android 12 and older draw the notification's own buttons: mute or unmute, as the call is, and Hang up.
            builder
                .addAction(muteAction(id))
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

    /** The notification's mute or unmute button, as the call is now. */
    private fun muteAction(id: Long): NotificationCompat.Action {
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
        return NotificationCompat.Action(
            if (playing) R.drawable.ic_call_mute else R.drawable.ic_call_unmute,
            getString(if (playing) R.string.call_notification_mute else R.string.call_notification_unmute),
            toggle
        )
    }

    /** What the system shows as the call's other party: its conversation, or the app's name. */
    private fun displayName(): String = title.trim().ifEmpty { getString(R.string.call_notification_title) }

    /** What the call shows now and the page's mute ([ACTION_UPDATE]). */
    private fun update(mine: CallSession.Session, title: String, muted: Boolean) {
        val renamed = title != this.title
        this.title = title
        pageMuted = muted
        applyMute()
        if (renamed && control != null) {
            getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification(mine.id))
        }
    }

    /**
     * The page's mute given to the system's microphone mute while Telecom holds the call, so that the system's call (a car
     * kit, a watch) shows it as the page does, and a mute the system made is lifted when the person unmutes in the app.
     */
    private fun applyMute() {
        val muted = pageMuted ?: return
        if (control == null) return
        val audio = getSystemService(AudioManager::class.java)
        if (audio.isMicrophoneMute != muted) {
            echoes.addLast(muted)
            audio.isMicrophoneMute = muted
            mutedTheSystem = muted
        }
    }

    /**
     * Adds the call to the system's Telecom as a self-managed outgoing audio call, for [mine], and runs it for its length.
     * Refused (Telecom will not take a call now, an emergency call is on), the call stays the app's own.
     */
    private fun addTelecomCall(mine: CallSession.Session) {
        val calls = CallsManager(this)
        try {
            calls.registerAppWithTelecom(CallsManager.CAPABILITY_BASELINE)
        } catch (error: RuntimeException) {
            Log.w(TAG, "Telecom: the app was not registered; the call stays the app's own", error)
            return
        }
        val attributes = CallAttributesCompat(
            displayName = displayName(),
            address = Uri.parse(CallSession.callAddress(mine.id)),
            direction = CallAttributesCompat.DIRECTION_OUTGOING,
            callType = CallAttributesCompat.CALL_TYPE_AUDIO_CALL,
            // Another call (a phone call) may hold this one, as it holds any call: the app pauses it meanwhile.
            callCapabilities = CallAttributesCompat.SUPPORTS_SET_INACTIVE,
        )
        telecom = scope.launch {
            var added: CallControlScope? = null
            try {
                calls.addCall(
                    attributes,
                    onAnswer = { },
                    // Hung up from outside the app: its notification, a Bluetooth or car kit, a watch.
                    onDisconnect = { withContext(Dispatchers.Main) { if (session?.id == mine.id) finish("hang-up") } },
                    onSetActive = { CallSession.held(mine, false) },
                    onSetInactive = { CallSession.held(mine, true) },
                ) {
                    val call = this
                    added = call
                    if (telecomEnded || session?.id != mine.id) {
                        // The call ended before Telecom gave it: disconnected now, so that none is left behind.
                        launch {
                            call.disconnect(DisconnectCause(DisconnectCause.LOCAL))
                            call.cancel()
                        }
                        return@addCall
                    }
                    // Kept from now: an end while it is being set active disconnects it (`endTelecom`).
                    registered = call
                    launch {
                        val result = call.setActive()
                        if (registered !== call) return@launch // ended meanwhile, and disconnected
                        if (result !is CallControlResult.Success) {
                            Log.w(TAG, "Telecom: the call was not set active: $result; it stays the app's own")
                            registered = null
                            call.disconnect(DisconnectCause(DisconnectCause.ERROR))
                            // Its collectors (mute, route) do not end with the disconnect: they go now, and the call's
                            // coroutine with them.
                            call.cancel()
                        } else {
                            telecomHolds(mine, call)
                        }
                    }
                    // A mute from outside the app: a change of the system's microphone mute (Telecom's mute is that). What it
                    // already is says nothing (Telecom may tell it at the start); the page's own mute coming back is the same
                    // as the page, and changes nothing there.
                    launch {
                        val audio = getSystemService(AudioManager::class.java)
                        var known = audio.isMicrophoneMute
                        call.isMuted.collect { muted ->
                            if (muted == known) return@collect
                            known = muted
                            // The service's own mutes (the page's) coming back, up to this one: acknowledged, not reported.
                            val own = echoes.indexOf(muted)
                            if (own >= 0) {
                                repeat(own + 1) { echoes.removeFirst() }
                                return@collect
                            }
                            // Late: the microphone is no longer so (a newer change came after it), so it says nothing now.
                            if (audio.isMicrophoneMute != muted) return@collect
                            if (control === call) CallSession.muted(mine, muted)
                        }
                    }
                    // The loudspeaker rather than the earpiece, as on iOS: a headset or a Bluetooth device stays chosen.
                    launch {
                        combine(call.currentCallEndpoint, call.availableEndpoints) { current, available -> current to available }
                            .collect { (current, available) ->
                                if (current.type == CallEndpointCompat.TYPE_EARPIECE) {
                                    available.firstOrNull { it.type == CallEndpointCompat.TYPE_SPEAKER }
                                        ?.let { call.requestEndpointChange(it) }
                                }
                            }
                    }
                }
                // The call is over in Telecom. Not by the app (it lets go of `registered` first): the service ends with it.
                if (added != null && registered === added && session?.id == mine.id) finish("system")
            } catch (error: CancellationException) {
                throw error
            } catch (error: Exception) {
                Log.w(TAG, "Telecom refused the call; it stays the app's own", error)
            }
        }
    }

    /**
     * Telecom holds [call] (active): its audio is Telecom's from now, so the service's own focus request is given up (a
     * loss reported meanwhile is given back as a gain), the page's mute is given to the system's, and the notification
     * becomes a call's.
     */
    private fun telecomHolds(mine: CallSession.Session, call: CallControlScope) {
        control = call
        abandonFocus()
        if (focusLost) {
            focusLost = false
            CallSession.focus(mine, "gained")
        }
        applyMute()
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification(mine.id))
    }

    /**
     * The call leaves Telecom: disconnected by the app, and its coroutines let go once it is. One Telecom has not given yet
     * is disconnected when it arrives (`telecomEnded`); its coroutine ends with it.
     */
    private fun endTelecom() {
        telecomEnded = true
        val call = registered
        registered = null
        control = null
        echoes.clear()
        val job = telecom
        telecom = null
        if (mutedTheSystem) {
            getSystemService(AudioManager::class.java).isMicrophoneMute = false
            mutedTheSystem = false
        }
        if (call == null) return
        scope.launch {
            try {
                call.disconnect(DisconnectCause(DisconnectCause.LOCAL))
            } catch (error: Exception) {
                Log.w(TAG, "Telecom: the call was not disconnected", error)
            } finally {
                job?.cancel()
            }
        }
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
     * when it changes hands; the app pauses the call while it is lost and resumes it when it is gained (D-AUDIO-FOCUS).
     * Asked while a phone call is on, the system delays the grant (the app is told it is lost now, and gained when the call
     * ends); a refusal is reported as such (`refused`): there is no end to wait for, so the call stays paused until the person unmutes.
     */
    private fun requestFocus(mine: CallSession.Session, reportGranted: Boolean = false) {
        val audio = getSystemService(AudioManager::class.java)
        val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build()
            )
            .setAcceptsDelayedFocusGain(true)
            .setOnAudioFocusChangeListener({ change -> reportFocus(mine, focusState(change)) }, Handler(Looper.getMainLooper()))
            .build()
        focusRequest = request
        when (audio.requestAudioFocus(request)) {
            // Asked again by the person: the listener only hears of changes, so a grant is reported as the gain it is.
            AudioManager.AUDIOFOCUS_REQUEST_GRANTED -> if (reportGranted) reportFocus(mine, "gained")
            // A phone call is on: the grant comes when it ends, as a gain.
            AudioManager.AUDIOFOCUS_REQUEST_DELAYED -> reportFocus(mine, "lost-transient")
            // Nothing will be given later.
            else -> reportFocus(mine, "refused")
        }
    }

    private fun reportFocus(mine: CallSession.Session, state: String) {
        focusLost = state != "gained" && state != "duck"
        CallSession.focus(mine, state)
    }

    /**
     * The person asks to go on after a focus that was lost for good or refused: asked again, the answer reported. While
     * Telecom holds the call's audio there is nothing to ask for: it is the call's.
     */
    private fun refocus(mine: CallSession.Session) {
        if (control != null) {
            reportFocus(mine, "gained")
            return
        }
        abandonFocus()
        requestFocus(mine, reportGranted = true)
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
        const val ACTION_REFOCUS = "ai.sidevoice.mobile.plugin.CALL_REFOCUS"
        const val ACTION_TOGGLE_MUTE = "ai.sidevoice.mobile.plugin.CALL_TOGGLE_MUTE"
        const val ACTION_UPDATE = "ai.sidevoice.mobile.plugin.CALL_UPDATE"
        /** The session's custom action that ends the call (the media controls of Android 13 and later). */
        private const val CUSTOM_HANG_UP = "ai.sidevoice.mobile.plugin.HANG_UP"
        /** The session (a number) an intent is for. */
        const val EXTRA_SESSION = "ai.sidevoice.mobile.plugin.SESSION"
        /** The conversation the call is on. */
        const val EXTRA_TITLE = "ai.sidevoice.mobile.plugin.TITLE"
        /** Whether the page has the call muted. */
        const val EXTRA_MUTED = "ai.sidevoice.mobile.plugin.MUTED"
        private const val TAG = "SidevoiceCall"
        private const val NO_SESSION = -1L
        private const val CHANNEL_ID = "call"
        private const val NOTIFICATION_ID = 1
        // What the manifest declares for the service: the call listens, the agent speaks, and it is a phone call (Telecom's,
        // which the manifest's MANAGE_OWN_CALLS allows).
        private const val FOREGROUND_SERVICE_TYPES =
            ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE or ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK or
                ServiceInfo.FOREGROUND_SERVICE_TYPE_PHONE_CALL
    }
}
