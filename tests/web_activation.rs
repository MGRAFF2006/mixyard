//! Actual HTTP binary + private IPC inventory. Inventory/helper fixtures are
//! simulated, never evidence of native audio, OS routing or capture permission.
#[path = "web_support/mod.rs"]
mod support;
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use support::*;
#[cfg(unix)]
#[allow(dead_code)]
#[path = "../src/runtime/unix.rs"]
mod transport;
#[cfg(windows)]
#[allow(dead_code)]
#[path = "../src/runtime/windows.rs"]
mod transport;
const TICK: Duration = Duration::from_millis(10);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
struct InventoryRuntime {
    stop: Arc<AtomicBool>,
    delay: Arc<AtomicU64>,
    requests: Arc<AtomicU64>,
    inventory: Arc<Mutex<Value>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl InventoryRuntime {
    fn start(fixture: &Fixture) -> Self {
        let endpoint = mixyard::runtime::endpoint(&fixture.instance()).unwrap();
        let (ready, bound) = std::sync::mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let delay = Arc::new(AtomicU64::new(0));
        let requests = Arc::new(AtomicU64::new(0));
        let inventory = Arc::new(Mutex::new(
            json!({"schemaVersion":2,"revision":1,"platform":"linux","evidence":"simulated","sources":[],"outputs":[],"mediaSessions":[]}),
        ));
        let (owned, wait, count, data) = (
            stop.clone(),
            delay.clone(),
            requests.clone(),
            inventory.clone(),
        );
        let worker = thread::spawn(move || {
            // Windows OVERLAPPED handles stay entirely on their owning worker.
            let mut listener = transport::Listener::bind(&endpoint).unwrap();
            ready.send(()).unwrap();
            while !owned.load(Ordering::Acquire) {
                let Some(mut stream) = listener.accept().unwrap() else {
                    thread::sleep(TICK);
                    continue;
                };
                let start = Instant::now();
                let mut input = vec![];
                while !input.contains(&b'\n')
                    && start.elapsed() < Duration::from_secs(3)
                    && !owned.load(Ordering::Acquire)
                {
                    let mut buf = [0u8; 1024];
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => input.extend_from_slice(&buf[..n]),
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(TICK),
                        Err(_) => break,
                    }
                }
                let Ok(request) = serde_json::from_slice::<Value>(&input) else {
                    continue;
                };
                assert_eq!(
                    request["command"], "inventory",
                    "Service must use only actual read-only inventory IPC"
                );
                count.fetch_add(1, Ordering::AcqRel);
                let waiting = Instant::now();
                while waiting.elapsed().as_millis() < wait.load(Ordering::Acquire) as u128
                    && !owned.load(Ordering::Acquire)
                {
                    thread::sleep(TICK);
                }
                let mut bytes=serde_json::to_vec(&json!({"ipcVersion":1,"requestId":request["requestId"],"result":data.lock().unwrap().clone()})).unwrap();
                bytes.push(b'\n');
                let mut sent = 0;
                while sent < bytes.len()
                    && start.elapsed() < Duration::from_secs(5)
                    && !owned.load(Ordering::Acquire)
                {
                    match stream.write(&bytes[sent..]) {
                        Ok(0) => break,
                        Ok(n) => sent += n,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(TICK),
                        Err(_) => break,
                    }
                }
                // Windows must retain its pipe until the client consumes the frame.
                while start.elapsed() < Duration::from_secs(3) && !owned.load(Ordering::Acquire) {
                    let mut b = [0u8; 1];
                    match stream.read(&mut b) {
                        Ok(0) => break,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(TICK),
                        _ => break,
                    }
                }
            }
        });
        bound.recv_timeout(Duration::from_secs(3)).unwrap();
        Self {
            stop,
            delay,
            requests,
            inventory,
            worker: Some(worker),
        }
    }
}
impl Drop for InventoryRuntime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(w) = self.worker.take() {
            w.join().unwrap();
        }
    }
}
fn config(gain: f64, revision: u64) -> Value {
    json!({"schemaVersion":2,"revision":revision,"sources":[],"mixes":[{"id":"desk","name":"Desk","channels":2,"master":{"mute":false,"gain":gain}}],"sends":[],"outputs":[],"mediaControls":[]})
}
fn settings() -> Value {
    let catalog = serde_json::to_value(mixyard::automation::standard_catalog()).unwrap();
    json!({"graph":{"schemaVersion":1,"id":"owned-policy","name":"Automatic stored intent","revision":1,"root":"choose","nodes":[
        {"kind":"decision","id":"choose","condition":{"kind":"fact","key":{"name":"manual.flag","subject":"desk"},"predicate":{"op":"eq","value":{"type":"bool","value":true}}},"onTrue":"selected","onFalse":"hold","onUnknown":"hold"},
        {"kind":"terminal","id":"selected","action":{"kind":"select","profileId":"profile-1","overrides":[]}},
        {"kind":"terminal","id":"hold","action":{"kind":"no-change"}}
    ]},"catalog":catalog,"declarations":[{"key":{"name":"manual.flag","subject":"desk"},"value":true}],
        "transitionPolicy":{"settleMs":200,"minimumHoldMs":0,"cooldownMs":200},"observer":{"applications":{},"hostLabel":null,"timezone":"Europe/Berlin"}})
}
fn setup(f: &Fixture, settings: Value) -> u64 {
    use mixyard::profiles::{Edit, Store};
    let store = Store::new(f.root.join("store")).unwrap();
    store
        .edit(
            0,
            Edit::Save {
                id: None,
                name: "Quiet".into(),
                config: serde_json::from_value(config(0.25, 7)).unwrap(),
            },
        )
        .unwrap();
    store
        .edit(
            1,
            Edit::Draft(serde_json::from_value(config(1.0, 1)).unwrap()),
        )
        .unwrap();
    // A higher unrelated saved revision proves the actual store preview is used.
    store
        .edit(
            2,
            Edit::Save {
                id: None,
                name: "High water".into(),
                config: serde_json::from_value(config(1.0, 900)).unwrap(),
            },
        )
        .unwrap();
    let settings_store =
        mixyard::automation::settings::Store::sibling_of_profiles(f.root.join("store")).unwrap();
    settings_store
        .write(0, &serde_json::from_value(settings).unwrap())
        .unwrap();
    3
}
fn status(s: &Service) -> Value {
    s.ok("/api/automation/status", json!({}))
}
fn review_arm(s: &Service) -> Value {
    let review = s.ok("/api/automation/review", json!({}));
    assert_eq!(review["blocked"], Value::Null, "{review}");
    assert_eq!(review["audioApplied"], false);
    assert_eq!(review["review"]["actions"][0]["candidate"]["revision"], 901);
    assert_eq!(
        review["review"]["actions"][0]["preview"]["plan"]["configRevision"],
        901
    );
    let armed = s.ok(
        "/api/automation/arm",
        json!({"reviewId":review["review"]["id"]}),
    );
    assert_eq!(armed["armed"], true, "{armed}");
    assert_eq!(armed["review"], Value::Null);
    armed
}
fn until(s: &Service, predicate: impl Fn(&Value) -> bool) -> Value {
    let start = Instant::now();
    loop {
        let value = status(s);
        if predicate(&value) {
            return value;
        }
        assert!(
            start.elapsed() < Duration::from_secs(12),
            "Status deadline: {value}"
        );
        thread::sleep(Duration::from_millis(30));
    }
}
#[test]
fn automatic_owned_service_stores_profile_once_uses_high_water_and_restarts_disarmed() {
    let f = Fixture::new();
    let initial = setup(&f, settings());
    let _runtime = InventoryRuntime::start(&f);
    let s = f.start();
    assert_eq!(status(&s)["armed"], false);
    review_arm(&s);
    let applied = until(&s, |v| v["storeRevision"] == initial + 1);
    assert_eq!(applied["configRevision"], 901);
    assert_eq!(applied["audioApplied"], false);
    assert_eq!(applied["backendAvailable"], false);
    let saved = s.ok("/api/profiles/read", json!({}));
    assert_eq!(saved["draft"]["mixes"][0]["master"]["gain"], 0.25);
    until(&s, |v| v["decision"]["kind"] == "already-applied");
    assert_eq!(
        s.ok("/api/profiles/read", json!({}))["storeRevision"],
        initial + 1
    );
    drop(s);
    let s = f.start();
    assert_eq!(status(&s)["armed"], false);
    assert_eq!(status(&s)["review"], Value::Null);
    assert_eq!(
        s.ok("/api/automation/settings/read", json!({}))["settingsRevision"],
        1
    );
    assert_eq!(s.ok("/api/profiles/read", json!({})), saved);
}
#[test]
fn actual_runtime_absence_blocks_review_and_live_but_keeps_context_and_edits_available() {
    let f = Fixture::new();
    setup(&f, settings());
    let s = f.start();
    let live = s.ok("/api/automation/live", json!({}));
    assert_eq!(live["blocked"]["code"], "inventory-unavailable");
    assert_eq!(live["inventory"], Value::Null);
    assert_eq!(live["status"]["inventoryRevision"], Value::Null);
    let review = s.ok("/api/automation/review", json!({}));
    assert_eq!(review["review"], Value::Null);
    assert_eq!(review["blocked"]["code"], "inventory-unavailable");
    assert_ne!(
        s.post("/api/automation/arm", json!({"reviewId":"fabricated"}))
            .status,
        200
    );
    assert_eq!(
        s.ok("/api/automation/context", json!({}))["mode"],
        "read-only-live-context"
    );
    s.edit(3, json!({"kind":"draft","config":config(0.7,0)}));
    assert_eq!(status(&s)["armed"], false);
    for path in ["live", "review", "status", "disarm"] {
        assert_eq!(
            s.post(
                &format!("/api/automation/{path}"),
                json!({"snapshot":{},"facts":[],"inventory":{}})
            )
            .status,
            400
        );
    }
    assert_eq!(
        s.post(
            "/api/automation/arm",
            json!({"reviewId":"id","reviewed":true})
        )
        .status,
        400
    );
}
#[test]
fn timed_ownership_pause_resumes_after_fresh_settle_and_clear_never_arms() {
    let f = Fixture::new();
    let mut options = settings();
    options["transitionPolicy"]["settleMs"] = json!(800);
    setup(&f, options);
    let _runtime = InventoryRuntime::start(&f);
    let s = f.start();
    s.ok(
        "/api/automation/manual",
        json!({"mode":"for","durationMs":300}),
    );
    thread::sleep(Duration::from_millis(350));
    assert_eq!(status(&s)["armed"], false);
    s.ok("/api/automation/manual", json!({"mode":"clear"}));
    review_arm(&s);
    until(&s, |v| v["decision"]["kind"] == "waiting");
    let paused = s.ok(
        "/api/automation/manual",
        json!({"mode":"for","durationMs":800}),
    );
    assert_eq!(paused["armed"], true);
    until(&s, |v| v["decision"]["kind"] == "manual-override");
    assert_eq!(s.ok("/api/profiles/read", json!({}))["storeRevision"], 3);
    until(&s, |v| {
        v["manualOverride"].is_null() && v["decision"]["kind"] == "waiting"
    });
    assert_eq!(
        s.ok("/api/profiles/read", json!({}))["storeRevision"],
        3,
        "Blocked time must not accrue settle"
    );
    until(&s, |v| v["storeRevision"] == 4);
    let paused = s.ok("/api/automation/manual", json!({"mode":"until-cleared"}));
    assert_eq!(paused["armed"], true);
    let cleared = s.ok("/api/automation/manual", json!({"mode":"clear"}));
    assert_eq!(cleared["armed"], true);
    s.ok("/api/automation/disarm", json!({}));
    s.ok("/api/automation/manual", json!({"mode":"clear"}));
    assert_eq!(status(&s)["armed"], false);
}
#[test]
fn manual_profile_edit_and_external_cli_revoke_automatic_ownership() {
    let f = Fixture::new();
    let mut options = settings();
    options["transitionPolicy"]["settleMs"] = json!(3000);
    setup(&f, options);
    let _runtime = InventoryRuntime::start(&f);
    let s = f.start();
    review_arm(&s);
    s.edit(3, json!({"kind":"draft","config":config(0.7,0)}));
    assert_eq!(status(&s)["armed"], false);
    assert_eq!(status(&s)["manualOverride"]["mode"], "until-cleared");
    let review = s.ok("/api/automation/review", json!({}));
    s.ok(
        "/api/automation/arm",
        json!({"reviewId":review["review"]["id"]}),
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mixyard"))
        .args(["profiles", "--store"])
        .arg(f.root.join("store"))
        .args(["rename", "4", "profile-1", "External rename"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let observed = until(&s, |v| v["storeRevision"] == 5);
    assert_eq!(observed["armed"], false);
    assert_eq!(observed["manualOverride"]["mode"], "until-cleared");
    assert_eq!(observed["review"], Value::Null);
}
#[test]
fn settings_catalog_and_inventory_changes_revoke_review_and_arming() {
    let f = Fixture::new();
    setup(&f, settings());
    let runtime = InventoryRuntime::start(&f);
    let s = f.start();
    let review = s.ok("/api/automation/review", json!({}));
    let mut options = settings();
    options["catalog"][0]["maxAgeMs"] = json!(4000);
    s.ok(
        "/api/automation/settings/write",
        json!({"expectedSettingsRevision":1,"settings":options}),
    );
    assert_ne!(
        s.post(
            "/api/automation/arm",
            json!({"reviewId":review["review"]["id"]})
        )
        .status,
        200
    );
    assert_eq!(
        s.post(
            "/api/automation/settings/write",
            json!({"expectedSettingsRevision":1,"settings":settings()})
        )
        .status,
        409
    );
    let mut options = settings();
    options["transitionPolicy"]["settleMs"] = json!(3000);
    s.ok(
        "/api/automation/settings/write",
        json!({"expectedSettingsRevision":2,"settings":options}),
    );
    review_arm(&s);
    runtime.inventory.lock().unwrap()["revision"] = json!(2);
    until(&s, |v| v["inventoryRevision"] == 2 && v["armed"] == false);
    assert_eq!(s.ok("/api/profiles/read", json!({}))["storeRevision"], 3);
}
#[test]
fn failed_store_attempt_has_cooldown_no_hold_and_retries_after_recovery() {
    let f = Fixture::new();
    let mut options = settings();
    options["transitionPolicy"] = json!({"settleMs":100,"minimumHoldMs":10_000,"cooldownMs":800});
    setup(&f, options);
    let _runtime = InventoryRuntime::start(&f);
    let s = f.start();
    fs::write(f.root.join("store/write.lock"), "").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            f.root.join("store/write.lock"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    review_arm(&s);
    let failed = until(&s, |v| v["decision"]["kind"] == "write-failed");
    assert_eq!(failed["lastAppliedAtMs"], Value::Null);
    assert_eq!(failed["storeRevision"], 3);
    fs::remove_file(f.root.join("store/write.lock")).unwrap();
    let waiting = until(&s, |v| v["decision"]["kind"] == "waiting");
    assert_eq!(waiting["decision"]["holdRemainingMs"], 0);
    assert!(waiting["decision"]["cooldownRemainingMs"].as_u64().unwrap() > 0);
    until(&s, |v| v["storeRevision"] == 4);
}
#[test]
fn disarm_and_manual_edit_cancel_inflight_ipc_without_worker_starvation() {
    let f = Fixture::new();
    let mut options = settings();
    options["transitionPolicy"]["settleMs"] = json!(0);
    setup(&f, options);
    let runtime = InventoryRuntime::start(&f);
    let s = f.start();
    review_arm(&s);
    runtime.delay.store(1900, Ordering::Release);
    let before = runtime.requests.load(Ordering::Acquire);
    let start = Instant::now();
    while runtime.requests.load(Ordering::Acquire) == before {
        assert!(start.elapsed() < Duration::from_secs(4));
        thread::sleep(TICK);
    }
    let start = Instant::now();
    let disarmed = s.ok("/api/automation/disarm", json!({}));
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(disarmed["armed"], false);
    runtime.delay.store(0, Ordering::Release);
    assert_eq!(s.ok("/api/profiles/read", json!({}))["storeRevision"], 3);
    let review = s.ok("/api/automation/review", json!({}));
    s.ok(
        "/api/automation/arm",
        json!({"reviewId":review["review"]["id"]}),
    );
    runtime.delay.store(1900, Ordering::Release);
    let before = runtime.requests.load(Ordering::Acquire);
    let start = Instant::now();
    while runtime.requests.load(Ordering::Acquire) == before {
        assert!(start.elapsed() < Duration::from_secs(4));
        thread::sleep(TICK);
    }
    let edited = s.edit(3, json!({"kind":"draft","config":config(0.8,0)}));
    assert_eq!(edited["storeRevision"], 4);
    runtime.delay.store(0, Ordering::Release);
    assert_eq!(status(&s)["armed"], false);
    assert_eq!(
        s.ok("/api/profiles/read", json!({}))["draft"]["mixes"][0]["master"]["gain"],
        0.8
    );
}

#[cfg(target_os = "linux")]
fn observer_fixture(f: &Fixture, state: Value) -> std::process::Command {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir(f.root.join("tools")).unwrap();
    fs::write(f.root.join("observer-state.json"), state.to_string()).unwrap();
    // This substitutes only the local observation boundary through a test-owned
    // PATH. The production Collector still runs, bounds, timestamps and cancels
    // its actual child. It does not replace evaluation, preview, IPC or writing.
    fs::write(f.root.join("tools/python3"),r#"#!/usr/bin/python3
import json, os, pathlib, sys, time
root=pathlib.Path(os.environ['MIXYARD_TEST_OBSERVER_ROOT'])
json.load(sys.stdin)
state=json.loads((root/'observer-state.json').read_text())
(root/'capture-started').write_text(str(time.monotonic_ns()))
time.sleep(state.get('delayMs',0)/1000)
names=[('application.running','linux.proc'),('power.source','linux.sysfs'),('power.battery-percent','linux.sysfs'),('civil.timezone','civil.zoneinfo'),('civil.date','civil.zoneinfo'),('civil.weekday','civil.zoneinfo'),('civil.minute-of-day','civil.zoneinfo'),('device.connected','linux.pipewire')]
facts=[]
for name,provider in names:
    known=name in ['application.running','device.connected']
    facts.append({'key':{'name':name},'status':state.get('status','known') if name=='application.running' else ('known' if known else 'unknown'),'value':{'type':'text-set','value':state.get('running',[]) if name=='application.running' else ['one-visible-endpoint']} if known else None,'provider':provider,'evidence':'platform-observed','observedAtMs':None,'validForMs':5000,'completeness':state.get('completeness','complete') if name=='application.running' else 'complete','reason':None if known else 'test-provider-unavailable'})
print(json.dumps(facts))
"#).unwrap();
    fs::set_permissions(
        f.root.join("tools/python3"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let mut command = f.command();
    command
        .env(
            "PATH",
            format!(
                "{}:{}",
                f.root.join("tools").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("MIXYARD_TEST_OBSERVER_ROOT", &f.root);
    command
}
#[cfg(target_os = "linux")]
fn running_settings() -> Value {
    let mut options = settings();
    options["observer"]["applications"] = json!({"/opt/mixyard-test-player":"mapped-present","/opt/mixyard-test-absent":"mapped-absent"});
    options["graph"]["nodes"][0]["condition"] = json!({"kind":"fact","key":{"name":"application.running"},"predicate":{"op":"contains","value":"mapped-present"}});
    options
}
#[cfg(target_os = "linux")]
#[test]
fn live_domain_restriction_and_explicit_declarations_preserve_unknowns() {
    let f = Fixture::new();
    let mut options = running_settings();
    options["graph"]["nodes"][0]["condition"] = json!({"kind":"not","condition":{"kind":"fact","key":{"name":"application.running"},"predicate":{"op":"contains","value":"unmapped"}}});
    setup(&f, options.clone());
    let _runtime = InventoryRuntime::start(&f);
    let s = f.start_command(observer_fixture(&f, json!({"running":["mapped-present"]})));
    let live = s.ok("/api/automation/live", json!({}));
    assert_eq!(live["evaluation"]["terminal"], "hold");
    assert_eq!(
        live["evaluation"]["trace"][0]["detail"]["unknownReason"],
        "outside-application-mapping-domain"
    );
    assert_eq!(
        live["context"]["applicationScope"],
        json!(["mapped-absent", "mapped-present"])
    );
    let request: mixyard::automation::Request =
        serde_json::from_value(live["request"].clone()).unwrap();
    let evaluation = mixyard::automation::evaluate_scoped(
        &request,
        &["mapped-absent".into(), "mapped-present".into()],
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(evaluation).unwrap(),
        live["evaluation"]
    );
    // A visible endpoint is not a hardware-monitor declaration.
    options["graph"]["nodes"][0]["condition"] = json!({"kind":"fact","key":{"name":"device.hardware-monitor-declared","subject":"one-visible-endpoint"},"predicate":{"op":"eq","value":{"type":"bool","value":true}}});
    s.ok(
        "/api/automation/settings/write",
        json!({"expectedSettingsRevision":1,"settings":options}),
    );
    let unknown = s.ok("/api/automation/live", json!({}));
    assert_eq!(unknown["evaluation"]["terminal"], "hold");
    assert_eq!(
        unknown["evaluation"]["trace"][0]["detail"]["result"],
        "unknown"
    );
    options["declarations"] = json!([{"key":{"name":"device.hardware-monitor-declared","subject":"one-visible-endpoint"},"value":true}]);
    s.ok(
        "/api/automation/settings/write",
        json!({"expectedSettingsRevision":2,"settings":options}),
    );
    let declared = s.ok("/api/automation/live", json!({}));
    assert_eq!(declared["evaluation"]["terminal"], "selected");
    let fact = declared["evaluation"]["observations"][0].clone();
    assert_eq!(fact["evidence"], "user-declared");
    assert_eq!(fact["provider"], "local.declaration");
    options["declarations"][0]["evidence"] = json!("platform-observed");
    assert_eq!(
        s.post(
            "/api/automation/settings/write",
            json!({"expectedSettingsRevision":3,"settings":options})
        )
        .status,
        400
    );
}
#[cfg(target_os = "linux")]
#[test]
fn actual_worker_flaps_restart_settle_and_slow_owned_observer_is_cancelled() {
    let f = Fixture::new();
    let mut options = running_settings();
    options["transitionPolicy"]["settleMs"] = json!(1000);
    setup(&f, options);
    let _runtime = InventoryRuntime::start(&f);
    let s = f.start_command(observer_fixture(&f, json!({"running":["mapped-present"]})));
    review_arm(&s);
    until(&s, |v| v["decision"]["kind"] == "waiting");
    fs::write(
        f.root.join("observer-state.json"),
        json!({"running":[]}).to_string(),
    )
    .unwrap();
    until(&s, |v| v["decision"]["kind"] == "no-change");
    assert_eq!(s.ok("/api/profiles/read", json!({}))["storeRevision"], 3);
    fs::write(
        f.root.join("observer-state.json"),
        json!({"running":["mapped-present"]}).to_string(),
    )
    .unwrap();
    until(&s, |v| v["decision"]["kind"] == "waiting");
    assert_eq!(s.ok("/api/profiles/read", json!({}))["storeRevision"], 3);
    until(&s, |v| v["storeRevision"] == 4);
    // A slow helper must leave status/config validation/store reads responsive.
    let marker = fs::read_to_string(f.root.join("capture-started")).unwrap();
    fs::write(
        f.root.join("observer-state.json"),
        json!({"running":["mapped-present"],"delayMs":3500}).to_string(),
    )
    .unwrap();
    let start = Instant::now();
    while fs::read_to_string(f.root.join("capture-started")).unwrap() == marker {
        assert!(start.elapsed() < Duration::from_secs(4));
        thread::sleep(TICK);
    }
    assert_eq!(
        s.ok("/api/validate", json!({"config":config(1.0,1)}))["valid"],
        true
    );
    assert_eq!(
        s.post("/api/automation/context", json!({})).json()["error"]["code"],
        "context-busy"
    );
    let start = Instant::now();
    assert_eq!(s.ok("/api/automation/disarm", json!({}))["armed"], false);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(s.ok("/api/profiles/read", json!({}))["storeRevision"], 4);
}

#[cfg(target_os = "linux")]
#[test]
fn changed_observation_can_select_another_reviewed_terminal_after_successful_hold() {
    let f = Fixture::new();
    let mut options = running_settings();
    options["transitionPolicy"] = json!({"settleMs":100,"minimumHoldMs":1500,"cooldownMs":100});
    options["graph"]["nodes"][0]["onFalse"] = json!("other");
    options["graph"]["nodes"].as_array_mut().unwrap().push(json!({"kind":"terminal","id":"other","action":{"kind":"select","profileId":"profile-3","overrides":[]}}));
    setup(&f, options);
    let _runtime = InventoryRuntime::start(&f);
    let s = f.start_command(observer_fixture(&f, json!({"running":["mapped-present"]})));
    let review = s.ok("/api/automation/review", json!({}));
    assert_eq!(review["review"]["actions"].as_array().unwrap().len(), 2);
    s.ok(
        "/api/automation/arm",
        json!({"reviewId":review["review"]["id"]}),
    );
    until(&s, |v| v["storeRevision"] == 4);
    fs::write(
        f.root.join("observer-state.json"),
        json!({"running":[]}).to_string(),
    )
    .unwrap();
    let held = until(&s, |v| {
        v["decision"]["kind"] == "waiting"
            && v["decision"]["holdRemainingMs"]
                .as_u64()
                .is_some_and(|n| n > 0)
    });
    assert_eq!(held["storeRevision"], 4);
    assert_eq!(held["armed"], true);
    let changed = until(&s, |v| v["storeRevision"] == 5);
    assert_eq!(changed["armed"], true);
    assert_eq!(
        s.ok("/api/profiles/read", json!({}))["draft"]["mixes"][0]["master"]["gain"],
        1.0
    );
}

#[test]
fn review_surfaces_sensitive_paths_and_preserves_consent_exclusions_and_media_policy() {
    use mixyard::profiles::{Edit, Store};
    let f = Fixture::new();
    let mut options = settings();
    options["graph"]["nodes"][1]["action"]["overrides"] =
        json!([{"scope":"send","source":"mic","mix":"desk","gain":0.1,"mute":true}]);
    let mut current = config(1.0, 0);
    current["sources"] = json!([{"id":"mic","selector":{"kind":"mic","deviceUid":"explicit-mic"}}]);
    current["mixes"][0]["excludeGroups"] = json!(["call-return"]);
    current["sends"] = json!([{"source":"mic","mix":"desk","gain":0.25}]);
    current["outputs"] = json!([
        {"id":"monitor","mix":"desk","target":{"kind":"device","deviceUid":"explicit-headphones"},"gain":1,"allowMicMonitor":true},
        {"id":"recorder","mix":"desk","target":{"kind":"virtual-input","endpointId":"explicit-recording"},"gain":1}
    ]);
    current["mediaControls"] =
        json!([{"deviceUid":"explicit-headphones","policy":{"mode":"os-default"}}]);
    let store = Store::new(f.root.join("store")).unwrap();
    store
        .edit(
            0,
            Edit::Save {
                id: None,
                name: "Explicit stored consent".into(),
                config: serde_json::from_value(current.clone()).unwrap(),
            },
        )
        .unwrap();
    store
        .edit(
            1,
            Edit::Draft(serde_json::from_value(config(1.0, 0)).unwrap()),
        )
        .unwrap();
    mixyard::automation::settings::Store::sibling_of_profiles(f.root.join("store"))
        .unwrap()
        .write(0, &serde_json::from_value(options).unwrap())
        .unwrap();
    let _runtime = InventoryRuntime::start(&f);
    let s = f.start();
    let review = s.ok("/api/automation/review", json!({}));
    assert_eq!(review["blocked"], Value::Null, "{review}");
    let paths = review["review"]["actions"][0]["preview"]["affectedSensitivePathsAfter"]
        .as_array()
        .unwrap();
    assert!(paths.iter().any(|p| p["reason"] == "microphone-monitoring"));
    assert!(paths.iter().any(|p| p["reason"] == "potential-recording"));
    s.ok(
        "/api/automation/arm",
        json!({"reviewId":review["review"]["id"]}),
    );
    until(&s, |v| v["storeRevision"] == 3);
    let draft = s.ok("/api/profiles/read", json!({}))["draft"].clone();
    assert_eq!(draft["outputs"][0]["allowMicMonitor"], true);
    assert_eq!(
        draft["mixes"][0]["excludeGroups"],
        current["mixes"][0]["excludeGroups"]
    );
    assert_eq!(draft["mediaControls"], current["mediaControls"]);
    assert_eq!(draft["sends"][0]["gain"], 0.1);
    assert_eq!(draft["sends"][0]["mute"], true);
    assert_eq!(status(&s)["audioApplied"], false);
    assert_eq!(status(&s)["backendAvailable"], false);
}
#[test]
fn aggregate_review_admission_rejects_large_catalogs_before_expanding_paths() {
    let f = Fixture::new();
    setup(&f, settings());
    let store = mixyard::profiles::Store::new(f.root.join("store")).unwrap();
    for revision in 3..90 {
        store
            .edit(
                revision,
                mixyard::profiles::Edit::Save {
                    id: None,
                    name: format!("Profile {revision}"),
                    config: serde_json::from_value(config(1.0, 0)).unwrap(),
                },
            )
            .unwrap();
    }
    let _runtime = InventoryRuntime::start(&f);
    let s = f.start();
    let review = s.ok("/api/automation/review", json!({}));
    assert_eq!(review["review"], Value::Null);
    assert_eq!(review["blocked"]["code"], "resource-limit");
    assert_eq!(s.ok("/api/profiles/read", json!({}))["storeRevision"], 90);
    assert_eq!(status(&s)["armed"], false);
}

fn assert_offline_tools(s: &Service) {
    let result=s.ok("/api/dsp/preview",json!({"config":{"name":"Fallback proof","format":{"sampleRateHz":48000,"channels":["mono"]},"effects":[{"name":"Gain","enabled":true,"effect":{"kind":"gain","gain":2,"invertPolarity":false}}]},"frames":2,"iterations":1,"bypassed":false,"signal":{"kind":"samples","samples":[0.125,-0.25]}}));
    assert_eq!(result["output"], json!([0.25, -0.5]));
    let request: Value = serde_json::from_str(include_str!(
        "../examples/automation/headphone-priority.json"
    ))
    .unwrap();
    let simulation = s.ok("/api/automation/evaluate", request);
    assert_eq!(simulation["evaluation"]["terminal"], "headphones");
    assert_eq!(simulation["evaluation"]["audioApplied"], false);
}
#[test]
fn corrupt_settings_disable_only_automatic_owner_and_preserve_manual_console_and_bytes() {
    let f = Fixture::new();
    setup(&f, settings());
    let store =
        mixyard::automation::settings::Store::sibling_of_profiles(f.root.join("store")).unwrap();
    let path = store.directory().join("settings.json");
    let original = fs::read(&path).unwrap();
    let corrupt = b"{broken-settings-preserve";
    fs::write(&path, corrupt).unwrap();
    let s = f.start();
    assert_offline_tools(&s);
    let unavailable = s.post("/api/automation/status", json!({}));
    assert_eq!(unavailable.status, 503);
    assert_eq!(
        unavailable.json()["error"]["code"],
        "automation-unavailable"
    );
    assert!(
        !unavailable.json().get("settingsRevision").is_some(),
        "No fake revision-zero status"
    );
    s.edit(3, json!({"kind":"draft","config":config(0.6,0)}));
    assert_eq!(
        s.ok("/api/profiles/read", json!({}))["draft"]["mixes"][0]["master"]["gain"],
        0.6
    );
    assert_ne!(
        s.post("/api/automation/settings/read", json!({})).status,
        200
    );
    assert_eq!(
        s.post(
            "/api/automation/settings/write",
            json!({"expectedSettingsRevision":0,"settings":settings()})
        )
        .status,
        503
    );
    assert_eq!(
        s.post("/api/automation/arm", json!({"reviewId":"fabricated"}))
            .status,
        503
    );
    assert_eq!(fs::read(&path).unwrap(), corrupt);
    assert!(!store.directory().join("write.lock").exists());
    assert!(!store.directory().join("pending.json").exists());
    drop(s);
    fs::write(&path, &original).unwrap();
    let recovered = f.start();
    assert_eq!(status(&recovered)["armed"], false);
    assert_eq!(
        recovered.ok("/api/automation/settings/read", json!({}))["settingsRevision"],
        1
    );
}
#[test]
fn corrupt_profile_store_keeps_offline_console_available_and_never_overwrites_corruption() {
    let f = Fixture::new();
    setup(&f, settings());
    let path = f.root.join("store/profiles.json");
    let original = fs::read(&path).unwrap();
    let corrupt = b"{broken-profile-preserve";
    fs::write(&path, corrupt).unwrap();
    let s = f.start();
    assert_offline_tools(&s);
    assert_eq!(s.post("/api/automation/status", json!({})).status, 503);
    assert_eq!(s.post("/api/profiles/read", json!({})).status, 500);
    let edit = s.post(
        "/api/profiles/edit",
        json!({"expectedRevision":0,"edit":{"kind":"draft","config":config(0.7,0)}}),
    );
    assert_eq!(edit.status, 500);
    assert_eq!(edit.json()["error"]["code"], "corrupt-store");
    assert_eq!(fs::read(&path).unwrap(), corrupt);
    assert!(!f.root.join("store/write.lock").exists());
    assert!(!f.root.join("store/pending.json").exists());
    assert_eq!(
        s.ok("/api/automation/settings/read", json!({}))["settingsRevision"],
        1
    );
    drop(s);
    fs::write(&path, &original).unwrap();
    let recovered = f.start();
    assert_eq!(status(&recovered)["armed"], false);
    assert_eq!(
        recovered.ok("/api/profiles/read", json!({}))["storeRevision"],
        3
    );
}
