use http_body_util::{BodyExt, Full};
use hyper::{body::Bytes, Request};
use hyper_util::rt::TokioIo;
use mixyard_control::{Control, IdentityStore, Options, Permission};
use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName};
use serde_json::{json, Value};
use std::{fs, net::SocketAddr, path::PathBuf, sync::Arc};
use tokio_rustls::TlsConnector;

struct Fixture {
    path: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mixyard-control-security-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder.create(&path).unwrap();
        fs::create_dir(path.join("assets")).unwrap();
        fs::write(path.join("assets/index.html"), "fixture").unwrap();
        Self { path }
    }
    fn options(&self) -> Options {
        Options {
            enabled: true,
            bind: "127.0.0.1:0".parse().unwrap(),
            store: self.path.join("profiles"),
            assets: self.path.join("assets"),
            instance: format!("ctrl-test-{}", std::process::id()),
        }
    }
    fn initialize(&self) -> IdentityStore {
        IdentityStore::open(
            &self.path.join("identity"),
            Some("127.0.0.1".parse().unwrap()),
        )
        .unwrap()
    }
    fn reopen(&self) -> IdentityStore {
        IdentityStore::open(&self.path.join("identity"), None).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}

fn connector(pem: &str) -> TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(pem.as_bytes()).unwrap())
        .unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}
async fn request(
    tls: &TlsConnector,
    addr: SocketAddr,
    path: &str,
    credential: Option<&str>,
    body: Value,
    extra: &[(&str, &str)],
) -> (u16, Value) {
    request_raw(
        tls,
        addr,
        path,
        credential,
        &serde_json::to_vec(&body).unwrap(),
        extra,
    )
    .await
}
async fn request_raw(
    tls: &TlsConnector,
    addr: SocketAddr,
    path: &str,
    credential: Option<&str>,
    body: &[u8],
    extra: &[(&str, &str)],
) -> (u16, Value) {
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let stream = tls
        .connect(ServerName::try_from("mixyard-control").unwrap(), tcp)
        .await
        .unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut req = Request::builder()
        .method("POST")
        .uri(path)
        .header("host", addr.to_string())
        .header("content-type", "application/json");
    if let Some(credential) = credential {
        req = req.header("authorization", format!("Bearer {credential}"));
    }
    for (name, value) in extra {
        req = req.header(*name, *value);
    }
    let response = sender
        .send_request(
            req.body(Full::<Bytes>::new(Bytes::copy_from_slice(body)))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_canonical_validation_and_remote_private_errors() {
    let fixture = Fixture::new();
    let mut identity = fixture.initialize();
    let id = identity.device_id().to_owned();
    let tls = connector(&identity.trust().certificate_pem);
    let invite = identity
        .invite(vec![Permission::View, Permission::Settings], 300)
        .unwrap();
    let control = Control::bind(identity, fixture.options()).await.unwrap();
    let address = control.address();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(control.serve_until(async {
        let _ = stop_rx.await;
    }));
    let (_, paired) = request(
        &tls,
        address,
        "/control/pair",
        None,
        json!({"deviceId":id,"invitation":invite["invitation"],"name":"raw regression"}),
        &[],
    )
    .await;
    let credential = paired["credential"].as_str().unwrap();
    let (_, session) = request(
        &tls,
        address,
        "/control/session",
        Some(credential),
        json!({}),
        &[],
    )
    .await;
    let (_, settings) = request(
        &tls,
        address,
        "/control/request",
        Some(credential),
        envelope(&session, "/api/automation/settings/read", json!({})),
        &[],
    )
    .await;
    let settings_json = settings["body"]["settings"].to_string();
    assert!(settings_json.contains("\"applications\":{}"));
    let ambiguous = settings_json.replace("\"applications\":{}", "\"applications\":{\"/not-executed/example\":\"first\",\"/not-executed/example\":\"second\"}");
    let wire = format!(
        r#"{{"deviceId":{},"authorityId":{},"revision":{},"path":"/api/automation/settings/write","body":{{"expectedSettingsRevision":0,"settings":{ambiguous}}}}}"#,
        session["deviceId"], session["authorityId"], session["revision"]
    );
    let (status, rejected) = request_raw(
        &tls,
        address,
        "/control/request",
        Some(credential),
        wire.as_bytes(),
        &[],
    )
    .await;
    assert!(
        status >= 400,
        "ambiguous mappings must reach canonical duplicate-key rejection"
    );
    assert!(rejected["body"]["error"]["message"]
        .as_str()
        .unwrap()
        .contains("duplicate"));
    let (_, after) = request(
        &tls,
        address,
        "/control/request",
        Some(credential),
        envelope(&session, "/api/automation/settings/read", json!({})),
        &[],
    )
    .await;
    assert_eq!(
        after["body"]["settingsRevision"], 0,
        "rejected write cannot persist settings"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(fixture.path.join("profiles")).unwrap();
        fs::set_permissions(
            fixture.path.join("profiles"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let (status, error) = request(
            &tls,
            address,
            "/control/session",
            Some(credential),
            json!({}),
            &[],
        )
        .await;
        assert_eq!(status, 503);
        assert!(
            !error.to_string().contains(fixture.path.to_str().unwrap()),
            "session errors cannot disclose private store paths"
        );
        fs::set_permissions(
            fixture.path.join("profiles"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let (status, _) = request(
            &tls,
            address,
            "/control/request",
            Some(credential),
            envelope(
                &session,
                "/api/automation/settings/write",
                json!({"expectedSettingsRevision":0,"settings":settings["body"]["settings"]}),
            ),
            &[],
        )
        .await;
        assert_eq!(status, 200);
        let settings_path = fixture.path.join("situation-settings/settings.json");
        fs::set_permissions(&settings_path, fs::Permissions::from_mode(0o644)).unwrap();
        let (status, error) = request(
            &tls,
            address,
            "/control/request",
            Some(credential),
            envelope(&session, "/api/automation/settings/read", json!({})),
            &[],
        )
        .await;
        assert_eq!(status, 500);
        assert_eq!(error["body"]["error"]["code"], "unsafe-store-path");
        assert!(
            !error.to_string().contains(fixture.path.to_str().unwrap()),
            "canonical errors cannot disclose private store paths"
        );
        fs::set_permissions(settings_path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    stop_tx.send(()).unwrap();
    task.await.unwrap().unwrap();
}
fn envelope(session: &Value, path: &str, body: Value) -> Value {
    json!({"deviceId":session["deviceId"],"authorityId":session["authorityId"],"revision":session["revision"],"path":path,"body":body})
}
fn admin_envelope(session: &Value, body: Value) -> Value {
    json!({"deviceId":session["deviceId"],"authorityId":session["authorityId"],"revision":session["revision"],"body":body})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn singleton_settings_reads_and_saves_only_settings_without_view() {
    let fixture = Fixture::new();
    let mut identity = fixture.initialize();
    let id = identity.device_id().to_owned();
    let tls = connector(&identity.trust().certificate_pem);
    let invite = identity.invite(vec![Permission::Settings], 300).unwrap();
    let control = Control::bind(identity, fixture.options()).await.unwrap();
    let address = control.address();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(control.serve_until(async {
        let _ = stop_rx.await;
    }));
    let (_, pair) = request(
        &tls,
        address,
        "/control/pair",
        None,
        json!({"deviceId":id,"invitation":invite["invitation"],"name":"settings only"}),
        &[],
    )
    .await;
    let credential = pair["credential"].as_str().unwrap();
    let (_, session) = request(
        &tls,
        address,
        "/control/session",
        Some(credential),
        json!({}),
        &[],
    )
    .await;
    assert_eq!(session["permissions"], json!(["settings"]));
    let (status, snapshot) = request(
        &tls,
        address,
        "/control/request",
        Some(credential),
        envelope(&session, "/api/automation/settings/read", json!({})),
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "settings editor must read its revision before saving"
    );
    let (status, saved) = request(&tls, address, "/control/request", Some(credential), envelope(&session, "/api/automation/settings/write", json!({"expectedSettingsRevision":snapshot["body"]["settingsRevision"],"settings":snapshot["body"]["settings"]})), &[]).await;
    assert_eq!(status, 200);
    assert_eq!(saved["body"]["settingsRevision"], 1);
    for route in [
        "/api/status",
        "/api/profiles/read",
        "/api/profiles/edit",
        "/api/automation/context",
    ] {
        assert_eq!(
            request(
                &tls,
                address,
                "/control/request",
                Some(credential),
                envelope(&session, route, json!({})),
                &[]
            )
            .await
            .0,
            403,
            "Settings must not imply View or Control"
        );
    }
    let (_, current) = request(
        &tls,
        address,
        "/control/session",
        Some(credential),
        json!({}),
        &[],
    )
    .await;
    assert_eq!(current["permissions"], json!(["settings"]));
    stop_tx.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_tls_authorization_revision_rotation_revoke_and_restart() {
    let fixture = Fixture::new();
    let mut identity = fixture.initialize();
    let id = identity.device_id().to_owned();
    let public = identity.trust();
    let tls = connector(&public.certificate_pem);
    let admin_invite = identity
        .invite(
            vec![Permission::View, Permission::Control, Permission::Settings],
            300,
        )
        .unwrap();
    let view_invite = identity.invite(vec![Permission::View], 300).unwrap();
    let control = Control::bind(identity, fixture.options()).await.unwrap();
    let addr = control.address();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(control.serve_until(async {
        let _ = stop_rx.await;
    }));
    assert_eq!(
        request(&tls, addr, "/control/session", None, json!({}), &[])
            .await
            .0,
        401
    );
    assert_eq!(
        request(&tls, addr, "/api/profiles/edit", None, json!({}), &[])
            .await
            .0,
        401
    );
    assert_eq!(
        request(&tls, addr, "/control/revoke", None, json!({}), &[])
            .await
            .0,
        401
    );
    let pair = json!({"deviceId":id,"invitation":admin_invite["invitation"],"name":"owner"});
    let (status, admin) = request(&tls, addr, "/control/pair", None, pair.clone(), &[]).await;
    assert_eq!(status, 200);
    assert_eq!(
        request(&tls, addr, "/control/pair", None, pair, &[])
            .await
            .0,
        401,
        "invitation must not be reusable"
    );
    let (_, viewer) = request(
        &tls,
        addr,
        "/control/pair",
        None,
        json!({"deviceId":id,"invitation":view_invite["invitation"],"name":"viewer"}),
        &[],
    )
    .await;
    let secret = admin["credential"].as_str().unwrap();
    let viewer_secret = viewer["credential"].as_str().unwrap();
    let (_, session) = request(&tls, addr, "/control/session", Some(secret), json!({}), &[]).await;
    assert_eq!(session["backendAvailable"], false);
    assert_eq!(session["audioApplied"], false);
    let config: Value = serde_json::from_str(include_str!("../../examples/config.json")).unwrap();
    let edit = envelope(
        &session,
        "/api/profiles/edit",
        json!({"expectedRevision":0,"edit":{"kind":"save","name":"owned","config":config}}),
    );
    assert_eq!(
        request(
            &tls,
            addr,
            "/control/request",
            Some(viewer_secret),
            edit.clone(),
            &[]
        )
        .await
        .0,
        403
    );
    assert_eq!(
        request(
            &tls,
            addr,
            "/control/request",
            Some(viewer_secret),
            envelope(&session, "/api/automation/settings/write", json!({})),
            &[]
        )
        .await
        .0,
        403
    );
    let mut wrong = edit.clone();
    wrong["deviceId"] = "other-device".into();
    assert_eq!(
        request(&tls, addr, "/control/request", Some(secret), wrong, &[])
            .await
            .0,
        409
    );
    assert_eq!(
        request(
            &tls,
            addr,
            "/control/request",
            Some(secret),
            edit.clone(),
            &[("origin", "https://attacker.invalid")]
        )
        .await
        .0,
        403
    );
    let (status, saved) = request(
        &tls,
        addr,
        "/control/request",
        Some(secret),
        edit.clone(),
        &[],
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(saved["revision"], 1);
    assert_eq!(
        request(&tls, addr, "/control/request", Some(secret), edit, &[])
            .await
            .0,
        409,
        "a competing old revision must not overwrite"
    );
    let (_, fresh) = request(&tls, addr, "/control/session", Some(secret), json!({}), &[]).await;
    assert_eq!(
        request(
            &tls,
            addr,
            "/control/request",
            Some(secret),
            envelope(&fresh, "/api/automation/arm", json!({"reviewId":"forged"})),
            &[]
        )
        .await
        .0,
        422
    );
    let (status, rotation) = request(
        &tls,
        addr,
        "/control/rotate",
        Some(secret),
        admin_envelope(&fresh, json!({})),
        &[],
    )
    .await;
    assert_eq!(status, 200);
    let rotated = rotation["body"]["credential"].as_str().unwrap().to_owned();
    assert_eq!(
        request(&tls, addr, "/control/session", Some(secret), json!({}), &[])
            .await
            .0,
        401
    );
    let (status, _) = request(
        &tls,
        addr,
        "/control/revoke",
        Some(&rotated),
        admin_envelope(&fresh, json!({"credentialId":viewer["credentialId"]})),
        &[],
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        request(
            &tls,
            addr,
            "/control/session",
            Some(viewer_secret),
            json!({}),
            &[]
        )
        .await
        .0,
        401
    );
    stop_tx.send(()).unwrap();
    task.await.unwrap().unwrap();
    assert!(!fixture.path.join("identity/owner.lock").exists());
    let persisted = fs::read_to_string(fixture.path.join("identity/state.json")).unwrap();
    assert!(!persisted.contains(secret));
    assert!(!persisted.contains(&rotated));
    assert!(!persisted.contains(admin_invite["invitation"].as_str().unwrap()));
    let reopened = fixture.reopen();
    assert_eq!(reopened.device_id(), id);
    assert_eq!(
        reopened.trust().certificate_sha256,
        public.certificate_sha256
    );
    assert!(reopened.audio_identity_material("wrong-device").is_err());
    let audio = reopened.audio_identity_material(&id).unwrap();
    assert_eq!(audio.device_id, id);
    assert_eq!(audio.server_name, "mixyard-control");
    let control = Control::bind(reopened, fixture.options()).await.unwrap();
    let addr = control.address();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(control.serve_until(async {
        let _ = stop_rx.await;
    }));
    let (status, restarted) = request(
        &tls,
        addr,
        "/control/session",
        Some(&rotated),
        json!({}),
        &[],
    )
    .await;
    assert_eq!(status, 200);
    assert_ne!(restarted["authorityId"], fresh["authorityId"]);
    assert_eq!(restarted["revision"], 1);
    assert_eq!(
        request(
            &tls,
            addr,
            "/control/request",
            Some(&rotated),
            envelope(&fresh, "/api/profiles/read", json!({})),
            &[]
        )
        .await
        .1["error"]["code"],
        "session-changed"
    );
    assert_eq!(
        request(
            &tls,
            addr,
            "/control/session",
            Some(viewer_secret),
            json!({}),
            &[]
        )
        .await
        .0,
        401
    );
    let (_, read) = request(
        &tls,
        addr,
        "/control/request",
        Some(&rotated),
        envelope(&restarted, "/api/profiles/read", json!({})),
        &[],
    )
    .await;
    assert_eq!(read["body"]["profiles"][0]["name"], "owned");
    stop_tx.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_provisioning_opt_in_trust_and_role_cannot_be_supplied_by_caller() {
    let fixture = Fixture::new();
    let mut identity = fixture.initialize();
    let id = identity.device_id().to_owned();
    assert!(
        IdentityStore::open(&fixture.path.join("identity"), None).is_err(),
        "second owner must be denied"
    );
    let invite = identity.invite(vec![Permission::View], 300).unwrap();
    let public = identity.trust();
    let control = Control::bind(identity, fixture.options()).await.unwrap();
    let addr = control.address();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(control.serve_until(async {
        let _ = stop_rx.await;
    }));
    let tls = connector(&public.certificate_pem);
    let mut pair = json!({"deviceId":id,"invitation":invite["invitation"],"name":"controller","permissions":["settings"]});
    assert_eq!(
        request(&tls, addr, "/control/pair", None, pair.clone(), &[])
            .await
            .0,
        400
    );
    pair.as_object_mut().unwrap().remove("permissions");
    pair["deviceId"] = "wrong-device".into();
    assert_eq!(
        request(&tls, addr, "/control/pair", None, pair.clone(), &[])
            .await
            .0,
        409
    );
    pair["deviceId"] = id.into();
    assert_eq!(
        request(&tls, addr, "/control/pair", None, pair, &[])
            .await
            .1["permissions"],
        json!(["view"])
    );
    let other = Fixture::new();
    let other_identity = other.initialize();
    let wrong_tls = connector(&other_identity.trust().certificate_pem);
    let socket = tokio::net::TcpStream::connect(addr).await.unwrap();
    assert!(
        wrong_tls
            .connect(ServerName::try_from("mixyard-control").unwrap(), socket)
            .await
            .is_err(),
        "a different provisioned cert must fail normal TLS validation"
    );
    stop_tx.send(()).unwrap();
    task.await.unwrap().unwrap();
    let mut options = fixture.options();
    options.enabled = false;
    assert!(Control::bind(fixture.reopen(), options).await.is_err());
    let mut options = fixture.options();
    options.bind = "0.0.0.0:0".parse().unwrap();
    assert!(Control::bind(fixture.reopen(), options).await.is_err());
    let mut options = fixture.options();
    options.bind = "8.8.8.8:0".parse().unwrap();
    assert!(Control::bind(fixture.reopen(), options).await.is_err());
}
