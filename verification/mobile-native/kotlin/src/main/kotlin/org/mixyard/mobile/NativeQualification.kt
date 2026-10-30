package org.mixyard.mobile

import org.json.JSONObject

/** JSONL test driver; secrets enter stdin and are never included in replies. */
fun main() {
    val vault = CredentialStore()
    var client = PinnedControl(vault)
    val captured = mutableMapOf<String, Ticket>()
    println("{\"event\":\"ready\"}"); System.out.flush()
    generateSequence(::readLine).forEach { line ->
        val reply = JSONObject()
        try {
            val command = JSONObject(line)
            val expected = if (command.has("expected")) captured.getValue(command.getString("expected")) else client.ticket()
            when (command.getString("op")) {
                "pair" -> client.pair(command.getString("endpoint"), command.getString("pin"), command.getString("certificatePem"), command.getString("deviceId"), command.getString("invitation"), "Owned native qualification", expected)
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
        println(reply.toString())
        System.out.flush()
    }
}
