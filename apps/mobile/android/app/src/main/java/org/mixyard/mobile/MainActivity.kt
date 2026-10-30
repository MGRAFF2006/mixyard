package org.mixyard.mobile

import android.app.AlertDialog
import android.app.Activity
import android.os.Bundle
import android.text.InputType
import android.webkit.JsResult
import android.webkit.WebChromeClient
import android.graphics.Bitmap
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
    private val entry = "https://appassets.androidplatform.net/assets/controller/index.html"
    private var resumed = false
    private var documentGeneration = 0L
    private var confirmation: (() -> Unit)? = null
    private fun cancelConfirmation() { confirmation?.invoke() }
    private fun confirm(view: WebView, url: String, message: String, result: JsResult): Boolean {
        // Android exposes the requesting URL, not frame identity. Top-level
        // navigation is pinned below; the bundled page CSP forbids all frames.
        if (url != entry || view !== web || web.url != entry || !resumed || isFinishing || isDestroyed || message.length > 4096 || confirmation != null) {
            result.cancel(); return true
        }
        val document = documentGeneration
        val epoch = control.ticket().epoch
        var finished = false
        var dialog: AlertDialog? = null
        fun finish(approve: Boolean) {
            if (finished) return
            finished = true; confirmation = null
            val current = approve && resumed && !isFinishing && !isDestroyed &&
                documentGeneration == document && web.url == entry && control.ticket().epoch == epoch
            dialog?.dismiss()
            if (current) result.confirm() else result.cancel()
        }
        confirmation = { finish(false) }
        val shown = AlertDialog.Builder(this).setTitle("Confirm change").setMessage(message)
            .setNegativeButton(android.R.string.cancel) { _, _ -> finish(false) }
            .setPositiveButton(android.R.string.ok) { _, _ -> finish(true) }
            .setOnCancelListener { finish(false) }.create()
        dialog = shown
        shown.setOnDismissListener { finish(false) }
        try { shown.show() }
        catch (_: android.view.WindowManager.BadTokenException) { finish(false) }
        return true
    }
    private val endpoint = NativeEndpoint()
    private val worker = ThreadPoolExecutor(1, 1, 0, TimeUnit.SECONDS, ArrayBlockingQueue(1))
    private fun stop(reason: StopReason) {
        cancelConfirmation(); endpoint.stop(reason); control.invalidate()
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
        val trustRecord = field("Public OOB trust JSON from owner").apply { isSingleLine = false; minLines = 2; maxLines = 4 }
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
        web.webChromeClient = object : WebChromeClient() {
            override fun onJsConfirm(view: WebView, url: String, message: String, result: JsResult) = confirm(view, url, message, result)
        }
        web.webViewClient = object : WebViewClient() {
            override fun onPageStarted(view: WebView, url: String, favicon: Bitmap?) {
                cancelConfirmation(); documentGeneration++
            }
            override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse =
                assets.shouldInterceptRequest(request.url) ?: WebResourceResponse("text/plain", "UTF-8", 403, "Forbidden", emptyMap(), "Blocked".byteInputStream())
            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest) = request.url.toString() != entry
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
                try { response.put("value", control.invoke(value.getString("method"), value.get("payload"), expected)) }
                catch (_: Exception) { response.put("error", "Native control failed or session changed. Verify OOB certificate provisioning (legacy credentials are retained), then Refresh; no replay.") }
                runOnUiThread {
                    if (control.ticket().epoch != expected.epoch) cancelConfirmation()
                    if (!isDestroyed && ticket == System.identityHashCode(web)) reply.postMessage(response.toString())
                }
            } } catch (_: java.util.concurrent.RejectedExecutionException) {
                reply.postMessage(JSONObject().put("id", id).put("error", "Native request busy. Retry explicitly.").toString())
            }
        }
        layout.addView(Button(this).apply { text = "Pair verified device"; setOnClickListener {
            stop(StopReason.PEER_REVOKED)
            val values = listOf(origin.text.toString(), trustRecord.text.toString(), device.text.toString(), invitation.text.toString(), name.text.toString())
            invitation.text.clear()
            val expected = control.ticket()
            try { worker.execute {
                val result = try { control.pair(values[0], values[1], values[2], values[3], values[4], expected); "Paired. Refresh controller explicitly." }
                    catch (_: Exception) { "Pair failed. Verify the OOB certificate, fingerprint, target and invitation. Existing local credentials are retained." }
                runOnUiThread { status.text = result }
            } } catch (_: java.util.concurrent.RejectedExecutionException) { status.text = "Native request busy. Pair again explicitly." }
        } })
        layout.addView(Button(this).apply { text = "Disconnect and forget local credential"; setOnClickListener {
            stop(StopReason.PEER_REVOKED)
            try { control.forget(); status.text = "Local credential removed. Revoke this controller on the host separately." }
            catch (_: Exception) { status.text = "Credential removal failed. Clear app storage to repair." }
        } })
        layout.addView(Button(this).apply { text = "Open controller / Refresh connection"; setOnClickListener { cancelConfirmation(); control.invalidate(); web.reload() } })
        layout.addView(status)
        layout.addView(web, LinearLayout.LayoutParams(-1, 0, 1f))
        setContentView(layout)
        web.loadUrl(entry)
    }
    override fun onResume() { super.onResume(); resumed = true }
    override fun onPause() { resumed = false; cancelConfirmation(); super.onPause() }
    override fun onStop() { if (::web.isInitialized && ::control.isInitialized) stop(StopReason.BACKGROUND); super.onStop() }
    override fun onDestroy() { cancelConfirmation(); if (::control.isInitialized) control.invalidate(); worker.shutdownNow(); if (::web.isInitialized) web.destroy(); super.onDestroy() }
}
