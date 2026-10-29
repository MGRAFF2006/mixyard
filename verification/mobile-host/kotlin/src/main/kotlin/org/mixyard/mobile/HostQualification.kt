package org.mixyard.mobile

import java.io.File
import org.json.JSONObject
import org.mixyard.mobile.host.NativeHostWorker

/** Owned offline binary-buffer probe of the actual JNI -> Rust policy/DSP path. */
fun main(args: Array<String>) {
    require(args.size == 1)
    val admitted = File(args[0]).readText()
    val request = JSONObject(admitted)
    // Exercise canonical mix-minus admission even with an explicit return send.
    request.getJSONObject("config").getJSONArray("sends").put(JSONObject().put("source", "return").put("mix", "call").put("gain", 1.0))
    request.getJSONObject("limits").put("maxSends", 4)
    fun ports(name: String): List<NativeHostWorker.Port> {
        val values = request.getJSONArray(name)
        return (0 until values.length()).map { index ->
            val value = values.getJSONObject(index)
            fun strings(field: String): List<String> {
                val array = value.getJSONArray(field)
                return (0 until array.length()).map(array::getString)
            }
            NativeHostWorker.Port(value.getString("observedId"), RawFormat(Sample.F64LE, value.getLong("sampleRateHz"), strings("channels")), strings("mapping"))
        }
    }
    val sources = ports("sources"); val outputs = ports("outputs")
    fun seed(worker: NativeHostWorker) {
        for ((index, level) in listOf(999.0, 4.0, 2.0).withIndex()) {
            val buffer = worker.input(index)
            for (sample in 0 until 8) buffer.putDouble(sample * 8, level)
        }
    }
    var rejectedMixMinus = false
    try { NativeHostWorker(request.toString(), 4, sources, outputs).close() }
    catch (_: IllegalStateException) { rejectedMixMinus = true }
    check(rejectedMixMinus) { "excluded-call-return-admitted" }
    NativeHostWorker(admitted, 4, sources, outputs).use { worker ->
        check(!worker.backendAvailable && !worker.audioApplied)
        seed(worker)
        val stamp = NativeHostWorker.Stamp(7, 1, 11, 0, 0, 100, 1000)
        worker.process(200, stamp, List(sources.size) { stamp })
        for ((index, expected) in listOf(2.0, 1.0).withIndex()) {
            val buffer = worker.output(index)
            for (sample in 0 until 8) check(buffer.getDouble(sample * 8) == expected) { "canonical-policy-or-destination-DSP-regression" }
        }
        var rejected = false
        val foreign = Thread { try { worker.output(0) } catch (_: IllegalStateException) { rejected = true } }
        foreign.start(); foreign.join(); check(rejected) { "cross-thread-worker-access" }
        val stale = NativeHostWorker.Stamp(8, 1, 11, 1, 4, 300, 1000)
        try { worker.process(400, stale, List(sources.size) { stale }); error("stale-binding-accepted") }
        catch (failure: IllegalStateException) { check(failure.message != "stale-binding-accepted") }
        for (index in outputs.indices) {
            val buffer = worker.output(index)
            for (byte in 0 until buffer.capacity()) check(buffer.get(byte) == 0.toByte()) { "revoked-PCM-retained" }
        }
        for (index in sources.indices) {
            val buffer = worker.input(index)
            for (byte in 0 until buffer.capacity()) check(buffer.get(byte) == 0.toByte()) { "revoked-input-retained" }
        }
    }
    val failedOutput = JSONObject(admitted)
    failedOutput.getJSONArray("outputDsp").getJSONObject(1).getJSONArray("effects").getJSONObject(0).getJSONObject("effect").put("gain", Double.MAX_VALUE)
    NativeHostWorker(failedOutput.toString(), 4, sources, outputs).use { worker ->
        seed(worker)
        for (sequence in 0L..1L) {
            val captured = if (sequence == 0L) 100L else 83_434L
            val stamp = NativeHostWorker.Stamp(7, 1, 11, sequence, sequence * 4, captured, 1_000_000)
            worker.process(captured + 100, stamp, List(sources.size) { stamp })
            for (index in outputs.indices) {
                check(worker.outputStatus(index) == if (index == 0) 0 else 7) { "output-failure-not-isolated" }
                val buffer = worker.output(index)
                for (sample in 0 until 8) check(buffer.getDouble(sample * 8) == if (index == 0) 2.0 else 0.0)
            }
        }
    }
    println("PASS: actual Kotlin/JNI/canonical Rust offline policy, mix-minus, independent DSP/output failure, worker fence and revoke scrubbing; no native audio qualification")
}
