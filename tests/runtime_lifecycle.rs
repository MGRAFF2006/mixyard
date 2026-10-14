//! Actual child processes and OS endpoints, not a simulated transport.
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU32, Ordering},
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
static NEXT: AtomicU32 = AtomicU32::new(0);
const LIMIT: Duration = Duration::from_secs(8);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(windows)]
const TICK: Duration = Duration::from_millis(10);
fn instance() -> String {
    format!(
        "test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
fn cli(instance: &str, command: &str) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_mixyard"));
    c.args([command, "--instance", instance]);
    headless(&mut c);
    c
}
fn headless(c: &mut Command) {
    for name in [
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "DBUS_SESSION_BUS_ADDRESS",
        "DBUS_SYSTEM_BUS_ADDRESS",
    ] {
        c.env_remove(name);
    }
}
fn wait(child: &mut Child) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if start.elapsed() > LIMIT {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("child process timed out");
        }
        thread::sleep(Duration::from_millis(10));
    }
}
struct Daemon {
    child: Child,
    instance: String,
}
impl Daemon {
    fn start(instance: String) -> Self {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mixyard-runtime"));
        c.args(["serve", "--instance", &instance])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        headless(&mut c);
        let mut d = Self {
            child: c.spawn().unwrap(),
            instance,
        };
        let start = Instant::now();
        loop {
            assert!(
                d.child.try_wait().unwrap().is_none(),
                "daemon exited before readiness"
            );
            if transport::connect(&mixyard::runtime::endpoint(&d.instance).unwrap()).is_ok() {
                return d;
            }
            assert!(start.elapsed() < LIMIT, "daemon did not become ready");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn run(&self, command: &str) -> Value {
        let mut c = cli(&self.instance, command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        assert!(wait(&mut c).success());
        let output = c.wait_with_output().unwrap();
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn stop(&mut self) {
        assert_eq!(self.run("shutdown")["result"]["shutdown"], "requested");
        assert!(wait(&mut self.child).success());
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            self.child.kill().unwrap();
            self.child.wait().unwrap();
        }
        #[cfg(unix)]
        {
            // Test-owned namespace only, cleanup even if an assertion killed the child.
            let path = mixyard::runtime::endpoint(&self.instance).unwrap();
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_dir(std::path::Path::new(&path).parent().unwrap());
        }
    }
}
fn write(stream: &mut transport::Stream, bytes: &[u8]) {
    let mut sent = 0;
    let start = Instant::now();
    while sent < bytes.len() {
        match stream.write(&bytes[sent..]) {
            Ok(n) if n > 0 => sent += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10))
            }
            other => panic!("write failed: {other:?}"),
        }
        assert!(start.elapsed() < LIMIT, "write timeout");
    }
}
fn read(stream: &mut transport::Stream) -> Value {
    let start = Instant::now();
    let mut input = Vec::new();
    loop {
        let mut buf = [0; 1024];
        match stream.read(&mut buf) {
            Ok(n) if n > 0 => {
                input.extend_from_slice(&buf[..n]);
                if let Some(end) = input.iter().position(|b| *b == b'\n') {
                    return serde_json::from_slice(&input[..end]).unwrap();
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10))
            }
            other => panic!("read failed: {other:?}"),
        }
        assert!(input.len() <= 8192);
        assert!(start.elapsed() < LIMIT, "read timeout");
    }
}
fn connect(d: &Daemon) -> transport::Stream {
    transport::connect(&mixyard::runtime::endpoint(&d.instance).unwrap()).unwrap()
}
fn raw(d: &Daemon, request: &[u8]) -> Value {
    let mut s = connect(d);
    write(&mut s, request);
    read(&mut s)
}
fn request(command: &str) -> Vec<u8> {
    format!(
        "{}\n",
        json!({"ipcVersion":1,"requestId":"raw","command":command})
    )
    .into_bytes()
}

#[test]
fn real_headless_lifetime_events_and_concurrent_clients() {
    let mut d = Daemon::start(instance());
    let mut events = cli(&d.instance, "events")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // Bound this subprocess even if an assertion fails.
    struct EventGuard(Child);
    impl Drop for EventGuard {
        fn drop(&mut self) {
            if self.0.try_wait().unwrap().is_none() {
                self.0.kill().unwrap();
                self.0.wait().unwrap();
            }
        }
    }
    let stdout = events.stdout.take().unwrap();
    let mut guard = EventGuard(events);
    let mut one = cli(&d.instance, "status")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut two = cli(&d.instance, "status")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    assert!(wait(&mut one).success());
    assert!(wait(&mut two).success());
    for c in [one, two] {
        let value: Value = serde_json::from_slice(&c.wait_with_output().unwrap().stdout).unwrap();
        assert_eq!(value["ipcVersion"], 1);
        assert_eq!(value["result"]["daemonReachable"], true);
        assert_eq!(value["result"]["backendAvailable"], false);
        for key in ["audioRevision", "appliedRevision", "desiredRevision"] {
            assert!(value["result"][key].is_null());
        }
        assert_eq!(value["result"]["capabilities"]["apply"], false);
    }
    // Closing a subscribed CLI has no effect on daemon lifetime.
    guard.0.kill().unwrap();
    guard.0.wait().unwrap();
    drop(stdout);
    assert!(d.child.try_wait().unwrap().is_none());
    d.run("status");
    let mut stream = connect(&d);
    write(&mut stream, &request("events"));
    let snapshot = read(&mut stream);
    assert_eq!(snapshot["result"]["event"], "snapshot");
    assert_eq!(snapshot["result"]["resync"], true);
    assert_eq!(
        snapshot["result"]["availability"]["backendAvailable"],
        false
    );
    // A real CLI stream terminates with the daemon (no background threads).
    let mut events = cli(&d.instance, "events")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // Give its request time to be accepted before shutdown.
    thread::sleep(Duration::from_millis(150));
    let stopped = d.run("shutdown");
    assert_eq!(stopped["result"]["shutdown"], "requested");
    assert_eq!(read(&mut stream)["result"]["lifecycle"], "stopping");
    drop(stream);
    assert!(wait(&mut d.child).success());
    assert!(wait(&mut events).success());
    let lines: Vec<Value> = BufReader::new(events.stdout.take().unwrap())
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["result"]["sequence"], 2);
    let mut absent = cli(&d.instance, "status")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert_eq!(wait(&mut absent).code(), Some(4));
    assert!(absent.wait_with_output().unwrap().stdout.is_empty());
    // Same namespace can restart after explicit shutdown.
    let mut restart = Daemon::start(d.instance.clone());
    restart.stop();
}

#[test]
fn hostile_clients_are_bounded_and_do_not_wedge_health_or_shutdown() {
    let mut d = Daemon::start(instance());
    let mut stalled = connect(&d);
    write(&mut stalled, b"{");
    assert_eq!(raw(&d, b"garbage\n")["error"]["code"], "malformed-request");
    assert_eq!(
        raw(
            &d,
            b"{\"ipcVersion\":2,\"requestId\":\"wrong\",\"command\":\"status\"}\n"
        )["error"]["code"],
        "unsupported-version"
    );
    assert_eq!(
        raw(
            &d,
            b"{\"ipcVersion\":1,\"requestId\":\"id\",\"command\":\"status\",\"extra\":true}\n"
        )["error"]["code"],
        "malformed-request"
    );
    assert_eq!(
        raw(&d, &vec![b'x'; 8193])["error"]["code"],
        "frame-too-large"
    );
    for command in ["apply", "mute", "native-action"] {
        assert_eq!(
            raw(&d, &request(command))["error"]["code"],
            "backend-unavailable"
        );
    }
    for _ in 0..20 {
        drop(connect(&d));
        d.run("status");
    }
    assert_eq!(read(&mut stalled)["error"]["code"], "request-timeout");
    // All event slots have bounded snapshots. Excess subscribers get a JSON error,
    // leaving ordinary status/shutdown capacity available.
    let mut subscribers = Vec::new();
    for _ in 0..8 {
        let mut s = connect(&d);
        write(&mut s, &request("events"));
        assert_eq!(read(&mut s)["result"]["event"], "snapshot");
        subscribers.push(s);
    }
    assert_eq!(raw(&d, &request("events"))["error"]["code"], "busy");
    // Reconnect always resyncs, including an obsolete/future cursor.
    drop(subscribers.pop());
    thread::sleep(Duration::from_millis(50));
    assert_eq!(raw(&d,b"{\"ipcVersion\":1,\"requestId\":\"cursor\",\"command\":\"events\",\"afterSequence\":999}\n")["result"]["resync"],true);
    let mut unread = connect(&d);
    write(&mut unread, &request("events"));
    let mut stalled = connect(&d);
    write(&mut stalled, b"{");
    d.run("status");
    d.stop();
    drop(stalled);
    drop(unread);
    drop(subscribers);
}

#[test]
fn second_daemon_does_not_replace_or_shutdown_first() {
    let mut d = Daemon::start(instance());
    let mut second = Command::new(env!("CARGO_BIN_EXE_mixyard-runtime"))
        .args(["serve", "--instance", &d.instance])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert_eq!(wait(&mut second).code(), Some(4));
    d.run("status");
    d.stop();
}

#[cfg(unix)]
#[test]
fn actual_unix_permissions_symlinks_stale_and_replaced_endpoint() {
    use std::{
        fs,
        os::unix::{
            fs::{symlink, MetadataExt, PermissionsExt},
            net::UnixListener,
        },
        path::Path,
    };
    let mut d = Daemon::start(instance());
    let path = mixyard::runtime::endpoint(&d.instance).unwrap();
    let parent = Path::new(&path).parent().unwrap();
    assert_eq!(fs::symlink_metadata(parent).unwrap().mode() & 0o777, 0o700);
    assert_eq!(fs::symlink_metadata(&path).unwrap().mode() & 0o777, 0o600);
    // Replace path while the original listener lives: its cleanup must not remove
    // the replacement, even when it belongs to this same user.
    let mut stop = connect(&d);
    fs::remove_file(&path).unwrap();
    let replacement = UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    write(&mut stop, &request("shutdown"));
    assert_eq!(read(&mut stop)["result"]["shutdown"], "requested");
    drop(stop);
    assert!(wait(&mut d.child).success());
    assert!(fs::symlink_metadata(&path).is_ok());
    drop(replacement);
    // A stale, current-user socket is refused, never silently unlinked.
    let mut second = Command::new(env!("CARGO_BIN_EXE_mixyard-runtime"))
        .args(["serve", "--instance", &d.instance])
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert_eq!(wait(&mut second).code(), Some(4));
    assert!(fs::symlink_metadata(&path).is_ok());
    fs::remove_file(&path).unwrap();
    symlink("missing-target", &path).unwrap();
    let mut second = Command::new(env!("CARGO_BIN_EXE_mixyard-runtime"))
        .args(["serve", "--instance", &d.instance])
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert_eq!(wait(&mut second).code(), Some(4));
    assert!(fs::symlink_metadata(&path)
        .unwrap()
        .file_type()
        .is_symlink());
    fs::remove_file(&path).unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).unwrap();
    let mut second = Command::new(env!("CARGO_BIN_EXE_mixyard-runtime"))
        .args(["serve", "--instance", &d.instance])
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert_eq!(wait(&mut second).code(), Some(4));
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn offline_planning_needs_no_runtime_or_display() {
    let mut c = Command::new(env!("CARGO_BIN_EXE_mixyard"));
    c.args(["validate", "examples/config.json"])
        .stdout(Stdio::piped());
    headless(&mut c);
    let output = c.output().unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schemaVersion"], 2);
    for args in [
        vec!["plan", "examples/config.json", "examples/inventory.json"],
        vec![
            "plan-media",
            "examples/config.json",
            "examples/inventory.json",
            "examples/media-request.json",
        ],
    ] {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mixyard"));
        c.args(args);
        headless(&mut c);
        assert!(c.output().unwrap().status.success());
    }
}
