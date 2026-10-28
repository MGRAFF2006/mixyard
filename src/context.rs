//! Read-only local observations. The collector owns the monotonic clock and revision.
//! Imported fixtures never acquire native evidence. See docs/context.md for scopes.
use crate::automation::{self, *};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "linux")]
use std::time::Duration;
use std::time::Instant;

const MAX_OUTPUT: usize = 1024 * 1024;
#[cfg(target_os = "linux")]
const MAX_CONFIG: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct Options {
    /// Explicit exact executable path -> application identity mapping, never basenames.
    pub applications: BTreeMap<String, String>,
    pub host_label: Option<String>,
    pub timezone: String,
    /// Local trusted installation paths; never accept these from an HTTP request.
    pub helper: PathBuf,
    pub linux_adapter: PathBuf,
}
impl Default for Options {
    fn default() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        Self {
            applications: BTreeMap::new(),
            host_label: None,
            timezone: "Europe/Berlin".into(),
            helper: root.join("observers/linux.py"),
            linux_adapter: root.join("adapters/linux/mixyard_linux.py"),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderState {
    Complete,
    Partial,
    Unsupported,
    Error,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderReport {
    pub provider: String,
    pub status: ProviderState,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capture {
    pub snapshot: Snapshot,
    /// Capture completion time, in snapshot.epoch. Individual facts use capture start.
    pub now_ms: u64,
    pub providers: Vec<ProviderReport>,
    /// Exact configured application identities. Unmapped membership operands must
    /// remain unknown in a live adapter, even when the mapped set is complete.
    pub application_scope: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservationError(pub &'static str);
impl std::fmt::Display for ObservationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ObservationError {}

pub struct Collector {
    options: Options,
    start: Instant,
    epoch: String,
    revision: u64,
}
impl Collector {
    pub fn new(options: Options) -> Result<Self, ObservationError> {
        let text_ok = |s: &str| !s.trim().is_empty() && s.len() <= MAX_TEXT_BYTES;
        if !text_ok(&options.timezone)
            || options.host_label.as_deref().is_some_and(|s| !text_ok(s))
            || options.applications.len() > 32
            || options.applications.iter().any(|(path, id)| {
                !PathBuf::from(path).is_absolute() || !text_ok(path) || !text_ok(id)
            })
        {
            return Err(ObservationError("invalid-observer-options"));
        }
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(|_| ObservationError("epoch-entropy-unavailable"))?;
        let epoch = random.iter().map(|b| format!("{b:02x}")).collect();
        Ok(Self {
            options,
            start: Instant::now(),
            epoch,
            revision: 0,
        })
    }

    pub fn now_ms(&self) -> Result<u64, ObservationError> {
        u64::try_from(self.start.elapsed().as_millis())
            .map_err(|_| ObservationError("monotonic-clock-exhausted"))
    }

    /// No native input is accepted. Cancellation kills/reaps only this collector's children.
    pub fn capture(&mut self, cancelled: &AtomicBool) -> Result<Capture, ObservationError> {
        if cancelled.load(Ordering::Relaxed) {
            return Err(ObservationError("cancelled"));
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(ObservationError("snapshot-revision-exhausted"))?;
        let observed = self.now_ms()?;
        let mut facts = unsupported_globals();
        let native = if cfg!(target_os = "linux") {
            self.linux(cancelled)
        } else {
            Ok(Vec::new())
        };
        match native {
            Ok(native) => {
                for mut fact in native {
                    // This path exclusively consumes the owned local helper, not HTTP/imported JSON.
                    fact.observed_at_ms = Some(observed);
                    fact.valid_for_ms = fact.valid_for_ms.min(5_000);
                    facts.retain(|existing| existing.key != fact.key);
                    facts.push(fact);
                }
            }
            Err(ObservationError("cancelled")) => return Err(ObservationError("cancelled")),
            Err(error) => {
                for name in [
                    "application.running",
                    "power.source",
                    "power.battery-percent",
                    "civil.timezone",
                    "civil.date",
                    "civil.weekday",
                    "civil.minute-of-day",
                    "device.connected",
                ] {
                    facts.retain(|f| f.key.name != name);
                    facts.push(unavailable(
                        name,
                        "local.helper",
                        FactStatus::Error,
                        error.0,
                    ));
                }
            }
        }
        facts.retain(|f| f.key.name != "host.identity");
        facts.push(match &self.options.host_label {
            Some(label) => Fact {
                key: FactKey {
                    name: "host.identity".into(),
                    subject: None,
                },
                status: FactStatus::Known,
                value: Some(Value::Text(label.clone())),
                provider: "local.declaration".into(),
                evidence: Evidence::UserDeclared,
                observed_at_ms: Some(observed),
                valid_for_ms: 5_000,
                completeness: Completeness::Complete,
                reason: None,
            },
            None => unavailable(
                "host.identity",
                "local.declaration",
                FactStatus::Unknown,
                "explicit-local-host-label-required",
            ),
        });
        for id in self.options.applications.values().collect::<BTreeSet<_>>() {
            for name in [
                "application.fullscreen",
                "application.playback",
                "application.capture",
                "application.call-active",
            ] {
                let mut fact = unavailable(
                    name,
                    "unimplemented",
                    FactStatus::Unsupported,
                    "provider-not-implemented",
                );
                fact.key.subject = Some(id.clone());
                facts.push(fact);
            }
        }
        facts.sort_by(|a, b| a.key.cmp(&b.key));
        let snapshot = Snapshot {
            schema_version: 1,
            epoch: self.epoch.clone(),
            revision,
            facts,
        };
        let now_ms = self.now_ms()?;
        validate_snapshot(&snapshot, now_ms)?;
        let providers = reports(&snapshot.facts);
        self.revision = revision;
        Ok(Capture {
            snapshot,
            now_ms,
            providers,
            application_scope: self
                .options
                .applications
                .values()
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        })
    }

    #[cfg(target_os = "linux")]
    fn linux(&self, cancelled: &AtomicBool) -> Result<Vec<Fact>, ObservationError> {
        let config = serde_json::to_vec(&serde_json::json!({
            "applications": self.options.applications, "timezone": self.options.timezone,
        }))
        .map_err(|_| ObservationError("invalid-observer-options"))?;
        if config.len() > MAX_CONFIG {
            return Err(ObservationError("observer-options-size-limit"));
        }
        let mut command = std::process::Command::new("python3");
        command
            .arg("-I")
            .arg("-B")
            .arg(&self.options.helper)
            .arg("--adapter")
            .arg(&self.options.linux_adapter);
        let raw = child::run(command, &config, Duration::from_secs(4), cancelled)?;
        let facts: Vec<Fact> = serde_json::from_slice(&raw)
            .map_err(|_| ObservationError("malformed-helper-output"))?;
        let required = [
            ("application.running", "linux.proc"),
            ("power.source", "linux.sysfs"),
            ("power.battery-percent", "linux.sysfs"),
            ("civil.timezone", "civil.zoneinfo"),
            ("civil.date", "civil.zoneinfo"),
            ("civil.weekday", "civil.zoneinfo"),
            ("civil.minute-of-day", "civil.zoneinfo"),
            ("device.connected", "linux.pipewire"),
        ];
        if required.iter().any(|(name, provider)| {
            !facts
                .iter()
                .any(|f| f.key.name == *name && f.key.subject.is_none() && f.provider == *provider)
        }) {
            return Err(ObservationError("incomplete-helper-contract"));
        }
        // Reject values outside the local helper's scope and any declaration claim.
        if facts.len() > MAX_FACTS
            || facts.iter().any(|f| {
                !matches!(
                    f.provider.as_str(),
                    "linux.proc" | "linux.sysfs" | "linux.pipewire" | "civil.zoneinfo"
                ) || !matches!(f.evidence, Evidence::PlatformObserved | Evidence::Derived)
                    || (f.key.name == "device.hardware-monitor-declared"
                        && f.status == FactStatus::Known)
                    || !matches!(
                        f.key.name.as_str(),
                        "application.running"
                            | "power.source"
                            | "power.battery-percent"
                            | "civil.timezone"
                            | "civil.date"
                            | "civil.weekday"
                            | "civil.minute-of-day"
                            | "device.connected"
                            | "device.kind"
                            | "device.capabilities"
                            | "device.channels"
                            | "device.sample-rate-hz"
                            | "device.hardware-monitor-declared"
                    )
            })
        {
            return Err(ObservationError("invalid-helper-contract"));
        }
        // Invalid/oversized helper data becomes an explicit provider error capture.
        let probe = Snapshot {
            schema_version: 1,
            revision: 0,
            epoch: "probe".into(),
            facts: facts.clone(),
        };
        validate_snapshot(&probe, 0)?;
        Ok(facts)
    }
    #[cfg(not(target_os = "linux"))]
    fn linux(&self, _: &AtomicBool) -> Result<Vec<Fact>, ObservationError> {
        Ok(Vec::new())
    }
}

fn unavailable(name: &str, provider: &str, status: FactStatus, reason: &str) -> Fact {
    Fact {
        key: FactKey {
            name: name.into(),
            subject: None,
        },
        status,
        value: None,
        provider: provider.into(),
        evidence: Evidence::Derived,
        observed_at_ms: None,
        valid_for_ms: 0,
        completeness: Completeness::Partial,
        reason: Some(reason.into()),
    }
}
fn unsupported_globals() -> Vec<Fact> {
    automation::standard_catalog()
        .into_iter()
        .filter(|d| d.scope == Scope::Global)
        .map(|d| {
            unavailable(
                &d.name,
                "unimplemented",
                FactStatus::Unsupported,
                if cfg!(target_os = "linux") {
                    "provider-not-implemented"
                } else {
                    "platform-not-implemented"
                },
            )
        })
        .collect()
}
fn reports(facts: &[Fact]) -> Vec<ProviderReport> {
    let mut groups: BTreeMap<&str, Vec<&Fact>> = BTreeMap::new();
    for fact in facts {
        groups.entry(&fact.provider).or_default().push(fact);
    }
    groups
        .into_iter()
        .map(|(provider, facts)| {
            let status = if facts.iter().any(|f| f.status == FactStatus::Error) {
                ProviderState::Error
            } else if facts.iter().all(|f| f.status == FactStatus::Unsupported) {
                ProviderState::Unsupported
            } else if facts
                .iter()
                .all(|f| f.status == FactStatus::Known && f.completeness == Completeness::Complete)
            {
                ProviderState::Complete
            } else {
                ProviderState::Partial
            };
            let reasons: BTreeSet<_> = facts.iter().filter_map(|f| f.reason.as_deref()).collect();
            ProviderReport {
                provider: provider.into(),
                status,
                reason: if reasons.is_empty() {
                    None
                } else {
                    Some(reasons.into_iter().collect::<Vec<_>>().join("; "))
                },
            }
        })
        .collect()
}

fn validate_snapshot(snapshot: &Snapshot, now_ms: u64) -> Result<(), ObservationError> {
    // Share core type, scope, duplicate, text/set cardinality and aggregate work admission.
    let request = Request {
        graph: Graph {
            schema_version: 1,
            id: "observer-validation".into(),
            name: "Observer validation".into(),
            revision: 0,
            root: "end".into(),
            nodes: vec![Node::Terminal {
                id: "end".into(),
                action: Action::NoChange {},
            }],
        },
        catalog: standard_catalog(),
        snapshot: snapshot.clone(),
        now_ms,
        context: Context {
            config_revision: 0,
            store_revision: 0,
            inventory_revision: 0,
            manual_revision: 0,
        },
        manual_override: false,
    };
    automation::validate(&request)
        .map_err(|_| ObservationError("invalid-or-oversized-observer-facts"))
}

/// Strict bounded fixture import. Never call this to admit native or user-declared evidence.
/// Timestamps/epoch remain fixture coordinates; they are not the live collector's clock.
pub fn parse_fixture(bytes: &[u8]) -> Result<Snapshot, ObservationError> {
    if bytes.len() > MAX_OUTPUT {
        return Err(ObservationError("fixture-size-limit"));
    }
    let mut snapshot: Snapshot =
        serde_json::from_slice(bytes).map_err(|_| ObservationError("malformed-fixture"))?;
    for fact in &mut snapshot.facts {
        fact.evidence = Evidence::Fixture;
    }
    let now = snapshot
        .facts
        .iter()
        .filter_map(|f| f.observed_at_ms)
        .max()
        .unwrap_or(0);
    validate_snapshot(&snapshot, now)?;
    Ok(snapshot)
}

#[cfg(target_os = "linux")]
mod child {
    use super::*;
    use std::io::{Read, Write};
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    use std::process::{Child, Command, Stdio};
    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            // A separate process group contains only our helper and its subprocesses.
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            let _ = self.0.wait();
        }
    }
    fn nonblocking(fd: i32) -> Result<(), ObservationError> {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(ObservationError("child-pipe-unavailable"));
        }
        Ok(())
    }
    fn drain(
        pipe: &mut impl Read,
        output: Option<&mut Vec<u8>>,
        count: &mut usize,
    ) -> Result<bool, ObservationError> {
        let mut block = [0; 8192];
        let mut output = output;
        loop {
            match pipe.read(&mut block) {
                Ok(0) => return Ok(true),
                Ok(n) => {
                    *count += n;
                    if *count > MAX_OUTPUT {
                        return Err(ObservationError("child-output-limit"));
                    }
                    if let Some(out) = output.as_deref_mut() {
                        out.extend_from_slice(&block[..n]);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(ObservationError("child-pipe-error")),
            }
        }
    }
    pub(super) fn run(
        mut command: Command,
        input: &[u8],
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<Vec<u8>, ObservationError> {
        command
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = OwnedChild(
            command
                .spawn()
                .map_err(|_| ObservationError("child-tool-unavailable"))?,
        );
        let mut stdin = child.0.stdin.take();
        let mut stdout = child
            .0
            .stdout
            .take()
            .ok_or(ObservationError("child-pipe-unavailable"))?;
        let mut stderr = child
            .0
            .stderr
            .take()
            .ok_or(ObservationError("child-pipe-unavailable"))?;
        nonblocking(
            stdin
                .as_ref()
                .ok_or(ObservationError("child-pipe-unavailable"))?
                .as_raw_fd(),
        )?;
        nonblocking(stdout.as_raw_fd())?;
        nonblocking(stderr.as_raw_fd())?;
        let start = Instant::now();
        let mut output = Vec::new();
        let mut count = 0;
        let mut sent = 0;
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(ObservationError("cancelled"));
            }
            if start.elapsed() >= timeout {
                return Err(ObservationError("child-timeout"));
            }
            if let Some(pipe) = &mut stdin {
                match pipe.write(&input[sent..]) {
                    Ok(n) => sent += n,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(_) => return Err(ObservationError("child-input-error")),
                }
                if sent == input.len() {
                    stdin = None;
                }
            }
            let out_done = drain(&mut stdout, Some(&mut output), &mut count)?;
            let err_done = drain(&mut stderr, None, &mut count)?;
            // Observe exit without reaping: the leader PID must remain reserved
            // until Drop kills its group, so cleanup cannot target a reused PID.
            let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
            if unsafe {
                libc::waitid(
                    libc::P_PID,
                    child.0.id(),
                    info.as_mut_ptr(),
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            } != 0
            {
                return Err(ObservationError("child-wait-error"));
            }
            let info = unsafe { info.assume_init() };
            if unsafe { info.si_pid() } != 0 {
                if info.si_code != libc::CLD_EXITED || unsafe { info.si_status() } != 0 {
                    return Err(ObservationError("child-failed"));
                }
                if out_done && err_done {
                    return Ok(output);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exhausted_revision_cannot_wrap_or_run_a_provider() {
        let mut collector = Collector::new(Options::default()).unwrap();
        collector.revision = u64::MAX;
        assert_eq!(
            collector.capture(&AtomicBool::new(false)).unwrap_err().0,
            "snapshot-revision-exhausted"
        );
        assert_eq!(collector.revision, u64::MAX);
    }
    #[test]
    fn partial_or_ambiguous_facts_never_report_a_complete_provider() {
        let mut fact = unavailable(
            "device.connected",
            "fixture",
            FactStatus::Ambiguous,
            "duplicate",
        );
        assert_eq!(reports(&[fact.clone()])[0].status, ProviderState::Partial);
        fact.status = FactStatus::Known;
        fact.value = Some(Value::TextSet(vec![]));
        assert_eq!(reports(&[fact])[0].status, ProviderState::Partial);
    }
}
