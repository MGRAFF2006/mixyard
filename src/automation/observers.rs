//! Read-only provider composition and revocable observation attachment.
//! The existing Controller and profile writer retain transition/write authority.
use super::{
    settings::Declaration, Completeness, Context, Controller, Error, Evidence, Fact, FactKey,
    FactStatus, StateToken, TransitionCandidate, Value, MAX_FACTS, MAX_TEXT_BYTES,
};
use crate::context::{
    Capture, Collector, ObservationError, Options, ProviderReport, ProviderState,
};
use std::{collections::BTreeSet, path::Path, sync::atomic::AtomicBool, sync::Arc};

const FRESH_MS: u64 = 5_000;

/// Trusted local boundary, never implemented by accepting client-submitted facts.
/// Timestamps and now_ms must share one monotonic epoch; restart uses a fresh epoch.
/// Providers must bound their work and honor cancellation. Injection alone is not
/// evidence that an OS implementation is qualified.
pub trait Provider {
    fn capture(&mut self, cancelled: &AtomicBool) -> Result<Capture, ObservationError>;
    fn now_ms(&self) -> Result<u64, ObservationError>;
}

impl Provider for Collector {
    fn capture(&mut self, cancelled: &AtomicBool) -> Result<Capture, ObservationError> {
        Collector::capture(self, cancelled)
    }
    fn now_ms(&self) -> Result<u64, ObservationError> {
        Collector::now_ms(self)
    }
}

/// Reuses the existing private-helper collector for device/application/power/time.
/// Adds Linux link metadata only; all other OS network providers remain unsupported.
pub struct NativeProvider {
    collector: Collector,
}

impl NativeProvider {
    pub fn new(options: Options) -> Result<Self, ObservationError> {
        Ok(Self {
            collector: Collector::new(options)?,
        })
    }
}

impl Provider for NativeProvider {
    fn capture(&mut self, cancelled: &AtomicBool) -> Result<Capture, ObservationError> {
        let mut capture = self.collector.capture(cancelled)?;
        let observed = self.collector.now_ms()?;
        let result = if cfg!(target_os = "linux") {
            Some(linux_link_state(Path::new("/sys/class/net")))
        } else {
            None
        };
        let (provider, status, value, completeness, reason) = match result {
            Some(Ok(connected)) => (
                "linux.net-sysfs",
                FactStatus::Known,
                Some(Value::Bool(connected)),
                Completeness::Complete,
                None,
            ),
            Some(Err(error)) => (
                "linux.net-sysfs",
                FactStatus::Unknown,
                None,
                Completeness::Partial,
                Some(error.0.into()),
            ),
            None => (
                "network.unsupported",
                FactStatus::Unsupported,
                None,
                Completeness::Partial,
                Some("network-provider-not-implemented".into()),
            ),
        };
        let fact = Fact {
            key: FactKey {
                name: "network.connected".into(),
                subject: None,
            },
            provider: provider.into(),
            status,
            value,
            completeness,
            reason,
            evidence: Evidence::PlatformObserved,
            observed_at_ms: Some(observed),
            valid_for_ms: FRESH_MS,
        };
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ObservationError("cancelled"));
        }
        capture.snapshot.facts.retain(|f| f.key != fact.key);
        capture.providers.push(ProviderReport {
            provider: fact.provider.clone(),
            status: match fact.status {
                FactStatus::Known => ProviderState::Complete,
                FactStatus::Unsupported => ProviderState::Unsupported,
                _ => ProviderState::Partial,
            },
            reason: fact.reason.clone(),
        });
        capture.snapshot.facts.push(fact);
        capture.now_ms = self.collector.now_ms()?;
        Ok(capture)
    }

    fn now_ms(&self) -> Result<u64, ObservationError> {
        self.collector.now_ms()
    }
}

/// Read at most 64 links and 64 bytes per consumed file. The directory is a
/// trusted local sysfs root (or a synthetic fixture), never an HTTP parameter.
/// True means a non-loopback interface reports operstate=up, including virtual
/// links. It proves neither a route, Internet access, pairing nor an SSID.
/// Any unreadable/unclassified link makes the whole result unknown, not false.
pub fn linux_link_state(root: &Path) -> Result<bool, ObservationError> {
    use std::io::Read;
    fn text(path: &Path) -> Result<String, ObservationError> {
        let file =
            std::fs::File::open(path).map_err(|_| ObservationError("network-link-unavailable"))?;
        let mut bytes = Vec::new();
        file.take(65)
            .read_to_end(&mut bytes)
            .map_err(|_| ObservationError("network-link-unavailable"))?;
        if bytes.len() > 64 {
            return Err(ObservationError("network-link-size-limit"));
        }
        String::from_utf8(bytes)
            .map(|s| s.trim().to_owned())
            .map_err(|_| ObservationError("network-link-malformed"))
    }
    let links =
        std::fs::read_dir(root).map_err(|_| ObservationError("network-enumeration-unavailable"))?;
    let mut connected = false;
    for (count, entry) in links.enumerate() {
        if count >= 64 {
            return Err(ObservationError("network-link-count-limit"));
        }
        let path = entry
            .map_err(|_| ObservationError("network-enumeration-unavailable"))?
            .path();
        let kind = text(&path.join("type"))?
            .parse::<u16>()
            .map_err(|_| ObservationError("network-link-malformed"))?;
        if kind == 772 {
            continue;
        } // ARPHRD_LOOPBACK, independent of interface name.
        match text(&path.join("operstate"))?.as_str() {
            "up" => connected = true,
            "down" | "notpresent" | "lowerlayerdown" => {}
            _ => return Err(ObservationError("network-link-state-unknown")),
        }
    }
    Ok(connected)
}

/// Opaque, process-local proof of the latest accepted capture for one attachment.
/// Not serialized, persisted, or accepted as authentication or route consent.
#[derive(Clone, Debug)]
pub struct Lease {
    identity: Arc<()>,
}

pub struct AttachedCapture {
    pub capture: Capture,
    pub lease: Lease,
}

struct Previous {
    epoch: String,
    revision: u64,
    now_ms: u64,
    context: Context,
    declarations: Vec<Declaration>,
}

struct Current {
    identity: Arc<()>,
    epoch: String,
    revision: u64,
    captured_ms: u64,
    expires_ms: u64,
    context: Context,
    fixture: bool,
}

/// Observation admission only, owned by the same serialized control worker as
/// Controller. No timer thread, persisted arming, profile writes or audio access.
pub struct Attachment<P> {
    provider: P,
    current: Option<Current>,
    previous: Option<Previous>,
}

impl<P: Provider> Attachment<P> {
    pub fn new(provider: P) -> Self {
        Self {
            provider,
            current: None,
            previous: None,
        }
    }

    /// Invalidate pending work immediately, including before a later refresh fails.
    pub fn revoke(&mut self) {
        self.current = None;
    }

    /// Reconnection never restores a lease. Keep revision high-water marks; a
    /// restarted provider must rotate epoch, while owner revisions cannot regress.
    pub fn reconnect(&mut self, provider: P) {
        self.revoke();
        self.provider = provider;
    }

    pub fn now_ms(&self) -> Result<u64, ObservationError> {
        self.provider.now_ms()
    }

    /// Declarations are the existing trusted, saved settings values, never raw
    /// imported facts. Any declaration change requires a fresh manual revision.
    pub fn refresh(
        &mut self,
        cancelled: &AtomicBool,
        context: &Context,
        declarations: &[Declaration],
    ) -> Result<AttachedCapture, Error> {
        self.revoke();
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(fail("cancelled"));
        }
        if declarations.len() > MAX_FACTS {
            return Err(fail("observer-fact-limit"));
        }
        let mut capture = self.provider.capture(cancelled).map_err(|e| fail(e.0))?;
        if capture.snapshot.facts.len() > MAX_FACTS {
            return Err(fail("observer-fact-limit"));
        }
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(fail("cancelled"));
        }
        if let Some(previous) = &self.previous {
            let Previous {
                epoch,
                revision,
                now_ms: now,
                context: old_context,
                declarations: old_declarations,
            } = previous;
            if (epoch == &capture.snapshot.epoch
                && (capture.snapshot.revision <= *revision || capture.now_ms < *now))
                || context.config_revision < old_context.config_revision
                || context.store_revision < old_context.store_revision
                || context.inventory_revision < old_context.inventory_revision
                || context.manual_revision < old_context.manual_revision
                || (old_declarations != declarations
                    && context.manual_revision <= old_context.manual_revision)
            {
                return Err(fail("observer-revision-regression"));
            }
        }
        let fixture = capture
            .snapshot
            .facts
            .iter()
            .any(|f| f.evidence == Evidence::Fixture);
        apply_declarations(&mut capture, declarations)?;
        let now = self.provider.now_ms().map_err(|e| fail(e.0))?;
        if capture.now_ms > now {
            return Err(fail("observer-clock-regression"));
        }
        let mut expires_ms = capture.now_ms.saturating_add(FRESH_MS);
        for fact in &capture.snapshot.facts {
            if fact.status != FactStatus::Known {
                continue;
            }
            let at = fact
                .observed_at_ms
                .ok_or_else(|| fail("observer-missing-time"))?;
            if at > capture.now_ms || fact.completeness != Completeness::Complete {
                return Err(fail("observer-incomplete-or-future-fact"));
            }
            expires_ms = expires_ms.min(at.saturating_add(fact.valid_for_ms.min(FRESH_MS)));
        }
        if now > expires_ms {
            return Err(fail("observer-capture-expired"));
        }
        let identity = Arc::new(());
        self.current = Some(Current {
            identity: identity.clone(),
            epoch: capture.snapshot.epoch.clone(),
            revision: capture.snapshot.revision,
            captured_ms: capture.now_ms,
            expires_ms,
            context: context.clone(),
            fixture,
        });
        self.previous = Some(Previous {
            epoch: capture.snapshot.epoch.clone(),
            revision: capture.snapshot.revision,
            now_ms: capture.now_ms,
            context: context.clone(),
            declarations: declarations.to_vec(),
        });
        Ok(AttachedCapture {
            capture,
            lease: Lease { identity },
        })
    }

    /// Call immediately before arming and every actual commit guard, serialized
    /// with revoke/reconnect/manual events. Tokens must come from trusted evaluation.
    pub fn check(&self, lease: &Lease, token: &StateToken) -> Result<(), Error> {
        let current = self
            .current
            .as_ref()
            .ok_or_else(|| fail("observer-detached"))?;
        let now = self.provider.now_ms().map_err(|e| fail(e.0))?;
        if !Arc::ptr_eq(&current.identity, &lease.identity)
            || token.epoch != current.epoch
            || token.snapshot_revision != current.revision
            || token.context != current.context
            || token.evaluated_at_ms < current.captured_ms
            || token.evaluated_at_ms > now
            || now < current.captured_ms
            || now > current.expires_ms
            || current.fixture
        {
            return Err(fail("observer-stale-or-untrusted-lease"));
        }
        Ok(())
    }

    /// Delegates to the existing controller; failed admission always disarms it.
    pub fn arm(
        &self,
        controller: &mut Controller,
        lease: &Lease,
        token: &StateToken,
    ) -> Result<(), Error> {
        if let Err(error) = self.check(lease, token) {
            let _ = controller.disarm(token);
            return Err(error);
        }
        controller.arm(token)
    }

    pub fn guard(
        &self,
        controller: &mut Controller,
        lease: &Lease,
        candidate: &TransitionCandidate,
        token: &StateToken,
    ) -> Result<(), Error> {
        if let Err(error) = self.check(lease, token) {
            let _ = controller.disarm(token);
            return Err(error);
        }
        controller.guard(candidate, token)
    }
}

fn fail(reason: &'static str) -> Error {
    Error::new("observer-unavailable", reason)
}

/// Same declaration vocabulary as settings/web activation. Absent flags remain
/// unknown: only exact declared subjects produce true or false facts.
pub fn apply_declarations(
    capture: &mut Capture,
    declarations: &[Declaration],
) -> Result<(), Error> {
    if capture.snapshot.facts.len() > MAX_FACTS || declarations.len() > MAX_FACTS {
        return Err(fail("observer-fact-limit"));
    }
    admit_capture(capture)?;
    let mut keys = BTreeSet::new();
    for declaration in declarations {
        if !matches!(
            declaration.key.name.as_str(),
            "manual.flag" | "device.hardware-monitor-declared"
        ) || !declaration
            .key
            .subject
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty() && s.len() <= MAX_TEXT_BYTES)
            || !keys.insert(&declaration.key)
        {
            return Err(fail("observer-invalid-declaration"));
        }
    }
    let retained = capture
        .snapshot
        .facts
        .iter()
        .filter(|f| !keys.contains(&f.key))
        .count();
    if retained + declarations.len() > MAX_FACTS {
        return Err(fail("observer-fact-limit"));
    }
    capture.snapshot.facts.retain(|f| !keys.contains(&f.key));
    for declaration in declarations {
        capture.snapshot.facts.push(Fact {
            key: declaration.key.clone(),
            status: FactStatus::Known,
            value: Some(Value::Bool(declaration.value)),
            provider: "local.declaration".into(),
            evidence: Evidence::UserDeclared,
            observed_at_ms: Some(capture.now_ms),
            valid_for_ms: FRESH_MS,
            completeness: Completeness::Complete,
            reason: None,
        });
    }
    // Provider summaries must describe the final composition, not old unsupported
    // facts removed by a legitimate declaration.
    capture.providers = summarize(&capture.snapshot.facts);
    Ok(())
}

fn summarize(facts: &[Fact]) -> Vec<ProviderReport> {
    let mut providers = std::collections::BTreeMap::<_, Vec<_>>::new();
    for fact in facts {
        providers.entry(&fact.provider).or_default().push(fact);
    }
    providers
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
            // Keep one bounded reason; per-fact reasons remain authoritative.
            let reason = facts.iter().find_map(|f| f.reason.clone());
            ProviderReport {
                provider: provider.clone(),
                status,
                reason,
            }
        })
        .collect()
}

fn admit_capture(capture: &mut Capture) -> Result<(), Error> {
    // Move, rather than clone, unadmitted provider input. Restore even on error.
    let snapshot = std::mem::replace(
        &mut capture.snapshot,
        super::Snapshot {
            schema_version: 1,
            epoch: String::new(),
            revision: 0,
            facts: Vec::new(),
        },
    );
    let request = super::Request {
        graph: super::Graph {
            schema_version: 1,
            id: "observer-admission".into(),
            name: "Observer admission".into(),
            revision: 0,
            root: "end".into(),
            nodes: vec![super::Node::Terminal {
                id: "end".into(),
                action: super::Action::NoChange {},
            }],
        },
        catalog: super::standard_catalog(),
        snapshot,
        now_ms: capture.now_ms,
        context: Context {
            config_revision: 0,
            store_revision: 0,
            inventory_revision: 0,
            manual_revision: 0,
        },
        manual_override: false,
    };
    let result = super::evaluate_scoped(&request, &capture.application_scope).map(|_| ());
    capture.snapshot = request.snapshot;
    result
}
