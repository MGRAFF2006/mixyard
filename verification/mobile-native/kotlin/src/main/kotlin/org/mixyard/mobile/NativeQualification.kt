package org.mixyard.mobile

import org.json.JSONObject

/** JSONL test driver; secrets enter stdin and are never included in replies. */
fun main() {
    val vault = CredentialStore()
    var client = PinnedControl(vault)
    val captured = mutableMapOf<String, Ticket>()
    var legacyCredential: String? = null
    println("{\"event\":\"ready\"}"); System.out.flush()
    generateSequence(::readLine).forEach { line ->
        val reply = JSONObject()
        try {
            val command = JSONObject(line)
            val expected = if (command.has("expected")) captured.getValue(command.getString("expected")) else client.ticket()
            when (command.getString("op")) {
                "pair" -> client.pair(command.getString("endpoint"), command.getString("trustRecord"), command.getString("deviceId"), command.getString("invitation"), "Owned native qualification", expected)
                "evaluateSystemCertificate" -> {
                    val record = OobRecord.parse(command.getString("trustRecord"), command.getString("deviceId"))
                    val anchor = org.mixyard.mobile.http.OobCertificate(record.certificatePem, record.certificateSha256)
                    val system = javax.net.ssl.TrustManagerFactory.getInstance(javax.net.ssl.TrustManagerFactory.getDefaultAlgorithm()).apply { init(null as java.security.KeyStore?) }.trustManagers.filterIsInstance<javax.net.ssl.X509TrustManager>().single()
                    var found = false
                    for (certificate in system.acceptedIssuers.take(16)) {
                        val type = if (certificate.publicKey.algorithm == "EC") "ECDHE_ECDSA" else "RSA"
                        try { system.checkServerTrusted(arrayOf(certificate), type) } catch (_: Exception) { continue }
                        found = true
                        var rejected = false
                        try { anchor.trustManager().checkServerTrusted(arrayOf(certificate), type) } catch (_: Exception) { rejected = true }
                        check(rejected) { "private-anchor-accepted-unrelated-system-certificate" }
                        break
                    }
                    check(found) { "system-certificate-evaluation-unavailable" }
                    reply.put("systemCertificateObservedTrusted", true).put("privateAnchorRejected", true).put("evaluatorOnly", true)
                }
                "installLegacyFixture" -> {
                    client.invalidate(); legacyCredential = "owned-legacy-fixture-credential"
                    vault.save(JSONObject().put("endpoint", command.getString("endpoint")).put("pin", command.getString("pin")).put("deviceId", command.getString("deviceId")).put("credentialId", "owned-legacy-fixture-id").put("credential", legacyCredential))
                    client = PinnedControl(vault)
                }
                "invoke" -> {
                    val method = command.getString("method")
                    val payload: Any = if (method == "request") command.getString("payload") else JSONObject()
                    val value = client.invoke(method, payload, expected)
                    if (method == "request") {
                        reply.put("raw", value.getString("json")).put("status", value.getInt("status"))
                    } else reply.put("raw", value.toString())
                }
                "capture" -> captured[command.getString("key")] = client.ticket()
                "restartClient" -> { client.invalidate(); client = PinnedControl(vault) }
                "storageFail" -> vault.failWrites = command.getBoolean("enabled")
                "forget" -> client.forget()
                else -> error("unknown-harness-command")
            }
            reply.put("ok", true)
        } catch (failure: Exception) {
            // Only classification is returned. No endpoint, invitation, bearer,
            // arbitrary exception text or native provisioning is exported.
            reply.put("ok", false).put("failure", failure.javaClass.simpleName)
        }
        reply.put("hasAuthority", client.ticket().authority != null)
        legacyCredential?.let { reply.put("recordRetained", vault.load()?.optString("credential") == it) }
        println(reply.toString())
        System.out.flush()
    }
}
