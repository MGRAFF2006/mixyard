//! Actual loopback HTTP against the production binary; all stores are disposable.
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc, Arc, Barrier,
    },
    thread,
    time::{Duration, Instant},
};
static NEXT: AtomicU32 = AtomicU32::new(0);
struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "mixyard-web-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("assets")).unwrap();
        fs::write(
            root.join("assets/index.html"),
            "<!doctype html><title>Web fixture</title>",
        )
        .unwrap();
        fs::write(root.join("assets/app.js"), "console.log('fixture');").unwrap();
        fs::write(root.join("secret.txt"), "outside secret").unwrap();
        Self { root }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mixyard-web"));
        command
            .arg("--store")
            .arg(self.root.join("store"))
            .arg("--assets")
            .arg(self.root.join("assets"))
            .args(["--port", "0", "--instance", &self.instance()]);
        command
    }
    fn instance(&self) -> String {
        format!(
            "web-{}-{}",
            std::process::id(),
            self.root
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .rsplit('-')
                .next()
                .unwrap()
        )
    }
    fn start(&self) -> Service {
        let mut child = self
            .command()
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(stdout).read_line(&mut line).unwrap();
            let _ = tx.send(line);
        });
        let mut service = Service {
            child,
            address: String::new(),
            token: String::new(),
        };
        let line = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("web startup deadline");
        service.address = line
            .trim()
            .strip_prefix("Mixyard web: http://")
            .expect("printed URL")
            .into();
        assert!(service.address.starts_with("127.0.0.1:"));
        let session = service.request("GET", "/api/session", &[], "");
        assert_eq!(session.status, 200);
        assert_eq!(session.json()["apiVersion"], 1);
        assert_eq!(session.json()["audioAvailable"], false);
        service.token = session.json()["csrfToken"].as_str().unwrap().into();
        assert!(service.token.len() >= 32);
        service
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
struct Service {
    child: Child,
    address: String,
    token: String,
}
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
struct Reply {
    status: u16,
    headers: String,
    body: Vec<u8>,
}
impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}
impl Service {
    fn request(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
        request(&self.address, method, path, headers, body)
    }
    fn post(&self, path: &str, body: Value) -> Reply {
        self.request(
            "POST",
            path,
            &[
                ("X-Mixyard-Token", &self.token),
                ("Content-Type", "application/json"),
            ],
            &body.to_string(),
        )
    }
    fn ok(&self, path: &str, body: Value) -> Value {
        let reply = self.post(path, body);
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        reply.json()
    }
    fn edit(&self, revision: u64, edit: Value) -> Value {
        self.ok(
            "/api/profiles/edit",
            json!({"expectedRevision":revision,"edit":edit}),
        )
    }
}
fn request(address: &str, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let host = if headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("host"))
    {
        String::new()
    } else {
        format!("Host: {address}\r\n")
    };
    let extra: String = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\n{host}{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    let _ = stream.write_all(body.as_bytes());
    let mut bytes = Vec::new();
    match stream.read_to_end(&mut bytes) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset && !bytes.is_empty() => {}
        Err(e) => panic!("HTTP response: {e}"),
    }
    let split = bytes
        .windows(4)
        .position(|x| x == b"\r\n\r\n")
        .expect("HTTP response headers");
    let headers = String::from_utf8(bytes[..split].to_vec()).unwrap();
    let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
    Reply {
        status,
        headers,
        body: bytes[split + 4..].to_vec(),
    }
}
fn config() -> Value {
    serde_json::from_str(include_str!("../examples/config.json")).unwrap()
}
fn inventory() -> Value {
    serde_json::from_str(include_str!("../examples/inventory.json")).unwrap()
}

#[test]
fn core_validation_inspection_and_disconnected_status() {
    let fixture = Fixture::new();
    let service = fixture.start();
    assert_eq!(
        service.ok("/api/validate", json!({"config":config()})),
        json!({"valid":true})
    );
    let expected = serde_json::to_value(mixyard::diagnostics::inspect(
        &serde_json::from_value(config()).unwrap(),
        &serde_json::from_value(inventory()).unwrap(),
    ))
    .unwrap();
    assert_eq!(
        service.ok(
            "/api/inspect",
            json!({"config":config(),"inventory":inventory()})
        ),
        expected
    );
    let mut invalid = config();
    invalid["schemaVersion"] = json!(1);
    assert_eq!(
        service
            .post("/api/validate", json!({"config":invalid}))
            .status,
        422
    );
    let inspection = service.ok(
        "/api/inspect",
        json!({"config":invalid,"inventory":inventory()}),
    );
    assert_eq!(inspection["planAvailable"], false);
    assert!(inspection["preflightError"].is_string());
    let status = service.ok("/api/status", json!({}));
    assert_eq!(status["connected"], false);
    assert_eq!(status["snapshot"], Value::Null);
    assert!(status["error"].is_string());
    assert!(!fixture.root.join("store").exists());
}

#[test]
fn expanding_inspection_is_rejected_without_taking_down_service() {
    let fixture = Fixture::new();
    let service = fixture.start();
    let pad = "x".repeat(1000);
    let config = json!({
        "schemaVersion":2,"revision":1,
        "sources": (0..300).map(|i| json!({"id":format!("s{i}-{pad}"),"selector":{"kind":"app","applicationId":format!("app{i}")}})).collect::<Vec<_>>(),
        "mixes":[{"id":"m","name":"Mix","channels":2}],"sends":[],
        "outputs": (0..300).map(|i| json!({"id":format!("o{i}-{pad}"),"mix":"m","target":{"kind":"virtual-input","endpointId":format!("out{i}")},"gain":1})).collect::<Vec<_>>()
    });
    let inventory = json!({"schemaVersion":2,"revision":1,"platform":"linux","evidence":"simulated","sources":[],"outputs":[],"mediaSessions":[],"limits":{}});
    let body = json!({"config":config,"inventory":inventory});
    assert!(body.to_string().len() < 1024 * 1024);
    assert_eq!(
        service.ok("/api/validate", json!({"config":config})),
        json!({"valid":true})
    );
    let rejected = service.post("/api/inspect", body);
    assert_eq!(rejected.status, 422);
    assert_eq!(rejected.json()["error"]["code"], "inspection-too-large");
    // Small path count alone is insufficient: long repeated source/mix strings
    // can exhaust memory too, so exercise byte-weighted admission separately.
    for field in ["source", "mix"] {
        let mut config = json!({"schemaVersion":2,"revision":1,
            "sources":[{"id":"s","selector":{"kind":"app","applicationId":"a"}}],
            "mixes":[{"id":"m","name":"Mix","channels":2}],"sends":[],
            "outputs":(0..100).map(|i|json!({"id":format!("o{i}"),"mix":"m","target":{"kind":"virtual-input","endpointId":format!("out{i}")},"gain":1})).collect::<Vec<_>>()});
        if field == "source" {
            config["sources"][0]["id"] = json!("x".repeat(500_000));
        } else {
            config["mixes"][0]["name"] = json!("x".repeat(500_000));
        }
        let rejected = service.post(
            "/api/inspect",
            json!({"config":config,"inventory":inventory}),
        );
        assert_eq!(rejected.status, 422);
        assert_eq!(rejected.json()["error"]["code"], "inspection-too-large");
    }
    assert_eq!(service.request("GET", "/api/session", &[], "").status, 200);
    assert_eq!(
        service.ok("/api/profiles/read", json!({}))["storeRevision"],
        0
    );
}

#[test]
fn repeated_observation_ids_are_bounded_under_concurrent_inspection() {
    let fixture = Fixture::new();
    let service = fixture.start();
    let config = json!({"schemaVersion":2,"revision":1,
        "sources":[{"id":"s","selector":{"kind":"app","applicationId":"a"}}],
        "mixes":(0..180).map(|i|json!({"id":format!("m{i}"),"name":"Mix","channels":2})).collect::<Vec<_>>(),
        "sends":(0..180).map(|i|json!({"source":"s","mix":format!("m{i}"),"gain":1})).collect::<Vec<_>>(),"outputs":[]});
    assert_eq!(
        service.ok("/api/validate", json!({"config":config})),
        json!({"valid":true})
    );
    let inventory = json!({"schemaVersion":2,"revision":1,"platform":"linux","evidence":"simulated",
        "sources":[{"observedId":"x".repeat(950_000),"selector":{"kind":"app","applicationId":"a"},"origin":"external","format":{"channels":2,"sampleRateHz":48000}}],
        "outputs":[],"mediaSessions":[],"limits":{}});
    let body = Arc::new(json!({"config":config,"inventory":inventory}).to_string());
    assert!(body.len() < 1024 * 1024);
    let barrier = Arc::new(Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let (address, token, body, barrier) = (
                service.address.clone(),
                service.token.clone(),
                body.clone(),
                barrier.clone(),
            );
            thread::spawn(move || {
                barrier.wait();
                let reply = request(
                    &address,
                    "POST",
                    "/api/inspect",
                    &[
                        ("X-Mixyard-Token", &token),
                        ("Content-Type", "application/json"),
                    ],
                    &body,
                );
                assert_eq!(reply.status, 422);
                assert_eq!(reply.json()["error"]["code"], "inspection-too-large");
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(service.request("GET", "/api/session", &[], "").status, 200);
    assert_eq!(
        service.ok("/api/profiles/read", json!({}))["storeRevision"],
        0
    );
}

#[test]
fn dsp_preview_uses_real_float64_processing_without_store_or_audio_mutation() {
    let fixture = Fixture::new();
    let service = fixture.start();
    let config = json!({"name":"Offline DSP","format":{"sampleRateHz":12345,"channels":["R","L","aux"]},
        "effects":[{"name":"Boost","enabled":true,"effect":{"kind":"gain","gain":2,"invertPolarity":false}}]});
    let validated = service.ok("/api/dsp/validate", json!({"config":config,"frames":2}));
    assert_eq!(validated["valid"], true);
    assert_eq!(validated["precision"], "float64");
    assert!(validated.get("timing").is_none());
    let result = service.ok(
        "/api/dsp/preview",
        json!({"config":config,"frames":2,"iterations":3,"bypassed":false,
        "signal":{"kind":"samples","samples":[0.125,-0.25,0.75,0.125,-0.5,0.001]}}),
    );
    assert_eq!(result["mode"], "offline-dsp-preview");
    assert_eq!(result["precision"], "float64");
    assert_eq!(result["sampleRateHz"], 12345);
    assert_eq!(result["channels"], json!(["R", "L", "aux"]));
    assert_eq!(
        result["output"],
        json!([0.25, -0.5, 1.5, 0.25, -1.0, 0.002])
    );
    assert_eq!(result["report"]["overFullScaleSamples"], 1);
    assert_eq!(result["meters"][2]["peak"], 1.5);
    assert_eq!(result["timing"]["iterations"], 3);
    assert_eq!(
        result["timing"]["overBudget"],
        result["timing"]["deadlineMisses"].as_u64().unwrap() > 0
    );
    let bypass = service.ok(
        "/api/dsp/preview",
        json!({"config":config,"frames":1,"iterations":1,"bypassed":true,
        "signal":{"kind":"samples","samples":[1.0000000000000002,-0.0,0.0]}}),
    );
    assert_eq!(bypass["output"][0].as_f64(), Some(1.0000000000000002));
    assert!(bypass["output"][1].as_f64().unwrap().is_sign_negative());
    assert_eq!(bypass["report"]["bypassed"], true);
    assert!(!fixture.root.join("store").exists());
    assert_eq!(service.ok("/api/status", json!({}))["connected"], false);
}

#[test]
fn dsp_http_retains_float64_sample_and_parameter_bits() {
    let fixture = Fixture::new();
    let service = fixture.start();
    let mut config = json!({"name":"Exact JSON", "format":{"sampleRateHz":48000,"channels":["mono"]},
        "effects":[{"name":"Gain","enabled":true,"effect":{"kind":"gain","gain":1,"invertPolarity":false}}]});
    let samples = [
        0.9554529902278269_f64,
        0.9932393975813079,
        -0.0,
        f64::from_bits(1),
        1e-300,
        1.0000000000000002,
    ];
    let result = service.ok(
        "/api/dsp/preview",
        json!({"config":config,"frames":samples.len(),
        "iterations":1,"bypassed":true,"signal":{"kind":"samples","samples":samples}}),
    );
    for (actual, expected) in result["output"].as_array().unwrap().iter().zip(samples) {
        assert_eq!(actual.as_f64().unwrap().to_bits(), expected.to_bits());
    }
    for gain in &samples[..2] {
        config["effects"][0]["effect"]["gain"] = json!(gain);
        let result = service.ok(
            "/api/dsp/preview",
            json!({"config":config,"frames":1,
            "iterations":1,"bypassed":false,"signal":{"kind":"constant","amplitude":1}}),
        );
        assert_eq!(
            result["output"][0].as_f64().unwrap().to_bits(),
            gain.to_bits()
        );
    }
}

#[test]
fn actual_delay_measurement_and_explicit_alignment_do_not_change_profiles() {
    let fixture = Fixture::new();
    let service = fixture.start();
    let mut request = json!({"config":{"name":"Path probe","format":{"sampleRateHz":48000,"channels":["R","L"]},
        "effects":[{"name":"R delay","enabled":true,"channels":["R"],"effect":{"kind":"delay","frames":3,"wet":1,"dry":0,"feedback":0}},
        {"name":"L delay","enabled":true,"channels":["L"],"effect":{"kind":"delay","frames":7,"wet":1,"dry":0,"feedback":0}}]},
        "probe":{"blockFrames":2,"observationFrames":16,"referenceChannels":["R","L"],"amplitude":1,"detectionThreshold":0,"bypassed":false},
        "alignment":{"channels":["R","L"],"feature":"first-response"}});
    let reply = service.ok("/api/dsp/delay", request.clone());
    assert_eq!(reply["measurement"]["mode"], "offline-impulse-response");
    assert_eq!(reply["measurement"]["channels"][0]["firstResponseFrame"], 3);
    assert_eq!(reply["measurement"]["channels"][1]["strongestPeakFrame"], 7);
    assert_eq!(reply["alignment"]["delays"][0]["frames"], 4);
    assert_eq!(reply["alignment"]["delays"][1]["frames"], 0);
    assert!(reply["alignmentError"].is_null());
    request["config"]["effects"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"Explicit compensation","enabled":true,
        "channels":["R"],"effect":{"kind":"delay","frames":4,"wet":1,"dry":0,"feedback":0}}));
    let aligned = service.ok("/api/dsp/delay", request.clone());
    assert!(aligned["measurement"]["channels"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["firstResponseFrame"] == 7));
    request["probe"]["observationFrames"] = json!(131_072);
    assert_eq!(service.post("/api/dsp/delay", request.clone()).status, 422);
    request["probe"]["observationFrames"] = json!(2);
    let partial = service.ok("/api/dsp/delay", request);
    assert!(partial["measurement"]["channels"][0]["firstResponseFrame"].is_null());
    assert!(!partial["measurement"]["responseWindowComplete"]
        .as_bool()
        .unwrap());
    assert!(partial["alignment"].is_null());
    assert!(partial["alignmentError"]
        .as_str()
        .unwrap()
        .contains("NoResponse"));
    assert_eq!(
        service.ok("/api/profiles/read", json!({}))["storeRevision"],
        0
    );
    assert!(!fixture.root.join("store").exists());
}

#[test]
fn dsp_measurement_resets_state_and_preserves_slow_full_output() {
    let fixture = Fixture::new();
    let service = fixture.start();
    let config = json!({"name":"Delay","format":{"sampleRateHz":48000,"channels":["mono"]},
        "effects":[{"name":"tap","enabled":true,"effect":{"kind":"delay","frames":2,"wet":1,"dry":0,"feedback":0}}]});
    let request = json!({"config":config,"frames":4,"iterations":4,"bypassed":false,"signal":{"kind":"impulse","amplitude":0.25}});
    for _ in 0..2 {
        let result = service.ok("/api/dsp/preview", request.clone());
        assert_eq!(result["output"], json!([0.0, 0.0, 0.25, 0.0]));
        assert_eq!(result["admission"]["explicitDelayFrames"], 2);
        assert_eq!(result["admission"]["algorithmicLatencyFrames"], 0);
    }
    let slow = json!({"name":"High-rate FIR","format":{"sampleRateHz":4294967295u32,"channels":["mono"]},
        "effects":[{"name":"IR","enabled":true,"effect":{"kind":"fir","coefficients":[0.5,0.25,0.125]}}]});
    let result = service.ok("/api/dsp/preview", json!({"config":slow,"frames":3,"iterations":2,"bypassed":false,"signal":{"kind":"impulse","amplitude":1}}));
    assert_eq!(result["output"], json!([0.5, 0.25, 0.125]));
    if result["timing"]["measured"] == true {
        assert_eq!(result["timing"]["overBudget"], true);
        assert!(result["timing"]["deadlineMisses"].as_u64().unwrap() > 0);
    } else {
        assert_eq!(result["timing"]["meanMs"], Value::Null);
        assert_eq!(result["timing"]["maxMs"], Value::Null);
    }
}

#[test]
fn dsp_invalid_and_oversized_jobs_reject_without_partial_results_or_service_loss() {
    let fixture = Fixture::new();
    let service = fixture.start();
    let config =
        json!({"name":"Empty","format":{"sampleRateHz":48000,"channels":["mono"]},"effects":[]});
    let base = json!({"config":config,"frames":8,"iterations":1,"bypassed":false,"signal":{"kind":"constant","amplitude":0.25}});
    for (field, value) in [
        ("frames", json!(0)),
        ("frames", json!(4097)),
        ("iterations", json!(0)),
        ("iterations", json!(65)),
    ] {
        let mut request = base.clone();
        request[field] = value;
        let reply = service.post("/api/dsp/preview", request);
        assert_eq!(reply.status, 422);
        assert!(reply.json().get("output").is_none());
    }
    let mut malformed = base.clone();
    malformed["signal"] = json!({"kind":"samples","samples":[0.25]});
    assert_eq!(service.post("/api/dsp/preview", malformed).status, 422);
    let mut invalid = base.clone();
    invalid["config"]["effects"] = json!([{"name":"bad","enabled":true,"effect":{"kind":"gain","gain":-1,"invertPolarity":false}}]);
    assert_eq!(service.post("/api/dsp/preview", invalid).status, 422);
    let mut budget = base.clone();
    budget["config"]["effects"] = json!([{"name":"large delay","enabled":true,"effect":{"kind":"delay","frames":1000000000,"wet":1,"dry":0,"feedback":0}}]);
    assert_eq!(service.post("/api/dsp/preview", budget).status, 422);
    let mut overflow = base.clone();
    overflow["signal"] = json!({"kind":"constant","amplitude":1.7976931348623157e308});
    overflow["config"]["effects"] = json!([{"name":"overflow","enabled":true,"effect":{"kind":"gain","gain":2,"invertPolarity":false}}]);
    let reply = service.post("/api/dsp/preview", overflow);
    assert_eq!(reply.status, 422);
    assert_eq!(reply.json()["error"]["code"], "dsp-processing-failed");
    assert!(reply.json().get("output").is_none());
    assert_eq!(
        service.ok("/api/dsp/preview", base)["output"],
        json!(vec![0.25; 8])
    );
    assert_eq!(service.request("GET", "/api/session", &[], "").status, 200);
    assert!(!fixture.root.join("store").exists());
}

#[test]
fn persistent_profiles_conflicts_and_all_edits_survive_restart() {
    let fixture = Fixture::new();
    let service = fixture.start();
    assert_eq!(
        service.ok("/api/profiles/read", json!({}))["storeRevision"],
        0
    );
    let saved = service.edit(
        0,
        json!({"kind":"save","id":null,"name":"Call","config":config()}),
    );
    let id = saved["profiles"][0]["id"].clone();
    assert_eq!(
        service.edit(1, json!({"kind":"rename","id":id,"name":"Meeting"}))["profiles"][0]["name"],
        "Meeting"
    );
    let loaded = service.edit(2, json!({"kind":"load","id":id}));
    assert!(loaded["draft"].is_object());
    let mut changed = config();
    changed["revision"] = json!(100);
    let drafted = service.edit(3, json!({"kind":"draft","config":changed}));
    assert_eq!(drafted["draft"]["revision"], 101);
    let undone = service.edit(4, json!({"kind":"undo"}));
    assert_eq!(undone["draft"]["revision"], 102);
    let bytes = fs::read(fixture.root.join("store/profiles.json")).unwrap();
    let stale = service.post(
        "/api/profiles/edit",
        json!({"expectedRevision":0,"edit":{"kind":"rename","id":id,"name":"stale"}}),
    );
    assert_eq!(stale.status, 409);
    assert_eq!(stale.json()["error"]["code"], "store-conflict");
    let bad = service.post(
        "/api/profiles/edit",
        json!({"expectedRevision":5,"edit":{"kind":"rename","id":id,"name":" "}}),
    );
    assert_eq!(bad.status, 422);
    assert_eq!(
        fs::read(fixture.root.join("store/profiles.json")).unwrap(),
        bytes
    );
    let old_token = service.token.clone();
    drop(service);
    let restarted = fixture.start();
    assert_ne!(old_token, restarted.token);
    assert_eq!(restarted.ok("/api/profiles/read", json!({})), undone);
    assert_eq!(
        restarted
            .request(
                "POST",
                "/api/profiles/read",
                &[
                    ("Content-Type", "application/json"),
                    ("X-Mixyard-Token", &old_token)
                ],
                "{}"
            )
            .status,
        403
    );
    fs::write(fixture.root.join("store/write.lock"), "").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            fixture.root.join("store/write.lock"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    assert_eq!(
        restarted
            .post(
                "/api/profiles/edit",
                json!({"expectedRevision":5,"edit":{"kind":"load","id":id}})
            )
            .status,
        409
    );
    assert_eq!(
        fs::read(fixture.root.join("store/profiles.json")).unwrap(),
        bytes
    );
}

#[test]
fn concurrent_writers_cannot_overwrite_each_other() {
    let fixture = Fixture::new();
    let service = fixture.start();
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for name in ["one", "two"] {
        let barrier = barrier.clone();
        let address = service.address.clone();
        let token = service.token.clone();
        workers.push(thread::spawn(move || {
            barrier.wait();
            request(&address, "POST", "/api/profiles/edit", &[("Content-Type","application/json"),("X-Mixyard-Token", &token)], &json!({"expectedRevision":0,"edit":{"kind":"save","name":name,"id":null,"config":config()}}).to_string()).status
        }));
    }
    barrier.wait();
    let mut statuses: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    statuses.sort();
    assert_eq!(statuses, vec![200, 409]);
    let snapshot = service.ok("/api/profiles/read", json!({}));
    assert_eq!(snapshot["storeRevision"], 1);
    assert_eq!(snapshot["profiles"].as_array().unwrap().len(), 1);
}

#[test]
fn security_checks_static_isolation_and_size_limits() {
    let fixture = Fixture::new();
    let service = fixture.start();
    let page = service.request("GET", "/", &[], "");
    assert_eq!(page.status, 200);
    assert!(String::from_utf8_lossy(&page.body).contains("Web fixture"));
    assert!(page.headers.contains("content-security-policy:"));
    assert!(page.headers.contains("cache-control: no-store"));
    assert!(!page.headers.contains("access-control-allow-origin"));
    assert_eq!(service.request("GET", "/app.js?v=1", &[], "").status, 200);
    for path in [
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/assets%2f..%2fsecret.txt",
        "/.env",
        "/missing",
        "/api/shutdown",
        "/api/apply",
    ] {
        let reply = if path.starts_with("/api/") {
            service.post(path, json!({}))
        } else {
            service.request("GET", path, &[], "")
        };
        assert!(!reply.status.eq(&200), "{path}");
        assert!(!String::from_utf8_lossy(&reply.body).contains("outside secret"));
    }
    assert_eq!(
        service
            .request("GET", "/api/session", &[("Host", "evil.example")], "")
            .status,
        403
    );
    assert_eq!(
        service
            .request(
                "GET",
                "/api/session",
                &[("Host", &service.address), ("Host", &service.address)],
                ""
            )
            .status,
        403
    );
    assert_eq!(
        service
            .request(
                "GET",
                "/api/session",
                &[("Origin", "https://evil.example")],
                ""
            )
            .status,
        403
    );
    assert_eq!(
        service
            .request(
                "POST",
                "/api/profiles/read",
                &[("Content-Type", "application/json")],
                "{}"
            )
            .status,
        403
    );
    assert_eq!(
        service
            .request(
                "POST",
                "/api/profiles/read",
                &[("X-Mixyard-Token", &service.token)],
                "{}"
            )
            .status,
        415
    );
    assert_eq!(
        service.request("GET", "/api/profiles/read", &[], "").status,
        405
    );
    assert_eq!(
        service
            .request(
                "OPTIONS",
                "/api/profiles/edit",
                &[("Origin", "https://evil.example")],
                ""
            )
            .status,
        403
    );
    let origin = format!("http://{}", service.address);
    let headers = [
        ("X-Mixyard-Token", service.token.as_str()),
        ("Content-Type", "application/json; charset=utf-8"),
        ("Origin", origin.as_str()),
    ];
    assert_eq!(
        service
            .request("POST", "/api/profiles/read", &headers, "{}")
            .status,
        200
    );
    assert_eq!(
        service
            .request("POST", "/api/profiles/read", &headers, "{broken")
            .status,
        400
    );
    assert_eq!(
        service
            .request(
                "POST",
                "/api/profiles/read",
                &headers,
                "{\"path\":\"/tmp/other\"}"
            )
            .status,
        400
    );
    assert_eq!(
        service
            .request(
                "POST",
                "/api/profiles/edit",
                &headers,
                "{\"expectedRevision\":0,\"edit\":{\"kind\":\"undo\",\"path\":\"/tmp\"}}"
            )
            .status,
        400
    );
    assert_eq!(
        service
            .request(
                "POST",
                "/api/profiles/read",
                &headers,
                &" ".repeat(1024 * 1024 + 1)
            )
            .status,
        413
    );
    assert!(!fixture.root.join("store").exists());
}

#[test]
fn incomplete_request_times_out_without_blocking_other_clients() {
    let fixture = Fixture::new();
    let service = fixture.start();
    let mut slow = TcpStream::connect(&service.address).unwrap();
    slow.set_read_timeout(Some(Duration::from_secs(7))).unwrap();
    write!(slow, "POST /api/profiles/read HTTP/1.1\r\nHost: {}\r\nX-Mixyard-Token: {}\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{{", service.address, service.token).unwrap();
    let start = Instant::now();
    assert_eq!(
        service.ok("/api/profiles/read", json!({}))["storeRevision"],
        0
    );
    let mut reply = String::new();
    slow.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 408"), "{reply}");
    assert!(start.elapsed() < Duration::from_secs(7));
}

#[test]
fn startup_errors_are_actionable_and_assets_cannot_be_symlinks() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .arg("--assets")
        .arg(fixture.root.join("missing"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("scripts/run-web.sh"));
    let output = fixture
        .command()
        .args(["--instance", "../unsafe"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            fixture.root.join("secret.txt"),
            fixture.root.join("assets/leak.txt"),
        )
        .unwrap();
        let output = fixture.command().output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("symlinks"));
    }
}

#[test]
fn status_reads_the_configured_runtime_without_lifecycle_mutation() {
    struct Runtime {
        child: Child,
        instance: String,
    }
    impl Drop for Runtime {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            #[cfg(unix)]
            {
                let path = mixyard::runtime::endpoint(&self.instance).unwrap();
                let _ = fs::remove_file(&path);
                let _ = fs::remove_dir(std::path::Path::new(&path).parent().unwrap());
            }
        }
    }
    let fixture = Fixture::new();
    let mut runtime = Runtime {
        child: Command::new(env!("CARGO_BIN_EXE_mixyard-runtime"))
            .args(["serve", "--instance", &fixture.instance()])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
        instance: fixture.instance(),
    };
    let start = Instant::now();
    loop {
        assert!(
            runtime.child.try_wait().unwrap().is_none(),
            "runtime exited"
        );
        if mixyard::runtime::ReadOnlyClient::connect(&runtime.instance).is_ok() {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(8));
        thread::sleep(Duration::from_millis(20));
    }
    let service = fixture.start();
    let status = service.ok("/api/status", json!({}));
    assert_eq!(status["connected"], true);
    assert_eq!(status["error"], Value::Null);
    assert_eq!(status["snapshot"]["lifecycle"], "running");
    assert_eq!(
        status["snapshot"]["availability"]["backendAvailable"],
        false
    );
    assert_eq!(
        status["snapshot"]["availability"]["capabilities"]["apply"],
        false
    );
    assert_eq!(service.post("/api/shutdown", json!({})).status, 404);
    drop(service);
    assert!(runtime.child.try_wait().unwrap().is_none());
    assert!(mixyard::runtime::ReadOnlyClient::connect(&runtime.instance).is_ok());
}

#[test]
fn connection_saturation_is_bounded_and_cached_assets_do_not_follow_later_changes() {
    let fixture = Fixture::new();
    let service = fixture.start();
    // Later files are not exposed, even inside the configured directory.
    fs::write(fixture.root.join("assets/later.txt"), "later private data").unwrap();
    assert_eq!(service.request("GET", "/later.txt", &[], "").status, 404);
    fs::write(fixture.root.join("assets/index.html"), "replacement").unwrap();
    let page = service.request("GET", "/", &[], "");
    assert!(String::from_utf8_lossy(&page.body).contains("Web fixture"));
    let mut held = Vec::new();
    for _ in 0..32 {
        let mut stream = TcpStream::connect(&service.address).unwrap();
        stream.write_all(b"GET / HTTP/1.1\r\n").unwrap();
        held.push(stream);
    }
    let mut excess = TcpStream::connect(&service.address).unwrap();
    excess
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let _ = write!(
        excess,
        "GET /api/session HTTP/1.1\r\nHost: {}\r\n\r\n",
        service.address
    );
    let mut byte = [0];
    match excess.read(&mut byte) {
        Ok(0) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        result => panic!("Excess connection was not promptly closed: {result:?}"),
    }
    drop(held);
}
