package org.mixyard.mobile

import org.json.JSONObject

/** JVM platform-boundary double only: not Android Keystore or durable disk proof. */
class CredentialStore {
    private var saved: String? = null
    var failWrites = false
    fun load(): JSONObject? = saved?.let(::JSONObject)
    fun save(value: JSONObject) {
        check(!failWrites) { "credential-storage-failed" }
        saved = value.toString()
    }
    fun forget() {
        check(!failWrites) { "credential-storage-failed" }
        saved = null
    }
}
