import Foundation
import MobileCore
import NativeHost

@main struct HostQualification {
    static func main() throws {
        guard CommandLine.arguments.count == 2,
              var request = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))) as? [String: Any],
              var config = request["config"] as? [String: Any], var sends = config["sends"] as? [[String: Any]],
              var limits = request["limits"] as? [String: Any] else { throw MobileError.invalid("fixture") }
        sends.append(["source": "return", "mix": "call", "gain": 1.0]); config["sends"] = sends
        limits["maxSends"] = 4; request["config"] = config; request["limits"] = limits
        func ports(_ name: String) throws -> [HostPort] {
            guard let values = request[name] as? [[String: Any]] else { throw MobileError.invalid("fixture-ports") }
            return try values.map { value in
                guard let id = value["observedId"] as? String, let rate = value["sampleRateHz"] as? UInt32,
                      let channels = value["channels"] as? [String], let mapping = value["mapping"] as? [String] else { throw MobileError.invalid("fixture-port") }
                return HostPort(observedId: id, format: try RawFormat(sample: .f64le, rate: rate, channels: channels), mapping: mapping)
            }
        }
        let sources = try ports("sources"), outputs = try ports("outputs")
        let json = String(decoding: try JSONSerialization.data(withJSONObject: request), as: UTF8.self)
        let worker = try NativeHostWorker(requestJSON: json, frames: 4, sources: sources, outputs: outputs)
        defer { try? worker.close() }
        precondition(!worker.backendAvailable && !worker.audioApplied)
        for (index, level) in [999.0, 4.0, 2.0].enumerated() {
            try worker.withInput(index) { storage in
                for sample in 0..<8 { storage.storeBytes(of: level, toByteOffset: sample * 8, as: Double.self) }
            }
        }
        let stamp = HostStamp(generation: 7, revision: 1, clockId: 11, sequence: 0, firstFrame: 0, capturedNs: 100, deadlineNs: 1000)
        try worker.process(nowNs: 200, engine: stamp, sourceStamps: Array(repeating: stamp, count: sources.count))
        for (index, expected) in [2.0, 1.0].enumerated() {
            try worker.withOutput(index) { storage in
                for sample in 0..<8 { precondition(storage.load(fromByteOffset: sample * 8, as: Double.self) == expected, "canonical-policy-or-destination-DSP-regression") }
            }
        }
        let stale = HostStamp(generation: 8, revision: 1, clockId: 11, sequence: 1, firstFrame: 4, capturedNs: 300, deadlineNs: 1000)
        var rejected = false
        do { try worker.process(nowNs: 400, engine: stale, sourceStamps: Array(repeating: stale, count: sources.count)) }
        catch { rejected = true }
        precondition(rejected, "stale-binding-accepted")
        for index in outputs.indices {
            try worker.withOutput(index) { storage in precondition(storage.allSatisfy { $0 == 0 }, "revoked-PCM-retained") }
        }
        for index in sources.indices {
            try worker.withInput(index) { storage in precondition(storage.allSatisfy { $0 == 0 }, "revoked-input-retained") }
        }
        print("PASS: actual Swift/canonical Rust offline policy, mix-minus, independent DSP and revoke scrubbing; no native audio qualification")
    }
}
