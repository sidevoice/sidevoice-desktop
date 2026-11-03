package ai.sidevoice.mobile.plugin

import android.media.AudioDeviceInfo

/**
 * The phone's outputs, told by their type. Android answers the phone's model for the product name of every built-in device
 * (`22101316G`), which the output picker listed three times; the app words the kinds (Rust `output_names`), in the page's
 * language, and an external device (a headset, USB, Bluetooth) names itself.
 */
internal object AudioOutputs {
    /** What an output is to the picker; [wire] is how the app reads it (Rust `OutputKind`). */
    enum class Kind(val wire: String) {
        EARPIECE("earpiece"),
        SPEAKER("speaker"),
        /** A built-in output the picker does not offer. */
        HIDDEN("hidden"),
        EXTERNAL("external"),
    }

    /**
     * The kind of each output of [outputs] (id, `AudioDeviceInfo` type), in their order. No kind is given twice: the
     * loudspeaker is `TYPE_BUILTIN_SPEAKER`, and `TYPE_BUILTIN_SPEAKER_SAFE` (the same speaker through the safe path) is
     * left out beside it, or is the loudspeaker where the phone has no other; the telephony path (it plays nothing) and the
     * platform's bus are left out.
     */
    fun kinds(outputs: List<Pair<Int, Int>>): List<Pair<Int, Kind>> {
        val taken = mutableSetOf<Kind>()
        val byId = mutableMapOf<Int, Kind>()
        // The plain loudspeaker and the earpiece first, so that the safe path only stands in for a missing one.
        val first = outputs.sortedBy { (_, type) -> if (type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER_SAFE) 1 else 0 }
        for ((id, type) in first) {
            val kind = when (type) {
                AudioDeviceInfo.TYPE_BUILTIN_EARPIECE -> Kind.EARPIECE
                AudioDeviceInfo.TYPE_BUILTIN_SPEAKER, AudioDeviceInfo.TYPE_BUILTIN_SPEAKER_SAFE -> Kind.SPEAKER
                AudioDeviceInfo.TYPE_TELEPHONY, AudioDeviceInfo.TYPE_BUS -> Kind.HIDDEN
                else -> Kind.EXTERNAL
            }
            byId[id] = if (kind == Kind.EARPIECE || kind == Kind.SPEAKER) {
                if (taken.add(kind)) kind else Kind.HIDDEN
            } else {
                kind
            }
        }
        return outputs.map { (id, _) -> id to byId.getValue(id) }
    }
}
