package ai.sidevoice.mobile.plugin

import android.media.AudioDeviceInfo
import androidx.annotation.StringRes

/**
 * The phone's built-in outputs, named by what they are. Android answers the phone's model for the product name of every
 * built-in device (`22101316G`), which the output picker listed three times (earpiece, loudspeaker, and the one the
 * voice's host does not type). An external device (a headset, USB, Bluetooth) names itself, so it is not here.
 */
internal object AudioOutputs {
    /** What a built-in output is, with the word that names it (in every language of the app, `res/values-*`). */
    enum class Kind(@StringRes val label: Int) {
        EARPIECE(R.string.output_earpiece),
        SPEAKER(R.string.call_route_speaker),
        /** The phone's other built-in outputs: the telephony path and the platform's bus output. */
        BUILT_IN(R.string.output_built_in),
    }

    /** The kind of the output of `AudioDeviceInfo` type [type]; null for an external one, which keeps its own name. */
    fun kind(type: Int): Kind? = when (type) {
        AudioDeviceInfo.TYPE_BUILTIN_EARPIECE -> Kind.EARPIECE
        AudioDeviceInfo.TYPE_BUILTIN_SPEAKER, AudioDeviceInfo.TYPE_BUILTIN_SPEAKER_SAFE -> Kind.SPEAKER
        AudioDeviceInfo.TYPE_TELEPHONY, AudioDeviceInfo.TYPE_BUS -> Kind.BUILT_IN
        else -> null
    }

    /** The built-in outputs among [outputs] (id, type), each by id with the word [label] gives its kind. */
    fun names(outputs: List<Pair<Int, Int>>, label: (Kind) -> String): List<Pair<Int, String>> =
        outputs.mapNotNull { (id, type) -> kind(type)?.let { id to label(it) } }
}
