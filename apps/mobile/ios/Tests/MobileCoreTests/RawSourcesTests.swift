import XCTest
import Foundation
@testable import MobileCore
final class RawSourcesTests: XCTestCase {
    func testGeneratedFileLoopbackPreservesIeeeLowBit() throws {
        let format = try RawFormat(sample: .f64le, rate: 192000, channels: ["mono"])
        let frame: [UInt8] = [1, 0, 0, 0, 0, 0, 0xf0, 0x3f] // 1.0000000000000002; f32 conversion would lose bit zero.
        let source = try GeneratedSource(format: format, frame: frame, frames: 3)
        let generated = try source.readFrames(maxFrames: 4)
        XCTAssertEqual(generated, frame + frame + frame)
        XCTAssertEqual(try source.readFrames(maxFrames: 4), [])
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: url) }
        try Data(generated).write(to: url)
        let file = RawFileSource(file: try FileHandle(forReadingFrom: url), format: format)
        defer { try? file.close() }
        let q = try PcmQueue(format: format, generation: 1, capacityFrames: 4)
        try q.push(generation: 1, sequence: 0, firstFrame: 0, deadlineNs: 50, nowNs: 0, bytes: file.readFrames(maxFrames: 2))
        try q.push(generation: 1, sequence: 1, firstFrame: 2, deadlineNs: 50, nowNs: 1, bytes: file.readFrames(maxFrames: 2))
        var out = [UInt8](repeating: 0, count: 24)
        XCTAssertEqual(try q.pull(into: &out, nowNs: 2), 3); XCTAssertEqual(out, generated)
    }
    func testRawFileTailRejectsWithoutPadding() throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: url) }
        try Data([1, 2, 3]).write(to: url)
        let file = RawFileSource(file: try FileHandle(forReadingFrom: url), format: try RawFormat(sample: .s16le, rate: 44117, channels: ["mono"]))
        XCTAssertThrowsError(try file.readFrames(maxFrames: 4))
    }
}
