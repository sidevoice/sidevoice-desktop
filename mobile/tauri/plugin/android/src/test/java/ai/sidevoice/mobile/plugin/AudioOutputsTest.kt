package ai.sidevoice.mobile.plugin

import android.media.AudioDeviceInfo
import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AudioOutputsTest {
    @Test
    fun theBuiltInOutputsAreNamedByType() {
        assertEquals(AudioOutputs.Kind.EARPIECE, AudioOutputs.kind(AudioDeviceInfo.TYPE_BUILTIN_EARPIECE))
        assertEquals(AudioOutputs.Kind.SPEAKER, AudioOutputs.kind(AudioDeviceInfo.TYPE_BUILTIN_SPEAKER))
        assertEquals(AudioOutputs.Kind.SPEAKER, AudioOutputs.kind(AudioDeviceInfo.TYPE_BUILTIN_SPEAKER_SAFE))
        assertEquals(AudioOutputs.Kind.BUILT_IN, AudioOutputs.kind(AudioDeviceInfo.TYPE_TELEPHONY))
        assertEquals(AudioOutputs.Kind.BUILT_IN, AudioOutputs.kind(AudioDeviceInfo.TYPE_BUS))
    }

    @Test
    fun anExternalOutputKeepsItsOwnName() {
        for (type in listOf(
            AudioDeviceInfo.TYPE_WIRED_HEADSET,
            AudioDeviceInfo.TYPE_WIRED_HEADPHONES,
            AudioDeviceInfo.TYPE_USB_HEADSET,
            AudioDeviceInfo.TYPE_USB_DEVICE,
            AudioDeviceInfo.TYPE_BLUETOOTH_A2DP,
            AudioDeviceInfo.TYPE_BLUETOOTH_SCO,
            AudioDeviceInfo.TYPE_BLE_HEADSET,
            AudioDeviceInfo.TYPE_HDMI,
        )) {
            assertNull("type $type", AudioOutputs.kind(type))
        }
    }

    @Test
    fun theNamesAreByIdAndOnlyOfTheBuiltInOnes() {
        val outputs = listOf(
            2 to AudioDeviceInfo.TYPE_BUILTIN_EARPIECE,
            3 to AudioDeviceInfo.TYPE_BUILTIN_SPEAKER,
            4 to AudioDeviceInfo.TYPE_TELEPHONY,
            9 to AudioDeviceInfo.TYPE_BLUETOOTH_A2DP,
        )
        assertEquals(
            listOf(2 to "EARPIECE", 3 to "SPEAKER", 4 to "BUILT_IN"),
            AudioOutputs.names(outputs) { it.name },
        )
    }

    @Test
    fun everyLanguageNamesTheOutputs() {
        val res = File("src/main/res")
        val folders = res.listFiles { file -> file.name.startsWith("values") && File(file, "strings.xml").exists() }!!
        assertTrue("the languages of the app", folders.size >= 8)
        for (folder in folders) {
            val strings = File(folder, "strings.xml").readText()
            for (name in listOf("output_earpiece", "output_built_in", "call_route_speaker")) {
                assertTrue("${folder.name}: $name", Regex("""<string name="$name">[^<]+</string>""").containsMatchIn(strings))
            }
        }
    }
}
