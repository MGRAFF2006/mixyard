import Foundation
import Security
import CryptoKit
import MobileCore

// Each exchange has an ephemeral session, an exact origin, bounded receive data, no redirects and normal PKI + leaf pin.
private final class PinnedExchange: NSObject, URLSessionDataDelegate {
    let endpoint: URL; let pin: String
    private var buffer = Data(); private var status = 0
    private var continuation: CheckedContinuation<(Int, String), Error>?
    private var session: URLSession?
    init(endpoint: URL, pin: String) { self.endpoint = endpoint; self.pin = pin.lowercased() }
    func run(_ request: URLRequest) async throws -> (Int, String) {
        try await withCheckedThrowingContinuation { continuation in
            self.continuation = continuation
            let config = URLSessionConfiguration.ephemeral
            config.httpShouldSetCookies = false; config.urlCache = nil; config.requestCachePolicy = .reloadIgnoringLocalCacheData
            config.timeoutIntervalForRequest = 8; config.timeoutIntervalForResource = 12
            let session = URLSession(configuration: config, delegate: self, delegateQueue: nil)
            self.session = session; session.dataTask(with: request).resume()
        }
    }
    func cancel() { session?.invalidateAndCancel() }
    func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge, completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              challenge.protectionSpace.host == endpoint.host,
              challenge.protectionSpace.port == (endpoint.port ?? 443),
              let trust = challenge.protectionSpace.serverTrust,
              SecTrustEvaluateWithError(trust, nil), let certificate = SecTrustGetCertificateAtIndex(trust, 0) else {
            completionHandler(.cancelAuthenticationChallenge, nil); return
        }
        let hash = SHA256.hash(data: SecCertificateCopyData(certificate) as Data).map { String(format: "%02x", $0) }.joined()
        guard hash == pin else { completionHandler(.cancelAuthenticationChallenge, nil); return }
        completionHandler(.useCredential, URLCredential(trust: trust))
    }
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) { completionHandler(nil) }
    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse, completionHandler: @escaping (URLSession.ResponseDisposition) -> Void) {
        guard let http = response as? HTTPURLResponse, !(300...399).contains(http.statusCode), response.expectedContentLength <= 2_097_152 else { completionHandler(.cancel); return }
        status = http.statusCode; completionHandler(.allow)
    }
    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        guard data.count <= 2_097_152 - buffer.count else { dataTask.cancel(); return }
        buffer.append(data)
    }
    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        defer { continuation = nil; self.session = nil; session.finishTasksAndInvalidate() }
        if let error { continuation?.resume(throwing: error); return }
        do {
            guard let body = String(data: buffer, encoding: .utf8) else { throw MobileError.invalid("control-utf8") }
            continuation?.resume(returning: (status, body))
        } catch { continuation?.resume(throwing: error) }
    }
}

@MainActor final class ControlClient {
    private var fence = SessionFence()
    private let store: any CredentialVault
    private var configuration: Provisioning?
    private var active: PinnedExchange?
    private var busy = false
    var ticket: Ticket { fence.ticket }
    init(store: any CredentialVault = CredentialStore()) throws { self.store = store; configuration = try store.load() }
    private func object(_ json: String) throws -> [String: Any] {
        guard let value = try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any] else { throw MobileError.invalid("control-json") }
        return value
    }
    func invalidate() { fence.invalidate(); active?.cancel(); active = nil }
    func forget() throws { invalidate(); try store.forget(); configuration = nil }
    private func permissions(_ value: [String: Any]) throws {
        guard let p = value["permissions"] as? [String], Set(p).count == p.count,
              p.allSatisfy({ ["view", "control", "settings"].contains($0) }) else { throw MobileError.invalid("permissions") }
    }
    private func authority(_ value: [String: Any]) throws -> Authority {
        guard let device = value["deviceId"] as? String, let authority = value["authorityId"] as? String,
              let revision = value["revision"] as? NSNumber, CFGetTypeID(revision) != CFBooleanGetTypeID(),
              revision.doubleValue.isFinite, revision.doubleValue >= 0,
              revision.doubleValue <= 9_007_199_254_740_991, revision.doubleValue.rounded(.down) == revision.doubleValue else { throw MobileError.invalid("unsafe-authority") }
        return try Authority(deviceId: device, authorityId: authority, revision: revision.uint64Value)
    }
    private func request(config: Provisioning, path: String, body: String, ticket: Ticket, credential: Bool) async throws -> (Int, String) {
        let paths = ["/control/pair", "/control/session", "/control/request", "/control/credentials", "/control/rotate", "/control/revoke"]
        guard paths.contains(path), let endpoint = URL(string: config.endpoint), endpoint.scheme == "https", endpoint.host != nil,
              endpoint.user == nil, endpoint.password == nil, endpoint.query == nil, endpoint.fragment == nil,
              ["", "/"].contains(endpoint.path), config.pin.range(of: "^[a-fA-F0-9]{64}$", options: .regularExpression) != nil,
              let url = URL(string: path, relativeTo: endpoint)?.absoluteURL else { throw MobileError.invalid("exact-pinned-https-origin-required") }
        try fence.check(ticket)
        var request = URLRequest(url: url); request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if credential { request.setValue("Bearer \(config.credential)", forHTTPHeaderField: "Authorization") }
        let bytes = Data(body.utf8)
        guard bytes.count <= 2_097_152 else { throw MobileError.invalid("request-too-large") }
        request.httpBody = bytes
        let exchange = PinnedExchange(endpoint: endpoint, pin: config.pin); active = exchange
        defer { if active === exchange { active = nil } }
        let result = try await exchange.run(request)
        try fence.check(ticket)
        return result
    }
    func pair(endpoint: String, pin: String, device: String, invitation: String, name: String, expected: Ticket) async throws {
        try fence.check(expected); guard !busy else { throw MobileError.invalid("native-busy") }
        invalidate(); let ticket = fence.ticket; busy = true; defer { busy = false }
        let config = Provisioning(endpoint: endpoint, pin: pin, deviceId: device, credentialId: "", credential: "")
        let pairBody = try JSONSerialization.data(withJSONObject: ["deviceId": device, "invitation": invitation, "name": name])
        let (status, rawReply) = try await request(config: config, path: "/control/pair", body: String(decoding: pairBody, as: UTF8.self), ticket: ticket, credential: false)
        let value = try object(rawReply)
        try permissions(value)
        guard status == 200, value["deviceId"] as? String == device, let credentialId = value["credentialId"] as? String,
              !credentialId.isEmpty, let credential = value["credential"] as? String, !credential.isEmpty,
              credential.utf8.count <= 4096, credential.utf8.allSatisfy({ (33...126).contains($0) }) else { throw MobileError.invalid("pair-failed") }
        try fence.check(ticket)
        let next = Provisioning(endpoint: endpoint, pin: pin, deviceId: device, credentialId: credentialId, credential: credential)
        try store.save(next); configuration = next
    }
    func invoke(method: String, payload: Any, expected: Ticket) async throws -> [String: Any] {
        try fence.check(expected)
        if method == "disconnect" { invalidate(); return [:] }
        guard !busy else { throw MobileError.invalid("native-busy") }
        if method == "session" { invalidate() }
        guard let config = configuration else { throw MobileError.invalid("pair-in-native-settings") }
        let ticket = fence.ticket; busy = true; defer { busy = false }
        do {
            if method == "session" {
                let (status, rawReply) = try await request(config: config, path: "/control/session", body: "{}", ticket: ticket, credential: true)
                let value = try object(rawReply)
                try permissions(value)
                guard status == 200, let backend = value["backendAvailable"] as? NSNumber, CFGetTypeID(backend) == CFBooleanGetTypeID(), !backend.boolValue,
                      let applied = value["audioApplied"] as? NSNumber, CFGetTypeID(applied) == CFBooleanGetTypeID(), !applied.boolValue else { throw MobileError.invalid("unsupported-backend-version") }
                let next = try authority(value)
                guard next.deviceId == config.deviceId else { throw MobileError.invalid("device-mismatch") }
                try fence.establish(ticket, next); return value
            }
            guard method == "request", let rawRequest = payload as? String else { throw MobileError.invalid("raw-control-json-required") }
            guard rawRequest.utf8.count <= 2_097_152 else { throw MobileError.invalid("request-too-large") }
            let envelope = try object(rawRequest)
            guard let before = ticket.authority, let path = envelope["path"] as? String,
                  path.range(of: "^/api/[a-z0-9/-]+$", options: .regularExpression) != nil, !path.contains("//"), path != "/api/session", envelope["body"] != nil, !["/api/automation/arm", "/api/automation/review", "/api/automation/live"].contains(path) else { throw MobileError.invalid("request-path-denied") }
            guard try authority(envelope) == before else { throw MobileError.invalid("frozen-request-identity-mismatch") }
            let (status, rawReply) = try await request(config: config, path: "/control/request", body: rawRequest, ticket: ticket, credential: true)
            let value = try object(rawReply)
            if let failure = value["error"] as? [String: Any] {
                guard (400...599).contains(status), let code = failure["code"] as? String, failure["message"] is String else { throw MobileError.invalid("control-error") }
                if status == 401 || status == 409 || code == "invalid-origin" { invalidate() }
                return ["status": status, "json": rawReply]
            }
            guard let canonical = value["status"] as? Int, canonical == status, value["body"] != nil else { throw MobileError.invalid("protocol-or-envelope-error") }
            try fence.accept(ticket, authority(value)); return ["status": status, "json": rawReply]
        } catch { if fence.ticket.epoch == ticket.epoch { invalidate() }; throw error }
    }
}
