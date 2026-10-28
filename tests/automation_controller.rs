use mixyard::automation::*;

// Use the real evaluator and literal headphone/application-priority graph throughout.
struct Situation(Request);

impl Situation {
    fn new() -> Self {
        let mut request = parse_request(include_bytes!(
            "../examples/automation/headphone-priority.json"
        ))
        .unwrap();
        request.now_ms = 0;
        for fact in &mut request.snapshot.facts {
            fact.observed_at_ms = Some(0);
        }
        Self(request)
    }

    fn capture(&mut self, now: u64) -> Evaluation {
        self.0.now_ms = now;
        self.0.snapshot.revision += 1;
        for fact in &mut self.0.snapshot.facts {
            fact.observed_at_ms = Some(now);
        }
        evaluate(&self.0).unwrap()
    }

    fn call(&mut self) {
        self.0.snapshot.facts[0].value = Some(Value::TextSet(vec![]));
    }

    fn headphones(&mut self) {
        self.0.snapshot.facts[0].value =
            Some(Value::TextSet(vec!["example:wired-headphones".into()]));
    }

    fn unknown(&mut self) {
        self.0.snapshot.facts[0].status = FactStatus::Unsupported;
        self.0.snapshot.facts[0].value = None;
    }

    fn restored(&mut self) {
        self.0.snapshot.facts[0].status = FactStatus::Known;
        self.headphones();
    }

    fn controller(&self, settle: u64, hold: u64, cooldown: u64) -> Controller {
        Controller::new(
            TransitionPolicy {
                settle_ms: settle,
                minimum_hold_ms: hold,
                cooldown_ms: cooldown,
            },
            &evaluate(&self.0).unwrap().token,
        )
        .unwrap()
    }

    fn commit_token(&mut self, candidate: &TransitionCandidate, at: u64) -> StateToken {
        // Models only the successful guarded store boundary, never native audio.
        let mut committed = candidate.token().clone();
        committed.evaluated_at_ms = at;
        committed.context.config_revision += 1;
        committed.context.store_revision += 1;
        self.0.context = committed.context.clone();
        self.0.now_ms = at;
        committed
    }
}

fn observe(controller: &mut Controller, evaluation: &Evaluation) -> ControllerDecision {
    controller.observe(evaluation, &evaluation.token).unwrap()
}

fn candidate(controller: &mut Controller, evaluation: &Evaluation) -> TransitionCandidate {
    match observe(controller, evaluation) {
        ControllerDecision::Candidate(candidate) => candidate,
        other => panic!("expected candidate, got {other:?}"),
    }
}

fn waiting(decision: ControllerDecision, settle: u64, hold: u64, cooldown: u64) {
    match decision {
        ControllerDecision::Waiting {
            settle_remaining_ms,
            hold_remaining_ms,
            cooldown_remaining_ms,
        } => assert_eq!(
            (
                settle_remaining_ms,
                hold_remaining_ms,
                cooldown_remaining_ms
            ),
            (settle, hold, cooldown)
        ),
        other => panic!("expected waiting, got {other:?}"),
    }
}

#[test]
fn explicit_arm_and_stable_action_settle_survive_every_live_capture() {
    let mut situation = Situation::new();
    let mut controller = situation.controller(1_000, 0, 0);
    let first = situation.capture(0);
    assert!(matches!(
        observe(&mut controller, &first),
        ControllerDecision::Disarmed
    ));
    controller.arm(&first.token).unwrap();
    waiting(observe(&mut controller, &first), 1_000, 0, 0);
    for now in [100, 250, 500, 999] {
        waiting(
            observe(&mut controller, &situation.capture(now)),
            1_000 - now,
            0,
            0,
        );
    }
    let ready = situation.capture(1_000);
    let selected = candidate(&mut controller, &ready);
    assert_eq!(selected.action(), &ready.action);
    assert_eq!(selected.token(), &ready.token);
    controller.guard(&selected, &ready.token).unwrap();
    assert_eq!(controller.last_applied_at_ms(), None);
}

#[test]
fn action_changes_and_unknown_no_change_restart_settle_without_selecting_a_fallback() {
    let mut situation = Situation::new();
    let mut controller = situation.controller(100, 0, 0);
    let first = situation.capture(0);
    controller.arm(&first.token).unwrap();
    waiting(observe(&mut controller, &first), 100, 0, 0);
    situation.call();
    waiting(observe(&mut controller, &situation.capture(90)), 100, 0, 0);
    situation.unknown();
    let unknown = situation.capture(180);
    assert_eq!(unknown.action, Action::NoChange {});
    assert!(matches!(
        observe(&mut controller, &unknown),
        ControllerDecision::NoChange
    ));
    situation.restored();
    waiting(observe(&mut controller, &situation.capture(190)), 100, 0, 0);
    waiting(observe(&mut controller, &situation.capture(289)), 1, 0, 0);
    candidate(&mut controller, &situation.capture(290));
}

#[test]
fn an_explicit_unknown_branch_action_is_preserved_without_controller_inference() {
    let mut situation = Situation::new();
    situation.unknown();
    let Node::Decision { on_unknown, .. } = &mut situation.0.graph.nodes[0] else {
        panic!()
    };
    *on_unknown = "call".into();
    let evaluation = situation.capture(0);
    let mut controller = situation.controller(0, 0, 0);
    controller.arm(&evaluation.token).unwrap();
    assert_eq!(
        candidate(&mut controller, &evaluation).action(),
        &evaluation.action
    );
}

#[test]
fn successful_guarded_commit_is_required_and_duplicate_observations_never_rewrite() {
    let mut situation = Situation::new();
    let mut controller = situation.controller(0, 0, 0);
    let first = situation.capture(0);
    controller.arm(&first.token).unwrap();
    let pending = candidate(&mut controller, &first);
    let repeated = candidate(&mut controller, &first);
    let committed = situation.commit_token(&pending, 10);
    assert!(controller.acknowledge(&pending, &committed).is_err());
    controller.guard(&repeated, &first.token).unwrap();
    controller.acknowledge(&pending, &committed).unwrap();
    assert_eq!(controller.last_applied_at_ms(), Some(10));
    assert!(controller.acknowledge(&pending, &committed).is_err());
    for now in [10, 100, 10_000] {
        assert!(matches!(
            observe(&mut controller, &situation.capture(now)),
            ControllerDecision::AlreadyApplied
        ));
    }
    // Unknown/no-change doesn't forget the successfully applied action.
    situation.unknown();
    assert!(matches!(
        observe(&mut controller, &situation.capture(10_001)),
        ControllerDecision::NoChange
    ));
    situation.restored();
    assert!(matches!(
        observe(&mut controller, &situation.capture(10_002)),
        ControllerDecision::AlreadyApplied
    ));
}

#[test]
fn own_commit_retains_applied_ownership_but_external_identical_intent_needs_a_new_decision() {
    let mut situation = Situation::new();
    let mut controller = situation.controller(100, 1_000, 300);
    let first = situation.capture(0);
    controller.arm(&first.token).unwrap();
    observe(&mut controller, &first);
    let ready = situation.capture(100);
    let pending = candidate(&mut controller, &ready);
    controller.guard(&pending, &ready.token).unwrap();
    let committed = situation.commit_token(&pending, 110);
    controller.acknowledge(&pending, &committed).unwrap();
    controller.disarm(&committed).unwrap();
    controller.arm(&committed).unwrap();
    assert!(matches!(
        observe(&mut controller, &situation.capture(120)),
        ControllerDecision::AlreadyApplied
    ));
    // Another writer may have replaced content, even with the same proposed profile ID.
    situation.0.context.config_revision += 1;
    situation.0.context.store_revision += 1;
    waiting(
        observe(&mut controller, &situation.capture(130)),
        100,
        980,
        280,
    );
    waiting(
        observe(&mut controller, &situation.capture(230)),
        0,
        880,
        180,
    );
    candidate(&mut controller, &situation.capture(1_110));
    situation.0.context.manual_revision += 1;
    assert!(matches!(
        observe(&mut controller, &situation.capture(1_111)),
        ControllerDecision::ManualOverride
    ));
}

#[test]
fn no_change_cancels_an_already_guarded_candidate() {
    let mut situation = Situation::new();
    let mut controller = situation.controller(0, 0, 0);
    let first = situation.capture(0);
    controller.arm(&first.token).unwrap();
    let pending = candidate(&mut controller, &first);
    controller.guard(&pending, &first.token).unwrap();
    situation.unknown();
    assert!(matches!(
        observe(&mut controller, &situation.capture(1)),
        ControllerDecision::NoChange
    ));
    let committed = situation.commit_token(&pending, 2);
    assert!(controller.acknowledge(&pending, &committed).is_err());
    assert_eq!(controller.last_applied_at_ms(), None);
}

#[test]
fn hold_and_cooldown_use_actual_commit_time_and_settle_can_run_during_both() {
    let mut situation = Situation::new();
    let mut controller = situation.controller(100, 1_000, 300);
    let first = situation.capture(0);
    controller.arm(&first.token).unwrap();
    observe(&mut controller, &first);
    let ready = situation.capture(100);
    let pending = candidate(&mut controller, &ready);
    controller.guard(&pending, &ready.token).unwrap();
    let committed = situation.commit_token(&pending, 125);
    controller.acknowledge(&pending, &committed).unwrap();
    situation.call();
    waiting(
        observe(&mut controller, &situation.capture(200)),
        100,
        925,
        225,
    );
    waiting(
        observe(&mut controller, &situation.capture(300)),
        0,
        825,
        125,
    );
    waiting(observe(&mut controller, &situation.capture(1_124)), 0, 1, 0);
    candidate(&mut controller, &situation.capture(1_125));
}

#[test]
fn conflicts_and_failed_writes_retry_after_cooldown_without_starting_hold() {
    let mut situation = Situation::new();
    let mut controller = situation.controller(0, 5_000, 100);
    let first = situation.capture(0);
    controller.arm(&first.token).unwrap();
    let pending = candidate(&mut controller, &first);
    controller.guard(&pending, &first.token).unwrap();
    // A competing writer won the store revision guard. Re-read it before retry.
    situation.0.context.store_revision += 1;
    situation.0.context.config_revision += 1;
    let conflict = situation.capture(20);
    controller.reject(&pending, &conflict.token).unwrap();
    assert_eq!(controller.last_applied_at_ms(), None);
    assert!(controller.acknowledge(&pending, &conflict.token).is_err());
    waiting(observe(&mut controller, &situation.capture(119)), 0, 0, 1);
    let retry_evaluation = situation.capture(120);
    let retry = candidate(&mut controller, &retry_evaluation);
    controller.guard(&retry, &retry_evaluation.token).unwrap();
    controller
        .reject(&retry, &situation.capture(130).token)
        .unwrap();
    waiting(observe(&mut controller, &situation.capture(229)), 0, 0, 1);
    let ready = situation.capture(230);
    let successful = candidate(&mut controller, &ready);
    controller.guard(&successful, &ready.token).unwrap();
    let committed = situation.commit_token(&successful, 240);
    controller.acknowledge(&successful, &committed).unwrap();
    assert_eq!(controller.last_applied_at_ms(), Some(240));
    situation.call();
    waiting(
        observe(&mut controller, &situation.capture(250)),
        0,
        4_990,
        90,
    );
}

#[test]
fn failed_write_does_not_extend_an_existing_successful_hold() {
    let mut situation = Situation::new();
    let mut controller = situation.controller(0, 100, 0);
    let first = situation.capture(0);
    controller.arm(&first.token).unwrap();
    let pending = candidate(&mut controller, &first);
    controller.guard(&pending, &first.token).unwrap();
    let committed = situation.commit_token(&pending, 10);
    controller.acknowledge(&pending, &committed).unwrap();
    situation.call();
    let next = situation.capture(110);
    let failed = candidate(&mut controller, &next);
    controller
        .reject(&failed, &situation.capture(120).token)
        .unwrap();
    assert_eq!(controller.last_applied_at_ms(), Some(10));
    candidate(&mut controller, &situation.capture(121));
}

#[test]
fn disarming_cancels_a_candidate_and_rearming_restarts_settle() {
    let mut situation = Situation::new();
    let mut controller = situation.controller(100, 0, 0);
    let first = situation.capture(0);
    controller.arm(&first.token).unwrap();
    observe(&mut controller, &first);
    let ready = situation.capture(100);
    let pending = candidate(&mut controller, &ready);
    controller.disarm(&ready.token).unwrap();
    assert!(controller.guard(&pending, &ready.token).is_err());
    assert!(matches!(
        observe(&mut controller, &ready),
        ControllerDecision::Disarmed
    ));
    controller.arm(&ready.token).unwrap();
    waiting(observe(&mut controller, &ready), 100, 0, 0);
    let renewed = situation.capture(200);
    candidate(&mut controller, &renewed);
    let committed = situation.commit_token(&pending, 200);
    assert!(controller.acknowledge(&pending, &committed).is_err());
}

#[test]
fn timed_manual_override_blocks_even_explicit_arm_and_expiry_needs_new_settle() {
    let mut situation = Situation::new();
    let mut controller = situation.controller(100, 0, 0);
    let first = situation.capture(0);
    controller.arm(&first.token).unwrap();
    observe(&mut controller, &first);
    let pending = candidate(&mut controller, &situation.capture(100));
    situation.0.context.manual_revision += 1;
    let manual = situation.capture(110);
    controller
        .set_manual_override(&manual.token, ManualOverride::UntilMs(1_000))
        .unwrap();
    controller.arm(&manual.token).unwrap();
    assert!(controller.guard(&pending, &manual.token).is_err());
    assert!(matches!(
        observe(&mut controller, &situation.capture(999)),
        ControllerDecision::ManualOverride
    ));
    waiting(
        observe(&mut controller, &situation.capture(1_000)),
        100,
        0,
        0,
    );
    candidate(&mut controller, &situation.capture(1_100));
}

#[test]
fn manual_edits_fail_closed_until_explicit_revisioned_clear() {
    let mut situation = Situation::new();
    let mut controller = situation.controller(100, 0, 0);
    let first = situation.capture(0);
    controller.arm(&first.token).unwrap();
    observe(&mut controller, &first);
    situation.0.context.manual_revision += 1;
    let edit = situation.capture(50);
    // A manual revision without a lifetime event still pauses automation.
    assert!(matches!(
        observe(&mut controller, &edit),
        ControllerDecision::ManualOverride
    ));
    assert!(controller.clear_manual_override(&edit.token).is_err());
    assert!(matches!(
        observe(&mut controller, &situation.capture(50_000)),
        ControllerDecision::ManualOverride
    ));
    situation.0.context.manual_revision += 1;
    let clear = situation.capture(50_001);
    controller.clear_manual_override(&clear.token).unwrap();
    waiting(observe(&mut controller, &clear), 100, 0, 0);
    candidate(&mut controller, &situation.capture(50_101));
}

#[test]
fn explicit_indefinite_manual_override_and_evaluator_manual_no_change_are_respected() {
    let mut situation = Situation::new();
    let mut controller = situation.controller(0, 0, 0);
    let first = situation.capture(0);
    controller.arm(&first.token).unwrap();
    situation.0.context.manual_revision += 1;
    let manual = situation.capture(1);
    controller
        .set_manual_override(&manual.token, ManualOverride::UntilCleared)
        .unwrap();
    controller
        .set_manual_override(&manual.token, ManualOverride::UntilCleared)
        .unwrap();
    assert!(matches!(
        observe(&mut controller, &situation.capture(100_000)),
        ControllerDecision::ManualOverride
    ));
    situation.0.context.manual_revision += 1;
    let clear = situation.capture(100_001);
    controller.clear_manual_override(&clear.token).unwrap();
    situation.0.manual_override = true;
    let evaluator_manual = situation.capture(100_002);
    assert_eq!(evaluator_manual.terminal, None);
    assert!(matches!(
        observe(&mut controller, &evaluator_manual),
        ControllerDecision::NoChange
    ));
}

#[test]
fn policy_and_owned_state_changes_cancel_settle_even_for_identical_actions() {
    // Changes to each independently owned context must force a fresh stable interval.
    for change in 0..5 {
        let mut situation = Situation::new();
        let mut controller = situation.controller(100, 0, 0);
        let first = situation.capture(0);
        controller.arm(&first.token).unwrap();
        observe(&mut controller, &first);
        match change {
            0 => situation.0.graph.revision += 1,
            1 => situation.0.graph.id = "replacement-policy".into(),
            2 => situation.0.context.config_revision += 1,
            3 => situation.0.context.store_revision += 1,
            4 => situation.0.context.inventory_revision += 1,
            _ => unreachable!(),
        }
        waiting(observe(&mut controller, &situation.capture(99)), 100, 0, 0);
        waiting(observe(&mut controller, &situation.capture(198)), 1, 0, 0);
        candidate(&mut controller, &situation.capture(199));
    }
}

#[test]
fn current_tokens_are_required_for_guard_and_old_ack_cannot_cancel_the_new_candidate() {
    let mut situation = Situation::new();
    let mut controller = situation.controller(0, 0, 0);
    let first = situation.capture(0);
    controller.arm(&first.token).unwrap();
    let pending = candidate(&mut controller, &first);
    let fresh = situation.capture(1);
    // A new snapshot with identical observations revokes the old candidate.
    assert!(controller.guard(&pending, &fresh.token).is_err());
    let current = candidate(&mut controller, &fresh);
    let stale_commit = situation.commit_token(&pending, 2);
    assert!(controller.acknowledge(&pending, &stale_commit).is_err());
    controller.guard(&current, &fresh.token).unwrap();
    let committed = situation.commit_token(&current, 2);
    controller.acknowledge(&current, &committed).unwrap();
}

#[test]
fn evaluation_mismatch_and_each_regressing_owner_counter_block_and_cancel_pending_work() {
    for field in 0..8 {
        let mut situation = Situation::new();
        situation.0.context.store_revision = 1;
        let first = situation.capture(10);
        let mut controller = situation.controller(0, 0, 0);
        controller.arm(&first.token).unwrap();
        let pending = candidate(&mut controller, &first);
        let mut bad = first.token.clone();
        match field {
            0 => bad.evaluated_at_ms -= 1,
            1 => bad.snapshot_revision -= 1,
            2 => bad.graph_revision -= 1,
            3 => bad.context.config_revision -= 1,
            4 => bad.context.store_revision -= 1,
            5 => bad.context.inventory_revision -= 1,
            6 => bad.context.manual_revision -= 1,
            7 => bad.epoch = "another-owner".into(),
            _ => unreachable!(),
        }
        assert!(controller.observe(&first, &bad).is_err());
        assert!(controller.guard(&pending, &first.token).is_err());
        assert_eq!(controller.last_applied_at_ms(), None);
    }
    let mut situation = Situation::new();
    let mut controller = situation.controller(0, 0, 0);
    let first = situation.capture(0);
    controller.arm(&first.token).unwrap();
    let pending = candidate(&mut controller, &first);
    let fresh = situation.capture(1);
    assert!(controller.observe(&first, &fresh.token).is_err());
    assert!(controller.guard(&pending, &first.token).is_err());
}

#[test]
fn commit_ack_requires_exact_guarded_state_and_actual_config_and_store_revision_advances() {
    for field in 0..9 {
        let mut situation = Situation::new();
        let mut controller = situation.controller(0, 0, 0);
        let first = situation.capture(10);
        controller.arm(&first.token).unwrap();
        let pending = candidate(&mut controller, &first);
        controller.guard(&pending, &first.token).unwrap();
        let mut committed = situation.commit_token(&pending, 11);
        match field {
            0 => committed.context.config_revision = first.token.context.config_revision,
            1 => committed.context.store_revision = first.token.context.store_revision,
            2 => committed.context.inventory_revision += 1,
            3 => committed.context.manual_revision += 1,
            4 => committed.snapshot_revision += 1,
            5 => committed.graph_revision += 1,
            6 => committed.graph_id = "replacement".into(),
            7 => committed.epoch = "replacement".into(),
            8 => committed.evaluated_at_ms = 9,
            _ => unreachable!(),
        }
        assert!(controller.acknowledge(&pending, &committed).is_err());
        assert_eq!(controller.last_applied_at_ms(), None);
    }
}

#[test]
fn new_epoch_resets_timing_disarms_and_cannot_revive_an_old_acknowledgment() {
    let mut situation = Situation::new();
    let mut controller = situation.controller(0, 5_000, 1_000);
    let first = situation.capture(100_000);
    controller.arm(&first.token).unwrap();
    let pending = candidate(&mut controller, &first);
    controller.guard(&pending, &first.token).unwrap();
    let committed = situation.commit_token(&pending, 100_100);
    controller.acknowledge(&pending, &committed).unwrap();
    assert!(controller.reset(&committed).is_err());
    assert!(!controller.is_armed());
    situation.0.snapshot.epoch = "new-boot-owner".into();
    situation.0.snapshot.revision = 0;
    situation.0.context = Context {
        config_revision: 0,
        store_revision: 0,
        inventory_revision: 0,
        manual_revision: 0,
    };
    let reboot = situation.capture(0);
    controller.reset(&reboot.token).unwrap();
    assert_eq!(controller.last_applied_at_ms(), None);
    assert!(matches!(
        observe(&mut controller, &reboot),
        ControllerDecision::Disarmed
    ));
    controller.arm(&reboot.token).unwrap();
    let new = candidate(&mut controller, &reboot);
    assert!(controller.acknowledge(&pending, &committed).is_err());
    controller.guard(&new, &reboot.token).unwrap();
    let new_commit = situation.commit_token(&new, 1);
    controller.acknowledge(&new, &new_commit).unwrap();
}

#[test]
fn candidates_are_bound_to_one_controller_even_with_identical_policy_and_tokens() {
    let mut situation = Situation::new();
    let evaluation = situation.capture(0);
    let mut first = situation.controller(0, 0, 0);
    let mut second = situation.controller(0, 0, 0);
    first.arm(&evaluation.token).unwrap();
    second.arm(&evaluation.token).unwrap();
    let from_first = candidate(&mut first, &evaluation);
    let from_second = candidate(&mut second, &evaluation);
    assert!(second.guard(&from_first, &evaluation.token).is_err());
    second.guard(&from_second, &evaluation.token).unwrap();
    let committed = situation.commit_token(&from_second, 1);
    assert!(second.acknowledge(&from_first, &committed).is_err());
    second.acknowledge(&from_second, &committed).unwrap();
}

#[test]
fn intervals_near_u64_limit_do_not_overflow_or_unlock_early() {
    let mut situation = Situation::new();
    let mut controller = situation.controller(10, 0, 0);
    let first = situation.capture(u64::MAX - 10);
    controller.arm(&first.token).unwrap();
    waiting(observe(&mut controller, &first), 10, 0, 0);
    waiting(
        observe(&mut controller, &situation.capture(u64::MAX - 1)),
        1,
        0,
        0,
    );
    candidate(&mut controller, &situation.capture(u64::MAX));
}
