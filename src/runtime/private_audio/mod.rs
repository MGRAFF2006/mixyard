//! Opt-in owned private generated audio. Separate from lifecycle capabilities.
pub mod client;
mod descriptor;
mod process;
pub mod protocol;
pub mod state;

use crate::profiles::{CurrentSnapshot, CurrentSnapshotGuard, Store};
use protocol::*;
use state::{Coordinator, Transition};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Options {
    pub descriptor: PathBuf,
    pub profiles: PathBuf,
}
const STORE_LIMIT: usize = 1024 * 1024;
fn failure(code: ErrorCode, message: impl Into<String>) -> Failure {
    Failure {
        code,
        message: message.into(),
    }
}
fn store_failure(error: crate::profiles::Error) -> Failure {
    let code = match error.code {
        "stale-store" => ErrorCode::StaleStore,
        "stale-config" => ErrorCode::StaleConfig,
        "store-busy" => ErrorCode::Busy,
        _ => ErrorCode::OwnerFailed,
    };
    failure(code, error.to_string())
}
fn desired(snapshot: &CurrentSnapshot) -> DesiredSnapshot {
    DesiredSnapshot {
        store_revision: snapshot.store_revision,
        config_revision: snapshot.config_revision,
        config_sha256: snapshot.config_sha256.clone(),
        config: snapshot.config.clone(),
    }
}

struct Peer {
    stream: super::transport::Stream,
    input: Vec<u8>,
    output: Vec<u8>,
    written: usize,
    started: Instant,
    request_id: String,
    operation: Option<String>,
    done: bool,
}
impl Peer {
    fn new(stream: super::transport::Stream) -> Self {
        Self {
            stream,
            input: Vec::new(),
            output: Vec::new(),
            written: 0,
            started: Instant::now(),
            request_id: String::new(),
            operation: None,
            done: false,
        }
    }
    fn reply(&mut self, result: Result<ResultValue, Failure>) {
        let (result, error) = match result {
            Ok(value) => (Some(value), None),
            Err(error) => (None, Some(error)),
        };
        let reply = Reply {
            private_audio_version: VERSION,
            request_id: self.request_id.clone(),
            result,
            error,
        };
        match serde_json::to_vec(&reply) {
            Ok(mut bytes) if bytes.len() <= RESULT_LIMIT => {
                bytes.push(b'\n');
                self.output = bytes;
                self.written = 0;
                self.done = true;
                self.operation = None;
                self.started = Instant::now();
            }
            _ => {
                let fallback = Reply {
                    private_audio_version: VERSION,
                    request_id: self.request_id.clone(),
                    result: None,
                    error: Some(failure(
                        ErrorCode::ResourceLimit,
                        "private result exceeds 65536 bytes",
                    )),
                };
                self.output =
                    serde_json::to_vec(&fallback).expect("fixed error envelope serializes");
                self.output.push(b'\n');
                self.written = 0;
                self.done = true;
                self.operation = None;
                self.started = Instant::now();
            }
        }
    }
    fn receive(&mut self) -> io::Result<Option<Request>> {
        if self.done || self.operation.is_some() {
            let mut byte = [0; 1];
            return match self.stream.read(&mut byte) {
                Ok(0) => Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(_) => Err(io::ErrorKind::InvalidData.into()),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
                Err(e) => Err(e),
            };
        }
        let mut buffer = [0; 1024];
        match self.stream.read(&mut buffer) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => self.input.extend_from_slice(&buffer[..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(e) => return Err(e),
        }
        if let Some(end) = self.input.iter().position(|b| *b == b'\n') {
            if end > REQUEST_LIMIT || end + 1 != self.input.len() {
                self.reply(Err(failure(
                    ErrorCode::ProtocolError,
                    "invalid private request frame",
                )));
                return Ok(None);
            }
            match serde_json::from_slice::<Request>(&self.input[..end]) {
                Ok(request) => {
                    self.request_id = request.request_id.clone();
                    if let Err(reason) = request.validate() {
                        self.reply(Err(failure(ErrorCode::ProtocolError, reason)));
                        Ok(None)
                    } else {
                        Ok(Some(request))
                    }
                }
                Err(_) => {
                    self.reply(Err(failure(
                        ErrorCode::ProtocolError,
                        "closed private request schema rejected",
                    )));
                    Ok(None)
                }
            }
        } else if self.input.len() > REQUEST_LIMIT {
            self.reply(Err(failure(
                ErrorCode::ProtocolError,
                "private request exceeds 8192 bytes",
            )));
            Ok(None)
        } else {
            Ok(None)
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        for _ in 0..16 {
            if self.written == self.output.len() {
                break;
            }
            match self.stream.write(&self.output[self.written..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => self.written += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

struct Retirement {
    started: Instant,
    killed: bool,
    operation: Option<String>,
    receipt: Option<Fenced>,
}

pub(crate) struct Service {
    listener: super::transport::Listener,
    peers: Vec<Peer>,
    coordinator: Coordinator,
    processes: process::Processes,
    store: Store,
    prepared: Option<CurrentSnapshot>,
    guard: Option<CurrentSnapshotGuard>,
    install_started: Option<Instant>,
    fence_started: Option<Instant>,
    failed: bool,
    retired: bool,
    retirement: Option<Retirement>,
    directory: PathBuf,
    shutting_down: bool,
}
impl Service {
    pub(crate) fn start(instance: &str, directory: &Path, options: Options) -> io::Result<Self> {
        if !options.descriptor.is_absolute() || !options.profiles.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "private descriptor and profiles must both be absolute",
            ));
        }
        let qualified = descriptor::Qualified::load(&options.descriptor)?;
        let store = Store::new(options.profiles).map_err(io::Error::other)?;
        let mut entropy = [0; 32];
        getrandom::fill(&mut entropy).map_err(io::Error::other)?;
        let nonce = entropy
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let mut coordinator =
            Coordinator::new(nonce.clone()).map_err(|e| io::Error::other(e.message))?;
        let listener = super::transport::Listener::bind(&client::endpoint(instance)?)?;
        let processes = process::Processes::spawn(
            qualified,
            directory,
            &nonce,
            coordinator.snapshot().stop_target.control_epoch,
        )?;
        coordinator
            .register_children(&processes.companion, &processes.daemon)
            .map_err(|e| io::Error::other(e.message))?;
        Ok(Self {
            listener,
            peers: Vec::new(),
            coordinator,
            processes,
            store,
            prepared: None,
            guard: None,
            install_started: None,
            fence_started: None,
            failed: false,
            retired: false,
            retirement: None,
            directory: directory.to_owned(),
            shutting_down: false,
        })
    }
    fn dispatch(&mut self, operation: Operation) -> Result<Option<CompanionRequest>, Failure> {
        match operation {
            Operation::Status {} => Ok(None),
            Operation::Prepare {
                instance_nonce,
                expected_store_revision,
                expected_config_revision,
            } => {
                self.coordinator.check_instance(&instance_nonce)?;
                let snapshot = self
                    .store
                    .current_snapshot(STORE_LIMIT)
                    .map_err(store_failure)?;
                if snapshot.store_revision != expected_store_revision {
                    return Err(failure(
                        ErrorCode::StaleStore,
                        "canonical store revision differs",
                    ));
                }
                if snapshot.config_revision != expected_config_revision {
                    return Err(failure(
                        ErrorCode::StaleConfig,
                        "canonical config revision differs",
                    ));
                }
                let limits = &self.processes.qualified.descriptor.limits;
                if snapshot.config.sources.len() > limits.max_sources as usize
                    || snapshot.config.outputs.len() > limits.max_outputs as usize
                {
                    return Err(failure(
                        ErrorCode::ResourceLimit,
                        "canonical source/output budget exceeded",
                    ));
                }
                if self.retired {
                    let status = self.coordinator.snapshot();
                    if !matches!(status.phase, Phase::Stopped | Phase::FailedStopped) {
                        return Err(failure(
                            ErrorCode::OwnerFailed,
                            "owned retirement is incomplete",
                        ));
                    }
                    let qualified =
                        descriptor::Qualified::load(&self.processes.qualified.descriptor_path)
                            .map_err(|e| failure(ErrorCode::UnqualifiedSdk, e.to_string()))?;
                    let processes = process::Processes::spawn(
                        qualified,
                        &self.directory,
                        &status.stop_target.instance_nonce,
                        status.stop_target.control_epoch,
                    )
                    .map_err(|e| failure(ErrorCode::OwnerFailed, e.to_string()))?;
                    self.coordinator
                        .register_children(&processes.companion, &processes.daemon)?;
                    self.processes = processes;
                    self.retired = false;
                    self.failed = false;
                }
                let request = self.coordinator.begin_prepare(desired(&snapshot))?;
                self.prepared = Some(snapshot);
                Ok(Some(request))
            }
            Operation::Link {
                instance_nonce,
                prepared_token,
                output_id,
                consumer,
            } => self
                .coordinator
                .begin_link(&instance_nonce, &prepared_token, output_id, consumer)
                .map(Some),
            Operation::Apply {
                instance_nonce,
                prepared_token,
                expected_store_revision,
                expected_config_revision,
                expected_inventory_revision,
            } => self
                .coordinator
                .begin_stage(
                    &instance_nonce,
                    &prepared_token,
                    expected_store_revision,
                    expected_config_revision,
                    expected_inventory_revision,
                )
                .map(Some),
            Operation::Stop { expected } => {
                let request = self.coordinator.begin_stop(&expected)?;
                self.fence_started = Some(Instant::now());
                Ok(Some(request))
            }
        }
    }
    fn release(&mut self, permit: &state::ReleasePermit) -> Result<(), Failure> {
        if permit.allows_guard_release() {
            if let Some(guard) = &mut self.guard {
                guard.release().map_err(store_failure)?;
            }
            self.guard = None;
        }
        Ok(())
    }
    fn event(&mut self, reply: CompanionReply) -> Result<(), Failure> {
        let operation = reply.operation_id.clone();
        let event = reply.event.clone();
        let transition = self.coordinator.accept(reply)?;
        let result = match transition {
            Transition::Ready => {
                self.processes
                    .observe()
                    .map_err(|e| failure(ErrorCode::UnqualifiedSdk, e.to_string()))?;
                None
            }
            Transition::Prepared => Some(ResultValue::Prepared {
                status: self.coordinator.snapshot(),
            }),
            Transition::Linked => Some(ResultValue::Linked {
                status: self.coordinator.snapshot(),
            }),
            Transition::Staged => {
                self.processes
                    .observe()
                    .map_err(|e| failure(ErrorCode::UnqualifiedSdk, e.to_string()))?;
                let snapshot = self.prepared.as_ref().ok_or_else(|| {
                    failure(ErrorCode::OwnerFailed, "staged without canonical snapshot")
                })?;
                self.guard = Some(
                    self.store
                        .guard_current_snapshot(snapshot, STORE_LIMIT)
                        .map_err(store_failure)?,
                );
                let install = self.coordinator.begin_install(&desired(snapshot))?;
                let install_operation = install.operation_id.clone();
                self.processes
                    .queue(&install)
                    .map_err(|e| failure(ErrorCode::OwnerFailed, e.to_string()))?;
                self.install_started = Some(Instant::now());
                for peer in &mut self.peers {
                    if peer.operation.as_deref() == Some(&operation) {
                        peer.operation = Some(install_operation.clone());
                        peer.started = Instant::now();
                    }
                }
                None
            }
            Transition::Installed(permit) => {
                self.install_started = None;
                self.processes
                    .observe()
                    .map_err(|e| failure(ErrorCode::UnqualifiedSdk, e.to_string()))?;
                self.release(&permit)?;
                match event {
                    CompanionEvent::Installed { receipt } => Some(ResultValue::Installed {
                        receipt,
                        status: self.coordinator.snapshot(),
                    }),
                    _ => {
                        return Err(failure(
                            ErrorCode::ProtocolError,
                            "installed transition without receipt",
                        ))
                    }
                }
            }
            Transition::Fenced(permit) => {
                self.install_started = None;
                self.release(&permit)?;
                self.fence_started = None;
                let CompanionEvent::Fenced { receipt } = event else {
                    return Err(failure(
                        ErrorCode::ProtocolError,
                        "fenced without native receipt",
                    ));
                };
                let status = self.coordinator.snapshot();
                let shutdown = CompanionRequest {
                    companion_version: VERSION,
                    instance_nonce: status.stop_target.instance_nonce,
                    control_epoch: status.stop_target.control_epoch,
                    operation_id: operation.clone(),
                    action: CompanionAction::Shutdown {},
                };
                self.processes
                    .begin_graceful(&shutdown)
                    .map_err(|e| failure(ErrorCode::OwnerFailed, e.to_string()))?;
                self.retirement = Some(Retirement {
                    started: Instant::now(),
                    killed: false,
                    operation: Some(operation.clone()),
                    receipt: Some(receipt),
                });
                None
            }
            Transition::Snapshot => None,
            Transition::Fault => {
                return Err(self
                    .coordinator
                    .snapshot()
                    .last_error
                    .unwrap_or_else(|| failure(ErrorCode::OwnerFailed, "native owner fault")))
            }
        };
        if let Some(result) = result {
            for peer in &mut self.peers {
                if peer.operation.as_deref() == Some(&operation) {
                    peer.reply(Ok(result.clone()));
                }
            }
        }
        Ok(())
    }
    fn fail(&mut self, error: Failure) {
        self.coordinator.fail_held(error.code, &error.message);
        self.failed = true;
        for peer in &mut self.peers {
            if peer.operation.is_some() {
                let operation_id = peer.operation.clone().unwrap();
                peer.reply(Ok(ResultValue::Pending {
                    operation_id,
                    status: self.coordinator.snapshot(),
                }));
            }
        }
    }
    pub(crate) fn tick(&mut self) -> io::Result<()> {
        self.listener.owned_directory()?;
        if !self.shutting_down && self.peers.len() < 16 {
            if let Some(stream) = self.listener.accept()? {
                self.peers.push(Peer::new(stream));
            }
        }
        if !self.failed && self.retirement.is_none() && !self.retired {
            match self.processes.events() {
                Ok(events) => {
                    for event in events {
                        if let Err(error) = self.event(event) {
                            self.fail(error);
                            break;
                        }
                    }
                }
                Err(error) => self.fail(failure(ErrorCode::OwnerFailed, error.to_string())),
            }
        }
        if !self.failed
            && self.retirement.is_none()
            && !self.retired
            && self.processes.any_exited()?
        {
            self.fail(failure(
                ErrorCode::OwnerFailed,
                "owned child exited without retirement proof",
            ));
        }
        let mut index = 0;
        while index < self.peers.len() {
            let request = match self.peers[index].receive() {
                Ok(request) => request,
                Err(_) => {
                    self.peers.remove(index);
                    continue;
                }
            };
            if let Some(request) = request {
                let result = self.dispatch(request.operation);
                match result {
                    Ok(None) => self.peers[index].reply(Ok(ResultValue::Status {
                        status: self.coordinator.snapshot(),
                    })),
                    Ok(Some(command)) => {
                        let operation = command.operation_id.clone();
                        match self.processes.queue(&command) {
                            Ok(()) => {
                                self.peers[index].operation = Some(operation);
                                self.peers[index].started = Instant::now();
                            }
                            Err(e) => {
                                let error = failure(ErrorCode::OwnerFailed, e.to_string());
                                self.fail(error.clone());
                                self.peers[index].reply(Err(error));
                            }
                        }
                    }
                    Err(error) => {
                        if self.coordinator.snapshot().phase == Phase::FailedHeld {
                            self.fail(error.clone());
                        }
                        self.peers[index].reply(Err(error));
                    }
                }
            }
            if self.peers[index].started.elapsed() >= Duration::from_millis(INSTALL_MS)
                && !self.peers[index].done
            {
                if let Some(operation_id) = self.peers[index].operation.clone() {
                    self.peers[index].reply(Ok(ResultValue::Pending {
                        operation_id,
                        status: self.coordinator.snapshot(),
                    }));
                } else {
                    self.peers[index].reply(Err(failure(
                        ErrorCode::ProtocolError,
                        "private request deadline",
                    )));
                }
            }
            if self.peers[index].flush().is_err()
                || (self.peers[index].done
                    && self.peers[index].started.elapsed() >= Duration::from_secs(2))
            {
                self.peers.remove(index);
                continue;
            }
            index += 1;
        }
        // This deadline belongs to the owned install, not to any IPC peer.
        // A caller may disappear; only a real fence or actual reaps releases it.
        if self
            .install_started
            .is_some_and(|start| start.elapsed() >= Duration::from_millis(INSTALL_MS))
            && self.fence_started.is_none()
            && self.retirement.is_none()
            && !self.retired
        {
            self.install_started = None;
            let target = self.coordinator.snapshot().stop_target;
            match self.coordinator.begin_stop(&target) {
                Ok(command) => match self.processes.queue(&command) {
                    Ok(()) => self.fence_started = Some(Instant::now()),
                    Err(error) => self.fail(failure(ErrorCode::OwnerFailed, error.to_string())),
                },
                Err(error) => self.fail(error),
            }
        }
        if self
            .fence_started
            .is_some_and(|start| start.elapsed() >= Duration::from_millis(FENCE_MS))
        {
            self.fail(failure(
                ErrorCode::StopIncomplete,
                "native fence deadline; retiring owned children without scrub claim",
            ));
            self.coordinator.note_forced_termination();
            self.processes.begin_forced()?;
            self.retirement = Some(Retirement {
                started: Instant::now(),
                killed: false,
                operation: None,
                receipt: None,
            });
            self.fence_started = None;
        }
        if self.failed && self.retirement.is_none() && !self.retired && self.fence_started.is_none()
        {
            self.coordinator.note_forced_termination();
            self.processes.begin_forced()?;
            self.retirement = Some(Retirement {
                started: Instant::now(),
                killed: false,
                operation: None,
                receipt: None,
            });
        }
        self.progress_retirement()?;
        Ok(())
    }
    fn progress_retirement(&mut self) -> io::Result<()> {
        let Some(retirement) = &mut self.retirement else {
            return Ok(());
        };
        let reaped = if retirement.receipt.is_some() && !retirement.killed {
            match self.processes.graceful_progress() {
                Ok(reaped) => reaped,
                Err(error) => {
                    self.coordinator
                        .fail_held(ErrorCode::OwnerFailed, &error.to_string());
                    self.coordinator.note_forced_termination();
                    self.failed = true;
                    retirement.receipt = None;
                    retirement.operation = None;
                    self.processes.begin_forced()?;
                    self.processes.reaped()?
                }
            }
        } else {
            self.processes.reaped()?
        };
        if reaped {
            let retirement = self.retirement.take().unwrap();
            self.retired = true;
            let permit = self
                .coordinator
                .prove_children_reaped(&mut self.processes.companion, &mut self.processes.daemon)
                .map_err(|e| io::Error::other(e.message))?;
            if let Err(error) = self.release(&permit) {
                self.fail(error);
                return Ok(());
            }
            if permit.allows_stop_complete() {
                if let (Some(operation), Some(receipt)) = (retirement.operation, retirement.receipt)
                {
                    let status = self.coordinator.snapshot();
                    for peer in &mut self.peers {
                        if peer.operation.as_deref() == Some(&operation) {
                            peer.reply(Ok(ResultValue::StopComplete {
                                receipt: receipt.clone(),
                                children_reaped: true,
                                status: status.clone(),
                            }));
                        }
                    }
                }
            } else {
                self.failed = true;
            }
        } else if !retirement.killed
            && retirement.started.elapsed() >= Duration::from_millis(TERM_MS)
        {
            self.coordinator.note_forced_termination();
            self.processes.force_kill()?;
            retirement.killed = true;
        } else if retirement.killed
            && retirement.started.elapsed() >= Duration::from_millis(TERM_MS + KILL_MS)
        {
            self.fail(failure(
                ErrorCode::StopIncomplete,
                "owned child did not reap within retirement deadline; writer guard retained",
            ));
            self.retirement = None;
            self.retired = true;
        }
        Ok(())
    }
    pub(crate) fn shutdown(&mut self) -> io::Result<()> {
        if self.shutting_down {
            return Ok(());
        }
        self.shutting_down = true;
        if !self.retired && self.retirement.is_none() && self.fence_started.is_none() {
            let target = self.coordinator.snapshot().stop_target;
            match self.coordinator.begin_stop(&target) {
                Ok(command) => {
                    self.processes.queue(&command)?;
                    self.fence_started = Some(Instant::now());
                }
                Err(error) => self.fail(error),
            }
        }
        let started = Instant::now();
        while !self.retired
            && started.elapsed() < Duration::from_millis(FENCE_MS + TERM_MS + KILL_MS + 20)
        {
            self.tick()?;
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
mod socket_tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    fn connection() -> (Peer, UnixStream) {
        let (a, b) = UnixStream::pair().unwrap();
        a.set_nonblocking(true).unwrap();
        b.set_nonblocking(true).unwrap();
        (Peer::new(a), b)
    }
    fn receive(peer: &mut Peer) -> Option<Request> {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(request) = peer.receive().unwrap() {
                return Some(request);
            }
            if peer.done {
                return None;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    #[test]
    fn actual_socket_parser_rejects_unknown_fields_and_bounded_frames() {
        for bytes in [b"{\"privateAudioVersion\":1,\"requestId\":\"x\",\"operation\":{\"kind\":\"status\",\"pcm\":[]}}\n".to_vec(),vec![b'x';REQUEST_LIMIT+1]] {
            let (mut peer,mut client)=connection();client.write_all(&bytes).unwrap();assert!(receive(&mut peer).is_none());
            peer.flush().unwrap();let mut response=[0;1024];let count=client.read(&mut response).unwrap();let reply:Reply=serde_json::from_slice(&response[..count]).unwrap();assert_eq!(reply.error.unwrap().code,ErrorCode::ProtocolError);
        }
        let (mut peer, mut client) = connection();
        client.write_all(b"{\"privateAudioVersion\":1,\"requestId\":\"exact\",\"operation\":{\"kind\":\"status\"}}\n").unwrap();
        assert!(matches!(
            receive(&mut peer).unwrap().operation,
            Operation::Status {}
        ));
        let status = Coordinator::new("a".repeat(64)).unwrap().snapshot();
        peer.reply(Ok(ResultValue::Status { status }));
        peer.flush().unwrap();
        let mut response = [0; 4096];
        let count = client.read(&mut response).unwrap();
        let reply: Reply = serde_json::from_slice(&response[..count]).unwrap();
        assert_eq!(reply.request_id, "exact");
        assert!(reply.error.is_none());
    }
    #[test]
    fn disconnect_drops_only_peer_and_oversize_error_has_closed_bounded_response() {
        let (mut peer, client) = connection();
        peer.operation = Some("owned-install".into());
        drop(client);
        assert!(peer.receive().is_err());
        assert_eq!(peer.operation.as_deref(), Some("owned-install"));
        let (mut peer, mut client) = connection();
        peer.request_id = "bounded".into();
        peer.reply(Err(failure(
            ErrorCode::OwnerFailed,
            "x".repeat(RESULT_LIMIT),
        )));
        peer.flush().unwrap();
        let mut response = [0; 1024];
        let count = client.read(&mut response).unwrap();
        assert!(count < RESULT_LIMIT);
        let reply: Reply = serde_json::from_slice(&response[..count]).unwrap();
        assert_eq!(reply.error.unwrap().code, ErrorCode::ResourceLimit);
    }
}

#[cfg(test)]
mod service_tests;
