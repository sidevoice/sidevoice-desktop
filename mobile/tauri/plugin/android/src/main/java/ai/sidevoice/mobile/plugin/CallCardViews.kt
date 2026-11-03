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
import android.graphics.Typeface
import android.os.SystemClock
import android.text.SpannableStringBuilder
import android.text.Spanned
import android.text.style.ForegroundColorSpan
import android.text.style.StyleSpan
import android.view.View
import android.widget.RemoteViews
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
    /** Where the audio goes now: the headphones (named [name]), or the loudspeaker. */
    data class Route(val headphones: Boolean, val name: String)

    /** What each button does. [mute] sets the mute the button shows ("Silenciar" mutes, "Activar micrófono" unmutes). */
    data class Intents(
        val mute: PendingIntent?,
        val skip: PendingIntent?,
        val hangUp: PendingIntent?,
        val route: PendingIntent?,
        val conversations: PendingIntent?,
    )

    private val art = CallCardArt(context)

    /** The expanded card: tile, three lines, the audio switch; mute, skip, the stack and hang up. */
    fun expanded(): RemoteViews {
        val views = RemoteViews(context.packageName, R.layout.call_card)
        val tone = tone()
        views.setImageViewBitmap(R.id.call_card_tile, art.tile(card.still?.let(stills), card.still?.k ?: 1.0, tone.fill, grey = card.phase == CallCard.Phase.OFFLINE, muted = card.muted))
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
            views.setImageViewResource(R.id.call_card_route_icon, if (route.headphones) R.drawable.ic_card_headphones else R.drawable.ic_card_speaker)
            views.setContentDescription(R.id.call_card_route, card.words.route.replace("{route}", route.name).ifEmpty { route.name })
            intents.route?.let { views.setOnClickPendingIntent(R.id.call_card_route, it) }
        } else {
            views.setViewVisibility(R.id.call_card_route, View.GONE)
        }

        mute(views)
        views.setTextViewText(R.id.call_card_mute_label, muteLabel())
        views.setTextColor(R.id.call_card_mute_label, color(if (card.muted) R.color.call_card_muted_ink else R.color.call_card_text))

        // Skip is live only while the agent speaks; otherwise drawn at 32 % and inert.
        views.setFloat(R.id.call_card_skip, "setAlpha", if (card.canSkip) 1f else DISABLED_ALPHA)
        views.setBoolean(R.id.call_card_skip, "setEnabled", card.canSkip)
        views.setContentDescription(R.id.call_card_skip, card.words.skipName)
        intents.skip?.let { views.setOnClickPendingIntent(R.id.call_card_skip, it) }

        val faces = card.others.take(3)
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
        val line = SpannableStringBuilder()
        line.append(title(), StyleSpan(Typeface.BOLD), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        if (card.words.state.isNotEmpty()) {
            line.append(" · ")
            val start = line.length
            line.append(card.words.state)
            line.setSpan(StyleSpan(Typeface.BOLD), start, line.length, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
            line.setSpan(ForegroundColorSpan(tone().ink), start, line.length, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        }
        views.setTextViewText(R.id.call_card_line, line)
        mute(views)
        hangUp(views)
        return views
    }

    private fun mute(views: RemoteViews) {
        views.setInt(R.id.call_card_mute, "setBackgroundResource", if (card.muted) R.drawable.call_card_button_muted else R.drawable.call_card_button)
        views.setImageViewResource(R.id.call_card_mute_icon, if (card.muted) R.drawable.ic_card_mic_off else R.drawable.ic_card_mic)
        views.setContentDescription(R.id.call_card_mute, muteLabel())
        intents.mute?.let { views.setOnClickPendingIntent(R.id.call_card_mute, it) }
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
        CallCard.Tone.LILA -> Ink(color(R.color.call_card_lila_ink), color(R.color.call_card_lila))
        CallCard.Tone.GREEN -> Ink(color(R.color.call_card_green_ink), color(R.color.call_card_green))
        CallCard.Tone.WARN -> Ink(color(R.color.call_card_warn_ink), color(R.color.call_card_warn))
        CallCard.Tone.GREY -> Ink(color(R.color.call_card_grey_ink), color(R.color.call_card_grey))
    }

    private data class Ink(val ink: Int, val fill: Int)

    private fun color(id: Int): Int = context.getColor(id)

    private fun withAlpha(color: Int, alpha: Float): Int = ((alpha * 255).roundToInt() shl 24) or (color and 0xFFFFFF)

    private companion object {
        const val DISABLED_ALPHA = 0.32f
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
        paint.color = context.getColor(R.color.call_card_panel)
        canvas.drawCircle(cx, cy, radius + px(BADGE_RING), paint)
        paint.color = context.getColor(R.color.call_card_end)
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
        const val BADGE_GLYPH = 12
        const val FACE = 26
        const val FACE_OVERLAP = 8
        const val FACES_PAD_X = 6
        const val FACES_PAD_Y = 12
        const val ROUND = 24
    }
}
