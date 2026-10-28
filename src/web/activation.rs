//! One owned worker serializes saved-policy decisions and desired-profile writes.
//! Capture runs off the HTTP pool; cancellation is signalled before lifecycle commands.
use super::{error, json, Reply};
use hyper::StatusCode;
use mixyard::{
    automation as core, automation::settings, context, profiles, runtime::ReadOnlyClient, Config,
    Inventory,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const STORE_LIMIT: usize = 1024 * 1024;
const REVIEW_MS: u64 = 300_000;
const TICK: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Serialize)]
struct Failure {
    code: String,
    message: String,
}
type Result<T> = std::result::Result<T, Failure>;
fn fail(code: &str, message: impl Into<String>) -> Failure {
    Failure {
        code: code.into(),
        message: message.into(),
    }
}
impl From<core::Error> for Failure {
    fn from(e: core::Error) -> Self {
        fail(e.code, e.message)
    }
}
impl From<profiles::Error> for Failure {
    fn from(e: profiles::Error) -> Self {
        fail(e.code, e.message)
    }
}
impl From<context::ObservationError> for Failure {
    fn from(e: context::ObservationError) -> Self {
        fail("context-unavailable", e.to_string())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Save {
    expected_settings_revision: u64,
    settings: settings::Settings,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Arm {
    review_id: String,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
enum Manual {
    UntilCleared,
    For {
        #[serde(rename = "durationMs")]
        duration_ms: u64,
    },
    Clear,
}

enum Command {
    Live(bool),
    Context,
    Arm(String),
    Disarm,
    Manual(Manual),
    Settings(Save),
    Edit(u64, profiles::Edit),
}
struct Envelope {
    command: Command,
    deadline: Instant,
    reply: mpsc::SyncSender<Result<Value>>,
}
struct Shared {
    // Serializes signalling cancellation with the final guard/write/ack. Never held
    // during observation or IPC. A manual event cannot race a capture into commit.
    commit: Mutex<()>,
    cancelled: AtomicBool,
    stopped: AtomicBool,
    status: Mutex<Value>,
    slow: AtomicBool,
    observing: AtomicBool,
}
pub(super) struct Service {
    commands: Option<mpsc::SyncSender<Envelope>>,
    shared: Arc<Shared>,
    worker: Option<thread::JoinHandle<()>>,
    settings: Option<settings::Store>,
    initial_error: Option<Failure>,
}
impl Service {
    pub(super) fn start(profile_path: &Path, instance: String) -> Self {
        let settings_store =
            settings::Store::sibling_of_profiles(profile_path).map_err(Failure::from);
        let engine = (|| {
            let store = settings_store.as_ref().map_err(Clone::clone)?;
            Engine::new(
                profiles::Store::new(profile_path)?,
                settings::Store::new(store.directory())?,
                instance,
            )
        })();
        // Invalid automation storage disables only automation. Do not synthesize a
        // revision-zero settings record or prevent offline/manual console work.
        let initial_error = engine.as_ref().err().cloned();
        let shared = Arc::new(Shared {
            commit: Mutex::new(()),
            cancelled: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            status: Mutex::new(engine.as_ref().map_or(Value::Null, Engine::status)),
            slow: AtomicBool::new(false),
            observing: AtomicBool::new(false),
        });
        let mut service = Self {
            commands: None,
            shared,
            worker: None,
            settings: settings_store.ok(),
            initial_error,
        };
        if let Ok(engine) = engine {
            let (tx, rx) = mpsc::sync_channel::<Envelope>(4);
            let owned = service.shared.clone();
            match thread::Builder::new()
                .name("situation-owner".into())
                .spawn(move || engine.run(rx, owned))
            {
                Ok(worker) => {
                    service.worker = Some(worker);
                    service.commands = Some(tx);
                }
                Err(e) => service.initial_error = Some(fail("owner-unavailable", e.to_string())),
            }
        }
        service
    }
    pub(super) fn is_available(&self) -> bool {
        self.worker.as_ref().is_some_and(|w| !w.is_finished())
    }
    fn ensure_owner(&self) -> Result<()> {
        if let Some(e) = &self.initial_error {
            return Err(fail("automation-unavailable",format!("Automatic owner could not start ({}: {}). Preserve/recover the store and restart; no settings were defaulted or overwritten. Offline tools remain available.",e.code,e.message)));
        }
        if !self.is_available() {
            return Err(fail(
                "owner-unavailable",
                "Automatic owner is stopped; restart after inspecting/reconciling stores",
            ));
        }
        Ok(())
    }
    fn request(&self, command: Command, cancel: bool, slow: bool) -> Result<Value> {
        self.ensure_owner()?;
        if slow && self.shared.observing.load(Ordering::Acquire) {
            return Err(fail(
                "context-busy",
                "Owner is capturing context; retry shortly. Editing/status remain available",
            ));
        }
        if slow && self.shared.slow.swap(true, Ordering::AcqRel) {
            return Err(fail(
                "context-busy",
                "One live capture/review is already in progress; editing remains available",
            ));
        }
        struct Slow<'a>(&'a AtomicBool, bool);
        impl Drop for Slow<'_> {
            fn drop(&mut self) {
                if self.1 {
                    self.0.store(false, Ordering::Release);
                }
            }
        }
        let _slow = Slow(&self.shared.slow, slow);
        let (tx, rx) = mpsc::sync_channel(1);
        let envelope = Envelope {
            command,
            deadline: Instant::now() + Duration::from_secs(7),
            reply: tx,
        };
        {
            let _gate = self
                .shared
                .commit
                .lock()
                .map_err(|_| fail("owner-unavailable", "Owner lock failed"))?;
            if cancel {
                self.shared.cancelled.store(true, Ordering::Release);
            }
            self.commands
                .as_ref()
                .ok_or_else(|| fail("owner-unavailable", "Owner stopped"))?
                .try_send(envelope)
                .map_err(|_| {
                    fail(
                        "service-busy",
                        "Situation command queue is full or stopped; refresh before retrying",
                    )
                })?;
        }
        rx.recv_timeout(Duration::from_secs(8)).map_err(|_| {
            fail(
                "operation-timeout",
                "Owner response deadline exceeded; reread status/store before retrying a mutation",
            )
        })?
    }
    pub(super) fn edit(&self, revision: u64, edit: profiles::Edit) -> Reply {
        reply(self.request(Command::Edit(revision, edit), true, false))
    }
    pub(super) fn context(&self) -> Reply {
        reply(self.request(Command::Context, false, true))
    }
    pub(super) fn route(&self, path: &str, bytes: &[u8]) -> Reply {
        fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
            serde_json::from_slice(bytes).map_err(|e| fail("invalid-json", e.to_string()))
        }
        let result = (|| match path {
            "/api/automation/settings/read" => {
                let _: Empty = parse(bytes)?;
                let store = self.settings.as_ref().ok_or_else(|| {
                    self.initial_error.clone().unwrap_or_else(|| {
                        fail("automation-unavailable", "Settings path unavailable")
                    })
                })?;
                Ok(serde_json::to_value(store.read()?).expect("settings JSON"))
            }
            "/api/automation/settings/write" => {
                let save: Save = parse(bytes)?;
                save.settings.validate()?;
                self.request(Command::Settings(save), true, false)
            }
            "/api/automation/arm" => {
                let arm: Arm = parse(bytes)?;
                if arm.review_id.len() > 64 {
                    return Err(fail("review-required", "Invalid review ID"));
                }
                self.request(Command::Arm(arm.review_id), false, true)
            }
            "/api/automation/manual" => {
                let manual: Manual = parse(bytes)?;
                if matches!(manual,Manual::For{duration_ms} if duration_ms==0 || duration_ms>settings::MAX_DURATION_MS)
                {
                    return Err(fail("invalid-json", "durationMs must be 1..86400000"));
                }
                self.request(Command::Manual(manual), true, false)
            }
            _ => {
                let _: Empty = parse(bytes)?;
                match path {
                    "/api/automation/status" => {
                        self.ensure_owner()?;
                        Ok(self
                            .shared
                            .status
                            .lock()
                            .map_err(|_| fail("owner-unavailable", "Status lock failed"))?
                            .clone())
                    }
                    "/api/automation/disarm" => self.request(Command::Disarm, true, false),
                    "/api/automation/live" => self.request(Command::Live(false), false, true),
                    "/api/automation/review" => self.request(Command::Live(true), false, true),
                    _ => Err(fail("not-found", "Unknown automation endpoint")),
                }
            }
        })();
        reply(result)
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        self.shared.cancelled.store(true, Ordering::Release);
        self.commands.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn reply(result: Result<Value>) -> Reply {
    match result {
        Ok(value) => json(&value),
        Err(e) => error(
            match e.code.as_str() {
                "invalid-json" => StatusCode::BAD_REQUEST,
                "not-found" | "profile-not-found" => StatusCode::NOT_FOUND,
                "context-busy"
                | "context-unavailable"
                | "inventory-unavailable"
                | "service-busy"
                | "owner-unavailable"
                | "automation-unavailable" => StatusCode::SERVICE_UNAVAILABLE,
                "operation-timeout" => StatusCode::GATEWAY_TIMEOUT,
                "resource-limit" | "invalid-settings" | "invalid-automation" | "invalid-policy"
                | "invalid-override" | "invalid-name" | "no-undo" | "revision-exhausted" => {
                    StatusCode::UNPROCESSABLE_ENTITY
                }
                "store-conflict"
                | "store-busy"
                | "recovery-required"
                | "commit-uncertain"
                | "review-required"
                | "review-stale"
                | "cancelled"
                | "graph-required"
                | "draft-required"
                | "no-selection"
                | "stale-decision"
                | "controller-conflict" => StatusCode::CONFLICT,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            },
            &e.code,
            &e.message,
        ),
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Authority {
    epoch: String,
    settings_revision: u64,
    store_revision: u64,
    config_revision: u64,
    inventory_revision: u64,
    manual_revision: u64,
}
struct Approved {
    action: core::Action,
    candidate: Value,
}
struct Review {
    id: String,
    expires_at_ms: u64,
    authority: Authority,
    actions: Vec<Approved>,
    inventory: Value,
}
struct Live {
    capture: context::Capture,
    evaluation: core::Evaluation,
    request: core::Request,
    inventory: Option<Inventory>,
    blocked: Option<Failure>,
}
struct Engine {
    profiles: profiles::Store,
    settings_store: settings::Store,
    settings: settings::Snapshot,
    store: profiles::Snapshot,
    store_value: Value,
    instance: String,
    collector: context::Collector,
    controller: Option<core::Controller>,
    review: Option<Review>,
    armed_review: Option<Review>,
    manual: Option<core::ManualOverride>,
    manual_revision: u64,
    inventory: Option<Value>,
    inventory_revision: Option<u64>,
    blocked: Option<Failure>,
    decision: Value,
}
impl Engine {
    fn new(
        profiles: profiles::Store,
        settings_store: settings::Store,
        instance: String,
    ) -> Result<Self> {
        let settings = settings_store.read()?;
        let store = profiles.read_bounded(STORE_LIMIT)?;
        let collector = context::Collector::new(options(&settings.settings))?;
        let store_value = serde_json::to_value(&store).expect("validated profile JSON");
        Ok(Self {
            profiles,
            settings_store,
            settings,
            store,
            store_value,
            instance,
            collector,
            controller: None,
            review: None,
            armed_review: None,
            manual: None,
            manual_revision: 0,
            inventory: None,
            inventory_revision: None,
            blocked: None,
            decision: json!({"kind":"disarmed"}),
        })
    }
    fn run(mut self, rx: mpsc::Receiver<Envelope>, shared: Arc<Shared>) {
        while !shared.stopped.load(Ordering::Acquire) {
            match rx.recv_timeout(TICK) {
                Ok(envelope) => self.handle_envelope(envelope, &shared),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Never clear cancellation belonging to a queued lifecycle command.
                    if shared.cancelled.load(Ordering::Acquire) {
                        continue;
                    }
                    shared.observing.store(true, Ordering::Release);
                    if let Err(e) = self.tick(&shared) {
                        self.blocked = Some(e);
                    }
                    shared.observing.store(false, Ordering::Release);
                    self.publish(&shared);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    }
    fn handle_envelope(&mut self, envelope: Envelope, shared: &Shared) {
        let cancels = matches!(
            envelope.command,
            Command::Disarm | Command::Manual(_) | Command::Settings(_) | Command::Edit(_, _)
        );
        if Instant::now() > envelope.deadline {
            // Expiration must never resurrect ownership after an admitted manual
            // or disarm event. Skip late writes/arming but retain cancellation.
            if cancels {
                self.revoke();
            }
            self.blocked = Some(fail(
                "operation-timeout",
                "Command expired before execution; cancellation retained",
            ));
            self.publish(shared);
            let _ = envelope
                .reply
                .send(Err(self.blocked.clone().expect("timeout set")));
            return;
        }
        shared.cancelled.store(false, Ordering::Release);
        let observes = matches!(
            envelope.command,
            Command::Live(_) | Command::Context | Command::Arm(_)
        );
        shared.observing.store(observes, Ordering::Release);
        let result = self.command(envelope.command, shared);
        shared.observing.store(false, Ordering::Release);
        if let Err(e) = &result {
            if cancels {
                self.revoke();
            }
            self.blocked = Some(e.clone());
        }
        self.publish(shared);
        let _ = envelope.reply.send(result);
    }
    fn publish(&self, shared: &Shared) {
        if let Ok(mut status) = shared.status.lock() {
            *status = self.status();
        }
    }
    fn now(&self) -> u64 {
        self.collector.now_ms().unwrap_or(u64::MAX)
    }
    fn status(&self) -> Value {
        let now = self.now();
        let manual = match self.manual {
            Some(core::ManualOverride::UntilCleared) => json!({"mode":"until-cleared"}),
            Some(core::ManualOverride::UntilMs(end)) if now < end => {
                json!({"mode":"for","untilMs":end})
            }
            _ => Value::Null,
        };
        json!({"mode":"stored-intent-only","armed":self.controller.as_ref().is_some_and(|c|c.is_armed()),"manualOverride":manual,
            "settingsRevision":self.settings.settings_revision,"storeRevision":self.store.store_revision,
            "configRevision":self.store.draft.as_ref().map_or(0,|c|c.revision),"inventoryRevision":self.inventory_revision,
            "manualRevision":self.manual_revision,"decision":self.decision,"blocked":self.blocked,
            "lastAppliedAtMs":self.controller.as_ref().and_then(|c|c.last_applied_at_ms()),
            "review":self.review.as_ref().filter(|r|now<=r.expires_at_ms).map(|r|json!({"id":r.id,"expiresAtMs":r.expires_at_ms})),
            "audioApplied":false,"backendAvailable":false})
    }
    fn revoke(&mut self) {
        self.review = None;
        self.armed_review = None;
        if let Some(c) = &mut self.controller {
            let token = c.current().clone();
            let _ = c.disarm(&token);
        }
        self.decision = json!({"kind":"disarmed"});
    }
    fn manual_event(&mut self, manual: Option<core::ManualOverride>, revoke: bool) -> Result<()> {
        if revoke {
            self.revoke();
        }
        self.manual_revision = self
            .manual_revision
            .checked_add(1)
            .ok_or_else(|| fail("revision-exhausted", "Manual revision exhausted"))?;
        self.manual = manual;
        let now = self.now();
        if let Some(c) = &mut self.controller {
            let mut token = c.current().clone();
            token.evaluated_at_ms = now;
            token.context.manual_revision = self.manual_revision;
            match manual {
                Some(m) => c.set_manual_override(&token, m)?,
                None => c.clear_manual_override(&token)?,
            }
        }
        // Ownership-only pause changes no reviewed configuration, consent or policy.
        if let Some(r) = &mut self.armed_review {
            r.authority.manual_revision = self.manual_revision;
        }
        self.review = None;
        Ok(())
    }
    fn sync(&mut self) -> Result<()> {
        let settings = self.settings_store.read()?;
        if settings != self.settings {
            self.revoke();
            let remaining = match self.manual {
                Some(core::ManualOverride::UntilMs(end)) => Some(end.saturating_sub(self.now())),
                _ => None,
            };
            self.collector = context::Collector::new(options(&settings.settings))?;
            if let Some(remaining) = remaining {
                self.manual = if remaining == 0 {
                    None
                } else {
                    Some(core::ManualOverride::UntilMs(remaining))
                };
            }
            self.controller = None;
            self.inventory = None;
            self.inventory_revision = None;
            self.settings = settings;
        }
        let store = self.profiles.read_bounded(STORE_LIMIT)?;
        let value = serde_json::to_value(&store).expect("profile JSON");
        if value != self.store_value {
            self.manual_event(Some(core::ManualOverride::UntilCleared), true)?;
            self.store_value = value;
            self.store = store;
        }
        Ok(())
    }
    fn capture(&mut self, shared: &Shared) -> Result<context::Capture> {
        self.sync()?;
        let mut capture = self.collector.capture(&shared.cancelled)?;
        if capture
            .snapshot
            .facts
            .len()
            .saturating_add(self.settings.settings.declarations.len())
            > core::MAX_FACTS
        {
            return Err(fail(
                "resource-limit",
                "Combined observation/declaration admission exceeds 512 facts",
            ));
        }
        // Only strict saved declarations acquire fresh user-declared evidence.
        for declaration in &self.settings.settings.declarations {
            capture.snapshot.facts.retain(|f| f.key != declaration.key);
            capture.snapshot.facts.push(core::Fact {
                key: declaration.key.clone(),
                status: core::FactStatus::Known,
                value: Some(core::Value::Bool(declaration.value)),
                provider: "local.declaration".into(),
                evidence: core::Evidence::UserDeclared,
                observed_at_ms: Some(capture.now_ms),
                valid_for_ms: 5000,
                completeness: core::Completeness::Complete,
                reason: None,
            });
        }
        // Admission before any duplicate scans or request clones.
        if capture.snapshot.facts.len() > core::MAX_FACTS {
            return Err(fail(
                "resource-limit",
                "Combined observations/declarations exceed 512 facts",
            ));
        }
        Ok(capture)
    }
    fn live(&mut self, shared: &Shared) -> Result<Live> {
        let mut capture = self.capture(shared)?;
        if matches!(self.manual,Some(core::ManualOverride::UntilMs(end)) if self.now()>=end) {
            self.manual = None;
        }
        if self.settings.settings.graph.is_none() {
            return Err(fail(
                "graph-required",
                "Save a graph and catalog before live evaluation",
            ));
        }
        let inventory = ReadOnlyClient::inventory(&self.instance, &shared.cancelled).map_err(|e| {
            fail(
                "inventory-unavailable",
                format!(
                    "Actual runtime inventory unavailable: {e}; no automatic stored-profile writes"
                ),
            )
        });
        let (inventory, blocked) = match inventory {
            Ok(inventory) => {
                let value = serde_json::to_value(&inventory).expect("inventory JSON");
                if self.inventory_revision.is_some_and(|previous| {
                    inventory.revision < previous
                        || (inventory.revision == previous
                            && self.inventory.as_ref() != Some(&value))
                }) {
                    self.revoke();
                    return Err(fail("inventory-unavailable", "Runtime inventory regressed or changed content without advancing its revision; no automatic writes"));
                }
                if self
                    .inventory
                    .as_ref()
                    .is_some_and(|previous| previous != &value)
                {
                    self.revoke();
                }
                self.inventory = Some(value);
                self.inventory_revision = Some(inventory.revision);
                (Some(inventory), None)
            }
            Err(e) => (None, Some(e)),
        };
        capture.now_ms = self.collector.now_ms()?;
        let mut snapshot = capture.snapshot.clone();
        // Native capture reports the full standard vocabulary; evaluation admits only
        // definitions in this saved policy. Original context remains visible unchanged.
        snapshot.facts.retain(|f| {
            self.settings
                .settings
                .catalog
                .iter()
                .any(|d| d.name == f.key.name)
        });
        let request = core::Request {
            graph: self
                .settings
                .settings
                .graph
                .as_ref()
                .expect("live graph")
                .clone(),
            catalog: self.settings.settings.catalog.clone(),
            snapshot,
            now_ms: capture.now_ms,
            context: core::Context {
                config_revision: self.store.draft.as_ref().map_or(0, |c| c.revision),
                store_revision: self.store.store_revision,
                inventory_revision: inventory
                    .as_ref()
                    .map_or(self.inventory_revision.unwrap_or(0), |i| i.revision),
                manual_revision: self.manual_revision,
            },
            manual_override: false,
        };
        let evaluation = core::evaluate_scoped(&request, &capture.application_scope)?;
        Ok(Live {
            capture,
            evaluation,
            request,
            inventory,
            blocked,
        })
    }
    fn authority(&self, live: &Live) -> Result<Authority> {
        Ok(Authority {
            epoch: live.evaluation.token.epoch.clone(),
            settings_revision: self.settings.settings_revision,
            store_revision: self.store.store_revision,
            config_revision: live.evaluation.token.context.config_revision,
            inventory_revision: live
                .inventory
                .as_ref()
                .ok_or_else(|| fail("inventory-unavailable", "Actual inventory required"))?
                .revision,
            manual_revision: self.manual_revision,
        })
    }
    fn controller(&mut self, token: &core::StateToken) -> Result<&mut core::Controller> {
        if self.controller.is_none() {
            let p = &self.settings.settings.transition_policy;
            let mut initial = token.clone();
            if self.manual.is_some() {
                initial.context.manual_revision = self.manual_revision.saturating_sub(1);
            }
            let mut controller = core::Controller::new(
                core::TransitionPolicy {
                    settle_ms: p.settle_ms.0,
                    minimum_hold_ms: p.minimum_hold_ms.0,
                    cooldown_ms: p.cooldown_ms.0,
                },
                &initial,
            )?;
            if let Some(m) = self.manual {
                controller.set_manual_override(token, m)?;
            }
            self.controller = Some(controller);
        }
        Ok(self.controller.as_mut().expect("controller initialized"))
    }
    fn preview(
        &self,
        live: &Live,
        evaluation: &core::Evaluation,
    ) -> Result<(Config, profiles::Preview)> {
        let current = self.store.draft.as_ref().ok_or_else(|| {
            fail(
                "draft-required",
                "Save a validated current draft before reviewing automation",
            )
        })?;
        let inventory = live.inventory.as_ref().ok_or_else(|| {
            fail(
                "inventory-unavailable",
                "Actual inventory required for preview",
            )
        })?;
        admit(&self.store, current, inventory, 1)?;
        let proposal = core::preview_proposal(
            evaluation,
            &evaluation.token,
            current,
            &self.store.profiles,
            inventory,
        )?;
        let candidate = proposal
            .candidate
            .ok_or_else(|| fail("no-selection", "No stored change selected"))?;
        // The actual store high-water revision, not proposal's lower core revision.
        let preview = profiles::preview(&self.store, &candidate, inventory)?;
        Ok((candidate, preview))
    }
    fn review(&mut self, live: &Live) -> Result<Value> {
        let authority = self.authority(live)?;
        let graph = self.settings.settings.graph.as_ref().expect("live graph");
        let mut reachable = std::collections::BTreeSet::new();
        let mut todo = vec![graph.root.as_str()];
        while let Some(id) = todo.pop() {
            if !reachable.insert(id) {
                continue;
            }
            if let Some(core::Node::Decision {
                on_true,
                on_false,
                on_unknown,
                ..
            }) = graph.nodes.iter().find(|n| n.id() == id)
            {
                todo.extend([on_true.as_str(), on_false.as_str(), on_unknown.as_str()]);
            }
        }
        let actions: Vec<_> = graph
            .nodes
            .iter()
            .filter_map(|n| match n {
                core::Node::Terminal {
                    id,
                    action: action @ core::Action::Select { .. },
                } if reachable.contains(id.as_str()) => Some((id, action)),
                _ => None,
            })
            .collect();
        let current = self
            .store
            .draft
            .as_ref()
            .ok_or_else(|| fail("draft-required", "A current draft is required"))?;
        admit(
            &self.store,
            current,
            live.inventory.as_ref().expect("authority inventory"),
            actions.len(),
        )?;
        let mut approved = vec![];
        let mut previews = vec![];
        for (terminal, action) in actions {
            let mut evaluation = live.evaluation.clone();
            evaluation.action = action.clone();
            evaluation.terminal = Some(terminal.clone());
            let (candidate, preview) = self.preview(live, &evaluation)?;
            // Store prospective revision is surfaced with the reviewed candidate.
            let mut candidate = candidate;
            candidate.revision = preview.plan.config_revision;
            let value = serde_json::to_value(&candidate).expect("candidate JSON");
            approved.push(Approved {
                action: action.clone(),
                candidate: normalized(&candidate),
            });
            previews.push(
                json!({"terminal":terminal,"action":action,"candidate":value,"preview":preview}),
            );
        }
        if approved.is_empty() {
            return Err(fail(
                "no-selection",
                "No reachable Select terminal to review",
            ));
        }
        let id = random_id()?;
        let expires_at_ms = self
            .now()
            .checked_add(REVIEW_MS)
            .ok_or_else(|| fail("revision-exhausted", "Review clock exhausted"))?;
        let response =
            json!({"id":id,"expiresAtMs":expires_at_ms,"authority":authority,"actions":previews});
        self.review = Some(Review {
            id,
            expires_at_ms,
            authority,
            actions: approved,
            inventory: self.inventory.clone().expect("inventory captured"),
        });
        Ok(response)
    }
    fn command(&mut self, command: Command, shared: &Shared) -> Result<Value> {
        match command {
            Command::Disarm => {
                self.revoke();
                self.blocked = None;
                Ok(self.status())
            }
            Command::Manual(manual) => {
                self.sync()?;
                let manual = match manual {
                    Manual::UntilCleared => Some(core::ManualOverride::UntilCleared),
                    Manual::For { duration_ms } => Some(core::ManualOverride::UntilMs(
                        self.now()
                            .checked_add(duration_ms)
                            .ok_or_else(|| fail("revision-exhausted", "Manual clock exhausted"))?,
                    )),
                    Manual::Clear => None,
                };
                self.manual_event(manual, false)?;
                self.blocked = None;
                self.decision =
                    json!({"kind":if manual.is_some(){"manual-override"}else{"waiting"}});
                Ok(self.status())
            }
            Command::Settings(save) => {
                self.revoke();
                let saved = self
                    .settings_store
                    .write(save.expected_settings_revision, &save.settings)?;
                self.sync()?;
                self.blocked = None;
                Ok(serde_json::to_value(saved).expect("settings JSON"))
            }
            Command::Edit(revision, edit) => {
                self.manual_event(Some(core::ManualOverride::UntilCleared), true)?;
                self.profiles.read_bounded(super::MAX_RESPONSE)?;
                let saved = self.profiles.edit(revision, edit)?;
                self.store_value = serde_json::to_value(&saved).expect("profile JSON");
                self.store = saved;
                self.blocked = None;
                Ok(self.store_value.clone())
            }
            Command::Context => {
                let capture = self.capture(shared)?;
                Ok(
                    json!({"mode":"read-only-live-context","snapshot":capture.snapshot,"nowMs":capture.now_ms,"providers":capture.providers,"applicationScope":capture.application_scope,"catalog":core::standard_catalog()}),
                )
            }
            Command::Live(wants_review) => {
                let live = self.live(shared)?;
                let mut blocked = live.blocked.clone();
                let preview = if live.inventory.is_some()
                    && matches!(live.evaluation.action, core::Action::Select { .. })
                {
                    match self.preview(&live, &live.evaluation) {
                        Ok((mut candidate, preview)) => {
                            candidate.revision = preview.plan.config_revision;
                            Some(json!({"candidate":candidate,"preview":preview}))
                        }
                        Err(e) => {
                            blocked = Some(e);
                            None
                        }
                    }
                } else {
                    None
                };
                let review = if wants_review {
                    match self.review(&live) {
                        Ok(r) => Some(r),
                        Err(e) => {
                            self.review = None;
                            blocked = Some(e);
                            None
                        }
                    }
                } else {
                    None
                };
                self.blocked = blocked.clone();
                Ok(
                    json!({"mode":"stored-intent-only","audioApplied":false,"backendAvailable":false,"status":self.status(),"evaluation":live.evaluation,"request":live.request,"context":live.capture,"inventory":live.inventory,"preview":preview,"review":review,"blocked":blocked}),
                )
            }
            Command::Arm(id) => {
                // Fresh capture/actual stores/inventory, never client snapshot claims.
                let live = self.live(shared)?;
                let authority = self.authority(&live)?;
                let review = self.review.as_ref().ok_or_else(|| {
                    fail(
                        "review-required",
                        "Create and inspect a concrete review before arming",
                    )
                })?;
                if review.id != id
                    || self.now() > review.expires_at_ms
                    || review.authority != authority
                    || self.inventory.as_ref() != Some(&review.inventory)
                {
                    self.review = None;
                    return Err(fail(
                        "review-stale",
                        "Review expired or owned authority changed; review again",
                    ));
                }
                for approved in &review.actions {
                    let mut evaluation = live.evaluation.clone();
                    evaluation.action = approved.action.clone();
                    let (candidate, _) = self.preview(&live, &evaluation)?;
                    if normalized(&candidate) != approved.candidate {
                        return Err(fail(
                            "review-stale",
                            "Reviewed candidate changed; review again",
                        ));
                    }
                }
                let _gate = shared
                    .commit
                    .lock()
                    .map_err(|_| fail("owner-unavailable", "Owner lock failed"))?;
                if shared.cancelled.load(Ordering::Acquire) {
                    return Err(fail("cancelled", "Arming canceled by a lifecycle event"));
                }
                // Re-read after expensive capture and all preview work.
                self.sync()?;
                if self.settings.settings_revision != authority.settings_revision
                    || self.store.store_revision != authority.store_revision
                    || self.manual_revision != authority.manual_revision
                {
                    return Err(fail(
                        "review-stale",
                        "Authority changed during arming; review again",
                    ));
                }
                self.controller(&live.evaluation.token)?
                    .arm(&live.evaluation.token)?;
                self.armed_review = self.review.take();
                self.blocked = None;
                self.decision = json!({"kind":"armed"});
                Ok(self.status())
            }
        }
    }
    fn tick(&mut self, shared: &Shared) -> Result<()> {
        if let Err(e) = self.sync() {
            self.revoke();
            return Err(e);
        }
        if !self.controller.as_ref().is_some_and(|c| c.is_armed()) {
            return Ok(());
        }
        let live = self.live(shared)?;
        if let Some(e) = &live.blocked {
            return Err(e.clone());
        }
        if !self.controller.as_ref().is_some_and(|c| c.is_armed()) {
            return Ok(());
        }
        self.check_armed(&live)?;
        let decision = self
            .controller(&live.evaluation.token)?
            .observe(&live.evaluation, &live.evaluation.token)?;
        self.decision = decision_json(&decision);
        self.blocked = None;
        if !matches!(decision, core::ControllerDecision::Candidate(_)) {
            return Ok(());
        }
        // A second server-owned capture immediately before resolving the candidate.
        let fresh = self.live(shared)?;
        if let Some(e) = &fresh.blocked {
            return Err(e.clone());
        }
        self.check_armed(&fresh)?;
        let decision = self
            .controller(&fresh.evaluation.token)?
            .observe(&fresh.evaluation, &fresh.evaluation.token)?;
        self.decision = decision_json(&decision);
        if !matches!(decision, core::ControllerDecision::Candidate(_)) {
            return Ok(());
        }
        let (input, preview) = self.preview(&fresh, &fresh.evaluation)?;
        let expected_config_revision = preview.plan.config_revision;
        let normalized_input = normalized(&input);
        let approved = self
            .armed_review
            .as_ref()
            .expect("checked armed review")
            .actions
            .iter()
            .any(|a| a.action == fresh.evaluation.action && a.candidate == normalized_input);
        if !approved {
            self.revoke();
            return Err(fail(
                "review-stale",
                "Selected candidate differs from exact reviewed actions/configuration",
            ));
        }
        let _gate = shared
            .commit
            .lock()
            .map_err(|_| fail("owner-unavailable", "Owner lock failed"))?;
        if shared.cancelled.load(Ordering::Acquire) || shared.stopped.load(Ordering::Acquire) {
            return Err(fail("cancelled", "Pending capture canceled before commit"));
        }
        self.sync()?;
        self.check_armed(&fresh)?;
        // Refresh the monotonic clock after previews/store reads, rechecking all fact
        // freshness and branch selection immediately before guard and persistence.
        let evaluation = self.reevaluate(&fresh)?;
        let decision = self
            .controller(&evaluation.token)?
            .observe(&evaluation, &evaluation.token)?;
        let core::ControllerDecision::Candidate(candidate) = decision else {
            self.decision = decision_json(&decision);
            return Ok(());
        };
        if candidate.action() != &fresh.evaluation.action {
            return Ok(());
        }
        self.controller(&evaluation.token)?
            .guard(&candidate, &evaluation.token)?;
        let current_bytes = serde_json::to_vec(&self.store).expect("store JSON").len();
        let candidate_bytes = serde_json::to_vec(&input).expect("candidate JSON").len();
        if current_bytes.saturating_add(candidate_bytes.saturating_mul(2)) > STORE_LIMIT {
            self.revoke();
            return Err(fail(
                "resource-limit",
                "Prospective automatic store exceeds the 1 MiB admission budget",
            ));
        }
        let write = self
            .profiles
            .edit(self.store.store_revision, profiles::Edit::Draft(input));
        self.complete_write(write, &evaluation, &candidate, expected_config_revision)
    }
    fn complete_write(
        &mut self,
        write: std::result::Result<profiles::Snapshot, profiles::Error>,
        evaluation: &core::Evaluation,
        candidate: &core::TransitionCandidate,
        expected_config_revision: u64,
    ) -> Result<()> {
        match write {
            Ok(committed) => {
                let config_revision = committed
                    .draft
                    .as_ref()
                    .ok_or_else(|| fail("commit-uncertain", "Committed draft missing"))?
                    .revision;
                let mut token = evaluation.token.clone();
                token.evaluated_at_ms = self.now();
                token.context.config_revision = config_revision;
                token.context.store_revision = committed.store_revision;
                self.store_value = serde_json::to_value(&committed).expect("profile JSON");
                self.store = committed;
                if config_revision != expected_config_revision {
                    self.revoke();
                    return Err(fail("commit-uncertain","Actual config revision differs from store preview; state reconciled and disarmed"));
                }
                if let Err(e) = self
                    .controller(&evaluation.token)?
                    .acknowledge(candidate, &token)
                {
                    self.revoke();
                    return Err(fail("commit-uncertain",format!("Stored intent changed; acknowledgment failed, reconciled and disarmed: {e}")));
                }
                if let Some(review) = &mut self.armed_review {
                    review.authority.store_revision = token.context.store_revision;
                    review.authority.config_revision = token.context.config_revision;
                }
                self.decision = json!({"kind":"applied-stored-intent","storeRevision":token.context.store_revision,"configRevision":config_revision,"audioApplied":false});
                self.blocked = None;
            }
            Err(e) => {
                // Complete the failed attempt without starting hold. Reread even for
                // uncertain replacement; a changed store revokes ownership before retry.
                let mut completion = evaluation.token.clone();
                completion.evaluated_at_ms = self.now();
                let rejected = self
                    .controller(&evaluation.token)?
                    .reject(candidate, &completion);
                if rejected.is_err() {
                    self.revoke();
                }
                if let Err(reconcile) = self.sync() {
                    self.revoke();
                    return Err(reconcile);
                }
                if e.code == "commit-uncertain" {
                    self.revoke();
                }
                self.decision = json!({"kind":"write-failed"});
                return Err(e.into());
            }
        }
        Ok(())
    }
    fn check_armed(&mut self, live: &Live) -> Result<()> {
        let authority = self.authority(live)?;
        if self.armed_review.as_ref().is_none_or(|r| {
            r.authority != authority || self.inventory.as_ref() != Some(&r.inventory)
        }) {
            self.revoke();
            return Err(fail(
                "review-stale",
                "Approved settings/store/config/inventory/manual authority changed",
            ));
        }
        Ok(())
    }
    fn reevaluate(&self, live: &Live) -> Result<core::Evaluation> {
        let mut snapshot = live.capture.snapshot.clone();
        snapshot.facts.retain(|f| {
            self.settings
                .settings
                .catalog
                .iter()
                .any(|d| d.name == f.key.name)
        });
        let request = core::Request {
            graph: self
                .settings
                .settings
                .graph
                .clone()
                .ok_or_else(|| fail("graph-required", "Graph removed"))?,
            catalog: self.settings.settings.catalog.clone(),
            snapshot,
            now_ms: self.collector.now_ms()?,
            context: live.evaluation.token.context.clone(),
            manual_override: false,
        };
        Ok(core::evaluate_scoped(
            &request,
            &live.capture.application_scope,
        )?)
    }
}
fn options(settings: &settings::Settings) -> context::Options {
    context::Options {
        applications: settings.observer.applications.clone(),
        host_label: settings.observer.host_label.clone(),
        timezone: settings.observer.timezone.clone(),
        ..Default::default()
    }
}
fn normalized(config: &Config) -> Value {
    let mut value = serde_json::to_value(config).expect("config JSON");
    value["revision"] = json!(0);
    value
}
fn random_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| fail("owner-unavailable", "Review entropy unavailable"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn decision_json(decision: &core::ControllerDecision) -> Value {
    match decision {
        core::ControllerDecision::Disarmed => json!({"kind":"disarmed"}),
        core::ControllerDecision::ManualOverride => json!({"kind":"manual-override"}),
        core::ControllerDecision::NoChange => json!({"kind":"no-change"}),
        core::ControllerDecision::AlreadyApplied => json!({"kind":"already-applied"}),
        core::ControllerDecision::Waiting {
            settle_remaining_ms,
            hold_remaining_ms,
            cooldown_remaining_ms,
        } => {
            json!({"kind":"waiting","settleRemainingMs":settle_remaining_ms,"holdRemainingMs":hold_remaining_ms,"cooldownRemainingMs":cooldown_remaining_ms})
        }
        core::ControllerDecision::Candidate(_) => json!({"kind":"candidate"}),
    }
}

// Bound every reachable action's expansion before any proposal/config/path clone.
// These conservative aggregate limits admit real small local policies and reject
// excessive work explicitly; nothing is truncated or silently dropped.
fn admit(
    store: &profiles::Snapshot,
    current: &Config,
    inventory: &Inventory,
    actions: usize,
) -> Result<()> {
    let configs = store
        .profiles
        .iter()
        .map(|p| &p.config)
        .chain(std::iter::once(current));
    if store.profiles.len() > core::MAX_ITEMS || actions > core::MAX_NODES {
        return Err(fail(
            "resource-limit",
            "Too many profiles or reachable actions",
        ));
    }
    let mut entities = 0usize;
    let mut paths = 0usize;
    for config in configs {
        let counts = [
            config.sources.len(),
            config.mixes.len(),
            config.sends.len(),
            config.outputs.len(),
            config.media_controls.len(),
        ];
        if counts.iter().any(|n| *n > core::MAX_ITEMS) {
            return Err(fail(
                "resource-limit",
                "Configuration exceeds 256 items per collection",
            ));
        }
        entities = entities.saturating_add(counts.iter().sum::<usize>());
        paths = paths.saturating_add(config.sends.len().saturating_mul(config.outputs.len()));
    }
    entities = entities
        .saturating_add(inventory.sources.len())
        .saturating_add(inventory.outputs.len())
        .saturating_add(inventory.media_sessions.len());
    if entities.saturating_pow(3).saturating_mul(actions.max(1)) > 50_000_000
        || paths.saturating_mul(actions.max(1)) > 4096
    {
        return Err(fail(
            "resource-limit",
            "Aggregate automatic review/preview work exceeds admission budget",
        ));
    }
    // Serialization here is linear, before multiplicative planner/path expansion.
    let bytes = serde_json::to_vec(store)
        .map_err(|_| fail("resource-limit", "Store cannot serialize"))?
        .len();
    let copies = entities
        .saturating_add(paths)
        .saturating_add(1)
        .saturating_mul(actions.max(1));
    if bytes.saturating_mul(copies).saturating_mul(32) > 32 * 1024 * 1024 {
        return Err(fail(
            "resource-limit",
            "Aggregate automatic review/preview exceeds 32 MiB expansion budget",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn prepared() -> (
        Fixture,
        Engine,
        core::Evaluation,
        core::TransitionCandidate,
        Config,
        u64,
    ) {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "mixyard-owner-completion-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let fixture = Fixture(root);
        let profiles = profiles::Store::new(fixture.0.join("profiles")).unwrap();
        let config:Config=serde_json::from_value(json!({"schemaVersion":2,"revision":0,"sources":[],"mixes":[{"id":"desk","name":"Desk","channels":2}],"sends":[],"outputs":[]})).unwrap();
        profiles.edit(0, profiles::Edit::Draft(config)).unwrap();
        let store = settings::Store::sibling_of_profiles(fixture.0.join("profiles")).unwrap();
        let graph:core::Graph=serde_json::from_value(json!({"schemaVersion":1,"id":"completion-fixture","name":"Completion","revision":1,"root":"select","nodes":[{"kind":"terminal","id":"select","action":{"kind":"select","overrides":[{"scope":"mix","mix":"desk","gain":0.25,"mute":null}]}}]})).unwrap();
        let settings = settings::Settings {
            graph: Some(graph),
            catalog: core::standard_catalog(),
            transition_policy: settings::TransitionPolicy {
                settle_ms: settings::Milliseconds(0),
                minimum_hold_ms: settings::Milliseconds(10_000),
                cooldown_ms: settings::Milliseconds(100),
            },
            ..Default::default()
        };
        store.write(0, &settings).unwrap();
        let mut engine = Engine::new(profiles, store, "completion-fixture".into()).unwrap();
        let request = core::Request {
            graph: settings.graph.unwrap(),
            catalog: settings.catalog,
            snapshot: core::Snapshot {
                schema_version: 1,
                revision: 1,
                epoch: "owned-unit-fixture".into(),
                facts: vec![],
            },
            now_ms: engine.now(),
            context: core::Context {
                config_revision: engine.store.draft.as_ref().unwrap().revision,
                store_revision: 1,
                inventory_revision: 1,
                manual_revision: 0,
            },
            manual_override: false,
        };
        let evaluation = core::evaluate(&request).unwrap();
        let inventory:Inventory=serde_json::from_value(json!({"schemaVersion":2,"revision":1,"evidence":"simulated","platform":"linux","sources":[],"outputs":[]})).unwrap();
        let proposal = core::preview_proposal(
            &evaluation,
            &evaluation.token,
            engine.store.draft.as_ref().unwrap(),
            &[],
            &inventory,
        )
        .unwrap();
        let config = proposal.candidate.unwrap();
        let revision = profiles::preview(&engine.store, &config, &inventory)
            .unwrap()
            .plan
            .config_revision;
        let controller = engine.controller(&evaluation.token).unwrap();
        controller.arm(&evaluation.token).unwrap();
        let core::ControllerDecision::Candidate(candidate) =
            controller.observe(&evaluation, &evaluation.token).unwrap()
        else {
            panic!("expected actual evaluator candidate")
        };
        controller.guard(&candidate, &evaluation.token).unwrap();
        (fixture, engine, evaluation, candidate, config, revision)
    }
    #[test]
    fn uncertain_completion_after_real_replacement_reconciles_and_requires_new_review() {
        let (_fixture, mut engine, evaluation, candidate, config, revision) = prepared();
        let committed = engine
            .profiles
            .edit(1, profiles::Edit::Draft(config))
            .unwrap();
        assert_eq!(committed.draft.as_ref().unwrap().revision, revision);
        // Inject only the external completion report after a real atomic write,
        // representing a failed directory sync/lock cleanup, not a fake write.
        let error = profiles::Error {
            code: "commit-uncertain",
            message: "test: completion lost after real replacement".into(),
            exit_code: 4,
        };
        assert_eq!(
            engine
                .complete_write(Err(error), &evaluation, &candidate, revision)
                .unwrap_err()
                .code,
            "commit-uncertain"
        );
        assert_eq!(engine.store.store_revision, 2);
        assert_eq!(
            engine.store.draft.as_ref().unwrap().mixes[0].master.gain,
            0.25
        );
        assert!(!engine.controller.as_ref().unwrap().is_armed());
        assert_eq!(
            engine.controller.as_ref().unwrap().last_applied_at_ms(),
            None
        );
        assert!(matches!(
            engine.manual,
            Some(core::ManualOverride::UntilCleared)
        ));
        assert!(engine.review.is_none() && engine.armed_review.is_none());
        let bytes = std::fs::read(_fixture.0.join("profiles/profiles.json")).unwrap();
        engine.sync().unwrap();
        assert_eq!(
            std::fs::read(_fixture.0.join("profiles/profiles.json")).unwrap(),
            bytes
        );
    }
    #[test]
    fn actual_expected_revision_conflict_is_reconciled_without_overwriting_manual_draft() {
        let (_fixture, mut engine, evaluation, candidate, config, revision) = prepared();
        let mut manual = config.clone();
        manual.mixes[0].master.gain = 0.7;
        engine
            .profiles
            .edit(1, profiles::Edit::Draft(manual))
            .unwrap();
        let result = engine.profiles.edit(1, profiles::Edit::Draft(config));
        assert_eq!(result.as_ref().unwrap_err().code, "store-conflict");
        assert_eq!(
            engine
                .complete_write(result, &evaluation, &candidate, revision)
                .unwrap_err()
                .code,
            "store-conflict"
        );
        assert_eq!(engine.store.store_revision, 2);
        assert_eq!(
            engine.store.draft.as_ref().unwrap().mixes[0].master.gain,
            0.7
        );
        assert!(!engine.controller.as_ref().unwrap().is_armed());
        assert_eq!(
            engine.controller.as_ref().unwrap().last_applied_at_ms(),
            None
        );
        assert!(matches!(
            engine.manual,
            Some(core::ManualOverride::UntilCleared)
        ));
    }
    #[test]
    fn expired_or_failed_lifecycle_commands_cannot_restore_automatic_ownership() {
        for command in [
            Command::Disarm,
            Command::Manual(Manual::For { duration_ms: 100 }),
            Command::Edit(1, profiles::Edit::Undo),
        ] {
            let (_fixture, mut engine, _, _, _, _) = prepared();
            let shared = Shared {
                commit: Mutex::new(()),
                cancelled: AtomicBool::new(true),
                stopped: AtomicBool::new(false),
                status: Mutex::new(engine.status()),
                slow: AtomicBool::new(false),
                observing: AtomicBool::new(false),
            };
            let (tx, rx) = mpsc::sync_channel(1);
            engine.handle_envelope(
                Envelope {
                    command,
                    deadline: Instant::now() - Duration::from_millis(1),
                    reply: tx,
                },
                &shared,
            );
            assert_eq!(rx.recv().unwrap().unwrap_err().code, "operation-timeout");
            assert!(!engine.controller.as_ref().unwrap().is_armed());
            assert_eq!(engine.profiles.read().unwrap().store_revision, 1);
        }
        let (fixture, mut engine, _, _, _, _) = prepared();
        std::fs::write(
            fixture.0.join("situation-settings/settings.json"),
            "corrupt",
        )
        .unwrap();
        let shared = Shared {
            commit: Mutex::new(()),
            cancelled: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            status: Mutex::new(engine.status()),
            slow: AtomicBool::new(false),
            observing: AtomicBool::new(false),
        };
        let (tx, rx) = mpsc::sync_channel(1);
        engine.handle_envelope(
            Envelope {
                command: Command::Manual(Manual::UntilCleared),
                deadline: Instant::now() + Duration::from_secs(1),
                reply: tx,
            },
            &shared,
        );
        assert!(rx.recv().unwrap().is_err());
        assert!(!engine.controller.as_ref().unwrap().is_armed());
        assert_eq!(engine.profiles.read().unwrap().store_revision, 1);
    }
}
