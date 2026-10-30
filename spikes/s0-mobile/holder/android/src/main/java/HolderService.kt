package ai.sidevoice.spike.holder

import android.app.KeyguardManager
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaRecorder
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.os.SystemClock
import app.tauri.plugin.JSObject
import org.json.JSONObject
import java.io.File
import java.io.FileWriter
import kotlin.math.abs

/**
 * S0 spike, run N on Android: a foreground service of type microphone holding the call's native audio, AudioRecord
 * reading the microphone and a silent AudioTrack writing zeros, with nothing done with either but counting. Every
 * 5 s it writes what it counted to `holder.jsonl` in the run directory, with the wall clock (`t`, ms) and the boot
 * clock (`up`, ms), the screen and keyguard as it ticks, and the task's removal. Lock, screen and network events are
 * `SystemWatch`'s, in both runs. Nothing here is product code.
 */
class HolderService : Service() {
    companion object {
        const val EXTRA_DIR = "dir"
        private const val CHANNEL = "s0-holder"
        private const val NOTIFICATION_ID = 4290
        private const val RATE = 16000

        @Volatile private var running = false
        @Volatile private var captureFrames = 0L
        @Volatile private var renderFrames = 0L
        @Volatile private var capturePeak = 0
        @Volatile private var lastError: String? = null

        fun status(): JSObject = JSObject()
            .put("running", running)
            .put("captureFrames", captureFrames)
            .put("renderFrames", renderFrames)
            .put("error", lastError)
    }

    private var log: FileWriter? = null
    private var record: AudioRecord? = null
    private var track: AudioTrack? = null
    private val threads = mutableListOf<Thread>()

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val dir = intent?.getStringExtra(EXTRA_DIR)
        if (running) {
            write("holder-start-ignored", JSONObject().put("reason", "already running"))
            return START_NOT_STICKY
        }
        if (dir == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        log = FileWriter(File(dir, "holder.jsonl"), true)
        startForeground()
        running = true
        captureFrames = 0
        renderFrames = 0
        write("holder-start", JSONObject().put("sdk", Build.VERSION.SDK_INT).put("device", "${Build.MANUFACTURER} ${Build.MODEL}"))
        startAudio()
        threads += Thread({ ticker() }, "s0-ticker").also { it.start() }
        return START_NOT_STICKY
    }

    private fun startForeground() {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(CHANNEL, getString(R.string.s0_channel), NotificationManager.IMPORTANCE_LOW))
        val open = packageManager.getLaunchIntentForPackage(packageName)?.let {
            PendingIntent.getActivity(this, 0, it, PendingIntent.FLAG_IMMUTABLE)
        }
        val notification = Notification.Builder(this, CHANNEL)
            .setContentTitle(getString(R.string.s0_notification_title))
            .setContentText(getString(R.string.s0_notification_text))
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setOngoing(true)
            .setContentIntent(open)
            .build()
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun startAudio() {
        try {
            val inMin = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
            val rec = AudioRecord(MediaRecorder.AudioSource.VOICE_COMMUNICATION, RATE, AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT, maxOf(inMin, RATE / 5 * 2))
            rec.startRecording()
            record = rec
            threads += Thread({
                val buffer = ShortArray(RATE / 10)
                while (running) {
                    val n = rec.read(buffer, 0, buffer.size)
                    if (n < 0) {
                        lastError = "AudioRecord.read $n"
                        write("capture-error", JSONObject().put("code", n))
                        Thread.sleep(500)
                        continue
                    }
                    var peak = 0
                    for (i in 0 until n) peak = maxOf(peak, abs(buffer[i].toInt()))
                    capturePeak = maxOf(capturePeak, peak)
                    captureFrames += n
                }
            }, "s0-capture").also { it.start() }
        } catch (e: Exception) {
            lastError = "capture: ${e.message}"
            write("capture-error", JSONObject().put("message", e.message))
        }
        try {
            val outMin = AudioTrack.getMinBufferSize(RATE, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT)
            val out = AudioTrack.Builder()
                .setAudioAttributes(AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build())
                .setAudioFormat(AudioFormat.Builder()
                    .setSampleRate(RATE)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .build())
                .setBufferSizeInBytes(maxOf(outMin, RATE / 5 * 2))
                .setTransferMode(AudioTrack.MODE_STREAM)
                .build()
            out.play()
            track = out
            threads += Thread({
                val silence = ShortArray(RATE / 10)
                while (running) {
                    val n = out.write(silence, 0, silence.size)
                    if (n < 0) {
                        lastError = "AudioTrack.write $n"
                        write("render-error", JSONObject().put("code", n))
                        Thread.sleep(500)
                        continue
                    }
                    renderFrames += n
                }
            }, "s0-render").also { it.start() }
        } catch (e: Exception) {
            lastError = "render: ${e.message}"
            write("render-error", JSONObject().put("message", e.message))
        }
    }

    private fun screen(): JSONObject = JSONObject()
        .put("interactive", getSystemService(PowerManager::class.java).isInteractive)
        .put("keyguard", getSystemService(KeyguardManager::class.java).isKeyguardLocked)

    private fun ticker() {
        var lastCapture = 0L
        var lastRender = 0L
        while (running) {
            try { Thread.sleep(5000) } catch (_: InterruptedException) { break }
            val capture = captureFrames
            val render = renderFrames
            val peak = capturePeak
            capturePeak = 0
            write("holder-tick", screen()
                .put("capture_frames", capture).put("capture_delta", capture - lastCapture).put("capture_peak", peak)
                .put("render_frames", render).put("render_delta", render - lastRender)
                .put("record_state", record?.recordingState ?: -1)
                .put("track_head", track?.playbackHeadPosition ?: -1))
            lastCapture = capture
            lastRender = render
        }
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        write("task-removed", screen())
        super.onTaskRemoved(rootIntent)
    }

    override fun onDestroy() {
        running = false
        write("holder-stop", JSONObject().put("capture_frames", captureFrames).put("render_frames", renderFrames))
        threads.forEach { runCatching { it.interrupt(); it.join(1000) } }
        threads.clear()
        record?.let { runCatching { it.stop(); it.release() } }
        track?.let { runCatching { it.stop(); it.release() } }
        record = null
        track = null
        synchronized(this) { runCatching { log?.close() }; log = null }
        super.onDestroy()
    }

    @Synchronized
    private fun write(kind: String, data: JSONObject) {
        val line = data.put("t", System.currentTimeMillis()).put("up", SystemClock.elapsedRealtime())
            .put("src", "holder").put("k", kind)
        runCatching { log?.apply { write(line.toString()); write("\n"); flush() } }
    }
}
