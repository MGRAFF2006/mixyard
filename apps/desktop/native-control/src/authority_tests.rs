//! Deterministic native worker races through production Paired and REAL TLS.
//! Only the credential vault boundary is memory-backed; no alternate state model.
use super::*;
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{atomic::AtomicU64, mpsc, Condvar},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Checkpoint {
    OpenAdmitted,
    DisconnectAdmitted,
    OpenPublication,
}
pub(super) type Hook = Arc<dyn Fn(Checkpoint, u64) + Send + Sync>;

#[derive(Default)]
struct MemoryVault(Mutex<BTreeMap<String, String>>);
impl Vault for MemoryVault {
    fn read(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn write(&self, key: &str, value: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}
struct Gate {
    state: Mutex<(bool, bool)>,
    event: Condvar,
}
impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new((false, false)),
            event: Condvar::new(),
        })
    }
    fn pause(&self) {
        let mut state = self.state.lock().unwrap();
        state.0 = true;
        self.event.notify_all();
        while !state.1 {
            state = self.event.wait(state).unwrap();
        }
    }
    fn entered(&self) {
        let state = self.state.lock().unwrap();
        let (state, _) = self
            .event
            .wait_timeout_while(state, Duration::from_secs(10), |state| !state.0)
            .unwrap();
        let entered = state.0;
        drop(state);
        assert!(entered, "Owned native worker did not reach its barrier");
    }
    fn release(&self) {
        self.state.lock().unwrap().1 = true;
        self.event.notify_all();
    }
}
struct Release(Arc<Gate>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct Fixture {
    root: PathBuf,
    process: Option<Child>,
    reader: Option<thread::JoinHandle<()>>,
    client: Arc<Paired<MemoryVault>>,
    target: Option<Target>,
    authority: String,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir()
            .canonicalize()
            .expect("Trusted existing OS temp base");
        let root = base.join(format!(
            "mixyard-native-race-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder
            .create(&root)
            .expect("Exclusive owned fixture directory");
        let mut fixture = Self {
            root,
            process: None,
            reader: None,
            client: Arc::new(Paired::new(MemoryVault::default())),
            target: None,
            authority: String::new(),
        };
        let binary = std::env::var_os("MIXYARD_TEST_CONTROL_BINARY")
            .expect("Set owned production control CLI for native TLS races");
        fixture.cli(&binary, &["init", "--ip", "127.0.0.1"]);
        let invite: Invitation = serde_json::from_value(fixture.cli(
            &binary,
            &["invite", "--permissions", "view,control,settings"],
        ))
        .expect("Owned invitation shape");
        let assets = fixture.root.join("assets");
        std::fs::create_dir(&assets).unwrap();
        std::fs::write(
            assets.join("index.html"),
            "<!doctype html><title>Owned native race</title>",
        )
        .unwrap();
        let mut process = Command::new(&binary)
            .arg("--state")
            .arg(fixture.root.join("identity"))
            .args([
                "serve",
                "--enable-control",
                "--bind",
                "127.0.0.1:0",
                "--store",
            ])
            .arg(fixture.root.join("profiles"))
            .arg("--assets")
            .arg(assets)
            .args(["--instance", "owned-native-race"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("Owned service starts");
        let stdout = process.stdout.take().unwrap();
        fixture.process = Some(process);
        let (send, receive) = mpsc::sync_channel(1);
        fixture.reader = Some(thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = send.send(line);
        }));
        let line = receive
            .recv_timeout(Duration::from_secs(20))
            .expect("Owned TLS readiness deadline");
        let ready: serde_json::Value = serde_json::from_str(&line).expect("Owned readiness JSON");
        fixture.authority = ready["authorityId"].as_str().unwrap().into();
        let endpoint = format!("https://{}", ready["address"].as_str().unwrap());
        fixture.target = Some(
            fixture
                .client
                .pair(endpoint, "Owned race controller".into(), invite)
                .expect("Actual production pairing"),
        );
        fixture
    }
    fn cli(&self, binary: &std::ffi::OsStr, args: &[&str]) -> serde_json::Value {
        let mut child = Command::new(binary)
            .arg("--state")
            .arg(self.root.join("identity"))
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("Owned CLI starts");
        let deadline = Instant::now() + Duration::from_secs(20);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Owned CLI deadline");
            }
            thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "Owned CLI succeeded");
        serde_json::from_slice(&output.stdout).expect("Owned CLI JSON")
    }
    fn target(&self) -> Target {
        self.target.as_ref().unwrap().clone()
    }
    fn raw(&self) -> String {
        serde_json::json!({"deviceId":self.target().device_id,"authorityId":self.authority,"revision":0,"path":"/api/status","body":{}}).to_string()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(process) = self.process.as_mut() {
            let _ = process.kill();
            let _ = process.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn open(
    client: Arc<Paired<MemoryVault>>,
    target: Target,
) -> thread::JoinHandle<Result<OpenReply, String>> {
    thread::spawn(move || client.open(&target.device_id, &target.credential_id))
}
fn hook(fixture: &Fixture, callback: impl Fn(Checkpoint, u64) + Send + Sync + 'static) {
    *fixture
        .client
        .host(&fixture.target().device_id)
        .unwrap()
        .hook
        .lock()
        .unwrap() = Some(Arc::new(callback));
}

#[test]
fn native_old_open_admission_cannot_clear_replacement_session() {
    let fixture = Fixture::new();
    let gate = Gate::new();
    let _release = Release(gate.clone());
    let blocked = gate.clone();
    hook(&fixture, move |stage, generation| {
        if stage == Checkpoint::OpenAdmitted && generation == 1 {
            blocked.pause();
        }
    });
    let old = open(fixture.client.clone(), fixture.target());
    gate.entered();
    let current = fixture
        .client
        .open(&fixture.target().device_id, &fixture.target().credential_id)
        .unwrap();
    assert_eq!(current.generation, 2);
    gate.release();
    assert!(old.join().unwrap().is_err());
    let reply = fixture.client.request(
        &fixture.target().device_id,
        current.generation,
        &fixture.raw(),
    );
    if matches!(&reply, Err(error) if error == "Read a fresh paired session before requesting") {
        println!("CAUSAL_RED old-open-erased-new-session");
    }
    assert!(
        reply.is_ok(),
        "Replacement native session must still make an ACTUAL TLS request"
    );
    assert_eq!(reply.unwrap().status, 200);
}

#[test]
fn native_delayed_expected_disconnect_cannot_clear_replacement_session() {
    let fixture = Fixture::new();
    let first = fixture
        .client
        .open(&fixture.target().device_id, &fixture.target().credential_id)
        .unwrap();
    let gate = Gate::new();
    let _release = Release(gate.clone());
    let blocked = gate.clone();
    hook(&fixture, move |stage, generation| {
        if stage == Checkpoint::DisconnectAdmitted && generation == 2 {
            blocked.pause();
        }
    });
    let client = fixture.client.clone();
    let id = fixture.target().device_id;
    let old = thread::spawn(move || client.disconnect(&id, Some(first.generation)));
    gate.entered();
    let current = fixture
        .client
        .open(&fixture.target().device_id, &fixture.target().credential_id)
        .unwrap();
    gate.release();
    old.join().unwrap().unwrap();
    let reply = fixture.client.request(
        &fixture.target().device_id,
        current.generation,
        &fixture.raw(),
    );
    if matches!(&reply, Err(error) if error == "Read a fresh paired session before requesting") {
        println!("CAUSAL_RED old-disconnect-erased-new-session");
    }
    assert!(
        reply.is_ok(),
        "Delayed disconnect cannot erase the newer native session"
    );
    assert_eq!(reply.unwrap().status, 200);
}

#[test]
fn native_stale_tls_completion_cannot_publish_for_a_new_admission() {
    let fixture = Fixture::new();
    let publication = Gate::new();
    let admission = Gate::new();
    let _release_a = Release(publication.clone());
    let _release_b = Release(admission.clone());
    let gate_a = publication.clone();
    let gate_b = admission.clone();
    hook(&fixture, move |stage, generation| {
        if stage == Checkpoint::OpenPublication && generation == 1 {
            gate_a.pause();
        }
        if stage == Checkpoint::OpenAdmitted && generation == 2 {
            gate_b.pause();
        }
    });
    let old = open(fixture.client.clone(), fixture.target());
    publication.entered();
    let replacement = open(fixture.client.clone(), fixture.target());
    admission.entered();
    publication.release();
    assert!(old.join().unwrap().is_err());
    // B has been admitted but has not performed its own TLS exchange. No A
    // result may confer its session to B merely because the generation is2.
    let premature = fixture
        .client
        .request(&fixture.target().device_id, 2, &fixture.raw());
    admission.release();
    let current = replacement.join().unwrap().unwrap();
    let actual = fixture.client.request(
        &fixture.target().device_id,
        current.generation,
        &fixture.raw(),
    );
    if premature.as_ref().is_ok_and(|reply| reply.status == 200) {
        println!("CAUSAL_RED stale-tls-granted-new-generation-before-its-open");
    }
    assert!(
        premature.is_err(),
        "Stale TLS result must not publish under another generation"
    );
    assert!(
        actual.is_ok(),
        "Replacement must establish its OWN actual TLS session"
    );
    assert_eq!(actual.unwrap().status, 200);
}
