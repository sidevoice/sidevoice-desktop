package ai.sidevoice.mobile.plugin

import android.media.AudioDeviceInfo
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class AudioOutputsTest {
    private fun kinds(vararg outputs: Pair<Int, Int>) = AudioOutputs.kinds(outputs.toList()).map { it.second }

    @Test
    fun theEarpieceAndTheLoudspeakerAreToldByType() {
        assertEquals(
            listOf(AudioOutputs.Kind.EARPIECE, AudioOutputs.Kind.SPEAKER),
            kinds(2 to AudioDeviceInfo.TYPE_BUILTIN_EARPIECE, 3 to AudioDeviceInfo.TYPE_BUILTIN_SPEAKER),
        )
    }

    @Test
    fun theSafeLoudspeakerTelephonyAndBusAreLeftOutBesideTheLoudspeaker() {
        val outputs = kinds(
            2 to AudioDeviceInfo.TYPE_BUILTIN_EARPIECE,
            3 to AudioDeviceInfo.TYPE_BUILTIN_SPEAKER,
            4 to AudioDeviceInfo.TYPE_BUILTIN_SPEAKER_SAFE,
            5 to AudioDeviceInfo.TYPE_TELEPHONY,
            6 to AudioDeviceInfo.TYPE_BUS,
        )
        assertEquals(
            listOf(
                AudioOutputs.Kind.EARPIECE, AudioOutputs.Kind.SPEAKER,
                AudioOutputs.Kind.HIDDEN, AudioOutputs.Kind.HIDDEN, AudioOutputs.Kind.HIDDEN,
            ),
            outputs,
        )
    }

    @Test
    fun theSafeLoudspeakerStandsInWhereThePhoneHasNoOther() {
        assertEquals(listOf(AudioOutputs.Kind.SPEAKER), kinds(4 to AudioDeviceInfo.TYPE_BUILTIN_SPEAKER_SAFE))
        // Whichever comes first in the list, the plain one is the loudspeaker.
        assertEquals(
            listOf(AudioOutputs.Kind.HIDDEN, AudioOutputs.Kind.SPEAKER),
            kinds(4 to AudioDeviceInfo.TYPE_BUILTIN_SPEAKER_SAFE, 3 to AudioDeviceInfo.TYPE_BUILTIN_SPEAKER),
        )
    }

    @Test
    fun noKindIsGivenTwiceAndAnExternalOutputKeepsItsOwnName() {
        val outputs = kinds(
            3 to AudioDeviceInfo.TYPE_BUILTIN_SPEAKER,
            7 to AudioDeviceInfo.TYPE_BUILTIN_SPEAKER,
            9 to AudioDeviceInfo.TYPE_BLUETOOTH_A2DP,
            10 to AudioDeviceInfo.TYPE_WIRED_HEADSET,
            11 to AudioDeviceInfo.TYPE_USB_HEADSET,
        )
        assertEquals(
            listOf(
                AudioOutputs.Kind.SPEAKER, AudioOutputs.Kind.HIDDEN,
                AudioOutputs.Kind.EXTERNAL, AudioOutputs.Kind.EXTERNAL, AudioOutputs.Kind.EXTERNAL,
            ),
            outputs,
        )
    }

    @Test
    fun theAnswerIsInTheOrderOfTheOutputsAndByTheirIds() {
        val answered = AudioOutputs.kinds(listOf(9 to AudioDeviceInfo.TYPE_BLUETOOTH_A2DP, 3 to AudioDeviceInfo.TYPE_BUILTIN_SPEAKER))
        assertEquals(listOf(9, 3), answered.map { it.first })
        assertTrue(AudioOutputs.kinds(emptyList()).isEmpty())
    }
}
