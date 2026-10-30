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
    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) { cancelConfirmation(); documentGeneration &+= 1 }
    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) { cancelConfirmation(); documentGeneration &+= 1 }
    override init() {
        super.init()
        do { control = try ControlClient() } catch { status = "Keychain unavailable. Repair native credentials before connecting." }
        let config = WKWebViewConfiguration(); config.websiteDataStore = .nonPersistent()
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
    func pair(origin: String, pin: String, device: String, invitation: String, name: String) {
        suspend(.peerRevoked)
        guard let control else { return }
        let expected = control.ticket
        Task {
            do { try await control.pair(endpoint: origin, pin: pin, device: device, invitation: invitation, name: name, expected: expected); status = "Paired. Open controller explicitly." }
            catch { status = "Pair failed. Check invitation, TLS trust, verified identity and leaf fingerprint." }
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
            catch { cancelConfirmation(); replyHandler(nil, "Native control failed or session changed. Verify pairing/trust, then Refresh; no replay.") }
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
    @State private var device = ""
    @State private var invitation = ""
    @State private var name = ""
    var body: some View {
        VStack {
            DisclosureGroup("Pair verified device") {
                TextField("Exact trusted HTTPS origin", text: $origin).textInputAutocapitalization(.never).autocorrectionDisabled()
                TextField("Leaf certificate SHA-256 hex", text: $pin).textInputAutocapitalization(.never).autocorrectionDisabled()
                TextField("Expected device ID", text: $device).textInputAutocapitalization(.never).autocorrectionDisabled()
                SecureField("One-time invitation", text: $invitation).textContentType(.oneTimeCode)
                TextField("Controller name", text: $name)
                Button("Pair") { let secret = invitation; invitation = ""; shell.pair(origin: origin, pin: pin, device: device, invitation: secret, name: name) }
                Button("Disconnect and forget local credential") { shell.forget() }
            }.padding(.horizontal)
            Button("Open controller / Refresh connection") { shell.open() }
            Text(shell.status).font(.caption).padding(.horizontal).accessibilityAddTraits(.updatesFrequently)
            ControllerView(shell: shell)
        }.onChange(of: phase) { next in if next != .active { invitation = ""; shell.suspend(.background) } }
    }
}
