package ai.sidevoice.spike.holder

import android.app.KeyguardManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.Network
import android.os.PowerManager
import android.os.SystemClock
import org.json.JSONObject
import java.io.File
import java.io.FileWriter

/**
 * S0 spike: what the phone does around a run, in both runs (N and P), written to `system.jsonl` in the run directory:
 * screen off/on, unlock, the default network coming and going. Registered on the application context, so it outlives
 * the activity (task removal) for as long as the process does.
 */
object SystemWatch {
    private var log: FileWriter? = null
    private var receiver: BroadcastReceiver? = null
    private var network: ConnectivityManager.NetworkCallback? = null
    private var context: Context? = null

    @Synchronized
    fun start(context: Context, dir: String) {
        stop()
        val app = context.applicationContext
        this.context = app
        log = FileWriter(File(dir, "system.jsonl"), true)
        write("watch-start", state(app))
        receiver = object : BroadcastReceiver() {
            override fun onReceive(c: Context, intent: Intent) {
                val kind = when (intent.action) {
                    Intent.ACTION_SCREEN_OFF -> "screen-off"
                    Intent.ACTION_SCREEN_ON -> "screen-on"
                    Intent.ACTION_USER_PRESENT -> "unlocked"
                    else -> intent.action ?: "?"
                }
                write(kind, state(app))
            }
        }.also {
            app.registerReceiver(it, IntentFilter().apply {
                addAction(Intent.ACTION_SCREEN_OFF)
                addAction(Intent.ACTION_SCREEN_ON)
                addAction(Intent.ACTION_USER_PRESENT)
            })
        }
        network = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(net: Network) { write("network-available", JSONObject().put("network", net.toString())) }
            override fun onLost(net: Network) { write("network-lost", JSONObject().put("network", net.toString())) }
        }.also { app.getSystemService(ConnectivityManager::class.java).registerDefaultNetworkCallback(it) }
    }

    @Synchronized
    fun stop() {
        val app = context ?: return
        write("watch-stop", state(app))
        receiver?.let { runCatching { app.unregisterReceiver(it) } }
        network?.let { runCatching { app.getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(it) } }
        receiver = null
        network = null
        runCatching { log?.close() }
        log = null
        context = null
    }

    private fun state(app: Context): JSONObject = JSONObject()
        .put("interactive", app.getSystemService(PowerManager::class.java).isInteractive)
        .put("keyguard", app.getSystemService(KeyguardManager::class.java).isKeyguardLocked)

    @Synchronized
    private fun write(kind: String, data: JSONObject) {
        val line = data.put("t", System.currentTimeMillis()).put("up", SystemClock.elapsedRealtime())
            .put("src", "system").put("k", kind)
        runCatching { log?.apply { write(line.toString()); write("\n"); flush() } }
    }
}
