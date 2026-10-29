import Foundation
import Security
import MobileCore

struct Provisioning: Codable {
    let endpoint: String; let pin: String; let deviceId: String
    let credentialId: String; let credential: String
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
