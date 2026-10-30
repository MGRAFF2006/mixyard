import Foundation
import MobileCore

// The only replaced behavior is the vault platform boundary. ControlClient,
// URLSession, Security PKI/pin checks and raw serializer are shipping sources.
final class MemoryVault: CredentialVault {
    var saved: Provisioning?
    var failWrites = false
    func load() throws -> Provisioning? { saved }
    func save(_ value: Provisioning) throws {
        guard !failWrites else { throw MobileError.invalid("credential-storage-failed") }
        saved = value
    }
    func forget() throws {
        guard !failWrites else { throw MobileError.invalid("credential-storage-failed") }
        saved = nil
    }
}

@main struct NativeQualification {
    @MainActor static func main() async throws {
        let vault = MemoryVault()
        var client = try ControlClient(store: vault)
        var captured: [String: Ticket] = [:]
        FileHandle.standardOutput.write(Data("{\"event\":\"ready\"}\n".utf8))
        while let line = readLine() {
            var reply: [String: Any] = [:]
            do {
                guard let bytes = line.data(using: .utf8),
                      let command = try JSONSerialization.jsonObject(with: bytes) as? [String: Any],
                      let op = command["op"] as? String else { throw MobileError.invalid("driver-command") }
                func text(_ key: String) throws -> String {
                    guard let value = command[key] as? String else { throw MobileError.invalid("driver-field") }
                    return value
                }
                let expected: Ticket
                if let key = command["expected"] as? String {
                    guard let value = captured[key] else { throw MobileError.invalid("missing-driver-ticket") }
                    expected = value
                } else { expected = client.ticket }
                switch op {
                case "pair":
                    try await client.pair(endpoint: text("endpoint"), pin: text("pin"), certificatePem: text("certificatePem"), device: text("deviceId"), invitation: text("invitation"), name: "Owned native qualification", expected: expected)
                case "invoke":
                    let method = try text("method")
                    let payload: Any = method == "request" ? try text("payload") : [String: Any]()
                    let value = try await client.invoke(method: method, payload: payload, expected: expected)
                    if method == "request" {
                        guard let raw = value["json"] as? String, let status = value["status"] as? Int else { throw MobileError.invalid("raw-driver-reply") }
                        reply["raw"] = raw; reply["status"] = status
                    } else {
                        reply["raw"] = String(decoding: try JSONSerialization.data(withJSONObject: value), as: UTF8.self)
                    }
                case "capture": captured[try text("key")] = client.ticket
                case "restartClient": client.invalidate(); client = try ControlClient(store: vault)
                case "storageFail":
                    guard let enabled = command["enabled"] as? Bool else { throw MobileError.invalid("driver-field") }
                    vault.failWrites = enabled
                case "forget": try client.forget()
                default: throw MobileError.invalid("driver-command")
                }
                reply["ok"] = true
            } catch {
                // Export classification only, never arbitrary errors containing
                // credentials, invitations or native provisioning.
                reply["ok"] = false
                let native = error as NSError
                let known = [NSURLErrorDomain, NSPOSIXErrorDomain, NSOSStatusErrorDomain]
                reply["failure"] = known.contains(native.domain) ? "\(native.domain):\(native.code)" : String(describing: type(of: error))
            }
            reply["hasAuthority"] = client.ticket.authority != nil
            let data = try JSONSerialization.data(withJSONObject: reply)
            FileHandle.standardOutput.write(data)
            FileHandle.standardOutput.write(Data([10]))
        }
    }
}
