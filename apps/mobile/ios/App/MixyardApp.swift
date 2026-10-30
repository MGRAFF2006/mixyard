import SwiftUI
import WebKit
import UIKit
import AVFAudio
import MobileCore

@main struct MixyardApp: App { var body: some Scene { WindowGroup { MobileView() } } }
@MainActor final class Shell: NSObject, ObservableObject, WKScriptMessageHandlerWithReply, WKNavigationDelegate, WKUIDelegate {
    @Published var status = "Audio unavailable: shared native core/transport not linked. Controller edits stored intent only."
    private var control: ControlClient?
    let endpoint = NativeEndpoint()
    private(set) var web: WKWebView!
    private var root: URL?
    weak var presenter: UIViewController?
    private var documentGeneration: UInt64 = 0
    #if OWNED_CONFIRMATION_EVIDENCE
    // Only the disposable UI qualification build enables this read-only probe.
    // It never starts the app, changes its CSP, answers a dialog or writes data.
    @Published private(set) var ownedPageEvidence = "Owned page evidence pending"
    private var ownedNavigation = "created"
    private static let ownedObservationScript = """
    (() => {
      const state = {script: 'none', csp: 'none', errorName: 'none', line: 0, column: 0, moduleState: 'pending', errorRole: 'unknown', resources: {'classic-probe': 'pending', 'module-probe': 'pending'}};
      const name = value => ['Error', 'TypeError', 'ReferenceError', 'SyntaxError', 'RangeError', 'EvalError', 'URIError', 'AggregateError'].includes(value) ? value : 'other';
      const position = value => Number.isInteger(value) ? Math.max(0, Math.min(10000000, value)) : 0;
      window.__mixyardOwnedEvidence = state;
      addEventListener('error', event => {
        const resource = event.target instanceof HTMLScriptElement;
        const role = ['app', 'classic-probe', 'module-probe'].includes(event.target?.dataset?.ownedRole) ? event.target.dataset.ownedRole : 'unknown';
        if (resource && role in state.resources) { state.resources[role] = 'failed'; return; }
        if (resource && role === 'app') state.moduleState = 'failed';
        // Preserve the first non-probe error; telemetry cannot overwrite it.
        if (state.script !== 'none') return;
        state.script = resource ? 'script-resource' : 'runtime'; state.errorRole = role;
        state.errorName = name(event.error?.name);
        state.line = position(event.lineno); state.column = position(event.colno);
      }, true);
      addEventListener('load', event => {
        if (!(event.target instanceof HTMLScriptElement)) return;
        const role = event.target.dataset?.ownedRole;
        if (role in state.resources) state.resources[role] = 'loaded';
        if (role === 'app') state.moduleState = 'loaded';
      }, true);
      addEventListener('unhandledrejection', event => { if (state.script === 'none') { state.script = 'rejection'; state.errorName = name(event.reason?.name); } });
      addEventListener('securitypolicyviolation', event => {
        const allowed = ['script-src-elem', 'script-src', 'style-src-elem', 'style-src', 'connect-src', 'default-src'];
        state.csp = allowed.includes(event.effectiveDirective) ? event.effectiveDirective : 'other';
      });
    })();
    """
    private static func checkedOwnedResource(_ raw: Any?) -> [String: Any]? {
        guard let value = raw as? [String: Any], Set(value.keys) == Set(["load", "requestComplete", "executed", "errorClass"]),
              let load = value["load"] as? String, ["pending", "loaded", "failed"].contains(load),
              let complete = value["requestComplete"] as? Bool, complete == (load != "pending"),
              let executed = value["executed"] as? Bool,
              let errorClass = value["errorClass"] as? String, errorClass == (load == "failed" ? "resource-error" : "none") else { return nil }
        return ["load":load, "requestComplete":complete, "executed":executed, "errorClass":errorClass]
    }
    func sampleOwnedPageEvidence() {
        let generation = documentGeneration
        web.evaluateJavaScript("""
        JSON.stringify({
          ready: document.readyState,
          marker: !!document.getElementById('observed'),
          tools: !!document.querySelector('nav[aria-label="Owned fixture controls"]'),
          buttons: Math.min(1000, document.querySelectorAll('nav[aria-label="Owned fixture controls"] button').length),
          appChildren: Math.min(1000, document.getElementById('app')?.childElementCount ?? 0),
          script: window.__mixyardOwnedEvidence?.script ?? 'unobserved',
          csp: window.__mixyardOwnedEvidence?.csp ?? 'unobserved',
          errorName: window.__mixyardOwnedEvidence?.errorName ?? 'unobserved',
          line: window.__mixyardOwnedEvidence?.line ?? 0,
          column: window.__mixyardOwnedEvidence?.column ?? 0,
          moduleState: window.__mixyardOwnedEvidence?.moduleState ?? 'unobserved',
          boot: window.__mixyardOwnedFixturePhase ?? 'unentered',
          errorRole: window.__mixyardOwnedEvidence?.errorRole ?? 'unknown',
          resourceProbes: Object.fromEntries(['classic-probe', 'module-probe'].map(role => {
            const status = window.__mixyardOwnedEvidence?.resources?.[role] ?? 'pending';
            const executed = role === 'classic-probe' ? window.__mixyardOwnedClassicProbeExecuted === true : window.__mixyardOwnedModuleProbeExecuted === true;
            return [role, {load: status, requestComplete: status !== 'pending', executed, errorClass: status === 'failed' ? 'resource-error' : 'none'}];
          })),
          capabilities: { Response: typeof Response === 'function', fetch: typeof fetch === 'function',
            AbortController: typeof AbortController === 'function', TextEncoder: typeof TextEncoder === 'function',
            cryptoRandomUUID: typeof globalThis.crypto?.randomUUID === 'function', matchMedia: typeof matchMedia === 'function',
            moduleScript: 'noModule' in HTMLScriptElement.prototype }

        })
        """) { [weak self] value, _ in
            let encoded = value as? String
            Task { @MainActor [weak self] in
                guard let self, self.documentGeneration == generation else { return }
                var evidence: [String: Any] = ["navigation": self.ownedNavigation,
                    "indexPresent": self.root.map { FileManager.default.fileExists(atPath: $0.appendingPathComponent("index.html").path) } ?? false,
                    "loading": self.web.isLoading, "document": "unavailable"]
                if let root = self.root, let names = try? FileManager.default.contentsOfDirectory(atPath: root.appendingPathComponent("assets").path) {
                    evidence["scriptAssets"] = min(1000, names.filter { $0.hasSuffix(".js") }.count)
                }
                if let encoded, let bytes = encoded.data(using: .utf8), bytes.count <= 2048,
                   let fields = (try? JSONSerialization.jsonObject(with: bytes)) as? [String: Any],
                   Set(fields.keys) == Set(["ready", "marker", "tools", "buttons", "appChildren", "script", "csp", "errorName", "line", "column", "moduleState", "boot", "capabilities", "errorRole", "resourceProbes"]),
                   let ready = fields["ready"] as? String, ["loading", "interactive", "complete"].contains(ready),
                   let marker = fields["marker"] as? Bool, let tools = fields["tools"] as? Bool,
                   let buttons = fields["buttons"] as? Int, (0...1000).contains(buttons),
                   let children = fields["appChildren"] as? Int, (0...1000).contains(children),
                   let script = fields["script"] as? String, ["none", "runtime", "rejection", "script-resource", "unobserved"].contains(script),
                   let csp = fields["csp"] as? String, ["none", "script-src-elem", "script-src", "style-src-elem", "style-src", "connect-src", "default-src", "other", "unobserved"].contains(csp),
                   let errorName = fields["errorName"] as? String, ["none", "Error", "TypeError", "ReferenceError", "SyntaxError", "RangeError", "EvalError", "URIError", "AggregateError", "other", "unobserved"].contains(errorName),
                   let line = fields["line"] as? Int, (0...10000000).contains(line),
                   let column = fields["column"] as? Int, (0...10000000).contains(column),
                   let moduleState = fields["moduleState"] as? String, ["pending", "loaded", "failed", "unobserved"].contains(moduleState),
                   let boot = fields["boot"] as? String, ["unentered", "module-entered", "api-created", "mounting", "mounted", "observing", "controls-ready"].contains(boot),
                   let errorRole = fields["errorRole"] as? String, ["app", "unknown"].contains(errorRole),
                   let resources = fields["resourceProbes"] as? [String: Any], Set(resources.keys) == Set(["classic-probe", "module-probe"]),
                   let classic = Self.checkedOwnedResource(resources["classic-probe"]), let module = Self.checkedOwnedResource(resources["module-probe"]),
                   let capabilities = fields["capabilities"] as? [String: Bool],
                   Set(capabilities.keys) == Set(["Response", "fetch", "AbortController", "TextEncoder", "cryptoRandomUUID", "matchMedia", "moduleScript"]) {
                    evidence.merge(["document":ready, "marker":marker, "tools":tools, "buttons":buttons,
                                    "appChildren":children, "script":script, "csp":csp, "errorName":errorName,
                                    "line":line, "column":column, "moduleState":moduleState, "boot":boot, "capabilities":capabilities, "errorRole":errorRole,
                                    "resourceProbes":["classic-probe":classic, "module-probe":module]]) { _, new in new }
                }
                if let bytes = try? JSONSerialization.data(withJSONObject: evidence, options: [.sortedKeys]),
                   let text = String(data: bytes, encoding: .utf8) { self.ownedPageEvidence = text }
            }
        }
    }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        ownedNavigation = "finished"; sampleOwnedPageEvidence()
    }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        ownedNavigation = "failed"; sampleOwnedPageEvidence()
    }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        ownedNavigation = "provisional-failed"; sampleOwnedPageEvidence()
    }
    #endif
    private final class Answer {
        private var callback: ((Bool) -> Void)?
        init(_ callback: @escaping (Bool) -> Void) { self.callback = callback }
        func finish(_ value: Bool) { let call = callback; callback = nil; call?(value) }
    }
    private var pending: (id: UUID, reply: Answer, alert: UIAlertController)?
    func cancelConfirmation() { if let pending { finishConfirmation(pending.id, false) } }
    private func finishConfirmation(_ id: UUID, _ approved: Bool) {
        guard let current = pending, current.id == id else { return }
        pending = nil
        current.alert.dismiss(animated: false)
        current.reply.finish(approved)
    }
    private func trusted(_ frame: WKFrameInfo) -> Bool {
        guard frame.isMainFrame, let root, let url = frame.request.url else { return false }
        return url.isFileURL && url.standardizedFileURL.path == root.appendingPathComponent("index.html").standardizedFileURL.path
    }
    func webView(_ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String, initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping (Bool) -> Void) {
        guard webView === web, trusted(frame), message.utf8.count <= 4096, pending == nil,
              UIApplication.shared.applicationState == .active, let presenter, presenter.view.window != nil,
              presenter.presentedViewController == nil else { completionHandler(false); return }
        let id = UUID(), document = documentGeneration, epoch = control?.ticket.epoch
        let alert = UIAlertController(title: "Confirm change", message: message, preferredStyle: .alert)
        let answer = Answer { [weak self] approve in
            guard let self else { completionHandler(false); return }
            let current = approve && self.documentGeneration == document && self.control?.ticket.epoch == epoch &&
                UIApplication.shared.applicationState == .active && self.web.url?.standardizedFileURL == frame.request.url?.standardizedFileURL
            completionHandler(current)
        }
        pending = (id, answer, alert)
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel) { [weak self] _ in
            if let self { self.finishConfirmation(id, false) } else { answer.finish(false) }
        })
        alert.addAction(UIAlertAction(title: "Continue", style: .default) { [weak self] _ in
            if let self { self.finishConfirmation(id, true) } else { answer.finish(false) }
        })
        presenter.present(alert, animated: true)
    }
    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        cancelConfirmation(); documentGeneration &+= 1
        #if OWNED_CONFIRMATION_EVIDENCE
        ownedNavigation = "started"
        #endif
    }
    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        cancelConfirmation(); documentGeneration &+= 1
        #if OWNED_CONFIRMATION_EVIDENCE
        ownedNavigation = "content-terminated"; sampleOwnedPageEvidence()
        #endif
    }
    override init() {
        super.init()
        do { control = try ControlClient() } catch { status = "Keychain unavailable. Repair native credentials before connecting." }
        let config = WKWebViewConfiguration(); config.websiteDataStore = .nonPersistent()
        #if OWNED_CONFIRMATION_EVIDENCE
        config.userContentController.addUserScript(WKUserScript(source: Self.ownedObservationScript, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        #endif
        config.userContentController.addScriptMessageHandler(self, contentWorld: .page, name: "mixyard")
        web = WKWebView(frame: .zero, configuration: config); web.navigationDelegate = self; web.uiDelegate = self
        root = Bundle.main.resourceURL?.appendingPathComponent("controller", isDirectory: true)
        NotificationCenter.default.addObserver(self, selector: #selector(interrupted), name: AVAudioSession.interruptionNotification, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(routeChanged), name: AVAudioSession.routeChangeNotification, object: nil)
        open()
    }
    func suspend(_ reason: StopReason) {
        cancelConfirmation(); endpoint.stop(reason); control?.invalidate()
        web.evaluateJavaScript("window.mixyardInvalidate?.()", completionHandler: nil)
    }
    @objc private func interrupted() { suspend(.interruption) }
    @objc private func routeChanged() { suspend(.routeChange) }
    func open() {
        cancelConfirmation(); control?.invalidate()
        guard let root else { status = "Bundled controller assets missing. Run the mobile asset build."; return }
        web.loadFileURL(root.appendingPathComponent("index.html"), allowingReadAccessTo: root)
    }
    func forget() {
        suspend(.peerRevoked)
        do { try control?.forget(); status = "Local credential removed. Revoke this controller on the host separately." }
        catch { status = "Keychain removal failed. Repair native credential storage." }
    }
    func pair(origin: String, pin: String, certificatePem: String, device: String, invitation: String, name: String) {
        suspend(.peerRevoked)
        guard let control else { return }
        let expected = control.ticket
        Task {
            do { try await control.pair(endpoint: origin, pin: pin, certificatePem: certificatePem, device: device, invitation: invitation, name: name, expected: expected); status = "Paired. Open controller explicitly." }
            catch { status = "Pair failed. Verify the OOB certificate, fingerprint, target and invitation. Existing local credentials are retained." }
        }
    }
    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage, replyHandler: @escaping (Any?, String?) -> Void) {
        guard message.frameInfo.isMainFrame, let root, let url = message.frameInfo.request.url, url.isFileURL,
              url.standardizedFileURL.path == root.appendingPathComponent("index.html").standardizedFileURL.path,
              let request = message.body as? [String: Any], let method = request["method"] as? String,
              let payload = request["payload"], let control,
              let bytes = try? JSONSerialization.data(withJSONObject: request), bytes.count <= 2_097_152 else { replyHandler(nil, "Native bridge origin/size/command denied."); return }
        let expected = control.ticket
        Task {
            do {
                let value = try await control.invoke(method: method, payload: payload, expected: expected)
                if control.ticket.epoch != expected.epoch { cancelConfirmation() }
                replyHandler(value, nil)
            }
            catch { cancelConfirmation(); replyHandler(nil, "Native control failed or session changed. Verify OOB certificate provisioning (legacy credentials are retained), then Refresh; no replay.") }
        }
    }
    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard navigationAction.targetFrame?.isMainFrame == true, let url = navigationAction.request.url, let root,
              url.isFileURL, url.standardizedFileURL.path == root.appendingPathComponent("index.html").standardizedFileURL.path else { decisionHandler(.cancel); return }
        decisionHandler(.allow)
    }
}
@MainActor final class ControllerHost: UIViewController {
    let shell: Shell
    init(shell: Shell) { self.shell = shell; super.init(nibName: nil, bundle: nil) }
    required init?(coder: NSCoder) { fatalError("ControllerHost uses its owned shell") }
    override func loadView() { view = shell.web; shell.presenter = self }
    override func viewWillDisappear(_ animated: Bool) { shell.cancelConfirmation(); super.viewWillDisappear(animated) }
}
struct ControllerView: UIViewControllerRepresentable {
    let shell: Shell
    func makeUIViewController(context: Context) -> ControllerHost { ControllerHost(shell: shell) }
    func updateUIViewController(_ uiViewController: ControllerHost, context: Context) {}
    static func dismantleUIViewController(_ uiViewController: ControllerHost, coordinator: ()) { uiViewController.shell.cancelConfirmation() }
}
struct MobileView: View {
    @StateObject private var shell = Shell()
    @Environment(\.scenePhase) private var phase
    @State private var origin = ""
    @State private var pin = ""
    @State private var certificatePem = ""
    @State private var device = ""
    @State private var invitation = ""
    @State private var name = ""
    var body: some View {
        VStack {
            DisclosureGroup("Pair verified device") {
                TextField("Exact trusted HTTPS origin", text: $origin).textInputAutocapitalization(.never).autocorrectionDisabled()
                TextField("Leaf certificate SHA-256 hex", text: $pin).textInputAutocapitalization(.never).autocorrectionDisabled()
                TextEditor(text: $certificatePem).frame(height: 70).accessibilityLabel("Out-of-band certificate PEM (one certificate)")
                TextField("Expected device ID", text: $device).textInputAutocapitalization(.never).autocorrectionDisabled()
                SecureField("One-time invitation", text: $invitation).textContentType(.oneTimeCode)
                TextField("Controller name", text: $name)
                Button("Pair") { let secret = invitation; invitation = ""; shell.pair(origin: origin, pin: pin, certificatePem: certificatePem, device: device, invitation: secret, name: name) }
                Button("Disconnect and forget local credential") { shell.forget() }
            }.padding(.horizontal)
            Button("Open controller / Refresh connection") { shell.open() }
            Text(shell.status).font(.caption).padding(.horizontal).accessibilityAddTraits(.updatesFrequently)
            #if OWNED_CONFIRMATION_EVIDENCE
            Text(shell.ownedPageEvidence).font(.caption2).accessibilityIdentifier("owned-webkit-evidence")
            Button("Read owned page status") { shell.sampleOwnedPageEvidence() }
            #endif
            ControllerView(shell: shell)
        }.onChange(of: phase) { next in if next != .active { invitation = ""; shell.suspend(.background) } }
    }
}
