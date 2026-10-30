import Foundation
import Security
import SwiftASN1

/// Supplementary self-signature verification for the canonical control identity.
/// This is deliberately not a CA/path validator. Callers must still perform the
/// ordinary SSL/name/date/Only-anchor SecTrust evaluation and exact leaf pin.
public enum CanonicalControlCertificate {
    public enum Rejection: Error { case invalidCanonicalIdentity }

    public static func verify(_ certificateDER: Data) throws {
        do { try verifyCanonical(certificateDER) }
        catch { throw Rejection.invalidCanonicalIdentity } // No parser/key/trust data escapes.
    }

    private static func verifyCanonical(_ data: Data) throws {
        guard !data.isEmpty, data.count <= 65536 else { throw Rejection.invalidCanonicalIdentity }
        let root = try DER.parse(Array(data)) // DER only: one root, strict lengths, bounded depth.
        let parts = try DER.sequence(root, identifier: .sequence) { nodes in
            (try next(&nodes), try next(&nodes), try next(&nodes))
        }
        try signatureAlgorithm(parts.1)
        let tbsAlgorithm = try DER.sequence(parts.0, identifier: .sequence) { nodes in
            let version = try DER.explicitlyTagged(&nodes, tagNumber: 0, tagClass: .contextSpecific) {
                try UInt8(derEncoded: $0)
            }
            guard version == 2 else { throw Rejection.invalidCanonicalIdentity }
            let serial = try PositiveInteger(derEncoded: next(&nodes))
            guard !serial.bytes.isEmpty, serial.bytes.count <= 20 else { throw Rejection.invalidCanonicalIdentity }
            let algorithm = try next(&nodes)
            try signatureAlgorithm(algorithm)
            try name(next(&nodes)) // Issuer/subject identity is checked through public Security below.
            try DER.sequence(next(&nodes), identifier: .sequence) { validity in
                for _ in 0..<2 {
                    let time = try next(&validity)
                    guard time.identifier == .utcTime || time.identifier == .generalizedTime,
                          case .primitive(let bytes) = time.content, !bytes.isEmpty else {
                        throw Rejection.invalidCanonicalIdentity
                    }
                }
            } // No date parsing: actual SecTrust enforces validity.
            try name(next(&nodes))
            try publicKeyInfo(next(&nodes))
            if let extensions = nodes.next() {
                try DER.explicitlyTagged(extensions, tagNumber: 3, tagClass: .contextSpecific) { value in
                    guard value.identifier == .sequence, case .constructed = value.content else {
                        throw Rejection.invalidCanonicalIdentity
                    }
                }
            } // Canonical v3 has no issuer/subject unique IDs; no other trailing fields.
            return algorithm
        }
        guard tbsAlgorithm.encodedBytes.elementsEqual(parts.1.encodedBytes) else {
            throw Rejection.invalidCanonicalIdentity
        }
        let signature = try ASN1BitString(derEncoded: parts.2)
        guard signature.paddingBits == 0, !signature.bytes.isEmpty else { throw Rejection.invalidCanonicalIdentity }
        try DER.sequence(DER.parse(signature.bytes), identifier: .sequence) { nodes in
            for _ in 0..<2 {
                let integer = try PositiveInteger(derEncoded: next(&nodes))
                guard !integer.bytes.isEmpty, integer.bytes.count <= 32 else { throw Rejection.invalidCanonicalIdentity }
            }
        } // Exactly r and s; minimal unsigned DER, nonzero and P256 width. SecKey checks range.
        guard let certificate = SecCertificateCreateWithData(nil, data as CFData),
              SecCertificateCopyData(certificate) as Data == data,
              let issuer = SecCertificateCopyNormalizedIssuerSequence(certificate),
              let subject = SecCertificateCopyNormalizedSubjectSequence(certificate),
              !(issuer as Data).isEmpty, issuer as Data == subject as Data,
              let key = SecCertificateCopyKey(certificate),
              let attributes = SecKeyCopyAttributes(key) as? [String: Any],
              attributes[kSecAttrKeyType as String] as? String == kSecAttrKeyTypeECSECPrimeRandom as String,
              attributes[kSecAttrKeyClass as String] as? String == kSecAttrKeyClassPublic as String,
              attributes[kSecAttrKeySizeInBits as String] as? Int == 256,
              SecKeyIsAlgorithmSupported(key, .verify, .ecdsaSignatureMessageX962SHA256),
              SecKeyVerifySignature(key, .ecdsaSignatureMessageX962SHA256,
                  Data(parts.0.encodedBytes) as CFData, Data(signature.bytes) as CFData, nil) else {
            throw Rejection.invalidCanonicalIdentity
        }
        // Message variant hashes the ORIGINAL complete TBS TLV once. Never
        // reserialize, prehash, normalize signature values or implement curve math.
    }

    private static func next(_ nodes: inout ASN1NodeCollection.Iterator) throws -> ASN1Node {
        guard let node = nodes.next() else { throw Rejection.invalidCanonicalIdentity }
        return node
    }
    private static func name(_ node: ASN1Node) throws {
        guard node.identifier == .sequence, case .constructed(let children) = node.content else {
            throw Rejection.invalidCanonicalIdentity
        }
        var iterator = children.makeIterator()
        guard iterator.next() != nil else { throw Rejection.invalidCanonicalIdentity }
    }
    private static func signatureAlgorithm(_ node: ASN1Node) throws {
        try DER.sequence(node, identifier: .sequence) { nodes in
            let oid = try ASN1ObjectIdentifier(derEncoded: next(&nodes))
            guard oid == ASN1ObjectIdentifier(stringLiteral: "1.2.840.10045.4.3.2") else { throw Rejection.invalidCanonicalIdentity }
        } // Parameters MUST be absent, including NULL.
    }
    private static func publicKeyInfo(_ node: ASN1Node) throws {
        try DER.sequence(node, identifier: .sequence) { nodes in
            try DER.sequence(next(&nodes), identifier: .sequence) { algorithm in
                let type = try ASN1ObjectIdentifier(derEncoded: next(&algorithm))
                let curve = try ASN1ObjectIdentifier(derEncoded: next(&algorithm))
                guard type == ASN1ObjectIdentifier(stringLiteral: "1.2.840.10045.2.1"),
                      curve == ASN1ObjectIdentifier(stringLiteral: "1.2.840.10045.3.1.7") else { throw Rejection.invalidCanonicalIdentity }
            }
            let key = try ASN1BitString(derEncoded: next(&nodes))
            guard key.paddingBits == 0, key.bytes.count == 65, key.bytes.first == 4 else {
                throw Rejection.invalidCanonicalIdentity
            }
        }
    }
}

/// Use the library's typed INTEGER decoder (minimal encoding/sign checks),
/// retaining bounded unsigned bytes instead of truncating a signature to UInt64.
private struct PositiveInteger: ASN1IntegerRepresentable {
    typealias IntegerBytes = ArraySlice<UInt8>
    static let isSigned = false
    let bytes: ArraySlice<UInt8>
    init(derIntegerBytes bytes: ArraySlice<UInt8>) throws { self.bytes = bytes }
    init(berIntegerBytes: ArraySlice<UInt8>) throws { throw CanonicalControlCertificate.Rejection.invalidCanonicalIdentity }
    func withBigEndianIntegerBytes<T>(_ body: (ArraySlice<UInt8>) throws -> T) rethrows -> T { try body(bytes) }
}
