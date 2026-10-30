package org.mixyard.mobile
import java.io.InputStream

/** Explicit raw little-endian files only. Compressed/encoded files require a separately admitted decoder. */
class RawFileSource(private val input: InputStream, val format: RawFormat) : AutoCloseable {
    fun readFrames(maxFrames: Int): ByteArray {
        require(maxFrames > 0)
        val buffer = ByteArray(Math.multiplyExact(maxFrames, format.frameBytes))
        var length = 0
        try {
            while (length < buffer.size) {
                val size = input.read(buffer, length, buffer.size - length)
                if (size < 0) break
                check(size > 0) { "file-read-no-progress" }
                length += size
            }
            check(length % format.frameBytes == 0) { "partial-file-frame-no-padding" }
            return buffer.copyOf(length)
        } catch (failure: Exception) { close(); throw failure }
    }
    override fun close() = input.close()
}
/** A generated exact raw frame, useful for deterministic no-device loopback. No rate/precision conversion. */
class GeneratedSource(val format: RawFormat, frame: ByteArray, private var remainingFrames: Long) {
    private val rawFrame = frame.copyOf()
    init { require(frame.size == format.frameBytes && remainingFrames >= 0) }
    fun readFrames(maxFrames: Int): ByteArray {
        require(maxFrames > 0)
        val count = minOf(remainingFrames, maxFrames.toLong()).toInt()
        val result = ByteArray(Math.multiplyExact(count, format.frameBytes))
        for (i in 0 until count) rawFrame.copyInto(result, i * format.frameBytes)
        remainingFrames -= count
        return result
    }
}
