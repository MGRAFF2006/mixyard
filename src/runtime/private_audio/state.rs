//! Parent-owned authority/receipt transitions. Native effects remain in the owned
//! companion; these checks cannot turn a test double into native qualification.
use super::protocol::*;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, process::Child};

fn failure(code: ErrorCode, message: &str) -> Failure {
    Failure {
        code,
        message: message.chars().take(512).collect(),
    }
}

#[derive(Clone, Debug)]
pub struct ReleasePermit {
    nonce: String,
    operation: String,
    stop_complete: bool,
}
impl ReleasePermit {
    pub fn allows_guard_release(&self) -> bool {
        true
    }
    pub fn allows_stop_complete(&self) -> bool {
        self.stop_complete
    }
    pub fn instance_nonce(&self) -> &str {
        &self.nonce
    }
    pub fn operation_id(&self) -> &str {
        &self.operation
    }
}

#[derive(Debug)]
pub enum Transition {
    Ready,
    Reserved,
    Prepared,
    Linked,
    Staged,
    Installed(ReleasePermit),
    Fenced(ReleasePermit),
    Snapshot,
    Fault,
}

#[derive(Clone)]
struct Stage {
    operation: String,
    generation: u64,
    config_revision: u64,
    inventory_revision: u64,
    request_digest: String,
    endpoint_digest: String,
}

pub struct Coordinator {
    status: Status,
    next_operation: u64,
    next_generation: u64,
    snapshot: Option<DesiredSnapshot>,
    reserved_generation: Option<u64>,
    expected_native: Option<CompanionRequest>,
    reservation_digest: Option<String>,
    native_reserved: bool,
    stage: Option<Stage>,
    native_stage_token: Option<String>,
    stopping: Option<(StopTarget, CompanionRequest)>,
    fenced: Option<Fenced>,
    late_install: Option<Installed>,
    children: Option<(u32, u32)>,
    forced: bool,
}

impl Coordinator {
    pub fn new(nonce: String) -> Result<Self, Failure> {
        if !hex_digest(&nonce) {
            return Err(failure(ErrorCode::ProtocolError, "invalid instance nonce"));
        }
        Ok(Self {
            status: Status {
                scope: Scope::OwnedPrivateGeneratedV1,
                private_core: None,
                phase: Phase::Idle,
                stop_target: StopTarget {
                    instance_nonce: nonce,
                    control_epoch: 1,
                    active_generation: None,
                    pending_install: None,
                },
                desired_store_revision: None,
                desired_config_revision: None,
                inventory_revision: None,
                prepared_token: None,
                installed: None,
                outputs: vec![],
                pending_operation: None,
                last_error: None,
            },
            next_operation: 1,
            next_generation: 1,
            snapshot: None,
            reserved_generation: None,
            expected_native: None,
            reservation_digest: None,
            native_reserved: false,
            stage: None,
            native_stage_token: None,
            stopping: None,
            fenced: None,
            late_install: None,
            children: None,
            forced: false,
        })
    }
    pub fn snapshot(&self) -> Status {
        self.status.clone()
    }
    pub fn reserve_request(
        &mut self,
        request: &CompanionRequest,
    ) -> Result<CompanionRequest, Failure> {
        let expected = self
            .expected_native
            .as_ref()
            .ok_or_else(|| failure(ErrorCode::ProtocolError, "no pending native command"))?;
        if request.operation_id != expected.operation_id || self.reservation_digest.is_some() {
            return Err(failure(ErrorCode::Busy, "native reservation already owned"));
        }
        let action_kind = match &request.action {
            CompanionAction::Prepare { .. } => "prepare",
            CompanionAction::Link { .. } => "link",
            CompanionAction::Stage { .. } => "stage",
            _ => {
                return Err(failure(
                    ErrorCode::ProtocolError,
                    "action does not use a reservation",
                ))
            }
        };
        let bytes = serde_json::to_vec(request)
            .map_err(|_| failure(ErrorCode::ProtocolError, "request encoding failed"))?;
        if bytes
            != serde_json::to_vec(expected)
                .map_err(|_| failure(ErrorCode::ProtocolError, "request encoding failed"))?
        {
            return Err(failure(
                ErrorCode::ProtocolError,
                "request differs from owned operation",
            ));
        }
        let digest = format!("{:x}", Sha256::digest(bytes));
        self.reservation_digest = Some(digest.clone());
        self.native_reserved = false;
        Ok(self.request(
            request.operation_id.clone(),
            CompanionAction::Reserve {
                action_kind: action_kind.into(),
                reserved_generation: self.reserved_generation.unwrap(),
                request_digest: digest,
            },
        ))
    }
    pub fn register_private_core(&mut self, core: PrivateCore) -> Result<(), Failure> {
        if self.children.is_none()
            || self.status.private_core.is_some()
            || !core.runtime_directory.is_absolute()
            || !core.config_directory.is_absolute()
            || core.remote != "mixyard-private"
        {
            return Err(failure(
                ErrorCode::OwnerFailed,
                "private core registration changed",
            ));
        }
        self.status.private_core = Some(core);
        Ok(())
    }
    pub fn check_instance(&self, nonce: &str) -> Result<(), Failure> {
        if nonce != self.status.stop_target.instance_nonce {
            Err(failure(
                ErrorCode::StaleInstance,
                "runtime instance changed",
            ))
        } else {
            Ok(())
        }
    }
    pub fn fail_held(&mut self, code: ErrorCode, message: &str) {
        self.status.phase = Phase::FailedHeld;
        self.status.last_error = Some(failure(code, message));
    }
    fn err<T>(&mut self, code: ErrorCode, text: &str) -> Result<T, Failure> {
        self.fail_held(code, text);
        Err(failure(code, text))
    }
    fn operation(&mut self) -> Result<String, Failure> {
        let value = self.next_operation;
        let Some(next) = value.checked_add(1) else {
            return self.err(ErrorCode::ResourceLimit, "operation counter exhausted");
        };
        self.next_operation = next;
        Ok(format!("op-{value}"))
    }
    fn request(&self, operation: String, action: CompanionAction) -> CompanionRequest {
        CompanionRequest {
            companion_version: VERSION,
            instance_nonce: self.status.stop_target.instance_nonce.clone(),
            control_epoch: self.status.stop_target.control_epoch,
            operation_id: operation,
            action,
        }
    }
    fn idle_mutator(&self) -> Result<(), Failure> {
        if self.status.pending_operation.is_some() || self.stopping.is_some() {
            Err(failure(ErrorCode::Busy, "one owned mutation is pending"))
        } else {
            Ok(())
        }
    }
    pub fn begin_prepare(
        &mut self,
        snapshot: DesiredSnapshot,
    ) -> Result<CompanionRequest, Failure> {
        self.idle_mutator()?;
        if !matches!(
            self.status.phase,
            Phase::Idle | Phase::Stopped | Phase::FailedStopped
        ) {
            return Err(failure(
                ErrorCode::StopRequired,
                "stop the current private graph before Prepare",
            ));
        }
        validate_snapshot(&snapshot)?;
        let generation = self.next_generation;
        let Some(next) = generation.checked_add(1) else {
            return self.err(ErrorCode::ResourceLimit, "generation exhausted");
        };
        let op = self.operation()?;
        self.next_generation = next;
        self.snapshot = Some(snapshot.clone());
        self.reserved_generation = Some(generation);
        self.stage = None;
        self.native_stage_token = None;
        self.fenced = None;
        self.late_install = None;
        self.forced = false;
        self.status.phase = Phase::Preparing;
        self.status.prepared_token = None;
        self.status.outputs.clear();
        self.status.desired_store_revision = Some(snapshot.store_revision);
        self.status.desired_config_revision = Some(snapshot.config_revision);
        self.status.pending_operation = Some(op.clone());
        self.status.stop_target.pending_install = Some(PendingInstall {
            operation_id: op.clone(),
            reserved_generation: generation,
        });
        self.status.last_error = None;
        let request = self.request(
            op,
            CompanionAction::Prepare {
                generation,
                snapshot,
            },
        );
        self.expected_native = Some(request.clone());
        Ok(request)
    }
    pub fn begin_link(
        &mut self,
        nonce: &str,
        token: &str,
        output: String,
        consumer: Endpoint,
    ) -> Result<CompanionRequest, Failure> {
        self.check_instance(nonce)?;
        self.idle_mutator()?;
        self.prepared(token)?;
        if !endpoint_valid(&consumer)
            || !self.status.outputs.iter().any(|o| o.output_id == output)
            || self
                .status
                .outputs
                .iter()
                .any(|o| o.endpoint.node_id == consumer.node_id)
        {
            return Err(failure(
                ErrorCode::EndpointChanged,
                "invalid, absent or recapturing consumer endpoint",
            ));
        }
        let op = self.operation()?;
        self.status.pending_operation = Some(op.clone());
        self.status.stop_target.pending_install = Some(PendingInstall {
            operation_id: op.clone(),
            reserved_generation: self.reserved_generation.unwrap(),
        });
        let request = self.request(
            op,
            CompanionAction::Link {
                prepared_token: token.into(),
                output_id: output,
                consumer,
            },
        );
        self.expected_native = Some(request.clone());
        Ok(request)
    }
    fn prepared(&self, token: &str) -> Result<(), Failure> {
        if self.status.phase != Phase::Prepared
            || self.status.prepared_token.as_deref() != Some(token)
        {
            Err(failure(
                ErrorCode::StaleGeneration,
                "prepared token/state changed",
            ))
        } else {
            Ok(())
        }
    }
    pub fn begin_stage(
        &mut self,
        nonce: &str,
        token: &str,
        store: u64,
        config: u64,
        inventory: u64,
    ) -> Result<CompanionRequest, Failure> {
        self.check_instance(nonce)?;
        self.idle_mutator()?;
        self.prepared(token)?;
        if self.status.desired_store_revision != Some(store) {
            return Err(failure(ErrorCode::StaleStore, "desired store changed"));
        }
        if self.status.desired_config_revision != Some(config) {
            return Err(failure(ErrorCode::StaleConfig, "desired config changed"));
        }
        if self.status.inventory_revision != Some(inventory) {
            return Err(failure(
                ErrorCode::StaleInventory,
                "private inventory changed",
            ));
        }
        if self.status.outputs.is_empty()
            || self
                .status
                .outputs
                .iter()
                .any(|o| o.linked_consumer.is_none())
        {
            return Err(failure(
                ErrorCode::EndpointChanged,
                "every configured output requires its explicit consumer link",
            ));
        }
        let op = self.operation()?;
        self.status.phase = Phase::Installing;
        self.status.pending_operation = Some(op.clone());
        self.status.stop_target.pending_install = Some(PendingInstall {
            operation_id: op.clone(),
            reserved_generation: self.reserved_generation.unwrap(),
        });
        let request = self.request(
            op,
            CompanionAction::Stage {
                prepared_token: token.into(),
                expected_inventory_revision: inventory,
            },
        );
        self.expected_native = Some(request.clone());
        Ok(request)
    }
    pub fn begin_install(
        &mut self,
        current: &DesiredSnapshot,
    ) -> Result<CompanionRequest, Failure> {
        validate_snapshot(current)?;
        if self.status.phase != Phase::Installing
            || self.expected_native.is_some()
            || self.stopping.is_some()
        {
            return Err(failure(
                ErrorCode::Busy,
                "native Stage has not completed or Stop is pending",
            ));
        }
        let expected = self
            .snapshot
            .as_ref()
            .ok_or_else(|| failure(ErrorCode::ProtocolError, "no desired snapshot"))?;
        if current.store_revision != expected.store_revision {
            return Err(failure(ErrorCode::StaleStore, "guarded store changed"));
        }
        if current.config_revision != expected.config_revision
            || current.config_sha256 != expected.config_sha256
        {
            return Err(failure(
                ErrorCode::StaleConfig,
                "guarded desired config changed",
            ));
        }
        let stage = self
            .stage
            .as_ref()
            .ok_or_else(|| failure(ErrorCode::ProtocolError, "no staged host"))?;
        let request = self.request(
            stage.operation.clone(),
            CompanionAction::Install {
                staged_token: self.staged_token()?,
            },
        );
        self.expected_native = Some(request.clone());
        Ok(request)
    }
    fn staged_token(&self) -> Result<String, Failure> {
        // Stage token is kept separately from the publicly exposed Prepare token.
        self.native_stage_token
            .clone()
            .ok_or_else(|| failure(ErrorCode::ProtocolError, "missing stage token"))
    }
    pub fn begin_stop(&mut self, expected: &StopTarget) -> Result<CompanionRequest, Failure> {
        self.check_instance(&expected.instance_nonce)?;
        if &self.status.stop_target != expected {
            return Err(failure(ErrorCode::StaleControlEpoch, "Stop target changed"));
        }
        if self.stopping.is_some() {
            return Err(failure(ErrorCode::Busy, "the exact Stop is already owned"));
        }
        let Some(epoch) = expected.control_epoch.checked_add(1) else {
            return self.err(ErrorCode::ResourceLimit, "control epoch exhausted");
        };
        let operation = format!("fence-{epoch}");
        let request = CompanionRequest {
            companion_version: VERSION,
            instance_nonce: expected.instance_nonce.clone(),
            control_epoch: expected.control_epoch,
            operation_id: operation.clone(),
            action: CompanionAction::Fence {
                expected: expected.clone(),
                new_control_epoch: epoch,
            },
        };
        self.stopping = Some((expected.clone(), request.clone()));
        self.expected_native = None;
        self.fenced = None;
        self.status.phase = Phase::Stopping;
        self.status.stop_target.control_epoch = epoch;
        self.status.pending_operation = Some(operation);
        self.status.prepared_token = None;
        Ok(request)
    }
    pub fn register_children(&mut self, companion: &Child, server: &Child) -> Result<(), Failure> {
        if self.children.is_some()
            || companion.id() == server.id()
            || companion.id() == 0
            || server.id() == 0
        {
            return Err(failure(
                ErrorCode::OwnerFailed,
                "owned child registration changed",
            ));
        }
        self.children = Some((companion.id(), server.id()));
        self.forced = false;
        Ok(())
    }
    pub fn note_forced_termination(&mut self) {
        self.forced = true;
    }
    pub fn prove_children_reaped(
        &mut self,
        companion: &mut Child,
        server: &mut Child,
    ) -> Result<ReleasePermit, Failure> {
        if self.children != Some((companion.id(), server.id())) {
            return self.err(ErrorCode::OwnerFailed, "different child handles");
        }
        let a = companion
            .try_wait()
            .map_err(|_| failure(ErrorCode::OwnerFailed, "companion reap unknown"))?;
        let b = server
            .try_wait()
            .map_err(|_| failure(ErrorCode::OwnerFailed, "server reap unknown"))?;
        let (Some(a), Some(b)) = (a, b) else {
            return self.err(ErrorCode::StopIncomplete, "owned children not both reaped");
        };
        #[cfg(unix)]
        let server_normal = {
            use std::os::unix::process::ExitStatusExt;
            b.success() || b.signal() == Some(libc::SIGTERM)
        };
        #[cfg(not(unix))]
        let server_normal = b.success();
        let complete = self.fenced.is_some() && !self.forced && a.success() && server_normal;
        let operation = self
            .status
            .pending_operation
            .clone()
            .unwrap_or_else(|| "retired-after-owner-failure".into());
        let permit = ReleasePermit {
            nonce: self.status.stop_target.instance_nonce.clone(),
            operation,
            stop_complete: complete,
        };
        self.status.phase = if complete {
            Phase::Stopped
        } else {
            Phase::FailedStopped
        };
        if !complete {
            self.status.last_error = Some(failure(
                ErrorCode::OwnerFailed,
                "processes reaped; native scrub/normal StopComplete unverified",
            ));
        }
        self.status.installed = None;
        self.status.private_core = None;
        self.status.prepared_token = None;
        self.status.outputs.clear();
        self.status.stop_target.active_generation = None;
        self.status.stop_target.pending_install = None;
        self.status.pending_operation = None;
        self.status.inventory_revision = None;
        self.expected_native = None;
        self.stage = None;
        self.reservation_digest = None;
        self.native_reserved = false;
        self.native_stage_token = None;
        self.stopping = None;
        self.reserved_generation = None;
        self.children = None;
        Ok(permit)
    }
    fn validate_outputs(&self, outputs: &[PublishedOutput]) -> Result<(), Failure> {
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or_else(|| failure(ErrorCode::ProtocolError, "no prepared config"))?;
        if outputs.len() != snapshot.config.outputs.len() || outputs.is_empty() {
            return Err(failure(ErrorCode::ProtocolError, "partial output set"));
        }
        let mut ids = BTreeSet::new();
        let mut endpoints = BTreeSet::new();
        for output in outputs {
            let expected = snapshot
                .config
                .outputs
                .iter()
                .find(|o| o.id == output.output_id)
                .ok_or_else(|| failure(ErrorCode::ProtocolError, "unknown canonical output"))?;
            if !ids.insert(output.output_id.as_str())
                || !endpoints.insert((output.endpoint.node_id, output.endpoint.port_id))
                || expected.mix != output.mix_id
                || !endpoint_valid(&output.endpoint)
                || !output.format.supported()
                || output.label.len() > 256
                || output
                    .linked_consumer
                    .as_ref()
                    .is_some_and(|p| !endpoint_valid(p))
            {
                return Err(failure(
                    ErrorCode::ProtocolError,
                    "invalid canonical endpoint/format set",
                ));
            }
        }
        Ok(())
    }
    fn validate_installed(
        &self,
        receipt: &Installed,
        epoch: u64,
        operation: &str,
    ) -> Result<(), Failure> {
        let stage = self
            .stage
            .as_ref()
            .ok_or_else(|| failure(ErrorCode::ProtocolError, "install without completed Stage"))?;
        let snapshot = self.snapshot.as_ref().unwrap();
        let expected: BTreeSet<_> = self
            .status
            .outputs
            .iter()
            .map(|o| o.output_id.as_str())
            .collect();
        let actual: BTreeSet<_> = receipt
            .output_commits
            .iter()
            .map(|o| o.output_id.as_str())
            .collect();
        if receipt.instance_nonce != self.status.stop_target.instance_nonce
            || receipt.control_epoch != epoch
            || receipt.operation_id != operation
            || receipt.operation_id != stage.operation
            || receipt.generation != stage.generation
            || receipt.config_revision != stage.config_revision
            || receipt.inventory_revision != stage.inventory_revision
            || receipt.store_revision != snapshot.store_revision
            || receipt.request_digest != stage.request_digest
            || receipt.endpoint_set_digest != stage.endpoint_digest
            || receipt.clock_id == 0
            || receipt.engine_first_frame != 0
            || !receipt.owner_install_terminal
            || !receipt.no_late_install
            || receipt.first_processed_frame < receipt.native_switch_frame
            || expected != actual
            || actual.len() != receipt.output_commits.len()
            || receipt
                .output_commits
                .iter()
                .any(|o| o.native_frame < receipt.native_switch_frame)
        {
            return Err(failure(
                ErrorCode::ProtocolError,
                "actual owner install receipt binding/terminal/commit proof missing",
            ));
        }
        Ok(())
    }
    pub fn accept(&mut self, reply: CompanionReply) -> Result<Transition, Failure> {
        if reply.companion_version != VERSION || self.check_instance(&reply.instance_nonce).is_err()
        {
            return self.err(
                ErrorCode::StaleInstance,
                "native reply instance/version mismatch",
            );
        }
        // Only the exact canceled operation can have a legitimate late reply.
        if let Some((old, _)) = &self.stopping {
            if reply.control_epoch == old.control_epoch
                && reply.operation_id == "startup"
                && self.children.is_some()
                && matches!(&reply.event, CompanionEvent::Ready { .. })
            {
                return Ok(Transition::Snapshot);
            }
            if reply.control_epoch == old.control_epoch
                && old
                    .pending_install
                    .as_ref()
                    .is_some_and(|p| p.operation_id == reply.operation_id)
            {
                match &reply.event {
                    CompanionEvent::Installed { receipt } => {
                        self.validate_installed(receipt, old.control_epoch, &reply.operation_id)?;
                        self.late_install = Some(receipt.clone());
                        return Ok(Transition::Snapshot);
                    }
                    CompanionEvent::Prepared { .. }
                    | CompanionEvent::Reserved { .. }
                    | CompanionEvent::Linked { .. }
                    | CompanionEvent::Staged { .. } => return Ok(Transition::Snapshot),
                    _ => {
                        return self.err(ErrorCode::ProtocolError, "unexpected late canceled event")
                    }
                }
            }
        }
        if reply.control_epoch != self.status.stop_target.control_epoch {
            return self.err(ErrorCode::StaleControlEpoch, "native reply epoch mismatch");
        }
        if let CompanionEvent::Reserved {
            reserved_generation,
            request_digest,
        } = &reply.event
        {
            if self.native_reserved
                || self.reservation_digest.as_ref() != Some(request_digest)
                || Some(*reserved_generation) != self.reserved_generation
                || self
                    .expected_native
                    .as_ref()
                    .is_none_or(|r| r.operation_id != reply.operation_id)
            {
                return self.err(
                    ErrorCode::ProtocolError,
                    "native reservation binding changed",
                );
            }
            self.native_reserved = true;
            return Ok(Transition::Reserved);
        }
        if let CompanionEvent::Ready {
            inventory_revision,
            endpoint_set_digest,
        } = &reply.event
        {
            if self.children.is_none()
                || reply.operation_id != "startup"
                || !matches!(
                    self.status.phase,
                    Phase::Idle | Phase::Stopped | Phase::FailedStopped | Phase::Preparing
                )
                || *inventory_revision == 0
                || !hex_digest(endpoint_set_digest)
            {
                return self.err(ErrorCode::ProtocolError, "unexpected native Ready");
            }
            if self.status.phase != Phase::Preparing {
                self.status.phase = Phase::Idle;
            }
            self.status.inventory_revision = Some(
                self.status
                    .inventory_revision
                    .unwrap_or(0)
                    .max(*inventory_revision),
            );
            return Ok(Transition::Ready);
        }
        if let CompanionEvent::Fault { error } = &reply.event {
            self.fail_held(error.code, &error.message);
            return Ok(Transition::Fault);
        }
        if let CompanionEvent::Fenced { receipt } = &reply.event {
            let (old, request) = self
                .stopping
                .as_ref()
                .ok_or_else(|| failure(ErrorCode::ProtocolError, "unsolicited fence"))?;
            if reply.operation_id != request.operation_id
                || receipt.instance_nonce != reply.instance_nonce
                || receipt.control_epoch != reply.control_epoch
                || receipt.operation_id != reply.operation_id
                || !receipt.owner_quiesced
                || !receipt.pending_pcm_zero
                || !receipt.no_late_install
            {
                return self.err(ErrorCode::ProtocolError, "owner fence proof incomplete");
            }
            match receipt.outcome {
                FenceOutcome::CanceledBeforeSwitch
                    if old.pending_install.is_some()
                        && receipt.switched.is_none()
                        && receipt.switch_evidence.is_none()
                        && self.late_install.is_none() => {}
                FenceOutcome::NoPending
                    if old.pending_install.is_none()
                        && receipt.switch_evidence.is_none()
                        && self.late_install.is_none()
                        && receipt
                            .switched
                            .as_ref()
                            .is_none_or(|r| self.status.installed.as_ref() == Some(r)) => {}
                FenceOutcome::AbortedAfterSwitch => {
                    let pending = old.pending_install.as_ref().ok_or_else(|| {
                        failure(ErrorCode::ProtocolError, "switch without pending target")
                    })?;
                    let switch = receipt.switch_evidence.as_ref().ok_or_else(|| {
                        failure(ErrorCode::ProtocolError, "missing partial switch evidence")
                    })?;
                    let stage = self
                        .stage
                        .as_ref()
                        .ok_or_else(|| failure(ErrorCode::ProtocolError, "switch before Stage"))?;
                    if switch.instance_nonce != old.instance_nonce
                        || switch.control_epoch != old.control_epoch
                        || switch.operation_id != pending.operation_id
                        || switch.generation != pending.reserved_generation
                        || switch.generation != stage.generation
                        || switch.config_revision != stage.config_revision
                        || switch.inventory_revision != stage.inventory_revision
                        || switch.clock_id == 0
                    {
                        return self.err(
                            ErrorCode::ProtocolError,
                            "partial switch does not bind canceled install",
                        );
                    }
                    if let Some(complete) = &receipt.switched {
                        self.validate_installed(
                            complete,
                            old.control_epoch,
                            &pending.operation_id,
                        )?;
                        if complete.native_switch_frame != switch.native_switch_frame
                            || complete.clock_id != switch.clock_id
                        {
                            return self.err(
                                ErrorCode::ProtocolError,
                                "switch evidence contradicts completed receipt",
                            );
                        }
                    }
                    if self
                        .late_install
                        .as_ref()
                        .is_some_and(|late| receipt.switched.as_ref() != Some(late))
                    {
                        return self.err(
                            ErrorCode::ProtocolError,
                            "completed late switch omitted/contradicted by fence",
                        );
                    }
                }
                _ => {
                    return self.err(
                        ErrorCode::ProtocolError,
                        "fence outcome contradicts observed switch",
                    )
                }
            }
            self.fenced = Some(receipt.clone());
            self.expected_native = None;
            self.status.installed = None;
            self.status.stop_target.active_generation = None;
            // Child reaping still required; a fence permit alone is NOT StopComplete.
            return Ok(Transition::Fenced(ReleasePermit {
                nonce: reply.instance_nonce,
                operation: reply.operation_id,
                stop_complete: false,
            }));
        }
        if matches!(&reply.event, CompanionEvent::Stopped {})
            && self.fenced.is_some()
            && self.status.phase == Phase::Stopping
            && self.status.pending_operation.as_deref() == Some(&reply.operation_id)
        {
            return Ok(Transition::Snapshot);
        }
        let expected = self
            .expected_native
            .clone()
            .ok_or_else(|| failure(ErrorCode::ProtocolError, "no expected native operation"))?;
        if expected.operation_id != reply.operation_id {
            return self.err(ErrorCode::ProtocolError, "native operation mismatch");
        }
        if !self.native_reserved {
            return self.err(
                ErrorCode::ProtocolError,
                "native command completed without reservation",
            );
        }
        match (expected.action, reply.event) {
            (
                CompanionAction::Prepare { .. },
                CompanionEvent::Prepared {
                    prepared_token,
                    inventory_revision,
                    outputs,
                },
            ) => {
                self.validate_outputs(&outputs)?;
                if !hex_digest(&prepared_token) || inventory_revision == 0 {
                    return self.err(ErrorCode::ProtocolError, "invalid preparation receipt");
                }
                self.status.prepared_token = Some(prepared_token);
                self.status.inventory_revision = Some(inventory_revision);
                self.status.outputs = outputs;
                self.status.phase = Phase::Prepared;
                self.finish_mutator();
                Ok(Transition::Prepared)
            }
            (
                CompanionAction::Link {
                    output_id,
                    consumer,
                    ..
                },
                CompanionEvent::Linked {
                    inventory_revision,
                    outputs,
                },
            ) => {
                self.validate_outputs(&outputs)?;
                if inventory_revision == 0
                    || !outputs.iter().any(|o| {
                        o.output_id == output_id && o.linked_consumer.as_ref() == Some(&consumer)
                    })
                    || outputs.iter().any(|o| {
                        !self.status.outputs.iter().any(|p| {
                            p.output_id == o.output_id
                                && p.endpoint == o.endpoint
                                && (o.output_id == output_id
                                    || p.linked_consumer == o.linked_consumer)
                        })
                    })
                {
                    return self.err(
                        ErrorCode::EndpointChanged,
                        "link receipt changed endpoint set or omitted exact consumer",
                    );
                }
                self.status.outputs = outputs;
                self.status.inventory_revision = Some(inventory_revision);
                self.finish_mutator();
                Ok(Transition::Linked)
            }
            (
                CompanionAction::Stage { .. },
                CompanionEvent::Staged {
                    staged_token,
                    generation,
                    config_revision,
                    inventory_revision,
                    request_digest,
                    endpoint_set_digest,
                },
            ) => {
                if !hex_digest(&staged_token)
                    || !hex_digest(&request_digest)
                    || !hex_digest(&endpoint_set_digest)
                    || Some(generation) != self.reserved_generation
                    || Some(config_revision) != self.status.desired_config_revision
                    || Some(inventory_revision) != self.status.inventory_revision
                {
                    return self.err(ErrorCode::ProtocolError, "stage receipt binding changed");
                }
                self.stage = Some(Stage {
                    operation: reply.operation_id,
                    generation,
                    config_revision,
                    inventory_revision,
                    request_digest,
                    endpoint_digest: endpoint_set_digest,
                });
                self.native_stage_token = Some(staged_token);
                self.expected_native = None;
                Ok(Transition::Staged)
            }
            (CompanionAction::Install { .. }, CompanionEvent::Installed { receipt }) => {
                self.validate_installed(&receipt, reply.control_epoch, &reply.operation_id)?;
                self.status.installed = Some(receipt.clone());
                self.status.stop_target.active_generation = Some(receipt.generation);
                self.status.phase = Phase::Running;
                self.finish_mutator();
                Ok(Transition::Installed(ReleasePermit {
                    nonce: reply.instance_nonce,
                    operation: reply.operation_id,
                    stop_complete: false,
                }))
            }
            _ => self.err(ErrorCode::ProtocolError, "unexpected native event kind"),
        }
    }
    fn finish_mutator(&mut self) {
        self.status.pending_operation = None;
        self.status.stop_target.pending_install = None;
        self.expected_native = None;
        self.reservation_digest = None;
        self.native_reserved = false;
    }
}

fn endpoint_valid(value: &Endpoint) -> bool {
    value.node_id != 0 && value.port_id != 0 && value.node_serial != 0 && value.port_serial != 0
}

pub fn validate_snapshot(snapshot: &DesiredSnapshot) -> Result<(), Failure> {
    if snapshot.config_revision == 0
        || snapshot.config.revision != snapshot.config_revision
        || !hex_digest(&snapshot.config_sha256)
        || snapshot.config.validate().is_err()
    {
        return Err(failure(
            ErrorCode::StaleConfig,
            "invalid desired snapshot/revision",
        ));
    }
    let bytes = serde_json::to_vec(&snapshot.config)
        .map_err(|_| failure(ErrorCode::ProtocolError, "config serialization failed"))?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != snapshot.config_sha256 {
        return Err(failure(
            ErrorCode::StaleConfig,
            "config wire digest mismatch",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
