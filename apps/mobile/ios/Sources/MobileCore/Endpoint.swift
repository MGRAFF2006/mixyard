import Foundation

public enum MobileError: Error { case unsupported(String), invalid(String) }
public enum Sample: Int, CaseIterable { case s16le = 2, s24le = 3, s32le = 4, f32le = 5, f64le = 8
    public var bytes: Int { self == .f32le ? 4 : rawValue }
}
public struct RawFormat: Equatable {
    public let sample: Sample
    public let rate: UInt32
    public let channels: [String]
    public var frameBytes: Int { sample.bytes * channels.count }
    public init(sample: Sample, rate: UInt32, channels: [String]) throws {
        guard rate > 0, !channels.isEmpty, channels.allSatisfy({ !$0.isEmpty }), Set(channels).count == channels.count,
              channels.count <= Int.max / sample.bytes else { throw MobileError.invalid("exact-format-required") }
        self.sample = sample; self.rate = rate; self.channels = channels
    }
}
public struct Authority: Equatable {
    public let deviceId: String
    public let authorityId: String
    public let revision: UInt64
    public init(deviceId: String, authorityId: String, revision: UInt64) throws {
        guard !deviceId.isEmpty, !authorityId.isEmpty, revision <= 9_007_199_254_740_991 else { throw MobileError.invalid("unsafe-authority") }
        self.deviceId = deviceId; self.authorityId = authorityId; self.revision = revision
    }
}
public struct Ticket: Equatable { public let epoch: UInt64; public let authority: Authority? }
public struct SessionFence {
    private var epoch: UInt64 = 0
    private var authority: Authority?
    public init() {}
    public var ticket: Ticket { Ticket(epoch: epoch, authority: authority) }
    public mutating func invalidate() { precondition(epoch < UInt64.max); epoch += 1; authority = nil }
    public func check(_ before: Ticket) throws { guard ticket == before else { throw MobileError.invalid("session-changed-no-replay") } }
    public mutating func establish(_ before: Ticket, _ next: Authority) throws { try check(before); authority = next }
    public mutating func accept(_ before: Ticket, _ next: Authority) throws {
        try check(before)
        guard let old = before.authority, old.deviceId == next.deviceId, old.authorityId == next.authorityId, next.revision >= old.revision else { throw MobileError.invalid("authority-changed") }
        authority = next
    }
}
public struct Provenance {
    public let sources: [String]; public let groups: Set<String>; public let visitedPeers: [String]
    public init(sources: [String], groups: Set<String>, visitedPeers: [String]) { self.sources = sources; self.groups = groups; self.visitedPeers = visitedPeers }
    public func admit(localPeer: String, excludedGroups: Set<String>) throws {
        guard !sources.isEmpty, sources.allSatisfy({ !$0.isEmpty }), Set(sources).count == sources.count,
              !visitedPeers.isEmpty, Set(visitedPeers).count == visitedPeers.count, !visitedPeers.contains(localPeer),
              groups.isDisjoint(with: excludedGroups) else { throw MobileError.invalid("rebuild-upstream-mix-minus-or-feedback-cycle") }
    }
}

// One exclusive worker owner. Array operations are not a qualified realtime callback boundary.
public final class PcmQueue {
    public let format: RawFormat; public let generation: UInt64
    private var deadlines: [UInt64]
    private var storage: [UInt8]; private var read = 0; private var count = 0
    private var sequence: UInt64 = 0; private var frame: UInt64 = 0; private var lastNow: UInt64 = 0
    public private(set) var active = true
    public init(format: RawFormat, generation: UInt64, capacityFrames: Int) throws {
        guard generation > 0, capacityFrames > 0, capacityFrames <= Int.max / format.frameBytes else { throw MobileError.invalid("queue-budget") }
        self.format = format; self.generation = generation
        self.storage = [UInt8](repeating: 0, count: capacityFrames * format.frameBytes)
        self.deadlines = [UInt64](repeating: 0, count: capacityFrames)
    }
    public func invalidate() { _ = storage.withUnsafeMutableBytes { $0.initializeMemory(as: UInt8.self, repeating: 0) }; deadlines = deadlines.map { _ in 0 }; read = 0; count = 0; active = false }
    public func push(generation: UInt64, sequence: UInt64, firstFrame: UInt64, deadlineNs: UInt64, nowNs: UInt64, bytes: [UInt8]) throws {
        do {
            guard active, generation == self.generation, sequence == self.sequence, firstFrame == frame,
                  nowNs >= lastNow, deadlineNs > nowNs, !bytes.isEmpty, bytes.count % format.frameBytes == 0,
                  bytes.count <= storage.count - count, self.sequence < UInt64.max else { throw MobileError.invalid("inactive-replayed-expired-full-or-partial") }
            let (next, overflow) = frame.addingReportingOverflow(UInt64(bytes.count / format.frameBytes))
            guard !overflow else { throw MobileError.invalid("frame-overflow") }
            for (i, byte) in bytes.enumerated() { storage[(read + count + i) % storage.count] = byte }
            for i in stride(from: 0, to: bytes.count, by: format.frameBytes) { deadlines[((read + count + i) % storage.count) / format.frameBytes] = deadlineNs }
            count += bytes.count; frame = next; self.sequence += 1; lastNow = nowNs
        } catch { invalidate(); throw error }
    }
    @discardableResult public func pull(into output: inout [UInt8], nowNs: UInt64) throws -> Int {
        _ = output.withUnsafeMutableBytes { $0.initializeMemory(as: UInt8.self, repeating: 0) }
        guard output.count % format.frameBytes == 0 else { invalidate(); throw MobileError.invalid("partial-frame") }
        guard active else { return 0 }
        let taken = min(count, output.count)
        if nowNs < lastNow || stride(from: 0, to: taken, by: format.frameBytes).contains(where: { deadlines[((read + $0) % storage.count) / format.frameBytes] <= nowNs }) { invalidate(); return 0 }
        lastNow = nowNs
        for i in 0..<taken { output[i] = storage[read]; storage[read] = 0; read = (read + 1) % storage.count }
        count -= taken
        return taken / format.frameBytes
    }
}
public enum AudioRole: String, CaseIterable { case receive, microphone, external, file, generated, duplex, standaloneDSP }
public enum StopReason { case background, focusLoss, interruption, routeChange, permissionRevoked, peerRevoked, networkChange, restart }
public final class NativeEndpoint {
    public let backendAvailable = false; public let audioApplied = false
    public init() {}
    public var capabilities: [String: String] { Dictionary(uniqueKeysWithValues: AudioRole.allCases.map { ($0.rawValue, "shared-core-and-transport-not-linked") }) }
    public func start(_ role: AudioRole) throws { throw MobileError.unsupported("\(role.rawValue): shared-core-and-transport-not-linked; explicit consent and exact route required") }
    public func stop(_ reason: StopReason) { /* No native audio handle is opened before linked backend admission. */ }
}
