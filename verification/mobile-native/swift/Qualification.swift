import Foundation
import MobileCore
import Security

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
        var legacyCredential: String?
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
                    try await client.pair(endpoint: text("endpoint"), trustRecord: text("trustRecord"), device: text("deviceId"), invitation: text("invitation"), name: "Owned native qualification", expected: expected)
                case "preflightCertificate":
                    let record = try OobRecord.parse(text("trustRecord"), expectedDevice: text("deviceId"))
                    _ = try OobCertificate(pem: record.certificatePem, pin: record.certificateSha256)
                case "diagnosticFixture": throw MobileError.invalid("PRIVATE_PROVISIONING_MUST_NEVER_BE_EXPORTED")
                case "evaluateSystemCertificate":
                    let record = try OobRecord.parse(text("trustRecord"), expectedDevice: text("deviceId"))
                    let anchor = try OobCertificate(pem: record.certificatePem, pin: record.certificateSha256)
                    var roots: CFArray?
                    guard SecTrustCopyAnchorCertificates(&roots) == errSecSuccess, let certificates = roots as? [SecCertificate] else { throw MobileError.invalid("system-certificate-evaluation-unavailable") }
                    var found = false
                    for certificate in certificates.prefix(16) {
                        var ordinary: SecTrust?
                        guard SecTrustCreateWithCertificates(certificate, SecPolicyCreateBasicX509(), &ordinary) == errSecSuccess, let ordinary,
                              SecTrustSetNetworkFetchAllowed(ordinary, false) == errSecSuccess, SecTrustEvaluateWithError(ordinary, nil) else { continue }
                        found = true
                        guard !anchor.admits(ordinary, host: "127.0.0.1") else { throw MobileError.invalid("private-anchor-accepted-unrelated-system-certificate") }
                        break
                    }
                    guard found else { throw MobileError.invalid("system-certificate-evaluation-unavailable") }
                    reply["systemCertificateObservedTrusted"] = true; reply["privateAnchorRejected"] = true; reply["evaluatorOnly"] = true
                case "installLegacyFixture":
                    client.invalidate(); legacyCredential = "owned-legacy-fixture-credential"
                    let legacy: [String:Any] = ["endpoint":try text("endpoint"), "pin":try text("pin"), "deviceId":try text("deviceId"), "credentialId":"owned-legacy-fixture-id", "credential":legacyCredential!]
                    vault.saved = try JSONDecoder().decode(Provisioning.self, from: JSONSerialization.data(withJSONObject: legacy))
                    client = try ControlClient(store: vault)
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
                let recordCodes: Set<String> = ["oob-record-too-large", "strict-oob-json-required", "oob-json-string", "oob-string-required", "oob-unknown-or-duplicate-field", "oob-json-object", "oob-selected-device-or-record-mismatch", "oob-trailing-json", "oob-missing-field", "oob-expected-device-format", "oob-selected-device-mismatch", "oob-server-name-mismatch"]
                let certificateCodes: Set<String> = ["explicit-oob-certificate-required", "one-explicit-oob-certificate-required", "certificate-base64-invalid", "exact-certificate-der-required", "oob-certificate-fingerprint-mismatch", "oob-certificate-too-large", "oob-pin-format", "certificate-base64-decode", "certificate-base64-canonical", "certificate-decode", "certificate-der-roundtrip", "canonical-self-signature-required"]
                let evaluatorCodes: Set<String> = ["system-certificate-evaluation-unavailable", "private-anchor-accepted-unrelated-system-certificate"]
                if case MobileError.invalid(let code) = error, recordCodes.contains(code) || certificateCodes.contains(code) || evaluatorCodes.contains(code) {
                    reply["phase"] = recordCodes.contains(code) ? "oob-record" : certificateCodes.contains(code) ? "oob-certificate" : "public-evaluator"; reply["failure"] = code
                } else {
                    let native = error as NSError
                    let known = [NSURLErrorDomain, NSPOSIXErrorDomain, NSOSStatusErrorDomain]
                    reply["phase"] = known.contains(native.domain) ? "tls-platform" : "native"
                    reply["failure"] = known.contains(native.domain) ? "\(native.domain):\(native.code)" : "native-invalid"
                }
            }
            reply["hasAuthority"] = client.ticket.authority != nil
            if let legacyCredential { reply["recordRetained"] = vault.saved?.credential == legacyCredential }
            let data = try JSONSerialization.data(withJSONObject: reply)
            FileHandle.standardOutput.write(data)
            FileHandle.standardOutput.write(Data([10]))
        }
    }
}
