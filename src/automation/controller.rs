//! In-memory transition timing, without observation, storage or write authority.
//! The owner must serialize fresh evaluation, guard, external commit and acknowledgment.

use super::{validate, Action, Context, Error, Evaluation, StateToken};
use std::sync::Arc;

/// Durations use the owner's monotonic milliseconds, never wall-clock time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransitionPolicy {
    pub settle_ms: u64,
    pub minimum_hold_ms: u64,
    /// Minimum interval after a successful or failed external write attempt.
    pub cooldown_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualOverride {
    UntilCleared,
    /// Exclusive deadline in the current epoch. Expiry requires a fresh observation.
    UntilMs(u64),
}

/// A proposal, never permission to write. Private identity prevents fabricated acknowledgments.
#[derive(Clone, Debug)]
pub struct TransitionCandidate {
    owner: Arc<()>,
    serial: u64,
    token: StateToken,
    action: Action,
}

impl TransitionCandidate {
    pub fn token(&self) -> &StateToken {
        &self.token
    }

    pub fn action(&self) -> &Action {
        &self.action
    }
}

#[derive(Clone, Debug)]
pub enum ControllerDecision {
    Disarmed,
    ManualOverride,
    NoChange,
    AlreadyApplied,
    Waiting {
        settle_remaining_ms: u64,
        hold_remaining_ms: u64,
        cooldown_remaining_ms: u64,
    },
    Candidate(TransitionCandidate),
}

struct StableAction {
    action: Action,
    since_ms: u64,
}

/// One serialized automatic owner in one clock epoch. Starts disarmed.
///
/// Inputs must come from the trusted evaluator and current server-owned state.
/// Tokens are concurrency checks, not authenticated evidence. No state is persisted.
pub struct Controller {
    policy: TransitionPolicy,
    owner: Arc<()>,
    serial: u64,
    current: StateToken,
    armed: bool,
    manual: Option<ManualOverride>,
    stable: Option<StableAction>,
    candidate: Option<TransitionCandidate>,
    guarded_serial: Option<u64>,
    applied: Option<Action>,
    last_applied_at_ms: Option<u64>,
    last_attempt_at_ms: Option<u64>,
}

impl Controller {
    pub fn new(policy: TransitionPolicy, current: &StateToken) -> Result<Self, Error> {
        validate_token(current)?;
        Ok(Self {
            policy,
            owner: Arc::new(()),
            serial: 0,
            current: current.clone(),
            armed: false,
            manual: None,
            stable: None,
            candidate: None,
            guarded_serial: None,
            applied: None,
            last_applied_at_ms: None,
            last_attempt_at_ms: None,
        })
    }

    pub fn is_armed(&self) -> bool {
        self.armed
    }

    pub fn current(&self) -> &StateToken {
        &self.current
    }

    pub fn last_applied_at_ms(&self) -> Option<u64> {
        self.last_applied_at_ms
    }

    pub fn arm(&mut self, current: &StateToken) -> Result<(), Error> {
        self.advance(current)?;
        self.armed = true;
        Ok(())
    }

    pub fn disarm(&mut self, current: &StateToken) -> Result<(), Error> {
        // Even a bad caller timestamp must not leave automatic mode armed.
        self.armed = false;
        self.cancel();
        self.advance(current)
    }

    /// Manual edits must advance manual_revision, even for identical restored content.
    pub fn set_manual_override(
        &mut self,
        current: &StateToken,
        manual: ManualOverride,
    ) -> Result<(), Error> {
        if matches!(manual, ManualOverride::UntilMs(end) if end <= current.evaluated_at_ms) {
            self.cancel();
            return Err(error("manual override deadline must be in the future"));
        }
        if current.context.manual_revision <= self.current.context.manual_revision {
            // Retrying the same command is harmless; changing lifetime needs a new revision.
            if current == &self.current && self.manual == Some(manual) {
                return Ok(());
            }
            self.cancel();
            return Err(error("manual edits must advance manual revision"));
        }
        self.advance(current)?;
        self.manual = Some(manual);
        self.cancel();
        Ok(())
    }

    pub fn clear_manual_override(&mut self, current: &StateToken) -> Result<(), Error> {
        if current.context.manual_revision <= self.current.context.manual_revision {
            if current == &self.current && self.manual.is_none() {
                return Ok(());
            }
            self.cancel();
            return Err(error(
                "clearing manual override must advance manual revision",
            ));
        }
        self.advance(current)?;
        self.manual = None;
        self.cancel();
        Ok(())
    }

    /// A reset requires a new, never reused owner epoch; it always disarms.
    /// Candidate serials survive reset, so prior acknowledgments cannot revive.
    pub fn reset(&mut self, current: &StateToken) -> Result<(), Error> {
        self.armed = false;
        self.cancel();
        validate_token(current)?;
        if current.epoch == self.current.epoch {
            return Err(error("reset requires a new owner epoch"));
        }
        self.current = current.clone();
        self.manual = None;
        self.applied = None;
        self.last_applied_at_ms = None;
        self.last_attempt_at_ms = None;
        Ok(())
    }

    /// Evaluate timing for exactly the owner's current evaluation token.
    /// Unknown stays whatever explicit action the evaluator's unknown branch selected.
    pub fn observe(
        &mut self,
        evaluation: &Evaluation,
        current: &StateToken,
    ) -> Result<ControllerDecision, Error> {
        self.advance(current)?;
        if evaluation.token != *current
            || evaluation.schema_version != 1
            || evaluation.audio_applied
            || (evaluation.terminal.is_none() && !matches!(evaluation.action, Action::NoChange {}))
        {
            self.cancel();
            return Err(error("evaluation does not match current owned state"));
        }
        // Reuse bounded action admission, including for direct Rust callers.
        if let Err(err) = validate::action(&evaluation.action) {
            self.cancel();
            return Err(err);
        }
        if !self.armed {
            self.cancel();
            return Ok(ControllerDecision::Disarmed);
        }
        if let Some(manual) = self.manual {
            if matches!(manual, ManualOverride::UntilMs(end) if current.evaluated_at_ms >= end) {
                self.manual = None;
                self.cancel();
            } else {
                self.cancel();
                return Ok(ControllerDecision::ManualOverride);
            }
        }
        if matches!(evaluation.action, Action::NoChange {}) {
            self.cancel();
            return Ok(ControllerDecision::NoChange);
        }
        if self.applied.as_ref() == Some(&evaluation.action) {
            self.cancel();
            return Ok(ControllerDecision::AlreadyApplied);
        }
        let now = current.evaluated_at_ms;
        if self.stable.as_ref().map(|s| &s.action) != Some(&evaluation.action) {
            self.cancel();
            self.stable = Some(StableAction {
                action: evaluation.action.clone(),
                since_ms: now,
            });
        }
        let settle_remaining_ms = remaining(
            self.policy.settle_ms,
            self.stable.as_ref().map(|s| s.since_ms),
            now,
        );
        let hold_remaining_ms =
            remaining(self.policy.minimum_hold_ms, self.last_applied_at_ms, now);
        let cooldown_remaining_ms =
            remaining(self.policy.cooldown_ms, self.last_attempt_at_ms, now);
        if settle_remaining_ms > 0 || hold_remaining_ms > 0 || cooldown_remaining_ms > 0 {
            return Ok(ControllerDecision::Waiting {
                settle_remaining_ms,
                hold_remaining_ms,
                cooldown_remaining_ms,
            });
        }
        if self.candidate.is_none() {
            self.serial = self
                .serial
                .checked_add(1)
                .ok_or_else(|| error("candidate identity exhausted; replace owner"))?;
            self.candidate = Some(TransitionCandidate {
                owner: Arc::clone(&self.owner),
                serial: self.serial,
                token: current.clone(),
                action: evaluation.action.clone(),
            });
        }
        Ok(ControllerDecision::Candidate(
            self.candidate.as_ref().expect("candidate created").clone(),
        ))
    }

    /// Check immediately before the owner's guarded external write, under its lock.
    /// The owner must still reevaluate fresh facts, preview and check store revisions.
    pub fn guard(
        &mut self,
        candidate: &TransitionCandidate,
        current: &StateToken,
    ) -> Result<(), Error> {
        self.check_candidate(candidate)?;
        self.advance(current)?;
        self.check_candidate(candidate)?;
        self.guarded_serial = Some(candidate.serial);
        Ok(())
    }

    /// Report a failed/conflicted attempt. Only cooldown advances, never hold/applied.
    /// Supply the freshly reread state and completion time, not the failed preview token.
    pub fn reject(
        &mut self,
        candidate: &TransitionCandidate,
        current: &StateToken,
    ) -> Result<(), Error> {
        self.check_candidate(candidate)?;
        self.advance(current)?;
        self.candidate = None;
        self.last_attempt_at_ms = Some(current.evaluated_at_ms);
        Ok(())
    }

    /// Call only after successful expected-store-revision commit. This is a caller
    /// assertion of success, not proof of persistence or native audio delivery.
    /// Config/store must advance; other state must still match the guarded candidate.
    pub fn acknowledge(
        &mut self,
        candidate: &TransitionCandidate,
        committed: &StateToken,
    ) -> Result<(), Error> {
        self.check_candidate(candidate)?;
        let before = &candidate.token;
        if self.guarded_serial != Some(candidate.serial)
            || committed.epoch != before.epoch
            || committed.graph_id != before.graph_id
            || committed.graph_revision != before.graph_revision
            || committed.snapshot_revision != before.snapshot_revision
            || committed.evaluated_at_ms < before.evaluated_at_ms
            || committed.context.inventory_revision != before.context.inventory_revision
            || committed.context.manual_revision != before.context.manual_revision
            || committed.context.config_revision <= before.context.config_revision
            || committed.context.store_revision <= before.context.store_revision
        {
            return Err(error(
                "commit acknowledgment does not match guarded transition",
            ));
        }
        self.advance(committed)?;
        self.applied = Some(candidate.action.clone());
        self.last_applied_at_ms = Some(committed.evaluated_at_ms);
        self.last_attempt_at_ms = Some(committed.evaluated_at_ms);
        self.cancel();
        Ok(())
    }

    fn check_candidate(&self, candidate: &TransitionCandidate) -> Result<(), Error> {
        if !self.armed
            || self.manual.is_some()
            || !Arc::ptr_eq(&self.owner, &candidate.owner)
            || self.candidate.as_ref().map(|c| c.serial) != Some(candidate.serial)
            || candidate.token != self.current
        {
            return Err(error("candidate canceled, stale or from another owner"));
        }
        Ok(())
    }

    fn cancel(&mut self) {
        self.stable = None;
        self.candidate = None;
        self.guarded_serial = None;
    }

    fn advance(&mut self, current: &StateToken) -> Result<(), Error> {
        if let Err(err) = validate_token(current) {
            self.cancel();
            return Err(err);
        }
        let before = &self.current;
        if current.epoch != before.epoch
            || current.evaluated_at_ms < before.evaluated_at_ms
            || current.snapshot_revision < before.snapshot_revision
            || (current.graph_id == before.graph_id
                && current.graph_revision < before.graph_revision)
            || decreases(&current.context, &before.context)
        {
            self.cancel();
            return Err(error(
                "epoch, monotonic time or revisions regressed; reset requires a new epoch",
            ));
        }
        if current.graph_id != before.graph_id
            || current.graph_revision != before.graph_revision
            || current.context != before.context
        {
            self.cancel();
            // Another state owner can have replaced the actual configuration.
            self.applied = None;
        } else if current != before {
            // Captures change revision even with the same action. Keep its settle interval.
            self.candidate = None;
        }
        if current.context.manual_revision > self.current.context.manual_revision {
            // An unreported manual edit is fail-closed until explicitly cleared.
            self.manual = Some(ManualOverride::UntilCleared);
        }
        self.current = current.clone();
        Ok(())
    }
}

fn validate_token(token: &StateToken) -> Result<(), Error> {
    validate::text(&token.epoch)?;
    validate::text(&token.graph_id)
}

fn decreases(next: &Context, previous: &Context) -> bool {
    next.config_revision < previous.config_revision
        || next.store_revision < previous.store_revision
        || next.inventory_revision < previous.inventory_revision
        || next.manual_revision < previous.manual_revision
}

fn remaining(duration: u64, start: Option<u64>, now: u64) -> u64 {
    start.map_or(0, |start| duration.saturating_sub(now - start))
}

fn error(message: &'static str) -> Error {
    Error::new("controller-conflict", message)
}
