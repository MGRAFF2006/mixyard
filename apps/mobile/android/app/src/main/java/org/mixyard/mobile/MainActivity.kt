package org.mixyard.mobile

import android.app.Activity
import android.os.Bundle
import android.text.InputType
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView
import androidx.webkit.WebViewAssetLoader
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import org.json.JSONObject
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

class MainActivity : Activity() {
    private lateinit var web: WebView
    private lateinit var control: PinnedControl
    private lateinit var status: TextView
    private val endpoint = NativeEndpoint()
    private val worker = ThreadPoolExecutor(1, 1, 0, TimeUnit.SECONDS, ArrayBlockingQueue(1))
    private fun stop(reason: StopReason) {
        endpoint.stop(reason); control.invalidate()
        web.evaluateJavascript("window.mixyardInvalidate?.()", null)
    }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        status = TextView(this).apply { text = "Audio unavailable: native core/transport not linked. Pair controls stored intent only." }
        val layout = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        fun field(hintText: String, secret: Boolean = false) = EditText(this).apply {
            hint = hintText; isSingleLine = true
            inputType = if (secret) InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD else InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
            importantForAutofill = android.view.View.IMPORTANT_FOR_AUTOFILL_NO
            layout.addView(this)
        }
        val origin = field("Exact trusted HTTPS origin")
        val pin = field("Verified leaf certificate SHA-256 hex")
        val device = field("Expected device ID")
        val invitation = field("One-time invitation", true)
        val name = field("Controller name")
        control = try { PinnedControl(CredentialStore(this)) } catch (_: Exception) {
            status.text = "Native credential store unavailable. Clear app storage to repair; no connection attempted."
            layout.addView(status); setContentView(layout); return
        }
        web = WebView(this)
        val assets = WebViewAssetLoader.Builder().addPathHandler("/assets/", WebViewAssetLoader.AssetsPathHandler(this)).build()
        web.settings.javaScriptEnabled = true
        web.settings.domStorageEnabled = false; web.settings.allowFileAccess = false; web.settings.allowContentAccess = false
        web.settings.mixedContentMode = android.webkit.WebSettings.MIXED_CONTENT_NEVER_ALLOW
        web.webViewClient = object : WebViewClient() {
            override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse =
                assets.shouldInterceptRequest(request.url) ?: WebResourceResponse("text/plain", "UTF-8", 403, "Forbidden", emptyMap(), "Blocked".byteInputStream())
            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest) = request.url.toString() != "https://appassets.androidplatform.net/assets/controller/index.html"
        }
        if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) {
            status.text = "Unsupported WebView: update Android System WebView. Native control bridge disabled."
            layout.addView(status); setContentView(layout); return
        }
        WebViewCompat.addWebMessageListener(web, "mixyardNative", setOf("https://appassets.androidplatform.net")) { _, message, source, mainFrame, reply ->
            if (!mainFrame || source.scheme != "https" || source.host != "appassets.androidplatform.net" || source.port !in listOf(-1, 443)) return@addWebMessageListener
            val raw = try { message.data } catch (_: IllegalStateException) { null } ?: return@addWebMessageListener
            if (raw.toByteArray().size > 2_097_152) { control.invalidate(); return@addWebMessageListener }
            val value = try { JSONObject(raw) } catch (_: Exception) { return@addWebMessageListener }
            val id = value.optLong("id", -1)
            val ticket = System.identityHashCode(web)
            val expected = control.ticket()
            try { worker.execute {
                val response = JSONObject().put("id", id)
                try { response.put("value", control.invoke(value.getString("method"), value.getJSONObject("payload"), expected)) }
                catch (_: Exception) { response.put("error", "Native control failed or session changed. Verify pairing/trust, then Refresh; no replay.") }
                runOnUiThread { if (!isDestroyed && ticket == System.identityHashCode(web)) reply.postMessage(response.toString()) }
            } } catch (_: java.util.concurrent.RejectedExecutionException) {
                reply.postMessage(JSONObject().put("id", id).put("error", "Native request busy. Retry explicitly.").toString())
            }
        }
        layout.addView(Button(this).apply { text = "Pair verified device"; setOnClickListener {
            stop(StopReason.PEER_REVOKED)
            val values = listOf(origin.text.toString(), pin.text.toString(), device.text.toString(), invitation.text.toString(), name.text.toString())
            invitation.text.clear()
            val expected = control.ticket()
            try { worker.execute {
                val result = try { control.pair(values[0], values[1], values[2], values[3], values[4], expected); "Paired. Refresh controller explicitly." }
                    catch (_: Exception) { "Pair failed. Check invitation, verified identity, TLS certificate and fingerprint." }
                runOnUiThread { status.text = result }
            } } catch (_: java.util.concurrent.RejectedExecutionException) { status.text = "Native request busy. Pair again explicitly." }
        } })
        layout.addView(Button(this).apply { text = "Disconnect and forget local credential"; setOnClickListener {
            stop(StopReason.PEER_REVOKED)
            try { control.forget(); status.text = "Local credential removed. Revoke this controller on the host separately." }
            catch (_: Exception) { status.text = "Credential removal failed. Clear app storage to repair." }
        } })
        layout.addView(Button(this).apply { text = "Open controller / Refresh connection"; setOnClickListener { control.invalidate(); web.reload() } })
        layout.addView(status)
        layout.addView(web, LinearLayout.LayoutParams(-1, 0, 1f))
        setContentView(layout)
        web.loadUrl("https://appassets.androidplatform.net/assets/controller/index.html")
    }
    override fun onStop() { if (::web.isInitialized && ::control.isInitialized) stop(StopReason.BACKGROUND); super.onStop() }
    override fun onDestroy() { if (::control.isInitialized) control.invalidate(); worker.shutdownNow(); if (::web.isInitialized) web.destroy(); super.onDestroy() }
}
