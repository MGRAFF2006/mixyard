import Foundation
import MobileCore
import NativeHost

@main struct HostQualification {
    static func main() throws {
        guard CommandLine.arguments.count == 2 else { throw MobileError.invalid("fixture") }
        let admitted = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
        guard var request = try JSONSerialization.jsonObject(with: admitted) as? [String: Any],
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
        var rejectedFrameBudget = false
        do { let excessive = try NativeHostWorker(requestJSON: String(decoding: admitted, as: UTF8.self), frames: 65, sources: sources, outputs: outputs); try excessive.close() }
        catch { rejectedFrameBudget = true }
        precondition(rejectedFrameBudget, "worker-frame-budget-bypassed")
        func seed(_ worker: NativeHostWorker) throws {
            for (index, level) in [999.0, 4.0, 2.0].enumerated() {
                try worker.withInput(index) { storage in
                    for sample in 0..<8 { storage.storeBytes(of: level, toByteOffset: sample * 8, as: Double.self) }
                }
            }
        }
        let json = String(decoding: try JSONSerialization.data(withJSONObject: request), as: UTF8.self)
        var rejectedMixMinus = false
        do { let forbidden = try NativeHostWorker(requestJSON: json, frames: 4, sources: sources, outputs: outputs); try forbidden.close() }
        catch { rejectedMixMinus = true }
        precondition(rejectedMixMinus, "excluded-call-return-admitted")
        let worker = try NativeHostWorker(requestJSON: String(decoding: admitted, as: UTF8.self), frames: 4, sources: sources, outputs: outputs)
        defer { try? worker.close() }
        precondition(!worker.backendAvailable && !worker.audioApplied)
        try seed(worker)
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
        guard var failedOutput = try JSONSerialization.jsonObject(with: admitted) as? [String: Any],
              var chains = failedOutput["outputDsp"] as? [Any], var chain = chains[1] as? [String: Any],
              var effects = chain["effects"] as? [[String: Any]], var effect = effects[0]["effect"] as? [String: Any] else { throw MobileError.invalid("fixture-effect") }
        effect["gain"] = Double.greatestFiniteMagnitude; effects[0]["effect"] = effect; chain["effects"] = effects
        chains[1] = chain; failedOutput["outputDsp"] = chains
        let failedJSON = String(decoding: try JSONSerialization.data(withJSONObject: failedOutput), as: UTF8.self)
        let isolated = try NativeHostWorker(requestJSON: failedJSON, frames: 4, sources: sources, outputs: outputs)
        defer { try? isolated.close() }
        try seed(isolated)
        for sequence in UInt64(0)...UInt64(1) {
            let captured: UInt64 = sequence == 0 ? 100 : 83_434
            let stamp = HostStamp(generation: 7, revision: 1, clockId: 11, sequence: sequence, firstFrame: sequence * 4, capturedNs: captured, deadlineNs: 1_000_000)
            try isolated.process(nowNs: captured + 100, engine: stamp, sourceStamps: Array(repeating: stamp, count: sources.count))
            for index in outputs.indices {
                let status = try isolated.outputStatus(index)
                precondition(status == (index == 0 ? 0 : 7), "output-failure-not-isolated")
                try isolated.withOutput(index) { storage in
                    for sample in 0..<8 { precondition(storage.load(fromByteOffset: sample * 8, as: Double.self) == (index == 0 ? 2.0 : 0.0)) }
                }
            }
        }
        print("PASS: actual Swift/canonical Rust offline policy, mix-minus, independent DSP/output failure and revoke scrubbing; no native audio qualification")
    }
}
