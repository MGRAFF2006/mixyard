package org.mixyard.mobile
import java.io.ByteArrayInputStream
import kotlin.test.*
class RawSourcesTest {
    @Test fun generatedAndFileLoopbackPreserveIeeeLowBits() {
        val format = RawFormat(Sample.F64LE, 192000, listOf("mono"))
        // f64 1.0000000000000002 in little endian; float32 conversion loses the low bit.
        val frame = byteArrayOf(1, 0, 0, 0, 0, 0, 0xf0.toByte(), 0x3f)
        val source = GeneratedSource(format, frame, 3)
        val generated = source.readFrames(4)
        assertContentEquals(frame + frame + frame, generated)
        assertTrue(source.readFrames(4).isEmpty())
        RawFileSource(ByteArrayInputStream(generated), format).use { file ->
            val q = PcmQueue(format, 1, 4)
            q.push(1, 0, 0, 50, 0, file.readFrames(2))
            q.push(1, 1, 2, 50, 1, file.readFrames(2))
            val output = ByteArray(24); assertEquals(3, q.pullInto(output, 2)); assertContentEquals(generated, output)
        }
    }
    @Test fun fileTailIsNeverPaddedOrTruncated() {
        val source = RawFileSource(ByteArrayInputStream(byteArrayOf(1, 2, 3)), RawFormat(Sample.S16LE, 44117, listOf("mono")))
        assertFails { source.readFrames(4) }
    }
}
