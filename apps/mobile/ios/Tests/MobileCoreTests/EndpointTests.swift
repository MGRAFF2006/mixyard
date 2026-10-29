import XCTest
@testable import MobileCore
final class EndpointTests: XCTestCase {
    func testExactRawLoopbackAndSiblingRevocation() throws {
        for sample in Sample.allCases {
            let format = try RawFormat(sample: sample, rate: 44117, channels: ["right", "left", "aux"])
            let send = try PcmQueue(format: format, generation: 1, capacityFrames: 4)
            let receive = try PcmQueue(format: format, generation: 2, capacityFrames: 4)
            let input = (0..<(format.frameBytes * 2)).map { UInt8(($0 * 7 + 1) % 256) }
            try send.push(generation: 1, sequence: 0, firstFrame: 0, deadlineNs: 100, nowNs: 1, bytes: input)
            try receive.push(generation: 2, sequence: 0, firstFrame: 0, deadlineNs: 100, nowNs: 1, bytes: input)
            send.invalidate()
            var out = [UInt8](repeating: 42, count: input.count)
            XCTAssertEqual(try send.pull(into: &out, nowNs: 2), 0); XCTAssertEqual(out, [UInt8](repeating: 0, count: input.count))
            XCTAssertEqual(try receive.pull(into: &out, nowNs: 2), 2); XCTAssertEqual(out, input)
        }
    }
    func testOverflowAndReplayScrub() throws {
        let format = try RawFormat(sample: .s16le, rate: 96000, channels: ["mono"])
        for sequence: UInt64 in [0, 1] {
            let q = try PcmQueue(format: format, generation: 1, capacityFrames: 1)
            try q.push(generation: 1, sequence: 0, firstFrame: 0, deadlineNs: 20, nowNs: 0, bytes: [1, 2])
            XCTAssertThrowsError(try q.push(generation: 1, sequence: sequence, firstFrame: 1, deadlineNs: 20, nowNs: 1, bytes: [3, 4]))
            var out: [UInt8] = [9, 9]; XCTAssertEqual(try q.pull(into: &out, nowNs: 2), 0); XCTAssertEqual(out, [0, 0])
        }
    }
    func testAuthoritySwitchAndSafeRevision() throws {
        var fence = SessionFence(); let pending = fence.ticket; fence.invalidate()
        XCTAssertThrowsError(try fence.establish(pending, Authority(deviceId: "B", authorityId: "a", revision: 0)))
        try fence.establish(fence.ticket, Authority(deviceId: "A", authorityId: "a", revision: 1))
        XCTAssertThrowsError(try fence.accept(fence.ticket, Authority(deviceId: "B", authorityId: "a", revision: 2)))
        XCTAssertThrowsError(try Authority(deviceId: "A", authorityId: "a", revision: 9_007_199_254_740_992))
    }
    func testQueuedBytesExpireBeforeConsumption() throws {
        let q = try PcmQueue(format: RawFormat(sample: .s16le, rate: 44117, channels: ["mono"]), generation: 1, capacityFrames: 1)
        try q.push(generation: 1, sequence: 0, firstFrame: 0, deadlineNs: 3, nowNs: 0, bytes: [1, 2])
        var out: [UInt8] = [9, 9]
        XCTAssertEqual(try q.pull(into: &out, nowNs: 3), 0); XCTAssertFalse(q.active); XCTAssertEqual(out, [0, 0])
    }
    func testWholePremixExcludedAndAudioUnavailable() throws {
        XCTAssertThrowsError(try Provenance(sources: ["mic", "return"], groups: ["call"], visitedPeers: ["desktop"]).admit(localPeer: "phone", excludedGroups: ["call"]))
        XCTAssertThrowsError(try Provenance(sources: ["mic"], groups: [], visitedPeers: ["phone", "desktop"]).admit(localPeer: "phone", excludedGroups: []))
        XCTAssertThrowsError(try NativeEndpoint().start(.microphone))
    }
}
