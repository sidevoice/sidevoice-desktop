package ai.sidevoice.mobile.plugin

import android.app.PendingIntent
import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.ColorMatrix
import android.graphics.ColorMatrixColorFilter
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RadialGradient
import android.graphics.RectF
import android.graphics.Shader
import android.os.Build
import android.os.SystemClock
import android.text.TextPaint
import android.util.TypedValue
import android.view.View
import android.widget.RemoteViews
import androidx.core.content.res.ResourcesCompat
import kotlin.math.roundToInt

/**
 * The call's notification as Sidevoice's own card (HANDOVER §4 "Android", spec/GEOMETRY.md): the expanded and the collapsed
 * content views of a `DecoratedCustomViewStyle`, filled from a [CallCard]. The creature's tile and the stack of faces are
 * drawn here as bitmaps ([CallCardArt]), so that the glow, the accessory standing out of its slot and the mute badge look
 * the same on every Android version; the rest is the layouts' views.
 */
internal class CallCardViews(
    private val context: Context,
    private val card: CallCard,
    /** The audio switch, while headphones are connected; null hides it. */
    private val route: Route?,
    private val intents: Intents,
    /** A still's image by its file (decoded once by the caller); null where it cannot be read. */
    private val stills: (CallCard.Still) -> Bitmap?,
) {
    /** Where the audio goes now ([output]), and its name ([name]: the headphones' own, or the word for the speaker or the earpiece). */
    data class Route(val output: Output, val name: String)

    /** The outputs the audio switch shows: the headphones, the loudspeaker, or the earpiece (the switch then moves to the headphones). */
    enum class Output { HEADPHONES, SPEAKER, EARPIECE }

    /** What each button does. [mute] sets the mute the button shows ("Silenciar" mutes, "Activar micrófono" unmutes). */
    data class Intents(
        val mute: PendingIntent?,
        val skip: PendingIntent?,
        val cancelSend: PendingIntent?,
        val hangUp: PendingIntent?,
        val route: PendingIntent?,
        val conversations: PendingIntent?,
    )

    private val art = CallCardArt(context)

    /** The expanded card: tile, three lines (the state's with its action: skip or cancel send), the audio switch; mute, the
     *  stack and hang up. */
    fun expanded(): RemoteViews {
        val views = RemoteViews(context.packageName, R.layout.call_card)
        val tone = tone()
        views.setImageViewBitmap(R.id.call_card_tile, art.tile(card.still?.let(stills), card.still?.k ?: 1.0, tone.fill, grey = card.phase == CallCard.Phase.OFFLINE, muted = card.muted && card.phase != CallCard.Phase.OFFLINE))
        views.setTextViewText(R.id.call_card_title, title())
        views.setTextViewText(R.id.call_card_sub, card.sub)
        views.setViewVisibility(R.id.call_card_sub, if (card.sub.isEmpty()) View.GONE else View.VISIBLE)
        val stated = card.words.state.isNotEmpty()
        views.setTextViewText(R.id.call_card_state, card.words.state)
        views.setTextColor(R.id.call_card_state, tone.ink)
        views.setInt(R.id.call_card_dot, "setColorFilter", tone.fill)
        views.setViewVisibility(R.id.call_card_dot, if (stated) View.VISIBLE else View.GONE)
        if (card.timed) {
            // The system's timer since the working phase began, on the device's clock: " · 1:42".
            val since = SystemClock.elapsedRealtime() - (System.currentTimeMillis() - card.workingSince!!)
            views.setChronometer(R.id.call_card_since, since, null, true)
            for (id in listOf(R.id.call_card_since_dot, R.id.call_card_since)) {
                views.setTextColor(id, withAlpha(tone.ink, SINCE_ALPHA))
                views.setViewVisibility(id, View.VISIBLE)
            }
        } else {
            views.setChronometer(R.id.call_card_since, SystemClock.elapsedRealtime(), null, false)
            views.setViewVisibility(R.id.call_card_since_dot, View.GONE)
            views.setViewVisibility(R.id.call_card_since, View.GONE)
        }

        if (route != null) {
            views.setViewVisibility(R.id.call_card_route, View.VISIBLE)
            views.setImageViewResource(
                R.id.call_card_route_icon,
                when (route.output) {
                    Output.HEADPHONES -> R.drawable.ic_card_headphones
                    Output.SPEAKER -> R.drawable.ic_card_speaker
                    Output.EARPIECE -> R.drawable.ic_card_earpiece
                },
            )
            views.setContentDescription(R.id.call_card_route, card.words.route.replace("{route}", route.name).ifEmpty { route.name })
            intents.route?.let { views.setOnClickPendingIntent(R.id.call_card_route, it) }
        } else {
            views.setViewVisibility(R.id.call_card_route, View.GONE)
        }

        // The pill with its label when the label fits whole, otherwise the round glyph button: never an ellipsis.
        val faces = card.others.take(3)
        if (labelFits(faces.size)) {
            views.setViewVisibility(R.id.call_card_mute, View.VISIBLE)
            views.setViewVisibility(R.id.call_card_mute_round, View.GONE)
            mute(views, R.id.call_card_mute, R.id.call_card_mute_icon)
            views.setTextViewText(R.id.call_card_mute_label, muteLabel())
            views.setTextColor(R.id.call_card_mute_label, color(if (card.muted) R.color.call_card_muted_ink else R.color.call_card_text))
        } else {
            views.setViewVisibility(R.id.call_card_mute, View.GONE)
            views.setViewVisibility(R.id.call_card_mute_round, View.VISIBLE)
            mute(views, R.id.call_card_mute_round, R.id.call_card_mute_round_icon)
        }

        // The action the state offers, on the state line: Saltar while the agent speaks, Cancelar envío during the person's
        // turn, none otherwise.
        val action = card.action
        views.setViewVisibility(R.id.call_card_action, if (action != null) View.VISIBLE else View.GONE)
        when (action) {
            CallCard.Action.SKIP -> {
                views.setImageViewResource(R.id.call_card_action_icon, R.drawable.ic_card_skip)
                views.setTextViewText(R.id.call_card_action_label, card.words.skip.ifEmpty { context.getString(R.string.call_notification_skip) })
                views.setContentDescription(R.id.call_card_action, card.words.skipName.ifEmpty { card.words.skip })
                intents.skip?.let { views.setOnClickPendingIntent(R.id.call_card_action, it) }
            }
            CallCard.Action.CANCEL_SEND -> {
                views.setImageViewResource(R.id.call_card_action_icon, R.drawable.ic_card_cancel)
                views.setTextViewText(R.id.call_card_action_label, card.words.cancelSend.ifEmpty { context.getString(R.string.call_notification_cancel_send) })
                views.setContentDescription(R.id.call_card_action, null)
                intents.cancelSend?.let { views.setOnClickPendingIntent(R.id.call_card_action, it) }
            }
            null -> {}
        }

        if (faces.isEmpty()) {
            views.setViewVisibility(R.id.call_card_others, View.GONE)
        } else {
            views.setViewVisibility(R.id.call_card_others, View.VISIBLE)
            views.setImageViewBitmap(R.id.call_card_faces, art.faces(faces.map { other -> other?.let { it to stills(it) } }))
            views.setTextViewText(R.id.call_card_count, card.count.toString())
            views.setContentDescription(R.id.call_card_others, card.words.conversations)
            intents.conversations?.let { views.setOnClickPendingIntent(R.id.call_card_others, it) }
        }

        hangUp(views)
        return views
    }

    /** The collapsed card: the face, "{title} · {state}", mute and hang up. */
    fun collapsed(): RemoteViews {
        val views = RemoteViews(context.packageName, R.layout.call_card_collapsed)
        views.setImageViewBitmap(R.id.call_card_face, art.round(card.still?.let(stills)))
        // Only the title is cut (the layout's weight); the state always shows whole.
        views.setTextViewText(R.id.call_card_line, title())
        val stated = card.words.state.isNotEmpty()
        views.setTextViewText(R.id.call_card_line_state, card.words.state)
        views.setTextColor(R.id.call_card_line_state, tone().ink)
        views.setViewVisibility(R.id.call_card_line_dot, if (stated) View.VISIBLE else View.GONE)
        views.setViewVisibility(R.id.call_card_line_state, if (stated) View.VISIBLE else View.GONE)
        mute(views, R.id.call_card_mute, R.id.call_card_mute_icon)
        hangUp(views)
        return views
    }

    /** The mute button [button] with its glyph [icon]: salmon while the mic is off, its full label as its accessible name. */
    private fun mute(views: RemoteViews, button: Int, icon: Int) {
        views.setInt(button, "setBackgroundResource", if (card.muted) R.drawable.call_card_button_muted else R.drawable.call_card_button)
        views.setImageViewResource(icon, if (card.muted) R.drawable.ic_card_mic_off else R.drawable.ic_card_mic)
        views.setContentDescription(button, muteLabel())
        intents.mute?.let { views.setOnClickPendingIntent(button, it) }
    }

    /**
     * Whether the mute pill's label fits whole in the actions row, measured with the pill's font against the row's real
     * width ([rowWidth]), with the stack of [faces] faces and its count, and hang up beside it (call_card.xml).
     */
    private fun labelFits(faces: Int): Boolean {
        val metrics = context.resources.displayMetrics
        fun dp(value: Float) = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, value, metrics)
        fun paint(font: Int, sp: Float) = TextPaint(Paint.ANTI_ALIAS_FLAG).apply {
            typeface = ResourcesCompat.getFont(context, font)
            textSize = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, sp, metrics)
        }
        // The pill: 13 + glyph 17 + 6 + label + 16; the room's 8; hang up 56.
        val pill = dp(PILL_CHROME) + paint(R.font.dm_sans_650, 14f).measureText(muteLabel())
        val stack = if (faces == 0) 0f else {
            dp((CallCardArt.FACE + (faces - 1) * (CallCardArt.FACE - CallCardArt.FACE_OVERLAP) + STACK_GAPS).toFloat()) +
                paint(R.font.dm_sans_650, 13f).measureText(card.count.toString())
        }
        return pill + dp(ROW_FIXED) + stack <= dp(rowWidth())
    }

    /**
     * The actions row's width (dp): the notification as wide as the screen leaves (16 dp of shade at each side, at most the
     * design's 380), less its 16 dp padding at each side, and on Android 12 and later the system's indent behind its icon
     * column (36 dp: 312 of 348 in the design). An underestimate only shows the glyph where the label might have fitted.
     */
    private fun rowWidth(): Float {
        val notification = minOf(context.resources.configuration.screenWidthDp - 2 * SHADE_MARGIN, NOTIFICATION_WIDTH)
        val indent = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) SYSTEM_INDENT else 0
        return (notification - 2 * NOTIFICATION_PADDING - indent).toFloat()
    }

    private fun hangUp(views: RemoteViews) {
        views.setContentDescription(R.id.call_card_hang_up, card.words.hangUp.ifEmpty { context.getString(R.string.call_notification_hang_up) })
        intents.hangUp?.let { views.setOnClickPendingIntent(R.id.call_card_hang_up, it) }
    }

    private fun title(): String = card.title.trim().ifEmpty { context.getString(R.string.call_notification_title) }

    private fun muteLabel(): String =
        if (card.muted) card.words.unmute.ifEmpty { context.getString(R.string.call_notification_unmute) }
        else card.words.mute.ifEmpty { context.getString(R.string.call_notification_mute) }

    /** The phase's tone in this appearance: its ink (words) and fill (dot, glow). */
    private fun tone(): Ink = when (card.phase.tone) {
        CallCard.Tone.LISTEN -> Ink(color(R.color.call_card_listen_ink), color(R.color.call_card_listen))
        CallCard.Tone.SPEAK -> Ink(color(R.color.call_card_speak_ink), color(R.color.call_card_speak))
        CallCard.Tone.WORK -> Ink(color(R.color.call_card_work_ink), color(R.color.call_card_work))
        CallCard.Tone.WARN -> Ink(color(R.color.call_card_warn_ink), color(R.color.call_card_warn))
        CallCard.Tone.IDLE -> Ink(color(R.color.call_card_idle_ink), color(R.color.call_card_idle))
        CallCard.Tone.OFFLINE -> Ink(color(R.color.call_card_offline_ink), color(R.color.call_card_offline))
    }

    private data class Ink(val ink: Int, val fill: Int)

    private fun color(id: Int): Int = context.getColor(id)

    private fun withAlpha(color: Int, alpha: Float): Int = ((alpha * 255).roundToInt() shl 24) or (color and 0xFFFFFF)

    private companion object {
        const val PILL_CHROME = 13f + 17f + 6f + 16f
        /** The gap after the mute (8), hang up (56). */
        const val ROW_FIXED = 8f + 56f
        /** The count's gap (3) and the stack's gap before hang up (8). */
        const val STACK_GAPS = 3 + 8
        const val SHADE_MARGIN = 16
        const val NOTIFICATION_WIDTH = 380
        const val NOTIFICATION_PADDING = 16
        const val SYSTEM_INDENT = 36
        /** The working time is drawn a little lighter than the state (the design's 85 %). */
        const val SINCE_ALPHA = 0.85f
    }
}

/**
 * The card's drawn parts, at the screen's density (spec/GEOMETRY.md "androidExpanded" / "androidCollapsed", the gallery's
 * `.tile`, `.others` and `.face.round`):
 * - the tile: 52 dp, radius 30 %, a radial glow of the tone (34 %) into the tile's ground; the creature inset 7 % and
 *   enlarged by its `k` from 96 % of its height (D-LA-FIT, overflow), so an accessory stands out above; grey while offline;
 *   the red mic badge, 22 dp with a 2 dp ring, at −4 dp bottom right, while the mic is off;
 * - the other conversations' faces, 26 dp overlapping by 8, each enlarged by its `k` the same way;
 * - the collapsed card's face, 24 dp round, the creature at 118 % and 5 % down (the gallery's round face).
 *
 * Each bitmap carries [PAD] dp of room around its slot for what stands out; the layouts take it back with negative margins.
 */
internal class CallCardArt(private val context: Context) {
    private val density = context.resources.displayMetrics.density

    fun tile(still: Bitmap?, k: Double, tone: Int, grey: Boolean, muted: Boolean): Bitmap {
        val face = TILE
        val (bitmap, canvas) = canvas(face + 2 * TILE_PAD_X, face + 2 * TILE_PAD_Y)
        canvas.translate(px(TILE_PAD_X), px(TILE_PAD_Y + TILE_TOP))
        val size = px(face)
        val ground = context.getColor(R.color.call_card_tile)
        val glow = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            // CSS: radial-gradient(circle at 50% 70%, tone 34%, ground 72%), the circle reaching the farthest corner.
            val cx = size * 0.5f
            val cy = size * 0.7f
            val radius = kotlin.math.hypot(cx, cy)
            shader = RadialGradient(cx, cy, radius, intArrayOf(alpha(tone, 0.34f), ground, ground), floatArrayOf(0f, 0.72f, 1f), Shader.TileMode.CLAMP)
        }
        canvas.drawRoundRect(RectF(0f, 0f, size, size), size * 0.3f, size * 0.3f, glow)
        if (still != null) {
            val inset = size * 0.07f
            draw(canvas, still, RectF(inset, inset, size - inset, size - inset), k, grey)
        }
        if (muted) badge(canvas, size)
        return bitmap
    }

    fun faces(others: List<Pair<CallCard.Still, Bitmap?>?>): Bitmap {
        val width = FACE + (others.size - 1) * (FACE - FACE_OVERLAP)
        val (bitmap, canvas) = canvas(width + 2 * FACES_PAD_X, FACE + 2 * FACES_PAD_Y)
        canvas.translate(px(FACES_PAD_X), px(FACES_PAD_Y))
        others.forEachIndexed { index, other ->
            val left = px(index * (FACE - FACE_OVERLAP))
            val image = other?.second ?: return@forEachIndexed
            draw(canvas, image, RectF(left, 0f, left + px(FACE), px(FACE)), other.first.k, grey = false)
        }
        return bitmap
    }

    fun round(still: Bitmap?): Bitmap {
        val (bitmap, canvas) = canvas(ROUND, ROUND)
        val size = px(ROUND)
        if (still == null) return bitmap
        canvas.clipPath(Path().apply { addOval(RectF(0f, 0f, size, size), Path.Direction.CW) })
        val scaled = size * 1.18f
        val left = (size - scaled) / 2
        canvas.drawBitmap(still, null, RectF(left, left + scaled * 0.05f, left + scaled, left + scaled * 1.05f), Paint(Paint.FILTER_BITMAP_FLAG or Paint.ANTI_ALIAS_FLAG))
        return bitmap
    }

    /** [still] in [slot], enlarged by [k] from the point at 50 % across and 96 % down (the gallery's transform-origin). */
    private fun draw(canvas: Canvas, still: Bitmap, slot: RectF, k: Double, grey: Boolean) {
        val scale = k.toFloat()
        val originX = slot.centerX()
        val originY = slot.top + slot.height() * 0.96f
        val dest = RectF(
            originX - (originX - slot.left) * scale,
            originY - (originY - slot.top) * scale,
            originX + (slot.right - originX) * scale,
            originY + (slot.bottom - originY) * scale,
        )
        val paint = Paint(Paint.FILTER_BITMAP_FLAG or Paint.ANTI_ALIAS_FLAG)
        if (grey) {
            // The gallery's offline still: grayscale(1) brightness(.85), at 70 %.
            val matrix = ColorMatrix().apply { setSaturation(0f) }
            matrix.postConcat(ColorMatrix().apply { setScale(0.85f, 0.85f, 0.85f, 1f) })
            paint.colorFilter = ColorMatrixColorFilter(matrix)
            paint.alpha = (0.7f * 255).roundToInt()
        }
        canvas.drawBitmap(still, null, dest, paint)
    }

    private fun badge(canvas: Canvas, face: Float) {
        val radius = px(BADGE) / 2
        val cx = face + px(BADGE_OUT) - radius
        val cy = cx
        val paint = Paint(Paint.ANTI_ALIAS_FLAG)
        paint.color = context.getColor(R.color.call_card_badge_ring)
        canvas.drawCircle(cx, cy, radius + px(BADGE_RING), paint)
        paint.color = context.getColor(R.color.call_card_mute_badge)
        canvas.drawCircle(cx, cy, radius, paint)
        val glyph = context.getDrawable(R.drawable.ic_card_mic_off_white) ?: return
        val half = px(BADGE_GLYPH) / 2
        glyph.setBounds((cx - half).roundToInt(), (cy - half).roundToInt(), (cx + half).roundToInt(), (cy + half).roundToInt())
        glyph.draw(canvas)
    }

    private fun canvas(widthDp: Int, heightDp: Int): Pair<Bitmap, Canvas> {
        val bitmap = Bitmap.createBitmap(px(widthDp).roundToInt(), px(heightDp).roundToInt(), Bitmap.Config.ARGB_8888)
        // Drawn at the screen's density, so a view that wraps it takes it at its size in dp.
        bitmap.density = context.resources.displayMetrics.densityDpi
        return bitmap to Canvas(bitmap)
    }

    private fun px(dp: Int): Float = dp * density
    private fun px(dp: Float): Float = dp * density

    private fun alpha(color: Int, alpha: Float): Int = ((alpha * 255).roundToInt() shl 24) or (color and 0xFFFFFF)

    companion object {
        /** The tile (dp), and the room drawn around it: the layout's negative margins take it back. */
        const val TILE = 52
        const val TILE_PAD_X = 7
        const val TILE_PAD_Y = 10
        /** The tile's top in its row: the row is as tall as the three lines (66 dp), the design's tile sits 4.3 dp down. */
        const val TILE_TOP = 4f
        const val BADGE = 22
        const val BADGE_RING = 2
        const val BADGE_OUT = 4
        const val BADGE_GLYPH = 11
        const val FACE = 26
        const val FACE_OVERLAP = 8
        const val FACES_PAD_X = 6
        const val FACES_PAD_Y = 12
        const val ROUND = 24
    }
}
