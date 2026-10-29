package org.mixyard.mobile

import org.json.JSONObject
import java.net.URI
import java.security.MessageDigest
import java.security.cert.X509Certificate
import javax.net.ssl.HttpsURLConnection
import javax.net.ssl.SSLContext
import javax.net.ssl.TrustManagerFactory
import javax.net.ssl.X509TrustManager

/** No redirects, proxy login, cookie jar, local CSRF service, permissive trust or token in URLs. */
class PinnedControl(private val store: CredentialStore) {
    private val fence = SessionFence()
    private var configuration: JSONObject? = store.load()
    private var session: JSONObject? = null
    private var active: HttpsURLConnection? = null
    private val allowed = setOf("/control/pair", "/control/session", "/control/request", "/control/credentials", "/control/rotate", "/control/revoke")
    @Synchronized fun invalidate() { fence.invalidate(); session = null; active?.disconnect(); active = null }
    @Synchronized fun forget() { invalidate(); store.forget(); configuration = null }
    @Synchronized fun ticket(): Ticket = fence.ticket()
    private fun permissions(value: JSONObject) {
        val permissions = value.getJSONArray("permissions")
        val seen = mutableSetOf<String>()
        for (i in 0 until permissions.length()) require(permissions.get(i) is String && permissions.getString(i) in setOf("view", "control", "settings") && seen.add(permissions.getString(i))) { "invalid-permissions" }
    }
    private fun authority(value: JSONObject): Authority {
        val revision = value.get("revision")
        require(revision is Number && revision !is Float && revision !is Double) { "unsafe-revision" }
        return Authority(value.getString("deviceId"), value.getString("authorityId"), revision.toLong())
    }
    private fun endpoint(configuration: JSONObject): URI {
        val uri = URI(configuration.getString("endpoint"))
        require(uri.scheme == "https" && !uri.host.isNullOrBlank() && uri.rawUserInfo == null && uri.rawQuery == null && uri.rawFragment == null && uri.rawPath in listOf("", "/")) { "exact-https-origin-required" }
        require(configuration.getString("pin").matches(Regex("[a-fA-F0-9]{64}"))) { "leaf-sha256-required" }
        return uri
    }
    private fun request(config: JSONObject, path: String, body: String, ticket: Ticket, credential: Boolean): Pair<Int, String> {
        require(path in allowed)
        val origin = endpoint(config)
        val expectedPin = config.getString("pin").lowercase()
        val system = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm()).apply { init(null as java.security.KeyStore?) }.trustManagers.filterIsInstance<X509TrustManager>().single()
        val trust = object : X509TrustManager {
            override fun getAcceptedIssuers(): Array<X509Certificate> = system.acceptedIssuers
            override fun checkClientTrusted(chain: Array<X509Certificate>, type: String) = system.checkClientTrusted(chain, type)
            override fun checkServerTrusted(chain: Array<X509Certificate>, type: String) {
                system.checkServerTrusted(chain, type)
                val actual = MessageDigest.getInstance("SHA-256").digest(chain.first().encoded).joinToString("") { "%02x".format(it) }
                if (actual != expectedPin) throw java.security.cert.CertificateException("certificate-pin-mismatch")
            }
        }
        val ssl = SSLContext.getInstance("TLS").apply { init(null, arrayOf(trust), null) }
        val connection = origin.resolve(path).toURL().openConnection() as HttpsURLConnection
        synchronized(this) { fence.check(ticket); active = connection }
        try {
            connection.sslSocketFactory = ssl.socketFactory // Default hostname verifier remains enabled.
            connection.instanceFollowRedirects = false; connection.useCaches = false
            connection.connectTimeout = 5000; connection.readTimeout = 8000
            connection.requestMethod = "POST"; connection.doOutput = true
            connection.setRequestProperty("Content-Type", "application/json")
            if (credential) connection.setRequestProperty("Authorization", "Bearer " + config.getString("credential"))
            val encoded = body.toByteArray(Charsets.UTF_8)
            require(encoded.size <= 2_097_152) { "request-too-large" }
            connection.setFixedLengthStreamingMode(encoded.size)
            synchronized(this) { fence.check(ticket) }
            connection.outputStream.use { it.write(encoded) }
            val status = connection.responseCode
            require(status !in 300..399) { "redirect-denied" }
            val stream = if (status in 200..299) connection.inputStream else connection.errorStream ?: error("empty-control-error")
            val bytes = stream.use {
                val result = java.io.ByteArrayOutputStream()
                val chunk = ByteArray(8192)
                while (true) {
                    val size = it.read(chunk)
                    if (size < 0) break
                    require(size <= 2_097_152 - result.size()) { "response-too-large" }
                    result.write(chunk, 0, size)
                }
                result.toByteArray()
            }
            require(bytes.size <= 2_097_152) { "response-too-large" }
            synchronized(this) { fence.check(ticket) }
            return status to Charsets.UTF_8.newDecoder().decode(java.nio.ByteBuffer.wrap(bytes)).toString()
        } finally { connection.disconnect(); synchronized(this) { if (active === connection) active = null } }
    }
    fun pair(endpoint: String, pin: String, device: String, invitation: String, name: String, expected: Ticket) {
        val ticket: Ticket
        synchronized(this) { fence.check(expected); invalidate(); ticket = fence.ticket() }
        val config = JSONObject().put("endpoint", endpoint).put("pin", pin).put("deviceId", device)
        val (status, rawReply) = request(config, "/control/pair", JSONObject().put("deviceId", device).put("invitation", invitation).put("name", name).toString(), ticket, false)
        val reply = JSONObject(rawReply)
        require(status == 200 && reply.getString("deviceId") == device) { "pairing-failed" }
        permissions(reply)
        require(reply.getString("credential").isNotBlank() && reply.getString("credentialId").isNotBlank()) { "invalid-credential" }
        require(reply.getString("credential").length <= 4096 && reply.getString("credential").all { it.code in 33..126 }) { "invalid-credential" }
        config.put("credential", reply.getString("credential")).put("credentialId", reply.getString("credentialId"))
        synchronized(this) { fence.check(ticket); store.save(config); configuration = config }
    }
    fun invoke(method: String, payload: Any, expected: Ticket): JSONObject {
        synchronized(this) { fence.check(expected) }
        if (method == "disconnect") { invalidate(); return JSONObject() }
        val config: JSONObject; val ticket: Ticket
        synchronized(this) {
            fence.check(expected)
            if (method == "session") invalidate()
            config = configuration ?: error("pair-in-native-settings")
            ticket = fence.ticket()
        }
        try {
            if (method == "session") {
                val (status, rawReply) = request(config, "/control/session", "{}", ticket, true)
                val value = JSONObject(rawReply)
                require(status == 200) { "session-failed-http-$status" }
                permissions(value)
                require(value.get("backendAvailable") == false && value.get("audioApplied") == false) { "unsupported-backend-version" }
                val next = authority(value)
                require(next.deviceId == config.getString("deviceId")) { "device-mismatch" }
                synchronized(this) { fence.establish(ticket, next); session = value }
                return value
            }
            require(method == "request") { "unsupported-native-command" }
            require(payload is String) { "raw-control-json-required" }
            require(payload.toByteArray(Charsets.UTF_8).size <= 2_097_152) { "request-too-large" }
            val envelope = JSONObject(payload)
            val before = ticket.authority ?: error("session-unavailable")
            require(authority(envelope) == before) { "frozen-request-identity-mismatch" }
            val path = envelope.getString("path")
            require(path.matches(Regex("/api/[a-z0-9/-]+")) && !path.contains("//") && path != "/api/session") { "path-denied" }
            require(path !in setOf("/api/automation/arm", "/api/automation/review", "/api/automation/live") && envelope.has("body")) { "remote-arm-and-review-unsupported" }
            // Inspect authority, but send and return the original JSON bytes.
            // JSONObject must never reserialize Float64 canonical payloads.
            val (status, rawReply) = request(config, "/control/request", payload, ticket, true)
            val value = JSONObject(rawReply)
            if (value.has("error")) {
                require(status in 400..599) { "invalid-control-error" }
                val code = value.getJSONObject("error").getString("code")
                if (status == 401 || status == 409 || code == "invalid-origin") invalidate()
                return JSONObject().put("status", status).put("json", rawReply)
            }
            require(value.getInt("status") == status && value.has("body")) { "status-envelope-mismatch" }
            synchronized(this) { fence.accept(ticket, authority(value)) }
            return JSONObject().put("status", status).put("json", rawReply)
        } catch (failure: Exception) { synchronized(this) { if (fence.ticket().epoch == ticket.epoch) invalidate() }; throw failure }
    }
}
