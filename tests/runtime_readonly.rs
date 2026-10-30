//! Real child runtime plus hostile peers over the same private OS transport.
use mixyard::runtime::{
    self, Lifecycle, ReadOnlyClient, RuntimeEvent, ServerError, IPC_VERSION, MAX_FRAME,
};
use serde_json::{json, Value};
use std::{
    io::{self, Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
#[allow(dead_code)]
#[path = "../src/runtime/unix.rs"]
mod transport;
#[cfg(windows)]
#[allow(dead_code)]
#[path = "../src/runtime/windows.rs"]
mod transport;
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
const TICK: Duration = Duration::from_millis(10);
const LIMIT: Duration = Duration::from_secs(8);
static NEXT: AtomicU32 = AtomicU32::new(0);

fn instance() -> String {
    format!(
        "readonly-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn cleanup(instance: &str) {
    #[cfg(unix)]
    {
        let path = runtime::endpoint(instance).unwrap();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(std::path::Path::new(&path).parent().unwrap());
    }
    #[cfg(windows)]
    let _ = instance;
}

fn wait(child: &mut Child) {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        assert!(start.elapsed() < LIMIT, "child runtime timed out");
        thread::sleep(TICK);
    }
}

struct Daemon {
    child: Child,
    instance: String,
}

impl Daemon {
    fn start(instance: String) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mixyard-runtime"));
        command
            .args(["serve", "--instance", &instance])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        // Keep lifecycle tests independent of the host's live PipeWire session.
        #[cfg(target_os = "linux")]
        command.args([
            "--inventory-fixture-helper",
            "/nonexistent/mixyard-test-helper",
        ]);
        for name in [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "DBUS_SESSION_BUS_ADDRESS",
            "DBUS_SYSTEM_BUS_ADDRESS",
        ] {
            command.env_remove(name);
        }
        let mut daemon = Self {
            child: command.spawn().unwrap(),
            instance,
        };
        let started = Instant::now();
        loop {
            assert!(daemon.child.try_wait().unwrap().is_none());
            if let Ok(client) = ReadOnlyClient::connect(&daemon.instance) {
                drop(client);
                return daemon;
            }
            assert!(started.elapsed() < LIMIT, "runtime not ready");
            thread::sleep(TICK);
        }
    }

    fn command(&self, command: &str) -> Value {
        let mut bytes = Vec::new();
        runtime::client(&self.instance, command, &mut bytes).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            self.child.kill().unwrap();
            self.child.wait().unwrap();
        }
        cleanup(&self.instance);
    }
}

fn snapshot() -> Value {
    json!({"event":"snapshot", "sequence":1, "resync":true, "lifecycle":"running",
    "historyAvailable":false, "availability":{
        "daemonReachable":true,"backendAvailable":false,"backend":"unavailable",
        "audioRevision":null,"appliedRevision":null,"desiredRevision":null,
        "nativeAudioProof":"unverified",
        "capabilities":{"apply":false,"mute":false,"nativeActionDispatch":false}
    }})
}

fn stopping() -> Value {
    json!({"event":"lifecycle", "sequence":2, "lifecycle":"stopping", "backendAvailable":false})
}

fn envelope(id: &str, result: Value) -> Value {
    json!({"ipcVersion":IPC_VERSION,"requestId":id,"result":result})
}

fn frame(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).unwrap();
    bytes.push(b'\n');
    bytes
}

fn write(stream: &mut transport::Stream, bytes: &[u8]) -> io::Result<()> {
    let started = Instant::now();
    let mut sent = 0;
    while sent < bytes.len() {
        match stream.write(&bytes[sent..]) {
            Ok(n) if n > 0 => sent += n,
            Ok(_) => return Err(io::ErrorKind::WriteZero.into()),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(TICK),
            Err(e) => return Err(e),
        }
        assert!(started.elapsed() < LIMIT);
    }
    Ok(())
}

struct Fake {
    instance: String,
    done: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Fake {
    fn start(reply: impl FnOnce(&str) -> Vec<u8> + Send + 'static, hold_open: bool) -> Self {
        Self::command("events", reply, hold_open)
    }
    fn command(
        command: &'static str,
        reply: impl FnOnce(&str) -> Vec<u8> + Send + 'static,
        hold_open: bool,
    ) -> Self {
        Self::command_with_disconnect(command, reply, hold_open, false)
    }
    fn command_with_disconnect(
        command: &'static str,
        reply: impl FnOnce(&str) -> Vec<u8> + Send + 'static,
        hold_open: bool,
        allow_disconnect: bool,
    ) -> Self {
        let instance = instance();
        let endpoint = runtime::endpoint(&instance).unwrap();
        let (done, finish) = mpsc::channel();
        let (ready, bound) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut listener = transport::Listener::bind(&endpoint).unwrap();
            ready.send(()).unwrap();
            let started = Instant::now();
            let mut stream = loop {
                if let Some(stream) = listener.accept().unwrap() {
                    break stream;
                }
                assert!(started.elapsed() < LIMIT);
                thread::sleep(TICK);
            };
            let mut input = Vec::new();
            let request: Value = loop {
                let mut buf = [0; 1024];
                match stream.read(&mut buf) {
                    Ok(n) if n > 0 => input.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(TICK),
                    result => panic!("fake peer read: {result:?}"),
                }
                if input.last() == Some(&b'\n') {
                    break serde_json::from_slice(&input).unwrap();
                }
                assert!(input.len() <= MAX_FRAME && started.elapsed() < LIMIT);
            };
            assert_eq!(request["command"], command);
            assert_eq!(request["ipcVersion"], IPC_VERSION);
            assert_eq!(request.as_object().unwrap().len(), 3);
            #[cfg(unix)]
            if command == "inventory" {
                use std::os::fd::AsRawFd;
                // Exercise short OS reads/backpressure, as seen on macOS,
                // without introducing artificial inter-chunk writer delays.
                let size: libc::c_int = 2048;
                assert_eq!(
                    unsafe {
                        libc::setsockopt(
                            stream.as_raw_fd(),
                            libc::SOL_SOCKET,
                            libc::SO_SNDBUF,
                            (&size as *const libc::c_int).cast(),
                            std::mem::size_of_val(&size) as libc::socklen_t,
                        )
                    },
                    0
                );
                stream.set_write_timeout(Some(LIMIT)).unwrap();
                stream.set_nonblocking(false).unwrap();
            }
            if let Err(error) = write(&mut stream, &reply(request["requestId"].as_str().unwrap())) {
                // Only explicitly hostile oversized fixtures expect the reader
                // to close before the entire response has been written.
                assert!(
                    allow_disconnect
                        && matches!(
                            error.kind(),
                            io::ErrorKind::BrokenPipe
                                | io::ErrorKind::ConnectionReset
                                | io::ErrorKind::ConnectionAborted
                                | io::ErrorKind::NotConnected
                        ),
                    "fake peer write: {error:?}"
                );
            }
            if hold_open {
                finish.recv_timeout(LIMIT).unwrap();
            }
        });
        bound.recv_timeout(LIMIT).unwrap();
        Self {
            instance,
            done,
            worker: Some(worker),
        }
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = self.done.send(());
        let result = self.worker.take().unwrap().join();
        cleanup(&self.instance);
        if !thread::panicking() {
            result.unwrap();
        }
    }
}

fn next(client: &mut ReadOnlyClient) -> io::Result<RuntimeEvent> {
    let started = Instant::now();
    loop {
        if let Some(event) = client.poll()? {
            return Ok(event);
        }
        assert!(started.elapsed() < LIMIT, "poll timed out");
        thread::sleep(TICK);
    }
}

#[test]
fn child_runtime_snapshot_drop_reconnect_stopping_and_same_pid() {
    let mut daemon = Daemon::start(instance());
    let pid = daemon.child.id();
    for _ in 0..3 {
        let mut client = ReadOnlyClient::connect(&daemon.instance).unwrap();
        let initial = client.initial_snapshot();
        let mut expected = snapshot();
        expected.as_object_mut().unwrap().remove("event");
        assert_eq!(serde_json::to_value(initial).unwrap(), expected);
        assert_eq!(initial.availability.audio_revision, None);
        assert_eq!(initial.availability.applied_revision, None);
        assert_eq!(initial.availability.desired_revision, None);
        let started = Instant::now();
        for _ in 0..1000 {
            assert_eq!(client.poll().unwrap(), None);
        }
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "idle poll slept"
        );
        drop(client);
        assert!(daemon.child.try_wait().unwrap().is_none());
        assert_eq!(daemon.child.id(), pid);
        assert_eq!(
            daemon.command("status")["result"]["backendAvailable"],
            false
        );
    }
    let mut client = ReadOnlyClient::connect(&daemon.instance).unwrap();
    assert_eq!(
        daemon.command("shutdown")["result"]["shutdown"],
        "requested"
    );
    let event = next(&mut client).unwrap();
    assert_eq!(serde_json::to_value(&event).unwrap(), stopping());
    assert!(matches!(event, RuntimeEvent::Lifecycle(e) if e.lifecycle == Lifecycle::Stopping));
    assert_eq!(
        client.poll().unwrap_err().kind(),
        io::ErrorKind::NotConnected
    );
    wait(&mut daemon.child);
    assert!(ReadOnlyClient::connect(&daemon.instance).is_err());
    let mut restarted = Daemon::start(daemon.instance.clone());
    drop(ReadOnlyClient::connect(&restarted.instance).unwrap());
    restarted.command("shutdown");
    wait(&mut restarted.child);
}

#[test]
fn bounded_server_rejections_preserve_code_and_message() {
    for code in ["busy", "request-timeout"] {
        let fake = Fake::start(
            move |id| {
                frame(&json!({"ipcVersion":1,"requestId":id,
                    "error":{"code":code,"message":"retry later"}}))
            },
            true,
        );
        let error = ReadOnlyClient::connect(&fake.instance).err().unwrap();
        let error = error
            .get_ref()
            .unwrap()
            .downcast_ref::<ServerError>()
            .unwrap();
        assert_eq!(error.code, code);
        assert_eq!(error.message, "retry later");
    }
}

#[test]
fn malformed_snapshot_envelopes_and_capabilities_fail_closed() {
    let valid = envelope("placeholder", snapshot());
    let mut cases = Vec::new();
    for (pointer, value) in [
        ("/ipcVersion", json!(2)),
        ("/requestId", json!("wrong")),
        ("/result", Value::Null),
        ("/result/event", json!("audio")),
        ("/result/sequence", json!(0)),
        ("/result/sequence", json!(2)),
        ("/result/lifecycle", json!("stopping")),
        ("/result/resync", json!(false)),
        ("/result/historyAvailable", json!(true)),
        ("/result/availability/daemonReachable", json!(false)),
        ("/result/availability/backendAvailable", json!(true)),
        ("/result/availability/backend", json!("native")),
        ("/result/availability/nativeAudioProof", json!("verified")),
        ("/result/availability/audioRevision", json!(1)),
        ("/result/availability/appliedRevision", json!(1)),
        ("/result/availability/desiredRevision", json!(1)),
        ("/result/availability/capabilities/apply", json!(true)),
        ("/result/availability/capabilities/mute", json!(true)),
        (
            "/result/availability/capabilities/nativeActionDispatch",
            json!(true),
        ),
    ] {
        let mut reply = valid.clone();
        *reply.pointer_mut(pointer).unwrap() = value;
        cases.push(reply);
    }
    for pointer in [
        "",
        "/result",
        "/result/availability",
        "/result/availability/capabilities",
    ] {
        let mut reply = valid.clone();
        reply
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unexpected".into(), json!(false));
        cases.push(reply);
    }
    for field in ["audioRevision", "appliedRevision", "desiredRevision"] {
        let mut reply = valid.clone();
        reply["result"]["availability"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        cases.push(reply);
    }
    for error in [json!({"code":"busy","message":"retry"}), Value::Null] {
        let mut reply = valid.clone();
        reply["error"] = error;
        cases.push(reply);
    }
    let mut neither = valid.clone();
    neither.as_object_mut().unwrap().remove("result");
    cases.push(neither);
    cases.push(envelope("placeholder", stopping()));
    for error in [
        json!({"code":"","message":"retry"}),
        json!({"code":"x".repeat(65),"message":"retry"}),
        json!({"code":"busy","message":"x".repeat(1025)}),
        json!({"code":"busy","message":"retry","unknown":false}),
    ] {
        cases.push(json!({"ipcVersion":1,"requestId":"placeholder","error":error}));
    }
    for mut reply in cases {
        let fake = Fake::start(
            move |id| {
                if reply["requestId"] == "placeholder" {
                    reply["requestId"] = json!(id);
                }
                frame(&reply)
            },
            true,
        );
        let error = ReadOnlyClient::connect(&fake.instance).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
        assert!(error
            .get_ref()
            .and_then(|e| e.downcast_ref::<ServerError>())
            .is_none());
    }
    for (version, wrong_id) in [(2, false), (1, true)] {
        let fake = Fake::start(
            move |id| {
                frame(&json!({
                    "ipcVersion":version,"requestId":if wrong_id {"wrong"} else {id},
                    "error":{"code":"busy","message":"retry"}
                }))
            },
            true,
        );
        assert_eq!(
            ReadOnlyClient::connect(&fake.instance)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}

#[test]
fn malformed_json_duplicates_and_frame_limits() {
    for bytes in [
        b"garbage\n".to_vec(),
        vec![b'x'; MAX_FRAME + 1],
        [vec![b'x'; MAX_FRAME + 1], vec![b'\n']].concat(),
    ] {
        let fake = Fake::start(move |_| bytes, true);
        assert_eq!(
            ReadOnlyClient::connect(&fake.instance)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
    let fake = Fake::start(
        |id| {
            format!(
                "{{\"ipcVersion\":1,\"ipcVersion\":1,\"requestId\":\"{id}\",\"result\":{}}}\n",
                snapshot()
            )
            .into_bytes()
        },
        true,
    );
    assert_eq!(
        ReadOnlyClient::connect(&fake.instance)
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::InvalidData
    );
    for size in [MAX_FRAME, MAX_FRAME + 1] {
        let fake = Fake::start(
            move |id| {
                let mut bytes = serde_json::to_vec(&envelope(id, snapshot())).unwrap();
                bytes.resize(size, b' ');
                bytes.push(b'\n');
                bytes
            },
            true,
        );
        let result = ReadOnlyClient::connect(&fake.instance);
        if size == MAX_FRAME {
            assert!(result.is_ok());
        } else {
            assert_eq!(result.err().unwrap().kind(), io::ErrorKind::InvalidData);
        }
    }
}

#[test]
fn poll_rejects_repeated_snapshot_wrong_lifecycle_and_bad_envelopes() {
    let mut cases = vec![envelope("placeholder", snapshot())];
    for (key, value) in [
        ("sequence", json!(1)),
        ("sequence", json!(3)),
        ("lifecycle", json!("running")),
        ("lifecycle", json!("stopped")),
        ("backendAvailable", json!(true)),
        ("extra", json!(false)),
    ] {
        let mut result = stopping();
        result[key] = value;
        cases.push(envelope("placeholder", result));
    }
    let mut wrong_version = envelope("placeholder", stopping());
    wrong_version["ipcVersion"] = json!(2);
    cases.push(wrong_version);
    cases.push(envelope("wrong", stopping()));
    for mut second in cases {
        let fake = Fake::start(
            move |id| {
                if second["requestId"] == "placeholder" {
                    second["requestId"] = json!(id);
                }
                [frame(&envelope(id, snapshot())), frame(&second)].concat()
            },
            true,
        );
        let mut client = ReadOnlyClient::connect(&fake.instance).unwrap();
        assert_eq!(
            next(&mut client).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            client.poll().unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );
    }
    let fake = Fake::start(
        |id| {
            [
                frame(&envelope(id, snapshot())),
                frame(&envelope(id, stopping())),
                b"extra".to_vec(),
            ]
            .concat()
        },
        true,
    );
    let mut client = ReadOnlyClient::connect(&fake.instance).unwrap();
    assert_eq!(
        next(&mut client).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn abrupt_eof_and_partial_eof_are_disconnects() {
    for partial in [false, true] {
        let fake = Fake::start(
            move |id| {
                let mut bytes = frame(&envelope(id, snapshot()));
                if partial {
                    bytes.extend_from_slice(b"{\"ipcVersion\":");
                }
                bytes
            },
            true,
        );
        let mut client = ReadOnlyClient::connect(&fake.instance).unwrap();
        drop(fake);
        let error = next(&mut client).unwrap_err();
        assert!(
            error.kind() == io::ErrorKind::UnexpectedEof
                || (cfg!(windows) && error.raw_os_error() == Some(233)),
            "unexpected disconnect error: {error:?}"
        );
        assert_eq!(
            client.poll().unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );
    }
}

#[test]
fn initial_snapshot_and_partial_frame_have_finite_deadlines() {
    let fake = Fake::start(|_| b"{".to_vec(), true);
    let started = Instant::now();
    assert_eq!(
        ReadOnlyClient::connect(&fake.instance)
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::TimedOut
    );
    assert!(started.elapsed() < CLIENT_TIMEOUT + Duration::from_secs(1));
    drop(fake);
    let fake = Fake::start(
        |id| [frame(&envelope(id, snapshot())), b"{".to_vec()].concat(),
        true,
    );
    let mut client = ReadOnlyClient::connect(&fake.instance).unwrap();
    let started = Instant::now();
    assert_eq!(
        next(&mut client).unwrap_err().kind(),
        io::ErrorKind::TimedOut
    );
    assert!(started.elapsed() < CLIENT_TIMEOUT + Duration::from_secs(1));
    assert_eq!(
        client.poll().unwrap_err().kind(),
        io::ErrorKind::NotConnected
    );
}

#[cfg(unix)]
#[test]
fn readonly_connect_uses_existing_transport_permissions() {
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};
    let mut daemon = Daemon::start(instance());
    let path = runtime::endpoint(&daemon.instance).unwrap();
    let directory = Path::new(&path).parent().unwrap();
    for (target, insecure, secure) in [
        (&*path, 0o666, 0o600),
        (directory.to_str().unwrap(), 0o755, 0o700),
    ] {
        fs::set_permissions(target, fs::Permissions::from_mode(insecure)).unwrap();
        assert_eq!(
            ReadOnlyClient::connect(&daemon.instance)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        fs::set_permissions(target, fs::Permissions::from_mode(secure)).unwrap();
    }
    drop(ReadOnlyClient::connect(&daemon.instance).unwrap());
    daemon.command("shutdown");
    wait(&mut daemon.child);
}

#[test]
fn inventory_query_is_actual_typed_ipc_and_lifecycle_daemon_reports_unavailable() {
    use std::sync::atomic::AtomicBool;
    let daemon = Daemon::start(instance());
    let error = ReadOnlyClient::inventory(&daemon.instance, &AtomicBool::new(false)).unwrap_err();
    assert_eq!(
        error
            .get_ref()
            .unwrap()
            .downcast_ref::<ServerError>()
            .unwrap()
            .code,
        "inventory-unavailable"
    );
    assert_eq!(
        ReadOnlyClient::connect(&daemon.instance)
            .unwrap()
            .initial_snapshot()
            .lifecycle,
        Lifecycle::Running
    );
    let fake = Fake::command(
        "inventory",
        |id| {
            frame(&envelope(
                id,
                json!({"schemaVersion":2,"revision":77,"platform":"linux","evidence":"simulated","sources":[],"outputs":[]}),
            ))
        },
        true,
    );
    let inventory = ReadOnlyClient::inventory(&fake.instance, &AtomicBool::new(false)).unwrap();
    assert_eq!(inventory.revision, 77);
    assert_eq!(inventory.evidence, mixyard::Evidence::Simulated);
    assert!(ReadOnlyClient::inventory(&fake.instance, &AtomicBool::new(true)).is_err());
}
#[test]
fn valid_large_inventory_replies_arrive_over_actual_os_transport() {
    use std::sync::atomic::AtomicBool;
    for count in [130, 420] {
        let fake = Fake::command(
            "inventory",
            move |id| {
                let sources: Vec<Value> = (0..count)
                    .map(|i| {
                        json!({
                            "observedId":format!("source:{i}:{}", "x".repeat(2048)),
                            "selector":{"kind":"mic","deviceUid":format!("fixture-device-{i}")},
                            "origin":"external","format":null
                        })
                    })
                    .collect();
                let reply = frame(&envelope(
                    id,
                    json!({"schemaVersion":2,"revision":77,
                "platform":"linux","evidence":"simulated","sources":sources,"outputs":[]}),
                ));
                assert!(reply.len() > 256 * 1024 && reply.len() <= runtime::MAX_INVENTORY_FRAME);
                reply
            },
            true,
        );
        let inventory = ReadOnlyClient::inventory(&fake.instance, &AtomicBool::new(false)).unwrap();
        assert_eq!(inventory.revision, 77);
        assert_eq!(inventory.sources.len(), count);
        assert_eq!(
            inventory.sources[count - 1].observed_id,
            format!("source:{}:{}", count - 1, "x".repeat(2048))
        );
    }
}

#[test]
fn transport_completes_pending_reads_into_smaller_caller_slices() {
    let (waiting, ready) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let expected = vec![b'q'; 32 * 1024];
    let bytes = expected.clone();
    let fake = Fake::command(
        "inventory",
        move |_| {
            waiting.send(()).unwrap();
            held.recv_timeout(LIMIT).unwrap();
            bytes
        },
        true,
    );
    let mut stream = transport::connect(&runtime::endpoint(&fake.instance).unwrap()).unwrap();
    write(
        &mut stream,
        &frame(&json!({"ipcVersion":IPC_VERSION,
        "requestId":"pending-slice-check","command":"inventory"})),
    )
    .unwrap();
    ready.recv_timeout(LIMIT).unwrap();
    let mut initial = [0; 64 * 1024];
    assert_eq!(
        stream.read(&mut initial).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    release.send(()).unwrap();
    let start = Instant::now();
    let mut received = Vec::new();
    while received.len() < expected.len() {
        let mut small = [0; 17];
        match stream.read(&mut small) {
            Ok(n) if n > 0 => received.extend_from_slice(&small[..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(TICK),
            result => panic!("small read after pending large read: {result:?}"),
        }
        assert!(start.elapsed() < LIMIT);
    }
    assert_eq!(received, expected);
}

#[test]
fn inventory_rejects_bad_envelopes_oversize_and_is_cancellable_with_total_deadline() {
    use std::sync::{atomic::AtomicBool, Arc};
    for kind in [0, 1, 2, 3, 4, 6] {
        let fake = Fake::command(
            "inventory",
            move |id| {
                let mut reply = envelope(
                    id,
                    json!({"schemaVersion":2,"revision":1,"platform":"linux","evidence":"simulated","sources":[],"outputs":[]}),
                );
                match kind {
                0=>reply["requestId"]=json!("wrong"),
                1=>reply["ipcVersion"]=json!(2),
                2=>reply["error"]=json!({"code":"both","message":"ambiguous"}),
                3=>reply["result"]=Value::Null,
                4=>reply["result"]["schemaVersion"]=json!(1),
                6=>return format!("{{\"ipcVersion\":1,\"ipcVersion\":1,\"requestId\":\"{id}\",\"result\":null}}\n").into_bytes(),
                _=>unreachable!(),
            }
                frame(&reply)
            },
            true,
        );
        assert_eq!(
            ReadOnlyClient::inventory(&fake.instance, &AtomicBool::new(false))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData,
            "bad envelope {kind}"
        );
    }
    let oversized = Fake::command_with_disconnect(
        "inventory",
        |id| {
            frame(&envelope(
                id,
                json!({"schemaVersion":2,"revision":1,"platform":"linux",
            "evidence":"simulated","sources":[],"outputs":[],
            "extra":"x".repeat(runtime::MAX_INVENTORY_FRAME)}),
            ))
        },
        true,
        true,
    );
    let error =
        ReadOnlyClient::inventory(&oversized.instance, &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("1 MiB"));
    let fake = Fake::command("inventory", |_| b"{\"ipcVersion\":".to_vec(), true);
    let start = Instant::now();
    assert_eq!(
        ReadOnlyClient::inventory(&fake.instance, &AtomicBool::new(false))
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    drop(fake);
    let fake = Fake::command("inventory", |_| Vec::new(), true);
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    let worker = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        signal.store(true, Ordering::Release);
    });
    let start = Instant::now();
    assert_eq!(
        ReadOnlyClient::inventory(&fake.instance, &cancelled)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    worker.join().unwrap();
}

// Force a real empty-queue handoff before each small write. A fixed 10 ms
// inventory sleep would consume the two-second budget before reaching 1 MiB;
// the same bounded readiness wait must wake when each producer write arrives.
#[cfg(unix)]
#[test]
fn inventory_readiness_handoffs_do_not_accumulate_idle_waits() {
    use std::os::unix::net::UnixStream;
    let (mut reader, mut writer) = UnixStream::pair().unwrap();
    reader.set_nonblocking(true).unwrap();
    writer
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    thread::scope(|scope| {
        let (request, requests) = mpsc::channel();
        let producer = scope.spawn(move || {
            let mut sent = 0;
            while let Ok((index, count)) = requests.recv() {
                writer.write_all(&vec![index as u8; count]).unwrap();
                sent += count;
            }
            sent
        });
        let start = Instant::now();
        let mut total = 0;
        let mut index = 0;
        while total <= runtime::MAX_INVENTORY_FRAME {
            let mut probe = [0];
            assert_eq!(
                reader.read(&mut probe).unwrap_err().kind(),
                io::ErrorKind::WouldBlock,
                "producer must wait for an actual empty socket"
            );
            let count = (runtime::MAX_INVENTORY_FRAME + 1 - total).min(2048);
            request.send((index, count)).unwrap();
            let mut received = vec![0; count];
            let mut offset = 0;
            while offset < count {
                assert!(start.elapsed() < Duration::from_secs(2));
                transport::wait_inventory(&reader, false, TICK).unwrap();
                match reader.read(&mut received[offset..]) {
                    Ok(0) => panic!("producer closed before its complete chunk"),
                    Ok(n) => offset += n,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => panic!("inventory handoff read: {error}"),
                }
            }
            assert!(received.iter().all(|byte| *byte == index as u8));
            total += count;
            index += 1;
        }
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(total, runtime::MAX_INVENTORY_FRAME + 1);
        drop(request);
        assert_eq!(producer.join().unwrap(), total);
    });
}
