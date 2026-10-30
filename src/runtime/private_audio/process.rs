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
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
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
struct Outbound {
    bytes: Vec<u8>,
    at: usize,
}
impl Outbound {
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
    cancel: UnixStream,
    normal: Outbound,
    priority: Outbound,
    input: Vec<u8>,
    pub qualified: Qualified,
    forced: bool,
    retirement_started: bool,
}
fn environment(command: &mut Command, q: &Qualified, namespace: &Path) {
    let prefix = &q.descriptor.sdk_prefix;
    command
        .env_clear()
        .env("PATH", prefix.join("bin"))
        .env("HOME", namespace)
        .env("XDG_RUNTIME_DIR", namespace)
        .env("XDG_CONFIG_HOME", namespace)
        .env("PIPEWIRE_RUNTIME_DIR", namespace)
        .env("PIPEWIRE_CONFIG_DIR", namespace)
        .env("PIPEWIRE_CONFIG_NAME", "private.conf")
        .env("PIPEWIRE_REMOTE", "mixyard-private")
        .env("LD_LIBRARY_PATH", prefix.join("lib"))
        .env("PIPEWIRE_MODULE_DIR", prefix.join("lib/pipewire-0.3"))
        .env("SPA_PLUGIN_DIR", prefix.join("lib/spa-0.2"))
        .env("LC_ALL", "C");
}
fn parent_death(command: &mut Command, cancel: Option<RawFd>) {
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
        let mut server = Command::new(qualified.descriptor.sdk_prefix.join("bin/pipewire"));
        environment(&mut server, &qualified, &namespace);
        server
            .arg("-c")
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        parent_death(&mut server, None);
        let (cancel, child_cancel) = UnixStream::pair()?;
        cancel.set_nonblocking(true)?;
        let mut daemon = server.spawn()?;
        // Connect only after this owned core publishes its private socket.
        let ready = Instant::now();
        loop {
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
            .stderr(Stdio::null());
        parent_death(&mut command, Some(child_cancel.as_raw_fd()));
        let mut companion = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                terminate_startup(&mut daemon);
                return Err(error);
            }
        };
        drop(child_cancel);
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
            cancel,
            normal: Outbound::new(),
            priority: Outbound::new(),
            input: Vec::new(),
            qualified,
            forced: false,
            retirement_started: false,
        };
        nonblocking(processes.stdin.as_raw_fd())?;
        nonblocking(processes.stdout.as_raw_fd())?;
        Ok(processes)
    }
    pub fn queue(&mut self, request: &CompanionRequest) -> io::Result<()> {
        if matches!(request.action, CompanionAction::Fence { .. }) {
            self.priority.queue(request)
        } else {
            self.normal.queue(request)
        }
    }
    pub fn events(&mut self) -> io::Result<Vec<CompanionReply>> {
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
    pub fn observe(&self) -> io::Result<()> {
        self.qualified.observe_bindings(self.daemon.id(), true)?;
        self.qualified.observe_bindings(self.companion.id(), false)
    }
    pub fn begin_graceful(&mut self, request: &CompanionRequest) -> io::Result<()> {
        self.retirement_started = true;
        // A verified owner fence makes older normal commands permanently inert.
        self.normal = Outbound::new();
        self.normal.queue(request)?;
        self.normal.flush(&mut self.stdin)?;
        if self.daemon.try_wait()?.is_none() {
            unsafe {
                libc::kill(self.daemon.id() as libc::pid_t, libc::SIGTERM);
            }
        }
        Ok(())
    }
    pub fn graceful_progress(&mut self) -> io::Result<bool> {
        if self.companion.try_wait()?.is_none() {
            self.normal.flush(&mut self.stdin)?;
        }
        self.reaped()
    }
    pub fn reaped(&mut self) -> io::Result<bool> {
        Ok(self.daemon.try_wait()?.is_some() && self.companion.try_wait()?.is_some())
    }
    pub fn begin_forced(&mut self) -> io::Result<()> {
        self.retirement_started = true;
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
        self.retirement_started = true;
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
        if !self.retirement_started {
            let _ = self.retire();
        } /* Preserve namespace/evidence: no forced path claims scrub. */
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
