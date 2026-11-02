package org.mixyard.mobile

/** Worker-owned raw bytes, not a WebView DTO or a realtime callback queue. */
enum class Sample(val bytes: Int) { S16LE(2), S24LE(3), S32LE(4), F32LE(4), F64LE(8) }
class RawFormat(val sample: Sample, val rate: Long, channels: List<String>) {
    val channels: List<String> = channels.toList()
    init {
        require(rate in 1..0xffff_ffffL) { "unsupported-rate" }
        require(channels.isNotEmpty() && channels.all { it.isNotBlank() } && channels.toSet().size == channels.size) { "invalid-channel-order" }
        Math.multiplyExact(sample.bytes, channels.size)
    }
    val frameBytes: Int get() = sample.bytes * channels.size
}
data class Authority(val deviceId: String, val authorityId: String, val revision: Long) {
    init { require(deviceId.isNotBlank() && authorityId.isNotBlank()); require(revision in 0..9_007_199_254_740_991L) { "unsafe-revision" } }
}
data class Ticket(val epoch: Long, val authority: Authority?)
class SessionFence {
    private var epoch = 0L
    private var authority: Authority? = null
    @Synchronized fun invalidate() { epoch = Math.incrementExact(epoch); authority = null }
    @Synchronized fun ticket() = Ticket(epoch, authority)
    @Synchronized fun check(ticket: Ticket) { check(ticket == Ticket(epoch, authority)) { "session-changed-no-replay" } }
    @Synchronized fun establish(ticket: Ticket, next: Authority) { check(ticket); authority = next }
    @Synchronized fun accept(ticket: Ticket, next: Authority) {
        check(ticket)
        val before = ticket.authority ?: error("session-unavailable")
        check(next.deviceId == before.deviceId && next.authorityId == before.authorityId && next.revision >= before.revision) { "authority-changed" }
        authority = next
    }
}

/** Exact descriptor/provenance admission; summed exclusions must reject the whole premix. */
data class Provenance(val sources: List<String>, val groups: Set<String>, val visitedPeers: List<String>) {
    fun admit(localPeer: String, excludedGroups: Set<String>) {
        require(sources.isNotEmpty() && sources.all { it.isNotBlank() } && sources.toSet().size == sources.size) { "duplicate-origin" }
        require(visitedPeers.isNotEmpty() && visitedPeers.toSet().size == visitedPeers.size && localPeer !in visitedPeers) { "feedback-cycle" }
        require(groups.intersect(excludedGroups).isEmpty()) { "rebuild-upstream-mix-minus" }
    }
}

/** All operations have one exclusive worker owner. Terminal errors scrub queued bytes. */
class PcmQueue(val format: RawFormat, val generation: Long, capacityFrames: Int) {
    private val storage: ByteArray
    private val deadlines: LongArray
    private var read = 0
    private var count = 0
    private var nextSequence = 0L
    private var nextFrame = 0L
    private var lastNow = 0L
    var active = true; private set
    init {
        require(generation > 0 && capacityFrames > 0)
        storage = ByteArray(Math.multiplyExact(format.frameBytes, capacityFrames))
        deadlines = LongArray(capacityFrames)
    }
    fun invalidate() { storage.fill(0); deadlines.fill(0); read = 0; count = 0; active = false }
    fun push(generation: Long, sequence: Long, firstFrame: Long, deadlineNs: Long, nowNs: Long, bytes: ByteArray) {
        try {
            check(active) { "inactive" }
            require(generation == this.generation && sequence == nextSequence && firstFrame == nextFrame) { "stale-or-replayed-block" }
            require(nowNs >= 0 && nowNs >= lastNow && deadlineNs > nowNs) { "expired-or-invalid-clock" }
            require(bytes.isNotEmpty() && bytes.size % format.frameBytes == 0) { "partial-frame" }
            require(bytes.size <= storage.size - count) { "queue-full" }
            val next = Math.addExact(nextFrame, (bytes.size / format.frameBytes).toLong())
            val seq = Math.incrementExact(nextSequence)
            bytes.forEachIndexed { i, byte -> storage[(read + count + i) % storage.size] = byte }
            for (i in bytes.indices step format.frameBytes) deadlines[((read + count + i) % storage.size) / format.frameBytes] = deadlineNs
            count += bytes.size; nextFrame = next; nextSequence = seq; lastNow = nowNs
        } catch (failure: Exception) { invalidate(); throw failure }
    }
    fun pullInto(output: ByteArray, nowNs: Long): Int {
        output.fill(0)
        if (output.size % format.frameBytes != 0) { invalidate(); error("partial-frame") }
        if (!active) return 0
        val taken = minOf(count, output.size)
        if (nowNs < 0 || nowNs < lastNow || (0 until taken step format.frameBytes).any {
                deadlines[((read + it) % storage.size) / format.frameBytes] <= nowNs }) { invalidate(); return 0 }
        lastNow = nowNs
        for (i in 0 until taken) { output[i] = storage[read]; storage[read] = 0; read = (read + 1) % storage.size }
        count -= taken
        return taken / format.frameBytes
    }
}

enum class AudioRole { RECEIVE, MICROPHONE, EXTERNAL, FILE, GENERATED, DUPLEX, STANDALONE_DSP }
enum class StopReason { BACKGROUND, FOCUS_LOSS, INTERRUPTION, ROUTE_CHANGE, PERMISSION_REVOKED, PEER_REVOKED, NETWORK_CHANGE, RESTART }
class NativeEndpoint {
    private val queues = mutableListOf<PcmQueue>()
    val backendAvailable = false
    val audioApplied = false
    fun capabilities(): Map<AudioRole, String> = AudioRole.entries.associateWith { "shared-core-and-transport-not-linked" }
    fun start(role: AudioRole): Nothing = error("${role.name.lowercase()}: shared-core-and-transport-not-linked; explicit consent and exact route required")
    fun stop(reason: StopReason) { queues.forEach { it.invalidate() }; queues.clear() }
}
