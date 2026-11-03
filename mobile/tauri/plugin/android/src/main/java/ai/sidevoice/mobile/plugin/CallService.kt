package ai.sidevoice.mobile.plugin

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.res.Configuration
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.telecom.DisconnectCause
import android.util.Log
import android.content.pm.ServiceInfo
import android.net.Uri
import android.media.AudioAttributes
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
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
import android.os.SystemClock
import ai.sidevoice.app.MediaControls
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.telecom.CallAttributesCompat
import androidx.core.telecom.CallControlResult
import androidx.core.telecom.CallControlScope
import androidx.core.telecom.CallsManager
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

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
 * as on iOS. Telecom refusing the call leaves it the app's own, as before: the service's focus.
 *
 * **The notification is Sidevoice's own card** ([CallCardViews], HANDOVER §4 "Android"), whoever holds the call: the
 * conversation, its creature and state, mute, skip, the other conversations (they open the app's conversation list) and
 * hang up, drawn from the call surface the app sends with every update ([CallCard]). Its buttons come back here as
 * PendingIntents to this service, so they work without unlocking: hang up ends the call as before, mute and skip are
 * reported to the app as the person's actions, and the audio switch moves the call between the headphones and the
 * loudspeaker itself (`setCommunicationDevice`). The media session stays, without a card of its own: a headset's button
 * still mutes and unmutes the call.
 *
 * **The call's audio waits for Telecom.** Setting the call active switches the audio mode to communication and the route
 * to the loudspeaker, and AAudio cuts off any stream open across that change. So the app opens the call's microphone and
 * speaker only once the service reports `audio` (as iOS waits for CallKit's activation): when Telecom has made the call
 * active and its route has settled, or at once when the call stays the app's own (Telecom refused it), or past
 * [AUDIO_READY_LIMIT_MS] whatever Telecom is doing. Logged (`SidevoiceCall`) with which of them it was.
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
    /** What the notification draws ([ACTION_UPDATE]). */
    private var card = CallCard()
    /**
     * The stills the card draws, decoded once per version of their file: by name, and the decoded file's time and size. A
     * still keeps its name for a conversation (Rust `CallSurface`: `<thread>#<px>`) while the app writes it again with a new
     * creature, so the name alone would keep drawing the old one.
     */
    private val stillImages = HashMap<String, Pair<StillVersion, Bitmap>>()
    /** When the card was last drawn ([show]), on the uptime clock; and the drawing waiting for its turn, if one is. */
    private var lastShown = 0L
    private var pendingShow: Runnable? = null
    /** Whether the audio switch chose the call's device ([toggleRoute]): given back at the end. */
    private var routeChosen = false
    /** Redraws the card when a headset comes or goes, or the call's device changes ([watchRoute]). */
    private var routeWatch: (() -> Unit)? = null
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
    /** The own call's route (`routeCommunication`): whether the mode is taken, the mode before it, and what was set. */
    private var communicating = false
    private var modeBefore = AudioManager.MODE_NORMAL
    private var routedDevice = false
    private var routedSco = false
    /** Before Android 12: whether the own call turned the loudspeaker on, off the earpiece ([keepOffEarpiece]). */
    private var routedSpeakerphone = false
    /** The system's microphone mute while Telecom holds the call: the service's own mutes told from one from outside. */
    private var echoes: MuteEchoes? = null
    /** The page's mute, as last told ([ACTION_UPDATE]). */
    private var pageMuted: Boolean? = null
    /** Whether the service turned the system's microphone mute on: it is turned off again at the end. */
    private var mutedTheSystem = false
    /** Whether the audio focus was last reported lost (or refused): given back as gained when Telecom takes the call's audio. */
    private var focusLost = false
    /** The session whose call's audio was reported ready ([audioReady]): once per start. */
    private var audioReadyFor: Long? = null
    /** Reports the call's audio ready past [AUDIO_READY_LIMIT_MS], if Telecom has not by then. */
    private var audioDeadline: Runnable? = null
    private val main = Handler(Looper.getMainLooper())

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val id = intent?.getLongExtra(EXTRA_SESSION, NO_SESSION) ?: NO_SESSION
        when (intent?.action) {
            ACTION_START -> begin(id, intent.getStringExtra(EXTRA_TITLE) ?: "", intent.getBooleanExtra(EXTRA_SYSTEM_CALL, false))
            // What the call shows now (its title) and the page's mute, for the session this instance serves.
            ACTION_UPDATE -> session?.takeIf { it.id == id }?.let {
                update(
                    it,
                    intent.getStringExtra(EXTRA_TITLE) ?: "",
                    intent.getBooleanExtra(EXTRA_MUTED, false),
                    intent.getStringExtra(EXTRA_SURFACE),
                )
            } ?: leaveIfIdle(startId)
            // Hang up, from the notification of the session this instance serves; one of an older call's means nothing.
            ACTION_HANG_UP -> session?.takeIf { it.id == id }?.let { finish("hang-up") } ?: leaveIfIdle(startId)
            // Ask for the audio focus again (it was lost for good, or refused), for the session this instance serves.
            ACTION_REFOCUS -> session?.takeIf { it.id == id }?.let { refocus(it) } ?: leaveIfIdle(startId)
            // Mute, unmute or skip, from the card's buttons, of this session alone: the person's action, run by the app.
            ACTION_SURFACE -> session?.takeIf { it.id == id }?.let { mine ->
                intent.getStringExtra(EXTRA_ACTION)?.takeIf { it in SERVICE_ACTIONS }?.let { CallSession.action(mine, it) }
            } ?: leaveIfIdle(startId)
            // The card's audio switch, of this session alone.
            ACTION_ROUTE -> session?.takeIf { it.id == id }?.let { toggleRoute(it) } ?: leaveIfIdle(startId)
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

    /** The appearance changed (light, dark): the card is drawn again in it. */
    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        session?.let { show(it.id) }
    }

    /** Whoever ended it, the focus and the report are not left behind (the system may end a service on its own). */
    override fun onDestroy() {
        cancelAudioDeadline()
        cancelShow()
        endTelecom()
        unwatchRoute()
        clearCommunication()
        closeMedia()
        abandonFocus()
        session?.let { CallSession.end(it, "system") }
        session = null
        super.onDestroy()
    }

    /**
     * Starts the service for session [id]. [systemCall]: the call is also a call of the system's Telecom (the developer
     * option "Treat calls as system calls"); off, it is the app's own from the start: the foreground service, the focus
     * and the card, with no registration with Telecom.
     */
    private fun begin(id: Long, title: String, systemCall: Boolean) {
        this.title = title
        card = CallCard.of(title, muted = false, surface = null)
        // Within seconds of a startForegroundService, even when the call already ended: Android crashes the app otherwise.
        try {
            ServiceCompat.startForeground(this, NOTIFICATION_ID, notification(id), FOREGROUND_SERVICE_TYPES)
            lastShown = SystemClock.uptimeMillis()
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
        watchRoute(mine)
        telecomEnded = false
        audioReadyFor = null
        cancelAudioDeadline()
        if (!systemCall) {
            // Telecom is not here to put the call in the communication mode and route it: the service does it itself.
            routeCommunication()
            audioReady(mine, "own call: no Telecom call")
            return
        }
        val deadline = Runnable {
            audioReady(mine, "fallback: Telecom did not make the call active within $AUDIO_READY_LIMIT_MS ms")
        }
        audioDeadline = deadline
        main.postDelayed(deadline, AUDIO_READY_LIMIT_MS)
        addTelecomCall(mine)
    }

    /**
     * The call's audio of [mine] may open now, for the reason [why] (logged): reported once per start, and only while
     * [mine] is the session this instance serves.
     */
    private fun audioReady(mine: CallSession.Session, why: String) {
        if (session?.id != mine.id || audioReadyFor == mine.id) return
        audioReadyFor = mine.id
        cancelAudioDeadline()
        Log.i(TAG, "audio: the call's audio opens now: $why")
        CallSession.audioReady(mine)
    }

    private fun cancelAudioDeadline() {
        audioDeadline?.let { main.removeCallbacks(it) }
        audioDeadline = null
    }

    /** Ends the call's service: the focus, the notification and the service itself, and the one report of its session. */
    private fun finish(reason: String) {
        cancelAudioDeadline()
        cancelShow()
        endTelecom()
        unwatchRoute()
        clearCommunication()
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

    /**
     * The call's notification: Sidevoice's card (HANDOVER §4 "Android"), expanded and collapsed, and its public version for a
     * Lock Screen that hides sensitive content ("Sidevoice · In call"). The system draws the header (mark and name; no
     * time, `setShowWhen(false)`); tapping it elsewhere opens the app.
     */
    private fun notification(id: Long): Notification {
        val manager = getSystemService(NotificationManager::class.java)
        // Created each time, so a change of the system's language renames it. DEFAULT importance, so the card is not a
        // silent notification (those are hidden on the Lock Screen by default and collapsed in the shade), but with no
        // sound, vibration or badge. A channel's importance can't be raised once created: hence a new id, and the old
        // LOW channel is deleted so Settings doesn't list two.
        manager.deleteNotificationChannel(OLD_CHANNEL_ID)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_ID, getString(R.string.call_channel_name), NotificationManager.IMPORTANCE_DEFAULT).apply {
                setSound(null, null)
                enableVibration(false)
                setShowBadge(false)
            }
        )
        val open = packageManager.getLaunchIntentForPackage(packageName)?.let {
            PendingIntent.getActivity(this, REQUEST_OPEN, it, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        }
        val views = CallCardViews(this, card, route(), intents(id), ::still)
        val publicVersion = NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_call_notification)
            .setContentText(getString(R.string.call_notification_text))
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setShowWhen(false)
            .build()
        val notification = NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_call_notification)
            // What a surface that does not draw the card (a watch, an accessibility service) reads.
            .setContentTitle(card.title.trim().ifEmpty { getString(R.string.call_notification_title) })
            .setContentText(card.words.state.ifEmpty { getString(R.string.call_notification_text) })
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .setContentIntent(open)
            .setStyle(NotificationCompat.DecoratedCustomViewStyle())
            .setCustomContentView(views.collapsed())
            .setCustomBigContentView(views.expanded())
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setPublicVersion(publicVersion)
            .build()
        keepStills()
        return notification
    }

    /**
     * Draws the card again, for the session [id] this instance serves: at most once per [UPDATE_INTERVAL_MS] (Android drops a
     * notification's updates past about five a second, and the last one may be the one lost). Whatever asks inside the
     * interval (the page's updates, a headset's several device events, the audio switch) is drawn once at its end, with the
     * card as it is then.
     */
    private fun show(id: Long) {
        if (session?.id != id || pendingShow != null) return
        val wait = lastShown + UPDATE_INTERVAL_MS - SystemClock.uptimeMillis()
        if (wait <= 0) {
            draw(id)
            return
        }
        val later = Runnable {
            pendingShow = null
            // The session served by then, which may be a call started meanwhile.
            session?.let { draw(it.id) }
        }
        pendingShow = later
        main.postDelayed(later, wait)
    }

    private fun draw(id: Long) {
        if (session?.id != id) return
        lastShown = SystemClock.uptimeMillis()
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification(id))
    }

    private fun cancelShow() {
        pendingShow?.let { main.removeCallbacks(it) }
        pendingShow = null
    }

    /**
     * The card's buttons, for session [id]. Each PendingIntent is told apart by its data (Android compares action, component
     * and data, not extras), so one session's never carries the next one's number, and mute is not unmute.
     */
    private fun intents(id: Long): CallCardViews.Intents {
        fun service(action: String, address: String, extra: String? = null): PendingIntent = PendingIntent.getService(
            this,
            0,
            Intent(this, CallService::class.java)
                .setAction(action)
                .setData(Uri.parse(address))
                .putExtra(EXTRA_SESSION, id)
                .also { intent -> extra?.let { intent.putExtra(EXTRA_ACTION, it) } },
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )
        val mute = if (card.muted) "unmute" else "mute"
        // The stack opens the app on its conversation list: the activity (after unlocking), told by an extra that the
        // plugin reads (MobilePlugin.onNewIntent) and reports as the person's action.
        val conversations = packageManager.getLaunchIntentForPackage(packageName)?.let {
            it.putExtra(EXTRA_OPEN_CONVERSATIONS, true)
            PendingIntent.getActivity(this, REQUEST_CONVERSATIONS, it, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        }
        return CallCardViews.Intents(
            mute = service(ACTION_SURFACE, CallSession.surfaceAddress(id, mute), mute),
            skip = service(ACTION_SURFACE, CallSession.surfaceAddress(id, "skip"), "skip"),
            hangUp = service(ACTION_HANG_UP, CallSession.hangUpAddress(id)),
            route = service(ACTION_ROUTE, CallSession.surfaceAddress(id, "route")),
            conversations = conversations,
        )
    }

    /**
     * A still the card draws, from the stills folder the app writes (Rust `stills_dir`): decoded once per version of the
     * file, so a still the app wrote again under the same name is drawn anew.
     */
    private fun still(still: CallCard.Still): Bitmap? {
        val file = File(File(cacheDir, STILLS_DIR), still.file)
        val version = StillVersion(file.lastModified(), file.length())
        stillImages[still.file]?.takeIf { it.first == version }?.let { return it.second }
        val image = runCatching { BitmapFactory.decodeFile(file.path) }.getOrNull() ?: return null
        stillImages[still.file] = version to image
        return image
    }

    /** Which write of a still's file was decoded: its time and size. */
    private data class StillVersion(val modified: Long, val length: Long)

    /** Lets go of the decoded stills the card no longer draws. */
    private fun keepStills() {
        val drawn = (listOf(card.still) + card.others).mapNotNull { it?.file }.toSet()
        stillImages.keys.retainAll(drawn)
    }

    /** What the system shows as the call's other party: its conversation, or the app's name. */
    private fun displayName(): String = title.trim().ifEmpty { getString(R.string.call_notification_title) }

    /** What the call shows now, the page's mute and the call surface ([ACTION_UPDATE]): the card is drawn again. */
    private fun update(mine: CallSession.Session, title: String, muted: Boolean, surface: String?) {
        this.title = title
        pageMuted = muted
        card = CallCard.of(title, muted, surface)
        applyMute()
        show(mine.id)
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
            echoes?.set(muted)
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
            audioReady(mine, "fallback: Telecom did not register the app")
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
                            audioReady(mine, "fallback: Telecom did not set the call active")
                            registered = null
                            call.disconnect(DisconnectCause(DisconnectCause.ERROR))
                            // Its collectors (mute, route) do not end with the disconnect: they go now, and the call's
                            // coroutine with them.
                            call.cancel()
                        } else {
                            telecomHolds(mine, call)
                            // The mode is the call's now; the route moves to the loudspeaker (below) unless something
                            // else is chosen. The audio opens once it is where it stays.
                            val (route, _) = combine(call.currentCallEndpoint, call.availableEndpoints) { current, available ->
                                current to available
                            }.first { (current, available) -> CallRoute.settled(current.type, available.map { it.type }) }
                            audioReady(mine, "Telecom made the call active, on ${route.name} (endpoint type ${route.type})")
                        }
                    }
                    // A mute from outside the app: a change of the system's microphone mute (Telecom's mute is that), told
                    // from the service's own coming back ([MuteEchoes]).
                    val audio = getSystemService(AudioManager::class.java)
                    val mutes = MuteEchoes(start = audio.isMicrophoneMute)
                    echoes = mutes
                    launch {
                        call.isMuted.collect { muted ->
                            if (mutes.observed(muted, now = audio.isMicrophoneMute) && control === call) {
                                CallSession.muted(mine, muted)
                            }
                        }
                    }
                    // The loudspeaker rather than the earpiece, as on iOS: a headset or a Bluetooth device stays chosen.
                    launch {
                        combine(call.currentCallEndpoint, call.availableEndpoints) { current, available -> current to available }
                            .collect { (current, available) ->
                                CallRoute.preferred(current.type, available.map { it.type })?.let { wanted ->
                                    available.firstOrNull { it.type == wanted }?.let { call.requestEndpointChange(it) }
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
                audioReady(mine, "fallback: Telecom refused the call")
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
        // The audio switch is Telecom's while it holds the call: the card drops it.
        show(mine.id)
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
        echoes = null
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
     * The call's media session, made for [mine] and only while it runs: Android hands it a headset's or a car's play/pause (a
     * press arrives as pause while the call plays, as play while it is muted), which go to the shared plugin
     * ([MediaControls.command]) and from there to the call (D-NO-CALLKIT-DEFAULT: the headset's buttons are the call's). No
     * notification shows it (the card is the call's), so it draws no media card; its Hang up custom action stays for a
     * controller that asks.
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
            }
        }
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

    /**
     * The card's audio switch, while headphones are connected and the call is the app's own (Telecom routes its own call):
     * where the call's audio goes now. Android 12 and later (`setCommunicationDevice`); before, no switch.
     */
    private fun route(): CallCardViews.Route? {
        if (control != null || Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return null
        val audio = getSystemService(AudioManager::class.java)
        val headphones = audio.availableCommunicationDevices.firstOrNull { it.type in HEADPHONES } ?: return null
        val current = audio.communicationDevice
        // Named by where the audio really goes: the headphones by their name, the loudspeaker and the earpiece by the
        // page's word or the plugin's own (before the page's words arrive).
        return when {
            current != null && current.type in HEADPHONES ->
                CallCardViews.Route(CallCardViews.Output.HEADPHONES, current.productName.toString().ifEmpty { headphones.productName.toString() })
            current?.type == AudioDeviceInfo.TYPE_BUILTIN_EARPIECE ->
                CallCardViews.Route(CallCardViews.Output.EARPIECE, getString(R.string.call_route_earpiece))
            else -> CallCardViews.Route(CallCardViews.Output.SPEAKER, card.words.speaker.ifEmpty { getString(R.string.call_route_speaker) })
        }
    }

    /**
     * The audio switch: the call moves between the headphones and the loudspeaker (D-LA-ROUTE: one tap, no earpiece), among
     * the devices the system offers a call now; the microphone follows the device. Logged (`SidevoiceCall`).
     */
    private fun toggleRoute(mine: CallSession.Session) {
        if (control != null || Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return
        val audio = getSystemService(AudioManager::class.java)
        val available = audio.availableCommunicationDevices
        val onHeadphones = audio.communicationDevice?.let { it.type in HEADPHONES } == true
        val target = if (onHeadphones) {
            available.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }
        } else {
            available.firstOrNull { it.type in HEADPHONES }
        }
        if (target == null) {
            Log.i(TAG, "route: nothing to switch to")
        } else {
            val taken = audio.setCommunicationDevice(target)
            routeChosen = routeChosen || taken
            Log.i(TAG, "route: switch to ${target.productName} (type ${target.type}): ${if (taken) "set" else "refused"}")
        }
        show(mine.id)
    }

    /** Draws the card again when a headset comes or goes, or the call's device changes, while [mine] runs. */
    private fun watchRoute(mine: CallSession.Session) {
        unwatchRoute()
        val audio = getSystemService(AudioManager::class.java)
        val devices = object : AudioDeviceCallback() {
            override fun onAudioDevicesAdded(added: Array<out AudioDeviceInfo>) = moved(mine)
            override fun onAudioDevicesRemoved(removed: Array<out AudioDeviceInfo>) = moved(mine)
        }
        audio.registerAudioDeviceCallback(devices, main)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val changed = AudioManager.OnCommunicationDeviceChangedListener { moved(mine) }
            audio.addOnCommunicationDeviceChangedListener({ main.post(it) }, changed)
            routeWatch = {
                audio.unregisterAudioDeviceCallback(devices)
                audio.removeOnCommunicationDeviceChangedListener(changed)
            }
        } else {
            routeWatch = { audio.unregisterAudioDeviceCallback(devices) }
        }
    }

    /** A device came or went, or the call's device changed: off the earpiece if it landed there, and the card redrawn. */
    private fun moved(mine: CallSession.Session) {
        if (session?.id != mine.id) return
        keepOffEarpiece()
        show(mine.id)
    }

    /** Stops [watchRoute], and gives back the device the audio switch chose. */
    private fun unwatchRoute() {
        routeWatch?.invoke()
        routeWatch = null
        if (routeChosen && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            getSystemService(AudioManager::class.java).clearCommunicationDevice()
        }
        routeChosen = false
    }

    /**
     * The own call's audio route, which Telecom does for a system's call: the audio mode is communication, and the route a
     * Bluetooth headset (HFP, or LE Audio) or a hearing aid, then wired or USB headphones, if one is connected (Android's
     * self-managed audio guide: `AudioManager.setCommunicationDevice`). With none, the loudspeaker, never the earpiece
     * (D-LA-ROUTE; before Android 12 `setSpeakerphoneOn`, [keepOffEarpiece]). Logged (`SidevoiceCall`): the mode and the device taken.
     */
    private fun routeCommunication() {
        val audio = getSystemService(AudioManager::class.java)
        if (!communicating) {
            modeBefore = audio.mode
            audio.mode = AudioManager.MODE_IN_COMMUNICATION
            communicating = true
        }
        Log.i(TAG, "route: audio mode communication (was ${modeBefore})")
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // A headset if one is connected, otherwise the loudspeaker: never the earpiece, which the communication mode
            // would choose by itself (D-LA-ROUTE: the speaker by default, no earpiece).
            val available = audio.availableCommunicationDevices
            val device = CallRoute.communicationDevice(available.map { it.type })?.let { available[it] }
            if (device == null) {
                Log.i(TAG, "route: no headset or loudspeaker for the call; the system's route stays")
            } else {
                val taken = audio.setCommunicationDevice(device)
                Log.i(TAG, "route: communication device ${device.productName} (type ${device.type}): ${if (taken) "set" else "refused"}")
                routedDevice = taken
            }
        } else if (outputs(audio).any { it.type == AudioDeviceInfo.TYPE_BLUETOOTH_SCO }) {
            @Suppress("DEPRECATION")
            audio.startBluetoothSco()
            routedSco = true
            Log.i(TAG, "route: Bluetooth SCO started (Android before 12)")
        } else {
            keepOffEarpiece()
        }
    }

    /** The outputs connected now. */
    private fun outputs(audio: AudioManager): Array<AudioDeviceInfo> = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS)

    /**
     * The own call is never left on the earpiece, which the person never chooses (D-LA-ROUTE): when the route falls there by
     * itself (headphones removed, the communication mode's default), it moves to the loudspeaker. Telecom routes
     * its own call. Logged (`SidevoiceCall`). If the system refuses, the card shows the earpiece as it is.
     */
    @Suppress("DEPRECATION") // isSpeakerphoneOn, before Android 12 only
    private fun keepOffEarpiece() {
        if (!communicating || control != null) return
        val audio = getSystemService(AudioManager::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            if (audio.communicationDevice?.type != AudioDeviceInfo.TYPE_BUILTIN_EARPIECE) return
            val speaker = audio.availableCommunicationDevices.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }
            val taken = speaker != null && audio.setCommunicationDevice(speaker)
            routedDevice = routedDevice || taken
            Log.i(TAG, "route: on the earpiece by itself, to the loudspeaker: ${if (taken) "set" else "refused"}")
        } else {
            // Before Android 12 the communication mode plays on the earpiece unless headphones are connected.
            val headphones = outputs(audio).any { it.type in HEADPHONES }
            if (!headphones && !audio.isSpeakerphoneOn) {
                audio.isSpeakerphoneOn = true
                routedSpeakerphone = true
                Log.i(TAG, "route: no headphones, the loudspeaker (Android before 12)")
            } else if (headphones && routedSpeakerphone) {
                audio.isSpeakerphoneOn = false
                routedSpeakerphone = false
                Log.i(TAG, "route: headphones connected, off the loudspeaker (Android before 12)")
            }
        }
    }

    /** Gives the communication route and mode back at the call's end, if [routeCommunication] took them. */
    private fun clearCommunication() {
        if (!communicating) return
        val audio = getSystemService(AudioManager::class.java)
        if (routedDevice) {
            audio.clearCommunicationDevice()
            routedDevice = false
        }
        if (routedSco) {
            @Suppress("DEPRECATION")
            audio.stopBluetoothSco()
            routedSco = false
        }
        if (routedSpeakerphone) {
            @Suppress("DEPRECATION")
            audio.isSpeakerphoneOn = false
            routedSpeakerphone = false
        }
        audio.mode = modeBefore
        communicating = false
        Log.i(TAG, "route: communication route cleared, audio mode back to ${modeBefore}")
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
        /** A button of the card the app runs: [EXTRA_ACTION] says which ([SERVICE_ACTIONS]). */
        const val ACTION_SURFACE = "ai.sidevoice.mobile.plugin.CALL_SURFACE"
        /** The card's audio switch. */
        const val ACTION_ROUTE = "ai.sidevoice.mobile.plugin.CALL_ROUTE"
        const val ACTION_UPDATE = "ai.sidevoice.mobile.plugin.CALL_UPDATE"
        /** The session's custom action that ends the call (the media controls of Android 13 and later). */
        private const val CUSTOM_HANG_UP = "ai.sidevoice.mobile.plugin.HANG_UP"
        /** The session (a number) an intent is for. */
        const val EXTRA_SESSION = "ai.sidevoice.mobile.plugin.SESSION"
        /** The conversation the call is on. */
        const val EXTRA_TITLE = "ai.sidevoice.mobile.plugin.TITLE"
        /** Whether the page has the call muted. */
        const val EXTRA_MUTED = "ai.sidevoice.mobile.plugin.MUTED"
        /** The call surface the card draws (Rust `CallSurface`, as JSON). */
        const val EXTRA_SURFACE = "ai.sidevoice.mobile.plugin.SURFACE"
        /** The action of an [ACTION_SURFACE]. */
        const val EXTRA_ACTION = "ai.sidevoice.mobile.plugin.ACTION"
        /** On the app's activity: the person tapped the stack of conversations; the app opens its conversation list. */
        const val EXTRA_OPEN_CONVERSATIONS = "ai.sidevoice.mobile.plugin.OPEN_CONVERSATIONS"
        /** The card's buttons the app runs (Rust `SurfaceAction`); hang up is the service's own, the stack the activity's. */
        private val SERVICE_ACTIONS = setOf("mute", "unmute", "skip")
        /** The headphones the audio switch moves the call to: wired, USB, Bluetooth (HFP, LE Audio) and hearing aids. */
        private val HEADPHONES = CallRoute.HEADPHONES
        /** The folder the app keeps the call's stills in, in its cache (Rust `call_surface::stills_dir`). */
        private const val STILLS_DIR = "call-stills"
        private const val REQUEST_OPEN = 0
        private const val REQUEST_CONVERSATIONS = 1
        const val EXTRA_SYSTEM_CALL = "ai.sidevoice.mobile.plugin.SYSTEM_CALL"
        private const val TAG = "SidevoiceCall"

        private const val NO_SESSION = -1L
        private const val CHANNEL_ID = "call-card"
        /** The call's channel before the card (IMPORTANCE_LOW): deleted. */
        private const val OLD_CHANNEL_ID = "call"
        private const val NOTIFICATION_ID = 1
        /**
         * How long the call's audio waits for Telecom before it opens anyway (Telecom answers in well under a second):
         * shorter than the app's own wait for this report (call.rs, `SYSTEM_AUDIO_LIMIT`), past which the call is refused.
         */
        private const val AUDIO_READY_LIMIT_MS = 3_000L
        /** The least time between two drawings of the card (as the app's own updates, call.rs `UPDATE_INTERVAL`). */
        private const val UPDATE_INTERVAL_MS = 250L
        // What the manifest declares for the service: the call listens, the agent speaks, and it is a phone call (Telecom's,
        // which the manifest's MANAGE_OWN_CALLS allows).
        private const val FOREGROUND_SERVICE_TYPES =
            ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE or ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK or
                ServiceInfo.FOREGROUND_SERVICE_TYPE_PHONE_CALL
    }
}
