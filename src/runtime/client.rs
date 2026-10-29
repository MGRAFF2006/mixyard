//! Typed lifecycle subscription for a caller-owned background worker.
use super::{endpoint, frame, transport, CLIENT_TIMEOUT, IPC_VERSION, MAX_FRAME, TICK};
use serde::{de::Error, Deserialize, Deserializer, Serialize};
use serde_json::json;
use std::{
    fmt,
    io::{self, Read, Write},
    thread,
    time::Instant,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capabilities {
    #[serde(deserialize_with = "unavailable")]
    pub apply: bool,
    #[serde(deserialize_with = "unavailable")]
    pub mute: bool,
    #[serde(deserialize_with = "unavailable")]
    pub native_action_dispatch: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Backend {
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeAudioProof {
    Unverified,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Availability {
    #[serde(deserialize_with = "reachable")]
    pub daemon_reachable: bool,
    #[serde(deserialize_with = "unavailable")]
    pub backend_available: bool,
    pub backend: Backend,
    #[serde(deserialize_with = "null_revision")]
    pub audio_revision: Option<u64>,
    #[serde(deserialize_with = "null_revision")]
    pub applied_revision: Option<u64>,
    #[serde(deserialize_with = "null_revision")]
    pub desired_revision: Option<u64>,
    pub native_audio_proof: NativeAudioProof,
    pub capabilities: Capabilities,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Lifecycle {
    Running,
    Stopping,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub sequence: u64,
    #[serde(deserialize_with = "reachable")]
    pub resync: bool,
    pub lifecycle: Lifecycle,
    pub availability: Availability,
    #[serde(deserialize_with = "unavailable")]
    pub history_available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LifecycleEvent {
    pub sequence: u64,
    pub lifecycle: Lifecycle,
    #[serde(deserialize_with = "unavailable")]
    pub backend_available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase", deny_unknown_fields)]
pub enum RuntimeEvent {
    Snapshot(Snapshot),
    Lifecycle(LifecycleEvent),
}

fn reachable<'de, D: Deserializer<'de>>(de: D) -> Result<bool, D::Error> {
    if bool::deserialize(de)? {
        Ok(true)
    } else {
        Err(D::Error::custom("expected true for lifecycle runtime"))
    }
}

fn unavailable<'de, D: Deserializer<'de>>(de: D) -> Result<bool, D::Error> {
    if bool::deserialize(de)? {
        Err(D::Error::custom("unsupported lifecycle runtime capability"))
    } else {
        Ok(false)
    }
}

fn null_revision<'de, D: Deserializer<'de>>(de: D) -> Result<Option<u64>, D::Error> {
    if Option::<u64>::deserialize(de)?.is_some() {
        Err(D::Error::custom("lifecycle runtime revisions must be null"))
    } else {
        Ok(None)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Response {
    ipc_version: u32,
    request_id: String,
    #[serde(default, deserialize_with = "non_null")]
    result: Option<RuntimeEvent>,
    #[serde(default, deserialize_with = "non_null")]
    error: Option<ServerError>,
}

/// A bounded runtime rejection preserved as the source of an `io::Error`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerError {
    #[serde(deserialize_with = "error_code")]
    pub code: String,
    #[serde(deserialize_with = "error_message")]
    pub message: String,
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "runtime {}: {}", self.code, self.message)
    }
}

impl std::error::Error for ServerError {}

fn non_null<'de, D: Deserializer<'de>, T: Deserialize<'de>>(de: D) -> Result<Option<T>, D::Error> {
    T::deserialize(de).map(Some)
}

fn bounded_text<'de, D: Deserializer<'de>>(de: D, limit: usize) -> Result<String, D::Error> {
    let text = String::deserialize(de)?;
    if text.is_empty() || text.len() > limit {
        Err(D::Error::custom(
            "runtime error text exceeds supported bounds",
        ))
    } else {
        Ok(text)
    }
}

fn error_code<'de, D: Deserializer<'de>>(de: D) -> Result<String, D::Error> {
    bounded_text(de, 64)
}

fn error_message<'de, D: Deserializer<'de>>(de: D) -> Result<String, D::Error> {
    bounded_text(de, 1024)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

// One nonblocking read per call; a partial frame has a deadline even after connect.
fn read_event(
    stream: &mut transport::Stream,
    input: &mut Vec<u8>,
    started: &mut Option<Instant>,
    request_id: &str,
) -> io::Result<Option<RuntimeEvent>> {
    if started.is_some_and(|start| start.elapsed() >= CLIENT_TIMEOUT) {
        return Err(io::ErrorKind::TimedOut.into());
    }
    if !input.contains(&b'\n') {
        let mut buf = [0; 1024];
        let limit = buf.len().min(MAX_FRAME + 1 - input.len());
        match stream.read(&mut buf[..limit]) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                started.get_or_insert_with(Instant::now);
                input.extend_from_slice(&buf[..n]);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(e) => return Err(e),
        }
    }
    if let Some(end) = input.iter().position(|b| *b == b'\n') {
        if end > MAX_FRAME {
            return Err(invalid("runtime frame exceeds MAX_FRAME"));
        }
        let response: Response = serde_json::from_slice(&input[..end])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if response.ipc_version != IPC_VERSION || response.request_id != request_id {
            return Err(invalid("response version/requestId mismatch"));
        }
        input.drain(..=end);
        if input.is_empty() {
            *started = None;
        }
        match (response.result, response.error) {
            (Some(event), None) => Ok(Some(event)),
            (None, Some(error)) => Err(io::Error::other(error)),
            _ => Err(invalid("expected exactly one runtime result or error")),
        }
    } else if input.len() > MAX_FRAME {
        Err(invalid("runtime frame exceeds MAX_FRAME"))
    } else {
        Ok(None)
    }
}

/// One events connection, owned by the calling worker. Dropping it only disconnects.
/// Construct inside that worker: the private Windows transport need not be `Send`.
pub struct ReadOnlyClient {
    stream: Option<transport::Stream>,
    request_id: String,
    input: Vec<u8>,
    partial_started: Option<Instant>,
    snapshot: Snapshot,
}

impl ReadOnlyClient {
    /// Connect and consume a validated snapshot within the bounded startup deadline.
    /// No runtime is started, no status connection is opened, and no thread is spawned.
    pub fn connect(instance: &str) -> io::Result<Self> {
        let started = Instant::now();
        let mut stream = transport::connect(&endpoint(instance)?)?;
        let request_id = format!("readonly-{}", std::process::id());
        let request = frame(&json!({
            "ipcVersion": IPC_VERSION, "requestId": request_id, "command": "events"
        }));
        let mut sent = 0;
        while sent < request.len() {
            if started.elapsed() >= CLIENT_TIMEOUT {
                return Err(io::ErrorKind::TimedOut.into());
            }
            match stream.write(&request[sent..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => sent += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
            if sent < request.len() {
                thread::sleep(TICK);
            }
        }
        let mut input = Vec::new();
        let mut partial_started = None;
        loop {
            if started.elapsed() >= CLIENT_TIMEOUT {
                return Err(io::ErrorKind::TimedOut.into());
            }
            match read_event(&mut stream, &mut input, &mut partial_started, &request_id)? {
                Some(RuntimeEvent::Snapshot(snapshot))
                    if snapshot.sequence == 1 && snapshot.lifecycle == Lifecycle::Running =>
                {
                    return Ok(Self {
                        stream: Some(stream),
                        request_id,
                        input,
                        partial_started,
                        snapshot,
                    });
                }
                Some(_) => return Err(invalid("expected initial running snapshot at sequence 1")),
                None => thread::sleep(TICK),
            }
        }
    }

    /// The snapshot consumed by connect, including honest unavailable audio status.
    pub fn initial_snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Never sleeps or waits for bytes. Idle is `Ok(None)`; partial frames expire.
    /// Stopping is delivered once and closes the stream. Later polls return
    /// `NotConnected`. Any protocol/I/O error also closes the stream immediately.
    pub fn poll(&mut self) -> io::Result<Option<RuntimeEvent>> {
        let stream = self.stream.as_mut().ok_or(io::ErrorKind::NotConnected)?;
        let result = read_event(
            stream,
            &mut self.input,
            &mut self.partial_started,
            &self.request_id,
        )
        .and_then(|event| match event {
            None => Ok(None),
            Some(RuntimeEvent::Lifecycle(ref lifecycle))
                if lifecycle.sequence == 2
                    && lifecycle.lifecycle == Lifecycle::Stopping
                    && self.input.is_empty() =>
            {
                Ok(event)
            }
            Some(_) => Err(invalid(
                "expected terminal stopping lifecycle at sequence 2",
            )),
        });
        if !matches!(result, Ok(None)) {
            self.stream.take();
        }
        result
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InventoryResponse {
    ipc_version: u32,
    request_id: String,
    #[serde(default, deserialize_with = "non_null")]
    result: Option<crate::Inventory>,
    #[serde(default, deserialize_with = "non_null")]
    error: Option<ServerError>,
}

impl ReadOnlyClient {
    /// One bounded, cancellable, read-only inventory connection. Does not start a
    /// runtime or substitute an empty inventory when the backend has none.
    pub fn inventory(
        instance: &str,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> io::Result<crate::Inventory> {
        use std::sync::atomic::Ordering;
        let start = Instant::now();
        let mut stream = transport::connect_cancellable(
            &endpoint(instance)?,
            cancelled,
            std::time::Duration::from_secs(2),
        )?;
        let request_id = format!("inventory-{}", std::process::id());
        let request =
            frame(&json!({"ipcVersion":IPC_VERSION,"requestId":request_id,"command":"inventory"}));
        let mut sent = 0;
        let mut input = Vec::new();
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if start.elapsed() >= std::time::Duration::from_secs(2) {
                return Err(io::ErrorKind::TimedOut.into());
            }
            if sent < request.len() {
                match stream.write(&request[sent..]) {
                    Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                    Ok(n) => sent += n,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }
            } else {
                let mut buf = [0; 65536];
                let limit = buf.len().min(super::MAX_INVENTORY_FRAME + 1 - input.len());
                match stream.read(&mut buf[..limit]) {
                    Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                    Ok(n) => input.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }
                if let Some(end) = input.iter().position(|b| *b == b'\n') {
                    if end > super::MAX_INVENTORY_FRAME || end + 1 != input.len() {
                        return Err(invalid("invalid inventory frame"));
                    }
                    let reply: InventoryResponse = serde_json::from_slice(&input[..end])
                        .map_err(|_| invalid("malformed inventory response"))?;
                    if reply.ipc_version != IPC_VERSION || reply.request_id != request_id {
                        return Err(invalid("inventory version/requestId mismatch"));
                    }
                    return match (reply.result, reply.error) {
                        (Some(inventory), None) => {
                            super::inventory::validate(&inventory).map_err(invalid)?;
                            if cancelled.load(Ordering::Acquire) {
                                return Err(io::ErrorKind::Interrupted.into());
                            }
                            if start.elapsed() >= std::time::Duration::from_secs(2) {
                                return Err(io::ErrorKind::TimedOut.into());
                            }
                            Ok(inventory)
                        }
                        (None, Some(error)) => Err(io::Error::other(error)),
                        _ => Err(invalid("expected exactly one inventory result or error")),
                    };
                }
                if input.len() > super::MAX_INVENTORY_FRAME {
                    return Err(invalid(
                        "inventory frame exceeds 1 MiB; reduce visible graph",
                    ));
                }
            }
            thread::sleep(TICK);
        }
    }
}
