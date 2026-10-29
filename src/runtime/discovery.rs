//! Read-only Linux discovery. No graph, device-default or audio operations.
use crate::{
    AudioFormat, BackendLimits, Evidence, Inventory, Observation, Origin, OutputObservation,
    Platform, Selector, Target,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, OpenOptionsExt},
            process::CommandExt,
        },
    },
    path::Path,
    process::{Child, ChildStderr, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};

const MAX_DUMP: usize = 4 * 1024 * 1024;
const MAX_OBJECTS: usize = 8192;
const MAX_TEXT: usize = super::inventory::MAX_ID_BYTES;
const CAPTURE_TIMEOUT: Duration = Duration::from_millis(700);
pub(super) type Result<T> = std::result::Result<T, &'static str>;

use super::inventory::{Capture, Options};

pub(super) struct Job {
    child: Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    output: Vec<u8>,
    started: Instant,
    simulated: bool,
}
impl Drop for Job {
    fn drop(&mut self) {
        // waitid(WNOWAIT) keeps the leader PID reserved until group cleanup.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}
fn nonblocking(fd: i32) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err("discovery-pipe-unavailable");
    }
    Ok(())
}
impl Job {
    pub fn start(options: &Options) -> Result<Self> {
        let path = options
            .fixture_helper
            .as_deref()
            .unwrap_or(Path::new("/usr/bin/pw-dump"));
        if !path.is_absolute() {
            return Err("discovery-helper-requires-absolute-local-path");
        }
        let mut command = Command::new(path);
        command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| {
            if e.kind() == io::ErrorKind::PermissionDenied {
                "discovery-permission-denied"
            } else {
                "discovery-helper-unavailable"
            }
        })?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let job = Self {
            child,
            stdout,
            stderr,
            output: Vec::new(),
            started: Instant::now(),
            simulated: options.fixture_helper.is_some(),
        };
        nonblocking(job.stdout.as_raw_fd())?;
        nonblocking(job.stderr.as_raw_fd())?;
        Ok(job)
    }
}
impl Capture for Job {
    fn poll(&mut self) -> Result<Option<Inventory>> {
        if self.started.elapsed() >= CAPTURE_TIMEOUT {
            return Err("discovery-timeout-700ms");
        }
        // Bound work per runtime tick; stderr is a partial-observation signal, never logged.
        let mut block = [0; 65536];
        let mut out_done = false;
        for _ in 0..32 {
            out_done = match self.stdout.read(&mut block) {
                Ok(0) => true,
                Ok(n) => {
                    if self.output.len() + n > MAX_DUMP {
                        return Err("discovery-output-exceeds-4MiB-reduce-visible-graph");
                    }
                    self.output.extend_from_slice(&block[..n]);
                    false
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => false,
                Err(_) => return Err("discovery-pipe-error"),
            };
            if out_done {
                break;
            }
        }
        let err_done = match self.stderr.read(&mut block) {
            Ok(0) => true,
            Ok(_) => return Err("discovery-diagnostic-output-or-partial-permission"),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => false,
            Err(_) => return Err("discovery-pipe-error"),
        };
        let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
        if unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id(),
                info.as_mut_ptr(),
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } != 0
        {
            return Err("discovery-wait-error");
        }
        let info = unsafe { info.assume_init() };
        if unsafe { info.si_pid() } != 0 {
            if info.si_code != libc::CLD_EXITED || unsafe { info.si_status() } != 0 {
                return Err("discovery-helper-failed-or-permission-denied");
            }
            if out_done && err_done {
                let inventory = decode(
                    &self.output,
                    if self.simulated {
                        Evidence::Simulated
                    } else {
                        Evidence::NativeDiscovery
                    },
                )?;
                if self.started.elapsed() >= Duration::from_millis(1000) {
                    return Err("discovery-validation-deadline");
                }
                return Ok(Some(inventory));
            }
        }
        Ok(None)
    }
}

// Value's usual decoder accepts duplicate keys. Native observations must not.
struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Unique;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> std::result::Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Unique, E> {
                Ok(Unique(serde_json::json!(v)))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut out = Vec::new();
                while let Some(Unique(v)) = a.next_element()? {
                    out.push(v);
                }
                Ok(Unique(Value::Array(out)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut out = Map::new();
                while let Some((k, Unique(v))) = a.next_entry::<String, Unique>()? {
                    if out.insert(k, v).is_some() {
                        return Err(serde::de::Error::custom("duplicate key"));
                    }
                }
                Ok(Unique(Value::Object(out)))
            }
        }
        d.deserialize_any(Visitor)
    }
}
fn identity(v: &Value) -> Result<String> {
    let text = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.as_i64().ok_or("discovery-lossy-identity")?.to_string(),
        _ => return Err("discovery-invalid-identity"),
    };
    if text.trim().is_empty() || text.len() > MAX_TEXT || text.chars().any(char::is_control) {
        return Err("discovery-identity-exceeds-4096-bytes-or-invalid");
    }
    Ok(text)
}
fn object_id(v: &Value) -> Result<u32> {
    let n = v.as_u64().or_else(|| {
        v.as_str()
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse().ok())
    });
    n.and_then(|n| u32::try_from(n).ok())
        .ok_or("discovery-invalid-object-id")
}
fn props(v: &Value) -> Result<&Map<String, Value>> {
    v.get("info")
        .and_then(|v| v.get("props"))
        .and_then(Value::as_object)
        .ok_or("discovery-incomplete-info-props")
}
fn is(v: &Value, kind: &str) -> bool {
    v["type"]
        .as_str()
        .is_some_and(|s| s.strip_prefix("PipeWire:Interface:") == Some(kind))
}
fn boolean(p: &Map<String, Value>, key: &str) -> Result<bool> {
    match p.get(key) {
        None | Some(Value::Bool(false)) => Ok(false),
        Some(Value::Bool(true)) => Ok(true),
        Some(Value::String(s)) if s == "true" => Ok(true),
        Some(Value::String(s)) if s == "false" => Ok(false),
        _ => Err("discovery-invalid-boolean"),
    }
}
fn format(node: &Value) -> Result<Option<AudioFormat>> {
    let Some(params) = node["info"].get("params") else {
        return Ok(None);
    };
    let params = params.as_object().ok_or("discovery-malformed-params")?;
    let Some(formats) = params.get("Format") else {
        return Ok(None);
    };
    let formats = formats.as_array().ok_or("discovery-malformed-format")?;
    if formats.is_empty() {
        return Ok(None);
    }
    if formats.len() != 1 || !formats[0].is_object() {
        return Err("discovery-ambiguous-current-format");
    }
    let f = &formats[0];
    if f["mediaType"] != "audio" || f["mediaSubtype"] != "raw" {
        return Ok(None);
    }
    let rate = f["rate"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0);
    let channels = match (f["channels"].as_u64(), f.get("position")) {
        (Some(1), Some(v)) if *v == serde_json::json!(["MONO"]) => 1,
        (Some(2), Some(v)) if *v == serde_json::json!(["FL", "FR"]) => 2,
        _ => return Ok(None), // Do not flatten AUX/surround/reversed/unknown layouts into stereo.
    };
    Ok(rate.map(|sample_rate_hz| AudioFormat {
        channels,
        sample_rate_hz,
    }))
}
fn decode(bytes: &[u8], evidence: Evidence) -> Result<Inventory> {
    if bytes.len() > MAX_DUMP {
        return Err("discovery-output-limit");
    }
    let Unique(raw) =
        serde_json::from_slice(bytes).map_err(|_| "discovery-malformed-json-or-duplicate-key")?;
    let objects = raw.as_array().ok_or("discovery-expected-array")?;
    if objects.len() > MAX_OBJECTS {
        return Err("discovery-exceeds-8192-objects-reduce-visible-graph");
    }
    let mut ids = BTreeSet::new();
    let mut core = false;
    let nodes: BTreeSet<_> = objects
        .iter()
        .filter(|v| is(v, "Node"))
        .map(|v| object_id(&v["id"]))
        .collect::<Result<_>>()?;
    for obj in objects {
        if !obj.is_object()
            || !obj["type"].is_string()
            || !obj["id"].is_u64()
            || !ids.insert(object_id(&obj["id"])?)
        {
            return Err("discovery-malformed-or-duplicate-object");
        }
        if is(obj, "Core") {
            if obj["id"] != 0
                || !obj["info"]["props"].is_object()
                || obj["info"]["cookie"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .is_none()
            {
                return Err("discovery-incomplete-core-info");
            }
            core = true;
        }
        if is(obj, "Node") || is(obj, "Device") || is(obj, "Port") {
            let p = props(obj)?;
            if is(obj, "Node") && obj["info"].get("state").is_some_and(|v| !v.is_string()) {
                return Err("discovery-malformed-node-state");
            }
            if is(obj, "Port") {
                let parent = object_id(p.get("node.id").ok_or("discovery-port-missing-node")?)?;
                if !nodes.contains(&parent) {
                    return Err("discovery-incomplete-port-reference");
                }
                for key in ["port.direction", "audio.channel"] {
                    if p.get(key).is_some_and(|v| !v.is_string()) {
                        return Err("discovery-malformed-port");
                    }
                }
            }
            for key in [
                "node.name",
                "device.serial",
                "application.id",
                "object.serial",
            ] {
                if let Some(v) = p.get(key) {
                    identity(v)?;
                }
            }
            for key in ["device.id", "node.id", "port.id"] {
                if let Some(v) = p.get(key) {
                    object_id(v)?;
                }
            }
            for key in ["node.virtual", "bluez5.loopback", "port.control"] {
                boolean(p, key)?;
            }
        }
    }
    if !core {
        return Err("discovery-incomplete-missing-core");
    }
    let mut inventory = Inventory {
        schema_version: 2,
        revision: 0,
        platform: Platform::Linux,
        evidence,
        sources: vec![],
        outputs: vec![],
        media_sessions: vec![],
        limits: BackendLimits::default(),
    };
    for obj in objects.iter().filter(|v| is(v, "Node")) {
        let p = props(obj)?;
        let class = match p.get("media.class") {
            Some(Value::String(class)) => class.as_str(),
            None if p
                .get("factory.name")
                .is_some_and(|v| v == "support.node.driver") =>
            {
                continue
            }
            _ => return Err("discovery-incomplete-media-class"),
        };
        if !matches!(class, "Audio/Source" | "Audio/Sink" | "Stream/Output/Audio") {
            continue;
        }
        if obj["info"]["state"] == "error" || boolean(p, "bluez5.loopback")? {
            return Err("discovery-error-or-profile-switch-placeholder");
        }
        if let Some(id) = p.get("device.id") {
            let id = object_id(id)?;
            if !objects
                .iter()
                .any(|v| is(v, "Device") && object_id(&v["id"]) == Ok(id))
            {
                return Err("discovery-incomplete-device-reference");
            }
        }
        let name = identity(
            p.get("node.name")
                .ok_or("discovery-missing-node-identity")?,
        )?;
        let serial = p
            .get("object.serial")
            .map(identity)
            .transpose()?
            .unwrap_or_else(|| "unknown".into());
        let device = p
            .get("device.id")
            .and_then(|v| object_id(v).ok())
            .and_then(|id| {
                objects
                    .iter()
                    .find(|v| is(v, "Device") && object_id(&v["id"]) == Ok(id))
            });
        let device_serial = p
            .get("device.serial")
            .or_else(|| device.and_then(|d| d["info"]["props"].get("device.serial")))
            .map(identity)
            .transpose()?;
        // Bind identity and native Format detail which schema2 cannot represent
        // (sample encoding, nonstandard positions). Such changes revoke old reviews.
        let node_id = object_id(&obj["id"])?;
        let mut ports: Vec<_> = objects
            .iter()
            .filter(|v| is(v, "Port") && object_id(&v["info"]["props"]["node.id"]) == Ok(node_id))
            .map(|v| {
                serde_json::json!([
                    v["id"],
                    v["info"]["props"]["port.id"],
                    v["info"]["props"]["port.direction"],
                    v["info"]["props"]["audio.channel"],
                    v["info"]["props"]["port.control"],
                    v["info"]["props"]["object.serial"]
                ])
            })
            .collect();
        ports.sort_by_key(|v| v[0].as_u64());
        let native_detail = serde_json::json!([
            name,
            device_serial,
            ports,
            obj["info"]["params"].get("Format"),
            objects
                .iter()
                .find(|v| is(v, "Core"))
                .map(|v| &v["info"]["cookie"])
        ]);
        let fingerprint = Sha256::digest(
            serde_json::to_vec(&native_detail).map_err(|_| "discovery-identity-serialization")?,
        );
        let fingerprint: String = fingerprint.iter().map(|b| format!("{b:02x}")).collect();
        let observed_id = format!(
            "pipewire:object:{}:{serial}:{fingerprint}",
            object_id(&obj["id"])?
        );
        let uid = format!("pipewire:node.name:{name}");
        if uid.len() > super::inventory::MAX_ID_BYTES
            || observed_id.len() > super::inventory::MAX_ID_BYTES
        {
            return Err("discovery-semantic-id-exceeds-4096-bytes");
        }
        let owned = name.starts_with("mixyard.") || p.contains_key("mixyard.owner");
        let format = format(obj)?;
        if class == "Audio/Sink" {
            // Mixyard producers must never be admitted as physical output targets.
            if owned {
                return Err("discovery-owned-sink-not-representable");
            }
            inventory.outputs.push(OutputObservation {
                observed_id,
                target: Target::Device { device_uid: uid },
                format,
            });
        } else {
            let selector = if class == "Audio/Source" {
                Selector::Mic { device_uid: uid }
            } else {
                if !p.get("media.role").is_some_and(|v| {
                    v == "Music"
                        || v == "Movie"
                        || v == "Game"
                        || v == "Notification"
                        || v == "Production"
                        || v == "Accessibility"
                        || v == "Animation"
                        || v == "Event"
                        || v == "Test"
                }) {
                    return Err("discovery-stream-role-needs-trusted-call-return-group");
                }
                Selector::App {
                    application_id: identity(
                        p.get("application.id")
                            .ok_or("discovery-stream-missing-application-id")?,
                    )?,
                }
            };
            inventory.sources.push(Observation {
                observed_id,
                selector,
                origin: if owned {
                    Origin::MixyardProducer
                } else {
                    Origin::External
                },
                format,
            });
        }
        if inventory.sources.len() > super::inventory::MAX_ENTRIES
            || inventory.outputs.len() > super::inventory::MAX_ENTRIES
        {
            return Err("discovery-exceeds-1024-endpoints-per-direction-reduce-visible-graph");
        }
    }
    inventory
        .sources
        .sort_by(|a, b| a.observed_id.cmp(&b.observed_id));
    inventory
        .outputs
        .sort_by(|a, b| a.observed_id.cmp(&b.observed_id));
    Ok(inventory)
}

/// Digest only: never persist raw observations or application metadata.
pub(super) fn revision(inventory: &mut Inventory, directory: &Path) -> Result<()> {
    inventory.revision = 0;
    let bytes = serde_json::to_vec(inventory).map_err(|_| "inventory-serialization-failed")?;
    if bytes.len() > super::MAX_INVENTORY_FRAME - 256 {
        return Err("inventory-response-exceeds-1MiB-reduce-visible-graph");
    }
    let digest = Sha256::digest(&bytes);
    let path = directory.join("inventory-revision-v1");
    let previous = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
    {
        Ok(mut file) => {
            let meta = file.metadata().map_err(|_| "inventory-state-unavailable")?;
            if !meta.is_file()
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.mode() & 0o777 != 0o600
                || meta.len() != 80
                || meta.nlink() != 1
            {
                return Err("inventory-state-insecure-or-corrupt-do-not-reset");
            }
            let mut state = [0; 80];
            file.read_exact(&mut state)
                .map_err(|_| "inventory-state-corrupt-do-not-reset")?;
            if &state[..8] != b"MXYINV01" || Sha256::digest(&state[..48])[..] != state[48..] {
                return Err("inventory-state-corrupt-do-not-reset");
            }
            Some(state)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(_) => return Err("inventory-state-unavailable-do-not-reset"),
    };
    let old_revision = previous
        .as_ref()
        .map(|s| u64::from_be_bytes(s[8..16].try_into().unwrap()))
        .unwrap_or(0);
    if previous.is_some() && old_revision == 0 {
        return Err("inventory-state-corrupt-do-not-reset");
    }
    if previous.as_ref().is_some_and(|s| s[16..48] == digest[..]) {
        inventory.revision = old_revision;
        return Ok(());
    }
    let next = old_revision
        .checked_add(1)
        .ok_or("inventory-revision-exhausted")?;
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|_| "inventory-state-entropy-unavailable")?;
    let temporary = directory.join(format!(
        "inventory-revision-{:032x}.tmp",
        u128::from_be_bytes(random)
    ));
    let write = || -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let mut state = Vec::from(b"MXYINV01".as_slice());
        state.extend(next.to_be_bytes());
        state.extend(digest);
        let checksum = Sha256::digest(&state);
        state.extend(checksum);
        file.write_all(&state)?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;
        fs::File::open(directory)?.sync_all()
    };
    if write().is_err() {
        return Err("inventory-state-atomic-write-failed-do-not-reset");
    }
    inventory.revision = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn graph() -> Value {
        json!([
            {"id":0,"type":"PipeWire:Interface:Core","info":{"cookie":100,"props":{}}},
            {"id":10,"type":"PipeWire:Interface:Node","info":{"props":{"node.name":"mic","media.class":"Audio/Source"},"params":{"Format":[{"mediaType":"audio","mediaSubtype":"raw","rate":96000,"channels":2,"position":["FL","FR"]}]}}}
        ])
    }
    fn parse(g: &Value) -> Result<Inventory> {
        decode(&serde_json::to_vec(g).unwrap(), Evidence::Simulated)
    }
    #[test]
    fn native_formats_require_current_concrete_rate_and_semantic_channel_order() {
        let mut g = graph();
        assert_eq!(
            parse(&g).unwrap().sources[0].format.unwrap().sample_rate_hz,
            96000
        );
        for layout in [
            json!(["FR", "FL"]),
            json!(["AUX0", "AUX1"]),
            json!(["MONO"]),
            Value::Null,
        ] {
            g[1]["info"]["params"]["Format"][0]["position"] = layout;
            assert!(parse(&g).unwrap().sources[0].format.is_none());
        }
        g = graph();
        g[1]["info"]["params"]["Format"][0]["rate"] = json!([48000, 96000]);
        assert!(parse(&g).unwrap().sources[0].format.is_none());
        g = graph();
        g[1]["info"]["params"]["EnumFormat"] = g[1]["info"]["params"]["Format"].clone();
        g[1]["info"]["params"]["Format"] = json!([]);
        assert!(parse(&g).unwrap().sources[0].format.is_none());
    }
    #[test]
    fn exact_application_identity_duplicates_and_producer_origin_remain_visible() {
        let mut g = graph();
        g[1]["info"]["props"]["node.name"] = json!("mixyard.owned");
        assert_eq!(
            parse(&g).unwrap().sources[0].origin,
            Origin::MixyardProducer
        );
        g[1]["info"]["props"]["media.class"] = json!("Stream/Output/Audio");
        assert!(parse(&g).is_err());
        g[1]["info"]["props"]["application.id"] = json!(9007199254740993i64);
        g[1]["info"]["props"]["media.role"] = json!("Music");
        let mut duplicate = g[1].clone();
        duplicate["id"] = json!(11);
        g.as_array_mut().unwrap().push(duplicate);
        let inventory = parse(&g).unwrap();
        assert_eq!(inventory.sources.len(), 2);
        assert_eq!(
            inventory.sources[0].selector,
            Selector::App {
                application_id: "9007199254740993".into()
            }
        );
        assert_eq!(inventory.sources[0].selector, inventory.sources[1].selector);
        g[1]["info"]["props"]["media.role"] = json!("Communication");
        assert!(parse(&g).is_err());
        g[1]["info"]["props"]
            .as_object_mut()
            .unwrap()
            .remove("media.role");
        g[1]["info"]["props"]["application.id"] = json!(9007199254740993.0f64);
        assert!(parse(&g).is_err());
    }
    #[test]
    fn malformed_and_partial_native_graphs_cannot_claim_complete_inventory() {
        let mut g = graph();
        g[1]["info"]["props"]["device.id"] = json!(50);
        assert!(parse(&g).is_err());
        g = graph();
        g[1]["info"]["state"] = json!("error");
        assert!(parse(&g).is_err());
        g = graph();
        g[1]["info"]["props"]["bluez5.loopback"] = json!(true);
        assert!(parse(&g).is_err());
        g = graph();
        g[1]["info"]["params"]["Format"] = json!({"rate":48000});
        assert!(parse(&g).is_err());
        g = graph();
        let duplicate = g[1].clone();
        g.as_array_mut().unwrap().push(duplicate);
        assert!(parse(&g).is_err());
        g = graph();
        g.as_array_mut().unwrap().push(
            json!({"id":20,"type":"PipeWire:Interface:Port","info":{"props":{"node.id":999}}}),
        );
        assert!(parse(&g).is_err());
        assert!(decode(b"[]", Evidence::Simulated).is_err());
        assert!(decode(b"null", Evidence::Simulated).is_err());
        assert!(decode(
            b"[{\"id\":0,\"type\":\"PipeWire:Interface:Core\",\"x\":{\"a\":1,\"a\":2}}]",
            Evidence::Simulated
        )
        .is_err());
    }
    #[test]
    fn nonaudio_driver_does_not_hide_endpoints_and_identity_limits_never_truncate() {
        let mut g = graph();
        g.as_array_mut().unwrap().push(json!({"id":22,"type":"PipeWire:Interface:Node","info":{"props":{"node.name":"Dummy-Driver","factory.name":"support.node.driver"}}}));
        assert_eq!(parse(&g).unwrap().sources.len(), 1);
        g[1]["info"]["props"]["node.name"] = json!("n".repeat(4000));
        let inventory = parse(&g).unwrap();
        assert_eq!(
            inventory.sources[0].selector,
            Selector::Mic {
                device_uid: format!("pipewire:node.name:{}", "n".repeat(4000))
            }
        );
        g[1]["info"]["props"]["node.name"] = json!("n".repeat(4096));
        assert_eq!(
            parse(&g).unwrap_err(),
            "discovery-semantic-id-exceeds-4096-bytes"
        );
        g = graph();
        g[0]["info"] = Value::Null;
        assert!(parse(&g).is_err());
    }
}
