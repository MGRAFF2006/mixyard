import Foundation
import Darwin
import Security
import CryptoKit

// Read only the three owned fingerprint statuses and aggregate ADMIN count.
// No dump, trust mutation, exception text, certificate or path is ever output.
@main struct OwnedTrustReadback {
    static func main() {
        guard ProcessInfo.processInfo.environment["GITHUB_ACTIONS"] == "true",
              ProcessInfo.processInfo.environment["RUNNER_OS"] == "macOS",
              CommandLine.arguments.count == 7 else { exit(2) }
        var statuses: [OSStatus] = []
        for index in stride(from: 1, to: 7, by: 2) {
            guard let data = try? Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[index])), data.count <= 65536,
                  let certificate = SecCertificateCreateWithData(nil, data as CFData) else { exit(2) }
            let digest = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
            guard digest == CommandLine.arguments[index + 1] else { exit(2) }
            var settings: CFArray?
            statuses.append(SecTrustSettingsCopyTrustSettings(certificate, .admin, &settings))
        }
        let absent = statuses.allSatisfy { $0 == errSecItemNotFound || $0 == errSecNoTrustSettings }
        var domainCertificates: CFArray?
        let domainStatus = SecTrustSettingsCopyCertificates(.admin, &domainCertificates)
        let count: Int
        if domainStatus == errSecSuccess { count = domainCertificates.map { CFArrayGetCount($0) } ?? -1 }
        else if domainStatus == errSecNoTrustSettings || domainStatus == errSecItemNotFound { count = 0 }
        else { count = -1 }
        // Count/status only: no unrelated certificate identity or trust dump.
        let report: [String: Any] = ["statuses": statuses.map { Int($0) }, "ownedTrustAbsent": absent,
                                    "adminCount": count, "adminStatus": Int(domainStatus)]
        guard let encoded = try? JSONSerialization.data(withJSONObject: report) else { exit(2) }
        FileHandle.standardOutput.write(encoded)
        FileHandle.standardOutput.write(Data([10]))
    }
}
