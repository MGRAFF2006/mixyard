package org.mixyard.mobile
import kotlin.test.*
class EndpointTest {
    @Test fun exactBytesAndSiblingRevocation() {
        val format = RawFormat(Sample.F64LE, 44117, listOf("right", "left"))
        val send = PcmQueue(format, 1, 4); val receive = PcmQueue(format, 2, 4)
        // Arbitrary IEEE payload bytes including low bits are opaque, never decoded as Float32.
        val input = ByteArray(32) { (it * 7 + 1).toByte() }
        send.push(1, 0, 0, 100, 1, input); receive.push(2, 0, 0, 100, 1, input)
        send.invalidate()
        val silent = ByteArray(32) { 42 }; assertEquals(0, send.pullInto(silent, 2)); assertTrue(silent.all { it == 0.toByte() })
        val out = ByteArray(32); assertEquals(2, receive.pullInto(out, 2)); assertContentEquals(input, out)
    }
    @Test fun allPrecisionsAndOrderedChannels() {
        for (sample in Sample.entries) {
            val format = RawFormat(sample, 768001, listOf("c3", "c1", "c2"))
            val q = PcmQueue(format, 3, 2); val bytes = ByteArray(format.frameBytes * 2) { it.toByte() }
            q.push(3, 0, 0, 9, 0, bytes); val out = ByteArray(bytes.size); q.pullInto(out, 2); assertContentEquals(bytes, out)
        }
        assertFails { RawFormat(Sample.S24LE, 48000, listOf("L", "L")) }
    }
    @Test fun overflowReplayExpiryAreTerminal() {
        for (mode in listOf("full", "replay", "expiry", "generation", "partial")) {
            val q = PcmQueue(RawFormat(Sample.S16LE, 96000, listOf("mono")), 1, 1)
            q.push(1, 0, 0, 20, 0, byteArrayOf(1, 2))
            assertFails { q.push(if (mode == "generation") 2 else 1, if (mode == "replay") 0 else 1, 1,
                if (mode == "expiry") 1 else 20, 1, if (mode == "partial") byteArrayOf(1) else byteArrayOf(3, 4)) }
            val out = byteArrayOf(9, 9); assertFalse(q.active); q.pullInto(out, 2); assertContentEquals(byteArrayOf(0, 0), out)
        }
    }
    @Test fun staleCompletionsCannotChangeTarget() {
        val fence = SessionFence(); val pending = fence.ticket(); fence.invalidate()
        assertFails { fence.establish(pending, Authority("B", "a", 0)) }
        fence.establish(fence.ticket(), Authority("A", "a", 1)); val write = fence.ticket()
        assertFails { fence.accept(write, Authority("B", "a", 2)) }
        assertFails { Authority("A", "a", 9_007_199_254_740_992L) }
    }
    @Test fun queuedBytesExpireBeforeConsumption() {
        val q = PcmQueue(RawFormat(Sample.S16LE, 44117, listOf("mono")), 1, 1)
        q.push(1, 0, 0, 3, 0, byteArrayOf(1, 2))
        val out = byteArrayOf(9, 9); assertEquals(0, q.pullInto(out, 3))
        assertFalse(q.active); assertContentEquals(byteArrayOf(0, 0), out)
    }
    @Test fun mixedReturnCannotBeRemovedAfterSumming() {
        assertFails { Provenance(listOf("mic", "call-return"), setOf("call"), listOf("desktop")).admit("phone", setOf("call")) }
        assertFails { Provenance(listOf("mic"), emptySet(), listOf("phone", "desktop")).admit("phone", emptySet()) }
        Provenance(listOf("mic"), emptySet(), listOf("desktop")).admit("phone", setOf("call"))
        assertFails { NativeEndpoint().start(AudioRole.MICROPHONE) }
    }
}
