import Foundation
import CHost
import MobileCore

public struct HostPort {
    public let observedId: String
    public let format: RawFormat
    public let mapping: [String]
    public init(observedId: String, format: RawFormat, mapping: [String]) {
        self.observedId = observedId; self.format = format; self.mapping = mapping
    }
}
public struct HostStamp {
    public let generation, revision, clockId, sequence, firstFrame, capturedNs, deadlineNs: UInt64
    public init(generation: UInt64, revision: UInt64, clockId: UInt64, sequence: UInt64,
                firstFrame: UInt64, capturedNs: UInt64, deadlineNs: UInt64) {
        self.generation = generation; self.revision = revision; self.clockId = clockId
        self.sequence = sequence; self.firstFrame = firstFrame; self.capturedNs = capturedNs; self.deadlineNs = deadlineNs
    }
    fileprivate var native: mx_stamp {
        mx_stamp(generation: generation, revision: revision, clock_id: clockId, sequence: sequence,
                 first_frame: firstFrame, captured_ns: capturedNs, deadline_ns: deadlineNs)
    }
}

/// Exclusive offline worker, not an audio callback or a source of authority.
/// The canonical Rust host owns the only policy mixer, DSP and retained PCM.
public final class NativeHostWorker {
    private var host: OpaquePointer?
    private let owner = Thread.current
    private var borrowed = false
    public let frames: Int
    public let sources: [HostPort], outputs: [HostPort]
    private var inputStorage: [UnsafeMutableRawBufferPointer] = []
    private var outputStorage: [UnsafeMutableRawBufferPointer] = []
    private var inputs: [mx_input] = []
    public let backendAvailable = false, audioApplied = false

    public init(requestJSON: String, frames: Int, sources: [HostPort], outputs: [HostPort]) throws {
        self.frames = frames; self.sources = sources; self.outputs = outputs
        guard UInt16(littleEndian: 1) == 1, frames > 0, !sources.isEmpty, !outputs.isEmpty,
              sources.allSatisfy({ !$0.observedId.isEmpty && $0.mapping.count == $0.format.channels.count }),
              outputs.allSatisfy({ !$0.observedId.isEmpty && $0.mapping.count == $0.format.channels.count && $0.format.sample == .f64le }) else { throw MobileError.invalid("exact-host-format-required") }
        let bytes = Array(requestJSON.utf8)
        var error = [CChar](repeating: 0, count: 512)
        let errorCapacity = error.count
        let result = bytes.withUnsafeBufferPointer { pointer in
            mx_host_create(pointer.baseAddress, pointer.count, &host, &error, errorCapacity)
        }
        guard result == 0, let host else { throw MobileError.invalid("canonical-host-admission-rejected") }
        do {
            guard mx_host_source_count(host) == sources.count, mx_host_output_count(host) == outputs.count else { throw MobileError.invalid("exact-port-count-required") }
            for (index, port) in sources.enumerated() {
                try verify(port: port, index: index, output: false)
                let (size, overflow) = frames.multipliedReportingOverflow(by: port.format.frameBytes)
                guard !overflow else { throw MobileError.invalid("host-buffer-budget") }
                let storage = UnsafeMutableRawBufferPointer.allocate(byteCount: size, alignment: 8)
                storage.initializeMemory(as: UInt8.self, repeating: 0); inputStorage.append(storage)
                let encoding: UInt32
                switch port.format.sample { case .f32le: encoding = 1; case .f64le: encoding = 2; case .s16le: encoding = 3; case .s24le: encoding = 4; case .s32le: encoding = 5 }
                inputs.append(mx_input(bytes: storage.baseAddress!.assumingMemoryBound(to: UInt8.self), byte_count: size,
                    encoding: encoding, sample_rate_hz: port.format.rate, channels: UInt32(port.format.channels.count), stamp: mx_stamp()))
            }
            for (index, port) in outputs.enumerated() {
                try verify(port: port, index: index, output: true)
                let (size, overflow) = frames.multipliedReportingOverflow(by: port.format.frameBytes)
                guard !overflow else { throw MobileError.invalid("host-buffer-budget") }
                let storage = UnsafeMutableRawBufferPointer.allocate(byteCount: size, alignment: 8)
                storage.initializeMemory(as: UInt8.self, repeating: 0); outputStorage.append(storage)
            }
        } catch { release(); throw error }
    }
    private func verify(port: HostPort, index: Int, output: Bool) throws {
        let read = output ? mx_host_output_info : mx_host_source_info
        var needed = 0
        guard read(host, index, nil, 0, &needed) == 8, needed > 0, needed <= 1_048_576 else { throw MobileError.invalid("host-port-unavailable") }
        var bytes = [UInt8](repeating: 0, count: needed)
        let capacity = bytes.count
        guard read(host, index, &bytes, capacity, &needed) == 0,
              let value = try JSONSerialization.jsonObject(with: Data(bytes)) as? [String: Any],
              value["observedId"] as? String == port.observedId,
              (value["sampleRateHz"] as? NSNumber)?.uint32Value == port.format.rate,
              value["channels"] as? [String] == port.format.channels,
              value["mapping"] as? [String] == port.mapping else { throw MobileError.invalid("exact-port-format-required") }
        if !output {
            let encoding: String
            switch port.format.sample { case .s16le: encoding = "s16-le"; case .s24le: encoding = "s24-le"; case .s32le: encoding = "s32-le"; case .f32le: encoding = "f32-le"; case .f64le: encoding = "f64-le" }
            guard value["encoding"] as? String == encoding else { throw MobileError.invalid("exact-port-encoding-required") }
        }
    }
    private func owned() throws {
        guard Thread.current === owner, host != nil, !borrowed else { throw MobileError.invalid("exclusive-worker-required") }
    }
    /// Borrow only within the exclusive owner's operation; never retain after close.
    public func withInput<T>(_ index: Int, _ body: (UnsafeMutableRawBufferPointer) throws -> T) throws -> T {
        try owned(); guard inputStorage.indices.contains(index) else { throw MobileError.invalid("host-port-index") }
        borrowed = true; defer { borrowed = false }
        return try body(inputStorage[index])
    }
    /// The output copy has exact admitted Float64 little-endian format. No PCM JSON.
    public func withOutput<T>(_ index: Int, _ body: (UnsafeRawBufferPointer) throws -> T) throws -> T {
        try owned(); guard outputStorage.indices.contains(index) else { throw MobileError.invalid("host-port-index") }
        borrowed = true; defer { borrowed = false }
        return try body(UnsafeRawBufferPointer(outputStorage[index]))
    }
    public func process(nowNs: UInt64, engine: HostStamp, sourceStamps: [HostStamp]) throws {
        try owned()
        do {
            guard sourceStamps.count == inputs.count else { throw MobileError.invalid("exact-source-count-required") }
            for index in inputs.indices { inputs[index].stamp = sourceStamps[index].native }
            let result = inputs.withUnsafeBufferPointer { pointer in mx_host_process(host, nowNs, engine.native, frames, pointer.baseAddress, pointer.count) }
            guard result == 0 else { throw MobileError.invalid("canonical-host-process-\(result)") }
            for index in outputs.indices {
                var samples: UnsafePointer<Double>?
                var count = 0
                guard mx_host_output(host, index, &samples, &count) == 0, let samples,
                      count == outputStorage[index].count / MemoryLayout<Double>.size else { throw MobileError.invalid("host-output-buffer") }
                outputStorage[index].initializeMemory(as: UInt8.self, repeating: 0)
                outputStorage[index].baseAddress!.copyMemory(from: samples, byteCount: count * MemoryLayout<Double>.size)
            }
        } catch { revokeOwned(); throw error }
    }
    private func revokeOwned() {
        if let host { mx_host_revoke(host) }
        for storage in inputStorage { storage.initializeMemory(as: UInt8.self, repeating: 0) }
        for storage in outputStorage { storage.initializeMemory(as: UInt8.self, repeating: 0) }
    }
    public func revoke() throws { try owned(); revokeOwned() }
    public func close() throws { try owned(); release() }
    private func release() {
        revokeOwned()
        if let host { mx_host_destroy(host); self.host = nil }
        for storage in inputStorage { storage.initializeMemory(as: UInt8.self, repeating: 0); storage.deallocate() }
        for storage in outputStorage { storage.deallocate() }
        inputStorage.removeAll(); outputStorage.removeAll(); inputs.removeAll()
    }
    deinit { release() }
}
