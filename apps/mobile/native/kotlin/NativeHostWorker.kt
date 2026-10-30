package org.mixyard.mobile.host

import java.nio.ByteBuffer
import java.nio.ByteOrder
import org.json.JSONObject
import org.mixyard.mobile.RawFormat
import org.mixyard.mobile.Sample

/** An exclusive offline worker, never an audio callback or an audio authority.
 * The canonical Rust host owns policy, Processor64, DSP and retained PCM. */
class NativeHostWorker(
    requestJson: String,
    val frames: Int,
    sources: List<Port>,
    outputs: List<Port>,
) : AutoCloseable {
    data class Port(val observedId: String, val format: RawFormat, val mapping: List<String>)
    data class Stamp(val generation: Long, val revision: Long, val clockId: Long, val sequence: Long,
        val firstFrame: Long, val capturedNs: Long, val deadlineNs: Long)
    private val owner = Thread.currentThread().id
    val sources = sources.map { it.copy(mapping = it.mapping.toList()) }
    val outputs = outputs.map { it.copy(mapping = it.mapping.toList()) }
    private var handle = 0L
    private val inputs: List<ByteBuffer>
    private val rendered: List<ByteBuffer>
    private val views: List<ByteBuffer>
    private val stamps: ByteBuffer
    val backendAvailable = false
    val audioApplied = false

    init {
        require(ByteOrder.nativeOrder() == ByteOrder.LITTLE_ENDIAN) { "little-endian-host-required" }
        require(frames > 0 && sources.isNotEmpty() && outputs.isNotEmpty())
        // Snapshot admission descriptors: caller mutation cannot retarget native
        // ports. Mapping itself is canonical policy input, not inferred layout.
        (sources + outputs).forEach { require(it.observedId.isNotBlank() && it.mapping.size == it.format.channels.size) }
        require(requestJson.length <= 1_048_576) { "host-request-budget" }
        val encoded = requestJson.toByteArray(Charsets.UTF_8)
        require(encoded.size <= 1_048_576 && frames.toLong() <= JSONObject(requestJson).getLong("maxFrames")) { "host-frame-budget" }
        handle = create(encoded)
        check(handle != 0L) { "canonical-host-admission-rejected" }
        try {
            check(info(handle, false, sources.size) == null && info(handle, true, outputs.size) == null) { "exact-port-count-required" }
            sources.forEachIndexed { index, port -> verify(info(handle, false, index), port, false) }
            outputs.forEachIndexed { index, port ->
                require(port.format.sample == Sample.F64LE)
                verify(info(handle, true, index), port, true)
            }
            // Allocate only after Rust admission and exact port verification.
            inputs = sources.map { ByteBuffer.allocateDirect(Math.multiplyExact(frames, it.format.frameBytes)).order(ByteOrder.LITTLE_ENDIAN) }
            rendered = outputs.map { ByteBuffer.allocateDirect(Math.multiplyExact(frames, it.format.frameBytes)).order(ByteOrder.LITTLE_ENDIAN) }
            views = rendered.map { it.asReadOnlyBuffer().order(ByteOrder.LITTLE_ENDIAN) }
            stamps = ByteBuffer.allocateDirect(Math.multiplyExact(sources.size + 1, 56)).order(ByteOrder.LITTLE_ENDIAN)
            sources.forEachIndexed { index, port ->
                check(bindInput(handle, index, inputs[index], encoding(port.format.sample), port.format.rate.toInt(), port.format.channels.size) == 0)
            }
            outputs.forEachIndexed { index, port ->
                check(bindOutput(handle, index, rendered[index]) == 0)
            }
            check(bindStamps(handle, stamps) == 0)
        } catch (failure: Throwable) { revoke(handle); destroy(handle); handle = 0; throw failure }
    }
    private fun verify(json: String?, port: Port, output: Boolean) {
        val value = JSONObject(json ?: error("host-port-unavailable"))
        require(value.getString("observedId") == port.observedId && value.getLong("sampleRateHz") == port.format.rate)
        for ((name, expected) in listOf("channels" to port.format.channels, "mapping" to port.mapping)) {
            val actual = value.getJSONArray(name)
            require(actual.length() == expected.size && expected.indices.all { actual.getString(it) == expected[it] }) { "exact-channel-order-required" }
        }
        if (!output) require(value.getString("encoding") == when (port.format.sample) {
            Sample.S16LE -> "s16-le"; Sample.S24LE -> "s24-le"; Sample.S32LE -> "s32-le"; Sample.F32LE -> "f32-le"; Sample.F64LE -> "f64-le"
        })
    }
    private fun encoding(sample: Sample) = when (sample) {
        Sample.F32LE -> 1; Sample.F64LE -> 2; Sample.S16LE -> 3; Sample.S24LE -> 4; Sample.S32LE -> 5
    }
    private fun owned() { check(Thread.currentThread().id == owner && handle != 0L) { "exclusive-worker-required" } }
    fun input(index: Int): ByteBuffer { owned(); return inputs[index] }
    fun output(index: Int): ByteBuffer { owned(); return views[index].apply { clear() } }
    /** Last C ABI output result; not native device/permission/route availability. */
    fun outputStatus(index: Int): Int { owned(); return outputStatus(handle, index) }
    private fun stamp(index: Int, value: Stamp) {
        require(value.generation > 0 && value.revision >= 0 && value.clockId > 0 && value.sequence >= 0 && value.firstFrame >= 0 && value.capturedNs >= 0 && value.deadlineNs > value.capturedNs)
        val offset = index * 56
        stamps.putLong(offset, value.generation); stamps.putLong(offset + 8, value.revision)
        stamps.putLong(offset + 16, value.clockId); stamps.putLong(offset + 24, value.sequence)
        stamps.putLong(offset + 32, value.firstFrame); stamps.putLong(offset + 40, value.capturedNs)
        stamps.putLong(offset + 48, value.deadlineNs)
    }
    fun process(nowNs: Long, engine: Stamp, sourceStamps: List<Stamp>) {
        owned()
        try {
            require(nowNs >= 0 && sourceStamps.size == inputs.size)
            stamp(0, engine); sourceStamps.forEachIndexed { i, value -> stamp(i + 1, value) }
            val result = process(handle, nowNs, frames)
            check(result == 0) { "canonical-host-process-$result" }
        } catch (failure: Exception) { revoke(handle); throw failure }
    }
    fun revoke() { owned(); revoke(handle) }
    override fun close() { owned(); revoke(handle); destroy(handle); handle = 0 }

    private external fun create(request: ByteArray): Long
    private external fun info(handle: Long, output: Boolean, index: Int): String?
    private external fun bindInput(handle: Long, index: Int, bytes: ByteBuffer, encoding: Int, rate: Int, channels: Int): Int
    private external fun bindOutput(handle: Long, index: Int, bytes: ByteBuffer): Int
    private external fun bindStamps(handle: Long, stamps: ByteBuffer): Int
    private external fun process(handle: Long, nowNs: Long, frames: Int): Int
    private external fun outputStatus(handle: Long, index: Int): Int
    private external fun revoke(handle: Long)
    private external fun destroy(handle: Long)
    companion object { init { System.loadLibrary("mixyard_mobile_host") } }
}
