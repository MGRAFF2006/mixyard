import Foundation
import Security
import MobileCore

struct Provisioning: Codable {
    let endpoint: String; let pin: String; let deviceId: String
    let credentialId: String; let credential: String
    // Optional only to decode legacy records without erasing their credentials.
    // Such records cannot dispatch HTTP; explicit OOB reprovisioning is required.
    let certificatePem: String?
    let serverName: String?
    let version: Int?
}
// Flat canonical OOB object: platform JSON decodes each quoted token, while
// this bounded envelope reader preserves key multiplicity before dictionary use.
struct OobRecord {
    let deviceId: String; let serverName: String; let certificatePem: String; let certificateSha256: String
    static func parse(_ text: String, expectedDevice: String) throws -> OobRecord {
        guard text.utf8.count <= 65536 else { throw MobileError.invalid("oob-record-too-large") }
        let bytes = [UInt8](text.utf8)
        var at = 0
        func white() { while at < bytes.count && [9,10,13,32].contains(bytes[at]) { at += 1 } }
        func take(_ byte: UInt8) throws { white(); guard at < bytes.count, bytes[at] == byte else { throw MobileError.invalid("strict-oob-json-required") }; at += 1 }
        func string() throws -> String {
            white(); let start = at; try take(34)
            var closed = false
            while at < bytes.count {
                let byte = bytes[at]; at += 1
                if byte == 34 { closed = true; break }
                if byte == 92 { guard at < bytes.count else { throw MobileError.invalid("oob-json-string") }; at += 1 }
            }
            guard closed, let value = try JSONSerialization.jsonObject(with: Data(bytes[start..<at]), options: .fragmentsAllowed) as? String else { throw MobileError.invalid("oob-string-required") }
            return value
        }
        let keys: Set<String> = ["deviceId", "serverName", "certificatePem", "certificateSha256"]
        var values: [String:String] = [:]
        try take(123)
        while true {
            let key = try string()
            guard keys.contains(key), values[key] == nil else { throw MobileError.invalid("oob-unknown-or-duplicate-field") }
            try take(58); values[key] = try string(); white()
            guard at < bytes.count else { throw MobileError.invalid("oob-json-object") }
            if bytes[at] == 125 { at += 1; break }; try take(44)
        }
        white()
        guard at == bytes.count else { throw MobileError.invalid("oob-trailing-json") }
        guard Set(values.keys) == keys else { throw MobileError.invalid("oob-missing-field") }
        guard let device = values["deviceId"], expectedDevice.utf8.count == 64,
              expectedDevice.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }) else { throw MobileError.invalid("oob-expected-device-format") }
        guard device == expectedDevice else { throw MobileError.invalid("oob-selected-device-mismatch") }
        guard let name = values["serverName"], name == "mixyard-control" else { throw MobileError.invalid("oob-server-name-mismatch") }
        guard let pem = values["certificatePem"], let pin = values["certificateSha256"] else { throw MobileError.invalid("oob-missing-field") }
        return OobRecord(deviceId: device, serverName: name, certificatePem: pem, certificateSha256: pin)
    }
}

protocol CredentialVault {
    func load() throws -> Provisioning?
    func save(_ value: Provisioning) throws
    func forget() throws
}
struct CredentialStore: CredentialVault {
    private var query: [String: Any] { [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: "org.mixyard.mobile.control", kSecAttrAccount as String: "selected-device"] }
    func load() throws -> Provisioning? {
        var q = query; q[kSecReturnData as String] = true; q[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?; let result = SecItemCopyMatching(q as CFDictionary, &item)
        if result == errSecItemNotFound { return nil }
        guard result == errSecSuccess, let data = item as? Data else { throw MobileError.invalid("keychain-unavailable") }
        return try JSONDecoder().decode(Provisioning.self, from: data)
    }
    func save(_ value: Provisioning) throws {
        let data = try JSONEncoder().encode(value)
        let result = SecItemUpdate(query as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if result == errSecSuccess { return }
        guard result == errSecItemNotFound else { throw MobileError.invalid("keychain-write-failed") }
        var q = query; q[kSecValueData as String] = data
        q[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        guard SecItemAdd(q as CFDictionary, nil) == errSecSuccess else { throw MobileError.invalid("keychain-write-failed") }
    }
    func forget() throws {
        let result = SecItemDelete(query as CFDictionary)
        guard result == errSecSuccess || result == errSecItemNotFound else { throw MobileError.invalid("keychain-delete-failed") }
    }
}
