//! Independent lifecycle IPC. This module does not own or dispatch audio.
pub mod client;
#[cfg(target_os = "linux")]
pub mod discovery;
pub mod inventory;
pub use client::{
    Availability, Backend, Capabilities, Lifecycle, LifecycleEvent, NativeAudioProof,
    ReadOnlyClient, RuntimeEvent, ServerError, Snapshot,
};

use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::{self, Read, Write},
    thread,
    time::{Duration, Instant},
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[path = "unix.rs"]
mod transport;
#[cfg(windows)]
#[path = "windows.rs"]
mod transport;
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
compile_error!("The lifecycle runtime currently supports Linux, macOS and Windows only");

pub const IPC_VERSION: u32 = 1;
pub const MAX_FRAME: usize = 8192;
/// Inventory has separate bounded admission; requests/events keep the 8 KiB limit.
pub const MAX_INVENTORY_FRAME: usize = 1024 * 1024;
const MAX_CLIENTS: usize = 16;
const MAX_EVENTS: usize = 8;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
const TICK: Duration = Duration::from_millis(10);
const INVENTORY_TRANSFER_TICK: Duration = Duration::from_millis(1);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    ipc_version: u32,
    request_id: String,
    command: String,
    #[serde(default)]
    after_sequence: Option<u64>,
}

pub fn endpoint(instance: &str) -> io::Result<String> {
    if instance.is_empty()
        || instance.len() > 32
        || !instance
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "instance must contain 1..32 ASCII letters, digits or hyphens",
        ));
    }
    transport::endpoint(instance)
}

fn status() -> Value {
    json!({"daemonReachable":true,"backendAvailable":false,"backend":"unavailable",
        "audioRevision":null,"appliedRevision":null,"desiredRevision":null,
        "nativeAudioProof":"unverified","capabilities":{"apply":false,"mute":false,"nativeActionDispatch":false}})
}
fn envelope(id: Value, result: Value) -> Value {
    json!({"ipcVersion":IPC_VERSION,"requestId":id,"result":result})
}
fn error(id: Value, code: &str, message: &str) -> Value {
    json!({"ipcVersion":IPC_VERSION,"requestId":id,"error":{"code":code,"message":message}})
}
fn frame(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).expect("JSON values serialize");
    bytes.push(b'\n');
    bytes
}

struct Peer {
    stream: transport::Stream,
    input: Vec<u8>,
    // ponytail: at most two frames (snapshot then shutdown), no unbounded event history.
    output: Vec<u8>,
    written: usize,
    started: Instant,
    responding: bool,
    events: bool,
    id: Value,
    dead: bool,
    discovery: Option<Box<dyn inventory::Capture>>,
    inventory: bool,
}
impl Peer {
    fn new(stream: transport::Stream) -> Self {
        Self {
            stream,
            input: Vec::new(),
            output: Vec::new(),
            written: 0,
            started: Instant::now(),
            responding: false,
            events: false,
            id: Value::Null,
            dead: false,
            discovery: None,
            inventory: false,
        }
    }
    fn reply(&mut self, value: Value) {
        self.output.extend(frame(&value));
        self.responding = true;
        self.started = Instant::now();
    }
    fn read_request(&mut self) -> io::Result<Option<Value>> {
        let mut buf = [0; 1024];
        // One bounded read per tick, so one noisy client cannot monopolize the loop.
        match self.stream.read(&mut buf) {
            Ok(0) => Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                if self.events {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                self.input.extend_from_slice(&buf[..n]);
                if let Some(end) = self.input.iter().position(|b| *b == b'\n') {
                    if end > MAX_FRAME {
                        return Ok(Some(error(
                            Value::Null,
                            "frame-too-large",
                            "request exceeds 8192 bytes",
                        )));
                    }
                    let raw: Value = match serde_json::from_slice(&self.input[..end]) {
                        Ok(v) => v,
                        Err(_) => {
                            return Ok(Some(error(
                                Value::Null,
                                "malformed-request",
                                "expected one newline-delimited JSON object",
                            )))
                        }
                    };
                    let id = raw
                        .get("requestId")
                        .filter(|v| v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 64))
                        .cloned()
                        .unwrap_or(Value::Null);
                    self.id = id.clone();
                    if raw.get("ipcVersion").and_then(Value::as_u64) != Some(IPC_VERSION as u64) {
                        return Ok(Some(error(
                            id,
                            "unsupported-version",
                            "ipcVersion must be 1 (independent of policy schemaVersion)",
                        )));
                    }
                    let req: Request = match serde_json::from_value(raw) {
                        Ok(req) if id != Value::Null => req,
                        _ => {
                            return Ok(Some(error(
                                id,
                                "malformed-request",
                                "invalid request fields or requestId (1..64 bytes)",
                            )))
                        }
                    };
                    debug_assert_eq!(req.ipc_version, IPC_VERSION);
                    debug_assert_eq!(json!(req.request_id), id);
                    if req.command != "events" && req.after_sequence.is_some() {
                        return Ok(Some(error(
                            id,
                            "malformed-request",
                            "afterSequence is only valid for events",
                        )));
                    }
                    // Return an internal command object, never an audio dispatch.
                    Ok(Some(
                        json!({"command":req.command,"afterSequence":req.after_sequence}),
                    ))
                } else if self.input.len() > MAX_FRAME {
                    Ok(Some(error(
                        Value::Null,
                        "frame-too-large",
                        "request exceeds 8192 bytes",
                    )))
                } else {
                    Ok(None)
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
        }
    }
    fn flush(&mut self) -> io::Result<bool> {
        let budget = if self.inventory { 65536 } else { 4096 };
        let end = (self.written + budget).min(self.output.len());
        // Bound bytes and calls per peer, while allowing short OS writes to
        // make progress without imposing a full scheduler tick per write.
        for _ in 0..32 {
            if self.written == end {
                break;
            }
            match self.stream.write(&self.output[self.written..end]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => self.written += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        if self.written == self.output.len() {
            self.output.clear();
            self.written = 0;
            return Ok(true);
        }
        Ok(false)
    }
}

/// Foreground process, explicit lifetime; does not autostart from any client.
pub fn serve(instance: &str) -> io::Result<()> {
    serve_with_inventory(instance, inventory::Options::default())
}

pub fn serve_with_inventory(instance: &str, options: inventory::Options) -> io::Result<()> {
    let mut listener = transport::Listener::bind(&endpoint(instance)?)?;
    let mut peers: Vec<Peer> = Vec::new();
    let mut stopping: Option<Instant> = None;
    loop {
        if stopping.is_none() && peers.len() < MAX_CLIENTS {
            if let Some(stream) = listener.accept()? {
                peers.push(Peer::new(stream));
            }
        }
        let mut event_count = peers.iter().filter(|p| p.events).count();
        let mut discovering = peers.iter().any(|p| p.discovery.is_some());
        for peer in &mut peers {
            if let Some(job) = &mut peer.discovery {
                let mut byte = [0; 1];
                let connected = matches!(peer.stream.read(&mut byte), Err(e) if e.kind() == io::ErrorKind::WouldBlock);
                if !connected || stopping.is_some() {
                    peer.discovery = None;
                    peer.dead = true;
                    continue;
                }
                let result = if peer.started.elapsed() >= Duration::from_millis(1200) {
                    Err("inventory-total-preparation-deadline-1200ms")
                } else {
                    job.poll()
                }
                .and_then(|inventory| match inventory {
                    Some(mut inventory) => {
                        #[cfg(target_os = "linux")]
                        listener
                            .owned_directory()
                            .map_err(|_| "inventory-namespace-ownership-changed")?;
                        inventory::complete(instance, &mut inventory)?;
                        Ok(Some(
                            serde_json::to_value(inventory)
                                .map_err(|_| "inventory-serialization-failed")?,
                        ))
                    }
                    None => Ok(None),
                });
                let result = if peer.started.elapsed() >= Duration::from_millis(1200) {
                    Err("inventory-total-preparation-deadline-1200ms")
                } else {
                    result
                };
                match result {
                    Ok(None) => continue,
                    Ok(Some(value)) => peer.reply(envelope(peer.id.clone(), value)),
                    Err(reason) => {
                        peer.reply(error(peer.id.clone(), "inventory-unavailable", reason))
                    }
                }
                peer.discovery = None;
                // The next tick can admit another discovery after the previous child is reaped.
                continue;
            }
            if peer.responding && !peer.events {
                // Keep the endpoint alive until the client consumes its reply and
                // disconnects, or the deadline. Windows DisconnectNamedPipe drops
                // unread bytes, so write completion alone is insufficient.
                let mut buf = [0; 1024];
                match peer.stream.read(&mut buf) {
                    Ok(0) => peer.dead = true,
                    Err(e) if e.kind() != io::ErrorKind::WouldBlock => peer.dead = true,
                    _ => {}
                }
                continue;
            }
            match peer.read_request() {
                Ok(Some(command)) if stopping.is_none() => {
                    let reply = match command.get("command").and_then(Value::as_str) {
                        Some("status") => envelope(peer.id.clone(), status()),
                        Some("inventory") => {
                            peer.inventory = true;
                            if discovering {
                                error(
                                    peer.id.clone(),
                                    "busy",
                                    "one inventory discovery at a time; retry after completion",
                                )
                            } else {
                                match inventory::start(&options) {
                                    Ok(job) => {
                                        peer.discovery = Some(job);
                                        discovering = true;
                                        continue;
                                    }
                                    Err(reason) => {
                                        error(peer.id.clone(), "inventory-unavailable", reason)
                                    }
                                }
                            }
                        }
                        Some("shutdown") => {
                            stopping = Some(Instant::now());
                            envelope(peer.id.clone(), json!({"shutdown":"requested"}))
                        }
                        Some("events") if event_count < MAX_EVENTS => {
                            event_count += 1;
                            peer.events = true;
                            envelope(
                                peer.id.clone(),
                                json!({"event":"snapshot","sequence":1,"resync":true,
                                "lifecycle":"running","availability":status(),"historyAvailable":false}),
                            )
                        }
                        Some("events") => error(
                            peer.id.clone(),
                            "busy",
                            "at most 8 event subscribers; retry later",
                        ),
                        Some("apply" | "mute" | "native-action") => error(
                            peer.id.clone(),
                            "backend-unavailable",
                            "native audio and action dispatch are unavailable",
                        ),
                        Some(_) => error(
                            peer.id.clone(),
                            "unsupported-command",
                            "implemented commands: status, events, inventory, shutdown",
                        ),
                        None => command,
                    };
                    peer.reply(reply);
                }
                Ok(_) => {}
                Err(_) => {
                    peer.dead = true;
                }
            }
            if !peer.responding && peer.started.elapsed() >= REQUEST_TIMEOUT {
                peer.reply(error(
                    Value::Null,
                    "request-timeout",
                    "complete a request within 2 seconds",
                ));
            }
        }
        if stopping.is_some() {
            for peer in &mut peers {
                if peer.events && !peer.dead {
                    peer.reply(envelope(peer.id.clone(),json!({"event":"lifecycle","sequence":2,"lifecycle":"stopping","backendAvailable":false})));
                    peer.events = false;
                }
            }
        }
        peers.retain_mut(|peer| {
            if peer.dead {
                false
            } else {
                match peer.flush() {
                    Ok(done) => {
                        if peer.responding
                            && !peer.events
                            && peer.started.elapsed() >= REQUEST_TIMEOUT
                        {
                            return false;
                        }
                        if !done && peer.started.elapsed() >= REQUEST_TIMEOUT {
                            return false;
                        }
                        true
                    }
                    Err(_) => false,
                }
            }
        });
        if let Some(start) = stopping {
            if peers.is_empty() || start.elapsed() >= REQUEST_TIMEOUT {
                return Ok(());
            }
        }
        thread::sleep(
            if peers
                .iter()
                .any(|peer| peer.inventory && peer.written < peer.output.len())
            {
                INVENTORY_TRANSFER_TICK
            } else {
                TICK
            },
        );
    }
}

/// One explicit connection. Events remain attached until daemon shutdown or caller exit.
pub fn client(instance: &str, command: &str, output: &mut impl Write) -> io::Result<()> {
    let mut stream = transport::connect(&endpoint(instance)?)?;
    let request_id = format!("cli-{}", std::process::id());
    let request =
        frame(&json!({"ipcVersion":IPC_VERSION,"requestId":request_id,"command":command}));
    let start = Instant::now();
    let mut sent = 0;
    while sent < request.len() {
        match stream.write(&request[sent..]) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => sent += n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        if start.elapsed() >= CLIENT_TIMEOUT {
            return Err(io::ErrorKind::TimedOut.into());
        }
        if sent < request.len() {
            thread::sleep(TICK);
        }
    }
    let mut input = Vec::new();
    let mut received = false;
    loop {
        let mut buf = [0; 65536];
        let max_frame = if command == "inventory" {
            MAX_INVENTORY_FRAME
        } else {
            MAX_FRAME
        };
        match stream.read(&mut buf) {
            Ok(0) if received && command == "events" => return Ok(()),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                input.extend_from_slice(&buf[..n]);
                while let Some(end) = input.iter().position(|b| *b == b'\n') {
                    if end > max_frame {
                        return Err(io::ErrorKind::InvalidData.into());
                    }
                    let reply: Value = serde_json::from_slice(&input[..end])?;
                    if reply["ipcVersion"] != IPC_VERSION || reply["requestId"] != request_id {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "response version/requestId mismatch",
                        ));
                    }
                    output.write_all(&input[..=end])?;
                    output.flush()?;
                    input.drain(..=end);
                    received = true;
                    if reply.get("error").is_some() {
                        return Err(io::Error::other(
                            "runtime rejected request (see JSON response)",
                        ));
                    }
                    // The terminal event completes the subscription; close before
                    // Windows disconnects the pipe at the server's drain deadline.
                    if command != "events"
                        || (reply["result"]["event"] == "lifecycle"
                            && reply["result"]["lifecycle"] == "stopping")
                    {
                        return Ok(());
                    }
                }
                if input.len() > max_frame {
                    return Err(io::ErrorKind::InvalidData.into());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        if !received && start.elapsed() >= CLIENT_TIMEOUT {
            return Err(io::ErrorKind::TimedOut.into());
        }
        thread::sleep(TICK);
    }
}
