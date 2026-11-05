package ai.sidevoice.mobile.plugin

import org.json.JSONObject

/**
 * What the call's notification draws: the conversation's title and the page's mute ([CallService.ACTION_UPDATE]), and the
 * call surface the app sends with them (Rust `CallSurface`, docs/MOBILE.md "The call surface"). Every field has a default,
 * so a page that sends less, or nothing yet, draws an idle call with no words.
 *
 * The words are the page's, already in its language; the card never words the call's states itself.
 */
internal data class CallCard(
    val title: String = "",
    val muted: Boolean = false,
    val phase: Phase = Phase.IDLE,
    val canSkip: Boolean = false,
    /** Whether the person's turn can still be withdrawn (Cancelar envío). */
    val canCancelSend: Boolean = false,
    val model: String = "",
    val machine: String = "",
    /** When the working phase began (ms since the epoch); null when the page does not know it: no time is drawn then. */
    val workingSince: Long? = null,
    val still: Still? = null,
    /** Up to three other conversations' creatures, for the stack. */
    val others: List<Still?> = emptyList(),
    /** The number the stack shows: every conversation of the call. */
    val conversationCount: Int = 0,
    val words: Words = Words(),
) {
    /** What the call is doing, as the page's surface names it. */
    enum class Phase(val wire: String) {
        IDLE("idle"), LISTENING("listening"), TRANSCRIBING("transcribing"), SAID("said"), WORKING("working"),
        SPEAKING("speaking"), HOLDING("holding"), OFFLINE("offline");

        /**
         * The phase's tone, scheme E (the operator's, 2026-11-05): listening, transcribing and your words green, speaking
         * blue, working violet, can't receive amber, idle and offline greys.
         */
        val tone: Tone
            get() = when (this) {
                LISTENING, TRANSCRIBING, SAID -> Tone.LISTEN
                SPEAKING -> Tone.SPEAK
                WORKING -> Tone.WORK
                HOLDING -> Tone.WARN
                IDLE -> Tone.IDLE
                OFFLINE -> Tone.OFFLINE
            }

        companion object {
            fun of(wire: String?): Phase = values().firstOrNull { it.wire == wire } ?: IDLE
        }
    }

    enum class Tone { LISTEN, SPEAK, WORK, WARN, IDLE, OFFLINE }

    /** A creature's still: its PNG, [file] in the stills folder, and how much of it the body takes ([k], 1 by default). */
    data class Still(val file: String, val k: Double = 1.0)

    /** The words the card draws, in the page's language; empty where the page has not said them. */
    data class Words(
        val state: String = "",
        val mute: String = "",
        val unmute: String = "",
        /** Skip's short label and its accessible name; cancel send's label (its accessible name too). */
        val skip: String = "",
        val skipName: String = "",
        val cancelSend: String = "",
        val hangUp: String = "",
        /** The audio switch's accessible name, `{route}` in it for the output's name. */
        val route: String = "",
        /** The loudspeaker's name, for the audio switch. */
        val speaker: String = "",
        /** The stack's accessible name, its count in it. */
        val conversations: String = "",
    )

    /** The line under the title: "model · machine", or whichever of them the page said. */
    val sub: String get() = listOf(model, machine).filter { it.isNotBlank() }.joinToString(" · ")

    /** Whether the working time shows: only while working, and only when the page knows since when. */
    val timed: Boolean get() = phase == Phase.WORKING && workingSince != null

    /** The action on the state line, by the state (the operator's option B, 2026-11-05): skip while the agent speaks,
     *  cancel send during the person's turn, none otherwise. */
    enum class Action { SKIP, CANCEL_SEND }

    val action: Action?
        get() = when {
            phase == Phase.SPEAKING && canSkip -> Action.SKIP
            (phase == Phase.LISTENING || phase == Phase.TRANSCRIBING) && canCancelSend -> Action.CANCEL_SEND
            else -> null
        }

    /** The count the stack shows: the page's, or at least this conversation and the others it drew. */
    val count: Int get() = maxOf(conversationCount, others.size + 1)

    companion object {
        /** The card for [title] and [muted], with the surface [surface] (the JSON the app sends; null or broken: none). */
        fun of(title: String, muted: Boolean, surface: String?): CallCard {
            val json = surface?.let { runCatching { JSONObject(it) }.getOrNull() } ?: JSONObject()
            val words = json.optJSONObject("words") ?: JSONObject()
            val others = json.optJSONArray("others")
            return CallCard(
                title = title,
                muted = muted,
                phase = Phase.of(json.optString("phase", "")),
                canSkip = json.optBoolean("canSkip", false),
                canCancelSend = json.optBoolean("canCancelSend", false),
                model = json.optString("model", ""),
                machine = json.optString("machine", ""),
                workingSince = if (json.isNull("workingSince")) null else json.optDouble("workingSince").takeIf { !it.isNaN() }?.toLong(),
                still = still(json.optJSONObject("still")),
                others = (0 until (others?.length() ?: 0)).map { still(others!!.optJSONObject(it)?.optJSONObject("still")) }.take(3),
                conversationCount = json.optInt("conversationCount", 0),
                words = Words(
                    state = words.optString("state", ""),
                    mute = words.optString("mute", ""),
                    unmute = words.optString("unmute", ""),
                    skip = words.optString("skip", ""),
                    skipName = words.optString("skipName", ""),
                    cancelSend = words.optString("cancelSend", ""),
                    hangUp = words.optString("hangUp", ""),
                    route = words.optString("route", ""),
                    speaker = words.optString("speaker", ""),
                    conversations = words.optString("conversations", ""),
                ),
            )
        }

        private fun still(json: JSONObject?): Still? {
            val file = json?.optString("file", "").orEmpty()
            if (file.isEmpty()) return null
            val k = json!!.optDouble("k", 1.0)
            return Still(file, if (k.isNaN() || k <= 0) 1.0 else k)
        }
    }
}
