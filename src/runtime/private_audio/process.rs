//! Owned nonblocking process transport. No background waiter or audio buffers.
use super::{descriptor::Qualified, protocol::*};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, RawFd},
        unix::{
            fs::{DirBuilderExt, OpenOptionsExt},
            net::UnixStream,
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
// Pipes have bounded kernel storage. One unexpected byte invalidates the run;
// never accumulate unbounded logs or silently discard an SDK warning.
fn quiet(stderr: &mut ChildStderr) -> io::Result<()> {
    let mut byte = [0; 1];
    match stderr.read(&mut byte) {
        Ok(0) => Ok(()),
        Ok(_) => Err(invalid(
            "owned private child emitted unexpected diagnostics",
        )),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(()),
        Err(error) => Err(error),
    }
}
struct Outbound {
    bytes: Vec<u8>,
    at: usize,
}
impl Outbound {
    fn append_priority(&mut self, request: &CompanionRequest) -> io::Result<()> {
        let mut frame = serde_json::to_vec(request)?;
        if frame.len() > COMPANION_LIMIT {
            return Err(invalid("priority frame too large"));
        }
        frame.push(b'\n');
        if self.bytes.len() - self.at + frame.len() > 2 * COMPANION_LIMIT {
            return Err(invalid(
                "one reservation plus one fence exceeds priority bound",
            ));
        }
        if self.at > 0 {
            self.bytes.drain(..self.at);
            self.at = 0;
        }
        self.bytes.extend(frame);
        Ok(())
    }
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            at: 0,
        }
    }
    fn queue(&mut self, request: &CompanionRequest) -> io::Result<()> {
        if self.at < self.bytes.len() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        self.bytes = serde_json::to_vec(request)?;
        if self.bytes.len() > COMPANION_LIMIT {
            return Err(invalid("companion request too large"));
        }
        self.bytes.push(b'\n');
        self.at = 0;
        Ok(())
    }
    fn flush(&mut self, writer: &mut impl Write) -> io::Result<()> {
        for _ in 0..16 {
            if self.at == self.bytes.len() {
                self.bytes.clear();
                self.at = 0;
                break;
            }
            match writer.write(&self.bytes[self.at..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => self.at += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}
pub struct Processes {
    pub daemon: Child,
    pub companion: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    diagnostics: Vec<ChildStderr>,
    namespace: PathBuf,
    cancel: UnixStream,
    normal: Outbound,
    priority: Outbound,
    input: Vec<u8>,
    pub qualified: Qualified,
    forced: bool,
    daemon_term_sent: bool,
    #[cfg(test)]
    observer: Option<fn() -> io::Result<()>>,
}
fn environment(command: &mut Command, q: &Qualified, namespace: &Path) {
    let prefix = &q.descriptor.sdk_prefix;
    command
        .env_clear()
        .env("PATH", prefix.join("bin"))
        .env("HOME", namespace)
        .env("XDG_RUNTIME_DIR", namespace)
        .env("XDG_CONFIG_HOME", namespace)
        .env("XDG_STATE_HOME", namespace)
        .env("PIPEWIRE_RUNTIME_DIR", namespace)
        .env("PIPEWIRE_CONFIG_DIR", namespace)
        .env("PIPEWIRE_CONFIG_NAME", "client.conf")
        .env("PIPEWIRE_REMOTE", "mixyard-private")
        .env("LD_LIBRARY_PATH", prefix.join("lib"))
        .env("PIPEWIRE_MODULE_DIR", prefix.join("lib/pipewire-0.3"))
        .env("SPA_PLUGIN_DIR", prefix.join("lib/spa-0.2"))
        .env("LC_ALL", "C");
}
pub(super) fn parent_death(command: &mut Command, cancel: Option<RawFd>) {
    let parent = std::process::id() as libc::pid_t;
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if let Some(fd) = cancel {
                if libc::dup2(fd, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}
impl Processes {
    #[cfg(test)]
    pub(super) fn block_priority_for_test(&mut self) -> usize {
        let mut total = 0;
        loop {
            match self.cancel.write(&[b'x'; 4096]) {
                Ok(n) => total += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return total,
                result => panic!("unexpected owned socket result: {result:?}"),
            }
        }
    }
    #[cfg(test)]
    pub(super) fn priority_pending_bytes(&self) -> usize {
        self.priority.bytes.len() - self.priority.at
    }
    #[cfg(test)]
    pub(super) fn test_owned(
        qualified: Qualified,
        daemon: Child,
        mut companion: Child,
        cancel: UnixStream,
        observer: fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        let stdin = companion.stdin.take().unwrap();
        let stdout = companion.stdout.take().unwrap();
        nonblocking(stdin.as_raw_fd())?;
        nonblocking(stdout.as_raw_fd())?;
        cancel.set_nonblocking(true)?;
        Ok(Self {
            daemon,
            companion,
            stdin,
            stdout,
            diagnostics: Vec::new(), // Explicit test doubles, never SDK evidence.
            namespace: qualified.descriptor.sdk_prefix.clone(),
            cancel,
            normal: Outbound::new(),
            priority: Outbound::new(),
            input: Vec::new(),
            qualified,
            forced: false,
            daemon_term_sent: false,
            observer: Some(observer),
        })
    }
    pub fn spawn(
        qualified: Qualified,
        directory: &Path,
        nonce: &str,
        epoch: u64,
    ) -> io::Result<Self> {
        let mut entropy = [0; 8];
        getrandom::fill(&mut entropy).map_err(io::Error::other)?;
        let namespace = directory.join(format!(
            "private-{}-{}-{:x}",
            &nonce[..16],
            epoch,
            u64::from_ne_bytes(entropy)
        ));
        fs::DirBuilder::new().mode(0o700).create(&namespace)?;
        let config = namespace.join("private.conf");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&config)?;
        file.write_all(include_bytes!(
            "../../../adapters/linux/native-host/private.conf"
        ))?;
        drop(file);
        let mut client_config = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(namespace.join("client.conf"))?;
        client_config.write_all(b"context.properties = { support.dbus = false mem.warn-mlock = false }\ncontext.spa-libs = { support.* = support/libspa-support }\ncontext.modules = [\n    { name = libpipewire-module-protocol-native }\n    { name = libpipewire-module-client-node }\n    { name = libpipewire-module-metadata }\n]\n")?;
        drop(client_config);
        // An ordinary pw-record stream needs its own public adapter factory.
        // Keep it out of the native companion config and private daemon modules.
        let mut recorder_config = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(namespace.join("recorder.conf"))?;
        recorder_config.write_all(b"context.properties = { support.dbus = false mem.warn-mlock = false }\ncontext.spa-libs = { support.* = support/libspa-support }\ncontext.modules = [\n    { name = libpipewire-module-protocol-native }\n    { name = libpipewire-module-client-node }\n    { name = libpipewire-module-metadata }\n    { name = libpipewire-module-adapter }\n]\n")?;
        drop(recorder_config);
        let mut server = Command::new(qualified.descriptor.sdk_prefix.join("bin/pipewire"));
        environment(&mut server, &qualified, &namespace);
        server.env("PIPEWIRE_CONFIG_NAME", "private.conf");
        server
            .arg("-c")
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        parent_death(&mut server, None);
        let (cancel, child_cancel) = UnixStream::pair()?;
        cancel.set_nonblocking(true)?;
        let mut daemon = server.spawn()?;
        let mut daemon_stderr = daemon.stderr.take().expect("piped daemon stderr");
        if let Err(error) = nonblocking(daemon_stderr.as_raw_fd()) {
            terminate_startup(&mut daemon);
            return Err(error);
        }
        // Connect only after this owned core publishes its private socket.
        let ready = Instant::now();
        loop {
            if let Err(error) = quiet(&mut daemon_stderr) {
                terminate_startup(&mut daemon);
                return Err(error);
            }
            use std::os::unix::fs::{FileTypeExt, MetadataExt};
            if let Ok(metadata) = fs::symlink_metadata(namespace.join("mixyard-private")) {
                if metadata.file_type().is_socket() && metadata.uid() == unsafe { libc::geteuid() }
                {
                    break;
                }
                terminate_startup(&mut daemon);
                return Err(invalid("owned core published invalid socket"));
            }
            match daemon.try_wait() {
                Ok(Some(_)) => return Err(invalid("owned core exited before publishing socket")),
                Ok(None) => {}
                Err(error) => {
                    terminate_startup(&mut daemon);
                    return Err(error);
                }
            }
            if ready.elapsed() >= Duration::from_millis(INSTALL_MS) {
                terminate_startup(&mut daemon);
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut command = Command::new(&qualified.descriptor.companion_executable);
        environment(&mut command, &qualified, &namespace);
        command
            .arg("--descriptor")
            .arg(&qualified.descriptor_path)
            .arg("--instance-nonce")
            .arg(nonce)
            .arg("--control-epoch")
            .arg(epoch.to_string())
            .arg("--cancel-fd")
            .arg("3")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        parent_death(&mut command, Some(child_cancel.as_raw_fd()));
        let mut companion = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                terminate_startup(&mut daemon);
                return Err(error);
            }
        };
        drop(child_cancel);
        let companion_stderr = companion.stderr.take().expect("piped companion stderr");
        if let Err(error) = nonblocking(companion_stderr.as_raw_fd()) {
            terminate_startup(&mut companion);
            terminate_startup(&mut daemon);
            return Err(error);
        }
        let handles = companion.stdin.take().zip(companion.stdout.take());
        let Some((stdin, stdout)) = handles else {
            terminate_startup(&mut companion);
            terminate_startup(&mut daemon);
            return Err(invalid("missing piped companion handles"));
        };
        let processes = Self {
            daemon,
            companion,
            stdin,
            stdout,
            diagnostics: vec![daemon_stderr, companion_stderr],
            namespace,
            cancel,
            normal: Outbound::new(),
            priority: Outbound::new(),
            input: Vec::new(),
            qualified,
            forced: false,
            daemon_term_sent: false,
            #[cfg(test)]
            observer: None,
        };
        nonblocking(processes.stdin.as_raw_fd())?;
        nonblocking(processes.stdout.as_raw_fd())?;
        Ok(processes)
    }
    pub fn queue(&mut self, request: &CompanionRequest) -> io::Result<()> {
        if matches!(
            request.action,
            CompanionAction::Fence { .. } | CompanionAction::Reserve { .. }
        ) {
            self.priority.append_priority(request)
        } else {
            self.normal.queue(request)
        }
    }
    pub fn connection(&self) -> PrivateCore {
        PrivateCore {
            runtime_directory: self.namespace.clone(),
            config_directory: self.namespace.clone(),
            remote: "mixyard-private".into(),
        }
    }
    pub fn events(&mut self) -> io::Result<Vec<CompanionReply>> {
        self.check_diagnostics()?;
        self.priority.flush(&mut self.cancel)?;
        self.normal.flush(&mut self.stdin)?;
        let mut events = Vec::new();
        let mut buffer = [0; 8192];
        for _ in 0..16 {
            match self.stdout.read(&mut buffer) {
                Ok(0) if !events.is_empty() => break,
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => self.input.extend_from_slice(&buffer[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
            while let Some(end) = self.input.iter().position(|b| *b == b'\n') {
                if end > COMPANION_LIMIT {
                    return Err(invalid("companion frame too large"));
                }
                events.push(serde_json::from_slice(&self.input[..end])?);
                self.input.drain(..=end);
                if events.len() > 16 {
                    return Err(invalid("companion event flood"));
                }
            }
            if self.input.len() > COMPANION_LIMIT {
                return Err(invalid("companion frame too large"));
            }
        }
        Ok(events)
    }
    fn check_diagnostics(&mut self) -> io::Result<()> {
        for stderr in &mut self.diagnostics {
            quiet(stderr)?;
        }
        Ok(())
    }
    pub fn observe(&self) -> io::Result<()> {
        #[cfg(test)]
        if let Some(observer) = self.observer {
            return observer();
        }
        self.qualified.observe_bindings(self.daemon.id(), true)?;
        self.qualified.observe_bindings(self.companion.id(), false)
    }
    pub fn begin_graceful(&mut self, request: &CompanionRequest) -> io::Result<()> {
        // A fenced normal request may still have a prefix in the child's stdin.
        // Never append Shutdown to that incomplete frame or resume its suffix.
        self.priority.append_priority(request)?;
        self.priority.flush(&mut self.cancel)?;
        Ok(())
    }
    pub fn graceful_progress(&mut self) -> io::Result<bool> {
        self.check_diagnostics()?;
        if self.companion.try_wait()?.is_none() {
            self.priority.flush(&mut self.cancel)?;
        } else if !self.daemon_term_sent {
            // Preserve the shared collective 1s budget, but let the companion
            // destroy its private client context before terminating its core.
            // Otherwise our own normal Stop races a real core-error callback.
            if self.daemon.try_wait()?.is_none() {
                let result = unsafe { libc::kill(self.daemon.id() as libc::pid_t, libc::SIGTERM) };
                if result != 0 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        return Err(error);
                    }
                }
            }
            self.daemon_term_sent = true;
        }
        self.reaped()
    }
    pub fn reaped(&mut self) -> io::Result<bool> {
        Ok(self.daemon.try_wait()?.is_some() && self.companion.try_wait()?.is_some())
    }
    pub fn begin_forced(&mut self) -> io::Result<()> {
        self.forced = true;
        for child in [&mut self.daemon, &mut self.companion] {
            if child.try_wait()?.is_none() {
                unsafe {
                    libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
                }
            }
        }
        Ok(())
    }
    pub fn force_kill(&mut self) -> io::Result<()> {
        for child in [&mut self.daemon, &mut self.companion] {
            if child.try_wait()?.is_none() {
                self.forced = true;
                child.kill()?;
            }
        }
        Ok(())
    }
    pub fn retire(&mut self) -> io::Result<bool> {
        if self.reaped()? {
            return Ok(true);
        }
        self.begin_forced()?;
        for (index, budget) in [TERM_MS, KILL_MS].into_iter().enumerate() {
            let deadline = Instant::now() + Duration::from_millis(budget);
            loop {
                if self.reaped()? {
                    return Ok(true);
                }
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            if index == 0 {
                self.force_kill()?;
            }
        }
        Ok(false)
    }
    pub fn any_exited(&mut self) -> io::Result<bool> {
        Ok(self.daemon.try_wait()?.is_some() || self.companion.try_wait()?.is_some())
    }
}
fn terminate_startup(child: &mut Child) {
    let _ = child.kill();
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(KILL_MS) {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
impl Drop for Processes {
    fn drop(&mut self) {
        let _ = self.retire();
        // Preserve namespace/evidence: final fallback never claims scrub.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Real product pipes and shutdown transport, with an explicit owner API
    /// double. This does not execute or qualify the native companion handler.
    #[test]
    fn graceful_shutdown_uses_priority_after_a_partially_written_normal_frame() -> io::Result<()> {
        let mut daemon_command = Command::new("/usr/bin/python3");
        daemon_command
            .env_clear()
            .args(["-c", "import time;time.sleep(60)"]);
        parent_death(&mut daemon_command, None);
        let daemon = daemon_command.spawn()?;
        let (cancel, child_cancel) = UnixStream::pair()?;
        let mut command = Command::new("/usr/bin/python3");
        command
            .env_clear()
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .arg("-c")
            .arg(
                r#"
import json,os,select,sys
prefix=b''
while len(prefix)<16: prefix+=os.read(0,16-len(prefix))
def priority():
 b=b''
 while not b.endswith(b'\n'): b+=os.read(3,4096)
 return json.loads(b)
assert priority()['action']['kind']=='fence'
print('FENCED',flush=True) # API-double ACK, not native fence proof.
while True:
 ready,_,_=select.select([0,3],[],[],2)
 assert ready
 if 0 in ready:
  prefix+=os.read(0,4096)
  assert b'\n' not in prefix, 'normal frame was corrupted during shutdown'
 if 3 in ready:
  assert priority()['action']['kind']=='shutdown'
  sys.exit(0)
"#,
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        parent_death(&mut command, Some(child_cancel.as_raw_fd()));
        let companion = command.spawn()?;
        drop(child_cancel);
        let qualified = Qualified::test_double(Descriptor {
            schema_version: VERSION,
            scope: Scope::OwnedPrivateGeneratedV1,
            sdk_prefix: PathBuf::from("/dev/shm"),
            prefix_manifest: PathBuf::from("/not-an-sdk-manifest"),
            prefix_manifest_sha256: "0".repeat(64),
            companion_executable: PathBuf::from("/not-a-native-binary"),
            companion_sha256: "0".repeat(64),
            sdk_version: "1.4.8".into(),
            sdk_commit: "8b43ed5c910e892d53de5a96aa01e9280a5b6650".into(),
            generators: Vec::new(),
            limits: PrivateLimits {
                max_sources: 1,
                max_outputs: 4,
                max_resident_bytes: 1024 * 1024,
                max_frames: 128,
            },
        });
        let mut processes = Processes::test_owned(qualified, daemon, companion, cancel, || Ok(()))?;
        let normal = CompanionRequest {
            companion_version: VERSION,
            instance_nonce: "a".repeat(64),
            control_epoch: 1,
            operation_id: "normal-snapshot".into(),
            action: CompanionAction::Snapshot {},
        };
        processes.queue(&normal)?;
        // Deliver an actual prefix while retaining its unsent suffix. The
        // double ACKs Fence only after reading that prefix from real stdin.
        let written = processes.stdin.write(&processes.normal.bytes[..16])?;
        assert_eq!(written, 16);
        processes.normal.at = written;
        let normal_len = processes.normal.bytes.len();
        let fence = CompanionRequest {
            companion_version: VERSION,
            instance_nonce: "a".repeat(64),
            control_epoch: 1,
            operation_id: "fence-2".into(),
            action: CompanionAction::Fence {
                expected: StopTarget {
                    instance_nonce: "a".repeat(64),
                    control_epoch: 1,
                    active_generation: None,
                    pending_install: None,
                },
                new_control_epoch: 2,
            },
        };
        processes.queue(&fence)?;
        let started = Instant::now();
        let mut ack = Vec::new();
        while !ack.ends_with(b"\n") {
            processes.priority.flush(&mut processes.cancel)?;
            let mut buffer = [0; 32];
            match processes.stdout.read(&mut buffer) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => ack.extend_from_slice(&buffer[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
            assert!(started.elapsed() < Duration::from_secs(2));
            std::thread::yield_now();
        }
        assert_eq!(ack, b"FENCED\n");
        let shutdown = CompanionRequest {
            companion_version: VERSION,
            instance_nonce: "a".repeat(64),
            control_epoch: 2,
            operation_id: "fence-2".into(),
            action: CompanionAction::Shutdown {},
        };
        processes.begin_graceful(&shutdown)?;
        assert_eq!(processes.normal.at, 16);
        assert_eq!(processes.normal.bytes.len(), normal_len);
        let started = Instant::now();
        while !processes.graceful_progress()? {
            assert!(started.elapsed() < Duration::from_secs(2));
            std::thread::yield_now();
        }
        assert!(processes.companion.try_wait()?.unwrap().success());
        assert!(processes.reaped()?);
        Ok(())
    }
    #[test]
    #[ignore = "requires explicitly precompiled owned portable C helper; no SDK"]
    fn review_native_contract_launcher_fd3_reaches_production_validator() {
        let helper = std::env::var_os("MIXYARD_PRIVATE_OWNER_CHECK").expect("owned C helper path");
        for (mode, success) in [("--old-fifo-fd3", false), ("--validate-fd3", true)] {
            let (_parent, child) = UnixStream::pair().unwrap();
            let mut command = Command::new(&helper);
            command
                .env_clear()
                .arg(mode)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            parent_death(&mut command, Some(child.as_raw_fd()));
            let mut process = command.spawn().unwrap();
            drop(child);
            assert_eq!(process.wait().unwrap().success(), success);
        }
    }
    #[test]
    fn actual_success_exit_does_not_hide_owned_child_diagnostics() {
        for script in ["pass", "import os;os.write(2,b'warning')"] {
            let mut child = Command::new("/usr/bin/python3")
                .env_clear()
                .arg("-c")
                .arg(script)
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut stderr = child.stderr.take().unwrap();
            nonblocking(stderr.as_raw_fd()).unwrap();
            assert!(child.wait().unwrap().success());
            assert_eq!(quiet(&mut stderr).is_ok(), script == "pass");
        }
    }
    /// This is a pipe/cancel transport double, never a native owner proof.
    #[test]
    fn priority_cancel_reaches_owned_child_while_normal_pipe_is_saturated() {
        let (mut cancel, child_cancel) = UnixStream::pair().unwrap();
        cancel.set_nonblocking(true).unwrap();
        let mut command = Command::new("/usr/bin/python3");
        command.env_clear().env("PYTHONDONTWRITEBYTECODE","1").arg("-c").arg("import os,json,sys\nb=b''\nwhile not b.endswith(b'\\n'): b+=os.read(3,8192)\nr=json.loads(b)\nprint(json.dumps(r),flush=True)\n").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        parent_death(&mut command, Some(child_cancel.as_raw_fd()));
        let mut child = command.spawn().unwrap();
        drop(child_cancel);
        let result = (|| -> io::Result<()> {
            let mut stdin = child.stdin.take().unwrap();
            nonblocking(stdin.as_raw_fd())?;
            let mut normal = Outbound {
                bytes: vec![b'x'; COMPANION_LIMIT],
                at: 0,
            };
            normal.flush(&mut stdin)?;
            assert!(normal.at < normal.bytes.len());
            let request = CompanionRequest {
                companion_version: VERSION,
                instance_nonce: "a".repeat(64),
                control_epoch: 2,
                operation_id: "stop-owned".into(),
                action: CompanionAction::Fence {
                    expected: StopTarget {
                        instance_nonce: "a".repeat(64),
                        control_epoch: 1,
                        active_generation: None,
                        pending_install: Some(PendingInstall {
                            operation_id: "install-owned".into(),
                            reserved_generation: 1,
                        }),
                    },
                    new_control_epoch: 2,
                },
            };
            let mut priority = Outbound::new();
            priority.queue(&request)?;
            let mut stdout = child.stdout.take().unwrap();
            nonblocking(stdout.as_raw_fd())?;
            let mut bytes = Vec::new();
            let started = Instant::now();
            loop {
                priority.flush(&mut cancel)?;
                let mut buffer = [0; 4096];
                match stdout.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        bytes.extend_from_slice(&buffer[..n]);
                        if bytes.ends_with(b"\n") {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }
                if started.elapsed() > Duration::from_secs(2) {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                std::thread::yield_now();
            }
            let echoed: CompanionRequest = serde_json::from_slice(&bytes)?;
            assert_eq!(echoed.operation_id, "stop-owned");
            assert_eq!(echoed.control_epoch, 2);
            assert!(matches!(echoed.action, CompanionAction::Fence { .. }));
            assert!(normal.at < normal.bytes.len());
            Ok(())
        })();
        terminate_startup(&mut child);
        result.unwrap();
    }
}
