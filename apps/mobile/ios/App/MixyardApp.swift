import SwiftUI
import WebKit
import AVFAudio
import MobileCore

@main struct MixyardApp: App { var body: some Scene { WindowGroup { MobileView() } } }
@MainActor final class Shell: NSObject, ObservableObject, WKScriptMessageHandlerWithReply, WKNavigationDelegate {
    @Published var status = "Audio unavailable: shared native core/transport not linked. Controller edits stored intent only."
    private var control: ControlClient?
    let endpoint = NativeEndpoint()
    private(set) var web: WKWebView!
    private var root: URL?
    override init() {
        super.init()
        do { control = try ControlClient() } catch { status = "Keychain unavailable. Repair native credentials before connecting." }
        let config = WKWebViewConfiguration(); config.websiteDataStore = .nonPersistent()
        config.userContentController.addScriptMessageHandler(self, contentWorld: .page, name: "mixyard")
        web = WKWebView(frame: .zero, configuration: config); web.navigationDelegate = self
        root = Bundle.main.resourceURL?.appendingPathComponent("controller", isDirectory: true)
        NotificationCenter.default.addObserver(self, selector: #selector(interrupted), name: AVAudioSession.interruptionNotification, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(routeChanged), name: AVAudioSession.routeChangeNotification, object: nil)
        open()
    }
    func suspend(_ reason: StopReason) {
        endpoint.stop(reason); control?.invalidate()
        web.evaluateJavaScript("window.mixyardInvalidate?.()", completionHandler: nil)
    }
    @objc private func interrupted() { suspend(.interruption) }
    @objc private func routeChanged() { suspend(.routeChange) }
    func open() {
        control?.invalidate()
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
            do { replyHandler(try await control.invoke(method: method, payload: payload, expected: expected), nil) }
            catch { replyHandler(nil, "Native control failed or session changed. Verify pairing/trust, then Refresh; no replay.") }
        }
    }
    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard navigationAction.targetFrame?.isMainFrame == true, let url = navigationAction.request.url, let root,
              url.isFileURL, url.standardizedFileURL.path == root.appendingPathComponent("index.html").standardizedFileURL.path else { decisionHandler(.cancel); return }
        decisionHandler(.allow)
    }
}
struct ControllerView: UIViewRepresentable {
    let shell: Shell
    func makeUIView(context: Context) -> WKWebView { shell.web }
    func updateUIView(_ uiView: WKWebView, context: Context) {}
}
struct MobileView: View {
    @StateObject private var shell = Shell()
    @Environment(\.scenePhase) private var phase
    @State private var origin = "", pin = "", device = "", invitation = "", name = ""
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
