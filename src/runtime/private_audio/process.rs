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
const STDERR_LIMIT: usize = 4096;
// Retain the first bytes exactly, including binary diagnostics. Reads and
// storage stay bounded even when a failing child floods its pipe.
struct Diagnostic {
    name: &'static str,
    stderr: ChildStderr,
    bytes: Vec<u8>,
    observed: u64,
    eof: bool,
    overflow: bool,
    read_failed: bool,
}
impl Diagnostic {
    fn new(name: &'static str, stderr: ChildStderr) -> io::Result<Self> {
        nonblocking(stderr.as_raw_fd())?;
        Ok(Self {
            name,
            stderr,
            bytes: Vec::new(),
            observed: 0,
            eof: false,
            overflow: false,
            read_failed: false,
        })
    }
    fn capture(&mut self) {
        if self.eof || self.read_failed {
            return;
        }
        let mut buffer = [0; STDERR_LIMIT];
        for _ in 0..16 {
            match self.stderr.read(&mut buffer) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(n) => {
                    self.observed = self.observed.saturating_add(n as u64);
                    let retained = n.min(STDERR_LIMIT - self.bytes.len());
                    self.bytes.extend_from_slice(&buffer[..retained]);
                    self.overflow |= retained < n;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.read_failed = true;
                    break;
                }
            }
        }
    }
    fn failed(&self) -> bool {
        self.observed != 0 || self.read_failed
    }
    fn summary(&self) -> String {
        use std::fmt::Write;
        let mut hex = String::with_capacity(2 * self.bytes.len());
        for byte in &self.bytes {
            write!(&mut hex, "{byte:02x}").expect("writing to String");
        }
        format!(
            "owned stderr {}: observedBytes={} retainedBytes={} eof={} overflow={} incomplete={} readFailed={} rawHex={hex}",
            self.name, self.observed, self.bytes.len(), self.eof, self.overflow,
            !self.eof || self.overflow || self.read_failed, self.read_failed,
        )
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
fn realtime_policy(policy: i32, priority: i32) -> bool {
    policy & !libc::SCHED_RESET_ON_FORK == libc::SCHED_FIFO && priority == 20
}
fn task_start(bytes: &[u8], tid: u32) -> io::Result<u64> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("invalid task stat"))?;
    let (prefix, fields) = text
        .rsplit_once(')')
        .ok_or_else(|| invalid("task stat missing comm"))?;
    if prefix
        .split_once(' ')
        .and_then(|(id, _)| id.parse::<u32>().ok())
        != Some(tid)
    {
        return Err(invalid("task identity mismatch"));
    }
    fields
        .split_whitespace()
        .nth(19)
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| invalid("task starttime missing"))
}
// Point-only off-RT observation of the exact registered owned child.
fn observe_realtime_loop(pid: u32) -> io::Result<()> {
    let mut matching = 0;
    for (index, entry) in fs::read_dir(format!("/proc/{pid}/task"))?.enumerate() {
        if index >= 64 {
            return Err(invalid("owned task count exceeds bound"));
        }
        let entry = entry?;
        let tid = entry
            .file_name()
            .to_str()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|tid| *tid <= i32::MAX as u32)
            .ok_or_else(|| invalid("invalid task ID"))?;
        let comm = super::descriptor::bounded(&entry.path().join("comm"), 64)?;
        if comm != b"data-loop.0\n" {
            continue;
        }
        matching += 1;
        let path = entry.path().join("stat");
        let first = task_start(&super::descriptor::bounded(&path, 8192)?, tid)?;
        let mut param = libc::sched_param { sched_priority: 0 };
        let policy = unsafe { libc::sched_getscheduler(tid as libc::pid_t) };
        if policy < 0 || unsafe { libc::sched_getparam(tid as libc::pid_t, &mut param) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let last = task_start(&super::descriptor::bounded(&path, 8192)?, tid)?;
        if first != last
            || super::descriptor::bounded(&entry.path().join("comm"), 64)? != comm
            || !realtime_policy(policy, param.sched_priority)
        {
            return Err(invalid(
                "owned data-loop requires observed SCHED_FIFO priority20",
            ));
        }
    }
    if matching != 1 {
        return Err(invalid("expected one owned data-loop.0 task"));
    }
    Ok(())
}
#[cfg(test)]
#[test]
fn private_rt_policy_and_task_identity_fail_closed() {
    assert!(realtime_policy(libc::SCHED_FIFO, 20));
    assert!(realtime_policy(
        libc::SCHED_FIFO | libc::SCHED_RESET_ON_FORK,
        20
    ));
    assert!(!realtime_policy(libc::SCHED_OTHER, 0));
    assert!(!realtime_policy(libc::SCHED_RR, 20));
    assert!(!realtime_policy(libc::SCHED_FIFO, 19));
    let fields = format!("S {} 12345 0", ["0"; 18].join(" "));
    assert_eq!(
        task_start(format!("42 (data-loop.0) {fields}").as_bytes(), 42).unwrap(),
        12345
    );
    assert!(task_start(format!("43 (data-loop.0) {fields}").as_bytes(), 42).is_err());
    assert!(task_start(b"42 (data-loop.0) S", 42).is_err());
}

pub struct Processes {
    pub daemon: Child,
    pub companion: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    diagnostics: Vec<Diagnostic>,
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
        mut daemon: Child,
        mut companion: Child,
        cancel: UnixStream,
        observer: fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        let stdin = companion.stdin.take().unwrap();
        let stdout = companion.stdout.take().unwrap();
        nonblocking(stdin.as_raw_fd())?;
        nonblocking(stdout.as_raw_fd())?;
        cancel.set_nonblocking(true)?;
        let mut diagnostics = Vec::new();
        for (name, stderr) in [
            ("daemon", daemon.stderr.take()),
            ("companion", companion.stderr.take()),
        ] {
            if let Some(stderr) = stderr {
                diagnostics.push(Diagnostic::new(name, stderr)?);
            }
        }
        Ok(Self {
            daemon,
            companion,
            stdin,
            stdout,
            diagnostics, // Explicit test doubles, never SDK evidence.
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
            "../../../adapters/linux/private-audio/private-rt.conf"
        ))?;
        drop(file);
        let mut client_config = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(namespace.join("client.conf"))?;
        client_config.write_all(include_bytes!(
            "../../../adapters/linux/private-audio/client-rt.conf"
        ))?;
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
        let mut daemon_diagnostic =
            match Diagnostic::new("daemon", daemon.stderr.take().expect("piped daemon stderr")) {
                Ok(diagnostic) => diagnostic,
                Err(error) => {
                    terminate_startup(&mut daemon);
                    return Err(error);
                }
            };
        // Connect only after this owned core publishes its private socket.
        let ready = Instant::now();
        loop {
            daemon_diagnostic.capture();
            if daemon_diagnostic.failed() {
                terminate_startup(&mut daemon);
                daemon_diagnostic.capture();
                return Err(invalid(&format!(
                    "owned core startup diagnostics; {}",
                    daemon_diagnostic.summary()
                )));
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
        let companion_diagnostic = match Diagnostic::new(
            "companion",
            companion.stderr.take().expect("piped companion stderr"),
        ) {
            Ok(diagnostic) => diagnostic,
            Err(error) => {
                terminate_startup(&mut companion);
                terminate_startup(&mut daemon);
                return Err(error);
            }
        };
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
            diagnostics: vec![daemon_diagnostic, companion_diagnostic],
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
        self.read_events(false)
    }
    // Failure retirement may retain a final native diagnostic. Never flush a
    // fenced normal frame or treat these observations as control progress.
    pub fn retirement_events(&mut self) -> io::Result<Vec<CompanionReply>> {
        self.capture_diagnostics();
        self.read_events(true)
    }
    fn read_events(&mut self, allow_eof: bool) -> io::Result<Vec<CompanionReply>> {
        let mut events = Vec::new();
        let mut buffer = [0; 8192];
        for _ in 0..16 {
            match self.stdout.read(&mut buffer) {
                Ok(0) if allow_eof && !self.input.is_empty() => {
                    return Err(invalid("truncated final companion frame"));
                }
                Ok(0) if allow_eof || !events.is_empty() => break,
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
        self.capture_diagnostics();
        if self.diagnostics.iter().any(Diagnostic::failed) {
            return Err(invalid(
                "owned private child emitted unexpected diagnostics",
            ));
        }
        Ok(())
    }
    fn capture_diagnostics(&mut self) {
        for diagnostic in &mut self.diagnostics {
            diagnostic.capture();
        }
    }
    // Called after reaping, so a still-open pipe is explicitly incomplete.
    // At most two 4096-byte prefixes become 16384 hex characters plus labels.
    pub fn diagnostic_summary(&mut self) -> Option<String> {
        self.capture_diagnostics();
        self.diagnostics
            .iter()
            .any(|d| d.failed() || !d.eof)
            .then(|| {
                self.diagnostics
                    .iter()
                    .map(Diagnostic::summary)
                    .collect::<Vec<_>>()
                    .join("; ")
            })
    }
    pub fn observe(&self) -> io::Result<()> {
        #[cfg(test)]
        if let Some(observer) = self.observer {
            return observer();
        }
        self.qualified.observe_bindings(self.daemon.id(), true)?;
        self.qualified
            .observe_bindings(self.companion.id(), false)?;
        observe_realtime_loop(self.daemon.id())?;
        observe_realtime_loop(self.companion.id())
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
        let reaped = self.reaped()?;
        self.check_diagnostics()?;
        if reaped && self.diagnostics.iter().any(|d| !d.eof) {
            return Err(invalid(
                "owned private child stderr capture incomplete after reap",
            ));
        }
        Ok(reaped)
    }
    pub fn reaped(&mut self) -> io::Result<bool> {
        self.capture_diagnostics();
        let reaped = self.daemon.try_wait()?.is_some() && self.companion.try_wait()?.is_some();
        if reaped {
            self.capture_diagnostics();
        }
        Ok(reaped)
    }
    pub fn begin_forced(&mut self) -> io::Result<()> {
        self.capture_diagnostics();
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
        self.capture_diagnostics();
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
            let mut diagnostic =
                Diagnostic::new("companion", child.stderr.take().unwrap()).unwrap();
            assert!(child.wait().unwrap().success());
            diagnostic.capture();
            assert_eq!(!diagnostic.failed(), script == "pass");
            assert!(diagnostic.eof);
            assert_eq!(
                diagnostic.bytes,
                if script == "pass" {
                    b"".as_slice()
                } else {
                    b"warning".as_slice()
                }
            );
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
