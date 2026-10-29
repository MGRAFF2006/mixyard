//! Actual owned helper/runtime/typed IPC tests. All helper evidence is simulated.
#![cfg(target_os = "linux")]
#[path = "web_support/mod.rs"]
mod support;
use mixyard::{
    runtime::{self, ReadOnlyClient, ServerError},
    Evidence,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::{fs::PermissionsExt, net::UnixStream, process::CommandExt},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};
static NEXT: AtomicU32 = AtomicU32::new(0);

// comm can contain both whitespace and parentheses; fields start after its last ')'.
fn process_stat(pid: u32) -> Option<String> {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => Some(stat),
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                || e.raw_os_error() == Some(libc::ESRCH) =>
        {
            None
        }
        Err(e) => panic!("read stat for owned PID {pid}: {e}"),
    }
}
fn stat_fields(stat: &str) -> Vec<&str> {
    stat.rsplit_once(") ")
        .unwrap()
        .1
        .split_whitespace()
        .collect()
}
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn sentinel() -> OwnedChild {
    OwnedChild(
        Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap(),
    )
}
struct OwnedDescendant {
    pid: u32,
    start_time: String,
}
impl OwnedDescendant {
    fn terminate(&self) {
        if let Some(stat) = process_stat(self.pid) {
            // Never signal a reused PID during failure cleanup.
            if stat_fields(&stat)[19] == self.start_time {
                if unsafe { libc::kill(self.pid as i32, libc::SIGKILL) } != 0 {
                    let error = std::io::Error::last_os_error();
                    // The same owned task may disappear after stat but before
                    // signal delivery. Only ESRCH is idempotent absence.
                    assert_eq!(error.raw_os_error(), Some(libc::ESRCH), "{error}");
                }
            }
        }
    }
}
impl Drop for OwnedDescendant {
    fn drop(&mut self) {
        self.terminate();
    }
}
fn graph(name: &str) -> Value {
    json!([
        {"id":0,"type":"PipeWire:Interface:Core","info":{"cookie":100,"props":{}}},
        {"id":10,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"node.name":name,"media.class":"Audio/Source","object.serial":9007199254740993i64},"params":{"Format":[{"mediaType":"audio","mediaSubtype":"raw","rate":44100,"channels":1,"position":["MONO"]}]}}},
        {"id":20,"type":"PipeWire:Interface:Node","info":{"state":"idle","props":{"node.name":"speaker","media.class":"Audio/Sink"}}}
    ])
}
struct Runtime {
    instance: String,
    directory: PathBuf,
    child: Option<Child>,
}
impl Runtime {
    fn new() -> Self {
        let instance = format!(
            "inv-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        Self::named(instance)
    }
    fn named(instance: String) -> Self {
        let directory = PathBuf::from(runtime::endpoint(&instance).unwrap())
            .parent()
            .unwrap()
            .to_path_buf();
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let r = Self {
            instance,
            directory,
            child: None,
        };
        fs::write(
            r.directory.join("helper"),
            include_bytes!("runtime_support/linux_inventory_helper.py"),
        )
        .unwrap();
        fs::set_permissions(
            r.directory.join("helper"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        r.mode("ok");
        r.graph(&graph("mic"));
        r
    }
    fn mode(&self, mode: &str) {
        fs::write(self.directory.join("mode"), mode).unwrap();
    }
    fn graph(&self, graph: &Value) {
        fs::write(
            self.directory.join("graph"),
            serde_json::to_vec(graph).unwrap(),
        )
        .unwrap();
    }
    fn start(&mut self) {
        self.child = Some(
            Command::new(env!("CARGO_BIN_EXE_mixyard-runtime"))
                .args([
                    "serve",
                    "--instance",
                    &self.instance,
                    "--inventory-fixture-helper",
                ])
                .arg(self.directory.join("helper"))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while ReadOnlyClient::connect(&self.instance).is_err() {
            assert!(Instant::now() < deadline, "runtime startup");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn inventory(&self) -> mixyard::Inventory {
        ReadOnlyClient::inventory(&self.instance, &AtomicBool::new(false)).unwrap()
    }
    fn error(&self) -> ServerError {
        let start = Instant::now();
        let e = ReadOnlyClient::inventory(&self.instance, &AtomicBool::new(false)).unwrap_err();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "unavailable must arrive before typed deadline: {e}"
        );
        e.get_ref()
            .unwrap()
            .downcast_ref::<ServerError>()
            .unwrap()
            .clone()
    }
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            runtime::client(&self.instance, "shutdown", &mut Vec::new()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while child.try_wait().unwrap().is_none() {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    fn wait_pid(&self) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Ok(s) = fs::read_to_string(self.directory.join("pid")) {
                return s.parse().unwrap();
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn descendant(&self) -> u32 {
        fs::read_to_string(self.directory.join("descendant"))
            .unwrap()
            .parse()
            .unwrap()
    }
    fn assert_owned_group(&self, pid: u32) {
        assert_eq!(
            unsafe { libc::getpgid(self.descendant() as i32) },
            pid as i32
        );
    }
    fn reaped(&self, pid: u32) {
        let deadline = Instant::now() + Duration::from_secs(1);
        let descendant = self.descendant();
        loop {
            let leader = process_stat(pid);
            let child = process_stat(descendant);
            // Group SIGKILL and leader wait do not synchronize descendant exit.
            // Only a zombie or absence proves it stopped; pending SIGKILL does not.
            let stopped = child
                .as_deref()
                .is_none_or(|stat| stat_fields(stat)[0] == "Z");
            if leader.is_none() && stopped {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "owned discovery cleanup incomplete: leader={leader:?}, descendant={child:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}
#[test]
fn actual_helper_to_typed_ipc_preserves_precision_formats_and_monotonic_restart_revision() {
    let mut r = Runtime::new();
    r.start();
    let first = r.inventory();
    assert_eq!(first.evidence, Evidence::Simulated);
    assert!(first.sources[0]
        .observed_id
        .starts_with("pipewire:object:10:9007199254740993:"));
    assert_eq!(first.sources[0].format.unwrap().sample_rate_hz, 44100);
    assert!(first.outputs[0].format.is_none());
    assert_eq!(r.inventory().revision, first.revision);
    r.stop();
    r.start();
    assert_eq!(r.inventory().revision, first.revision);
    r.graph(&graph("new-mic"));
    let changed = r.inventory();
    assert!(changed.revision > first.revision);
    r.graph(&graph("mic"));
    assert!(r.inventory().revision > changed.revision);
    let before_detail = r.inventory();
    let mut detail = graph("mic");
    detail[1]["info"]["params"]["Format"][0]["format"] = json!("S24LE");
    r.graph(&detail);
    let after_detail = r.inventory();
    assert_eq!(
        after_detail.sources[0].format,
        before_detail.sources[0].format
    );
    assert!(
        after_detail.revision > before_detail.revision,
        "unrepresented PCM encoding must still invalidate review"
    );
    let snapshot = ReadOnlyClient::connect(&r.instance)
        .unwrap()
        .initial_snapshot()
        .clone();
    assert!(!snapshot.availability.backend_available);
    let output = Command::new(env!("CARGO_BIN_EXE_mixyard-runtime"))
        .args(["inventory", "--instance", &r.instance])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["result"]["evidence"],
        "simulated"
    );
    r.stop();
}
#[test]
fn malformed_partial_oversize_permission_and_timeout_never_become_empty_success() {
    let mut r = Runtime::new();
    r.start();
    for raw in [
        "[]",
        "{}",
        "[",
        "[{\"id\":0,\"id\":0,\"type\":\"PipeWire:Interface:Core\"}]",
    ] {
        fs::write(r.directory.join("graph"), raw).unwrap();
        assert_eq!(r.error().code, "inventory-unavailable");
    }
    r.graph(&graph("mic"));
    for mode in ["stderr", "fail", "oversize", "hang"] {
        r.mode(mode);
        let e = r.error();
        assert_eq!(e.code, "inventory-unavailable");
        assert!(!e.message.contains("private bind"));
    }
    r.reaped(r.wait_pid());
    r.mode("ok");
    assert!(!r.inventory().sources.is_empty());
    r.stop();
}
#[test]
fn disconnect_cancel_busy_and_shutdown_kill_only_owned_discovery_process_group() {
    let mut unrelated = sentinel();
    let mut r = Runtime::new();
    r.mode("hang");
    r.start();
    let mut stream = UnixStream::connect(runtime::endpoint(&r.instance).unwrap()).unwrap();
    stream
        .write_all(b"{\"ipcVersion\":1,\"requestId\":\"held\",\"command\":\"inventory\"}\n")
        .unwrap();
    let pid = r.wait_pid();
    r.assert_owned_group(pid);
    assert_eq!(r.error().code, "busy");
    assert!(ReadOnlyClient::connect(&r.instance).is_ok());
    drop(stream);
    r.reaped(pid);
    assert!(unrelated.0.try_wait().unwrap().is_none());
    fs::remove_file(r.directory.join("pid")).unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    let instance = r.instance.clone();
    let client = thread::spawn(move || ReadOnlyClient::inventory(&instance, &signal));
    let pid = r.wait_pid();
    r.assert_owned_group(pid);
    cancelled.store(true, Ordering::Release);
    assert_eq!(
        client.join().unwrap().unwrap_err().kind(),
        std::io::ErrorKind::Interrupted
    );
    r.reaped(pid);
    assert!(unrelated.0.try_wait().unwrap().is_none());
    fs::remove_file(r.directory.join("pid")).unwrap();
    let mut stream = UnixStream::connect(runtime::endpoint(&r.instance).unwrap()).unwrap();
    stream
        .write_all(b"{\"ipcVersion\":1,\"requestId\":\"stop\",\"command\":\"inventory\"}\n")
        .unwrap();
    let pid = r.wait_pid();
    r.assert_owned_group(pid);
    r.stop();
    r.reaped(pid);
    assert!(unrelated.0.try_wait().unwrap().is_none());
}

#[test]
fn process_stat_accepts_concurrent_owned_reaping_as_absence() {
    // /proc can report ESRCH as well as ENOENT while a task exits. Exercise the
    // actual reader against owned kill/wait races, never an unrelated process.
    for _ in 0..128 {
        let mut child = sentinel();
        let pid = child.0.id();
        let (tx, rx) = mpsc::channel();
        thread::scope(|scope| {
            let killer = scope.spawn(move || {
                rx.recv().unwrap();
                child.0.kill().unwrap();
                child.0.wait().unwrap();
            });
            tx.send(()).unwrap();
            for _ in 0..10_000 {
                if process_stat(pid).is_none() {
                    break;
                }
            }
            killer.join().unwrap();
        });
        assert!(process_stat(pid).is_none());
    }
}

#[test]
fn descendant_cleanup_is_idempotent_during_concurrent_owned_reaping() {
    for _ in 0..128 {
        let mut child = sentinel();
        let pid = child.0.id();
        let stat = process_stat(pid).unwrap();
        let descendant = OwnedDescendant {
            pid,
            start_time: stat_fields(&stat)[19].to_owned(),
        };
        let (tx, rx) = mpsc::channel();
        thread::scope(|scope| {
            let killer = scope.spawn(move || {
                rx.recv().unwrap();
                let _ = child.0.kill();
                child.0.wait().unwrap();
            });
            tx.send(()).unwrap();
            for _ in 0..10_000 {
                descendant.terminate();
                if process_stat(pid).is_none() {
                    break;
                }
            }
            killer.join().unwrap();
        });
        descendant.terminate();
        assert!(process_stat(pid).is_none());
    }
}

#[test]
fn cleanup_observer_waits_for_live_descendant_after_leader_is_reaped() {
    let r = Runtime::new();
    r.mode("orphan");
    let mut unrelated = sentinel();
    let mut helper = OwnedChild(
        // Execute the stable interpreter; the newly written owned script is
        // input data, not a text executable subject to concurrent ETXTBSY.
        Command::new("/usr/bin/python3")
            .arg(r.directory.join("helper"))
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let pid = r.wait_pid();
    r.assert_owned_group(pid);
    let descendant = r.descendant();
    let stat = process_stat(descendant).unwrap();
    let descendant = OwnedDescendant {
        pid: descendant,
        start_time: stat_fields(&stat)[19].to_owned(),
    };
    assert!(stat.contains("(owned (child))"), "{stat}");
    assert!(helper.0.wait().unwrap().success());
    assert!(process_stat(pid).is_none());

    let (waited, completed) = thread::scope(|scope| {
        let (tx, rx) = mpsc::channel();
        let runtime = &r;
        let observer = scope.spawn(move || {
            runtime.reaped(pid);
            tx.send(()).unwrap();
        });
        // A live descendant must hold the observer even though the leader is gone.
        let waited = matches!(
            rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        descendant.terminate();
        (waited, observer.join().is_ok())
    });
    assert!(
        waited,
        "observer finished before the live owned descendant stopped"
    );
    assert!(completed, "observer failed after descendant termination");
    assert!(unrelated.0.try_wait().unwrap().is_none());
}
#[test]
fn insecure_corrupt_and_symlink_revision_state_fail_closed_across_restart() {
    use std::os::unix::fs::symlink;
    let mut r = Runtime::new();
    r.start();
    let first = r.inventory();
    r.stop();
    let state = r.directory.join("inventory-revision-v1");
    let original = fs::read(&state).unwrap();
    assert_eq!(original.len(), 80);
    fs::set_permissions(&state, fs::Permissions::from_mode(0o644)).unwrap();
    r.start();
    assert_eq!(r.error().code, "inventory-unavailable");
    r.stop();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&state, b"corrupt").unwrap();
    r.start();
    assert_eq!(r.error().code, "inventory-unavailable");
    r.stop();
    let mut flipped = original.clone();
    flipped[9] ^= 1;
    fs::write(&state, &flipped).unwrap();
    r.start();
    assert_eq!(r.error().message, "inventory-state-corrupt-do-not-reset");
    r.stop();
    fs::write(&state, &original).unwrap();
    r.start();
    assert_eq!(r.inventory().revision, first.revision);
    r.stop();
    let mut exhausted = original.clone();
    exhausted[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
    use sha2::{Digest, Sha256};
    let checksum = Sha256::digest(&exhausted[..48]);
    exhausted[48..].copy_from_slice(&checksum);
    fs::write(&state, &exhausted).unwrap();
    r.graph(&graph("changed"));
    r.start();
    assert_eq!(r.error().message, "inventory-revision-exhausted");
    r.stop();
    assert_eq!(fs::read(&state).unwrap(), exhausted);
    fs::write(&state, &original).unwrap();
    fs::rename(&state, r.directory.join("saved")).unwrap();
    symlink(r.directory.join("saved"), &state).unwrap();
    r.start();
    assert_eq!(r.error().code, "inventory-unavailable");
    r.stop();
    assert_eq!(fs::read(r.directory.join("saved")).unwrap(), original);
}
#[test]
fn valid_inventory_larger_than_lifecycle_frame_is_not_truncated() {
    let mut r = Runtime::new();
    let mut g = graph("mic");
    let a = g.as_array_mut().unwrap();
    for id in 100..250 {
        let mut node = a[1].clone();
        node["id"] = json!(id);
        node["info"]["props"]["node.name"] = json!(format!("endpoint-{id}"));
        a.push(node);
    }
    r.graph(&g);
    r.start();
    let inv = r.inventory();
    assert_eq!(inv.sources.len(), 151);
    assert!(serde_json::to_vec(&inv).unwrap().len() > runtime::MAX_FRAME);
    r.stop();
}

#[test]
fn real_http_service_reads_actual_runtime_discovery_over_private_ipc() {
    let f = support::Fixture::new();
    let mut r = Runtime::named(f.instance());
    r.start();
    let settings = mixyard::automation::settings::Settings { graph: Some(serde_json::from_value(json!({"schemaVersion":1,"id":"inventory-proof","name":"Hold with actual inventory","revision":1,"root":"hold","nodes":[{"kind":"terminal","id":"hold","action":{"kind":"no-change"}}]})).unwrap()), catalog: mixyard::automation::standard_catalog(), ..Default::default() };
    mixyard::automation::settings::Store::sibling_of_profiles(f.root.join("store"))
        .unwrap()
        .write(0, &settings)
        .unwrap();
    let service = f.start();
    let live = service.ok("/api/automation/live", json!({}));
    assert_eq!(live["blocked"], Value::Null, "{live}");
    assert_eq!(live["inventory"]["evidence"], "simulated");
    assert_eq!(live["inventory"]["sources"].as_array().unwrap().len(), 1);
    assert_eq!(live["audioApplied"], false);
    assert_eq!(live["backendAvailable"], false);
    let revision = live["inventory"]["revision"].as_u64().unwrap();
    r.stop();
    r.start();
    let live = service.ok("/api/automation/live", json!({}));
    assert_eq!(live["blocked"], Value::Null, "{live}");
    assert_eq!(live["inventory"]["revision"], revision);
    drop(service);
    r.stop();
}
