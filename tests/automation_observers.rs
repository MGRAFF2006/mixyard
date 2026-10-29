use mixyard::automation::{observers::*, settings::Declaration, *};
use mixyard::context::{Capture, ObservationError, Options};
use std::{cell::Cell, rc::Rc, sync::atomic::AtomicBool};

// Synthetic trusted provider boundary; these tests qualify policy/lifecycle,
// never native metadata or physical audio. No process/audio helper is executed.
struct Synthetic {
    now: Rc<Cell<u64>>,
    epoch: String,
    revision: u64,
    facts: Vec<Fact>,
    error: bool,
}

impl Synthetic {
    fn new() -> Self {
        Self {
            now: Rc::new(Cell::new(100)),
            epoch: "synthetic-session-a".into(),
            revision: 0,
            facts: vec![],
            error: false,
        }
    }
}

impl Provider for Synthetic {
    fn now_ms(&self) -> Result<u64, ObservationError> {
        Ok(self.now.get())
    }
    fn capture(&mut self, _: &AtomicBool) -> Result<Capture, ObservationError> {
        if self.error {
            return Err(ObservationError("synthetic-failure"));
        }
        self.revision += 1;
        Ok(Capture {
            snapshot: Snapshot {
                schema_version: 1,
                epoch: self.epoch.clone(),
                revision: self.revision,
                facts: self.facts.clone(),
            },
            now_ms: self.now.get(),
            providers: vec![],
            application_scope: vec!["mapped-player".into()],
        })
    }
}

fn context() -> Context {
    Context {
        config_revision: 4,
        store_revision: 5,
        inventory_revision: 6,
        manual_revision: 7,
    }
}

fn declaration(value: bool) -> Declaration {
    Declaration {
        key: FactKey {
            name: "manual.flag".into(),
            subject: Some("meeting".into()),
        },
        value,
    }
}

fn evaluation(capture: &Capture, context: &Context) -> Evaluation {
    let graph = Graph {
        schema_version: 1,
        id: "synthetic-graph".into(),
        name: "Synthetic meeting flag".into(),
        revision: 1,
        root: "flag".into(),
        nodes: vec![
            Node::Decision {
                id: "flag".into(),
                condition: Condition::Fact {
                    key: declaration(true).key,
                    predicate: Predicate::Eq {
                        value: Value::Bool(true),
                    },
                },
                on_true: "select".into(),
                on_false: "leave".into(),
                on_unknown: "leave".into(),
            },
            Node::Terminal {
                id: "select".into(),
                action: Action::Select {
                    profile_id: Some("synthetic-meeting".into()),
                    overrides: vec![],
                },
            },
            Node::Terminal {
                id: "leave".into(),
                action: Action::NoChange {},
            },
        ],
    };
    evaluate_scoped(
        &Request {
            graph,
            catalog: standard_catalog(),
            snapshot: capture.snapshot.clone(),
            now_ms: capture.now_ms,
            context: context.clone(),
            manual_override: false,
        },
        &capture.application_scope,
    )
    .unwrap()
}

fn controller(token: &StateToken) -> Controller {
    Controller::new(
        TransitionPolicy {
            settle_ms: 0,
            minimum_hold_ms: 0,
            cooldown_ms: 0,
        },
        token,
    )
    .unwrap()
}

#[test]
fn fresh_capture_arms_and_guards_the_existing_controller_without_applying_audio() {
    let mut attachment = Attachment::new(Synthetic::new());
    let capture = attachment
        .refresh(&AtomicBool::new(false), &context(), &[declaration(true)])
        .unwrap();
    let evaluation = evaluation(&capture.capture, &context());
    assert!(!evaluation.audio_applied);
    let mut controller = controller(&evaluation.token);
    attachment
        .arm(&mut controller, &capture.lease, &evaluation.token)
        .unwrap();
    let candidate = match controller.observe(&evaluation, &evaluation.token).unwrap() {
        ControllerDecision::Candidate(candidate) => candidate,
        other => panic!("expected real controller candidate, got {other:?}"),
    };
    attachment
        .guard(
            &mut controller,
            &capture.lease,
            &candidate,
            &evaluation.token,
        )
        .unwrap();
    attachment.revoke();
    assert!(attachment
        .guard(
            &mut controller,
            &capture.lease,
            &candidate,
            &evaluation.token
        )
        .is_err());
    assert!(!controller.is_armed());
    let mut committed = evaluation.token.clone();
    committed.context.config_revision += 1;
    committed.context.store_revision += 1;
    assert!(controller.acknowledge(&candidate, &committed).is_err());
}

#[test]
fn expiry_checks_actual_provider_clock_and_disarms_even_with_old_evaluation_time() {
    let provider = Synthetic::new();
    let now = provider.now.clone();
    let mut attachment = Attachment::new(provider);
    let capture = attachment
        .refresh(&AtomicBool::new(false), &context(), &[declaration(true)])
        .unwrap();
    let token = evaluation(&capture.capture, &context()).token;
    let mut controller = controller(&token);
    now.set(5100); // inclusive freshness deadline
    attachment
        .arm(&mut controller, &capture.lease, &token)
        .unwrap();
    now.set(5101);
    assert!(attachment
        .arm(&mut controller, &capture.lease, &token)
        .is_err());
    assert!(!controller.is_armed());
}

#[test]
fn refreshed_reconnected_and_restarted_owners_reject_old_leases() {
    let mut attachment = Attachment::new(Synthetic::new());
    let first = attachment
        .refresh(&AtomicBool::new(false), &context(), &[])
        .unwrap();
    let first_token = evaluation(&first.capture, &context()).token;
    let second = attachment
        .refresh(&AtomicBool::new(false), &context(), &[])
        .unwrap();
    assert!(attachment.check(&first.lease, &first_token).is_err());
    let second_token = evaluation(&second.capture, &context()).token;
    attachment.check(&second.lease, &second_token).unwrap();
    let mut replacement = Synthetic::new();
    replacement.epoch = "synthetic-session-b".into();
    attachment.reconnect(replacement);
    assert!(attachment.check(&second.lease, &second_token).is_err());
    let third = attachment
        .refresh(&AtomicBool::new(false), &context(), &[])
        .unwrap();
    attachment
        .check(&third.lease, &evaluation(&third.capture, &context()).token)
        .unwrap();
    let mut restarted_owner = Attachment::new(Synthetic::new());
    let fresh = restarted_owner
        .refresh(&AtomicBool::new(false), &context(), &[])
        .unwrap();
    assert!(restarted_owner
        .check(&first.lease, &evaluation(&fresh.capture, &context()).token)
        .is_err());
}

#[test]
fn cancelled_and_failed_refresh_revoke_previous_arming_even_when_values_unchanged() {
    let mut attachment = Attachment::new(Synthetic::new());
    let first = attachment
        .refresh(&AtomicBool::new(false), &context(), &[])
        .unwrap();
    let token = evaluation(&first.capture, &context()).token;
    assert!(attachment
        .refresh(&AtomicBool::new(true), &context(), &[])
        .is_err());
    assert!(attachment.check(&first.lease, &token).is_err());
    let second = attachment
        .refresh(&AtomicBool::new(false), &context(), &[])
        .unwrap();
    let mut failed = Synthetic::new();
    failed.error = true;
    attachment.reconnect(failed);
    assert!(attachment
        .refresh(&AtomicBool::new(false), &context(), &[])
        .is_err());
    assert!(attachment
        .check(
            &second.lease,
            &evaluation(&second.capture, &context()).token
        )
        .is_err());
}

#[test]
fn context_revision_snapshot_and_clock_mismatches_cannot_arm() {
    let provider = Synthetic::new();
    let now = provider.now.clone();
    let mut attachment = Attachment::new(provider);
    let capture = attachment
        .refresh(&AtomicBool::new(false), &context(), &[])
        .unwrap();
    let token = evaluation(&capture.capture, &context()).token;
    for changed in 0..7 {
        let mut bad = token.clone();
        match changed {
            0 => bad.context.config_revision += 1,
            1 => bad.context.store_revision += 1,
            2 => bad.context.inventory_revision += 1,
            3 => bad.context.manual_revision += 1,
            4 => bad.snapshot_revision += 1,
            5 => bad.epoch = "wrong-session".into(),
            _ => bad.evaluated_at_ms += 1,
        }
        assert!(attachment
            .arm(&mut controller(&token), &capture.lease, &bad)
            .is_err());
    }
    now.set(99);
    assert!(attachment.check(&capture.lease, &token).is_err());
}

#[test]
fn reconnect_cannot_replay_same_epoch_revision_or_regress_owner_revisions() {
    let mut attachment = Attachment::new(Synthetic::new());
    attachment
        .refresh(&AtomicBool::new(false), &context(), &[])
        .unwrap();
    attachment.reconnect(Synthetic::new());
    assert!(attachment
        .refresh(&AtomicBool::new(false), &context(), &[])
        .is_err());
    let mut regressed = context();
    regressed.inventory_revision -= 1;
    assert!(attachment
        .refresh(&AtomicBool::new(false), &regressed, &[])
        .is_err());
}

#[test]
fn manual_false_is_known_absent_subject_is_unknown_and_edits_need_new_revision() {
    let mut attachment = Attachment::new(Synthetic::new());
    let absent = attachment
        .refresh(&AtomicBool::new(false), &context(), &[])
        .unwrap();
    assert!(!absent
        .capture
        .snapshot
        .facts
        .iter()
        .any(|f| f.key.name == "manual.flag"));
    assert!(matches!(
        evaluation(&absent.capture, &context()).action,
        Action::NoChange {}
    ));
    let mut updated = context();
    updated.manual_revision += 1;
    let negative = attachment
        .refresh(&AtomicBool::new(false), &updated, &[declaration(false)])
        .unwrap();
    let flag = &negative.capture.snapshot.facts[0];
    assert_eq!(flag.status, FactStatus::Known);
    assert_eq!(flag.value, Some(Value::Bool(false)));
    assert_eq!(flag.evidence, Evidence::UserDeclared);
    assert!(attachment
        .refresh(&AtomicBool::new(false), &updated, &[declaration(true)])
        .is_err());
    updated.manual_revision += 1;
    let positive = attachment
        .refresh(&AtomicBool::new(false), &updated, &[declaration(true)])
        .unwrap();
    assert!(matches!(
        evaluation(&positive.capture, &updated).action,
        Action::Select { .. }
    ));
}

fn fact(name: &str, value: Value) -> Fact {
    Fact {
        key: FactKey {
            name: name.into(),
            subject: None,
        },
        status: FactStatus::Known,
        value: Some(value),
        provider: "synthetic-provider".into(),
        evidence: Evidence::Fixture,
        observed_at_ms: Some(100),
        valid_for_ms: 5000,
        completeness: Completeness::Complete,
        reason: None,
    }
}

#[test]
fn fixture_evidence_never_arms_even_when_declaration_replaces_its_only_fact() {
    let mut provider = Synthetic::new();
    let mut flag = fact("manual.flag", Value::Bool(true));
    flag.key.subject = Some("meeting".into());
    provider.facts.push(flag);
    let mut attachment = Attachment::new(provider);
    let capture = attachment
        .refresh(&AtomicBool::new(false), &context(), &[declaration(true)])
        .unwrap();
    let token = evaluation(&capture.capture, &context()).token;
    assert!(attachment
        .arm(&mut controller(&token), &capture.lease, &token)
        .is_err());
}

#[test]
fn stale_incomplete_future_and_malformed_provider_facts_are_rejected() {
    for case in 0..5 {
        let mut provider = Synthetic::new();
        let mut observed = fact("network.connected", Value::Bool(true));
        match case {
            0 => observed.observed_at_ms = None,
            1 => observed.observed_at_ms = Some(101),
            2 => observed.completeness = Completeness::Partial,
            3 => {
                observed.observed_at_ms = Some(0);
                observed.valid_for_ms = 99;
            }
            _ => observed.value = Some(Value::Text("wrong type".into())),
        }
        provider.facts.push(observed);
        assert!(Attachment::new(provider)
            .refresh(&AtomicBool::new(false), &context(), &[])
            .is_err());
    }
}

#[test]
fn mapped_application_domain_is_preserved_and_unmapped_presence_is_rejected() {
    let mut provider = Synthetic::new();
    provider.facts.push(fact(
        "application.running",
        Value::TextSet(vec!["unmapped-player".into()]),
    ));
    assert!(Attachment::new(provider)
        .refresh(&AtomicBool::new(false), &context(), &[])
        .is_err());
}

#[test]
fn invalid_declarations_do_not_partially_modify_the_capture() {
    let mut provider = Synthetic::new();
    let mut capture = provider.capture(&AtomicBool::new(false)).unwrap();
    let before = capture.snapshot.clone();
    let mut invalid = declaration(true);
    invalid.key.name = "application.capture".into();
    assert!(apply_declarations(&mut capture, &[declaration(true), invalid]).is_err());
    assert_eq!(capture.snapshot, before);
    assert!(apply_declarations(&mut capture, &[declaration(true), declaration(false)]).is_err());
    assert!(apply_declarations(&mut capture, &vec![declaration(true); MAX_FACTS + 1]).is_err());
}

#[test]
fn native_provider_honors_cancellation_without_running_any_metadata_helper() {
    let mut provider = NativeProvider::new(Options::default()).unwrap();
    assert_eq!(
        provider.capture(&AtomicBool::new(true)).unwrap_err().0,
        "cancelled"
    );
}

struct Links(std::path::PathBuf);
impl Links {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "mixyard-observer-links-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn link(&self, name: &str, kind: &str, state: &str) {
        let path = self.0.join(name);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("type"), kind).unwrap();
        std::fs::write(path.join("operstate"), state).unwrap();
    }
}
impl Drop for Links {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn network_link_presence_is_not_loopback_presence_and_unknown_never_becomes_absence() {
    let links = Links::new();
    links.link("renamed-loopback", "772", "up");
    links.link("ethernet", "1", "down");
    assert!(!linux_link_state(&links.0).unwrap());
    std::fs::write(links.0.join("ethernet/operstate"), "up").unwrap();
    assert!(linux_link_state(&links.0).unwrap());
    links.link("virtual", "1", "unknown");
    assert!(linux_link_state(&links.0).is_err());
    std::fs::remove_file(links.0.join("virtual/operstate")).unwrap();
    assert!(linux_link_state(&links.0).is_err());
}

#[test]
fn network_file_and_link_count_limits_fail_closed() {
    let links = Links::new();
    links.link("oversized", "1", &"u".repeat(65));
    assert!(linux_link_state(&links.0).is_err());
    std::fs::write(links.0.join("oversized/operstate"), "down").unwrap();
    for i in 0..64 {
        links.link(&format!("extra-{i}"), "1", "down");
    }
    assert_eq!(
        linux_link_state(&links.0).unwrap_err().0,
        "network-link-count-limit"
    );
    assert!(linux_link_state(&links.0.join("missing")).is_err());
}
