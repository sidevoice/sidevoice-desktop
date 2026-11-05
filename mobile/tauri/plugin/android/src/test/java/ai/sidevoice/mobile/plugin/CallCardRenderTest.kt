package ai.sidevoice.mobile.plugin

import android.app.Notification
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.view.View
import android.widget.FrameLayout
import android.widget.RemoteViews
import androidx.core.app.NotificationCompat
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File
import java.io.FileOutputStream

/**
 * The call card drawn to PNG, every state, light and dark, mic on and off, plus the long texts: the notification as Android
 * itself lays it out (the platform's DecoratedCustomViewStyle template around [CallCardViews]' views), expanded, collapsed
 * and the public version. Robolectric's native graphics draw the real views (layouts, DM Sans, the bitmaps of
 * [CallCardArt]); what they cannot show is the device's own notification shade (its blur, its exact Material You colours).
 *
 * The PNGs go to `build/call-card-renders` (`cargo xtask mobile test android` copies them to `dist/android-call-card`, CI
 * uploads them): the evidence to compare with the design's renders (research/live-activity-design-2026-11-02/png). The
 * test fails only if a view cannot be drawn.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [36], qualifiers = "es-w412dp-h915dp-xxhdpi")
class CallCardRenderTest {
    private val out = File(System.getProperty("user.dir"), "build/call-card-renders").apply { mkdirs() }
    private val stills = mapOf(
        "straw-hat" to load("straw-hat"),
        "bunny-headphones" to load("bunny-headphones"),
        "wizard-hat" to load("wizard-hat"),
    )

    @Test
    fun everyStateLightAndDark() {
        // The header's name: the app's on a device, the test package's here.
        ApplicationProvider.getApplicationContext<Context>().applicationInfo.nonLocalizedLabel = "Sidevoice"
        for (night in listOf(false, true)) {
            RuntimeEnvironment.setQualifiers(if (night) "+night" else "+notnight")
            for (phase in PHASES) {
                for (muted in listOf(false, true)) {
                    val card = card(phase, muted)
                    val name = "${phase.wire}-${if (muted) "mic-off" else "mic-on"}-${if (night) "dark" else "light"}"
                    render(card, name, headphones = true)
                }
            }
            val appearance = if (night) "dark" else "light"
            // The long texts (the gallery's "long texts"): title and model · machine end in an ellipsis.
            render(
                card(CallCard.Phase.SPEAKING, false).copy(
                    title = "Refactorizar las notificaciones del servicio de llamadas en Android",
                    model = "claude-opus-5-5 · xhigh",
                    machine = "MacBook-Pro-de-Ruben.local",
                ),
                "long-speaking-$appearance",
                headphones = true,
            )
            // The state line's action in English, muted.
            for (phase in listOf(CallCard.Phase.SPEAKING, CallCard.Phase.LISTENING)) {
                val card = card(phase, true)
                val state = if (phase == CallCard.Phase.SPEAKING) "Speaking" else "Listening"
                render(
                    card.copy(words = card.words.copy(state = state, mute = "Mute", unmute = "Unmute", skip = "Skip", cancelSend = "Cancel sending")),
                    "${phase.wire}-en-mic-off-$appearance",
                    headphones = true,
                )
            }
            // No headphones: no audio switch.
            render(card(CallCard.Phase.IDLE, false), "idle-no-headphones-$appearance", headphones = false)
            save(publicVersion(), "public-$appearance", collapsed = true)
        }
        assertTrue(out.listFiles()!!.isNotEmpty())
    }

    private fun render(card: CallCard, name: String, headphones: Boolean) {
        val context: Context = ApplicationProvider.getApplicationContext()
        val route = if (headphones) CallCardViews.Route(CallCardViews.Output.HEADPHONES, name = "AirPods Pro") else null
        val intents = CallCardViews.Intents(null, null, null, null, null, null)
        val views = CallCardViews(context, card, route, intents) { still -> stills[still.file] }
        val notification = NotificationCompat.Builder(context, "call-card")
            .setSmallIcon(R.drawable.ic_call_notification)
            .setShowWhen(false)
            .setStyle(NotificationCompat.DecoratedCustomViewStyle())
            .setCustomContentView(views.collapsed())
            .setCustomBigContentView(views.expanded())
            .build()
        save(notification, "$name-expanded", collapsed = false)
        save(notification, "$name-collapsed", collapsed = true)
    }

    private fun publicVersion(): Notification {
        val context: Context = ApplicationProvider.getApplicationContext()
        return NotificationCompat.Builder(context, "call-card")
            .setSmallIcon(R.drawable.ic_call_notification)
            .setContentText(context.getString(R.string.call_notification_text))
            .setShowWhen(false)
            .build()
    }

    /** The notification as the platform's template draws it, on the shade's colour, 380 dp wide (the design's width). */
    private fun save(notification: Notification, name: String, collapsed: Boolean) {
        val context: Context = ApplicationProvider.getApplicationContext()
        val builder = Notification.Builder.recoverBuilder(context, notification)
        val remote: RemoteViews = if (collapsed) builder.createContentView() else builder.createBigContentView()
        val parent = FrameLayout(context)
        val view = remote.apply(context, parent)
        parent.addView(view)
        val density = context.resources.displayMetrics.density
        val width = (380 * density).toInt()
        parent.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY), View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.AT_MOST))
        parent.layout(0, 0, parent.measuredWidth, parent.measuredHeight)
        val bitmap = Bitmap.createBitmap(parent.measuredWidth, parent.measuredHeight.coerceAtLeast(1), Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        canvas.drawColor(if (night(context)) SHADE_DARK else SHADE_LIGHT)
        parent.draw(canvas)
        FileOutputStream(File(out, "$name.png")).use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
    }

    private fun night(context: Context): Boolean =
        context.resources.configuration.uiMode and android.content.res.Configuration.UI_MODE_NIGHT_MASK ==
            android.content.res.Configuration.UI_MODE_NIGHT_YES

    private fun card(phase: CallCard.Phase, muted: Boolean) = CallCard(
        title = "Globo del escenario",
        muted = muted,
        phase = phase,
        canSkip = phase == CallCard.Phase.SPEAKING,
        canCancelSend = phase == CallCard.Phase.LISTENING || phase == CallCard.Phase.TRANSCRIBING,
        model = "Opus 5.5",
        machine = "mac-studio",
        workingSince = if (phase == CallCard.Phase.WORKING) System.currentTimeMillis() - 102_000 else null,
        still = CallCard.Still("straw-hat", 1.3),
        others = listOf(CallCard.Still("bunny-headphones", 1.35), CallCard.Still("wizard-hat", 1.4)),
        conversationCount = 3,
        words = CallCard.Words(
            state = STATES.getValue(phase),
            mute = "Silenciar",
            unmute = "Activar micrófono",
            skip = "Saltar",
            skipName = "Saltar lo que está sonando",
            cancelSend = "Cancelar envío",
            hangUp = "Colgar",
            route = "Salida de audio: {route}",
            speaker = "Altavoz",
            conversations = "Conversaciones (3)",
        ),
    )

    private fun load(name: String): Bitmap =
        javaClass.classLoader!!.getResourceAsStream("stills/$name.png").use { BitmapFactory.decodeStream(it) }

    private companion object {
        val PHASES = listOf(
            CallCard.Phase.IDLE, CallCard.Phase.LISTENING, CallCard.Phase.TRANSCRIBING, CallCard.Phase.WORKING,
            CallCard.Phase.SPEAKING, CallCard.Phase.HOLDING, CallCard.Phase.OFFLINE,
        )
        /** The page's words in Spanish, as the design's renders show them. */
        val STATES = mapOf(
            CallCard.Phase.IDLE to "En reposo",
            CallCard.Phase.LISTENING to "Escuchándote",
            CallCard.Phase.TRANSCRIBING to "Transcribiendo…",
            CallCard.Phase.WORKING to "Trabajando",
            CallCard.Phase.SPEAKING to "Hablando",
            CallCard.Phase.HOLDING to "No puede recibir",
            CallCard.Phase.OFFLINE to "Desconectada",
        )
        /** The design's notification panel: rgba(247, 242, 250, .94) and rgba(43, 41, 48, .94), opaque here. */
        val SHADE_LIGHT = Color.rgb(247, 242, 250)
        val SHADE_DARK = Color.rgb(43, 41, 48)
    }
}
