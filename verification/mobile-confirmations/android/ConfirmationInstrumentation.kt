package org.mixyard.mobile

import android.app.Activity
import android.app.Instrumentation
import android.content.Intent
import android.os.Bundle
import android.view.View
import android.view.ViewGroup
import android.view.accessibility.AccessibilityNodeInfo
import android.webkit.WebView
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit

/** Hosted-owned emulator only: production activity/delegates and actual Vue App,
 * with the explicitly synthetic service boundary in the bundled UI fixture. */
class ConfirmationInstrumentation : Instrumentation() {
    private lateinit var activity: MainActivity
    private lateinit var web: WebView
    private var stage = "startup"
    private val automation get() = getUiAutomation(android.app.UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
    override fun onCreate(arguments: Bundle?) { super.onCreate(arguments); start() }
    private fun checkOwned(value: Boolean) { check(value) { "Owned UI assertion at $stage" } }
    private fun <T> ui(action: () -> T): T {
        var result: Result<T>? = null
        runOnMainSync { result = runCatching(action) }
        return result!!.getOrThrow()
    }
    private fun findWeb(view: View): WebView? = if (view is WebView) view else if (view is ViewGroup)
        (0 until view.childCount).firstNotNullOfOrNull { findWeb(view.getChildAt(it)) } else null
    private fun js(script: String): String {
        val replies = ArrayBlockingQueue<String>(1)
        ui { web.evaluateJavascript(script) { replies.offer(it) } }
        return replies.poll(10, TimeUnit.SECONDS) ?: error("Owned JS deadline at $stage")
    }
    private fun fire(script: String) { ui { web.evaluateJavascript(script, null) } }
    private fun awaitState(predicate: () -> Boolean) {
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(10)
        while (!predicate()) { checkOwned(System.nanoTime() < deadline); Thread.sleep(20) }
    }
    private fun button(label: String) = "[...document.querySelectorAll('button')].find(b=>b.textContent.trim()==${org.json.JSONObject.quote(label)}).click()"
    private fun observed() = js("document.querySelector('#observed')?.textContent")
    private fun nativeAnswer(label: String) {
        var button: AccessibilityNodeInfo? = null
        awaitState {
            val root = automation.rootInActiveWindow ?: return@awaitState false
            if (root.findAccessibilityNodeInfosByText("Confirm change").isEmpty()) return@awaitState false
            button = root.findAccessibilityNodeInfosByText(label).firstOrNull { it.text?.toString() == label && it.isClickable }
            button != null
        }
        checkOwned(button!!.performAction(AccessibilityNodeInfo.ACTION_CLICK))
    }
    override fun onStart() {
        var outcome = Activity.RESULT_CANCELED
        var detail = "failed-startup"
        try {
            activity = startActivitySync(Intent(targetContext, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) as MainActivity
            web = ui { findWeb(activity.window.decorView) ?: error("Owned WebView unavailable") }
            awaitState { js("document.querySelector('#observed')!=null") == "true" }
            stage = "dirty-replacement-cancel"
            fire(button("Load simulated example")); awaitState { !observed().contains("gain=none") }
            val initial = observed()
            js("(()=>{const s=document.querySelector('input[aria-label=\"mic to Call gain\"]');s.value='0.37';s.dispatchEvent(new Event('input',{bubbles:true}));return true})()")
            fire(button("Load simulated example")); nativeAnswer("Cancel")
            checkOwned(observed().contains("gain=0.37"))
            stage = "dirty-replacement-confirm"
            fire(button("Load simulated example")); nativeAnswer("OK")
            awaitState { observed() == initial }
            stage = "codec-removal-cancel"
            fire(button("Network codecs")); awaitState { observed().contains("drafts=1") }
            fire(button("Add stream draft")); awaitState { observed().contains("drafts=2") }
            js("(()=>{const s=[...document.querySelectorAll('label')].find(l=>l.textContent.trim()=='Stream ID').querySelector('input');s.value='42';s.dispatchEvent(new Event('input',{bubbles:true}));return true})()")
            fire(button("Remove draft")); nativeAnswer("Cancel"); checkOwned(observed().contains("drafts=2"))
            stage = "codec-removal-confirm"
            fire(button("Remove draft")); nativeAnswer("OK"); awaitState { observed().contains("drafts=1") }
            stage = "pause-cancels-pending"
            fire(button("Routing"))
            fire(button("Load simulated example"))
            awaitState { automation.rootInActiveWindow?.findAccessibilityNodeInfosByText("Confirm change")?.isNotEmpty() == true }
            ui { callActivityOnPause(activity) }
            awaitState { automation.rootInActiveWindow?.findAccessibilityNodeInfosByText("Confirm change")?.isEmpty() != false }
            ui { callActivityOnResume(activity) }
            stage = "untrusted-origin-denied"
            ui { web.loadDataWithBaseURL("https://untrusted.invalid/", "<p id='answer'></p><script>document.querySelector('#answer').textContent=String(confirm('untrusted'))</script>", "text/html", "UTF-8", null) }
            awaitState { js("document.querySelector('#answer')?.textContent") == "\"false\"" }
            outcome = Activity.RESULT_OK; detail = "passed"
        } catch (_: Throwable) { detail = "failed-$stage" }
        finally {
            if (::activity.isInitialized) {
                try { ui { activity.finish() } }
                catch (_: Throwable) { outcome = Activity.RESULT_CANCELED; detail = "failed-owned-disposal" }
            }
        }
        finish(outcome, Bundle().apply { putString("mixyard-confirmations", detail) })
    }
}
