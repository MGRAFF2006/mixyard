import Foundation
/** Explicit raw little-endian files. Encoded/compressed formats are unsupported without an admitted decoder. */
public final class RawFileSource {
    public let format: RawFormat
    private let file: FileHandle
    public init(file: FileHandle, format: RawFormat) { self.file = file; self.format = format }
    public func readFrames(maxFrames: Int) throws -> [UInt8] {
        guard maxFrames > 0, maxFrames <= Int.max / format.frameBytes else { throw MobileError.invalid("file-block-budget") }
        let limit = maxFrames * format.frameBytes
        var data = Data()
        do {
            while data.count < limit {
                let part = try file.read(upToCount: limit - data.count) ?? Data()
                if part.isEmpty { break }
                data.append(part)
            }
            guard data.count % format.frameBytes == 0 else { throw MobileError.invalid("partial-file-frame-no-padding") }
            return [UInt8](data)
        } catch { try? file.close(); throw error }
    }
    public func close() throws { try file.close() }
}
/** Finite repeated exact frame; synthetic worker input, never microphone or clock qualification. */
public final class GeneratedSource {
    public let format: RawFormat
    private let frame: [UInt8]
    private var remaining: UInt64
    public init(format: RawFormat, frame: [UInt8], frames: UInt64) throws {
        guard frame.count == format.frameBytes else { throw MobileError.invalid("generated-frame-format") }
        self.format = format; self.frame = frame; self.remaining = frames
    }
    public func readFrames(maxFrames: Int) throws -> [UInt8] {
        guard maxFrames > 0, maxFrames <= Int.max / format.frameBytes else { throw MobileError.invalid("generated-block-budget") }
        let count = Int(min(remaining, UInt64(maxFrames)))
        var result = [UInt8](repeating: 0, count: count * format.frameBytes)
        for i in 0..<count { result.replaceSubrange((i * format.frameBytes)..<((i + 1) * format.frameBytes), with: frame) }
        remaining -= UInt64(count)
        return result
    }
}
